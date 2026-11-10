// Breadboard nRF52840 SAADC (nRF52840 PS v1.1 section 6.23) with its host
// port, and a UART line tap (ADR-008 D7, D8), for Renode 1.17.0 only.
//
// BbNrf52Saadc is the SAADC that Renode 1.17.0 lacks, in ADR-005's style
// (bb_stm32f0_adc.cs): timed inputs from a host port, every deferred action
// a deadline of one one-shot LimitTimer at 1 GHz on the machine's clock
// source (one ClockEntry, armed for the earliest pending event), so each
// event instant is a clock limit that the lookahead promise sees, and the
// model never queues a synced action, starts a thread or reads host time.
//
// Timing (ADR-008 D7, approver decision A5):
//   - acquisition k of a SAMPLE starts at a_k (a_0 = the SAMPLE task: start
//     latency 0; a CPU write is timed instruction-accurately after
//     cpu.SyncTime()), its sample instant is s_k = a_k + TACQ (3, 5, 10, 15,
//     20, 40 us), and DONE comes at s_k + tCONV, tCONV = 2 us;
//   - BURST (CH[n].CONFIG.BURST, which nrfx sets whenever it oversamples):
//     one SAMPLE runs 2^OVERSAMPLE acquisitions back to back,
//     a_{k+1} = s_k + tCONV, with no CPU action; without BURST each
//     acquisition needs its own SAMPLE and the result follows the
//     2^OVERSAMPLE-th;
//   - scan mode: one SAMPLE converts every enabled channel (PSELP != NC) in
//     channel order, back to back;
//   - SAMPLERATE.MODE = Timers: after SAMPLE, a scan starts every
//     CC / 16 MHz (a_k = a_0 + ceil(k * CC * 62.5 ns), never accumulated)
//     until STOP.
// Transfer: RESULT = (V_P - V_N) * GAIN / REF * 2^(RES - m), m = 1
// differential, 0 single-ended, rounded half up at the configured
// resolution (approver decision A5), saturated (single-ended
// [0, 2^RES - 1], differential [-2^(RES-1), 2^(RES-1) - 1]); the
// oversampled result is the mean of the 2^OVERSAMPLE codes, rounded half up
// (the PS gives no rounding rule for either). REF is the internal 0.6 V or
// VDD/4 with VDD read at the sample instant (input 8, a second sampled
// input). Results are written by EasyDMA as 16-bit words to RESULT.PTR
// (latched at START); END fires after RESULT.MAXCNT results, and STOP while
// started ends the buffer (END, then STOPPED; nrfx_saadc.c: "the STOP task
// also triggers the END event"). LIMITH/LIMITL are set at the DONE that
// completes a result (result >= HIGH, <= LOW).
//
// Inputs (host-port indices): 0-7 AIN0-AIN7 (P0.02-P0.05, P0.28-P0.31),
// 8 VDD (PSELP = VDD, and REFSEL = VDD1_4), 9 VDDH (PSELP = VDDHDIV5 reads
// VDDH / 5). A sample at s reads the latest timed input of its input with
// t < s (ADR-005 D5.2), else the constructor default (AIN 0 V, VDD
// vddMicrovolts, VDDH vddhMicrovolts).
//
// The loading adc_in (ADR-008 D7): for every input in the host-configured
// loaded mask the model records each acquisition start a_k in a ring the
// Renode engine turns into the `#smp` toggle (one edge per acquisition), and
// refuses (error word bit 5, the run fails) an acquisition with
// TACQ < 2 L, L the lattice the host writes at startup. For every input in
// the trace mask it records each sample (s_k, input, uV, code) for the
// per-sample `log` records.
//
// Not modelled (a warning once each): offset calibration (CALIBRATEOFFSET
// completes after calibrationNs with no effect on results), the input
// resistor ladders RESP/RESN, gain and offset errors, the sample-and-hold
// (the loading view does that, in the analog engine), PPI event outputs
// (SAMPLE through PPI needs a Breadboard PPI first, ADR-008 D3), oversampling
// with more than one channel enabled (each channel averages its own
// samples; the PS calls the result undefined).
using System;
using System.Collections.Generic;
using Antmicro.Renode.Core;
using Antmicro.Renode.Core.Structure;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.Bus;
using Antmicro.Renode.Peripherals.Sensor;
using Antmicro.Renode.Peripherals.Timers;
using Antmicro.Renode.Peripherals.UART;
using Antmicro.Renode.Time;
using Antmicro.Renode.Utilities.RESD;

