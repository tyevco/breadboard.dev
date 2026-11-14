#!/usr/bin/env python3
"""Checks on a `bb run` of tests/device-smoke/board.cir (ADR-006 D7,
Amendment 6; Phase 1 M10): the device route end to end.

Usage: python3 scripts/check_device_smoke.py <out_dir>

Asserts, from the pin_edge records:

  * U1.PA5 rises at 50 ms (+ 1 fs: a CPU write on a lattice point) and
    alternates every 50 ms;
  * M1.HOME_N (driver `devices.M1.HOME_N`) changes exactly once in the run:
    it rises at exactly 57.04 ms, the end of the DRV5032 window that starts
    at phase 7 ms + 50 ms, after the rotor's one full step at about 50.1 ms
    (board.cir); the device's edges are at their exact femtosecond;
  * U1.PB0 has no pin_edge of its own: the CPU reads the device's kernel net
    directly (spec/package.md section 12);
  * U1.PA6 rises once, in (57.04 ms, 57.05 ms]: the CPU sees PB0 at
    ceil_L(57.04 ms) = 57.04 ms and its EXTI ISR copies it within the next
    lattice cell;
  * nets.csv's HOME_N node is low before and high after the edge (the
    device's output reaches the circuit through rule-1 glue).

Python 3 standard library only.
"""

import csv
import json
import sys
from pathlib import Path

MS = 10**12
L = 10 * 10**9


def main():
    out = Path(sys.argv[1])
    edges = []
    with open(out / "events.jsonl") as f:
        for line in f:
            r = json.loads(line)
            if r["type"] == "pin_edge":
                edges.append(r)
    by = {}
    for e in edges:
        by.setdefault(e["net"], []).append(e)
    bad = []

    pa5 = by.get("U1.PA5", [])
    if not pa5 or pa5[0]["t_fs"] != 50 * MS + 1 or pa5[0]["level"] != 1:
        bad.append(f"U1.PA5: first edge {pa5[:1]}, expected a rise at 50 ms + 1 fs")
    for a, b in zip(pa5, pa5[1:]):
        if abs((b["t_fs"] - a["t_fs"]) - 50 * MS) > 10**9 or a["level"] == b["level"]:
            bad.append(f"U1.PA5: {a} -> {b} is not a 50 ms toggle")

    home = by.get("M1.HOME_N", [])
    want = [{"t_fs": 57 * MS + 40 * 10**9, "level": 1, "driver": "devices.M1.HOME_N"}]
    got = [{k: e[k] for k in ("t_fs", "level", "driver")} for e in home]
    if got != want:
        bad.append(f"M1.HOME_N: {got}, expected {want}")

    if "U1.PB0" in by:
        bad.append(f"U1.PB0 has pin_edge records {by['U1.PB0'][:2]}: it should read M1.HOME_N directly")

    pa6 = by.get("U1.PA6", [])
    t0 = 57 * MS + 40 * 10**9
    if len(pa6) != 1 or pa6[0]["level"] != 1 or not (t0 < pa6[0]["t_fs"] <= t0 + L):
        bad.append(f"U1.PA6: {pa6}, expected one rise in (57.04 ms, 57.05 ms]")

    with open(out / "nets.csv", newline="") as f:
        rows = list(csv.reader(f))
    head = rows[0]
    col = head.index("HOME_N")
    for r in rows[1:]:
        t, v = int(r[0]), float(r[col])
        if t < 57 * MS and v > 0.1:
            bad.append(f"HOME_N node {v} V at {t} fs, before the edge")
            break
        if t > 57 * MS + 100 * 10**9 and v < 3.0:
            bad.append(f"HOME_N node {v} V at {t} fs, after the edge")
            break

    for b in bad:
        print(f"FAIL: {b}")
    if bad:
        sys.exit(1)
    print(
        f"device-smoke: PASS ({len(pa5)} PA5 edges, HOME_N at 57.04 ms, "
        f"PA6 at {pa6[0]['t_fs'] / MS:.6f} ms)"
    )


if __name__ == "__main__":
    main()
