// Breadboard STM32F0x1/x2/x8 ADC (RM0091 chapter 13) and its host port
// (ADR-005 D1-D5), for Renode 1.17.0 only.
//
// BbStm32F0Adc replaces Renode's Analog.STM32F0_ADC on Breadboard's
// STM32F072 (platforms/stm32f072-bb.repl). It differs from the stock model
// where ADR-005 "Context" lists the stock model's gaps:
//
//   - conversions start only from ADSTART (EXTEN = 00) or from an active
//     edge of the selected trigger input TRGn (GPIO input n, EXTSEL[2:0] at
//     bits 8:6, EXTEN[1:0] rising / falling / both), never from a clock of
//     its own;
//   - every conversion takes the RM0091 / datasheet time (D4): trigger
//     latency, sampling time (SMP), SAR time (RES) and the ADC_DR write
//     latency, at the ADC clock selected by CKMODE, computed exactly in
//     rationals and rounded up to whole ns only for the scheduled event;
//   - the input is read at the sample instant only, from timed inputs
//     (BbAdcHostPort) or, for a channel that never got one, from the IADC
//     channel source (External Control ADC SetValue, RESD), in microvolts:
//     code12 = min(4095, round_half_up(uV * 4095 / Vref_uV));
//   - the analog watchdog compares the raw 12-bit left-aligned data
//     (RM0091 Table 47) at the conversion's end, for the channels of Table
//     48;
//   - CONT, DISCEN, SCANDIR, WAIT, OVR/OVRMOD, ADSTP, ADCAL (83 ADC cycles),
//     EOSMP/EOC/EOSEQ and DMA requests per RM0091 13.4-13.6.
//
// Every deferred action is a one-shot LimitTimer at 1 GHz on the machine's
// clock source (one ClockEntry, armed for the earliest pending event), so
// each event instant is a clock limit that the lookahead promise sees
// (BaseClockSource.NearestLimitIn), and the model never queues a synced
// action, starts a thread or reads host time. Its state changes only in
// register accesses, in its own clock events, at trigger edges (a timer's
// LimitReached) and in host-port batches applied while Renode is paused.
//
// Not modelled (a warning once each): AUTOFF power-down timing (always on),
// TSEN / VREFEN stabilisation, CKMODE = 11 (treated as 00), the analog
// sample-and-hold (source impedance), offset and gain errors, TRG1
// (TIM1_CC4: STM32_Timer's compare channels are private), the DMA one-shot
// end (DMACFG = 0 behaves as circular), writes RM0091 13.4.4 forbids while
// ADSTART = 1 (applied, as silicon has no protection).
using System;
using System.Collections.Generic;
using System.Numerics;
using Antmicro.Renode.Core;
using Antmicro.Renode.Core.Structure;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.Bus;
using Antmicro.Renode.Peripherals.DMA;
using Antmicro.Renode.Peripherals.Sensor;
using Antmicro.Renode.Peripherals.Timers;
using Antmicro.Renode.Time;
using Antmicro.Renode.Utilities.RESD;

namespace Antmicro.Renode.Peripherals.Analog
{
    [AllowedTranslations(AllowedTranslation.ByteToDoubleWord | AllowedTranslation.WordToDoubleWord)]
    public class BbStm32F0Adc : IDoubleWordPeripheral, IKnownSize, IADC, IGPIOReceiver
    {
        public BbStm32F0Adc(IMachine machine, double referenceVoltage = 3.3, ulong pclkFrequency = 8000000,
            ulong asyncClockFrequency = 14000000, IDMA dmaPeripheral = null, int dmaChannel = 0,
            uint calibrationFactor = 0, uint tempSensorMicrovolts = 1430000, uint vrefintMicrovolts = 1230000,
            uint vbatMicrovolts = 1650000)
        {
            if(dmaPeripheral != null && (dmaChannel <= 0 || dmaChannel > dmaPeripheral.NumberOfChannels))
            {
                throw new ArgumentException($"dmaChannel {dmaChannel}: expected 1..{dmaPeripheral.NumberOfChannels}");
            }
            if(pclkFrequency == 0 || asyncClockFrequency == 0 || referenceVoltage <= 0)
            {
                throw new ArgumentException("clock frequencies and referenceVoltage must be positive");
            }
            this.machine = machine;
            sysbus = machine.GetSystemBus(this);
            clock = (BaseClockSource)machine.ClockSource;
            dma = dmaPeripheral;
            this.dmaChannel = dmaChannel;
            VrefMicrovolts = (ulong)Math.Round(referenceVoltage * 1e6);
            pclkHz = pclkFrequency;
            asyncHz = asyncClockFrequency;
            calFactor = calibrationFactor & 0x7F;
            internalDefaults = new uint[] { tempSensorMicrovolts, vrefintMicrovolts, vbatMicrovolts };
            ADCContainer = new SimpleContainerHelper<IRESDSampleSource<VoltageSample>>(machine, this);
            IRQ = new GPIO();
            inputs = new List<TimedInput>[ChannelCount];
            sampledAt = new ulong[ChannelCount];
            hasSampled = new bool[ChannelCount];
            for(var i = 0; i < ChannelCount; i++)
            {
                inputs[i] = new List<TimedInput>();
            }
            timer = new LimitTimer(machine.ClockSource, 1000000000, this, "bbAdcEvents", limit: ulong.MaxValue,
                direction: Direction.Ascending, enabled: false, workMode: WorkMode.OneShot, eventEnabled: true);
            timer.LimitReached += OnTimer;
            Reset();
            this.RegisterDefaultChildren(machine);
        }

