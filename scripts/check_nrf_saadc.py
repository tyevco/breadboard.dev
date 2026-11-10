#!/usr/bin/env python3
"""Check a run of examples/nrf-saadc (Phase 1 M8, ADR-008 D7/D8; make
nrf-saadc-model) from its events.jsonl:

- every burst (one per P0.13 toggle, the firmware's marker) has exactly 8
  #smp toggles (one edge per acquisition, alternating levels), 42 us apart
  (TACQ 40 us + tCONV 2 us), the first at the SAMPLE task;
- every traced sample is stamped at its acquisition start + TACQ, and its
  code is round_half_up(uV * (1/6) / 0.6 V * 4096);
- one charge transfer per acquisition: each transfer moves the pad to
  V + a * (VPRE - V), a = CS / (CS + C1 + CPAD), and between transfers the
  pad relaxes towards V0 through RLEAK (tau = RLEAK * (C1 + CPAD + CS) =
  105.5 s: about 15 uV per 5 ms gap); every sample is within TOL_UV of
  that, and the leak-free value of one transfer fewer or more (20-40 mV
  away) is more than 5 mV from it (a missing or a doubled transfer cannot
  pass);
- every UART line is "b=<k> ain0=<mean of the burst's codes> vdd=<code of
  3.0 V>", logged after the burst's last sample;
- no grant stopped early at a push (`stop:`): the input is sampled.

Usage: check_nrf_saadc.py <out-dir> [--unloaded]   (--unloaded: load=none,
no toggles, every sample reads V0)
"""
import json
import math
import sys
from fractions import Fraction

FS_PER_NS = 1_000_000
TACQ_NS, TCONV_NS = 40_000, 2_000
V0, VPRE = Fraction(12, 10), Fraction(3)
CS, C1, CPAD = Fraction(25, 10), Fraction(100), Fraction(3)  # pF
ALPHA = CS / (CS + C1 + CPAD)
TAU_S = 1e12 * float(C1 + CPAD + CS) * 1e-12
TOL_UV = 20


def code(uv):
    return math.floor(Fraction(uv) / 6 / 600_000 * 4096 + Fraction(1, 2))


def v_n(n):
    return float(VPRE - (VPRE - V0) * (1 - ALPHA) ** n) * 1e6


def main():
    out = sys.argv[1]
    unloaded = "--unloaded" in sys.argv[2:]
    recs = [json.loads(l) for l in open(f"{out}/events.jsonl")]
    fails = []

    def check(c, msg):
        if not c:
            fails.append(msg)

    stops = [r for r in recs if r["type"] == "grant" and r["limited_by"].startswith("stop:")]
    check(not stops, f"{len(stops)} grants stopped early at a push")
    marks = [r["t_fs"] for r in recs if r["type"] == "pin_edge" and r["net"] == "U1.P0.13"]
    smp = [r for r in recs if r["type"] == "pin_edge" and r["net"] == "U1.P0.02#smp"]
    samples = []
    for r in recs:
        if r["type"] == "log" and r["msg"].startswith("saadc sample: P0.02"):
            uv = int(r["msg"].split(") ")[1].split(" uV")[0])
            c = int(r["msg"].rsplit("code ", 1)[1])
            samples.append((r["t_fs"], uv, c))
    lines = [(r["t_fs"], r["msg"]) for r in recs if r["type"] == "log" and r["msg"].startswith("uart0: ")]
    check(len(marks) >= 2, f"{len(marks)} bursts (P0.13 toggles)")
    check(len(samples) == 8 * len(marks), f"{len(samples)} AIN0 samples for {len(marks)} bursts of 8")
    if unloaded:
        check(not smp, f"{len(smp)} #smp toggles with load=none")
    else:
        check(len(smp) == 8 * len(marks), f"{len(smp)} #smp toggles for {len(marks)} bursts of 8")
        levels = [r["level"] for r in smp]
        check(levels == [(k + 1) % 2 for k in range(len(smp))], "#smp levels do not alternate from 1")
    V0f = float(V0)
    v, t_prev, worst = V0f, 0.0, 0.0
    for b, m in enumerate(marks):
        sb = samples[8 * b:8 * b + 8]
        a0 = None
        if not unloaded:
            tb = [r["t_fs"] for r in smp[8 * b:8 * b + 8]]
            a0 = tb[0]
            check(m <= a0 < m + 10 * 1000 * FS_PER_NS, f"burst {b}: first toggle {a0} not right after P0.13 {m}")
            # A toggle stamped on a lattice point is reported 1 fs after it.
            tb = [t - t % FS_PER_NS for t in tb]
            check(all(tb[k + 1] - tb[k] == (TACQ_NS + TCONV_NS) * FS_PER_NS for k in range(len(tb) - 1)),
                  f"burst {b}: toggles not 42 us apart: {tb}")
            check([s[0] for s in sb] == [t + TACQ_NS * FS_PER_NS for t in tb],
                  f"burst {b}: samples {[s[0] for s in sb]} not at the toggles + TACQ")
        else:
            st = [s[0] for s in sb]
            check(all(st[k + 1] - st[k] == (TACQ_NS + TCONV_NS) * FS_PER_NS for k in range(7)),
                  f"burst {b}: samples not 42 us apart")
        for k, (t, uv, c) in enumerate(sb):
            n = 8 * b + k + 1
            check(c == code(uv), f"sample {n}: code {c}, expected {code(uv)} for {uv} uV")
            if unloaded:
                check(abs(uv - 1_200_000) <= 1, f"sample {n}: {uv} uV unloaded, expected 1200000")
            else:
                a_s = (t - TACQ_NS * FS_PER_NS) / 1e15
                v = V0f + (v - V0f) * math.exp(-(a_s - t_prev) / TAU_S)
                v += float(ALPHA) * (float(VPRE) - v)
                t_prev = a_s
                want = (V0f + (v - V0f) * math.exp(-TACQ_NS * 1e-9 / TAU_S)) * 1e6
                check(abs(uv - want) <= TOL_UV, f"sample {n}: {uv} uV, one transfer per acquisition "
                      f"gives {want:.0f} uV (n - 1: {v_n(n - 1):.0f}, n + 1: {v_n(n + 1):.0f})")
                check(min(abs(uv - v_n(n - 1)), abs(uv - v_n(n + 1))) > 5000,
                      f"sample {n}: within 5 mV of a neighbouring transfer count")
                worst = max(worst, abs(uv - want))
        want_line = f"uart0: b={b} ain0={math.floor(Fraction(sum(s[2] for s in sb), 8) + Fraction(1, 2))} " \
                    f"vdd={code(3_000_000)}"
        got = lines[b] if b < len(lines) else (None, None)
        check(got[1] == want_line, f"burst {b}: UART line {got[1]!r}, expected {want_line!r}")
        if got[0] is not None and sb:
            check(got[0] > sb[-1][0], f"burst {b}: UART line at {got[0]} before the last sample")
    if fails:
        for f in fails[:20]:
            print("check_nrf_saadc: FAIL: " + f)
        sys.exit(1)
    last = samples[-1][1] if samples else 0
    print(f"check_nrf_saadc: {len(marks)} bursts, {len(smp)} #smp toggles, {len(samples)} samples, "
          f"{len(lines)} UART lines; last sample {last} uV (one transfer per acquisition: "
          f"{v_n(len(samples)):.0f} uV without the leak), at most {worst:.0f} uV from the transfer "
          f"model{' (unloaded)' if unloaded else ''}: PASS")


if __name__ == "__main__":
    main()
