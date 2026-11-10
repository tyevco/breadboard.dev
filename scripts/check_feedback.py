#!/usr/bin/env python3
"""ADR-003 plan step 4 checks on a `bb run` of examples/feedback or
examples/feedback-open.

Usage: python3 scripts/check_feedback.py <out_dir> [--open | --nrf] [--lattice-us 10]

--nrf checks examples/nrf-feedback (Phase 1 M2), the same circuit on the
nRF52840: U1.P0.13, U1.P0.11 and U1.P0.15 in place of U1.PA5, U1.PB0 and
U1.PA6, and a drive period of 1638/32768 s (RTC0) in place of 50 ms.

The drive edges are the U1.PA5 pin_edge records (examples/feedback) or,
with --open, the board's V_DRV pulse (rising at 50 + 100 k ms, falling at
100 + 100 k ms; examples/feedback-open). Asserts:

  * for every drive edge with time to settle before the end, exactly one
    U1.PB0 transition (the hysteresis gives one per direction), at the
    analytic RC crossing: tau = 1025 ohm * (1 uF + 5 pF), rising through
    vih = 2.0 V after tau*ln(3.3/1.3), falling through vil = 0.8 V after
    tau*ln(v/0.8) (v = SENSE when the drive falls), plus half the 5 ns
    drive ramp; within 1 us, and in the same lattice cell
    (ceil_L(t_x) equal), e.g. 50.95-50.96 ms and 101.45-101.46 ms;
  * every U1.PA6 edge follows a PB0 transition with its level, in
    (ceil_L(t_x), ceil_L(t_x) + L]: never before the MCU can see the input
    (ceil_L), and at most one cell after it; one PA6 edge per PB0
    transition;
  * examples/feedback: U1.PA5 alternates every 50 ms (+- 1 us) and LED_B
    sits on its high plateau (1.7-2.2 V) while PA6 is high and below
    0.1 V while it is low (samples at least 100 us after a PA6 edge);
  * nets.csv's U1.PB0 column agrees with the pin_edge records.

Python 3 standard library only.
"""

import argparse
import csv
import json
import math
import sys
from pathlib import Path

FS_PER_US = 10**9
FS_PER_MS = 10**12
TAU = 1025.0 * (1e-6 + 5e-12)
VOH, VIH, VIL = 3.3, 2.0, 0.8
RAMP_HALF_S = 2.5e-9
TOL_FS = 1 * FS_PER_US
PA5_SPACING_TOL_FS = 1 * FS_PER_US
SETTLE_FS = 100 * FS_PER_US
LED_HIGH = (1.7, 2.2)
LED_LOW_MAX = 0.1
# The board's nets and drive period (--nrf changes them).
DRV, IN, COPY = "U1.PA5", "U1.PB0", "U1.PA6"
PERIOD_FS = 50 * FS_PER_MS


def fail(msg):
    print(f"check_feedback: FAIL: {msg}")
    sys.exit(1)


def edges(path):
    out = {}
    with open(path) as f:
        for line in f:
            ev = json.loads(line)
            if ev["type"] == "pin_edge":
                out.setdefault(ev["net"], []).append((ev["t_fs"], ev["level"]))
    return out


def load_csv(path):
    with open(path, newline="") as f:
        rows = list(csv.reader(f))
    header, body = rows[0], rows[1:]
    if header[0] != "t_fs":
        fail(f"{path}: unexpected header {header}")
    cols = {name: [r[i] for r in body] for i, name in enumerate(header)}
    return [int(r[0]) for r in body], cols


