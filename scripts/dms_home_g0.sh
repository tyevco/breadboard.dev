#!/usr/bin/env bash
# Phase 2 H5, ADR-014 gate G0 (the prototype measurement, before the rest of H5 is costed): E3 typ
# (homing.elf on examples/dial-me-spinner/e3/board.cir, 2700 ms) measured three ways with the ADR-003
# protocol (interleaved rounds, N = G0_N (default 3), medians of the simulation time, fresh Renode):
#   base   the committed run before H5: <base-bb> on <base-tree>'s board.cir (the MOT nets glued into
#          ngspice with their pull-downs, dms/mechanism@1, one-cell grants); its outputs must equal
#          <base-tree>'s tests/golden/dms-home/typ hashes, so it is the committed run
#   d4     the model change alone (ADR-014 D4): this tree's board.cir (the MOT nets digital-only, no
#          pull-downs) at one-cell grants (--lookahead off)
#   proto  D1-D4: the same board at bb's defaults (lookahead on, quiet grants with several producers)
# proto must equal d4 except grant records (nets.csv, nets.vcd, non-grant records). The gate: the
# protocol gain, median(d4 sim) / median(proto sim), at least G0_MIN (3); below it ADR-014 is
# withdrawn (plan section 4, H5). The model change's own speed-up (base / d4) is reported.
#
#   scripts/dms_home_g0.sh <bb> <out> [<base-bb> <base-tree>]
#
# Without <base-bb> the base case is skipped (the protocol gain needs only d4 and proto). make
# dms-home-g0 runs it under the host measurement lock; run it on an otherwise idle host (about 20
# minutes at N = 3 with the base case).
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
BB=$(realpath "$1")
OUT=$(realpath -m "$2")
BASE_BB=${3:+$(realpath "$3")}
BASE_TREE=${4:+$(realpath "$4")}
N=${G0_N:-3}
MIN=${G0_MIN:-3}
PY=${PYTHON:-python3}
rm -rf "$OUT" && mkdir -p "$OUT"
FWD=examples/dial-me-spinner/firmware/prebuilt
PINS="--probe net:VSYS --probe net:HOME_N --probe net:U10.P0.26 --probe net:U10.P0.05 --probe net:U10.P0.07 \
--probe net:U10.P0.11 --probe net:U10.P1.15 --probe net:M1.HOME_N"
# The committed run's probes, in its order (MOT_NSLEEP was an analog net before D4).
PINS_BASE="--probe net:VSYS --probe net:HOME_N --probe net:MOT_NSLEEP --probe net:U10.P0.26 --probe net:U10.P0.05 \
--probe net:U10.P0.07 --probe net:U10.P0.11 --probe net:U10.P1.15 --probe net:M1.HOME_N"
ARGS="--firmware $FWD/homing.elf --target U10 --for 2700ms --sample-rate 10kHz"
results=$OUT/results.tsv
printf 'round\tcase\tgrants\ttotal_s\tstartup_s\tsim_s\tload1\n' > "$results"
status=0
fail() { echo "dms-home-g0: FAIL: $*"; status=1; }

