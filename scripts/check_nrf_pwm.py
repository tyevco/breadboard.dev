#!/usr/bin/env python3
"""Check a run of examples/nrf-pwm/board.cir (Phase 1 M10, ADR-008 Amendment 9: the PWM engine path).

nrf_pwm_step.elf microsteps 4 full steps at 446 PPS on PWM0 (P0.26, P0.05, P0.07, P0.11). The
Renode engine reads each PWM pin change from the host port's pin ring and reports it as a
transition of the bound pin at its exact instant, arbitrated with the pin's GPIO level (the
firmware leaves the GPIO at reset: OUT low). This script recomputes the firmware's duty table
and the pin changes the PS rules give (examples/nrf-pwm/bb_pwm_test.py's `expected`, independent
of the model) and requires the run's pin_edge records of the four pins to be exactly those
changes, arbitrated with a low GPIO: same instants to the femtosecond (half-ns PWM ticks, the
lattice +1 fs rule on a lattice point), same levels. It also checks the UART lines.

Usage: check_nrf_pwm.py <out dir>
"""

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "examples/nrf-pwm"))
import bb_pwm_test as t  # noqa: E402

L_FS = 10_000_000_000  # the 10 us lattice
NET = {26: "U1.P0.26", 5: "U1.P0.05", 7: "U1.P0.07", 11: "U1.P0.11"}


def fs_of(hns):
    fs = hns * 500_000
    return fs + 1 if fs % L_FS == 0 else fs


def arbitrate(entries):
    """Per pin: [(t_fs, level)] the engine must report, from the ring entries and a GPIO held low."""
    out = {p: [] for p in NET}
    cur = {p: 0 for p in NET}
    for hns, _inst, _ch, kind, level, pin in sorted(entries):
        want = 0 if kind == t.RELEASE else level
        if want == cur[pin]:
            continue
        tf = fs_of(hns)
        if out[pin] and out[pin][-1][0] == tf:
            out[pin].pop()
        else:
            out[pin].append((tf, want))
        cur[pin] = want
    return out


def main():
    d = Path(sys.argv[1])
    edges = {p: [] for p in NET}
    lines = []
    for ln in (d / "events.jsonl").read_text().splitlines():
        r = json.loads(ln)
        if r["type"] == "pin_edge":
            for p, n in NET.items():
                if r["net"] == n:
                    edges[p].append((r["t_fs"], r["level"]))
        elif r["type"] == "log" and r["msg"].startswith("uart0: "):
            lines.append(r["msg"][len("uart0: "):])
    fail = []
    first = min((e[0][0] for e in edges.values() if e), default=None)
    if first is None:
        print("check_nrf_pwm: FAIL: no pin edge on the four PWM pins")
        return 1
    t0 = first // 500_000  # the SEQSTART instant in half-ns (the four connects)
    plan = [t.microstep(u) for u in range(8 * t.FW_STEPS) for _ in range(t.FW_REFRESH + 1)]
    plan += [t.microstep(8 * t.FW_STEPS - 1)] * (7 * (t.FW_REFRESH + 1) + 1)
    rel = t0 + len(plan) * t.period_hns(t.FW_TOP)
    exp = arbitrate(t.expected(t0, [(v, t.FW_TOP, 0, 0) for v in plan], t.FW_PINS, release=rel))
    n = 0
    for p in NET:
        n += len(edges[p])
        if edges[p] != exp[p]:
            diff = next(((g, e) for g, e in zip(edges[p] + [None] * 3, exp[p] + [None] * 3) if g != e), None)
            fail.append(f"{NET[p]}: {len(edges[p])} edges, expected {len(exp[p])}; first difference "
                        f"(got, expected) {diff}")
    want_lines = ["temp=100", "pwm seqends=5 stopped"]
    if len(lines) != 3 or lines[0] != want_lines[0] or lines[2] != want_lines[1] \
            or not lines[1].startswith("nvmc before=c0ffee00 after=ffffffff"):
        fail.append(f"UART lines {lines}")
    for f in fail:
        print(f"check_nrf_pwm: FAIL: {f}")
    if fail:
        return 1
    print(f"check_nrf_pwm: PASS: {n} pin edges on the four PWM pins equal the duty table at their exact "
          f"instants (SEQSTART at {t0 / 2:.1f} ns, release {(rel - t0) / 2e6:.3f} ms later); UART {lines}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
