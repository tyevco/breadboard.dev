// Breadboard nRF52840 POWER (nRF52840 PS v1.1 section 5.3) on the shared
// POWER/CLOCK base, for Renode 1.17.0 only (ADR-009 D6, Phase 2 milestone P4).
//
// Renode 1.17.0's NRF_CLOCK sits at 0x40000000, the base the nRF52840's POWER
// and CLOCK share, and has no POFCON, EVENTS_POFWARN, RESETREAS or GPREGRET
// (ADR-009 Context, risk R4). NRF_CLOCK is sealed, so BbNrf52Power WRAPS it:
// it owns a private NRF_CLOCK (not on the bus) and passes it every access it
// does not decode itself, so every CLOCK register behaves exactly as on the
// stock platform. The shared POWER_CLOCK interrupt (IRQ 0) is the OR of the
// wrapped clock's line and POFWARN's. Decoded here:
//
//   0x078 TASKS_CONSTLAT, 0x07C TASKS_LOWPWR   no-ops (no timing or current)
//   0x108 EVENTS_POFWARN                       set by the comparator (below)
//   0x304 INTENSET / 0x308 INTENCLR, bit 2      POFWARN's enable; the write is
//                                               also passed on (CLOCK's bits)
//   0x400 RESETREAS                            0 after power-on; write 1 to clear
//   0x510 POFCON                               POF (bit 0), THRESHOLD (4..1),
//                                               THRESHOLDVDDH (11..8)
//   0x51C GPREGRET, 0x520 GPREGRET2            8 bits each
//
// The power-fail comparator is NOT here: it is the package's `pi` view
// (nordic/nrf52840-qiaa, view `pof`, kind pof_comparator) in the analog
// engine, which compares V(VDD) with the threshold the CPU engine reports as
// the internal outputs #pofen and #pofthr0..3 (glue rule 13 of
// spec/pin-model.md) and returns its output as the watched digital_in
// #pofwarn: this model's GPIO input 0 (an External Control SET_STATE while
// Renode is paused at ceil_L of the crossing, ADR-003 D2). It is an
// interrupt source like EXTI, not a sampled input (ADR-005 D3).
//
// The rules (ADR-009 D6):
//   - a rising #pofwarn with POFCON.POF = 1 raises EVENTS_POFWARN at once;
//   - "already below at enable": a POFCON write that sets POF (0 -> 1), or
//     changes THRESHOLD while POF = 1, with #pofwarn already high raises it at
//     the next lattice point, ceil_L of the write's time (L the global
//     quantum: the grant lattice), through a one-shot LimitTimer on the
//     machine's clock source, so the instant is a clock limit the lookahead
//     promise sees; a POFCON write that clears POF first cancels it. A write
//     that changes neither (nrfx writes POFCON twice per call: the second is
//     nrf_power_pofcon_vddh_set's read-modify-write) is not a
//     reconfiguration;
//   - a THRESHOLD code that does not exist in normal-voltage mode (0-3: the
//     MDK's "should not be used"; V17..V28 are 4..15) with POF = 1, or a
//     THRESHOLDVDDH other than its reset value 0 (the high-voltage mode is
//     not modelled), is REFUSED: the write does not take effect, the model
//     logs it at error level and reports it in the event ring (below), and
//     the Renode engine fails the run naming the write;
//   - POF blocks NVMC erases while POF = 1 and #pofwarn is high
//     (BbNrf52Nvmc asks BlocksNvmc; it refuses the erase and logs it).
//
// Reporting (the state block bb_nrf52_state.cs, armed with its pof bit by a
// run that binds #pofen/#pofthr*): PofChange(t_fs, k, level) for #pofen
// (k = 0) and #pofthr0..3 (k = 1..4) at the write's virtual time as Renode's
// peripherals see it (cpu.SyncTime(), as #hfxo); PofEvent(t_fs, kind, value)
// for each EVENTS_POFWARN raise (kind 0, value POFCON), each refused write
// (kind 1, value the word written) and each blocked erase (kind 2, value the
// NVMC offset). Every raise is also counted (host register 0xF08).
//
// Host registers (External Control only; a firmware access is ignored with
// a warning, error bit 1):
//   0xF00 ID 0x57504242 ("BBPW")     0xF04 RESETREAS preset (write: set)
//   0xF08 EVENTS_POFWARN raises       0xF0C error word (bit 0 a refused
//   0xF10 the #pofwarn input level          write, bit 1 firmware access)
//   0xF14 POFCON as applied           0xF18 the first refused word, 0xF1C/0xF20
//                                           its time in ns (low, high): the Renode
//                                           engine fails the run with them at its end
//                                           when no state block reported the refusal
using System;
using System.Collections.Generic;
using System.Linq;
using Antmicro.Renode.Core;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.Bus;
using Antmicro.Renode.Peripherals.CPU;
using Antmicro.Renode.Peripherals.Timers;
using Antmicro.Renode.Time;

