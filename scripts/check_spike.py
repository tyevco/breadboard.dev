#!/usr/bin/env python3
"""Phase 0 spike exit-gate checks on a `bb run` output directory.

Usage: python3 scripts/check_spike.py <out_dir>

Asserts (plan "Verification", AGENTS.md):
  * LED_A high plateau within [1.7, 2.2] V and low plateau below 0.1 V
    (settled samples: at least SETTLE after the nearest PA5 edge);
  * at least 8 PA5 pin_edge events, alternating levels, spaced 50 ms +- 0.1 ms;
  * at least 8 LED_A transitions (1.0 V crossings, interpolated between
    samples), spaced 50 ms +- 0.1 ms, with the first switched sample after its PA5 edge and within 2 samples of it
    (the LED follows the 5 ns edge plus a sub-microsecond RC, so it must have
    switched by the next sample).

Python 3 standard library only.
"""

import csv
import json
import sys
from pathlib import Path

FS_PER_MS = 10**12
PERIOD_FS = 50 * FS_PER_MS
SPACING_TOL_FS = FS_PER_MS // 10  # 0.1 ms
SETTLE_FS = 100 * 10**9  # 100 us
HIGH = (1.7, 2.2)
LOW_MAX = 0.1
THRESH = 1.0
MIN_TRANSITIONS = 8


def fail(msg):
    print(f"check_spike: FAIL: {msg}")
    sys.exit(1)


def load_csv(path):
    with open(path, newline="") as f:
        rows = list(csv.reader(f))
    header, body = rows[0], rows[1:]
    if header[0] != "t_fs" or "LED_A" not in header:
        fail(f"{path}: unexpected header {header}")
    col = header.index("LED_A")
    t = [int(r[0]) for r in body]
    v = [float(r[col]) for r in body]
    return t, v


def pin_edges(path):
    edges = []
    with open(path) as f:
        for line in f:
            ev = json.loads(line)
            if ev["type"] == "pin_edge" and ev["net"] == "U1.PA5":
                edges.append((ev["t_fs"], ev["level"]))
    return edges


def check_spacing(name, times):
    if len(times) < MIN_TRANSITIONS:
        fail(f"{name}: {len(times)} transitions, need >= {MIN_TRANSITIONS}")
    worst = 0
    for a, b in zip(times, times[1:]):
        d = b - a
        err = abs(d - PERIOD_FS)
        worst = max(worst, err)
        if err > SPACING_TOL_FS:
            fail(f"{name}: spacing {d / FS_PER_MS:.6f} ms between {a} and {b} fs")
    return worst


def main():
    if len(sys.argv) != 2:
        print(__doc__)
        sys.exit(2)
    out = Path(sys.argv[1])
    t, v = load_csv(out / "nets.csv")
    dt = t[1] - t[0]
    edges = pin_edges(out / "events.jsonl")

    # PA5 digital edges.
    for (_, a), (_, b) in zip(edges, edges[1:]):
        if a == b:
            fail("PA5 pin_edge levels do not alternate")
    worst_pin = check_spacing("PA5 pin_edge", [e[0] for e in edges])

    # LED_A transitions (interpolated threshold crossings).
    crossings = []
    for i in range(1, len(v)):
        lo, hi = v[i - 1], v[i]
        if (lo < THRESH) != (hi < THRESH):
            frac = (THRESH - lo) / (hi - lo)
            crossings.append((t[i - 1] + round(frac * (t[i] - t[i - 1])), hi > lo, t[i]))
    worst_led = check_spacing("LED_A", [c[0] for c in crossings])
    if len(crossings) != len(edges):
        fail(f"{len(crossings)} LED_A transitions but {len(edges)} PA5 edges")
    worst_lag = 0
    for (te, level), (tc, rising, t_after) in zip(edges, crossings):
        if bool(level) != rising:
            fail(f"LED_A transition at {tc} fs goes the wrong way for PA5 edge at {te}")
        # First sample past the 1 V crossing: it must come after the PA5 edge
        # (the LED cannot switch before its driver) and within 2 samples of it.
        lag = t_after - te
        if lag < 0 or lag > 2 * dt:
            fail(f"LED_A first switched sample at {t_after} fs is {lag} fs after "
                 f"PA5 edge at {te} fs")
        worst_lag = max(worst_lag, lag)

    # Plateaus: samples at least SETTLE away from every edge.
    edge_t = [e[0] for e in edges]
    highs, lows = [], []
    k = 0
    for ti, vi in zip(t, v):
        while k < len(edge_t) and edge_t[k] <= ti:
            k += 1
        near = any(abs(ti - edge_t[j]) < SETTLE_FS for j in (k - 1, k) if 0 <= j < len(edge_t))
        if near:
            continue
        level = 1 if k > 0 and edges[k - 1][1] == 1 else 0
        (highs if level else lows).append(vi)
    if not highs or not lows:
        fail("no settled high or low samples")
    if not (HIGH[0] <= min(highs) and max(highs) <= HIGH[1]):
        fail(f"LED_A high plateau [{min(highs):.6f}, {max(highs):.6f}] V outside {HIGH}")
    if max(lows) >= LOW_MAX:
        fail(f"LED_A low plateau max {max(lows):.6f} V >= {LOW_MAX}")

    print(f"check_spike: {len(t)} samples at {dt / 1e9:g} us")
    print(f"check_spike: LED_A high plateau {min(highs):.6f}..{max(highs):.6f} V "
          f"(want {HIGH[0]}..{HIGH[1]}), low max {max(lows):.6f} V (want < {LOW_MAX})")
    print(f"check_spike: PA5 edges {len(edges)} at "
          + ", ".join(f"{e[0] / FS_PER_MS:.6f}" for e in edges)
          + f" ms; worst spacing error {worst_pin / 1e9:.3f} us")
    print(f"check_spike: LED_A transitions {len(crossings)}; worst spacing error "
          f"{worst_led / 1e9:.3f} us (tol 100 us); worst first-switched-sample lag after PA5 edge "
          f"{worst_lag / 1e9:.3f} us (<= 2 samples)")
    print("check_spike: PASS")


if __name__ == "__main__":
    main()
