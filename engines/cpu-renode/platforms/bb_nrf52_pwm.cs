// Breadboard nRF52840 PWM, TEMP and NVMC models with their host port
// (ADR-008 D9, Amendment 9), for Renode 1.17.0 only. Renode 1.17.0 has none
// of the three on the nRF52840.
//
// Style of bb_nrf52_saadc.cs: every deferred action is a deadline of one
// one-shot LimitTimer at 1 GHz on the machine's clock source (one ClockEntry
// per model instance, armed for the earliest pending deadline), so each
// instant is a clock limit that the lookahead promise sees; no model queues
// a synced action, starts a thread or reads host time. A register write by
// the CPU is timed instruction-accurately after cpu.SyncTime().
//
// BbNrf52Pwm (nRF52840 PS, PWM chapter): PWM_CLK = 16 MHz / 2^PRESCALER; up
// mode (period COUNTERTOP ticks) and up-and-down mode (period 2 COUNTERTOP
// ticks); each 16-bit value is COMP (bits 14:0) and POLARITY (bit 15,
// 1 = FallingEdge: the period starts high). Up: start level on ticks
// [0, COMP), the other level on [COMP, COUNTERTOP); COMP = 0 gives the other
// level all period, COMP >= COUNTERTOP the start level (no edges). Up and
// down: edges at COMP and 2 COUNTERTOP - COMP. Decoder LOAD Common, Grouped,
// Individual, WaveForm (the fourth half-word is the period's COUNTERTOP,
// channel 3 is not driven); MODE RefreshCount (a step plays REFRESH + 1
// periods) or NextStep (a step plays until NEXTSTEP; REFRESH and ENDDELAY
// ignored). SEQSTART[n] latches SEQ[n] PTR, CNT, REFRESH, ENDDELAY (and LOOP);
// a step is read from RAM (EasyDMA) at the start of its first period.
// SEQSTARTED[n] at the start of the sequence's first period; SEQEND[n] at the
// start of its last step's first period; then ENDDELAY periods of the last
// step. LOOP.CNT > 0 chains SEQ[0] -> SEQ[1] and counts down after each
// SEQ[1]; LOOPSDONE at the boundary where it reaches 0 (SHORTS may restart a
// sequence or stop there). Without chaining the last step holds.
// PWMPERIODEND at every period boundary. STOP ends generation at the end of
// the current period (STOPPED there; at once when nothing runs). ENABLE = 0
// ends it at once without STOPPED.
//
// Recorded choices (the PS gives no figure; ADR-008 Amendment 9): start
// latency 0 (the first period starts at the SEQSTART write); a SEQSTART while
// running takes effect at the next period boundary; COUNTERTOP, MODE and
// PRESCALER are latched per period, DECODER per sequence; a SEQSTART whose
// CNT is less than one step is ignored (warning); two channels on one pin are
// reported as they are (warning); register-visible events are set at the
// event's nanosecond rounded down (period boundaries and compare points are
// multiples of 62.5 ns from the SEQSTART instant, so they can fall on half a
// nanosecond); STOP while idle sets STOPPED at once.
//
// Pins: while generation runs (from the first period's start to STOPPED or
// ENABLE = 0) a connected channel (PSEL.OUT[n].CONNECT = 0) drives its pin;
// otherwise the pin is the GPIO's OUT level. Renode's GPIO model is not
// touched: every change of a driven pin (the level, a connect with its level,
// a release) is recorded, at its exact time in half-nanoseconds, in the host
// port's ring, which the Renode engine reports as transitions of the bound
// pin. Each pin edge is a deadline of the model's timer, armed at the edge's
// nanosecond rounded down, so the lookahead promise ends at or before it.
//
// Not modelled (a warning once each where it matters): PPI event outputs and
// task inputs (D9: BbNrf52Ppi), the effect of the PWM on the GPIO IN
// register, glitches of a COUNTERTOP write inside a period (latched per
// period), the EasyDMA fetch time.
//
// BbNrf52Temp (PS, TEMP): DATARDY tTEMP = 36 us (PS typical) after START;
// TEMP then reads the host value (0.25 degC steps). STOP cancels. A0-A5,
// B0-B5, T0-T4 are storage with the PS reset values.
//
// BbNrf52Nvmc (PS, NVMC): READY, READYNEXT, CONFIG, ERASEPAGE (= ERASEPCR1),
// ERASEPCR0, ERASEALL, ERASEUICR, ERASEPAGEPARTIAL(+CFG), ICACHECNF, IHIT,
// IMISS. An erase in erase mode (CONFIG.WEN = Een) fills its range with 0xFF
// at once and holds READY and READYNEXT low for the PS maximum (tERASEPAGE
// 85 ms, tERASEALL 169 ms; ERASEUICR tERASEPAGE, an assumption); a partial
// erase holds them low for ERASEPAGEPARTIALCFG ms and erases the page once
// that page's partial erases add up to tERASEPAGE. GAP (recorded): word
// writes to flash go straight to Renode's MappedMemory, so they have no
// timing, no CONFIG.WEN gating and no 1-to-0-only rule. The UICR is not
// memory on this platform (an ERASEUICR only times). The CPU is not halted
// during an erase (silicon halts a CPU fetching from flash).
using System;
using System.Collections.Generic;
using Antmicro.Renode.Core;
using Antmicro.Renode.Core.Structure;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.Bus;
using Antmicro.Renode.Peripherals.Timers;
using Antmicro.Renode.Time;

