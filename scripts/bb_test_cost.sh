#!/usr/bin/env bash
# Milestone B3's budget measurement (plan section 4: "+0 s (replaces scripts)"; ADR-015 D11): the
# three dms-*-test targets as bb test files (scripts/dms_<x>.sh test) against the same targets as the
# Phase 1 scripts ran them (scripts/dms_<x>.sh test-scripted: every case a `bb run` of the script's),
# interleaved, N rounds (BB_TEST_COST_N, default 3), on one warm pool of POOL_SLOTS (5) processes, as
# make test runs them; medians of each target's wall time and of their sum. Under the host
# measurement lock (make bb-test-cost); run it on an otherwise idle host.
#
#   scripts/bb_test_cost.sh <bb> <out>
set -euo pipefail
BB=$(realpath "$1")
OUT=$(realpath -m "$2")
ROOT=$(cd "$(dirname "$0")/.." && pwd)
N=${BB_TEST_COST_N:-3}
rm -rf "$OUT" && mkdir -p "$OUT"
S=${BB_SCRATCH:-$HOME/.cache}
export DMS_RAIL_WORK=$S/bb-test-cost-rail DMS_VSENSE_WORK=$S/bb-test-cost-vsense
TSV=$OUT/times.tsv
printf 'round\tmode\ttarget\tseconds\tstatus\tload1\n' > "$TSV"
for r in $(seq 1 "$N"); do
  # Alternate which mode goes first in each round.
  modes=(test-scripted test)
  (( r % 2 == 0 )) && modes=(test test-scripted)
  for m in "${modes[@]}"; do
    for x in rail vsense home; do
      load=$(cut -d' ' -f1 /proc/loadavg)
      t0=$(date +%s.%N)
      set +e
      bash "$ROOT/scripts/dms_$x.sh" "$m" "$BB" "$OUT/$m-$x-$r" > "$OUT/$m-$x-$r.log" 2>&1
      rc=$?
      set -e
      t=$(echo "$(date +%s.%N) - $t0" | bc)
      printf '%s\t%s\t%s\t%.2f\t%s\t%s\n' "$r" "$m" "dms-$x-test" "$t" "$rc" "$load" >> "$TSV"
      echo "bb-test-cost: round $r: $m dms-$x-test ${t%.*} s (exit $rc, load $load)"
    done
  done
done
python3 - "$TSV" <<'EOF'
import csv, statistics, sys
rows = list(csv.DictReader(open(sys.argv[1]), delimiter="\t"))
bad = [r for r in rows if r["status"] != "0"]
med = lambda xs: statistics.median(xs)
print("bb-test-cost: medians (s): target, Phase 1 scripts, bb test, difference")
tot = {}
for t in ("dms-rail-test", "dms-vsense-test", "dms-home-test"):
    v = {m: [float(r["seconds"]) for r in rows if r["target"] == t and r["mode"] == m] for m in ("test-scripted", "test")}
    a, b = med(v["test-scripted"]), med(v["test"])
    print(f"bb-test-cost:   {t}: {a:.1f}, {b:.1f}, {b - a:+.1f}")
for m in ("test-scripted", "test"):
    rounds = {}
    for r in rows:
        if r["mode"] == m:
            rounds[r["round"]] = rounds.get(r["round"], 0) + float(r["seconds"])
    tot[m] = med(list(rounds.values()))
print(f"bb-test-cost:   sum: {tot['test-scripted']:.1f}, {tot['test']:.1f}, {tot['test'] - tot['test-scripted']:+.1f}"
      f" (N = {len({r['round'] for r in rows})}, load {min(float(r['load1']) for r in rows)}-{max(float(r['load1']) for r in rows)})")
if bad:
    print(f"bb-test-cost: FAIL: {len(bad)} runs exited non-zero: " + ", ".join(f"{r['mode']} {r['target']} round {r['round']}" for r in bad))
    sys.exit(1)
EOF