def ceil_l(t, l_fs):
    return -(-t // l_fs) * l_fs


def main():
    p = argparse.ArgumentParser()
    p.add_argument("out")
    p.add_argument("--open", action="store_true", help="examples/feedback-open (pulse drive)")
    p.add_argument("--lattice-us", type=int, default=10)
    p.add_argument("--nrf", action="store_true",
                   help="examples/nrf-feedback (Phase 1 M2): the same circuit on the nRF52840, "
                        "U1.P0.13 drives, U1.P0.11 reads, U1.P0.15 copies; RTC0 toggles the "
                        "drive every 1638/32768 s")
    a = p.parse_args()
    global DRV, IN, COPY, PERIOD_FS
    if a.nrf:
        DRV, IN, COPY = "U1.P0.13", "U1.P0.11", "U1.P0.15"
        PERIOD_FS = 1638 * 10**15 // 32768
    out = Path(a.out)
    l_fs = a.lattice_us * FS_PER_US
    ev = edges(out / "events.jsonl")
    t, cols = load_csv(out / "nets.csv")
    t_end = t[-1]

    # Drive edges (time fs, level).
    if a.open:
        drive = []
        k = 1
        while k * 50 * FS_PER_MS < t_end:
            drive.append((k * 50 * FS_PER_MS, k % 2))
            k += 1
        if "U1.PA5" in ev:
            fail("feedback-open must not bind U1.PA5")
    else:
        drive = ev.get(DRV, [])
        if len(drive) < 2:
            fail(f"expected {DRV} edges, got {drive}")
        # Spaced 50 ms apart within PA5_SPACING_TOL_FS (SysTick reload
        # timing in Renode drifts by tens of ns per period after 250 ms, as
        # in the spike), alternating from High. (--nrf: the RTC period.)
        for i, (ti, lv) in enumerate(drive):
            prev = drive[i - 1][0] if i else 0
            if abs(ti - prev - PERIOD_FS) > PA5_SPACING_TOL_FS or lv != (i + 1) % 2:
                fail(f"{DRV} edge {i} at {ti} fs level {lv}: expected {PERIOD_FS} fs after "
                     f"{prev} fs, level {(i + 1) % 2}")

    # Analytic crossings.
    want = []
    v_sense = 0.0
    t_prev = 0
    for ti, lv in drive:
        # SENSE at this drive edge (RC from the previous one).
        dt = (ti - t_prev) / 1e15
        target_prev = VOH if lv == 0 else 0.0
        v_sense = target_prev + (v_sense - target_prev) * math.exp(-dt / TAU)
        if lv == 1:
            d = TAU * math.log((VOH - v_sense) / (VOH - VIH))
        else:
            d = TAU * math.log(v_sense / VIL)
        tx = ti + round((RAMP_HALF_S + d) * 1e15)
        if tx < t_end:
            want.append((tx, lv))
        t_prev = ti
    pb0 = ev.get(IN, [])
    if len(pb0) != len(want):
        fail(f"{len(pb0)} {IN} transitions, expected {len(want)} (one per drive edge): {pb0}")
    worst = 0
    for (got, lv), (w, wl) in zip(pb0, want):
        if lv != wl:
            fail(f"{IN} at {got} fs has level {lv}, expected {wl}")
        worst = max(worst, abs(got - w))
        if abs(got - w) > TOL_FS:
            fail(f"{IN} -> {lv} at {got} fs, analytic {w} fs (diff {(got - w) / 1e6:.1f} ns)")
        if ceil_l(got, l_fs) != ceil_l(w, l_fs):
            fail(f"{IN} -> {lv} at {got} fs not in the analytic cell ending {ceil_l(w, l_fs)} fs")
    cells = ", ".join(
        f"{(ceil_l(g, l_fs) - l_fs) / FS_PER_MS:.2f}-{ceil_l(g, l_fs) / FS_PER_MS:.2f} ms" for g, _ in pb0
    )
    print(f"check_feedback: {len(pb0)} {IN} transitions in the analytic cells ({cells}); "
          f"max |t - analytic| {worst / 1e6:.1f} ns")

    # PA6 follows each PB0 transition in (ceil_L(t_x), ceil_L(t_x) + L].
    pa6 = ev.get(COPY, [])
    if len(pa6) != len(pb0):
        fail(f"{len(pa6)} {COPY} edges for {len(pb0)} {IN} transitions: {pa6}")
    lags = []
    for (te, lv), (tx, xl) in zip(pa6, pb0):
        c = ceil_l(tx, l_fs)
        if lv != xl:
            fail(f"{COPY} at {te} fs level {lv} after {IN} -> {xl}")
        if not c < te <= c + l_fs:
            fail(f"{COPY} edge at {te} fs outside (ceil_L(t_x), ceil_L(t_x) + L] = ({c}, {c + l_fs}] fs")
        lags.append(te - c)
    print(f"check_feedback: every {COPY} edge in (ceil_L(t_x), ceil_L(t_x) + L]; lag after "
          f"ceil_L(t_x): {sorted(set(lags))} fs")

    # nets.csv agrees with the PB0 edges.
    if IN in cols:
        level = 0
        k = 0
        for ti, v in zip(t, cols[IN]):
            while k < len(pb0) and pb0[k][0] <= ti:
                level = pb0[k][1]
                k += 1
            if int(v) != level:
                fail(f"nets.csv {IN} = {v} at {ti} fs, pin_edge records say {level}")

    if not a.open:
        if "LED_B" not in cols:
            fail("nets.csv lacks LED_B")
        led = [float(x) for x in cols["LED_B"]]
        n_hi = n_lo = 0
        k = 0
        level = 0
        last_edge = 0
        for ti, v in zip(t, led):
            while k < len(pa6) and pa6[k][0] <= ti:
                level, last_edge = pa6[k][1], pa6[k][0]
                k += 1
            if ti - last_edge < SETTLE_FS:
                continue
            if level and not LED_HIGH[0] <= v <= LED_HIGH[1]:
                fail(f"LED_B {v} V at {ti} fs while {COPY} high")
            if not level and v > LED_LOW_MAX:
                fail(f"LED_B {v} V at {ti} fs while {COPY} low")
            n_hi += level
            n_lo += 1 - level
        print(f"check_feedback: LED_B plateaus ok ({n_hi} high, {n_lo} low settled samples)")
    print("check_feedback: PASS")


if __name__ == "__main__":
    main()
