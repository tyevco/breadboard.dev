#!/usr/bin/env python3
"""Renode-only tests of Breadboard's nRF52840 PWM, TEMP and NVMC models (ADR-008 D9, Amendment 9).

Drives Renode 1.17.0 with Breadboard's nRF52840 (engines/cpu-renode/platforms/
nrf52840-bb.repl, nrf52840-bb@4: BbNrf52Pwm PWM0-PWM3, BbNrf52Temp,
BbNrf52Nvmc and their host port BbNrf52HostPort at 0x5FFC0000) through the
External Control API, paused, as the Renode engine does, with the session
helpers of examples/nrf-saadc/bb_saadc_test.py. The shape is ADR-005's test
plan 2 and 3. Every expected value here is computed independently from the
PS rules the ADR states, never read back from the model.

Groups:

  registers  (nrf_saadc_idle.elf: WFI, no interrupt; the host drives the
             models): reset values of the four PWMs, TEMP, NVMC and the port;
             every pin change of the pin ring (exact times in half-ns, levels,
             connect and release) for up and up-and-down mode x PRESCALER 0,
             1, 3, 7 x COUNTERTOP 3, 640, 1000 x COMP 0, 1, top/3, top - 1,
             top, top + 5, top/2, 2 at both polarities, and STOP at the end of
             the period; the sequencer: REFRESH, ENDDELAY, LOOP chaining
             SEQ[0] -> SEQ[1], every SEQSTARTED / SEQEND / PWMPERIODEND /
             LOOPSDONE / STOPPED instant (set at its ns, not 1 ns earlier), the
             shorts LOOPSDONE_SEQSTART0, SEQEND0_STOP and LOOPSDONE_STOP,
             NextStep mode, the Grouped, Common and WaveForm decoders (the
             per-step COUNTERTOP, channel 3 not driven), a PSEL change while
             running, ENABLE = 0 while running, STOP while idle, the IRQ lines
             of PWM0-3 (NVIC pending bits 28, 33, 34, 45), the ring overflow;
             TEMP: DATARDY at START + 36 us, the host value, STOP cancels,
             IRQ 12; NVMC: ERASEPAGE (0xFF at once, READY low for 85 ms), the
             CONFIG gating and the page check, ERASEPAGEPARTIAL accumulation
             (10 ms each, erased at the ninth), ERASEALL (169 ms),
             ICACHECNF. Run twice: the transcripts must be byte-identical.
  firmware   (nrf_pwm_step.elf, lookahead block armed): TEMP through its
             interrupt, an NVMC page erase polled on READY (RTC0 ticks across
             it), and PWM0 microstepping 4 full steps at 446 PPS as
             nrfx_pwm_complex_playback runs it (two ping-pong sequences,
             LOOPSDONE_SEQSTART0, SEQEND refills): every pin change against
             the duty table the firmware computes, the UART lines, and no
             lookahead promise (WFI, nearest clock limit) past the next PWM
             pin change.

Usage: bb_pwm_test.py [--renode PATH] [--out DIR] [--keep] [GROUP ...]
Exit status 0 on success.
"""

import argparse
import struct
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
sys.path.insert(0, str(ROOT / "examples/nrf-saadc"))
from bb_saadc_test import Env, Failures, find_renode  # noqa: E402

FW = ROOT / "engines/cpu-renode/tests/firmware/nrf52840/build"
L_NS = 10_000
US, MS = 1_000, 1_000_000

PWM = [0x4001C000, 0x40021000, 0x40022000, 0x4002D000]
PWM_IRQ = [28, 33, 34, 45]
STOP, SEQSTART0, SEQSTART1, NEXTSTEP = 0x004, 0x008, 0x00C, 0x010
EV = {"STOPPED": 0x104, "SEQSTARTED0": 0x108, "SEQSTARTED1": 0x10C, "SEQEND0": 0x110, "SEQEND1": 0x114,
      "PWMPERIODEND": 0x118, "LOOPSDONE": 0x11C}
SHORTS, INTEN, INTENSET, INTENCLR = 0x200, 0x300, 0x304, 0x308
ENABLE, MODE, COUNTERTOP, PRESCALER, DECODER, LOOP = 0x500, 0x504, 0x508, 0x50C, 0x510, 0x514
SEQ = [0x520, 0x540]  # + 0 PTR, 4 CNT, 8 REFRESH, 0xC ENDDELAY
PSEL = 0x560
SH_SEQEND0_STOP, SH_SEQEND1_STOP, SH_LD_SEQSTART0, SH_LD_SEQSTART1, SH_LD_STOP = 1, 2, 4, 8, 16
COMMON, GROUPED, INDIVIDUAL, WAVEFORM = 0, 1, 2, 3
NEXTSTEP_MODE = 0x100