namespace Antmicro.Renode.Peripherals.Miscellaneous
{
    public class BbNrf52Power : IDoubleWordPeripheral, IKnownSize, IGPIOReceiver, INumberedGPIOOutput
    {
        public BbNrf52Power(IMachine machine)
        {
            this.machine = machine;
            clock = (BaseClockSource)machine.ClockSource;
            IRQ = new GPIO();
            input = new GPIO();
            Connections = new Dictionary<int, IGPIO> { { 0, input } };
            inner = new NRF_CLOCK(machine);
            inner.IRQ.Connect(this, ClockIrqLine);
            timer = new LimitTimer(machine.ClockSource, 1000000000, this, "bbPofEnable", limit: ulong.MaxValue,
                direction: Direction.Ascending, enabled: false, workMode: WorkMode.OneShot, eventEnabled: true);
            timer.LimitReached += () =>
            {
                timer.Enabled = false;
                Raise(Now(), "enabled with #pofwarn high");
            };
            Reset();
        }

        public long Size => 0x1000;

        public GPIO IRQ { get; }

        // External Control GET_STATE reads input 0 here (the #pofwarn level).
        public IReadOnlyDictionary<int, IGPIO> Connections { get; }

        // ADR-009 D6 reporting (bb_nrf52_state.cs; unset unless armed).
        public Action<ulong, int, bool> PofChange { get; set; }
        public Action<ulong, int, uint> PofEvent { get; set; }

        public uint Raises { get; private set; }

        // POF blocks NVMC erases (ADR-009 D6): asked by BbNrf52Nvmc.
        public bool BlocksNvmc => (pofcon & 1) != 0 && input.IsSet;

        public void Reset()
        {
            inner.Reset();
            timer.Enabled = false;
            pofcon = 0;
            evPofwarn = false;
            intenPofwarn = false;
            gpregret = 0;
            gpregret2 = 0;
            clockIrq = false;
            UpdateIrq();
            // RESETREAS, the host registers and the input are not reset by a
            // machine reset (P5 decides what a brown-out does, ADR-009 D7).
        }

        public void OnGPIO(int number, bool value)
        {
            if(number == ClockIrqLine)
            {
                clockIrq = value;
                UpdateIrq();
                return;
            }
            if(number != 0)
            {
                this.Log(LogLevel.Warning, "bb_nrf52_power: GPIO input {0} does not exist (0 is #pofwarn)", number);
                return;
            }
            var rising = value && !input.IsSet;
            input.Set(value);
            if(rising && (pofcon & 1) != 0)
            {
                Raise(Now(), "#pofwarn rose");
            }
        }

        public uint ReadDoubleWord(long offset)
        {
            if(offset >= HostBase)
            {
                if(FromFirmware())
                {
                    ReportFirmwareAccess("read", offset);
                    return 0;
                }
                switch(offset)
                {
                case 0xF00: return 0x57504242;
                case 0xF04: return resetreas;
                case 0xF08: return Raises;
                case 0xF0C: return error;
                case 0xF10: return input.IsSet ? 1u : 0u;
                case 0xF14: return pofcon;
                case 0xF18: return refusedWord;
                case 0xF1C: return (uint)refusedNs;
                case 0xF20: return (uint)(refusedNs >> 32);
                default: return 0;
                }
            }
            switch(offset)
            {
            case EventsPofwarn:
                return evPofwarn ? 1u : 0u;
            case IntenSet:
            case IntenClr:
                return (inner.ReadDoubleWord(offset) & ~IntPofwarn) | (intenPofwarn ? IntPofwarn : 0u);
            case Resetreas:
                return resetreas;
            case Pofcon:
                return pofcon;
            case Gpregret:
                return gpregret;
            case Gpregret2:
                return gpregret2;
            default:
                return inner.ReadDoubleWord(offset);
            }
        }

        public void WriteDoubleWord(long offset, uint value)
        {
            if(offset >= HostBase)
            {
                if(FromFirmware())
                {
                    ReportFirmwareAccess("write", offset);
                    return;
                }
                switch(offset)
                {
                case 0xF04: resetreas = value; return;
                case 0xF0C: error = 0; return;
                default: return;
                }
            }
            switch(offset)
            {
            case TasksConstlat:
            case TasksLowpwr:
                return;
            case EventsPofwarn:
                evPofwarn = (value & 1) != 0;
                UpdateIrq();
                return;
            case IntenSet:
                if((value & IntPofwarn) != 0)
                {
                    intenPofwarn = true;
                    UpdateIrq();
                }
                inner.WriteDoubleWord(offset, value & ~IntPofwarn);
                return;
            case IntenClr:
                if((value & IntPofwarn) != 0)
                {
                    intenPofwarn = false;
                    UpdateIrq();
                }
                inner.WriteDoubleWord(offset, value & ~IntPofwarn);
                return;
            case Resetreas:
                resetreas &= ~value;
                return;
            case Pofcon:
                WritePofcon(value);
                return;
            case Gpregret:
                gpregret = value & 0xFF;
                return;
            case Gpregret2:
                gpregret2 = value & 0xFF;
                return;
            default:
                inner.WriteDoubleWord(offset, value);
                return;
            }
        }

