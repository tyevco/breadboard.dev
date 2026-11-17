#!/usr/bin/env bash
# W1: POF blanking on a dead battery with the firmware in the loop (Phase 2 P4; plan section 2 W1;
# ADR-009 D6, G4, G5, Amendments 2 and 8), on examples/dial-me-spinner/w1 (simulate = ["main", "base"]:
# the BQ25180, the RT9080 and U2 as `pi` views, U10's POWER model BbNrf52Power and the comparator
# view nordic/nrf52840-qiaa:pof). W1 is gated at a LABELLED STRESS CORNER (decision D-W1: 55 mA
# static +3V3 load, suite budget 50 mA, RT9080 worst_dropout; bench/w1-stress.toml), with a gated
# margin check at the suite's 50 mA (bench/w1-margin.toml). Each case runs a copy of the project,
# with its image recorded by `bb flash`, in $WORK:
#
#   case          bench                  image          checks (scripts/check_dms_w1.py)
#   rule          w1-stress              w1_rule        GATED: every assertion of asserts/w1.toml, no
#                                                        POFWARN, the window, the static-load tolerances
#                                                        against the suite at 55 mA, the 4 ms premise
#   blank4        w1-stress              w1_blank4      GATED: exactly cue-held fails; POFWARN at
#                                                        ceil_L of the re-enable, inside rule's window
#   off           w1-stress              w1_blank_off   GATED: exactly cue-held fails; POFWARN within one
#                                                        lattice cell of the window's start
#   blank5        w1-stress              w1_blank5      reported (the suite predicts a pass)
#   typ, min      w1-stress-typ / -min   w1_rule        reported (VPOFTOL 0 and -5 %; POFWARN expected 0)
#   margin        w1-margin              w1_rule        GATED: asserts/w1-margin.toml (no POFWARN)
#   margin-off    w1-margin              w1_blank_off   GATED: the same, the comparator armed through it
#   cosim         w1-cosim               w1_rule        reported (U10's supply_load only)
#   cosim-budget  w1-cosim-budget        w1_rule        reported (plus the budget's remainder)
#
#   scripts/dms_w1.sh test <bb> <out>     make dms-w1-test (in make test): rule, blank4, off and
#                                         margin-off at bb's defaults with their checks and the
#                                         committed one-cell hashes (non-grant records), and rule
#                                         with --engine-order analog-first byte-identical
#   scripts/dms_w1.sh gate <bb> <out>     make dms-w1: every case with its checks; the gated cases
#                                         and the two co-simulations at bb's defaults (10 ms grants,
#                                         lookahead on), at --max-quantum 1ms and as their one-cell
#                                         references (--max-quantum 10us --lookahead off): equal but
#                                         for grant records, the references equal tests/golden/dms-w1;
#                                         analog-first byte-identical (rule, blank4, off); 10-run
#                                         determinism (rule, cosim, cosim-budget); the W-fw test on the model
#                                         (make dms-wfw-test) and the model's register tests
#                                         (make nrf-power-model)
#   scripts/dms_w1.sh ref <bb> <out>      the one-cell references only
#   scripts/dms_w1.sh update <out>        write tests/golden/dms-w1 from <out>'s references
#   scripts/dms_w1.sh prepare <bb>        only make the project copies (pool-equiv, cost-table)
set -euo pipefail
MODE=$1
shift
ROOT=$(cd "$(dirname "$0")/.." && pwd)
DMS=$ROOT/examples/dial-me-spinner
GOLD=$ROOT/tests/golden/dms-w1
WORK=${DMS_W1_WORK:-${BB_SCRATCH:-$HOME/.cache}/bb-dms-w1}
FOR=${DMS_W1_FOR:-20ms}
DET_RUNS=${DET_RUNS:-10}
DET_JOBS=${DET_JOBS:-5}
JOBS=${DMS_W1_JOBS:-6}
PY=${PYTHON:-python3}
ALL=(rule blank4 off blank5 typ min margin margin-off cosim cosim-budget)
GATED=(rule blank4 off margin margin-off)
EQUIV=(rule blank4 off margin margin-off cosim cosim-budget)
TESTED=(rule blank4 off margin-off)
status=0
pass() { echo "dms-w1: PASS: $*"; }
fail() { echo "dms-w1: FAIL: $*"; status=1; }