namespace Antmicro.Renode.Peripherals.Analog
{
    [AllowedTranslations(AllowedTranslation.ByteToDoubleWord | AllowedTranslation.WordToDoubleWord)]
    public class BbNrf52Saadc : IDoubleWordPeripheral, IKnownSize, IADC
    {
        public BbNrf52Saadc(IMachine machine, uint vddMicrovolts = 3300000, uint vddhMicrovolts = 3300000,
            ulong calibrationNs = 2000)
        {
            this.machine = machine;
            sysbus = machine.GetSystemBus(this);
            clock = (BaseClockSource)machine.ClockSource;
            defaults = new uint[InputCount];
            defaults[VddInput] = vddMicrovolts;
            defaults[VddhInput] = vddhMicrovolts;
            calNs = calibrationNs;
            ADCContainer = new SimpleContainerHelper<IRESDSampleSource<VoltageSample>>(machine, this);
            IRQ = new GPIO();
            inputs = new List<TimedInput>[InputCount];
            sampledAt = new ulong[InputCount];
            hasSampled = new bool[InputCount];
            for(var i = 0; i < InputCount; i++)
            {
                inputs[i] = new List<TimedInput>();
            }
            timer = new LimitTimer(machine.ClockSource, 1000000000, this, "bbSaadcEvents", limit: ulong.MaxValue,
                direction: Direction.Ascending, enabled: false, workMode: WorkMode.OneShot, eventEnabled: true);
            timer.LimitReached += OnTimer;
            Stats = new BbSaadcStats();
            Reset();
            this.RegisterDefaultChildren(machine);
        }

        public long Size => 0x1000;

        public GPIO IRQ { get; }

        public int ADCChannelCount => InputCount;

        public SimpleContainerHelper<IRESDSampleSource<VoltageSample>> ADCContainer { get; }

        public BbSaadcStats Stats { get; private set; }

        // Host-port configuration (ADR-008 D7), kept across a machine reset.
        public ulong LatticeNs { get; set; }
        public uint LoadedMask { get; set; }
        public uint TraceMask { get; set; }

        public void Reset()
        {
            timer.Enabled = false;
            events.Clear();
            ev = 0;
            inten = 0;
            enable = false;
            for(var n = 0; n < Channels; n++)
            {
                pselp[n] = 0;
                pseln[n] = 0;
                config[n] = 0x00020000;
                limit[n] = 0x7FFF8000;
                accSum[n] = 0;
                accCount[n] = 0;
            }
            resolution = 1;
            oversample = 0;
            samplerate = 0;
            ptr = 0;
            maxcnt = 0;
            amount = 0;
            latchedPtr = 0;
            latchedMax = 0;
            started = false;
            busy = false;
            timerMode = false;
            scan = null;
            for(var i = 0; i < InputCount; i++)
            {
                inputs[i].Clear();
                sampledAt[i] = 0;
                hasSampled[i] = false;
            }
            Stats = new BbSaadcStats();
            IRQ.Unset();
        }

        // ---- bus interface ------------------------------------------------

        public uint ReadDoubleWord(long offset)
        {
            if(offset >= EventsStart && offset < EventsEnd)
            {
                return (ev >> (int)((offset - EventsStart) / 4)) & 1;
            }
            if(offset >= ChBase && offset < ChBase + 16 * Channels)
            {
                var n = (int)((offset - ChBase) / 16);
                switch((offset - ChBase) % 16)
                {
                case 0: return pselp[n];
                case 4: return pseln[n];
                case 8: return config[n];
                default: return limit[n];
                }
            }
            switch(offset)
            {
            case TaskStart:
            case TaskSample:
            case TaskStop:
            case TaskCalibrate:
                return 0;
            case IntenOffset:
            case IntenSetOffset:
            case IntenClrOffset:
                return inten;
            case StatusOffset:
                return busy ? 1u : 0u;
            case EnableOffset:
                return enable ? 1u : 0u;
            case ResolutionOffset:
                return resolution;
            case OversampleOffset:
                return oversample;
            case SamplerateOffset:
                return samplerate;
            case PtrOffset:
                return ptr;
            case MaxcntOffset:
                return maxcnt;
            case AmountOffset:
                return amount;
            default:
                WarnOnce("read", offset);
                return 0;
            }
        }

        public void WriteDoubleWord(long offset, uint value)
        {
            if(offset >= EventsStart && offset < EventsEnd)
            {
                var bit = 1u << (int)((offset - EventsStart) / 4);
                if((value & 1) == 0)
                {
                    ev &= ~bit;
                }
                else
                {
                    ev |= bit; // software can set an event, as on silicon
                }
                UpdateIrq();
                return;
            }
            if(offset >= ChBase && offset < ChBase + 16 * Channels)
            {
                var n = (int)((offset - ChBase) / 16);
                switch((offset - ChBase) % 16)
                {
                case 0:
                    pselp[n] = value & 0x1F;
                    break;
                case 4:
                    pseln[n] = value & 0x1F;
                    break;
                case 8:
                    config[n] = value & ConfigMask;
                    break;
                default:
                    limit[n] = value;
                    break;
                }
                return;
            }
            switch(offset)
            {
            case TaskStart:
                if((value & 1) != 0)
                {
                    TaskStartNow();
                }
                break;
            case TaskSample:
                if((value & 1) != 0)
                {
                    SyncCpu();
                    TaskSampleNow();
                }
                break;
            case TaskStop:
                if((value & 1) != 0)
                {
                    TaskStopNow();
                }
                break;
            case TaskCalibrate:
                if((value & 1) != 0 && Enabled("CALIBRATEOFFSET"))
                {
                    SyncCpu();
                    WarnOnceText("cal", "CALIBRATEOFFSET is not modelled: CALIBRATEDONE after a fixed time, results unchanged");
                    Schedule(new Event(Now() + calNs, EventKind.CalibrateDone, -1));
                }
                break;
            case IntenOffset:
                inten = value & EventMask;
                UpdateIrq();
                break;
            case IntenSetOffset:
                inten |= value & EventMask;
                UpdateIrq();
                break;
            case IntenClrOffset:
                inten &= ~(value & EventMask);
                UpdateIrq();
                break;
            case EnableOffset:
                enable = (value & 1) != 0;
                break;
            case ResolutionOffset:
                resolution = value & 7;
                if(resolution > 3)
                {
                    WarnOnceText("res", "RESOLUTION > 3 is reserved; treated as 14 bits");
                }
                break;
            case OversampleOffset:
                oversample = value & 0xF;
                if(oversample > 8)
                {
                    WarnOnceText("ovs", "OVERSAMPLE > 8 is reserved; treated as 256x");
                }
                break;
            case SamplerateOffset:
                samplerate = value & 0x17FF;
                break;
            case PtrOffset:
                ptr = value;
                break;
            case MaxcntOffset:
                maxcnt = value & 0x7FFF;
                break;
            default:
                WarnOnce("write", offset);
                break;
            }
        }

