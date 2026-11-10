#!/usr/bin/env python3
"""Compare two runs' nets.csv and nets.vcd within a tolerance (ADR-007 D9,
gate M4a's fallback): digital values and every non-analog line must be
identical, analog values may differ by at most --tol-uv microvolts, and
the files must have the same shape (rows, columns, VCD lines). Prints how
many analog values differ and the largest difference.

usage: compare_outputs.py <run a> <run b> [--tol-uv 1000]
"""

import argparse
import csv
import os
import sys


def load_csv(path):
    with open(path, newline="") as f:
        return list(csv.reader(f))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("a")
    ap.add_argument("b")
    ap.add_argument("--tol-uv", type=int, default=1000)
    args = ap.parse_args()
    tol = args.tol_uv
    bad = []

    a = load_csv(os.path.join(args.a, "nets.csv"))
    b = load_csv(os.path.join(args.b, "nets.csv"))
    if a[0] != b[0] or len(a) != len(b):
        sys.exit(f"compare: nets.csv shape differs: {a[0]} x {len(a)} vs {b[0]} x {len(b)}")
    n_csv, max_csv = 0, 0
    for ra, rb in zip(a[1:], b[1:]):
        if len(ra) != len(rb) or ra[0] != rb[0]:
            sys.exit(f"compare: nets.csv rows differ in shape at t_fs {ra[0]}")
        for col, (x, y) in enumerate(zip(ra[1:], rb[1:]), start=1):
            if x == y:
                continue
            if "." not in x or "." not in y:
                bad.append(f"nets.csv t_fs {ra[0]} {a[0][col]}: {x} vs {y} (not analog)")
                continue
            d = abs(int(x.replace(".", "")) - int(y.replace(".", "")))
            n_csv += 1
            max_csv = max(max_csv, d)
            if d > tol:
                bad.append(f"nets.csv t_fs {ra[0]} {a[0][col]}: {x} vs {y}")

    va = open(os.path.join(args.a, "nets.vcd")).read().splitlines()
    vb = open(os.path.join(args.b, "nets.vcd")).read().splitlines()
    if len(va) != len(vb):
        sys.exit(f"compare: nets.vcd line counts differ: {len(va)} vs {len(vb)}")
    n_vcd, max_vcd = 0, 0
    for k, (x, y) in enumerate(zip(va, vb)):
        if x == y:
            continue
        if x.startswith("r") and y.startswith("r") and x.split()[1] == y.split()[1]:
            d = abs(int(x.split()[0][1:].replace(".", "")) - int(y.split()[0][1:].replace(".", "")))
            n_vcd += 1
            max_vcd = max(max_vcd, d)
            if d > tol:
                bad.append(f"nets.vcd line {k + 1}: {x} vs {y}")
        else:
            bad.append(f"nets.vcd line {k + 1}: {x!r} vs {y!r}")

    print(f"compare: nets.csv {n_csv} analog values differ (max {max_csv} uV), "
          f"nets.vcd {n_vcd} values differ (max {max_vcd} uV); tolerance {tol} uV, "
          f"digital values and timing identical" if not bad else "")
    if bad:
        for x in bad[:20]:
            print("compare: FAIL:", x)
        sys.exit(1)


if __name__ == "__main__":
    main()
