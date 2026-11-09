// Breadboard STM32 timer with a trigger output (ADR-005 D1.2), for Renode
// 1.17.0 only.
//
// Renode's STM32_Timer has no TRGO: CR2.MMS is a 2-bit tag at bit 4 that is
// never stored (RM0091 18.4.2 puts MMS[2:0] at bits 6:4). BbStm32Timer is
// STM32_Timer plus that output, and nothing else:
//
//   - CR2.MMS[2:0] is kept and read back. The base model never sees the MMS
//     bits (they are masked out of its CR2 write), so it logs no tag warning.
//   - TRGO, a GPIO, is pulsed (Blink: set, then unset at the same instant)
//       MMS = 010 (update): at every update event, i.e. every LimitReached
//                 unless CR1.UDIS, and every EGR.UG write unless CR1.UDIS
//                 (RM0091: UG generates an update event);
//       MMS = 000 (reset):  at every EGR.UG write ("the UG bit is used as
//                 trigger output");
//       MMS = 001, 011, 1xx: not modelled. A warning once, no pulse.
//   - Counting is the stock model's, unchanged: the update period is ARR
//     ticks, not ARR + 1 (ADR-003 Amendment 5 fact 5), at the repl's fixed
//     frequency (10 MHz on stm32f0.repl), whatever RCC says. ADR-005
//     approver decision 5 leaves both alone.
//
// The TRGO pulse of an update runs inside the clock source's LimitReached
// handler, at the update's exact virtual time (the limit), after the base
// model's own handler (registered first, in the base constructor). A
// receiver (BbStm32F0Adc) therefore sees the trigger at that instant.
using System;
using Antmicro.Renode.Core;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.Bus;

namespace Antmicro.Renode.Peripherals.Timers
{
    [AllowedTranslations(AllowedTranslation.ByteToDoubleWord | AllowedTranslation.WordToDoubleWord)]
    public class BbStm32Timer : STM32_Timer, IDoubleWordPeripheral
    {
        public BbStm32Timer(IMachine machine, ulong frequency, uint initialLimit) : base(machine, frequency, initialLimit)
        {
            TRGO = new GPIO();
            LimitReached += () =>
            {
                if(UpdateDisabled())
                {
                    return;
                }
                if(mms == MmsUpdate)
                {
                    Pulse();
                }
            };
        }

        public GPIO TRGO { get; }

        // Number of TRGO pulses since reset (wraps), for tests: `timerN TrgoPulses`.
        public uint TrgoPulses => trgoPulses;

        public new uint ReadDoubleWord(long offset)
        {
            var v = base.ReadDoubleWord(offset);
            if(offset == Cr2Offset)
            {
                v = (v & ~MmsMask) | (mms << MmsShift);
            }
            return v;
        }

        public new void WriteDoubleWord(long offset, uint value)
        {
            if(offset == Cr2Offset)
            {
                mms = (value & MmsMask) >> MmsShift;
                if(mms != MmsReset && mms != MmsUpdate && !warnedMms)
                {
                    warnedMms = true;
                    this.Log(LogLevel.Warning, "bb_stm32_timer: MMS = {0} is not modelled (only 000 reset and 010 update drive TRGO); no trigger output", Convert.ToString(mms, 2).PadLeft(3, '0'));
                }
                value &= ~MmsMask;
            }
            // The update-disable state before the write decides for UG.
            var udis = UpdateDisabled();
            base.WriteDoubleWord(offset, value);
            if(offset == EgrOffset && (value & EgrUg) != 0)
            {
                if(mms == MmsReset || (mms == MmsUpdate && !udis))
                {
                    Pulse();
                }
            }
        }

        public override void Reset()
        {
            base.Reset();
            mms = 0;
            warnedMms = false;
            trgoPulses = 0;
            // The base constructor calls Reset before TRGO exists.
            TRGO?.Unset();
        }

        private bool UpdateDisabled()
        {
            return (base.ReadDoubleWord(Cr1Offset) & Cr1Udis) != 0;
        }

        private void Pulse()
        {
            trgoPulses++;
            TRGO.Blink();
        }

        private uint mms;
        private bool warnedMms;
        private uint trgoPulses;

        private const long Cr1Offset = 0x00;
        private const long Cr2Offset = 0x04;
        private const long EgrOffset = 0x14;
        private const uint Cr1Udis = 1u << 1;
        private const uint EgrUg = 1u << 0;
        private const int MmsShift = 4;
        private const uint MmsMask = 7u << MmsShift;
        private const uint MmsReset = 0;
        private const uint MmsUpdate = 2;
    }
}