        // ---- host port interface (BbSaadcHostPort) --------------------------

        // Append a timed input value: from t_ns on, samples strictly after
        // t_ns read uv (ADR-005 D5.2). Returns an error code (0: ok).
        public uint AddTimedInput(int input, ulong tNs, uint uv)
        {
            if(input < 0 || input >= InputCount)
            {
                return Miscellaneous.BbSaadcHostPort.ErrChannel;
            }
            if(hasSampled[input] && tNs < sampledAt[input])
            {
                return Miscellaneous.BbSaadcHostPort.ErrLate;
            }
            var list = inputs[input];
            if(list.Count > 0)
            {
                var last = list[list.Count - 1];
                if(tNs < last.Ns)
                {
                    return Miscellaneous.BbSaadcHostPort.ErrOrder;
                }
                if(tNs == last.Ns)
                {
                    list[list.Count - 1] = new TimedInput(tNs, uv);
                    return 0;
                }
            }
            list.Add(new TimedInput(tNs, uv));
            return 0;
        }

        public uint LatestTimedInput(int input)
        {
            if(input < 0 || input >= InputCount || inputs[input].Count == 0)
            {
                return uint.MaxValue;
            }
            return inputs[input][inputs[input].Count - 1].Microvolts;
        }

        public ulong NowNs => Now();

        // The host port reports a refused acquisition (TACQ < 2 L on a loaded
        // input) through this.
        public Action<uint, ulong, uint, uint> ReportError;

        // The host port's notification line: a ring entry was recorded.
        public Action Notify;

        // ---- tasks --------------------------------------------------------

        private bool Enabled(string task)
        {
            if(!enable)
            {
                WarnOnceText("disabled-" + task, task + " with ENABLE = 0; ignored");
                Stats.TasksIgnored++;
            }
            return enable;
        }

        private void TaskStartNow()
        {
            if(!Enabled("START"))
            {
                return;
            }
            latchedPtr = ptr;
            latchedMax = maxcnt;
            amount = 0;
            started = true;
            if(latchedMax == 0)
            {
                WarnOnceText("maxcnt0", "START with RESULT.MAXCNT = 0: no result can be stored");
            }
            SetEvent(EvStarted);
        }

        private void TaskSampleNow()
        {
            if(!Enabled("SAMPLE"))
            {
                return;
            }
            if(busy)
            {
                // A scan is in progress (STATUS = Busy): the task is ignored.
                Stats.TasksIgnored++;
                WarnOnceText("busy", "SAMPLE while a conversion is in progress; ignored");
                return;
            }
            timerMode = (samplerate & SamplerateModeTimers) != 0;
            if(timerMode)
            {
                var cc = samplerate & 0x7FF;
                if(cc < 80)
                {
                    WarnOnceText("cc", $"SAMPLERATE.CC = {cc} is below the PS minimum of 80; used as is");
                }
                timerCc = Math.Max(cc, 1u);
                timerBase = Now();
                timerTicks = 0;
            }
            StartScan(Now());
        }

        private void TaskStopNow()
        {
            events.RemoveAll(e => e.Kind != EventKind.CalibrateDone);
            Rearm();
            busy = false;
            timerMode = false;
            scan = null;
            for(var n = 0; n < Channels; n++)
            {
                accSum[n] = 0;
                accCount[n] = 0;
            }
            if(started)
            {
                started = false;
                SetEvent(EvEnd);
            }
            SetEvent(EvStopped);
        }

        // ---- conversions --------------------------------------------------

