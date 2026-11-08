#!/usr/bin/env python3
"""Golden-waveform snapshot and diff for the Phase 0 spike.

  python3 scripts/golden.py write <nets.csv> <golden.csv> [--net LED_A] [--every-us 100]
  python3 scripts/golden.py check <nets.csv> <golden.csv> [--net LED_A] [--tol-mv 1] [--tol-samples 1]

`write` keeps the rows of `nets.csv` whose time is a multiple of --every-us
(default 100 us, i.e. 10 kHz) and the one column --net, in the same fixed
format as nets.csv (t_fs, volts with 6 decimals).

`check` passes when, for every golden row at time t, the run's value at t is
within --tol-mv of the golden value, or the run's value at a sample at most
--tol-samples run samples away from t is (the "1 mV / 1 sample" tolerance:
it absorbs a one-sample shift of a fast edge). Every golden time must exist
in the run.

Python 3 standard library only.
"""

import argparse
import csv
import sys


def read(path, net):
    with open(path, newline="") as f:
        rows = list(csv.reader(f))
    header = rows[0]
    if header[0] != "t_fs" or net not in header:
        sys.exit(f"golden: {path}: header {header} lacks t_fs/{net}")
    col = header.index(net)
    return [int(r[0]) for r in rows[1:]], [r[col] for r in rows[1:]]


def write(args):
    t, v = read(args.nets, args.net)
    step = args.every_us * 10**9
    n = 0
    with open(args.golden, "w", newline="\n") as f:
        f.write(f"t_fs,{args.net}\n")
        for ti, vi in zip(t, v):
            if ti % step == 0:
                f.write(f"{ti},{vi}\n")
                n += 1
    print(f"golden: wrote {n} rows ({args.net} every {args.every_us} us) to {args.golden}")


def check(args):
    t, v = read(args.nets, args.net)
    gt, gv = read(args.golden, args.net)
    index = {ti: i for i, ti in enumerate(t)}
    tol = args.tol_mv / 1000.0
    vals = [float(x) for x in v]
    worst_exact = 0.0
    shifted = 0
    bad = []
    for ti, gs in zip(gt, gv):
        g = float(gs)
        i = index.get(ti)
        if i is None:
            bad.append(f"t={ti} fs missing from run")
            continue
        d = abs(vals[i] - g)
        worst_exact = max(worst_exact, d)
        if d <= tol:
            continue
        lo, hi = max(0, i - args.tol_samples), min(len(vals), i + args.tol_samples + 1)
        if any(abs(vals[j] - g) <= tol for j in range(lo, hi)):
            shifted += 1
            continue
        bad.append(f"t={ti} fs: run {vals[i]:.6f} V vs golden {g:.6f} V (diff {d * 1000:.3f} mV)")
    print(f"golden: compared {len(gt)} rows of {args.net}; max |diff| at the same "
          f"time {worst_exact * 1000:.6f} mV; {shifted} rows matched within "
          f"+-{args.tol_samples} sample; {len(bad)} failures "
          f"(tol {args.tol_mv} mV / {args.tol_samples} sample)")
    if bad:
        for b in bad[:20]:
            print(f"golden: FAIL {b}")
        sys.exit(1)
    print("golden: PASS")


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)
    for name in ("write", "check"):
        s = sub.add_parser(name)
        s.add_argument("nets")
        s.add_argument("golden")
        s.add_argument("--net", default="LED_A")
    sub.choices["write"].add_argument("--every-us", type=int, default=100)
    sub.choices["check"].add_argument("--tol-mv", type=float, default=1.0)
    sub.choices["check"].add_argument("--tol-samples", type=int, default=1)
    args = p.parse_args()
    (write if args.cmd == "write" else check)(args)


if __name__ == "__main__":
    main()
