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
#   collapse      w1-collapse            w1_rule        reported: 60 mA ramped in, the collapse in the
#                                                        loop (scripts/check_dms_w1_sweep.py)
#   sweep-<L>     w1-sweep-<L> (made     w1_rule        GATED (check_dms_w1_sweep.py): W1's ramped
#                 from w1-collapse)                     static-load sweep, L = 30 to 65 mA with 40-48
#                                                        and 59-62 (decision D-close 5): every load runs,
#                                                        the classes monotone, the plan's sweep rows
#
#   scripts/dms_w1.sh test <bb> <out>     make dms-w1-test (in make test): tests/w1.toml through bb
#                                         test (rule, blank4, off, margin, margin-off at bb's defaults
#                                         and rule with --engine-order analog-first byte-identical;
#                                         test.json, junit.xml), then check_dms_w1.py and the committed
#                                         one-cell hashes (non-grant records) on its outputs
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
ALL=(rule blank4 off blank5 typ min margin margin-off cosim cosim-budget collapse)
GATED=(rule blank4 off margin margin-off)
EQUIV=(rule blank4 off margin margin-off cosim cosim-budget collapse)
# W1's ramped static-load sweep (decision D-close 5; 60 mA is the case `collapse`).
SWEEP=(30 35 40 41 42 43 44 45 46 47 48 50 55 59 61 62 65)
TESTED=(rule blank4 off margin-off)
status=0
pass() { echo "dms-w1: PASS: $*"; }
fail() { echo "dms-w1: FAIL: $*"; status=1; }

bench_of() { case $1 in
  rule|blank4|off|blank5) echo w1-stress ;; typ) echo w1-stress-typ ;; min) echo w1-stress-min ;;
  margin|margin-off) echo w1-margin ;; cosim) echo w1-cosim ;; cosim-budget) echo w1-cosim-budget ;;
  collapse) echo w1-collapse ;; sweep-*) echo "w1-$1" ;; esac; }
image_of() { case $1 in
  rule|typ|min|margin|cosim|cosim-budget|collapse|sweep-*) echo w1_rule ;; blank4) echo w1_blank4 ;;
  blank5) echo w1_blank5 ;; off|margin-off) echo w1_blank_off ;; esac; }
asserts_of() { case $1 in
  rule|blank4|off|blank5|typ|min) echo "--assert $WORK/dms/asserts/w1.toml" ;;
  margin|margin-off) echo "--assert $WORK/dms/asserts/w1-margin.toml" ;; *) echo "" ;; esac; }
blank_of() { case $(image_of "$1") in w1_rule) echo 10 ;; w1_blank4) echo 4 ;; w1_blank5) echo 5 ;; *) echo 0 ;; esac; }
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
  # The sweep's copy: bench/w1-collapse.toml with its ramp to L mA (ramp-<L>mA.csv, the form of
  # ramp-60mA.csv) as bench/w1-sweep-<L>.toml, one per load (bb run is read-only on the project).
  local ma
  mkdir -p "$d/w1-sweep"
  cp "$DMS/w1/bb.toml" "$DMS/w1/board.bbm" "$DMS/w1/bb.lock" "$d/w1-sweep/"
  cp -r "$DMS/w1/bench" "$d/w1-sweep/bench"
  "$bb" open --check "$d/w1-sweep" > "$d/open-sweep.log" 2>&1 || { cat "$d/open-sweep.log"; exit 1; }
  for ma in "${SWEEP[@]}"; do
    sed "s/^0\.0002,0\.06\$/0.0002,$(awk -v a="$ma" 'BEGIN { printf "%g", a / 1000 }')/" \
      "$DMS/w1/bench/ramp-60mA.csv" > "$d/w1-sweep/bench/ramp-${ma}mA.csv"
    sed "s/ramp-60mA\.csv/ramp-${ma}mA.csv/" "$DMS/w1/bench/w1-collapse.toml" > "$d/w1-sweep/bench/w1-sweep-$ma.toml"
  done
}

args_of() { # <case>: bb run arguments
  local p=$WORK/dms/w1-$1
  [[ $1 == sweep-* ]] && p=$WORK/dms/w1-sweep
  echo "--project $p --bench $p/bench/$(bench_of "$1").toml --for $FOR $(asserts_of "$1")"
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
      --json "$1/$c.json" "${extra[@]}" > "$1/$c.txt"; then
    pass "$c: check_dms_w1.py"
  else
    fail "$c: check_dms_w1.py"
  fi
  sed 's/^/    /' "$1/$c.txt"
}