        private void StartScan(ulong t)
        {
            var chans = new List<int>();
            for(var n = 0; n < Channels; n++)
            {
                if(pselp[n] != 0)
                {
                    chans.Add(n);
                }
            }
            if(chans.Count == 0)
            {
                WarnOnceText("nochan", "SAMPLE with no channel enabled (every PSELP = NC); nothing converted");
                return;
            }
            if(chans.Count > 1 && oversample != 0)
            {
                WarnOnceText("ovsmulti", "oversampling with more than one channel enabled: each channel averages its own samples (the PS calls the result undefined)");
            }
            if(timerMode && chans.Count > 1)
            {
                WarnOnceText("timermulti", "SAMPLERATE.MODE = Timers with more than one channel enabled (the PS allows one)");
            }
            busy = true;
            scan = chans;
            scanPos = 0;
            burstLeft = BurstCount(chans[0]);
            StartAcquisition(t);
        }

        private int Oversamples => 1 << (int)Math.Min(oversample, 8u);

        private int BurstCount(int n)
        {
            return (config[n] & ConfigBurst) != 0 ? Oversamples : 1;
        }

        private void StartAcquisition(ulong a)
        {
            var n = scan[scanPos];
            var input = InputOf(pselp[n]);
            var tacq = TacqNs(config[n]);
            Stats.Acquisitions++;
            if(input >= 0 && input < InputCount && (LoadedMask & (1u << input)) != 0)
            {
                Stats.RecordAcquisition(a, input, n);
                Notify?.Invoke();
                if(LatticeNs > 0 && tacq < 2 * LatticeNs)
                {
                    ReportError?.Invoke(Miscellaneous.BbSaadcHostPort.ErrTacq, a, (uint)input, (uint)tacq);
                }
            }
            Schedule(new Event(a + tacq, EventKind.Sample, n));
        }

        private void OnSample(int n)
        {
            var s = Now();
            var p = InputOf(pselp[n]);
            var vp = ReadInput(p, s);
            long v = vp;
            var diff = (config[n] & ConfigModeDiff) != 0;
            if(diff)
            {
                v -= ReadInput(InputOf(pseln[n]), s);
            }
            var refVdd = (config[n] & ConfigRefVdd) != 0;
            var vdd = refVdd ? ReadInputRaw(VddInput, s) : 0u;
            var code = Code(v, (int)((config[n] >> 8) & 7), refVdd, vdd, diff);
            accSum[n] += code;
            accCount[n]++;
            Stats.Samples++;
            if(p >= 0 && p < InputCount && (TraceMask & (1u << p)) != 0)
            {
                Stats.RecordSample(s, p, n, vp, code);
                Notify?.Invoke();
            }
            Schedule(new Event(s + TconvNs, EventKind.Done, n));
        }

        private void OnDone(int n)
        {
            var now = Now();
            SetEvent(EvDone);
            if(accCount[n] >= Oversamples)
            {
                var N = (long)accCount[n];
                var mean = FloorDiv(2 * accSum[n] + N, 2 * N);
                accSum[n] = 0;
                accCount[n] = 0;
                var high = (short)(limit[n] >> 16);
                var low = (short)(limit[n] & 0xFFFF);
                if(mean >= high)
                {
                    SetEvent(EvLimitH0 + 2 * n);
                }
                if(mean <= low)
                {
                    SetEvent(EvLimitH0 + 2 * n + 1);
                }
                Stats.Results++;
                SetEvent(EvResultDone);
                StoreResult((short)mean);
            }
            burstLeft--;
            if(burstLeft > 0)
            {
                StartAcquisition(now);
                return;
            }
            scanPos++;
            if(scanPos < scan.Count)
            {
                burstLeft = BurstCount(scan[scanPos]);
                StartAcquisition(now);
                return;
            }
            busy = false;
            scan = null;
            if(timerMode)
            {
                ScheduleTimerTick();
            }
        }

        private void ScheduleTimerTick()
        {
            // a_k = a_0 + ceil(k * CC * 62.5 ns), the next tick after now.
            var now = Now();
            ulong next;
            do
            {
                timerTicks++;
                next = timerBase + (timerTicks * timerCc * 125 + 1) / 2;
            }
            while(next <= now);
            Schedule(new Event(next, EventKind.TimerTick, -1));
        }

        private void StoreResult(short value)
        {
            if(!started)
            {
                Stats.ResultsDropped++;
                WarnOnceText("nobuf", "a result with no buffer started (START) is dropped");
                return;
            }
            if(amount < latchedMax)
            {
                sysbus.WriteWord(latchedPtr + 2 * (ulong)amount, (ushort)value);
                amount++;
                Stats.DmaWrites++;
            }
            if(amount >= latchedMax)
            {
                started = false;
                SetEvent(EvEnd);
            }
        }

        // round_half_up(v * G / REF * 2^(RES - m)), saturated.
        private long Code(long uv, int gain, bool refVdd, uint vddUv, bool diff)
        {
            var bits = ResBits[Math.Min(resolution, 3u)];
            var shift = bits - (diff ? 1 : 0);
            long num = uv * GainNum[gain] << shift;
            long den = GainDen[gain] * (long)InternalRefUv;
            if(refVdd)
            {
                // REF = VDD / 4: v * G * 4 / VDD.
                num *= 4;
                den = GainDen[gain] * (long)vddUv;
            }
            if(den <= 0)
            {
                WarnOnceText("vdd0", "REFSEL = VDD1_4 with VDD = 0 V: the result saturates");
                return uv > 0 ? (1L << shift) - 1 : 0;
            }
            var code = FloorDiv(2 * num + den, 2 * den);
            long lo = diff ? -(1L << (bits - 1)) : 0;
            long hi = diff ? (1L << (bits - 1)) - 1 : (1L << bits) - 1;
            return Math.Max(lo, Math.Min(hi, code));
        }