bench_of() { case $1 in
  rule|blank4|off|blank5) echo w1-stress ;; typ) echo w1-stress-typ ;; min) echo w1-stress-min ;;
  margin|margin-off) echo w1-margin ;; cosim) echo w1-cosim ;; cosim-budget) echo w1-cosim-budget ;; esac; }
image_of() { case $1 in
  rule|typ|min|margin|cosim|cosim-budget) echo w1_rule ;; blank4) echo w1_blank4 ;;
  blank5) echo w1_blank5 ;; off|margin-off) echo w1_blank_off ;; esac; }
asserts_of() { case $1 in
  rule|blank4|off|blank5|typ|min) echo "--assert $WORK/dms/asserts/w1.toml" ;;
  margin|margin-off) echo "--assert $WORK/dms/asserts/w1-margin.toml" ;; *) echo "" ;; esac; }
blank_of() { case $(image_of "$1") in w1_rule) echo 10 ;; w1_blank4) echo 4 ;; w1_blank5) echo 5 ;; *) echo 0 ;; esac; }
label_of() { case $1 in
  rule|blank4|off|blank5) echo "W1 stress corner: 55 mA static +3V3 load (suite budget 50 mA), RT9080 worst_dropout, POFWARN at V28 +5 % (2.94 V)" ;;
  typ) echo "W1 stress corner: 55 mA static +3V3 load (suite budget 50 mA), RT9080 worst_dropout, POFWARN at V28 nominal (2.80 V)" ;;
  min) echo "W1 stress corner: 55 mA static +3V3 load (suite budget 50 mA), RT9080 worst_dropout, POFWARN at V28 -5 % (2.66 V)" ;;
  margin|margin-off) echo "W1 margin check: the suite's 50 mA static +3V3 budget, RT9080 worst_dropout, POFWARN at V28 +5 % (2.94 V)" ;;
  cosim) echo "W1 co-simulation: U10's supply_load only (no static load), RT9080 worst_dropout, POFWARN at V28 +5 % (2.94 V)" ;;
  cosim-budget) echo "W1 co-simulation: U10's supply_load and the budget's 46.7 mA remainder, RT9080 worst_dropout, POFWARN at V28 +5 % (2.94 V)" ;; esac; }
check_case() { case $1 in
  rule|blank4|off|blank5|margin) echo "$1" ;; margin-off) echo margin ;; typ|min) echo corner ;;
  cosim|cosim-budget) echo cosim ;; esac; }

prepare() { # <bb>: $WORK/dms with one project copy per case, its image recorded by bb flash
  local bb=$1 d=$WORK/dms c
  rm -rf "$d"
  mkdir -p "$d/kicad/main" "$d/kicad/base" "$d/firmware/prebuilt" "$d/system" "$d/asserts"
  cp "$DMS/kicad/main/dial-me-spinner-main.net" "$d/kicad/main/"
  cp "$DMS/kicad/base/dial-me-spinner-base.net" "$d/kicad/base/"
  cp "$DMS"/firmware/prebuilt/w1_*.elf "$d/firmware/prebuilt/"
  cp -r "$DMS/system/packages" "$d/system/packages"
  cp "$DMS/asserts/w1.toml" "$DMS/asserts/w1-margin.toml" "$d/asserts/"
  for c in "${ALL[@]}"; do
    mkdir -p "$d/w1-$c"
    cp "$DMS/w1/bb.toml" "$DMS/w1/board.bbm" "$DMS/w1/bb.lock" "$d/w1-$c/"
    cp -r "$DMS/w1/bench" "$d/w1-$c/bench"
    # (The copy sits at another depth than the committed project, so bb open notes that the lock's
    # first-party package paths moved, ADR-015 D7; the tree hashes decide.)
    "$bb" open --check "$d/w1-$c" > "$d/open-$c.log" 2>&1 || { cat "$d/open-$c.log"; exit 1; }
    [[ $(image_of "$c") == w1_rule ]] || \
      "$bb" flash --project "$d/w1-$c" --target main:U10 "$d/firmware/prebuilt/$(image_of "$c").elf" > /dev/null
  done
}