corner_control() { # <out>: check_dms_w1.py reads the corner from run.json, not from its caller
  # The negative control: rule's own outputs with run.json's bench text at 56 mA instead of 55 mA
  # (the waveform still passes every rule check, as a 56 mA run did) must fail exactly `corner`.
  local d=$1/corner-control f
  rm -rf "$d"; mkdir -p "$d"
  for f in events.jsonl nets.csv; do cp "$1/rule/$f" "$d/$f"; done
  sed 's/amps = \\"55mA\\"/amps = \\"56mA\\"/' "$1/rule/run.json" > "$d/run.json"
  if cmp -s "$1/rule/run.json" "$d/run.json"; then fail "corner control: run.json has no 55 mA load to change"; return; fi
  "$PY" "$ROOT/scripts/check_dms_w1.py" "$d" --case rule --json "$d.json" > "$d.txt" || true
  if "$PY" -c "import json,sys; d=json.load(open(sys.argv[1])); f=[c['name'] for c in d['checks'] if c['gated'] and not c['pass']]; print(f); sys.exit(0 if f == ['corner'] and not d['pass'] and '56mA' in d['label'] else 1)" "$d.json" > "$d.failed"; then
    pass "corner control: rule's outputs with run.json at 56 mA fail exactly the corner check ($(grep -o 'W1 stress corner: 56mA[^]]*' "$d.txt" | head -1))"
  else
    fail "corner control: expected exactly [corner] to fail, got $(cat "$d.failed")"
  fi
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
  # Since the technical exit (plan section 5 item 1): tests/w1.toml through bb test (its assertions,
  # the controls' expected cue-held failures, analog-first byte-identical; test.json and junit.xml,
  # which CI publishes with the other out/dms-*-test/junit.xml), then on its outputs what is not an
  # assertion kind: check_dms_w1.py and the committed one-cell hashes. BB_TEST_JOBS (default 5) runs
  # at a time; bb test uses the pool in BB_RENODE_POOL when make provides one (POOL=0: every Renode
  # fresh).
  BB=$1 OUT=$2
  rm -rf "$OUT"; mkdir -p "$OUT"
  t0=$(date +%s.%N)
  if "$BB" test "$DMS/tests/w1.toml" --out "$OUT" --jobs "${BB_TEST_JOBS:-5}" --pool env > "$OUT/bb-test.log" 2>&1; then
    pass "bb test tests/w1.toml: every outcome as expected ($(grep -c '^bb test: PASS ' "$OUT/bb-test.log") cases: W1's assertions at the stress corner, blank4 and off fail exactly cue-held, the margin check, analog-first byte-identical)"
  else
    fail "bb test tests/w1.toml"
  fi
  grep '^bb test: \(PASS\|FAIL\)\|^bb test:   ' "$OUT/bb-test.log" | sed 's/^/    /'
  if "$PY" "$ROOT/scripts/check_bb_test.py" "$OUT" > "$OUT/check_bb_test.txt"; then
    pass "test.json and junit.xml against spec/test.md ($(grep -c '^check_bb_test: ok' "$OUT/check_bb_test.txt") checks)"
  else
    fail "check_bb_test.py"; grep -v '^check_bb_test: ok' "$OUT/check_bb_test.txt"
  fi
  for c in "${TESTED[@]}" margin; do
    [[ -f $OUT/$c/events.jsonl ]] || { fail "$c: no outputs"; continue; }
    grep -v '"type":"grant"' "$OUT/$c/events.jsonl" > "$OUT/$c/non-grant-records"
    checks "$OUT" "$c"
    hashes "$OUT/$c" "$c" nongrant
  done
  corner_control "$OUT"
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
  corner_control "$OUT"
  # W1's ramped static-load sweep with the firmware in the loop (decision D-close 5): every load from
  # 30 to 65 mA runs (40-48 and 59-62 mA among them), 60 mA being the reported case `collapse`.
  sweep_cases=()
  for ma in "${SWEEP[@]}"; do sweep_cases+=("sweep-$ma"); done
  par "$BB" "$OUT" "" "" "${sweep_cases[@]}"
  sweep_args=("60=$OUT/collapse")
  for ma in "${SWEEP[@]}"; do sweep_args+=("$ma=$OUT/sweep-$ma"); done
  if "$PY" "$ROOT/scripts/check_dms_w1_sweep.py" --bench-dir "$WORK/dms/w1-sweep/bench" \
      --gated-50 "$OUT/margin" --gated-55 "$OUT/rule" --json "$OUT/sweep.json" "${sweep_args[@]}" > "$OUT/sweep.txt"; then
    pass "W1's ramped static-load sweep: check_dms_w1_sweep.py (${#sweep_args[@]} loads)"
  else
    fail "W1's ramped static-load sweep: check_dms_w1_sweep.py"
  fi
  sed 's/^/    /' "$OUT/sweep.txt"
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
