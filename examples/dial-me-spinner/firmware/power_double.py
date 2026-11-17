# A test double of the nRF52840's POWER registers for the W-fw Renode-only test (bb_wfw_test.py).
#
# Renode 1.17.0's NRF_CLOCK, at the POWER/CLOCK base 0x40000000 of the nRF52840 platform, has no
# POFCON, EVENTS_POFWARN or RESETREAS (ADR-009 Context): it ignores the writes and reads 0. Until
# P4's BbNrf52Power replaces it (ADR-009 D6), this double gives the W1 and W2 firmware those
# registers through a write hook and a read hook on sysbus.clock, so the test can check each
# application's POWER configuration and how it acts on POFWARN:
#
#   POFCON (0x510), the POFWARN bit of the shared INTENSET/INTENCLR (0x304/0x308, bit 2) and
#   EVENTS_POFWARN (0x108) as ADR-009 D6 states them: a comparator input (the test's control word)
#   raises EVENTS_POFWARN on its rising edge while POFCON.POF = 1, and also when POF is enabled
#   with the input already high ("already below at enable"); the POWER_CLOCK interrupt (IRQ 0) is
#   pended while EVENTS_POFWARN and INTEN.POFWARN are both set. RESETREAS (0x400) reads a preset
#   value and is cleared by writing ones. Every other offset is left to NRF_CLOCK.
#
#   Control words (offsets the nRF52840 does not decode, written by the test over External
#   Control while Renode is paused at a lattice point): 0xF00 the comparator input (0/1), 0xF04
#   the RESETREAS preset.
#
# Every POWER access is journalled as "<t_ns> <W|R> <offset> <value>" (the time from the
# machine's clock, as the Renode engine's PIN_CNF hook reads it), one line per access, into the
# file the session names. It is not P4's model: it has no thresholds, no hysteresis, no timing of
# its own and no refusal of codes; P4's register and timing tests are the model's gates.
import System

POFWARN_EV, INTENSET, INTENCLR, RESETREAS, POFCON = 0x108, 0x304, 0x308, 0x400, 0x510
CTRL_COMP, CTRL_RESETREAS = 0xF00, 0xF04
INT_POFWARN = 1 << 2
NVIC_ISPR0 = 0xE000E200
JOURNALLED = (POFWARN_EV, INTENSET, INTENCLR, RESETREAS, POFCON, 0x51C, 0x520, 0x078, 0x07C, 0x500)


class PowerDouble(object):
    def __init__(self, journal):
        self.journal = journal
        self.pofcon = 0
        self.inten = 0
        self.event = 0
        self.comp = 0
        self.resetreas = 0

    def _log(self, machine, kind, offset, value):
        t = machine.LocalTimeSource.ElapsedVirtualTime.Ticks
        f = open(self.journal, "a")
        f.write("%d %s %#x %#x\n" % (t, kind, offset, value))
        f.close()

    def _raise(self, machine):
        self.event = 1
        self._update(machine)

    def _update(self, machine):
        if self.event and self.inten:
            machine.SystemBus.WriteDoubleWord(NVIC_ISPR0, 1)

    def write(self, machine, offset, value):
        offset, value = int(offset), int(value)
        if offset == CTRL_COMP:
            rising = value and not self.comp
            self.comp = 1 if value else 0
            self._log(machine, "C", offset, self.comp)
            if rising and self.pofcon & 1:
                self._raise(machine)
            return
        if offset == CTRL_RESETREAS:
            self.resetreas = value
            return
        if offset not in JOURNALLED:
            return
        self._log(machine, "W", offset, value)
        if offset == POFCON:
            was = self.pofcon
            self.pofcon = value & 0x1F
            # D6: POF enabled, or its threshold changed while enabled, with the input already high
            if self.pofcon & 1 and self.comp and (not was & 1 or (was ^ self.pofcon) & 0x1E):
                self._raise(machine)
        elif offset == INTENSET and value & INT_POFWARN:
            self.inten = 1
            self._update(machine)
        elif offset == INTENCLR and value & INT_POFWARN:
            self.inten = 0
        elif offset == POFWARN_EV:
            self.event = value & 1
            self._update(machine)
        elif offset == RESETREAS:
            self.resetreas &= ~value

    def read(self, machine, offset, value):
        offset, value = int(offset), int(value)
        if offset == POFWARN_EV:
            value = self.event
        elif offset in (INTENSET, INTENCLR):
            value = (value & ~INT_POFWARN) | (INT_POFWARN if self.inten else 0)
        elif offset == RESETREAS:
            value = self.resetreas
        elif offset == POFCON:
            value = self.pofcon
        else:
            return value
        self._log(machine, "R", offset, value)
        return value


def install(journal):
    System.AppDomain.CurrentDomain.SetData("bb_power_double", PowerDouble(journal))
