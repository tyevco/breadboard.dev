// Breadboard CPU lookahead block (ADR-003 D5), for Renode 1.17.0 only.
//
// A read-only register block that the Breadboard Renode engine maps at an
// unused address (default 0x5FFF0000) and reads with one External Control
// SYSTEM_BUS read of 8 dwords per Query. It reports the values Renode's own
// WFI fast-forward uses (BaseCPU.cs: InstructionsToNearestLimit and the
// WaitingForInterrupt branch), so the engine can promise that a sleeping CPU
// stays quiet until the clock source's nearest limit:
//
//   - in WFI: tracked with TranslationCPU.AddHookAtWfiStateChange. The NVIC
//     already registers a hook of its own (NVIC.cs, HandleWfiStateChange),
//     so the tlib hook flag is set in every Cortex-M run with or without
//     this block, and adding a second delegate changes nothing in tlib;
//   - NVIC pending: the private set NVIC.pendingIRQs, read by reflection.
//     Never FindPendingInterrupt(), which has side effects;
//   - queued synced-state actions: TimeSourceBase.delayedActions of the
//     machine's local time source and of the master time source;
//   - TranslationCPU.IsHalted;
//   - BaseClockSource.NearestLimitIn (SysTick, every LimitTimer including
//     STM32_Timer compare channels, ScheduleAction) and the clock-source time.
//
// Reading the block never changes emulation state: it reads fields, and the
// only state it keeps is its own (the WFI flag, the WFI exit counter and the
// first-exit latch below). Firmware accesses (a bus transaction with an
// initiator, or from a CPU thread) are ignored and logged: reads return 0
// and writes are dropped, so firmware cannot see or disturb the block.
//
// Registers (dword offsets; a read of STATUS takes the snapshot that the
// other registers return, and resets the first-exit latch):
//
//   0x00 STATUS
//        bit 0  armed (Arm() ran)
//        bit 1  self-test passed (see Arm)
//        bit 2  the CPU is in WFI
//        bit 3  the NVIC has a pending exception (pendingIRQs not empty)
//        bit 4  a synced-state action is queued (local or master source)
//        bit 5  the CPU is halted
//        bit 6  a clock limit exists (NearestLimitIn is finite)
//        bit 7  firmware accessed the block (sticky)
//        bit 8  a WFI exit happened since the previous snapshot
//        bits 16..23 self-test failure reasons:
//               16 not exactly one machine, 17 not exactly one CPU or not a
//               TranslationCPU, 18 not exactly one NVIC, 19 NVIC.pendingIRQs
//               missing, 20 TimeSourceBase.delayedActions missing or a time
//               source is not a TimeSourceBase, 21 a host backend (an
//               external other than the host machine, or a host machine
//               element: UART terminals, sockets, TAP, ...) is attached
//   0x04 / 0x08  NearestLimitIn in ns, low / high (all ones: no limit)
//   0x0C / 0x10  clock-source time in ns, low / high
//   0x14         number of WFI exits since Arm (wraps)
//   0x18 / 0x1C  clock-source time of the first WFI exit since the previous
//                snapshot, in ns, low / high (all ones: none)
//
// Arm() is called once from the generated monitor script
// (`sysbus.<name> Arm`) after the platform, the firmware and any extra
// monitor lines, and before the External Control server is created.
using System;
using System.Collections;
using System.Linq;
using System.Reflection;
using Antmicro.Renode.Core;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.Bus;
using Antmicro.Renode.Peripherals.CPU;
using Antmicro.Renode.Peripherals.IRQControllers;
using Antmicro.Renode.Time;

namespace Antmicro.Renode.Peripherals.Miscellaneous
{
    public class BbLookahead : IDoubleWordPeripheral, IKnownSize
    {
        public BbLookahead(IMachine machine)
        {
            this.machine = machine;
            snap = new uint[8];
        }

        public long Size => 0x20;

        public void Reset()
        {
            // The block has no firmware-visible state; arming survives a reset.
        }

        public void Arm()
        {
            lock(sync)
            {
                if(armed)
                {
                    return;
                }
                uint fail = 0;
                var emulation = EmulationManager.Instance.CurrentEmulation;
                if(emulation.MachinesCount != 1)
                {
                    fail |= 1u << 16;
                }
                var cpus = machine.SystemBus.GetCPUs().ToList();
                cpu = cpus.Count == 1 ? cpus[0] as TranslationCPU : null;
                if(cpu == null)
                {
                    fail |= 1u << 17;
                }
                var nvics = machine.GetPeripheralsOfType<NVIC>().ToList();
                nvic = nvics.Count == 1 ? nvics[0] : null;
                if(nvic == null)
                {
                    fail |= 1u << 18;
                }
                nvicPending = typeof(NVIC).GetField("pendingIRQs", BindingFlags.NonPublic | BindingFlags.Instance);
                if(nvicPending == null || !typeof(ICollection).IsAssignableFrom(nvicPending.FieldType)
                   && !nvicPending.FieldType.GetInterfaces().Any(i => i.IsGenericType && i.GetGenericTypeDefinition() == typeof(System.Collections.Generic.ICollection<>)))
                {
                    fail |= 1u << 19;
                }
                delayed = typeof(TimeSourceBase).GetField("delayedActions", BindingFlags.NonPublic | BindingFlags.Instance);
                localSource = machine.LocalTimeSource as TimeSourceBase;
                masterSource = emulation.MasterTimeSource as TimeSourceBase;
                if(delayed == null || !typeof(ICollection).IsAssignableFrom(delayed.FieldType) || localSource == null || masterSource == null)
                {
                    fail |= 1u << 20;
                }
                var externals = emulation.ExternalsManager.GetNames().ToList();
                var host = emulation.HostMachine;
                var hostName = HostMachine.HostMachineName;
                if(externals.Any(n => n != hostName) || (host != null && host.GetNames().Any()))
                {
                    fail |= 1u << 21;
                }
                selfTestFailures = fail;
                if(cpu != null)
                {
                    cpu.AddHookAtWfiStateChange(OnWfiStateChange);
                }
                armed = true;
                if(fail != 0)
                {
                    this.Log(LogLevel.Warning, "bb_lookahead: self-test failed (reasons 0x{0:X}); no quiet promises", fail >> 16);
                }
                else
                {
                    this.Log(LogLevel.Info, "bb_lookahead: armed");
                }
            }
        }