        public long Size => 0x400;

        public GPIO IRQ { get; }

        public int ADCChannelCount => ChannelCount;

        public SimpleContainerHelper<IRESDSampleSource<VoltageSample>> ADCContainer { get; }

        public ulong VrefMicrovolts { get; }

        public void Reset()
        {
            timer.Enabled = false;
            events.Clear();
            isr = 0;
            ier = 0;
            aden = false;
            adstart = false;
            adcal = false;
            cfgr1 = 0;
            cfgr2 = 0;
            smpr = 0;
            tr = 0x0FFF0000;
            chselr = 0;
            dr = 0;
            ccr = 0;
            busy = false;
            holding = false;
            heldStart = null;
            discPos = 0;
            current = null;
            for(var i = 0; i < ChannelCount; i++)
            {
                inputs[i].Clear();
                sampledAt[i] = 0;
                hasSampled[i] = false;
            }
            for(var i = 0; i < TriggerInputs; i++)
            {
                triggerLevel[i] = false;
            }
            Stats = new BbAdcStats();
            IRQ.Unset();
        }

        // ---- bus interface ------------------------------------------------

        public uint ReadDoubleWord(long offset)
        {
            switch(offset)
            {
            case IsrOffset:
                return isr;
            case IerOffset:
                return ier;
            case CrOffset:
                return (aden ? CrAden : 0) | (adstart ? CrAdstart : 0) | (adcal ? CrAdcal : 0);
            case Cfgr1Offset:
                return cfgr1;
            case Cfgr2Offset:
                return cfgr2;
            case SmprOffset:
                return smpr;
            case TrOffset:
                return tr;
            case ChselrOffset:
                return chselr;
            case DrOffset:
                // RM0091 13.12.1: reading ADC_DR clears EOC.
                var v = dr;
                if((isr & IsrEoc) != 0)
                {
                    isr &= ~IsrEoc;
                    UpdateIrq();
                }
                ReleaseHold();
                return v;
            case CcrOffset:
                return ccr;
            default:
                WarnOnce("read", offset);
                return 0;
            }
        }

        public void WriteDoubleWord(long offset, uint value)
        {
            switch(offset)
            {
            case IsrOffset:
                var eocCleared = (value & IsrEoc) != 0 && (isr & IsrEoc) != 0;
                isr &= ~(value & IsrMask);
                UpdateIrq();
                if(eocCleared)
                {
                    ReleaseHold();
                }
                break;
            case IerOffset:
                ier = value & IsrMask;
                UpdateIrq();
                break;
            case CrOffset:
                WriteCr(value);
                break;
            case Cfgr1Offset:
                WarnWhileStarted("ADC_CFGR1");
                cfgr1 = value & Cfgr1Mask;
                CheckConfig();
                break;
            case Cfgr2Offset:
                if(aden)
                {
                    WarnOnceText("cfgr2", "ADC_CFGR2 written with ADEN = 1 (RM0091 13.12.5 forbids it); applied");
                }
                cfgr2 = value & 0xC0000000u;
                if(Ckmode == 3)
                {
                    WarnOnceText("ckmode3", "CKMODE = 11 is reserved; treated as 00 (asynchronous clock)");
                }
                break;
            case SmprOffset:
                WarnWhileStarted("ADC_SMPR");
                smpr = value & 0x7;
                break;
            case TrOffset:
                WarnWhileStarted("ADC_TR");
                tr = value & 0x0FFF0FFF;
                break;
            case ChselrOffset:
                WarnWhileStarted("ADC_CHSELR");
                chselr = value & 0x7FFFF;
                break;
            case CcrOffset:
                ccr = value & (CcrVrefen | CcrTsen | CcrVbaten);
                break;
            default:
                WarnOnce("write", offset);
                break;
            }
        }

