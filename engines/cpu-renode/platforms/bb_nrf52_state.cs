// Breadboard nRF52840 CPU state outputs (ADR-009 D4, Phase 2 milestone P3),
// for Renode 1.17.0 only.
//
// BbNrf52State records, at their exact virtual times, the changes of the CPU
// states a `supply_load` view draws its current from, in a ring that the
// Renode engine reports as the internal digital outputs `#run`, `#hfxo`,
// `#nvmc` and `#saadc` (glue rule 12 of spec/pin-model.md). The engine maps
// it at 0x5FFE0000 (`bbState`) only on a run that binds one of them, after
// the platform and the firmware, and arms it with the mask of the states it
// reports (`sysbus.bbState Arm <mask>`; bit 0 run, 1 hfxo, 2 nvmc, 3
// saadc). Unarmed (every run without state outputs) it hooks nothing.
//
// The WFI entry. Renode 1.17.0 calls the WFI hook
// (TranslationCPU.AddHookAtWfiStateChange) for an entry only when the CPU
// thread next runs, at the start of the next slice, with every clock already
// there (measured: the entry hook of a WFI at 1.007266 ms sees 1.010 ms on
// every clock; a WFI in the last slice of a RUN_FOR is hooked in the next
// RUN_FOR). While the CPU runs, its virtual time is its executed
// instructions at PerformanceInMips, and a WFI ends a translation block, so
// the executed instruction count after it is exact. The block keeps a
// reference (t_w, n_w): the clock time of the last wake (Arm: the reset, 0)
// and the executed instruction count there. A WFI entry is at
// t_w + (n - n_w) * 10^9 / MIPS fs, n the count that includes the WFI: the
// instant after the WFI instruction, exact to the fs. It is recorded at the
// first of: the entry hook; the wake's hook (an entry not hooked yet); the
// engine's SYNC read after every advance, which finds the CPU asleep when
// the clock is past the instant its executed instructions reach (a running
// CPU's executed instructions always fill the time since the reference).
//
//   - run   (state 0): 1 while the CPU executes, 0 while it sleeps in
//           WFI/WFE: the entry as above; the exit at the clock time of the
//           wake (the clock limit or the input sync point that woke it),
//           exact, which becomes the new reference. 1 after reset.
//   - hfxo  (state 1): CLOCK TASKS_HFCLKSTART (1) and TASKS_HFCLKSTOP (0)
//           writes (a write hook on the POWER/CLOCK peripheral: the stock
//           NRF_CLOCK, or since nrf52840-bb@4 BbNrf52Power, which wraps it), at the write's
//           virtual time as Renode's peripherals see it: the clock after
//           cpu.SyncTime(), as the SAADC and NVMC models time a write (the
//           executing block's start, on Renode's clock grid). The crystal's
//           start-up time is not modelled: the level follows the task
//           (ADR-009 D4).
//   - nvmc  (state 2): BbNrf52Nvmc busy (READY low: an erase), from the
//           model's own BusyChange, at the model's clock times.
//   - saadc (state 3): BbNrf52Saadc active (a buffer started, or a scan in
//           progress), from the model's own ActiveChange, at its clock times.
//   - pofen (state 4), pofthr0..3 (states 5-8): BbNrf52Power's POFCON.POF
//           and THRESHOLD bits (ADR-009 D6, Phase 2 P4), from its PofChange
//           at the POFCON write's time (after cpu.SyncTime()); armed by any
//           of bits 4-8, which also records the model's events (PofEvent) as
//           entries with state 0x10 + kind (0 an EVENTS_POFWARN raise, 1 a
//           refused POFCON write, 2 an NVMC erase blocked by POF) and the
//           value in the entry's fourth word.
//
// Registers (host only; a firmware access is ignored and sets error bit 3):
//   0x000 ID 0x54534242 ("BBST")       0x004 error word (bit 0 ring overflow,
//   0x008 ring entries recorded (total)  bit 1 setup failed, bit 3 firmware
//   0x00C ring entries acknowledged      access; write: clear)
//         (the host writes the count it has read)
//   0x010 the armed mask
//   0x020 SYNC: a read records a WFI entry not recorded yet (above), then
//         returns the entries recorded (total); 0x024 the error word again
//   0x1000 + 16 j  ring entry j = k mod 2048 of entry k: t (fs) low, t (fs)
//                  high, state | level << 8, value (an event's; else 0)
// GPIO output 0 toggles at the first entry after the last acknowledgement
// and at an error, so the engine (an External Control GPIO event) reads the
// ring at most once per advance and only when it has news; with #run bound
// it reads SYNC after every advance.
using System;
using System.Collections.Generic;
using System.Linq;
using Antmicro.Renode.Core;
using Antmicro.Renode.Core.Structure;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.Analog;
using Antmicro.Renode.Peripherals.Bus;
using Antmicro.Renode.Peripherals.CPU;
using Antmicro.Renode.Time;