namespace Antmicro.Renode.Peripherals.Miscellaneous
{
    // A sink for the pin changes of the PWM instances (the host port).
    public delegate void BbPwmPinChange(ulong tHns, int instance, int channel, int kind, bool level, uint pin);

    [AllowedTranslations(AllowedTranslation.ByteToDoubleWord | AllowedTranslation.WordToDoubleWord)]
    public class BbNrf52Pwm : IDoubleWordPeripheral, IKnownSize
    {
        public BbNrf52Pwm(IMachine machine, int instance = 0)
        {
            this.machine = machine;
            this.instance = instance;
            sysbus = machine.GetSystemBus(this);
            clock = (BaseClockSource)machine.ClockSource;
            IRQ = new GPIO();
            timer = new LimitTimer(machine.ClockSource, 1000000000, this, "bbPwmEvents", limit: ulong.MaxValue,
                direction: Direction.Ascending, enabled: false, workMode: WorkMode.OneShot, eventEnabled: true);
            timer.LimitReached += OnTimer;
            Reset();
        }

        public long Size => 0x1000;

        public GPIO IRQ { get; }

        public int Instance => instance;

        // Set by the host port: every change of a driven pin.
        public BbPwmPinChange PinChange { get; set; }

        // Counters for the tests (host port 0x080 + 16 i).
        public uint Periods { get; private set; }
        public uint SequencesStarted { get; private set; }
        public uint StepsLoaded { get; private set; }

        public void Reset()
        {
            timer.Enabled = false;
            ev = 0;
            inten = 0;
            shorts = 0;
            enable = false;
            mode = 0;
            countertop = 0x3FF;
            prescaler = 0;
            decoder = 0;
            loop = 0;
            for(var n = 0; n < 2; n++)
            {
                seqPtr[n] = 0;
                seqCnt[n] = 0;
                seqRefresh[n] = 1;
                seqEndDelay[n] = 0;
            }
            for(var c = 0; c < Channels; c++)
            {
                psel[c] = 0xFFFFFFFF;
                value[c] = 0;
                level[c] = false;
                ownedPin[c] = NoPin;
            }
            running = false;
            edges.Clear();
            stopPending = false;
            pendingStart = -1;
            nextStepPending = false;
            Periods = 0;
            SequencesStarted = 0;
            StepsLoaded = 0;
            IRQ.Unset();
        }

        // ---- bus interface ------------------------------------------------

        public uint ReadDoubleWord(long offset)
        {
            if(offset >= EventsStart && offset <= EventsEnd)
            {
                return (ev >> (int)((offset - 0x100) / 4)) & 1;
            }
            if(offset >= SeqBase && offset < SeqBase + 0x40)
            {
                var n = (int)((offset - SeqBase) / 0x20);
                switch((offset - SeqBase) % 0x20)
                {
                case 0x0: return seqPtr[n];
                case 0x4: return seqCnt[n];
                case 0x8: return seqRefresh[n];
                case 0xC: return seqEndDelay[n];
                default: return 0;
                }
            }
            if(offset >= PselBase && offset < PselBase + 4 * Channels)
            {
                return psel[(offset - PselBase) / 4];
            }
            switch(offset)
            {
            case TaskStop:
            case TaskSeqStart0:
            case TaskSeqStart1:
            case TaskNextStep:
                return 0;
            case ShortsOffset: return shorts;
            case IntenOffset:
            case IntenSetOffset:
            case IntenClrOffset:
                return inten;
            case EnableOffset: return enable ? 1u : 0u;
            case ModeOffset: return mode;
            case CountertopOffset: return countertop;
            case PrescalerOffset: return prescaler;
            case DecoderOffset: return decoder;
            case LoopOffset: return loop;
            default:
                WarnOnce("read", offset);
                return 0;
            }
        }