        // TRGn (RM0091 Table 45): GPIO input n, from a BbStm32Timer's TRGO.
        public void OnGPIO(int number, bool value)
        {
            if(number < 0 || number >= TriggerInputs)
            {
                return;
            }
            inHandler++;
            try
            {
                OnTrigger(number, value);
            }
            finally
            {
                inHandler--;
            }
        }

        private void OnTrigger(int number, bool value)
        {
            var was = triggerLevel[number];
            triggerLevel[number] = value;
            if(was == value)
            {
                return;
            }
            var now = Now();
            Stats.RecordEdge(number, value, now);
            if(!aden || !adstart || Exten == 0 || number != Extsel)
            {
                return;
            }
            var polarityOk = (Exten == 1 && value) || (Exten == 2 && !value) || Exten == 3;
            if(!polarityOk)
            {
                return;
            }
            if(busy || holding)
            {
                // RM0091 13.5: triggers during a conversion are ignored.
                Stats.TriggersIgnored++;
                return;
            }
            Stats.TriggersAccepted++;
            if(Wait && (isr & IsrEoc) != 0)
            {
                // RM0091 13.7.1: the conversion waits until the data is read.
                holding = true;
                heldStart = () => StartChain(Now());
                return;
            }
            StartChain(now);
        }

        // ---- host port interface (BbAdcHostPort) ---------------------------

        public BbAdcStats Stats { get; private set; }