        public uint ReadDoubleWord(long offset)
        {
            if(FromFirmware())
            {
                ReportFirmwareAccess("read", offset);
                return 0;
            }
            lock(sync)
            {
                if(offset == 0)
                {
                    Snapshot();
                }
                var i = offset / 4;
                return (offset % 4 == 0 && i >= 0 && i < snap.Length) ? snap[i] : 0;
            }
        }

        public void WriteDoubleWord(long offset, uint value)
        {
            // Read-only: firmware writes are dropped and logged; the engine
            // never writes (arming is a monitor call).
            ReportFirmwareAccess("write", offset);
        }

        private bool FromFirmware()
        {
            if(machine.SystemBus.TryGetTransactionInitiator(out var initiator) && initiator != null)
            {
                return true;
            }
            return machine.SystemBus.TryGetCurrentCPU(out var _);
        }

        private void ReportFirmwareAccess(string what, long offset)
        {
            lock(sync)
            {
                if(!firmwareAccess)
                {
                    this.Log(LogLevel.Warning, "bb_lookahead: firmware {0} at offset 0x{1:X} ignored (the block is for the Breadboard engine only)", what, offset);
                }
                firmwareAccess = true;
            }
        }

        // Runs on the CPU thread inside tlib's WFI state change (under the
        // CPU's pause guard); it only updates this block's own fields.
        private void OnWfiStateChange(bool entered)
        {
            lock(sync)
            {
                inWfi = entered;
                if(entered)
                {
                    return;
                }
                wfiExits++;
                if(firstExitNs == ulong.MaxValue)
                {
                    firstExitNs = ClockNs();
                }
            }
        }

        private ulong ClockNs()
        {
            return ((BaseClockSource)machine.ClockSource).CurrentValue.TotalNanoseconds;
        }

        private static bool NonEmpty(FieldInfo f, object o)
        {
            var v = f?.GetValue(o);
            if(v is ICollection c)
            {
                return c.Count > 0;
            }
            if(v == null)
            {
                return false;
            }
            // ISet<int> (NVIC.pendingIRQs): a generic collection's Count.
            var count = v.GetType().GetProperty("Count");
            return count != null && Convert.ToInt32(count.GetValue(v)) > 0;
        }

        private void Snapshot()
        {
            uint s = 0;
            var limitNs = ulong.MaxValue;
            var clockNs = 0UL;
            if(armed)
            {
                s |= 1u << 0;
                if(selfTestFailures == 0)
                {
                    s |= 1u << 1;
                    if(inWfi)
                    {
                        s |= 1u << 2;
                    }
                    if(NonEmpty(nvicPending, nvic))
                    {
                        s |= 1u << 3;
                    }
                    if(NonEmpty(delayed, localSource) || NonEmpty(delayed, masterSource))
                    {
                        s |= 1u << 4;
                    }
                    if(cpu.IsHalted)
                    {
                        s |= 1u << 5;
                    }
                    var cs = (BaseClockSource)machine.ClockSource;
                    var lim = cs.NearestLimitIn;
                    if(lim.Ticks != ulong.MaxValue)
                    {
                        s |= 1u << 6;
                        limitNs = lim.TotalNanoseconds;
                    }
                    clockNs = cs.CurrentValue.TotalNanoseconds;
                }
                s |= selfTestFailures;
            }
            if(firmwareAccess)
            {
                s |= 1u << 7;
            }
            if(firstExitNs != ulong.MaxValue)
            {
                s |= 1u << 8;
            }
            snap[0] = s;
            snap[1] = (uint)limitNs;
            snap[2] = (uint)(limitNs >> 32);
            snap[3] = (uint)clockNs;
            snap[4] = (uint)(clockNs >> 32);
            snap[5] = wfiExits;
            snap[6] = (uint)firstExitNs;
            snap[7] = (uint)(firstExitNs >> 32);
            firstExitNs = ulong.MaxValue;
        }

        private readonly IMachine machine;
        private readonly object sync = new object();
        private readonly uint[] snap;
        private TranslationCPU cpu;
        private NVIC nvic;
        private FieldInfo nvicPending;
        private FieldInfo delayed;
        private TimeSourceBase localSource;
        private TimeSourceBase masterSource;
        private bool armed;
        private uint selfTestFailures;
        private bool inWfi;
        private bool firmwareAccess;
        private uint wfiExits;
        private ulong firstExitNs = ulong.MaxValue;
    }
}