args_of() { # <case>: bb run arguments
  echo "--project $WORK/dms/w1-$1 --bench $WORK/dms/w1-$1/bench/$(bench_of "$1").toml --for $FOR $(asserts_of "$1")"
}

run_case() { # <bb> <dir> <case> [extra bb run args]
  local bb=$1 dir=$2 c=$3
  shift 3
  # shellcheck disable=SC2046
  if ! "$bb" run $(args_of "$c") "$@" --out "$dir" > "$dir.log" 2>&1; then
    echo "dms-w1: FAIL: $c: bb run $* failed"; tail -8 "$dir.log"; return 1
  fi
  grep -v '"type":"grant"' "$dir/events.jsonl" > "$dir/non-grant-records"
}

par() { # <bb> <out> <suffix> <extra args> <cases...>: run cases in parallel, at most $JOBS at once
  local bb=$1 out=$2 sfx=$3 extra=$4 c n=0
  shift 4
  for c in "$@"; do
    # shellcheck disable=SC2086
    run_case "$bb" "$out/$c$sfx" "$c" $extra &
    n=$((n + 1))
    if (( n % JOBS == 0 )); then wait; fi
  done
  wait
}

window_end() { "$PY" -c "import json,sys; print(json.load(open(sys.argv[1]))['report']['2.94 V window'][1])" "$1"; }

checks() { # <out> <case>
  local c=$2 extra=()
  [[ $c == blank4 && -f $1/rule.json ]] && extra=(--window-end-ms "$(window_end "$1/rule.json")")
  if "$PY" "$ROOT/scripts/check_dms_w1.py" "$1/$c" --case "$(check_case "$c")" --blank-ms "$(blank_of "$c")" \
      --label "$(label_of "$c")" --json "$1/$c.json" "${extra[@]}" > "$1/$c.txt"; then
    pass "$c: check_dms_w1.py"
  else
    fail "$c: check_dms_w1.py"
  fi
  sed 's/^/    /' "$1/$c.txt"
}

hashes() { # <dir> <case> <all|nongrant>
  local lines
  if [[ $3 == nongrant ]]; then lines=$(grep -v ' events.jsonl$' "$GOLD/$2/outputs.sha256")
  else lines=$(cat "$GOLD/$2/outputs.sha256"); fi
  if (cd "$1" && sha256sum --quiet -c <<< "$lines"); then
    pass "$2: $(basename "$1") equals the committed one-cell hashes ($3)"
  else
    fail "$2: $(basename "$1") differs from tests/golden/dms-w1/$2/outputs.sha256 ($3)"
  fi
}

same() { # <a> <b> <what> <files...>
  local a=$1 b=$2 what=$3 f ok=1
  shift 3
  for f in "$@"; do cmp -s "$a/$f" "$b/$f" || { ok=0; fail "$what: $f differs"; }; done
  [[ $ok == 1 ]] && pass "$what ($*)"
  return 0
}

refs() { # <bb> <out>
  par "$1" "$2" -ref "--max-quantum 10us --lookahead off" "${EQUIV[@]}"
}

case $MODE in
prepare)
  prepare "$1"
  ;;