        // Append a timed input value: from t_ns on, samples strictly after
        // t_ns read uv (ADR-005 D5.2). Returns an error code (0: ok).
        public uint AddTimedInput(int channel, ulong tNs, uint uv)
        {
            if(channel < 0 || channel >= ChannelCount)
            {
                return Miscellaneous.BbAdcHostPort.ErrChannel;
            }
            if(hasSampled[channel] && tNs < sampledAt[channel])
            {
                // The entry would have changed a sample already taken.
                return Miscellaneous.BbAdcHostPort.ErrLate;
            }
            var list = inputs[channel];
            if(list.Count > 0)
            {
                var last = list[list.Count - 1];
                if(tNs < last.Ns)
                {
                    return Miscellaneous.BbAdcHostPort.ErrOrder;
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

        // The value of the channel's latest timed entry (what samples after
        // it read), or uint.MaxValue without one.
        public uint LatestTimedInput(int channel)
        {
            if(channel < 0 || channel >= ChannelCount || inputs[channel].Count == 0)
            {
                return uint.MaxValue;
            }
            return inputs[channel][inputs[channel].Count - 1].Microvolts;
        }

        public ulong NowNs => Now();

        public uint Code12(ulong uv)
        {
            // round_half_up(uv * 4095 / V) = floor((2 uv 4095 + V) / 2V), saturated.
            var v = VrefMicrovolts;
            var code = (2 * uv * FullScale + v) / (2 * v);
            return (uint)Math.Min(code, FullScale);
        }

        // ---- registers ----------------------------------------------------

        private void WriteCr(uint value)
        {
            if((value & CrAdcal) != 0)
            {
                if(aden || adcal)
                {
                    WarnOnceText("adcal", "ADCAL set with ADEN = 1 or during a calibration; ignored (RM0091 13.4.1)");
                }
                else
                {
                    adcal = true;
                    var done = CeilNs(Now(), CalibrationEighths, 0, 0);
                    Schedule(new Event(done, EventKind.CalibrationDone, null));
                }
            }
            if((value & CrAdsto) != 0 && adstart)
            {
                Stop();
            }
            if((value & CrAddis) != 0)
            {
                if(adstart)
                {
                    WarnOnceText("addis", "ADDIS with ADSTART = 1; ignored (RM0091 13.4.2)");
                }
                else if(aden)
                {
                    aden = false;
                }
            }
            if((value & CrAden) != 0 && !aden && !adcal)
            {
                aden = true;
                if(!Autoff)
                {
                    // t_STAB is modelled as 0 (datasheet: typ 0, max 1 us).
                    isr |= IsrAdrdy;
                }
                else
                {
                    WarnOnceText("autoff", "AUTOFF is not modelled: the ADC is always on (no ADRDY)");
                }
            }
            if((value & CrAdstart) != 0 && !adstart)
            {
                if(!aden || adcal)
                {
                    WarnOnceText("adstart", "ADSTART with ADEN = 0 or during calibration; ignored");
                }
                else
                {
                    adstart = true;
                    if(Exten == 0)
                    {
                        // Software trigger: at the instruction's time.
                        SyncCpu();
                        StartChain(Now());
                    }
                }
            }
            UpdateIrq();
        }

        private void CheckConfig()
        {
            if(Exten != 0 && Extsel == 1)
            {
                WarnOnceText("trg1", "EXTSEL = TRG1 (TIM1_CC4) is not supported: no trigger will arrive");
            }
            if(Exten != 0 && Extsel > 4)
            {
                WarnOnceText("trg5", "EXTSEL = TRG5..7 is reserved on STM32F0x2: no trigger will arrive");
            }
            if(Cont && Discen)
            {
                WarnOnceText("contdisc", "CONT = 1 with DISCEN = 1 is forbidden (RM0091 13.4.8); DISCEN wins");
            }
            if(Dmaen && (cfgr1 & Cfgr1Dmacfg) == 0)
            {
                WarnOnceText("dmaoneshot", "DMA one-shot mode (DMACFG = 0) is modelled as circular (ADR-005 D1.4)");
            }
        }

        // ---- conversions --------------------------------------------------

        private void StartChain(ulong t)
        {
            busy = true;
            var seq = Sequence();
            if(seq.Count == 0)
            {
                WarnOnceText("chsel", "conversion started with no channel selected (ADC_CHSELR = 0); nothing converted");
                busy = false;
                if(Exten == 0)
                {
                    adstart = false;
                }
                return;
            }
            if(discPos >= seq.Count)
            {
                discPos = 0;
            }
            chainSeq = seq;
            chainBase = t;
            chainAdcE = LatencyEighths();
            chainTrigger = t;
            StartConversion(Discen ? discPos : 0);
        }

        private void StartConversion(int pos)
        {
            var smplE = SmplEighths[smpr & 7];
            var sarE = SarEighths[Res];
            var a = chainAdcE;
            var conv = new BbAdcConversion
            {
                Channel = chainSeq[pos],
                Pos = pos,
                Trigger = chainTrigger,
                Sample = CeilNs(chainBase, a + smplE, 0, 0),
                End = CeilNs(chainBase, a + smplE + sarE, WriteLatencyAdcEighths(), WriteLatencyPclkEighths()),
                Res = Res,
                LeftAligned = (cfgr1 & Cfgr1Align) != 0,
            };
            // The next conversion of the chain would start back to back.
            chainAdcE = a + smplE + sarE;
            current = conv;
            Schedule(new Event(conv.Sample, EventKind.Sample, conv));
        }

        private void OnSample(BbAdcConversion conv)
        {
            isr |= IsrEosmp;
            conv.Code12 = ReadCode12(conv.Channel, conv.Sample);
            Stats.Samples++;
            var last = conv.Pos == chainSeq.Count - 1;
            conv.EndOfSequence = last;
            int next = -1;
            if(!Discen)
            {
                if(!last)
                {
                    next = conv.Pos + 1;
                }
                else if(Cont)
                {
                    next = 0;
                }
            }
            conv.EndsChain = next < 0;
            if(next >= 0)
            {
                if(Wait)
                {
                    // Held until the data of this conversion is read (13.7.1).
                    var n = next;
                    heldStart = () =>
                    {
                        chainBase = Now();
                        chainAdcE = 0;
                        StartConversion(n);
                    };
                }
                else
                {
                    StartConversion(next);
                }
            }
            Schedule(new Event(conv.End, EventKind.EndOfConversion, conv));
            UpdateIrq();
        }

        private void OnEndOfConversion(BbAdcConversion conv)
        {
            var data = Align(conv.Code12, conv.Res, conv.LeftAligned);
            var overrun = (isr & IsrEoc) != 0;
            if(overrun)
            {
                isr |= IsrOvr;
                if((cfgr1 & Cfgr1Ovrmod) != 0)
                {
                    dr = data;
                }
            }
            else
            {
                dr = data;
            }
            isr |= IsrEoc;
            if(Awden && (!Awdsgl || conv.Channel == Awdch))
            {
                var shift = 12 - ResBits[conv.Res];
                var raw = (conv.Code12 >> shift) << shift;
                var lt = tr & 0xFFF;
                var ht = (tr >> 16) & 0xFFF;
                if(raw < lt || raw > ht)
                {
                    isr |= IsrAwd;
                    conv.Awd = true;
                }
            }
            if(conv.EndOfSequence)
            {
                isr |= IsrEoseq;
            }
            conv.Overrun = overrun;
            conv.Dr = dr;
            Stats.RecordConversion(conv);
            // ADSTART clearing, RM0091 13.4.9.
            if(Exten == 0 && !Cont)
            {
                if(Discen || conv.EndOfSequence)
                {
                    adstart = false;
                }
            }
            if(conv.EndsChain)
            {
                busy = false;
                if(Discen)
                {
                    discPos = conv.EndOfSequence ? 0 : conv.Pos + 1;
                }
                else
                {
                    discPos = 0;
                }
                current = null;
            }
            if(heldStart != null && !holding)
            {
                holding = true;
            }
            UpdateIrq();
            if(Dmaen && dma != null && (isr & IsrOvr) == 0)
            {
                Stats.DmaRequests++;
                dma.RequestTransfer(dmaChannel);
            }
        }

        private void ReleaseHold()
        {
            if(!holding || heldStart == null)
            {
                return;
            }
            SyncCpu();
            holding = false;
            var start = heldStart;
            heldStart = null;
            start();
        }

        private void Stop()
        {
            // RM0091 13.4.11: abort, discard, reset the sequence.
            events.RemoveAll(e => e.Kind != EventKind.CalibrationDone);
            Rearm();
            busy = false;
            holding = false;
            heldStart = null;
            current = null;
            discPos = 0;
            adstart = false;
        }

        private uint ReadCode12(int channel, ulong s)
        {
            ulong uv;
            var list = inputs[channel];
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
                uv = ChannelSourceMicrovolts(channel);
                if(list.Count > 0)
                {
                    // The first timed entry is after this sample.
                    WarnOnceText("early", "a sample before the channel's first timed input reads its channel source");
                }
            }
            if(channel >= 16)
            {
                var enabled = channel == 16 ? (ccr & CcrTsen) != 0 : channel == 17 ? (ccr & CcrVrefen) != 0 : (ccr & CcrVbaten) != 0;
                if(!enabled)
                {
                    WarnOnceText("internal" + channel, $"channel {channel} converted with its source disabled in ADC_CCR; reads 0");
                    uv = 0;
                }
            }
            sampledAt[channel] = s;
            hasSampled[channel] = true;
            return Code12(uv);
        }

        private ulong ChannelSourceMicrovolts(int channel)
        {
            ulong uv = 0;
            if(ADCContainer.TryGetByAddress(channel, out var src))
            {
                uv = src.Sample.Voltage;
            }
            if(uv == 0 && channel >= 16)
            {
                uv = internalDefaults[channel - 16];
            }
            return uv;
        }

        private List<int> Sequence()
        {
            var seq = new List<int>();
            for(var i = 0; i < ChannelCount; i++)
            {
                if((chselr & (1u << i)) != 0)
                {
                    seq.Add(i);
                }
            }
            if((cfgr1 & Cfgr1Scandir) != 0)
            {
                seq.Reverse();
            }
            return seq;
        }

        // RM0091 Figure 37: right aligned, or left aligned at bit 15 (12, 10
        // and 8 bits) or at bits 7:2 (6 bits).
        private static uint Align(uint code12, int res, bool left)
        {
            var bits = ResBits[res];
            var data = code12 >> (12 - bits);
            if(left)
            {
                data <<= bits == 6 ? 2 : 16 - bits;
            }
            return data;
        }

        // ---- timing (D4) ----------------------------------------------------

        // ADC clock: CKMODE 00 the asynchronous clock, 01 PCLK/2, 10 PCLK/4.
        private int Ckmode => (int)(cfgr2 >> 30);

        private int LatencyEighths()
        {
            // tlatr: CKMODE 01 2.75 cycles (5.5 PCLK), 10 2.625 (10.5 PCLK),
            // 00 2.625 (the datasheet's lower bound, no jitter).
            return Ckmode == 1 ? 22 : 21;
        }

        private int WriteLatencyAdcEighths()
        {
            return (Ckmode == 1 || Ckmode == 2) ? 0 : 12; // 00: 1.5 ADC cycles
        }

        private int WriteLatencyPclkEighths()
        {
            switch(Ckmode)
            {
            case 1:
                return 36; // 4.5 PCLK cycles
            case 2:
                return 68; // 8.5 PCLK cycles
            default:
                return 16; // 00: + 2 PCLK cycles
            }
        }

        // base + adcE/8 ADC cycles + pclkE/8 PCLK cycles, rounded up to ns.
        private ulong CeilNs(ulong baseNs, long adcE, int extraAdcE, int pclkE)
        {
            // One ADC cycle: 1e9 / fAsync ns, or div * 1e9 / fPCLK ns.
            BigInteger adcNum, adcDen;
            switch(Ckmode)
            {
            case 1:
                adcNum = 2 * NsPerS;
                adcDen = pclkHz;
                break;
            case 2:
                adcNum = 4 * NsPerS;
                adcDen = pclkHz;
                break;
            default:
                adcNum = NsPerS;
                adcDen = asyncHz;
                break;
            }
            var num = (adcE + extraAdcE) * adcNum * pclkHz + pclkE * NsPerS * adcDen;
            var den = 8 * adcDen * pclkHz;
            var q = BigInteger.DivRem(num, den, out var rem);
            if(rem > 0)
            {
                q += 1;
            }
            return baseNs + (ulong)q;
        }

        // ---- events ---------------------------------------------------------

        private void Schedule(Event e)
        {
            e.Seq = nextSeq++;
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
                this.Log(LogLevel.Error, "bb_stm32f0_adc: event at {0} ns scheduled at {1} ns; fired at the next ns", at, now);
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
                OnTimerInner();
            }
            finally
            {
                inHandler--;
            }
        }

        private void OnTimerInner()
        {
            var now = Now();
            while(events.Count > 0 && events[0].At <= now)
            {
                var e = events[0];
                events.RemoveAt(0);
                switch(e.Kind)
                {
                case EventKind.Sample:
                    OnSample(e.Conv);
                    break;
                case EventKind.EndOfConversion:
                    OnEndOfConversion(e.Conv);
                    break;
                case EventKind.CalibrationDone:
                    adcal = false;
                    dr = calFactor;
                    break;
                }
            }
            Rearm();
        }

        // ---- helpers --------------------------------------------------------

        private ulong Now()
        {
            return clock.CurrentValue.TotalNanoseconds;
        }

        private void SyncCpu()
        {
            // Only for a register access by the CPU itself: never from
            // inside this model's clock event or a trigger edge (a clock
            // handler), where the clock is at the event's exact time.
            if(inHandler == 0 && sysbus.TryGetCurrentCPU(out var cpu))
            {
                cpu.SyncTime();
            }
        }

        private void UpdateIrq()
        {
            IRQ.Set((isr & ier & IsrMask) != 0);
        }

        private void WarnWhileStarted(string reg)
        {
            if(adstart)
            {
                WarnOnceText("started-" + reg, $"{reg} written with ADSTART = 1 (RM0091 13.4.4 forbids it); applied");
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
                this.Log(LogLevel.Warning, "bb_stm32f0_adc: {0}", text);
            }
        }

        private int Exten => (int)((cfgr1 >> 10) & 3);
        private int Extsel => (int)((cfgr1 >> 6) & 7);
        private int Res => (int)((cfgr1 >> 3) & 3);
        private bool Cont => (cfgr1 & Cfgr1Cont) != 0 && !Discen;
        private bool Discen => (cfgr1 & Cfgr1Discen) != 0;
        private bool Wait => (cfgr1 & Cfgr1Wait) != 0;
        private bool Autoff => (cfgr1 & Cfgr1Autoff) != 0;
        private bool Dmaen => (cfgr1 & Cfgr1Dmaen) != 0;
        private bool Awden => (cfgr1 & Cfgr1Awden) != 0;
        private bool Awdsgl => (cfgr1 & Cfgr1Awdsgl) != 0;
        private int Awdch => (int)((cfgr1 >> 26) & 0x1F);

        private readonly IMachine machine;
        private readonly IBusController sysbus;
        private readonly BaseClockSource clock;
        private readonly IDMA dma;
        private readonly int dmaChannel;
        private readonly ulong pclkHz;
        private readonly ulong asyncHz;
        private readonly uint calFactor;
        private readonly uint[] internalDefaults;
        private readonly LimitTimer timer;
        private readonly List<Event> events = new List<Event>();
        private readonly List<TimedInput>[] inputs;
        private readonly ulong[] sampledAt;
        private readonly bool[] hasSampled;
        private readonly bool[] triggerLevel = new bool[TriggerInputs];
        private readonly HashSet<string> warned = new HashSet<string>();
        private long nextSeq;
        private int inHandler;

        private uint isr, ier, cfgr1, cfgr2, smpr, tr, chselr, dr, ccr;
        private bool aden, adstart, adcal;
        private bool busy, holding;
        private Action heldStart;
        private int discPos;
        private List<int> chainSeq = new List<int>();
        private ulong chainBase, chainTrigger;
        private long chainAdcE;
        private BbAdcConversion current;

        public const int ChannelCount = 19;
        public const int TriggerInputs = 8;
        private const ulong FullScale = 4095;
        private static readonly BigInteger NsPerS = 1000000000;
        // SMP: 1.5, 7.5, 13.5, 28.5, 41.5, 55.5, 71.5, 239.5 cycles, in eighths.
        private static readonly int[] SmplEighths = { 12, 60, 108, 228, 332, 444, 572, 1916 };
        // t_SAR: 12.5, 11.5, 9.5, 7.5 cycles for RES = 12, 10, 8, 6 bits.
        private static readonly int[] SarEighths = { 100, 92, 76, 60 };
        private static readonly int[] ResBits = { 12, 10, 8, 6 };
        private const int CalibrationEighths = 83 * 8;

        private const long IsrOffset = 0x00, IerOffset = 0x04, CrOffset = 0x08, Cfgr1Offset = 0x0C,
            Cfgr2Offset = 0x10, SmprOffset = 0x14, TrOffset = 0x20, ChselrOffset = 0x28, DrOffset = 0x40,
            CcrOffset = 0x308;
        private const uint IsrAdrdy = 1u << 0, IsrEosmp = 1u << 1, IsrEoc = 1u << 2, IsrEoseq = 1u << 3,
            IsrOvr = 1u << 4, IsrAwd = 1u << 7, IsrMask = 0x9F;
        private const uint CrAden = 1u << 0, CrAddis = 1u << 1, CrAdstart = 1u << 2, CrAdsto = 1u << 4,
            CrAdcal = 1u << 31;
        private const uint Cfgr1Dmaen = 1u << 0, Cfgr1Dmacfg = 1u << 1, Cfgr1Scandir = 1u << 2,
            Cfgr1Align = 1u << 5,
            Cfgr1Ovrmod = 1u << 12, Cfgr1Cont = 1u << 13, Cfgr1Wait = 1u << 14, Cfgr1Autoff = 1u << 15,
            Cfgr1Discen = 1u << 16, Cfgr1Awdsgl = 1u << 22, Cfgr1Awden = 1u << 23, Cfgr1Mask = 0x7CC1FDFF;
        private const uint CcrVrefen = 1u << 22, CcrTsen = 1u << 23, CcrVbaten = 1u << 24;

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
            EndOfConversion,
            CalibrationDone,
        }