TEMP = 0x4000C000
NVMC = 0x4001E000
PORT = 0x5FFC0000
LOOKAHEAD = 0x5FFF0000
UARTLOG = 0x5FFF8000
NVIC_ISPR, NVIC_ICPR = 0xE000E200, 0xE000E280
BUF = 0x20010000
RING = 8192
LEVEL, CONNECT, RELEASE = 0, 1, 2
TICK = 125  # half-nanoseconds per 16 MHz tick


class Pwm:
    """The host's view of one PWM instance and the shared pin ring."""

    def __init__(self, s, inst=0):
        self.s, self.inst, self.base = s, inst, PWM[inst]

    def w(self, off, v):
        self.s.w(self.base + off, v)

    def r(self, off):
        return self.s.r(self.base + off)

    def ev(self, name):
        return self.r(EV[name])

    def clear_events(self):
        for off in EV.values():
            self.w(off, 0)


def ring(s):
    """New pin-ring entries since the last call, acknowledged:
    [(t_hns, instance, channel, kind, level, pin)]."""
    n = s.r(PORT + 0x08)
    seen = getattr(s, "ring_seen", 0)
    out = []
    k = seen
    while k < n:
        j = k % RING
        m = min(n - k, RING - j, 512)
        words = s.r(PORT + 0x1000 + 8 * j, 2 * m)
        for i in range(m):
            lo, hi = words[2 * i], words[2 * i + 1]
            out.append((lo | (hi & 0xFFFF) << 32, hi >> 21 & 3, hi >> 19 & 3, hi >> 17 & 3, hi >> 16 & 1,
                        hi >> 24 & 0x3F))
        k += m
    s.w(PORT + 0x0C, n & 0xFFFFFFFF)
    s.ring_seen = n
    return out


def put_halfwords(s, addr, vals):
    if len(vals) % 2:
        vals = vals + [0]
    for k in range(0, len(vals), 2):
        s.w(addr + 2 * k, vals[k] | vals[k + 1] << 16)


def expected(t0, periods, pins, inst=0, release=None, wave=False):
    """The pin changes of a run (ADR-008 Amendment 9, from the PS rules):
    periods = [(values[4], top, updown, prescaler)], pins[c] (None: not
    connected); a release of every driven channel at `release` (half-ns)."""
    out, lvl, t = [], [None] * 4, t0
    for vals, top, updown, presc in periods:
        tick = TICK << presc
        for c in range(4):
            if pins[c] is None or (wave and c == 3):
                continue
            comp, hi = vals[c] & 0x7FFF, bool(vals[c] & 0x8000)
            edges = []
            if comp == 0:
                start = not hi
            else:
                start = hi
                if comp < top:
                    edges.append((t + comp * tick, not hi))
                    if updown:
                        edges.append((t + (2 * top - comp) * tick, hi))
            if lvl[c] is None:
                out.append((t, inst, c, CONNECT, int(start), pins[c]))
            elif lvl[c] != start:
                out.append((t, inst, c, LEVEL, int(start), pins[c]))
            lvl[c] = start
            for te, lv in edges:
                out.append((te, inst, c, LEVEL, int(lv), pins[c]))
                lvl[c] = lv
        t += (2 * top if updown else top) * tick
    if release is not None:
        for c in range(4):
            if lvl[c] is not None:
                out.append((release, inst, c, RELEASE, int(lvl[c]), pins[c]))
    return sorted(out)


def period_hns(top, updown=False, presc=0):
    return (2 * top if updown else top) * (TICK << presc)