        private void WritePofcon(uint value)
        {
            SyncCpu();
            var tFs = Now();
            var pof = (value & 1) != 0;
            var thr = (value >> 1) & 0xF;
            var vddh = (value >> 8) & 0xF;
            if((pof && thr < 4) || vddh != 0)
            {
                var why = vddh != 0
                    ? $"THRESHOLDVDDH {vddh}: the high-voltage mode's threshold is not modelled (only its reset value 0 is accepted)"
                    : $"THRESHOLD code {thr} does not exist in normal-voltage mode (V17..V28 are codes 4..15)";
                this.Log(LogLevel.Error, "bb_nrf52_power: POFCON write 0x{0:X8} refused at {1} fs: {2} (ADR-009 D6)", value, tFs, why);
                if((error & ErrRefused) == 0)
                {
                    refusedWord = value;
                    refusedNs = tFs / FsPerNs;
                }
                error |= ErrRefused;
                PofEvent?.Invoke(tFs, EventRefused, value);
                return;
            }
            var old = pofcon;
            pofcon = value & PofconMask;
            var oldPof = (old & 1) != 0;
            var oldThr = (old >> 1) & 0xF;
            if(oldPof != pof)
            {
                PofChange?.Invoke(tFs, 0, pof);
            }
            for(var k = 0; k < 4; k++)
            {
                var a = ((oldThr >> k) & 1) != 0;
                var b = ((thr >> k) & 1) != 0;
                if(a != b)
                {
                    PofChange?.Invoke(tFs, 1 + k, b);
                }
            }
            if(!pof)
            {
                // Disabled before the next lattice point: no comparator, no event.
                timer.Enabled = false;
                return;
            }
            if(input.IsSet && (!oldPof || oldThr != thr))
            {
                // Already below at enable or at reconfiguration (ADR-009 D6):
                // at the next lattice point.
                // The clock counts whole ns, so tFs is a whole ns.
                var lNs = (ulong)machine.LocalTimeSource.Quantum.TotalNanoseconds;
                var tNs = tFs / FsPerNs;
                var nextNs = lNs == 0 ? tNs : (tNs + lNs - 1) / lNs * lNs;
                if(nextNs == tNs)
                {
                    Raise(tFs, "enabled with #pofwarn high");
                }
                else
                {
                    timer.Enabled = false;
                    timer.Value = 0;
                    timer.Limit = nextNs - tNs;
                    timer.Enabled = true;
                }
            }
        }

        private void Raise(ulong tFs, string why)
        {
            evPofwarn = true;
            Raises++;
            this.Log(LogLevel.Info, "bb_nrf52_power: EVENTS_POFWARN at {0} fs ({1}; POFCON 0x{2:X})", tFs, why, pofcon);
            PofEvent?.Invoke(tFs, EventPofwarn, pofcon);
            UpdateIrq();
        }

        // An NVMC erase refused while POF blocks it (BbNrf52Nvmc).
        public void ReportNvmcBlocked(long nvmcOffset)
        {
            var tFs = Now();
            this.Log(LogLevel.Warning, "bb_nrf52_power: NVMC erase (offset 0x{0:X}) blocked by POF at {1} fs (ADR-009 D6)", nvmcOffset, tFs);
            PofEvent?.Invoke(tFs, EventNvmcBlocked, (uint)nvmcOffset);
        }

        private void UpdateIrq()
        {
            IRQ.Set(clockIrq || (evPofwarn && intenPofwarn));
        }

        private ulong Now()
        {
            return clock.CurrentValue.TotalNanoseconds * FsPerNs;
        }

        private void SyncCpu()
        {
            if(machine.SystemBus.TryGetCurrentCPU(out var cpu))
            {
                cpu.SyncTime();
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
            if((error & ErrFirmware) == 0)
            {
                this.Log(LogLevel.Warning, "bb_nrf52_power: firmware {0} at offset 0x{1:X} ignored (a host register of the Breadboard engine)", what, offset);
            }
            error |= ErrFirmware;
        }

        public const int EventPofwarn = 0, EventRefused = 1, EventNvmcBlocked = 2;
        public const uint ErrRefused = 1u << 0, ErrFirmware = 1u << 1;

        private const int ClockIrqLine = 1;
        private const long TasksConstlat = 0x078, TasksLowpwr = 0x07C, EventsPofwarn = 0x108;
        private const long IntenSet = 0x304, IntenClr = 0x308, Resetreas = 0x400, Pofcon = 0x510;
        private const long Gpregret = 0x51C, Gpregret2 = 0x520, HostBase = 0xF00;
        private const uint IntPofwarn = 1u << 2, PofconMask = 0xF1F;
        private const ulong FsPerNs = 1000000;

        private readonly IMachine machine;
        private readonly BaseClockSource clock;
        private readonly NRF_CLOCK inner;
        private readonly GPIO input;
        private readonly LimitTimer timer;
        private uint pofcon;
        private bool evPofwarn;
        private bool intenPofwarn;
        private bool clockIrq;
        private uint resetreas;
        private uint gpregret, gpregret2;
        private uint error;
        private uint refusedWord;
        private ulong refusedNs;
    }
}