        private class Event
        {
            public Event(ulong at, EventKind kind, BbAdcConversion conv)
            {
                At = at;
                Kind = kind;
                Conv = conv;
            }

            public ulong At;
            public long Seq;
            public EventKind Kind;
            public BbAdcConversion Conv;
        }
    }

    public class BbAdcConversion
    {
        public int Channel;
        public int Pos;
        public int Res;
        public bool LeftAligned;
        public ulong Trigger;
        public ulong Sample;
        public ulong End;
        public uint Code12;
        public uint Dr;
        public bool EndOfSequence;
        public bool EndsChain;
        public bool Awd;
        public bool Overrun;
    }

    public class BbAdcStats
    {
        public uint Samples;
        public uint Conversions;
        public uint TriggersAccepted;
        public uint TriggersIgnored;
        public uint DmaRequests;
        public uint EdgeCount;
        public readonly uint[] RisingEdges = new uint[BbStm32F0Adc.TriggerInputs];
        public readonly BbAdcConversion[] Ring = new BbAdcConversion[RingSize];
        public readonly ulong[] EdgeRing = new ulong[RingSize];

        public void RecordConversion(BbAdcConversion c)
        {
            Ring[Conversions % RingSize] = c;
            Conversions++;
        }

        public void RecordEdge(int input, bool rising, ulong ns)
        {
            if(rising)
            {
                RisingEdges[input]++;
            }
            EdgeRing[EdgeCount % RingSize] = (ns & 0x00FFFFFFFFFFFFFFul) | ((ulong)input << 56) | (rising ? 1ul << 63 : 0);
            EdgeCount++;
        }

