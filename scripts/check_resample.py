#!/usr/bin/env python3
"""Check that a coarser-sampled nets.csv is a subset of a finer one.

Usage: python3 scripts/check_resample.py <fine_out_dir> <coarse_out_dir>

Every row of <coarse>/nets.csv must equal, byte for byte, the row of
<fine>/nets.csv at the same t_fs. With ADR-003 D2 (ngspice TMAX = the grant
lattice, not min(max_quantum, sample period)), the analog solution does not
depend on the sample rate or the grant partition, so a 1 kHz CSV of a run in
1 ms grants must sample exactly the waveform of the 100 kHz, 10 us-grant run.

Python 3 standard library only.
"""

import csv
import sys
from pathlib import Path


def main():
    if len(sys.argv) != 3:
        print(__doc__)
        sys.exit(2)
    fine_path = Path(sys.argv[1]) / "nets.csv"
    coarse_path = Path(sys.argv[2]) / "nets.csv"
    with open(fine_path, newline="") as f:
        fine_rows = list(csv.reader(f))
    with open(coarse_path, newline="") as f:
        coarse_rows = list(csv.reader(f))
    if fine_rows[0] != coarse_rows[0]:
        print(f"check_resample: FAIL: headers differ: {fine_rows[0]} vs {coarse_rows[0]}")
        sys.exit(1)
    fine = {r[0]: r for r in fine_rows[1:]}
    bad = [r for r in coarse_rows[1:] if fine.get(r[0]) != r]
    if bad:
        for r in bad[:5]:
            print(f"check_resample: row {r} != {fine.get(r[0])}")
        print(f"check_resample: FAIL: {len(bad)} of {len(coarse_rows) - 1} rows differ")
        sys.exit(1)
    print(f"check_resample: {len(coarse_rows) - 1} rows of {coarse_path} identical to "
          f"{fine_path} at the same instants")
    print("check_resample: PASS")


if __name__ == "__main__":
    main()