namespace Antmicro.Renode.Peripherals.Miscellaneous
{
    public class BbNrf52State : IDoubleWordPeripheral, IKnownSize, INumberedGPIOOutput
    {
        public BbNrf52State(IMachine machine)
        {
            this.machine = machine;
            line = new GPIO();
            Connections = new Dictionary<int, IGPIO> { { 0, line } };
        }

        public long Size => 0x1000 + 16 * RingSize;

        public IReadOnlyDictionary<int, IGPIO> Connections { get; }

        public void Reset()
        {
            // Host-port state survives a machine reset (the engine owns it).
        }

        public void Arm(uint mask)
        {
            if(armed)
            {
                return;
            }
            armed = true;
            this.mask = mask & 0x1FF;
            clock = (BaseClockSource)machine.ClockSource;
            cpu = machine.SystemBus.GetCPUs().OfType<TranslationCPU>().FirstOrDefault();
            if(cpu == null)
            {
                Fail("no TranslationCPU");
                return;
            }
            var mips = (ulong)cpu.PerformanceInMips;
            if(mips == 0 || 1000000000ul % mips != 0)
            {
                Fail($"PerformanceInMips {mips} does not divide 10^9 fs: no exact instruction time");
                return;
            }
            fsPerInstruction = 1000000000ul / mips;
            refFs = clock.CurrentValue.TotalNanoseconds * 1000000ul;
            refInstructions = cpu.ExecutedInstructions;
            running = true;
            if((this.mask & (1u << StateRun)) != 0)
            {
                cpu.AddHookAtWfiStateChange(OnWfi);
            }
            if((this.mask & (1u << StateHfxo)) != 0)
            {
                IBusPeripheral clk = machine.GetPeripheralsOfType<BbNrf52Power>().FirstOrDefault();
                if(clk == null)
                {
                    clk = machine.GetPeripheralsOfType<NRF_CLOCK>().FirstOrDefault();
                }
                if(clk == null)
                {
                    Fail("no POWER/CLOCK peripheral for the #hfxo output");
                }
                else
                {
                    machine.SystemBus.SetHookBeforePeripheralWrite<uint>(clk, OnClockWrite, new Antmicro.Renode.Core.Range(0, 8));
                }
            }
            if((this.mask & (1u << StateNvmc)) != 0)
            {
                var nvmc = machine.GetPeripheralsOfType<BbNrf52Nvmc>().FirstOrDefault();
                if(nvmc == null)
                {
                    Fail("no BbNrf52Nvmc for the #nvmc output");
                }
                else
                {
                    nvmc.BusyChange = (t, level) => Record(t * 1000000ul, StateNvmc, level);
                }
            }
            if((this.mask & (1u << StateSaadc)) != 0)
            {
                var saadc = machine.GetPeripheralsOfType<BbNrf52Saadc>().FirstOrDefault();
                if(saadc == null)
                {
                    Fail("no BbNrf52Saadc for the #saadc output");
                }
                else
                {
                    saadc.ActiveChange = (t, level) => Record(t * 1000000ul, StateSaadc, level);
                }
            }
            if((this.mask & PofMask) != 0)
            {
                var power = machine.GetPeripheralsOfType<BbNrf52Power>().FirstOrDefault();
                if(power == null)
                {
                    Fail("no BbNrf52Power for the #pofen/#pofthr outputs");
                }
                else
                {
                    power.PofChange = (t, k, level) => Record(t, StatePofen + k, level);
                    power.PofEvent = (t, kind, value) => Record(t, StateEvent + kind, true, value);
                }
            }
            this.Log(LogLevel.Info, "bb_nrf52_state: armed, mask 0x{0:X}", this.mask);
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
                if(offset >= 0x1000 && offset < 0x1000 + 16 * RingSize)
                {
                    var j = (offset - 0x1000) / 16;
                    switch((offset - 0x1000) % 16)
                    {
                    case 0: return (uint)ringT[j];
                    case 4: return (uint)(ringT[j] >> 32);
                    case 8: return ringS[j];
                    case 12: return ringV[j];
                    default: return 0;
                    }
                }
                switch(offset)
                {
                case 0x000: return 0x54534242;
                case 0x004: return error;
                case 0x008: return (uint)count;
                case 0x00C: return (uint)acked;
                case 0x010: return mask;
                case 0x020:
                    SyncEntry();
                    return (uint)count;
                case 0x024: return error;
                default: return 0;
                }
            }
        }