        public const int RingSize = 64;
    }
}

namespace Antmicro.Renode.Peripherals.Miscellaneous
{
    using Antmicro.Renode.Peripherals.Analog;

    // The host-only port of a BbStm32F0Adc (ADR-005 D5), mapped at an unused
    // address (default 0x5FFF0100). Only the Breadboard engine (External
    // Control SYSTEM_BUS) uses it; firmware accesses are ignored and logged.
    //
    // Reads (dword offsets):
    //   0x000 ID 0x50414242 ("BBAP")
    //   0x004 ERROR, sticky: bit 0 a timed input earlier than a sample
    //         already taken on its channel (late), 1 out of time order,
    //         2 bad batch framing, 3 firmware access, 4 bad channel.
    //         A host write of any value to 0x004 clears it.
    //   0x008 / 0x00C  t_ns of the first error, low / high; 0x010 its channel
    //   0x014 timed inputs accepted        0x018 conversions (EOC)
    //   0x01C samples                      0x020 triggers accepted
    //   0x024 triggers ignored             0x028 DMA requests
    //   0x02C trigger-input edges (rising and falling, every input)
    //   0x030 + 4n  rising edges on TRGn, n = 0..7
    //   0x058 / 0x05C  the ADC's clock time in ns, low / high
    //   0x100 + 4ch  the latest timed input of channel ch (0..18), uV, or
    //                0xFFFFFFFF without one
    //   0x200 + 32j  conversion ring, entry j = k mod 64 of conversion k:
    //                T lo, T hi, s lo, s hi, c lo, c hi,
    //                channel | AWD << 8 | OVR << 9 | EOSEQ << 10 | RES << 12,
    //                code12 | ADC_DR << 16
    //   0xA00 + 8j   trigger-edge ring, entry j = k mod 64 of edge k:
    //                t_ns (56 bits) | input << 56 | rising << 63, low / high
    // Writes:
    //   0x1000-0x1FFF  the batch stream, in order, whatever the offset:
    //                  header 0xBA7C0000 | n, then n entries of 3 dwords:
    //                  t_ns low, t_ns high (16 bits) | channel << 16, uV.
    public class BbAdcHostPort : IDoubleWordPeripheral, IKnownSize
    {
        public BbAdcHostPort(IMachine machine, BbStm32F0Adc adc)
        {
            this.machine = machine;
            this.adc = adc;
        }