        public void WriteDoubleWord(long offset, uint value)
        {
            if(offset >= EventsStart && offset <= EventsEnd)
            {
                var bit = 1u << (int)((offset - 0x100) / 4);
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
            if(offset >= SeqBase && offset < SeqBase + 0x40)
            {
                var n = (int)((offset - SeqBase) / 0x20);
                switch((offset - SeqBase) % 0x20)
                {
                case 0x0: seqPtr[n] = value; break;
                case 0x4: seqCnt[n] = value & 0x7FFF; break;
                case 0x8: seqRefresh[n] = value & 0xFFFFFF; break;
                case 0xC: seqEndDelay[n] = value & 0xFFFFFF; break;
                default: WarnOnce("write", offset); break;
                }
                return;
            }
            if(offset >= PselBase && offset < PselBase + 4 * Channels)
            {
                var c = (int)((offset - PselBase) / 4);
                psel[c] = value & 0x8000003F;
                if(running)
                {
                    SyncCpu();
                    UpdateOwnership(2 * Now());
                }
                return;
            }
            switch(offset)
            {
            case TaskStop:
                if((value & 1) != 0)
                {
                    SyncCpu();
                    StopTask(2 * Now());
                    Rearm();
                }
                return;
            case TaskSeqStart0:
            case TaskSeqStart1:
                if((value & 1) != 0)
                {
                    SyncCpu();
                    SeqStartTask(offset == TaskSeqStart0 ? 0 : 1, 2 * Now());
                    Rearm();
                }
                return;
            case TaskNextStep:
                if((value & 1) != 0)
                {
                    if((decoder & DecoderNextStep) != 0)
                    {
                        nextStepPending = true;
                    }
                }
                return;
            case ShortsOffset: shorts = value & 0x1F; return;
            case IntenOffset: inten = value & EventMask; UpdateIrq(); return;
            case IntenSetOffset: inten |= value & EventMask; UpdateIrq(); return;
            case IntenClrOffset: inten &= ~value; UpdateIrq(); return;
            case EnableOffset:
                var en = (value & 1) != 0;
                if(enable && !en && running)
                {
                    SyncCpu();
                    var t = 2 * Now();
                    ReleaseAll(t);
                    running = false;
                    edges.Clear();
                    stopPending = false;
                    pendingStart = -1;
                    timer.Enabled = false;
                }
                enable = en;
                return;
            case ModeOffset: mode = value & 1; return;
            case CountertopOffset: countertop = value & 0x7FFF; return;
            case PrescalerOffset: prescaler = value & 0x7; return;
            case DecoderOffset: decoder = value & 0x103; return;
            case LoopOffset: loop = value & 0xFFFF; return;
            default:
                WarnOnce("write", offset);
                return;
            }
        }

        // ---- tasks ----------------------------------------------------------

        private void SeqStartTask(int n, ulong t)
        {
            if(!enable)
            {
                WarnOnceText("seqstart-disabled", "SEQSTART with ENABLE = 0 ignored");
                return;
            }
            if(seqCnt[n] < StepSize())
            {
                WarnOnceText("seqstart-cnt", $"SEQSTART[{n}] with CNT {seqCnt[n]} < one step ignored");
                return;
            }
            loopsLeft = loop;
            chained = loop > 0;
            if(!running)
            {
                running = true;
                stopPending = false;
                pendingStart = -1;
                StartSequence(n, t);
                StartPeriod(t, true);
            }
            else
            {
                pendingStart = n;
            }
        }

        private void StopTask(ulong t)
        {
            if(!running)
            {
                SetEvent(EvStopped);
                return;
            }
            stopPending = true;
        }

        // ---- the generator --------------------------------------------------

        private void StartSequence(int n, ulong t)
        {
            cur = n;
            latPtr = seqPtr[n];
            latRefresh = seqRefresh[n];
            latEndDelay = seqEndDelay[n];
            latDecoder = decoder;
            stepSize = StepSize();
            stepCount = seqCnt[n] / stepSize;
            if(seqCnt[n] % stepSize != 0)
            {
                WarnOnceText("cnt-rest", $"SEQ[{n}].CNT {seqCnt[n]} not a multiple of the step size {stepSize}; the rest is ignored");
            }
            SequencesStarted++;
            SetEvent(n == 0 ? EvSeqStarted0 : EvSeqStarted1);
            LoadStep(0);
        }

        private void LoadStep(uint k)
        {
            stepIdx = k;
            phase = Phase.Playing;
            remaining = (latDecoder & DecoderNextStep) != 0 ? ulong.MaxValue : (ulong)latRefresh + 1;
            nextStepPending = false;
            var v = new ushort[4];
            for(var j = 0; j < stepSize; j++)
            {
                v[j] = sysbus.ReadWord(latPtr + 2 * (ulong)(k * stepSize + j));
            }
            switch(latDecoder & 0x3)
            {
            case LoadCommon:
                for(var c = 0; c < Channels; c++)
                {
                    value[c] = v[0];
                }
                break;
            case LoadGrouped:
                value[0] = value[1] = v[0];
                value[2] = value[3] = v[1];
                break;
            case LoadIndividual:
                for(var c = 0; c < Channels; c++)
                {
                    value[c] = v[c];
                }
                break;
            default: // WaveForm
                value[0] = v[0];
                value[1] = v[1];
                value[2] = v[2];
                waveTop = (uint)(v[3] & 0x7FFF);
                break;
            }
            StepsLoaded++;
            if(k + 1 == stepCount)
            {
                SetEvent(cur == 0 ? EvSeqEnd0 : EvSeqEnd1);
                if((shorts & (cur == 0 ? ShortSeqEnd0Stop : ShortSeqEnd1Stop)) != 0)
                {
                    stopPending = true;
                }
            }
        }

        // A period boundary at t: the previous period ended.
        private void Boundary(ulong t)
        {
            Periods++;
            SetEvent(EvPwmPeriodEnd);
            if(stopPending)
            {
                Stop(t);
                return;
            }
            if(pendingStart >= 0)
            {
                var n = pendingStart;
                pendingStart = -1;
                StartSequence(n, t);
                StartPeriod(t, false);
                return;
            }
            if(phase != Phase.Hold)
            {
                var done = false;
                if((latDecoder & DecoderNextStep) != 0 && phase == Phase.Playing)
                {
                    done = nextStepPending;
                }
                else if(remaining != ulong.MaxValue)
                {
                    remaining--;
                    done = remaining == 0;
                }
                if(done)
                {
                    Advance(t);
                    if(!running)
                    {
                        return;
                    }
                }
            }
            StartPeriod(t, false);
        }

        // The current state's periods are over at t: what plays next.
        private void Advance(ulong t)
        {
            var nextStep = (latDecoder & DecoderNextStep) != 0;
            if(phase == Phase.Playing && stepIdx + 1 < stepCount)
            {
                LoadStep(stepIdx + 1);
                return;
            }
            if(phase == Phase.Playing && !nextStep && latEndDelay > 0)
            {
                phase = Phase.EndDelay;
                remaining = latEndDelay;
                return;
            }
            // The sequence is over.
            if(chained)
            {
                if(cur == 0)
                {
                    StartSequenceChecked(1, t);
                    return;
                }
                if(loopsLeft > 0)
                {
                    loopsLeft--;
                }
                if(loopsLeft > 0)
                {
                    StartSequenceChecked(0, t);
                    return;
                }
                SetEvent(EvLoopsDone);
                if((shorts & ShortLoopsDoneStop) != 0)
                {
                    Stop(t);
                    return;
                }
                if((shorts & (ShortLoopsDoneSeqStart0 | ShortLoopsDoneSeqStart1)) != 0)
                {
                    var n = (shorts & ShortLoopsDoneSeqStart0) != 0 ? 0 : 1;
                    loopsLeft = loop;
                    chained = loop > 0;
                    StartSequenceChecked(n, t);
                    return;
                }
            }
            phase = Phase.Hold;
            remaining = ulong.MaxValue;
        }

        private void StartSequenceChecked(int n, ulong t)
        {
            if(seqCnt[n] < StepSize())
            {
                WarnOnceText("chain-cnt", $"SEQ[{n}] with CNT {seqCnt[n]} < one step: the last step holds");
                phase = Phase.Hold;
                remaining = ulong.MaxValue;
                return;
            }
            StartSequence(n, t);
        }

        private void Stop(ulong t)
        {
            ReleaseAll(t);
            running = false;
            stopPending = false;
            pendingStart = -1;
            edges.Clear();
            SetEvent(EvStopped);
        }

        // A new period starts at t with the current values.
        private void StartPeriod(ulong t, bool first)
        {
            var wave = (latDecoder & 0x3) == LoadWaveForm;
            var top = wave ? waveTop : countertop;
            if(top < 3)
            {
                WarnOnceText("top", $"COUNTERTOP {top} < 3 (the PS minimum) taken as 3");
                top = 3;
            }
            var tick = TickHns << (int)prescaler;
            var updown = mode == 1;
            periodStart = t;
            periodLen = (updown ? 2 * (ulong)top : top) * tick;
            edges.Clear();
            for(var c = 0; c < Channels; c++)
            {
                var comp = (uint)(value[c] & 0x7FFF);
                var startHigh = (value[c] & 0x8000) != 0;
                bool l0;
                if(comp == 0)
                {
                    l0 = !startHigh;
                }
                else
                {
                    l0 = startHigh;
                    if(comp < top)
                    {
                        edges.Add(new Edge(t + comp * tick, c, !startHigh));
                        if(updown)
                        {
                            edges.Add(new Edge(t + (2 * (ulong)top - comp) * tick, c, startHigh));
                        }
                    }
                }
                SetLevel(c, l0, t);
            }
            edges.Sort((a, b) => a.T != b.T ? a.T.CompareTo(b.T) : a.Channel.CompareTo(b.Channel));
            edgeNext = 0;
            UpdateOwnership(t);
        }

        private void SetLevel(int c, bool l, ulong t)
        {
            if(level[c] == l)
            {
                return;
            }
            level[c] = l;
            if(ownedPin[c] != NoPin)
            {
                PinChange?.Invoke(t, instance, c, KindLevel, l, ownedPin[c]);
            }
        }

        private bool Drives(int c)
        {
            if((psel[c] & PselDisconnected) != 0)
            {
                return false;
            }
            return !((latDecoder & 0x3) == LoadWaveForm && c == 3);
        }

        // Connect and release pins to match PSEL and the decoder at t.
        private void UpdateOwnership(ulong t)
        {
            for(var c = 0; c < Channels; c++)
            {
                var want = running && Drives(c) ? (psel[c] & 0x3F) : NoPin;
                if(want == ownedPin[c])
                {
                    continue;
                }
                if(ownedPin[c] != NoPin)
                {
                    PinChange?.Invoke(t, instance, c, KindRelease, level[c], ownedPin[c]);
                }
                ownedPin[c] = want;
                if(want != NoPin)
                {
                    for(var d = 0; d < Channels; d++)
                    {
                        if(d != c && ownedPin[d] == want)
                        {
                            WarnOnceText("pin-shared", $"channels {d} and {c} both drive pin {want >> 5}.{want & 31}");
                        }
                    }
                    PinChange?.Invoke(t, instance, c, KindConnect, level[c], want);
                }
            }
        }

        private void ReleaseAll(ulong t)
        {
            for(var c = 0; c < Channels; c++)
            {
                if(ownedPin[c] != NoPin)
                {
                    PinChange?.Invoke(t, instance, c, KindRelease, level[c], ownedPin[c]);
                    ownedPin[c] = NoPin;
                }
            }
        }

        // The earliest pending instant (half-nanoseconds), or none.
        private bool NextDeadline(out ulong t)
        {
            t = 0;
            if(!running)
            {
                return false;
            }
            t = periodStart + periodLen;
            if(edgeNext < edges.Count && edges[edgeNext].T < t)
            {
                t = edges[edgeNext].T;
            }
            return true;
        }

        private void Rearm()
        {
            timer.Enabled = false;
            if(!NextDeadline(out var t))
            {
                return;
            }
            var now = Now();
            var at = t / 2; // the nanosecond rounded down: a limit at or before the instant
            if(at <= now)
            {
                if(at < now)
                {
                    this.Log(LogLevel.Error, "bb_nrf52_pwm: deadline at {0} hns passed at {1} ns; fired at the next ns", t, now);
                }
                at = now + 1;
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
                while(NextDeadline(out var t) && t / 2 <= now)
                {
                    if(edgeNext < edges.Count && edges[edgeNext].T == t && t < periodStart + periodLen)
                    {
                        var e = edges[edgeNext++];
                        SetLevel(e.Channel, e.Level, e.T);
                    }
                    else
                    {
                        Boundary(t);
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

        private uint StepSize()
        {
            switch(decoder & 0x3)
            {
            case LoadCommon: return 1;
            case LoadGrouped: return 2;
            default: return 4;
            }
        }

        private ulong Now()
        {
            return clock.CurrentValue.TotalNanoseconds;
        }

        private void SyncCpu()
        {
            if(inHandler == 0 && sysbus.TryGetCurrentCPU(out var cpu))
            {
                cpu.SyncTime();
            }
        }

        private void SetEvent(int bit)
        {
            ev |= 1u << bit;
            UpdateIrq();
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
                this.Log(LogLevel.Warning, "bb_nrf52_pwm{0}: {1}", instance, text);
            }
        }

        private struct Edge
        {
            public Edge(ulong t, int channel, bool level)
            {
                T = t;
                Channel = channel;
                Level = level;
            }

            public readonly ulong T;
            public readonly int Channel;
            public readonly bool Level;
        }

        private enum Phase
        {
            Playing,
            EndDelay,
            Hold,
        }

        private readonly IMachine machine;
        private readonly int instance;
        private readonly IBusController sysbus;
        private readonly BaseClockSource clock;
        private readonly LimitTimer timer;
        private readonly HashSet<string> warned = new HashSet<string>();
        private readonly uint[] seqPtr = new uint[2];
        private readonly uint[] seqCnt = new uint[2];
        private readonly uint[] seqRefresh = new uint[2];
        private readonly uint[] seqEndDelay = new uint[2];
        private readonly uint[] psel = new uint[Channels];
        private readonly ushort[] value = new ushort[Channels];
        private readonly bool[] level = new bool[Channels];
        private readonly uint[] ownedPin = new uint[Channels];
        private readonly List<Edge> edges = new List<Edge>();
        private int edgeNext;
        private int inHandler;

        private uint ev, inten, shorts, mode, countertop, prescaler, decoder, loop;
        private bool enable;

        private bool running, stopPending, nextStepPending, chained;
        private int pendingStart;
        private int cur;
        private uint latPtr, latRefresh, latEndDelay, latDecoder, stepSize, stepCount, stepIdx, waveTop;
        private uint loopsLeft;
        private Phase phase;
        private ulong remaining;
        private ulong periodStart, periodLen;

        public const int Channels = 4;
        public const int KindLevel = 0, KindConnect = 1, KindRelease = 2;
        private const uint NoPin = 0xFFFFFFFF;
        private const uint PselDisconnected = 0x80000000;
        private const ulong TickHns = 125; // 62.5 ns at 16 MHz, in half-nanoseconds
        private const uint DecoderNextStep = 0x100;
        private const uint LoadCommon = 0, LoadGrouped = 1, LoadIndividual = 2, LoadWaveForm = 3;
        private const uint ShortSeqEnd0Stop = 1u << 0, ShortSeqEnd1Stop = 1u << 1, ShortLoopsDoneSeqStart0 = 1u << 2,
            ShortLoopsDoneSeqStart1 = 1u << 3, ShortLoopsDoneStop = 1u << 4;
        private const int EvStopped = 1, EvSeqStarted0 = 2, EvSeqStarted1 = 3, EvSeqEnd0 = 4, EvSeqEnd1 = 5,
            EvPwmPeriodEnd = 6, EvLoopsDone = 7;
        private const uint EventMask = 0xFE;
        private const long TaskStop = 0x004, TaskSeqStart0 = 0x008, TaskSeqStart1 = 0x00C, TaskNextStep = 0x010,
            EventsStart = 0x104, EventsEnd = 0x11C, ShortsOffset = 0x200, IntenOffset = 0x300,
            IntenSetOffset = 0x304, IntenClrOffset = 0x308, EnableOffset = 0x500, ModeOffset = 0x504,
            CountertopOffset = 0x508, PrescalerOffset = 0x50C, DecoderOffset = 0x510, LoopOffset = 0x514,
            SeqBase = 0x520, PselBase = 0x560;
    }

    [AllowedTranslations(AllowedTranslation.ByteToDoubleWord | AllowedTranslation.WordToDoubleWord)]
    public class BbNrf52Temp : IDoubleWordPeripheral, IKnownSize
    {
        public BbNrf52Temp(IMachine machine, ulong measurementNs = 36000, int quarterDegrees = 100)
        {
            sysbus = machine.GetSystemBus(this);
            clock = (BaseClockSource)machine.ClockSource;
            this.measurementNs = measurementNs;
            Temperature = quarterDegrees;
            IRQ = new GPIO();
            timer = new LimitTimer(machine.ClockSource, 1000000000, this, "bbTempEvents", limit: ulong.MaxValue,
                direction: Direction.Ascending, enabled: false, workMode: WorkMode.OneShot, eventEnabled: true);
            timer.LimitReached += OnTimer;
            Reset();
        }

        public long Size => 0x1000;

        public GPIO IRQ { get; }

        // The die temperature in 0.25 degC steps (the host port's 0x040).
        public int Temperature { get; set; }

        public uint Measurements { get; private set; }

        public void Reset()
        {
            timer.Enabled = false;
            measuring = false;
            datardy = false;
            inten = false;
            result = 0;
            Measurements = 0;
            Array.Copy(ResetA, a, 6);
            Array.Copy(ResetB, b, 6);
            Array.Copy(ResetT, tt, 5);
            IRQ.Unset();
        }

        public uint ReadDoubleWord(long offset)
        {
            if(offset >= 0x520 && offset < 0x538)
            {
                return a[(offset - 0x520) / 4];
            }
            if(offset >= 0x540 && offset < 0x558)
            {
                return b[(offset - 0x540) / 4];
            }
            if(offset >= 0x560 && offset < 0x574)
            {
                return tt[(offset - 0x560) / 4];
            }
            switch(offset)
            {
            case 0x000:
            case 0x004:
                return 0;
            case 0x100: return datardy ? 1u : 0u;
            case 0x304:
            case 0x308:
                return inten ? 1u : 0u;
            case 0x508: return (uint)result;
            default:
                WarnOnce("read", offset);
                return 0;
            }
        }

        public void WriteDoubleWord(long offset, uint value)
        {
            if(offset >= 0x520 && offset < 0x538)
            {
                a[(offset - 0x520) / 4] = value & 0x7FF;
                return;
            }
            if(offset >= 0x540 && offset < 0x558)
            {
                b[(offset - 0x540) / 4] = value & 0x3FFF;
                return;
            }
            if(offset >= 0x560 && offset < 0x574)
            {
                tt[(offset - 0x560) / 4] = value & 0xFF;
                return;
            }
            switch(offset)
            {
            case 0x000:
                if((value & 1) != 0 && !measuring)
                {
                    if(sysbus.TryGetCurrentCPU(out var cpu))
                    {
                        cpu.SyncTime();
                    }
                    measuring = true;
                    timer.Enabled = false;
                    timer.Value = 0;
                    timer.Limit = measurementNs;
                    timer.Enabled = true;
                }
                return;
            case 0x004:
                if((value & 1) != 0)
                {
                    measuring = false;
                    timer.Enabled = false;
                }
                return;
            case 0x100:
                datardy = (value & 1) != 0;
                UpdateIrq();
                return;
            case 0x304:
                if((value & 1) != 0)
                {
                    inten = true;
                }
                UpdateIrq();
                return;
            case 0x308:
                if((value & 1) != 0)
                {
                    inten = false;
                }
                UpdateIrq();
                return;
            default:
                WarnOnce("write", offset);
                return;
            }
        }

        private void OnTimer()
        {
            timer.Enabled = false;
            if(!measuring)
            {
                return;
            }
            measuring = false;
            result = Temperature;
            Measurements++;
            datardy = true;
            UpdateIrq();
        }

        private void UpdateIrq()
        {
            IRQ.Set(datardy && inten);
        }

        private void WarnOnce(string what, long offset)
        {
            if(warned.Add(what + offset))
            {
                this.Log(LogLevel.Warning, "bb_nrf52_temp: unhandled {0} at offset 0x{1:X}", what, offset);
            }
        }

        // nRF52840 PS, TEMP register reset values.
        private static readonly uint[] ResetA = { 0x326, 0x348, 0x3AA, 0x40E, 0x4BD, 0x5A3 };
        private static readonly uint[] ResetB = { 0x3FEF, 0x3FBE, 0x3FBE, 0x0012, 0x0124, 0x027C };
        private static readonly uint[] ResetT = { 0xE2, 0x00, 0x19, 0x3C, 0x50 };

        private readonly IBusController sysbus;
        private readonly BaseClockSource clock;
        private readonly LimitTimer timer;
        private readonly ulong measurementNs;
        private readonly HashSet<string> warned = new HashSet<string>();
        private readonly uint[] a = new uint[6];
        private readonly uint[] b = new uint[6];
        private readonly uint[] tt = new uint[5];
        private bool measuring, datardy, inten;
        private int result;
    }

    [AllowedTranslations(AllowedTranslation.ByteToDoubleWord | AllowedTranslation.WordToDoubleWord)]
    public class BbNrf52Nvmc : IDoubleWordPeripheral, IKnownSize
    {
        public BbNrf52Nvmc(IMachine machine, ulong flashSize = 0x100000, ulong erasePageNs = 85000000,
            ulong eraseAllNs = 169000000)
        {
            this.machine = machine;
            sysbus = machine.GetSystemBus(this);
            this.flashSize = flashSize;
            this.erasePageNs = erasePageNs;
            this.eraseAllNs = eraseAllNs;
            timer = new LimitTimer(machine.ClockSource, 1000000000, this, "bbNvmcBusy", limit: ulong.MaxValue,
                direction: Direction.Ascending, enabled: false, workMode: WorkMode.OneShot, eventEnabled: true);
            timer.LimitReached += () =>
            {
                timer.Enabled = false;
                busy = false;
                BusyChange?.Invoke(clock.CurrentValue.TotalNanoseconds, false);
            };
            clock = (BaseClockSource)machine.ClockSource;
            Reset();
        }

        public long Size => 0x1000;

        public uint Erases { get; private set; }

        // ADR-009 D4 (milestone P3): each change of busy (READY low) at its
        // exact clock time, for the #nvmc state output (bb_nrf52_state.cs).
        // Unset unless a run binds #nvmc.
        public Action<ulong, bool> BusyChange { get; set; }

        public void Reset()
        {
            timer.Enabled = false;
            busy = false;
            config = 0;
            partialCfg = 0x0A;
            icachecnf = 0;
            partialNs.Clear();
            Erases = 0;
        }

        public uint ReadDoubleWord(long offset)
        {
            switch(offset)
            {
            case 0x400:
            case 0x408:
                return busy ? 0u : 1u;
            case 0x504: return config;
            case 0x51C: return partialCfg;
            case 0x540: return icachecnf;
            case 0x548:
            case 0x54C:
                return 0;
            case 0x508:
            case 0x50C:
            case 0x510:
            case 0x514:
            case 0x518:
                return 0;
            default:
                WarnOnce("read", offset);
                return 0;
            }
        }

        public void WriteDoubleWord(long offset, uint value)
        {
            switch(offset)
            {
            case 0x504: config = value & 0x3; return;
            case 0x51C: partialCfg = value & 0x7F; return;
            case 0x540: icachecnf = value & 0x101; return;
            case 0x548:
            case 0x54C:
                return;
            case 0x508: // ERASEPAGE (= ERASEPCR1)
            case 0x510: // ERASEPCR0
                if(ErasePage(value, offset == 0x508 ? "ERASEPAGE" : "ERASEPCR0"))
                {
                    Busy(erasePageNs);
                }
                return;
            case 0x50C: // ERASEALL
                if((value & 1) != 0 && CheckEraseMode("ERASEALL"))
                {
                    Fill(0, flashSize);
                    partialNs.Clear();
                    Busy(eraseAllNs);
                }
                return;
            case 0x514: // ERASEUICR
                if((value & 1) != 0 && CheckEraseMode("ERASEUICR"))
                {
                    WarnOnceText("uicr", "ERASEUICR: the UICR is not memory on this platform; timed only");
                    Busy(erasePageNs);
                }
                return;
            case 0x518: // ERASEPAGEPARTIAL
                if(CheckPage(value, "ERASEPAGEPARTIAL") && CheckEraseMode("ERASEPAGEPARTIAL"))
                {
                    var ns = (ulong)partialCfg * 1000000;
                    partialNs.TryGetValue(value, out var acc);
                    acc += ns;
                    if(acc >= erasePageNs)
                    {
                        Fill(value, PageSize);
                        partialNs.Remove(value);
                    }
                    else
                    {
                        partialNs[value] = acc;
                    }
                    Busy(ns);
                }
                return;
            default:
                WarnOnce("write", offset);
                return;
            }
        }

        private bool ErasePage(uint addr, string what)
        {
            if(!CheckPage(addr, what) || !CheckEraseMode(what))
            {
                return false;
            }
            Fill(addr, PageSize);
            partialNs.Remove(addr);
            return true;
        }

        private bool CheckPage(uint addr, string what)
        {
            if(addr % PageSize != 0 || addr >= flashSize)
            {
                WarnOnceText("page" + what, $"{what} of 0x{addr:X}: not a flash page address; ignored");
                return false;
            }
            return true;
        }

        private bool CheckEraseMode(string what)
        {
            if(config != ConfigEen)
            {
                WarnOnceText("mode" + what, $"{what} with CONFIG = {config} (not Een); ignored");
                return false;
            }
            if(busy)
            {
                WarnOnceText("busy" + what, $"{what} while busy; ignored");
                return false;
            }
            // ADR-009 D6 (Phase 2 P4): POF blocks NVMC erases while POFCON.POF
            // is set and the power-fail comparator's output (#pofwarn) is high.
            // Word writes go to the flash directly (no write path here) and are
            // not blocked.
            if(power == null)
            {
                foreach(var pw in machine.GetPeripheralsOfType<BbNrf52Power>())
                {
                    power = pw;
                    break;
                }
            }
            if(power != null && power.BlocksNvmc)
            {
                this.Log(LogLevel.Warning, "{0} blocked by POF (VDD below the power-fail threshold)", what);
                power.ReportNvmcBlocked(OffsetOf(what));
                return false;
            }
            return true;
        }

        private static long OffsetOf(string what)
        {
            switch(what)
            {
            case "ERASEPAGE": return 0x508;
            case "ERASEALL": return 0x50C;
            case "ERASEPCR0": return 0x510;
            case "ERASEUICR": return 0x514;
            case "ERASEPAGEPARTIAL": return 0x518;
            default: return 0;
            }
        }

        private void Fill(ulong addr, ulong size)
        {
            var ff = new byte[PageSize];
            for(var i = 0; i < ff.Length; i++)
            {
                ff[i] = 0xFF;
            }
            for(ulong a = addr; a < addr + size; a += PageSize)
            {
                sysbus.WriteBytes(ff, a);
            }
            Erases++;
        }

        private void Busy(ulong ns)
        {
            if(sysbus.TryGetCurrentCPU(out var cpu))
            {
                cpu.SyncTime();
            }
            var was = busy;
            busy = true;
            timer.Enabled = false;
            timer.Value = 0;
            timer.Limit = Math.Max(1, ns);
            timer.Enabled = true;
            if(!was)
            {
                BusyChange?.Invoke(clock.CurrentValue.TotalNanoseconds, true);
            }
        }

        private void WarnOnce(string what, long offset)
        {
            WarnOnceText(what + offset, $"unhandled {what} at offset 0x{offset:X}");
        }

        private void WarnOnceText(string key, string text)
        {
            if(warned.Add(key))
            {
                this.Log(LogLevel.Warning, "bb_nrf52_nvmc: {0}", text);
            }
        }

        private const uint PageSize = 4096;
        private const uint ConfigEen = 2;
        private readonly IMachine machine;
        private BbNrf52Power power;
        private readonly IBusController sysbus;
        private readonly LimitTimer timer;
        private readonly BaseClockSource clock;
        private readonly ulong flashSize, erasePageNs, eraseAllNs;
        private readonly HashSet<string> warned = new HashSet<string>();
        private readonly SortedDictionary<uint, ulong> partialNs = new SortedDictionary<uint, ulong>();
        private bool busy;
        private uint config, partialCfg, icachecnf;
    }

    // The engine's side of the PWM, TEMP and NVMC models (ADR-008 Amendment 9).
    // Host reads and writes only; a firmware access is ignored and sets error
    // bit 3.
    //   0x000 ID 0x504E4242 ("BBNP")        0x004 error word (bit 0 ring overflow,
    //   0x008 ring entries recorded (total)  bit 3 firmware access; write: clear)
    //   0x00C ring entries acknowledged (host writes the count it has read)
    //   0x040 TEMP's die temperature, 0.25 degC steps (signed; read/write)
    //   0x044 TEMP measurements completed    0x048 NVMC erases
    //   0x080 + 16 i  PWM i: periods, sequences started, steps loaded
    //   0x1000 + 8 j  ring entry j = k mod 8192 of entry k:
    //                 t (half-ns) low; t high (16 bits) | level << 16 | kind << 17
    //                 (0 level, 1 connect, 2 release) | channel << 19 | instance
    //                 << 21 | PSEL pin (port << 5 | pin) << 24
    // GPIO output 0 toggles at the first entry after the last acknowledgement
    // and at an error, so the engine (an External Control GPIO event) reads the
    // ring at most once per advance and only when it has news.
    public class BbNrf52HostPort : IDoubleWordPeripheral, IKnownSize, INumberedGPIOOutput
    {
        public BbNrf52HostPort(IMachine machine, BbNrf52Pwm pwm0 = null, BbNrf52Pwm pwm1 = null,
            BbNrf52Pwm pwm2 = null, BbNrf52Pwm pwm3 = null, BbNrf52Temp temp = null, BbNrf52Nvmc nvmc = null)
        {
            this.machine = machine;
            pwms = new[] { pwm0, pwm1, pwm2, pwm3 };
            this.temp = temp;
            this.nvmc = nvmc;
            line = new GPIO();
            Connections = new Dictionary<int, IGPIO> { { 0, line } };
            foreach(var p in pwms)
            {
                if(p != null)
                {
                    p.PinChange = Record;
                }
            }
        }

        public long Size => 0x1000 + 8 * RingSize;

        public IReadOnlyDictionary<int, IGPIO> Connections { get; }

        public void Reset()
        {
            error = 0;
            count = 0;
            acked = 0;
            notified = false;
        }

        public uint ReadDoubleWord(long offset)
        {
            if(FromFirmware())
            {
                ReportFirmwareAccess("read", offset);
                return 0;
            }
            if(offset >= 0x1000 && offset < 0x1000 + 8 * RingSize)
            {
                var j = (offset - 0x1000) / 8;
                return offset % 8 == 0 ? (uint)ring[j] : (uint)(ring[j] >> 32);
            }
            if(offset >= 0x080 && offset < 0x0C0)
            {
                var p = pwms[(offset - 0x080) / 16];
                if(p == null)
                {
                    return 0;
                }
                switch((offset - 0x080) % 16)
                {
                case 0: return p.Periods;
                case 4: return p.SequencesStarted;
                case 8: return p.StepsLoaded;
                default: return 0;
                }
            }
            switch(offset)
            {
            case 0x000: return 0x504E4242;
            case 0x004: return error;
            case 0x008: return (uint)count;
            case 0x00C: return (uint)acked;
            case 0x040: return temp == null ? 0 : (uint)temp.Temperature;
            case 0x044: return temp == null ? 0 : temp.Measurements;
            case 0x048: return nvmc == null ? 0 : nvmc.Erases;
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
            case 0x00C:
                // The host acknowledges up to `value` (low 32 bits of the count).
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
            case 0x040:
                if(temp != null)
                {
                    temp.Temperature = (int)value;
                }
                return;
            }
        }

        private void Record(ulong tHns, int instance, int channel, int kind, bool level, uint pin)
        {
            ring[count % RingSize] = (tHns & 0xFFFFFFFFFFFFul) | ((level ? 1ul : 0ul) << 48) | ((ulong)kind << 49)
                | ((ulong)channel << 51) | ((ulong)instance << 53) | ((ulong)(pin & 0x3F) << 56);
            count++;
            if(count - acked > RingSize && (error & ErrOverflow) == 0)
            {
                error |= ErrOverflow;
                this.Log(LogLevel.Warning, "bb_nrf52_host_port: pin ring overflow at {0} hns (more than {1} entries unread)", tHns, RingSize);
                line.Set(!line.IsSet);
            }
            Notify();
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
            if((error & ErrFirmware) == 0)
            {
                this.Log(LogLevel.Warning, "bb_nrf52_host_port: firmware {0} at offset 0x{1:X} ignored (the port is for the Breadboard engine only)", what, offset);
                line.Set(!line.IsSet);
            }
            error |= ErrFirmware;
        }

        public const int RingSize = 8192;
        public const uint ErrOverflow = 1u << 0, ErrFirmware = 1u << 3;

        private readonly IMachine machine;
        private readonly BbNrf52Pwm[] pwms;
        private readonly BbNrf52Temp temp;
        private readonly BbNrf52Nvmc nvmc;
        private readonly GPIO line;
        private readonly ulong[] ring = new ulong[RingSize];
        private uint error;
        private ulong count, acked;
        private bool notified;
    }
}