        public void WriteDoubleWord(long offset, uint value)
        {
            if(FromFirmware())
            {
                ReportFirmwareAccess("write", offset);
                return;
            }
            lock(sync)
            {
                switch(offset)
                {
                case 0x004:
                    error = 0;
                    return;
                case 0x00C:
                    acked = (count & ~0xFFFFFFFFul) | value;
                    if(acked > count)
                    {
                        acked -= 0x100000000ul;
                    }
                    notified = false;
                    if(count > acked)
                    {
                        Notify();
                    }
                    return;
                }
            }
        }

        // The WFI entry at executed instruction count n (see the header),
        // once per sleep.
        private void Entry(ulong n)
        {
            if(!running)
            {
                return;
            }
            running = false;
            Record(refFs + (n - refInstructions) * fsPerInstruction, StateRun, false);
        }

        // The engine's read after an advance (the CPU thread is paused): a
        // CPU whose executed instructions do not fill the time since the
        // reference is asleep since the instant they reach.
        private void SyncEntry()
        {
            if(!running || cpu == null || (mask & (1u << StateRun)) == 0)
            {
                return;
            }
            var n = cpu.ExecutedInstructions;
            var nowFs = clock.CurrentValue.TotalNanoseconds * 1000000ul;
            if(nowFs > refFs + (n - refInstructions) * fsPerInstruction)
            {
                Entry(n);
            }
        }

        // On the CPU thread inside tlib's WFI state change.
        private void OnWfi(bool entered)
        {
            lock(sync)
            {
                // The entry (late; or not hooked at all before the wake).
                Entry(cpu.ExecutedInstructions);
                if(entered)
                {
                    return;
                }
                // The wake: the clock is its exact time; the new reference.
                refFs = clock.CurrentValue.TotalNanoseconds * 1000000ul;
                refInstructions = cpu.ExecutedInstructions;
                running = true;
                Record(refFs, StateRun, true);
            }
        }

        // A CPU write to NRF_CLOCK's TASKS_HFCLKSTART (0x000) or
        // TASKS_HFCLKSTOP (0x004); the value is passed on unchanged.
        private uint OnClockWrite(uint value, long offset)
        {
            if((value & 1) != 0 && (offset == 0x000 || offset == 0x004))
            {
                cpu.SyncTime();
                Record(clock.CurrentValue.TotalNanoseconds * 1000000ul, StateHfxo, offset == 0x000);
            }
            return value;
        }

        private void Record(ulong tFs, int state, bool level, uint value = 0)
        {
            lock(sync)
            {
                ringT[count % RingSize] = tFs;
                ringS[count % RingSize] = (uint)state | (level ? 1u << 8 : 0u);
                ringV[count % RingSize] = value;
                count++;
                if(count - acked > RingSize && (error & ErrOverflow) == 0)
                {
                    error |= ErrOverflow;
                    this.Log(LogLevel.Warning, "bb_nrf52_state: ring overflow at {0} fs (more than {1} entries unread)", tFs, RingSize);
                    line.Set(!line.IsSet);
                }
                Notify();
            }
        }

        private void Fail(string what)
        {
            this.Log(LogLevel.Error, "bb_nrf52_state: {0}", what);
            lock(sync)
            {
                error |= ErrSetup;
                line.Set(!line.IsSet);
            }
        }

        private void Notify()
        {
            if(!notified)
            {
                notified = true;
                line.Set(!line.IsSet);
            }
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
                if((error & ErrFirmware) == 0)
                {
                    this.Log(LogLevel.Warning, "bb_nrf52_state: firmware {0} at offset 0x{1:X} ignored (the block is for the Breadboard engine only)", what, offset);
                    line.Set(!line.IsSet);
                }
                error |= ErrFirmware;
            }
        }

        public const int RingSize = 2048;
        public const int StateRun = 0, StateHfxo = 1, StateNvmc = 2, StateSaadc = 3, StatePofen = 4;
        public const int StateEvent = 0x10;
        public const uint PofMask = 0x1F0;
        public const uint ErrOverflow = 1u << 0, ErrSetup = 1u << 1, ErrFirmware = 1u << 3;

        private readonly IMachine machine;
        private readonly GPIO line;
        private readonly object sync = new object();
        private readonly ulong[] ringT = new ulong[RingSize];
        private readonly uint[] ringS = new uint[RingSize];
        private readonly uint[] ringV = new uint[RingSize];
        private BaseClockSource clock;
        private TranslationCPU cpu;
        private ulong fsPerInstruction;
        private ulong refFs;
        private ulong refInstructions;
        private bool running;
        private bool armed;
        private uint mask;
        private uint error;
        private ulong count, acked;
        private bool notified;
    }
}