run_one() { # <round> <case>
  local k=$1 c=$2 dir=$OUT/$2-$1 load
  load=$(cut -d' ' -f1 /proc/loadavg)
  case $c in
    # The committed run: the base tree's board (with MOT_NSLEEP probed, as make dms-home did) and
    # first-party packages.
    base) (cd "$BASE_TREE" && BB_FIRST_PARTY_PACKAGES=$BASE_TREE/packages env -u BB_RENODE_POOL "$BASE_BB" run \
            --netlist examples/dial-me-spinner/e3/board.cir $ARGS $PINS_BASE --out "$dir") \
            > "$dir.log" 2>&1 ;;
    d4) (cd "$ROOT" && env -u BB_RENODE_POOL "$BB" run --netlist examples/dial-me-spinner/e3/board.cir $ARGS $PINS \
          --lookahead off --out "$dir") > "$dir.log" 2>&1 ;;
    proto) (cd "$ROOT" && env -u BB_RENODE_POOL "$BB" run --netlist examples/dial-me-spinner/e3/board.cir $ARGS \
             $PINS --out "$dir") > "$dir.log" 2>&1 ;;
  esac || { echo "dms-home-g0: $c run $k failed:"; tail -5 "$dir.log"; exit 1; }
  grep -v '"type":"grant"' "$dir/events.jsonl" > "$dir/non-grant-records"
  local grants total startup sim
  grants=$(sed -n 's/.* in \([0-9]*\) grants.*/\1/p' "$dir.log")
  total=$(sed -n 's/.*wall time \([0-9.]*\) s total.*/\1/p' "$dir.log")
  startup=$(sed -n 's/.*(\([0-9.]*\) s engine startup.*/\1/p' "$dir.log")
  sim=$(sed -n 's/.*, \([0-9.]*\) s simulation).*/\1/p' "$dir.log")
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$k" "$c" "$grants" "$total" "$startup" "$sim" "$load" >> "$results"
  echo "dms-home-g0: round $k $c: $grants grants, sim $sim s, startup $startup s, total $total s, load $load"
}

cases=(d4 proto)
[[ -n $BASE_BB ]] && cases=(base d4 proto)
echo "dms-home-g0: E3 typ, cases ${cases[*]}, N = $N, interleaved, fresh Renode"
for k in $(seq 1 "$N"); do
  for c in "${cases[@]}"; do run_one "$k" "$c"; done
done

# Equivalence: proto equals d4 except grant records; every repetition of a case is byte-identical.
for k in $(seq 1 "$N"); do
  for f in nets.csv nets.vcd non-grant-records; do
    cmp -s "$OUT/d4-1/$f" "$OUT/proto-$k/$f" || fail "proto-$k: $f differs from d4-1 (D1-D3 must change only grants)"
    cmp -s "$OUT/d4-1/$f" "$OUT/d4-$k/$f" || fail "d4-$k: $f differs from d4-1 (determinism)"
  done
  if [[ -n $BASE_BB ]]; then
    (cd "$OUT/base-$k" && sha256sum --quiet -c "$BASE_TREE/tests/golden/dms-home/typ/outputs.sha256") \
      || fail "base-$k: not the committed run (differs from the base tree's tests/golden/dms-home/typ)"
  fi
done
[[ $status == 0 ]] && echo "dms-home-g0: PASS: proto equals d4 except grant records, every case deterministic$([[ -n $BASE_BB ]] && echo ", base equals the committed typ hashes")"

"$PY" - "$results" "$MIN" <<'EOF' || status=1
import statistics, sys
rows = [l.rstrip("\n").split("\t") for l in open(sys.argv[1])][1:]
lim = float(sys.argv[2])
med = {}
for c in ("base", "d4", "proto"):
    r = [x for x in rows if x[1] == c]
    if not r:
        continue
    sim = statistics.median(float(x[5]) for x in r)
    tot = statistics.median(float(x[3]) for x in r)
    med[c] = sim
    print(f"dms-home-g0: {c:5s}: {r[0][2]} grants; median simulation {sim:.2f} s, total {tot:.2f} s "
          f"(sims {', '.join(x[5] for x in r)}; load {', '.join(x[6] for x in r)})")
if "base" in med:
    print(f"dms-home-g0: the model change alone (base / d4): {med['base'] / med['d4']:.2f}x in simulation time")
    print(f"dms-home-g0: D1-D4 against the committed run (base / proto): {med['base'] / med['proto']:.2f}x")
gain = med["d4"] / med["proto"]
ok = gain >= lim
print(f"dms-home-g0: {'PASS' if ok else 'FAIL'}: the protocol gain (d4 / proto) {gain:.2f}x "
      f"{'>=' if ok else '<'} {lim:g}x in simulation time (ADR-014 G0)")
sys.exit(0 if ok else 1)
EOF
if [[ $status == 0 ]]; then echo "dms-home-g0: PASS"; else echo "dms-home-g0: FAIL"; fi
exit $status