def to_lattice(s, extra=3 * US + 7):
    s.run_to((s.now // L_NS + 1) * L_NS + extra)


def configure(p, decoder, seqs, loop=0, shorts=0, top=1000, mode=0, presc=0, pins=(2, 3, 4, 5)):
    """seqs: [(values, refresh, enddelay)] for SEQ[0] (and SEQ[1])."""
    s = p.s
    p.w(ENABLE, 1)
    p.w(MODE, mode)
    p.w(PRESCALER, presc)
    p.w(COUNTERTOP, top)
    p.w(DECODER, decoder)
    p.w(LOOP, loop)
    p.w(SHORTS, shorts)
    for c in range(4):
        p.w(PSEL + 4 * c, pins[c] if pins[c] is not None else 0xFFFFFFFF)
    for n, (vals, refresh, enddelay) in enumerate(seqs):
        addr = BUF + 0x1000 * n
        put_halfwords(s, addr, vals)
        p.w(SEQ[n] + 0x0, addr)
        p.w(SEQ[n] + 0x4, len(vals))
        p.w(SEQ[n] + 0x8, refresh)
        p.w(SEQ[n] + 0xC, enddelay)
    p.clear_events()


def check_events(s, p, f, t0, expect, label):
    """expect: [(t_hns, name)], every occurrence of each name in the window.
    An event is set at its instant's ns (rounded down), not 1 ns before."""
    names = sorted({n for _, n in expect})
    times = sorted({t // 2 for t, _ in expect})
    bad = []
    for tn in times:
        if tn > s.now:
            s.run_to(tn - 1)
            for n in names:
                if p.ev(n):
                    bad.append(f"{n} set before {tn - t0 // 2} ns")
                    p.w(EV[n], 0)
            s.run_to(tn)
        due = sorted({n for t, n in expect if t // 2 == tn})
        for n in names:
            got = p.ev(n)
            if got != (n in due):
                bad.append(f"{n} {'missing' if n in due else 'set'} at {tn - t0 // 2} ns")
            p.w(EV[n], 0)
    f.check(not bad, f"{label}: events {bad[:6]}")
    return bad


def group_registers(env, f):
    print("registers: PWM waveforms and sequencer, TEMP, NVMC, the pin ring")
    out = []
    s = env.session(FW / "nrf_saadc_idle.elf")
    try:
        # Reset values (PS reset values; the port's ID).
        got = {}
        for i, b in enumerate(PWM):
            got[f"PWM{i}"] = (s.r(b + COUNTERTOP), s.r(b + SEQ[0] + 8), s.r(b + SEQ[1] + 8), s.r(b + PSEL),
                              s.r(b + PSEL + 12), s.r(b + ENABLE), s.r(b + DECODER), s.r(b + LOOP))
        got["TEMP"] = tuple(s.r(TEMP + o) for o in (0x520, 0x534, 0x540, 0x554, 0x560, 0x570, 0x508))
        got["NVMC"] = tuple(s.r(NVMC + o) for o in (0x400, 0x408, 0x504, 0x51C, 0x540))
        got["PORT"] = (s.r(PORT), s.r(PORT + 4), s.r(PORT + 8), s.r(PORT + 0x40))
        want = {f"PWM{i}": (0x3FF, 1, 1, 0xFFFFFFFF, 0xFFFFFFFF, 0, 0, 0) for i in range(4)}
        want["TEMP"] = (0x326, 0x5A3, 0x3FEF, 0x027C, 0xE2, 0x50, 0)
        want["NVMC"] = (1, 1, 0, 0x0A, 0)
        want["PORT"] = (0x504E4242, 0, 0, 100)
        f.check(got == want, f"reset values {got}, expected {want}")
        out.append(f"reset {sorted(got.items())}")
        p = Pwm(s)

        # Waveforms: one Individual step held, then STOP.
        cases = 0
        for updown in (0, 1):
            for presc in (0, 1, 3, 7):
                for top in (3, 640, 1000):
                    for group in range(2):
                        if group == 0:
                            comps, pols = [0, 1, top // 3, top - 1], [1, 0, 1, 0]
                        else:
                            comps, pols = [top, top + 5, top // 2, 2], [0, 1, 1, 0]
                        vals = [c | (0x8000 if pol else 0) for c, pol in zip(comps, pols)]
                        to_lattice(s)
                        configure(p, INDIVIDUAL, [(vals, 0, 0)], top=top, mode=updown, presc=presc)
                        ring(s)
                        t0 = 2 * s.now
                        p.w(SEQSTART0, 1)
                        P = period_hns(top, updown, presc)
                        ts = t0 + 2 * P + P // 3          # STOP inside period 2
                        s.run_to(ts // 2)
                        p.w(STOP, 1)
                        stop = t0 + 3 * P
                        s.run_to(stop // 2 - 1)
                        early = p.ev("STOPPED")
                        s.run_to(stop // 2)
                        got = ring(s)
                        exp = expected(t0, [(vals, top, updown, presc)] * 3, [2, 3, 4, 5], release=stop)
                        ok = sorted(got) == exp and early == 0 and p.ev("STOPPED") == 1
                        f.check(ok, f"waveform updown={updown} presc={presc} top={top} vals={vals}: "
                                f"got {[(e[0] - t0,) + e[1:] for e in sorted(got)][:12]} expected "
                                f"{[(e[0] - t0,) + e[1:] for e in exp][:12]}, STOPPED {early}->{p.ev('STOPPED')}")
                        out.append(f"wave updown={updown} presc={presc} top={top} vals={vals} "
                                   f"{[(e[0] - t0, e[2], e[3], e[4]) for e in sorted(got)]}")
                        cases += 1
        print(f"  {cases} waveform cases (mode x PRESCALER x COUNTERTOP x COMP x POLARITY): every pin change "
              "exact to 0.5 ns, STOP at the period's end")

        # The sequencer: REFRESH, ENDDELAY, LOOP 2, no shorts.
        def steps(base):
            return [(base + 10 * c) | 0x8000 for c in range(4)]
        A, B, C, D, E = (steps(b) for b in (100, 200, 300, 400, 500))
        top = 1000
        P = period_hns(top)
        to_lattice(s)
        configure(p, INDIVIDUAL, [(A + B + C, 2, 1), (D + E, 0, 0)], loop=2, top=top)
        ring(s)
        t0 = 2 * s.now
        p.w(SEQSTART0, 1)
        plan = [A] * 3 + [B] * 3 + [C] * 4 + [D, E]
        plan = plan + plan + [E] * 3
        evs = [(t0 + k * P, "PWMPERIODEND") for k in range(1, 27)]
        evs += [(t0, "SEQSTARTED0"), (t0 + 12 * P, "SEQSTARTED0"), (t0 + 6 * P, "SEQEND0"), (t0 + 18 * P, "SEQEND0"),
                (t0 + 10 * P, "SEQSTARTED1"), (t0 + 22 * P, "SEQSTARTED1"), (t0 + 11 * P, "SEQEND1"),
                (t0 + 23 * P, "SEQEND1"), (t0 + 24 * P, "LOOPSDONE")]
        bad = check_events(s, p, f, t0, evs, "sequencer LOOP 2")
        s.run_to((t0 + 26 * P + P // 2) // 2)
        p.w(STOP, 1)
        s.run_to((t0 + 27 * P) // 2)
        exp = expected(t0, [(v, top, 0, 0) for v in plan[:27]], [2, 3, 4, 5], release=t0 + 27 * P)
        got = sorted(ring(s))
        f.check(got == exp, f"sequencer LOOP 2: pin changes differ (first got {got[:4]}, expected {exp[:4]})")
        out.append(f"seq loop2 events_bad={bad} entries={len(got)}")

        # LOOPSDONE_SEQSTART0 (nrfx_pwm_complex_playback with NRFX_PWM_FLAG_LOOP).
        to_lattice(s)
        configure(p, INDIVIDUAL, [(A, 0, 0), (D, 0, 0)], loop=1, shorts=SH_LD_SEQSTART0, top=top)
        ring(s)
        t0 = 2 * s.now
        p.w(SEQSTART0, 1)
        evs = [(t0 + k * P, "PWMPERIODEND") for k in range(1, 9)]
        evs += [(t0 + k * P, n) for k in (0, 2, 4, 6, 8) for n in ("SEQSTARTED0", "SEQEND0")]
        evs += [(t0 + k * P, n) for k in (1, 3, 5, 7) for n in ("SEQSTARTED1", "SEQEND1")]
        evs += [(t0 + k * P, "LOOPSDONE") for k in (2, 4, 6, 8)]
        bad = check_events(s, p, f, t0, evs, "LOOPSDONE_SEQSTART0")
        s.run_to((t0 + 8 * P + P // 2) // 2)
        p.w(SHORTS, 0)
        p.w(STOP, 1)
        s.run_to((t0 + 9 * P) // 2)
        exp = expected(t0, [(A if k % 2 == 0 else D, top, 0, 0) for k in range(9)], [2, 3, 4, 5], release=t0 + 9 * P)
        got = sorted(ring(s))
        f.check(got == exp, "LOOPSDONE_SEQSTART0: pin changes differ")
        out.append(f"seq ping-pong events_bad={bad} entries={len(got)}")

        # SEQEND0_STOP: the last step plays one period.
        to_lattice(s)
        configure(p, INDIVIDUAL, [(A + B, 3, 0)], shorts=SH_SEQEND0_STOP, top=top)
        ring(s)
        t0 = 2 * s.now
        p.w(SEQSTART0, 1)
        evs = [(t0 + k * P, "PWMPERIODEND") for k in range(1, 6)] + [(t0 + 4 * P, "SEQEND0"),
                                                                    (t0 + 5 * P, "STOPPED")]
        bad = check_events(s, p, f, t0, evs, "SEQEND0_STOP")
        exp = expected(t0, [(A, top, 0, 0)] * 4 + [(B, top, 0, 0)], [2, 3, 4, 5], release=t0 + 5 * P)
        got = sorted(ring(s))
        f.check(got == exp, "SEQEND0_STOP: pin changes differ")
        out.append(f"seq seqend0_stop events_bad={bad} entries={len(got)}")

        # LOOPSDONE_STOP.
        to_lattice(s)
        configure(p, INDIVIDUAL, [(A, 1, 0), (D, 0, 0)], loop=1, shorts=SH_LD_STOP, top=top)
        ring(s)
        t0 = 2 * s.now
        p.w(SEQSTART0, 1)
        evs = [(t0 + k * P, "PWMPERIODEND") for k in range(1, 4)] + [(t0 + 3 * P, "LOOPSDONE"),
                                                                    (t0 + 3 * P, "STOPPED")]
        bad = check_events(s, p, f, t0, evs, "LOOPSDONE_STOP")
        exp = expected(t0, [(A, top, 0, 0)] * 2 + [(D, top, 0, 0)], [2, 3, 4, 5], release=t0 + 3 * P)
        got = sorted(ring(s))
        f.check(got == exp, "LOOPSDONE_STOP: pin changes differ")
        out.append(f"seq loopsdone_stop events_bad={bad} entries={len(got)}")

        # NextStep mode, Common decoder: a step plays until NEXTSTEP, then the
        # next boundary loads the next one; the last holds.
        to_lattice(s)
        configure(p, COMMON | NEXTSTEP_MODE, [([100 | 0x8000, 200 | 0x8000, 300 | 0x8000], 5, 5)], top=top)
        ring(s)
        t0 = 2 * s.now
        p.w(SEQSTART0, 1)
        for tk in (t0 + P + P // 2, t0 + 2 * P + P // 5, t0 + 5 * P + P // 2):
            s.run_to(tk // 2)
            p.w(NEXTSTEP, 1)
        s.run_to((t0 + 7 * P + P // 2) // 2)
        p.w(STOP, 1)
        s.run_to((t0 + 8 * P) // 2)
        seqend = p.ev("SEQEND0")
        common = [[v] * 4 for v in (100 | 0x8000, 200 | 0x8000, 300 | 0x8000)]
        exp = expected(t0, [(common[0], top, 0, 0)] * 2 + [(common[1], top, 0, 0)] + [(common[2], top, 0, 0)] * 5,
                       [2, 3, 4, 5], release=t0 + 8 * P)
        got = sorted(ring(s))
        f.check(got == exp and seqend == 1, f"NextStep: pin changes differ or no SEQEND0 ({seqend})")
        out.append(f"nextstep entries={len(got)}")

        # Grouped decoder.
        to_lattice(s)
        configure(p, GROUPED, [([100 | 0x8000, 700, 300 | 0x8000, 50], 1, 0)], top=top)
        ring(s)
        t0 = 2 * s.now
        p.w(SEQSTART0, 1)
        s.run_to((t0 + 3 * P + P // 4) // 2)
        p.w(STOP, 1)
        s.run_to((t0 + 4 * P) // 2)
        g = [[100 | 0x8000] * 2 + [700] * 2, [300 | 0x8000] * 2 + [50] * 2]
        exp = expected(t0, [(g[0], top, 0, 0)] * 2 + [(g[1], top, 0, 0)] * 2, [2, 3, 4, 5], release=t0 + 4 * P)
        got = sorted(ring(s))
        f.check(got == exp, "Grouped decoder: pin changes differ")
        out.append(f"grouped entries={len(got)}")

        # WaveForm: the fourth half-word is the period's COUNTERTOP; channel 3
        # is connected but not driven.
        to_lattice(s)
        wv = [50 | 0x8000, 100 | 0x8000, 150, 400, 300 | 0x8000, 10, 0 | 0x8000, 800]
        configure(p, WAVEFORM, [(wv, 1, 0)], top=1000)
        ring(s)
        t0 = 2 * s.now
        p.w(SEQSTART0, 1)
        per = [(wv[0:3] + [0], 400, 0, 0)] * 2 + [(wv[4:7] + [0], 800, 0, 0)] * 3
        tstop = t0 + 2 * period_hns(400) + 3 * period_hns(800)
        s.run_to((tstop - period_hns(800) // 2) // 2)
        p.w(STOP, 1)
        s.run_to(tstop // 2)
        exp = expected(t0, per, [2, 3, 4, 5], release=tstop, wave=True)
        got = sorted(ring(s))
        f.check(got == exp and all(e[2] != 3 for e in got), "WaveForm: pin changes differ or channel 3 driven")
        out.append(f"waveform entries={len(got)}")

        # PSEL changes while running: channel 1 moves to pin 9, channel 2 is
        # disconnected; both at the write instant.
        to_lattice(s)
        configure(p, INDIVIDUAL, [(A, 0, 0)], top=top)
        ring(s)
        t0 = 2 * s.now
        p.w(SEQSTART0, 1)
        tm = t0 + P + 30 * TICK + 1  # inside period 1, after channel 1's compare
        s.run_to(tm // 2)
        tm = 2 * s.now
        p.w(PSEL + 4, 9)
        p.w(PSEL + 8, 0x80000004)
        s.run_to((t0 + 2 * P + P // 2) // 2)
        p.w(STOP, 1)
        s.run_to((t0 + 3 * P) // 2)
        full = expected(t0, [(A, top, 0, 0)] * 3, [2, 3, 4, 5], release=t0 + 3 * P)
        exp = []
        lvl1 = lvl2 = None
        for e in full:
            t, inst, c, kind, lv, pin = e
            if c in (1, 2) and t <= tm:
                if c == 1:
                    lvl1 = lv
                else:
                    lvl2 = lv
            if c == 1 and t > tm:
                e = (t, inst, c, kind, lv, 9)
            if c == 2 and t > tm:
                continue
            exp.append(e)
        exp += [(tm, 0, 1, RELEASE, lvl1, 3), (tm, 0, 1, CONNECT, lvl1, 9), (tm, 0, 2, RELEASE, lvl2, 4)]
        got = sorted(ring(s))
        f.check(got == sorted(exp), f"PSEL change: got {[(e[0] - t0,) + e[1:] for e in got if e[2] in (1, 2)]}")
        out.append(f"psel entries={len(got)}")

        # ENABLE = 0 while running: released at once, no STOPPED, nothing
        # after. STOP while idle: STOPPED at once.
        to_lattice(s)
        configure(p, INDIVIDUAL, [(A, 0, 0)], top=top)
        ring(s)
        t0 = 2 * s.now
        p.w(SEQSTART0, 1)
        s.run_to((t0 + P + P // 2) // 2)
        td = 2 * s.now
        p.w(ENABLE, 0)
        s.run(3 * P // 2)
        part = [e for e in expected(t0, [(A, top, 0, 0)] * 2, [2, 3, 4, 5]) if e[0] <= td]
        lv = {}
        for e in part:
            lv[e[2]] = e[4]
        exp = sorted(part + [(td, 0, c, RELEASE, lv[c], 2 + c) for c in range(4)])
        got = sorted(ring(s))
        stopped = p.ev("STOPPED")
        p.w(ENABLE, 1)
        p.w(STOP, 1)
        idle = p.ev("STOPPED")
        f.check(got == exp and stopped == 0 and idle == 1,
                f"ENABLE = 0: entries {len(got)} vs {len(exp)}, STOPPED {stopped}; STOP idle {idle}")
        out.append(f"disable entries={len(got)} stopped={stopped} idle_stop={idle}")

        # IRQ lines: SEQEND0 with INTENSET pends NVIC 28, 33, 34, 45.
        pend = []
        for i in range(4):
            q = Pwm(s, i)
            to_lattice(s)
            s.w(NVIC_ICPR, 0xFFFFFFFF)
            s.w(NVIC_ICPR + 4, 0xFFFFFFFF)
            configure(q, INDIVIDUAL, [(A, 0, 0)], top=top)
            q.w(INTENSET, 1 << 4)
            irq = PWM_IRQ[i]
            before = s.r(NVIC_ISPR + 4 * (irq // 32)) >> (irq % 32) & 1
            q.w(SEQSTART0, 1)
            s.run(1 * US)
            after = s.r(NVIC_ISPR + 4 * (irq // 32)) >> (irq % 32) & 1
            q.w(STOP, 1)
            s.run(2 * P // 2)
            q.w(INTENCLR, 0xFF)
            q.w(ENABLE, 0)
            pend.append((before, after))
        ring(s)
        f.check(pend == [(0, 1)] * 4, f"PWM IRQ pending bits {pend}")
        out.append(f"irq {pend}")

        # Ring overflow: COUNTERTOP 3, 8 entries per 187.5 ns, no ack.
        to_lattice(s)
        configure(p, INDIVIDUAL, [([1 | 0x8000] * 4, 0, 0)], top=3)
        ring(s)
        p.w(SEQSTART0, 1)
        s.run(300 * US)
        n = s.r(PORT + 8) - s.ring_seen
        err = s.r(PORT + 4)
        p.w(STOP, 1)
        s.run(1 * US)
        p.w(ENABLE, 0)
        s.ring_seen = s.r(PORT + 8)
        s.w(PORT + 0x0C, s.ring_seen & 0xFFFFFFFF)
        s.w(PORT + 4, 0)
        f.check(n > RING and err & 1 == 1, f"ring overflow: {n} entries unread, error 0x{err:X}")
        out.append(f"overflow error={err & 1}")
        print("  sequencer: REFRESH, ENDDELAY, LOOP, SHORTS, NextStep, Grouped, WaveForm, PSEL, ENABLE, "
              "STOP, IRQ 28/33/34/45, ring overflow")

        # TEMP.
        s.w(PORT + 0x40, (-42) & 0xFFFFFFFF)
        s.w(NVIC_ICPR, 0xFFFFFFFF)
        to_lattice(s)
        t = s.now
        s.w(TEMP + 0x100, 0)
        s.w(TEMP + 0x304, 1)
        s.w(TEMP + 0x000, 1)
        s.run_to(t + 36 * US - 1)
        early = s.r(TEMP + 0x100)
        s.run_to(t + 36 * US)
        rdy, val, irq = s.r(TEMP + 0x100), s.r(TEMP + 0x508), s.r(NVIC_ISPR) >> 12 & 1
        s.w(TEMP + 0x100, 0)
        s.w(TEMP + 0x308, 1)
        s.w(TEMP + 0x000, 1)
        s.run(10 * US)
        s.w(TEMP + 0x004, 1)
        s.run(40 * US)
        cancelled = s.r(TEMP + 0x100)
        s.w(TEMP + 0x520, 0x111)
        a0 = s.r(TEMP + 0x520)
        f.check((early, rdy, val, irq, cancelled, a0, s.r(PORT + 0x44)) == (0, 1, 0xFFFFFFD6, 1, 0, 0x111, 1),
                f"TEMP: early {early} ready {rdy} TEMP 0x{val:X} irq {irq} after STOP {cancelled} A0 {a0:#x}")
        out.append(f"temp early={early} ready={rdy} value={val:#x} irq={irq} cancelled={cancelled}")
        s.w(PORT + 0x40, 100)

        # NVMC.
        page = 0x80000
        s.w(page, 0x12345678)
        s.w(NVMC + 0x508, page)                      # CONFIG = Ren: ignored
        ignored = (s.r(page), s.r(NVMC + 0x400))
        s.w(NVMC + 0x504, 2)
        s.w(NVMC + 0x508, page + 4)                  # not a page: ignored
        misaligned = s.r(NVMC + 0x400)
        to_lattice(s)
        t = s.now
        s.w(NVMC + 0x508, page)
        erased = (s.r(page), s.r(page + 4092), s.r(NVMC + 0x400), s.r(NVMC + 0x408))
        s.run_to(t + 85 * MS - 1)
        busy = s.r(NVMC + 0x400)
        s.run_to(t + 85 * MS)
        ready = s.r(NVMC + 0x400)
        f.check(ignored == (0x12345678, 1) and misaligned == 1 and erased[:2] == (0xFFFFFFFF, 0xFFFFFFFF)
                and erased[2:] == (0, 0) and busy == 0 and ready == 1,
                f"NVMC ERASEPAGE: ignored {ignored} misaligned {misaligned} erased {erased} busy {busy} ready {ready}")
        # Partial erase: 10 ms each, the page erased at the ninth (90 ms >= 85 ms).
        s.w(page, 0xA5A5A5A5)
        partial = []
        for k in range(9):
            t = s.now
            s.w(NVMC + 0x518, page)
            s.run_to(t + 10 * MS - 1)
            b = s.r(NVMC + 0x400)
            s.run_to(t + 10 * MS)
            partial.append((b, s.r(NVMC + 0x400), s.r(page)))
        f.check(all(x[:2] == (0, 1) for x in partial) and all(x[2] == 0xA5A5A5A5 for x in partial[:8])
                and partial[8][2] == 0xFFFFFFFF, f"NVMC partial erase {partial}")
        s.w(NVMC + 0x540, 0x101)
        icache = (s.r(NVMC + 0x540), s.r(NVMC + 0x548), s.r(NVMC + 0x54C))
        s.w(NVMC + 0x504, 0)
        f.check(icache == (0x101, 0, 0), f"NVMC ICACHECNF {icache}")
        out.append(f"nvmc ignored={ignored} misaligned={misaligned} erased={erased[:2]} busy={busy} "
                   f"ready={ready} partial={partial} icache={icache}")
        print("  TEMP: DATARDY at START + 36 us, the host value, STOP cancels, IRQ 12; NVMC: ERASEPAGE 85 ms, "
              "partial erase, CONFIG gating")
    finally:
        s.close()

    # ERASEALL wipes the firmware too: its own session, last.
    s = env.session(FW / "nrf_saadc_idle.elf")
    try:
        s.w(0x90000, 0x5A5A5A5A)
        s.w(NVMC + 0x504, 2)
        to_lattice(s)
        t = s.now
        s.w(NVMC + 0x50C, 1)
        wiped = s.r(0x90000)
        s.run_to(t + 169 * MS - 1)
        busy = s.r(NVMC + 0x400)
        s.run_to(t + 169 * MS)
        ready = s.r(NVMC + 0x400)
        f.check((wiped, busy, ready, s.r(PORT + 0x48)) == (0xFFFFFFFF, 0, 1, 1),
                f"NVMC ERASEALL: word {wiped:#x} busy {busy} ready {ready}")
        out.append(f"eraseall word={wiped:#x} busy={busy} ready={ready}")
    finally:
        s.close()
    return "\n".join(out) + "\n"


# The firmware's duty table (nrf_pwm_step.c), computed the same way.
FW_TOP, FW_REFRESH, FW_STEPS, FW_DPK = 640, 6, 4, 880
FW_PINS = [26, 5, 7, 11]
COS_TAB = [10000, 9808, 9239, 8315, 7071, 5556, 3827, 1951, 0]


def cos_u(u):
    u &= 31
    if u <= 8:
        return COS_TAB[u]
    if u <= 16:
        return -COS_TAB[16 - u]
    if u <= 24:
        return -COS_TAB[u - 16]
    return COS_TAB[32 - u]


def coil(c):
    amp = (FW_TOP * FW_DPK // 1000) * abs(c) // 10000
    hold, pwm = 0x8000 | FW_TOP, 0x8000 | (FW_TOP - amp)
    return [hold, pwm] if c >= 0 else [pwm, hold]


def microstep(u):
    return coil(cos_u(u)) + coil(cos_u(u - 8))


def group_firmware(env, f):
    print("firmware: TEMP, an NVMC erase and 4 full steps at 446 PPS (nrf_pwm_step.elf, lookahead armed)")
    s = env.session(FW / "nrf_pwm_step.elf", lookahead=True)
    out = []
    try:
        s.w(PORT + 0x40, 97)
        s.r(LOOKAHEAD, 8)
        # Until the PWM starts (after the 85 ms erase), then sample the
        # promise every 7 us for 3 ms.
        while s.r(PORT + 8) == 0:
            s.run(500 * US)
            if s.now > 200 * MS:
                break
        samples = []
        for _ in range(430):
            s.run(7 * US)
            st, lo, hi = s.r(LOOKAHEAD, 3)
            if st & (1 << 2) and st & (1 << 6):
                samples.append((s.now, lo | hi << 32))
        s.run(15 * MS)
        got = sorted(ring(s))
        conn = [e for e in got if e[3] == CONNECT]
        f.check(len(conn) == 4, f"{len(conn)} connects, expected 4")
        t0 = conn[0][0] if conn else 0
        plan = [microstep(u) for u in range(8 * FW_STEPS) for _ in range(FW_REFRESH + 1)]
        plan += [microstep(8 * FW_STEPS - 1)] * (7 * (FW_REFRESH + 1) + 1)
        P = period_hns(FW_TOP)
        rel = t0 + len(plan) * P
        exp = expected(t0, [(v, FW_TOP, 0, 0) for v in plan], FW_PINS, release=rel)
        f.check(got == exp, f"microstepping: {len(got)} pin changes, expected {len(exp)}; first difference "
                f"{next(((g[0] - t0,) + g[1:], (e[0] - t0,) + e[1:]) for g, e in zip(got + [None] * 9, exp) if g != e) if got != exp else None}")
        # No promise past the next PWM pin change.
        times = [e[0] for e in got]
        late, tight = [], 0
        k = 0
        for now, lim in samples:
            while k < len(times) and times[k] <= 2 * now:
                k += 1
            if k < len(times) and now + lim > times[k] // 2:
                late.append((now, lim, times[k]))
            if k < len(times) and now + lim == times[k] // 2:
                tight += 1
        # Non-vacuous: most promises end exactly at the next edge's ns (the
        # PWM's own clock limit is the nearest one).
        f.check(samples and not late and tight > len(samples) // 2,
                f"{len(samples)} promise samples, {len(late)} past the next edge {late[:3]}, {tight} tight")
        n = s.r(UARTLOG + 4)
        raw = []
        for j in range(n):
            lo, hi = s.r(UARTLOG + 0x100 + 8 * (j % 1024), 2)
            raw.append(hi >> 24)
        lines = bytes(raw).decode().splitlines()
        nv = lines[1].split() if len(lines) > 1 else []
        ticks = int(nv[3].split("=")[1]) if len(nv) > 3 else -1
        f.check(len(lines) == 3 and lines[0] == "temp=97"
                and nv[1:3] == ["before=c0ffee00", "after=ffffffff"] and nv[4] == "polled=1"
                and 2785 <= ticks <= 2786 and lines[2] == "pwm seqends=5 stopped",
                f"UART lines {lines} (erase ticks {ticks}, expected 2785-2786 = 85 ms / 30.52 us)")
        out.append(f"lines {lines}")
        out.append(f"pin changes {len(got)} from t0 {t0} hns, release at {rel - t0} hns after")
        out.append(f"promise samples {len(samples)} late {len(late)} tight {tight}")
        print(f"  {len(got)} pin changes as the duty table predicts (446 PPS: 7 periods of 40 us per microstep); "
              f"{len(samples)} promises, none past the next edge ({tight} end exactly at it); UART {lines}")
    finally:
        s.close()
    return "\n".join(out) + "\n"


GROUPS = {"registers": group_registers, "firmware": group_firmware}


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--renode")
    ap.add_argument("--out", default=str(ROOT / "out/nrf-pwm-model"))
    ap.add_argument("--keep", action="store_true")
    ap.add_argument("groups", nargs="*", default=list(GROUPS))
    a = ap.parse_args()
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    env = Env(find_renode(a.renode), out, a.keep)
    for e in ("nrf_saadc_idle.elf", "nrf_pwm_step.elf"):
        if not (FW / e).is_file():
            sys.exit(f"{FW / e} missing: make firmware")
    f = Failures()
    t0 = time.monotonic()
    for g in a.groups:
        text = GROUPS[g](env, f)
        (out / f"{g}.txt").write_text(text)
        again = GROUPS[g](env, Failures())
        f.check(again == text, f"{g}: two runs differ")
        print(f"  determinism: two {g} runs byte-identical")
    print(f"wall time {time.monotonic() - t0:.1f} s; transcripts in {out}")
    if f:
        print(f"bb-pwm-test: FAIL ({len(f)} failures)")
        return 1
    print("bb-pwm-test: PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
