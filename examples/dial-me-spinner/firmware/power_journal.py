# The POWER access journal of the W-fw Renode-only test (bb_wfw_test.py), on Breadboard's
# BbNrf52Power (ADR-009 D6, Phase 2 P4; it replaced W-fw's test double power_double.py, ADR-009
# Amendment 7).
#
# An observer only: a write hook and a read hook on sysbus.clock (the POWER/CLOCK peripheral,
# BbNrf52Power on nrf52840-bb@4) that journal every access to the POWER registers the W1 and W2
# firmware use and pass every value through unchanged, so the model alone decides what the
# firmware sees. One line per access, "<t_ns> <W|R> <offset> <value>", the time from the
# machine's clock as the Renode engine's PIN_CNF hook reads it, into the file the session names.
#
#   POFCON (0x510), INTENSET/INTENCLR (0x304/0x308), EVENTS_POFWARN (0x108), RESETREAS (0x400),
#   GPREGRET/GPREGRET2 (0x51C/0x520), TASKS_CONSTLAT/LOWPWR (0x078/0x07C), SYSTEMOFF (0x500).
#
# The comparator input is the model's GPIO input 0 (#pofwarn), which the test sets over External
# Control at lattice points as the engine applies a digital_in; RESETREAS's preset is the model's
# host register 0xF04.
import System

JOURNALLED = (0x108, 0x304, 0x308, 0x400, 0x510, 0x51C, 0x520, 0x078, 0x07C, 0x500)


class PowerJournal(object):
    def __init__(self, journal):
        self.journal = journal

    def _log(self, machine, kind, offset, value):
        t = machine.LocalTimeSource.ElapsedVirtualTime.Ticks
        f = open(self.journal, "a")
        f.write("%d %s %#x %#x\n" % (t, kind, offset, value))
        f.close()

    def write(self, machine, offset, value):
        offset, value = int(offset), int(value)
        if offset in JOURNALLED:
            self._log(machine, "W", offset, value)

    def read(self, machine, offset, value):
        o, v = int(offset), int(value)
        if o in JOURNALLED:
            self._log(machine, "R", o, v)
        return value


def install(journal):
    System.AppDomain.CurrentDomain.SetData("bb_power_journal", PowerJournal(journal))