        private static long FloorDiv(long a, long b)
        {
            var q = a / b;
            if((a % b != 0) && ((a < 0) != (b < 0)))
            {
                q--;
            }
            return q;
        }

        // PSELP/PSELN -> input index: AIN0-7 -> 0-7, VDD -> 8, VDDHDIV5 -> 9.
        private static int InputOf(uint psel)
        {
            if(psel >= 1 && psel <= 8)
            {
                return (int)psel - 1;
            }
            if(psel == 9)
            {
                return VddInput;
            }
            if(psel == 0x0D)
            {
                return VddhDiv5;
            }
            return -1;
        }

        private uint ReadInput(int input, ulong s)
        {
            if(input == VddhDiv5)
            {
                // VDDH / 5, rounded half up.
                return (uint)((2UL * ReadInputRaw(VddhInput, s) + 5) / 10);
            }
            if(input < 0)
            {
                WarnOnceText("psel", "a reserved or unconnected PSEL is converted as 0 V");
                return 0;
            }
            return ReadInputRaw(input, s);
        }

        private uint ReadInputRaw(int input, ulong s)
        {
            uint uv;
            var list = inputs[input];
            var k = -1;
            for(var i = list.Count - 1; i >= 0; i--)
            {
                if(list[i].Ns < s)
                {
                    k = i;
                    break;
                }
            }
            if(k >= 0)
            {
                uv = list[k].Microvolts;
                if(k > 0)
                {
                    list.RemoveRange(0, k);
                }
            }
            else
            {
                uv = defaults[input];
                if(ADCContainer.TryGetByAddress(input, out var src) && src.Sample.Voltage != 0)
                {
                    uv = src.Sample.Voltage;
                }
            }
            sampledAt[input] = s;
            hasSampled[input] = true;
            return uv;
        }

        private static ulong TacqNs(uint config)
        {
            var t = (int)((config >> 16) & 7);
            return TacqTable[Math.Min(t, 5)];
        }

        // ---- events ---------------------------------------------------------

        private void SetEvent(int bit)
        {
            ev |= 1u << bit;
            UpdateIrq();
        }

        private void Schedule(Event e)
        {
            var i = events.FindIndex(x => x.At > e.At);
            if(i < 0)
            {
                events.Add(e);
            }
            else
            {
                events.Insert(i, e);
            }
            Rearm();
        }

        private void Rearm()
        {
            timer.Enabled = false;
            if(events.Count == 0)
            {
                return;
            }
            var now = Now();
            var at = events[0].At;
            if(at <= now)
            {
                if(at < now)
                {
                    this.Log(LogLevel.Error, "bb_nrf52_saadc: event at {0} ns scheduled at {1} ns; fired at the next ns", at, now);
                }
                at = now + 1;
                events[0].At = at;
            }
            timer.Value = 0;
            timer.Limit = at - now;
            timer.Enabled = true;
        }

        private void OnTimer()
        {
            inHandler++;
            try
            {
                var now = Now();
                while(events.Count > 0 && events[0].At <= now)
                {
                    var e = events[0];
                    events.RemoveAt(0);
                    switch(e.Kind)
                    {
                    case EventKind.Sample:
                        OnSample(e.Channel);
                        break;
                    case EventKind.Done:
                        OnDone(e.Channel);
                        break;
                    case EventKind.TimerTick:
                        if(timerMode && !busy)
                        {
                            StartScan(now);
                        }
                        break;
                    case EventKind.CalibrateDone:
                        SetEvent(EvCalibrateDone);
                        break;
                    }
                }
                Rearm();
            }
            finally
            {
                inHandler--;
            }
        }

        // ---- helpers --------------------------------------------------------

        private ulong Now()
        {
            return clock.CurrentValue.TotalNanoseconds;
        }

        private void SyncCpu()
        {
            // Only for a register access by the CPU itself, never from this
            // model's own clock event (already at its exact time).
            if(inHandler == 0 && sysbus.TryGetCurrentCPU(out var cpu))
            {
                cpu.SyncTime();
            }
        }

        private void UpdateIrq()
        {
            IRQ.Set((ev & inten & EventMask) != 0);
        }

        private void WarnOnce(string what, long offset)
        {
            WarnOnceText(what + offset, $"unhandled {what} at offset 0x{offset:X}");
        }

        private void WarnOnceText(string key, string text)
        {
            if(warned.Add(key))
            {
                this.Log(LogLevel.Warning, "bb_nrf52_saadc: {0}", text);
            }
        }

