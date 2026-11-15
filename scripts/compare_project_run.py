#!/usr/bin/env python3
"""Compare a project run with the board-file run it stands for (Phase 2 R1, ADR-013 D1 gate G1).

The documented name mapping: kernel nets (an MCU pin's `U10.P0.26`, a device pin's `M1.HOME_N`)
are named alike on both routes, so the event stream maps by identity; a project's analog column is
the net's display name (`/Detent actuator & home sensor/MOT_A_OUT1`), which maps to the board file's
node by its leaf name after the last `/` (`MOT_A_OUT1`).

Gated: the non-grant records (`grep -v '"type":"grant"'` of events.jsonl) must be byte-identical,
and so must every digital column (a kernel net) of nets.csv the two runs share, row for row.
Reported: per shared analog column the largest |difference| (the decks differ: the project's is
elaborated from the KiCad netlist, so its element names, node names and some values are not the
board file's), and the columns only one run has.

    compare_project_run.py <board-file run> <project run>
"""
import csv
import sys
from pathlib import Path


def non_grant(d):
    return [ln for ln in (Path(d) / "events.jsonl").read_bytes().splitlines(keepends=True)
            if b'"type":"grant"' not in ln]


def leaf(name):
    return name.rsplit("/", 1)[-1]


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    a, b = sys.argv[1], sys.argv[2]
    fails = 0
    ra, rb = non_grant(a), non_grant(b)
    if ra == rb:
        print(f"compare_project_run: ok   non-grant records byte-identical ({len(ra)} records)")
    else:
        k = next((i for i, (x, y) in enumerate(zip(ra, rb)) if x != y), min(len(ra), len(rb)))
        print(f"compare_project_run: FAIL non-grant records differ ({len(ra)} vs {len(rb)} records; first at "
              f"record {k}: {ra[k][:240] if k < len(ra) else b'(none)'!r} vs {rb[k][:240] if k < len(rb) else b'(none)'!r})")
        fails += 1
    ga = [ln for ln in (Path(a) / "events.jsonl").read_bytes().splitlines() if b'"type":"grant"' in ln]
    gb = [ln for ln in (Path(b) / "events.jsonl").read_bytes().splitlines() if b'"type":"grant"' in ln]
    print(f"compare_project_run: info grant records {'identical' if ga == gb else 'differ'} "
          f"({len(ga)} vs {len(gb)}; reported: the partition is not part of the result)")
    ca = list(csv.reader((Path(a) / "nets.csv").open()))
    cb = list(csv.reader((Path(b) / "nets.csv").open()))
    ha, hb = ca[0], cb[0]
    if len(ca) != len(cb) or any(x[0] != y[0] for x, y in zip(ca, cb)):
        print(f"compare_project_run: FAIL nets.csv rows differ ({len(ca)} vs {len(cb)}, or their instants)")
        return 1
    # Digital columns are kernel nets; the board file probes a net's node name, the project its
    # display name: both map to the leaf name.
    ia = {leaf(n): i for i, n in enumerate(ha) if i}
    ib = {leaf(n): i for i, n in enumerate(hb) if i}
    for n in sorted(set(ia) & set(ib), key=lambda n: ib[n]):
        xa = [r[ia[n]] for r in ca[1:]]
        xb = [r[ib[n]] for r in cb[1:]]
        digital = all("." not in v for v in xa + xb)
        if digital:
            if xa == xb:
                print(f"compare_project_run: ok   {n}: digital column identical")
            else:
                print(f"compare_project_run: FAIL {n}: digital column differs")
                fails += 1
        else:
            d = max(abs(float(x) - float(y)) for x, y in zip(xa, xb))
            same = "identical" if xa == xb else f"max |difference| {d * 1e3:.3f} mV"
            print(f"compare_project_run: info {n}: analog column, {same} (reported)")
    only_a = sorted(set(ia) - set(ib), key=lambda n: ia[n])
    only_b = sorted(set(ib) - set(ia), key=lambda n: ib[n])
    if only_a or only_b:
        print(f"compare_project_run: info columns only in the board-file run: {', '.join(only_a) or '-'}; "
              f"only in the project run: {', '.join(only_b) or '-'}")
    print(f"compare_project_run: {'PASS' if not fails else 'FAIL'} ({b} against {a})")
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main())