test)
  BB=$1 OUT=$2
  rm -rf "$OUT"; mkdir -p "$OUT"
  t0=$(date +%s.%N)
  prepare "$BB"
  par "$BB" "$OUT" "" "" "${TESTED[@]}"
  run_case "$BB" "$OUT/rule-order" rule --engine-order analog-first
  for c in "${TESTED[@]}"; do
    checks "$OUT" "$c"
    hashes "$OUT/$c" "$c" nongrant
  done
  same "$OUT/rule" "$OUT/rule-order" "rule: --engine-order analog-first byte-identical" \
    nets.csv events.jsonl nets.vcd
  echo "dms-w1: test wall $(echo "$(date +%s.%N) - $t0" | bc) s"
  ;;
ref)
  BB=$1 OUT=$2
  rm -rf "$OUT"; mkdir -p "$OUT"
  prepare "$BB"
  refs "$BB" "$OUT"
  ;;
update)
  OUT=$1
  for c in "${EQUIV[@]}"; do
    mkdir -p "$GOLD/$c"
    (cd "$OUT/$c-ref" && sha256sum nets.csv events.jsonl nets.vcd non-grant-records) > "$GOLD/$c/outputs.sha256"
    echo "dms-w1: wrote $GOLD/$c/outputs.sha256"
  done
  ;;
gate)
  BB=$1 OUT=$2
  rm -rf "$OUT"; mkdir -p "$OUT"
  t0=$(date +%s.%N)
  prepare "$BB"
  par "$BB" "$OUT" "" "" "${ALL[@]}"
  for c in rule blank4 off blank5 typ min margin margin-off cosim cosim-budget; do checks "$OUT" "$c"; done
  # Partition independence (ADR-009 G5): 1 ms grants and the one-cell references.
  par "$BB" "$OUT" -1ms "--max-quantum 1ms" "${EQUIV[@]}"
  refs "$BB" "$OUT"
  for c in "${EQUIV[@]}"; do
    same "$OUT/$c" "$OUT/$c-ref" "$c: bb's defaults equal the one-cell reference except grant records" \
      nets.csv nets.vcd non-grant-records
    same "$OUT/$c-1ms" "$OUT/$c-ref" "$c: 1 ms grants equal the one-cell reference except grant records" \
      nets.csv nets.vcd non-grant-records
    hashes "$OUT/$c-ref" "$c" all
    g=$(grep -c '"type":"grant"' "$OUT/$c/events.jsonl"); r=$(grep -c '"type":"grant"' "$OUT/$c-ref/events.jsonl")
    echo "dms-w1: $c: $g grants at bb's defaults, $r one-cell"
  done
  par "$BB" "$OUT" -order "--engine-order analog-first" rule blank4 off
  for c in rule blank4 off; do
    same "$OUT/$c" "$OUT/$c-order" "$c: --engine-order analog-first byte-identical" nets.csv events.jsonl nets.vcd
  done
  # Determinism (ADR-009 G5; plan section 2 W1: the co-simulation's too): DET_RUNS runs each of
  # rule, cosim and cosim-budget, byte-identical.
  for c in rule cosim cosim-budget; do
    for k in $(seq 1 "$DET_RUNS"); do
      run_case "$BB" "$OUT/det-$c-$k" "$c" &
      (( k % DET_JOBS == 0 )) && wait
    done
    wait
    ok=1
    for k in $(seq 2 "$DET_RUNS"); do
      for f in nets.csv events.jsonl nets.vcd; do cmp -s "$OUT/det-$c-1/$f" "$OUT/det-$c-$k/$f" || ok=0; done
    done
    if [[ $ok == 1 ]]; then pass "$c: $DET_RUNS runs byte-identical (nets.csv events.jsonl nets.vcd)"
    else fail "$c: $DET_RUNS runs differ"; fi
  done
  echo "dms-w1: gate wall $(echo "$(date +%s.%N) - $t0" | bc) s"
  ;;
*)
  echo "usage: dms_w1.sh test|gate|ref|update|prepare ..." >&2; exit 2 ;;
esac
if [[ $MODE == test || $MODE == gate ]]; then
  if [[ $status == 0 ]]; then echo "dms-w1: $MODE: PASS"; else echo "dms-w1: $MODE: FAIL"; fi
fi
exit $status