        private readonly IMachine machine;
        private readonly IBusController sysbus;
        private readonly BaseClockSource clock;
        private readonly LimitTimer timer;
        private readonly ulong calNs;
        private readonly uint[] defaults;
        private readonly List<Event> events = new List<Event>();
        private readonly List<TimedInput>[] inputs;
        private readonly ulong[] sampledAt;
        private readonly bool[] hasSampled;
        private readonly HashSet<string> warned = new HashSet<string>();
        private readonly uint[] pselp = new uint[Channels];
        private readonly uint[] pseln = new uint[Channels];
        private readonly uint[] config = new uint[Channels];
        private readonly uint[] limit = new uint[Channels];
        private readonly long[] accSum = new long[Channels];
        private readonly int[] accCount = new int[Channels];
        private int inHandler;

        private uint ev, inten, resolution, oversample, samplerate, ptr, maxcnt, amount, latchedPtr, latchedMax;
        private bool enable, started, busy, timerMode;
        private List<int> scan;
        private int scanPos, burstLeft;
        private ulong timerBase, timerTicks, timerCc;

        public const int Channels = 8;
        public const int InputCount = 10;
        public const int VddInput = 8;
        public const int VddhInput = 9;
        private const int VddhDiv5 = 100;
        private const ulong TconvNs = 2000;
        private const uint InternalRefUv = 600000;
        private static readonly ulong[] TacqTable = { 3000, 5000, 10000, 15000, 20000, 40000 };
        private static readonly int[] ResBits = { 8, 10, 12, 14 };
        private static readonly long[] GainNum = { 1, 1, 1, 1, 1, 1, 2, 4 };
        private static readonly long[] GainDen = { 6, 5, 4, 3, 2, 1, 1, 1 };

        private const long TaskStart = 0x000, TaskSample = 0x004, TaskStop = 0x008, TaskCalibrate = 0x00C,
            EventsStart = 0x100, EventsEnd = 0x158, IntenOffset = 0x300, IntenSetOffset = 0x304,
            IntenClrOffset = 0x308, StatusOffset = 0x400, EnableOffset = 0x500, ChBase = 0x510,
            ResolutionOffset = 0x5F0, OversampleOffset = 0x5F4, SamplerateOffset = 0x5F8, PtrOffset = 0x62C,
            MaxcntOffset = 0x630, AmountOffset = 0x634;
        private const int EvStarted = 0, EvEnd = 1, EvDone = 2, EvResultDone = 3, EvCalibrateDone = 4,
            EvStopped = 5, EvLimitH0 = 6;
        private const uint EventMask = 0x3FFFFF;
        private const uint ConfigBurst = 1u << 24, ConfigModeDiff = 1u << 20, ConfigRefVdd = 1u << 12,
            ConfigMask = 0x01171733;
        private const uint SamplerateModeTimers = 1u << 12;

        private struct TimedInput
        {
            public TimedInput(ulong ns, uint uv)
            {
                Ns = ns;
                Microvolts = uv;
            }

            public ulong Ns;
            public uint Microvolts;
        }

        private enum EventKind
        {
            Sample,
            Done,
            TimerTick,
            CalibrateDone,
        }

        private class Event
        {
            public Event(ulong at, EventKind kind, int channel)
            {
                At = at;
                Kind = kind;
                Channel = channel;
            }

            public ulong At;
            public EventKind Kind;
            public int Channel;
        }
    }

    public class BbSaadcStats
    {
        public uint Acquisitions;
        public uint Samples;
        public uint Results;
        public uint ResultsDropped;
        public uint TasksIgnored;
        public uint DmaWrites;
        public uint AcqCount;
        public uint SampleCount;
        public readonly ulong[] AcqRing = new ulong[RingSize];
        public readonly uint[] AcqInfo = new uint[RingSize];
        public readonly ulong[] SampleRing = new ulong[RingSize];
        public readonly uint[] SampleUv = new uint[RingSize];
        public readonly uint[] SampleInfo = new uint[RingSize];

        public void RecordAcquisition(ulong a, int input, int channel)
        {
            AcqRing[AcqCount % RingSize] = a;
            AcqInfo[AcqCount % RingSize] = (uint)input | ((uint)channel << 8);
            AcqCount++;
        }

        public void RecordSample(ulong s, int input, int channel, uint uv, long code)
        {
            SampleRing[SampleCount % RingSize] = s;
            SampleUv[SampleCount % RingSize] = uv;
            SampleInfo[SampleCount % RingSize] = ((uint)code & 0xFFFF) | ((uint)input << 16) | ((uint)channel << 24);
            SampleCount++;
        }

        public const int RingSize = 256;
    }
}

namespace Antmicro.Renode.Peripherals.Miscellaneous
{
    using Antmicro.Renode.Peripherals.Analog;