        public long Size => 0x2000;

        public void Reset()
        {
            // Inputs and statistics live in the ADC; the port keeps only the
            // error word and the stream decoder.
            error = 0;
            firstErrorNs = 0;
            firstErrorChannel = 0;
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
            var st = adc.Stats;
            if(offset >= 0x100 && offset < 0x100 + 4 * BbStm32F0Adc.ChannelCount)
            {
                return adc.LatestTimedInput((int)((offset - 0x100) / 4));
            }
            if(offset >= 0x200 && offset < 0x200 + 32 * BbAdcStats.RingSize)
            {
                var c = st.Ring[(offset - 0x200) / 32];
                if(c == null)
                {
                    return 0;
                }
                switch((offset - 0x200) % 32 / 4)
                {
                case 0: return (uint)c.Trigger;
                case 1: return (uint)(c.Trigger >> 32);
                case 2: return (uint)c.Sample;
                case 3: return (uint)(c.Sample >> 32);
                case 4: return (uint)c.End;
                case 5: return (uint)(c.End >> 32);
                case 6: return (uint)c.Channel | (c.Awd ? 1u << 8 : 0) | (c.Overrun ? 1u << 9 : 0) | (c.EndOfSequence ? 1u << 10 : 0) | ((uint)c.Res << 12);
                default: return c.Code12 | (c.Dr << 16);
                }
            }
            if(offset >= 0xA00 && offset < 0xA00 + 8 * BbAdcStats.RingSize)
            {
                var e = st.EdgeRing[(offset - 0xA00) / 8];
                return (offset % 8 == 0) ? (uint)e : (uint)(e >> 32);
            }
            if(offset >= 0x30 && offset < 0x30 + 4 * BbStm32F0Adc.TriggerInputs)
            {
                return st.RisingEdges[(offset - 0x30) / 4];
            }
            switch(offset)
            {
            case 0x000: return 0x50414242;
            case 0x004: return error;
            case 0x008: return (uint)firstErrorNs;
            case 0x00C: return (uint)(firstErrorNs >> 32);
            case 0x010: return firstErrorChannel;
            case 0x014: return accepted;
            case 0x018: return st.Conversions;
            case 0x01C: return st.Samples;
            case 0x020: return st.TriggersAccepted;
            case 0x024: return st.TriggersIgnored;
            case 0x028: return st.DmaRequests;
            case 0x02C: return st.EdgeCount;
            case 0x058: return (uint)adc.NowNs;
            case 0x05C: return (uint)(adc.NowNs >> 32);
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
            if(offset == 0x004)
            {
                error = 0;
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
            var ch = (int)(entry[1] >> 16);
            var err = adc.AddTimedInput(ch, t, entry[2]);
            if(err != 0)
            {
                Error(err, t, (uint)ch);
            }
            else
            {
                accepted++;
            }
        }

        private void Error(uint bit, ulong t, uint ch)
        {
            if(error == 0)
            {
                firstErrorNs = t;
                firstErrorChannel = ch;
                this.Log(LogLevel.Warning, "bb_adc_host_port: error 0x{0:X} (t {1} ns, channel {2})", bit, t, ch);
            }
            error |= bit;
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
                this.Log(LogLevel.Warning, "bb_adc_host_port: firmware {0} at offset 0x{1:X} ignored (the port is for the Breadboard engine only)", what, offset);
            }
            error |= ErrFirmware;
        }

        public const uint ErrLate = 1u << 0, ErrOrder = 1u << 1, ErrFraming = 1u << 2, ErrFirmware = 1u << 3,
            ErrChannel = 1u << 4;
        private const uint BatchMagic = 0xBA7C0000;

        private readonly IMachine machine;
        private readonly BbStm32F0Adc adc;
        private readonly uint[] entry = new uint[3];
        private uint error;
        private ulong firstErrorNs;
        private uint firstErrorChannel;
        private uint remaining;
        private int entryWord;
        private uint accepted;
    }
}