    // The host-only port of a BbNrf52Saadc (ADR-008 D7; ADR-005 D5's layout
    // where they share a meaning), mapped at 0x5FFF0100. Only the Breadboard
    // engine (External Control SYSTEM_BUS) uses it; firmware accesses are
    // ignored and logged.
    //
    // Reads (dword offsets):
    //   0x000 ID 0x50534242 ("BBSP")
    //   0x004 ERROR, sticky: bit 0 a timed input earlier than a sample
    //         already taken on its input (late), 1 out of time order, 2 bad
    //         batch framing, 3 firmware access, 4 bad input, 5 an acquisition
    //         on a loaded input with TACQ < 2 L (ADR-008 D7 refusal).
    //         A host write of any value to 0x004 clears it.
    //   0x008 / 0x00C  t_ns of the first error, low / high; 0x010 its input
    //   0x014 timed inputs accepted         0x018 results (RESULTDONE)
    //   0x01C samples                       0x020 acquisitions
    //   0x024 tasks ignored                 0x028 EasyDMA result writes
    //   0x02C acquisition-ring count        0x030 sample-ring count
    //   0x034 the first refused acquisition's TACQ in ns
    //   0x058 / 0x05C  the SAADC's clock time in ns, low / high
    //   0x080 L in ns, 0x084 loaded input mask, 0x088 trace input mask
    //         (also written by the host at startup)
    //   0x100 + 4i  the latest timed input of input i (0..9), uV, or
    //               0xFFFFFFFF without one
    //   0x2000 + 16j  acquisition ring (loaded inputs), entry j = k mod 256:
    //                 a lo, a hi, input | channel << 8, 0
    //   0x3000 + 16j  sample ring (traced inputs), entry j = k mod 256:
    //                 s lo, s hi, uV, code (16 bits) | input << 16 | channel << 24
    // Writes:
    //   0x004 clears ERROR; 0x080 / 0x084 / 0x088 the configuration above
    //   0x1000-0x1FFF  the batch stream (ADR-005 D5.1): header
    //                  0xBA7C0000 | n, then n entries of 3 dwords:
    //                  t_ns low, t_ns high (16 bits) | input << 16, uV.
    public class BbSaadcHostPort : IDoubleWordPeripheral, IKnownSize, INumberedGPIOOutput
    {
        public BbSaadcHostPort(IMachine machine, BbNrf52Saadc saadc)
        {
            this.machine = machine;
            this.saadc = saadc;
            line = new GPIO();
            Connections = new Dictionary<int, IGPIO> { { 0, line } };
            saadc.ReportError = (bit, t, input, tacq) =>
            {
                if(error == 0)
                {
                    refusedTacq = tacq;
                }
                Error(bit, t, input);
            };
            saadc.Notify = Toggle;
        }

        public long Size => 0x4000;

        // GPIO output 0 toggles whenever a ring entry is recorded or an error
        // bit is set during the emulation, so the engine (an External Control
        // GPIO event) polls the port only in an advance that has news.
        public IReadOnlyDictionary<int, IGPIO> Connections { get; }

        private void Toggle()
        {
            line.Set(!line.IsSet);
        }

        public void Reset()
        {
            error = 0;
            firstErrorNs = 0;
            firstErrorInput = 0;
            refusedTacq = 0;
            remaining = 0;
            entryWord = 0;
            accepted = 0;
        }

        public uint ReadDoubleWord(long offset)
        {
            if(FromFirmware())
            {
                ReportFirmwareAccess("read", offset);
                return 0;
            }
            var st = saadc.Stats;
            if(offset >= 0x100 && offset < 0x100 + 4 * BbNrf52Saadc.InputCount)
            {
                return saadc.LatestTimedInput((int)((offset - 0x100) / 4));
            }
            if(offset >= 0x2000 && offset < 0x2000 + 16 * BbSaadcStats.RingSize)
            {
                var j = (offset - 0x2000) / 16;
                switch((offset - 0x2000) % 16 / 4)
                {
                case 0: return (uint)st.AcqRing[j];
                case 1: return (uint)(st.AcqRing[j] >> 32);
                case 2: return st.AcqInfo[j];
                default: return 0;
                }
            }
            if(offset >= 0x3000 && offset < 0x3000 + 16 * BbSaadcStats.RingSize)
            {
                var j = (offset - 0x3000) / 16;
                switch((offset - 0x3000) % 16 / 4)
                {
                case 0: return (uint)st.SampleRing[j];
                case 1: return (uint)(st.SampleRing[j] >> 32);
                case 2: return st.SampleUv[j];
                default: return st.SampleInfo[j];
                }
            }
            switch(offset)
            {
            case 0x000: return 0x50534242;
            case 0x004: return error;
            case 0x008: return (uint)firstErrorNs;
            case 0x00C: return (uint)(firstErrorNs >> 32);
            case 0x010: return firstErrorInput;
            case 0x014: return accepted;
            case 0x018: return st.Results;
            case 0x01C: return st.Samples;
            case 0x020: return st.Acquisitions;
            case 0x024: return st.TasksIgnored;
            case 0x028: return st.DmaWrites;
            case 0x02C: return st.AcqCount;
            case 0x030: return st.SampleCount;
            case 0x034: return refusedTacq;
            case 0x058: return (uint)saadc.NowNs;
            case 0x05C: return (uint)(saadc.NowNs >> 32);
            case 0x080: return (uint)saadc.LatticeNs;
            case 0x084: return saadc.LoadedMask;
            case 0x088: return saadc.TraceMask;
            default: return 0;
            }
        }

        public void WriteDoubleWord(long offset, uint value)
        {
            if(FromFirmware())
            {
                ReportFirmwareAccess("write", offset);
                return;
            }
            switch(offset)
            {
            case 0x004:
                error = 0;
                return;
            case 0x080:
                saadc.LatticeNs = value;
                return;
            case 0x084:
                saadc.LoadedMask = value;
                return;
            case 0x088:
                saadc.TraceMask = value;
                return;
            }
            if(offset < 0x1000 || offset >= 0x2000)
            {
                return;
            }
            if(remaining == 0)
            {
                if((value & 0xFFFF0000u) != BatchMagic)
                {
                    Error(ErrFraming, 0, 0);
                    return;
                }
                remaining = value & 0xFFFF;
                entryWord = 0;
                return;
            }
            entry[entryWord++] = value;
            if(entryWord < 3)
            {
                return;
            }
            entryWord = 0;
            remaining--;
            var t = (ulong)entry[0] | ((ulong)(entry[1] & 0xFFFF) << 32);
            var input = (int)(entry[1] >> 16);
            var err = saadc.AddTimedInput(input, t, entry[2]);
            if(err != 0)
            {
                Error(err, t, (uint)input);
            }
            else
            {
                accepted++;
            }
        }

        private void Error(uint bit, ulong t, uint input)
        {
            if(error == 0)
            {
                firstErrorNs = t;
                firstErrorInput = input;
                this.Log(LogLevel.Warning, "bb_saadc_host_port: error 0x{0:X} (t {1} ns, input {2})", bit, t, input);
            }
            error |= bit;
            Toggle();
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
                this.Log(LogLevel.Warning, "bb_saadc_host_port: firmware {0} at offset 0x{1:X} ignored (the port is for the Breadboard engine only)", what, offset);
            }
            error |= ErrFirmware;
        }

        public const uint ErrLate = 1u << 0, ErrOrder = 1u << 1, ErrFraming = 1u << 2, ErrFirmware = 1u << 3,
            ErrChannel = 1u << 4, ErrTacq = 1u << 5;
        private const uint BatchMagic = 0xBA7C0000;

        private readonly IMachine machine;
        private readonly BbNrf52Saadc saadc;
        private readonly GPIO line;
        private readonly uint[] entry = new uint[3];
        private uint error;
        private ulong firstErrorNs;
        private uint firstErrorInput;
        private uint refusedTacq;
        private uint remaining;
        private int entryWord;
        private uint accepted;
    }

    // A tap on the nRF52840's UARTE0/UARTE1 (ADR-008 D8): every byte the
    // firmware transmits, with the instruction-accurate virtual time of the
    // transmission (cpu.SyncTime() in the register write that started it),
    // in a ring the Renode engine turns into `log` records, one per line.
    // Mapped at 0x5FFF8000; host reads only, firmware accesses ignored.
    //   0x000 ID 0x4C554242 ("BBUL")   0x004 bytes recorded
    //   0x100 + 8j  ring entry j = k mod 1024 of byte k:
    //               t_ns low, t_ns high (16 bits) | uart << 16 | byte << 24
    // GPIO output 0 toggles at every '\n' and every 512th byte, so the
    // engine (an External Control GPIO event, sent inline with the run)
    // reads the ring only in an advance that has something to read.
    public class BbUartLog : IDoubleWordPeripheral, IKnownSize, INumberedGPIOOutput
    {
        public BbUartLog(IMachine machine, IUART uart0 = null, IUART uart1 = null)
        {
            this.machine = machine;
            line = new GPIO();
            Connections = new Dictionary<int, IGPIO> { { 0, line } };
            clock = (Antmicro.Renode.Time.BaseClockSource)machine.ClockSource;
            if(uart0 != null)
            {
                uart0.CharReceived += b => Record(0, b);
            }
            if(uart1 != null)
            {
                uart1.CharReceived += b => Record(1, b);
            }
        }

        public long Size => 0x2100;

        public IReadOnlyDictionary<int, IGPIO> Connections { get; }

        public void Reset()
        {
            count = 0;
        }

        public uint ReadDoubleWord(long offset)
        {
            if(FromFirmware())
            {
                return 0;
            }
            if(offset >= 0x100 && offset < 0x100 + 8 * RingSize)
            {
                var j = (offset - 0x100) / 8;
                return offset % 8 == 0 ? (uint)ring[j] : (uint)(ring[j] >> 32);
            }
            switch(offset)
            {
            case 0x000: return 0x4C554242;
            case 0x004: return count;
            default: return 0;
            }
        }

        public void WriteDoubleWord(long offset, uint value)
        {
        }

        private void Record(int uart, byte b)
        {
            if(machine.SystemBus.TryGetCurrentCPU(out var cpu))
            {
                cpu.SyncTime();
            }
            var t = clock.CurrentValue.TotalNanoseconds;
            ring[count % RingSize] = (t & 0xFFFFFFFFFFFFul) | ((ulong)uart << 48) | ((ulong)b << 56);
            count++;
            if(b == (byte)'\n' || count % (RingSize / 2) == 0)
            {
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

        public const int RingSize = 1024;
        private readonly IMachine machine;
        private readonly Antmicro.Renode.Time.BaseClockSource clock;
        private readonly ulong[] ring = new ulong[RingSize];
        private readonly GPIO line;
        private uint count;
    }
}
