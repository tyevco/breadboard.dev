#!/usr/bin/env bash
# W2: the S4 brown-out reboot loop on a dead battery with the firmware in the loop (Phase 2 P5; plan
# section 2 W2; ADR-009 D7, G4, G5, Amendment 9), on examples/dial-me-spinner/w2 (simulate = ["main",
# "base"]: the BQ25180, the RT9080, the OLED load switch U21 and the averaged TPS61040 boost U20 as `pi`
# views, U10's POWER model with its brownout reset: the `bor` output of nordic/nrf52840-qiaa:pof). Both
# suite cases run in static-load mode (the suite's 50 mA +3V3 budget in place of U10's supply_load), USB
# at ILIM 100 mA. Each case runs a copy of the project, with its image recorded by `bb flash`, in $WORK:
#
#   case      bench        image         for     checks (scripts/check_dms_w2.py)
#   old       w2-old       w2_old_rule   20 ms   GATED: asserts/w2-old.toml; the reset records at #bor's
#                                                crossings; the hold; RESETREAS 0 after the release; the
#                                                OLED enabled again by the reboot; the VBOR crossing (typ,
#                                                1.60 V) against the suite; REPORTED: each OLED attempt
#   old-min   w2-old-min   w2_old_rule   8 ms    GATED: the VBOR crossing at 1.57 V against the suite
#   old-max   w2-old-max   w2_old_rule   8 ms    GATED: the VBOR crossing at 1.63 V against the suite
#   rule      w2-rule      w2_rule       20 ms   GATED: asserts/w2-rule.toml (no reset, +3V3 >= 2.7 V,
#                                                OLED off); VSYS and +3V3 minima against the suite
#   old-200   w2-old       w2_old_rule   200 ms  GATED as old; REPORTED: the brown-outs in 200 ms (the
#                                                plan's window), each OLED attempt's minima, the loop
#   rule-200  w2-rule      w2_rule       200 ms  GATED as rule
#
# The equivalence set (old, rule, old-200, rule-200): bb's defaults and 1 ms grants equal the one-cell
# references but for grant records, and the references equal tests/golden/dms-w2.
#
#   scripts/dms_w2.sh test <bb> <out>     make dms-w2-test (in make test): old and rule at bb's defaults
#                                         with their checks and the committed one-cell hashes (non-grant
#                                         records), and rule with --engine-order analog-first
#                                         byte-identical
#   scripts/dms_w2.sh gate <bb> <out>     make dms-w2: every case with its checks; the equivalence set at
#                                         bb's defaults (10 ms grants, lookahead on), at --max-quantum 1ms
#                                         and as one-cell references (--max-quantum 10us --lookahead off):
#                                         equal but for grant records, the references equal
#                                         tests/golden/dms-w2; analog-first byte-identical (old, rule);
#                                         10-run determinism (old, rule); no promise broken
#   scripts/dms_w2.sh ref <bb> <out>      the one-cell references only
#   scripts/dms_w2.sh update <out>        write tests/golden/dms-w2 from <out>'s references
#   scripts/dms_w2.sh prepare <bb>        only make the project copies (pool-equiv, cost-table)
set -euo pipefail
MODE=$1
shift
ROOT=$(cd "$(dirname "$0")/.." && pwd)
DMS=$ROOT/examples/dial-me-spinner
GOLD=$ROOT/tests/golden/dms-w2
WORK=${DMS_W2_WORK:-${BB_SCRATCH:-$HOME/.cache}/bb-dms-w2}
DET_RUNS=${DET_RUNS:-10}
DET_JOBS=${DET_JOBS:-5}
JOBS=${DMS_W2_JOBS:-6}
PY=${PYTHON:-python3}
ALL=(old old-min old-max rule old-200 rule-200)
EQUIV=(old rule old-200 rule-200)
TESTED=(old rule)
status=0
pass() { echo "dms-w2: PASS: $*"; }
fail() { echo "dms-w2: FAIL: $*"; status=1; }

bench_of() { case $1 in
  old|old-200) echo w2-old ;; old-min) echo w2-old-min ;; old-max) echo w2-old-max ;;
  rule|rule-200) echo w2-rule ;; esac; }
image_of() { case $1 in rule|rule-200) echo w2_rule ;; *) echo w2_old_rule ;; esac; }
for_of() { case $1 in old|rule) echo 20ms ;; old-min|old-max) echo 8ms ;; *-200) echo 200ms ;; esac; }
asserts_of() { case $1 in
  old|old-200) echo "--assert $WORK/dms/asserts/w2-old.toml" ;;
  rule|rule-200) echo "--assert $WORK/dms/asserts/w2-rule.toml" ;; *) echo "" ;; esac; }
check_of() { case $1 in old|old-200) echo "--case old --vbor 1.60" ;; old-min) echo "--case corner --vbor 1.57" ;;
  old-max) echo "--case corner --vbor 1.63" ;; rule|rule-200) echo "--case rule" ;; esac; }

prepare() { # <bb>: $WORK/dms with one project copy per case, its image recorded by bb flash
  local bb=$1 d=$WORK/dms c
  rm -rf "$d"
  mkdir -p "$d/kicad/main" "$d/kicad/base" "$d/firmware/prebuilt" "$d/system" "$d/asserts"
  cp "$DMS/kicad/main/dial-me-spinner-main.net" "$d/kicad/main/"
  cp "$DMS/kicad/base/dial-me-spinner-base.net" "$d/kicad/base/"
  cp "$DMS"/firmware/prebuilt/w2_*.elf "$d/firmware/prebuilt/"
  cp -r "$DMS/system/packages" "$d/system/packages"
  cp "$DMS/asserts/w2-old.toml" "$DMS/asserts/w2-rule.toml" "$d/asserts/"
  for c in "${ALL[@]}"; do
    mkdir -p "$d/w2-$c"
    cp "$DMS/w2/bb.toml" "$DMS/w2/board.bbm" "$DMS/w2/bb.lock" "$d/w2-$c/"
    cp -r "$DMS/w2/bench" "$d/w2-$c/bench"
    "$bb" open --check "$d/w2-$c" > "$d/open-$c.log" 2>&1 || { cat "$d/open-$c.log"; exit 1; }
    [[ $(image_of "$c") == w2_old_rule ]] || \
      "$bb" flash --project "$d/w2-$c" --target main:U10 "$d/firmware/prebuilt/$(image_of "$c").elf" > /dev/null
  done
}

args_of() { # <case>: bb run arguments
  echo "--project $WORK/dms/w2-$1 --bench $WORK/dms/w2-$1/bench/$(bench_of "$1").toml --for $(for_of "$1") $(asserts_of "$1")"
}

run_case() { # <bb> <dir> <case> [extra bb run args]
  local bb=$1 dir=$2 c=$3
  shift 3
  # shellcheck disable=SC2046
  if ! "$bb" run $(args_of "$c") "$@" --out "$dir" > "$dir.log" 2>&1; then
    echo "dms-w2: FAIL: $c: bb run $* failed"; tail -8 "$dir.log"; return 1
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

checks() { # <out> <case>
  local c=$2
  # shellcheck disable=SC2046
  if "$PY" "$ROOT/scripts/check_dms_w2.py" "$1/$c" $(check_of "$c") --json "$1/$c.json" > "$1/$c.txt"; then
    pass "$c: check_dms_w2.py"
  else
    fail "$c: check_dms_w2.py"
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
    fail "$2: $(basename "$1") differs from tests/golden/dms-w2/$2/outputs.sha256 ($3)"
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
  echo "dms-w2: test wall $(echo "$(date +%s.%N) - $t0" | bc) s"
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
    echo "dms-w2: wrote $GOLD/$c/outputs.sha256"
  done
  ;;
gate)
  BB=$1 OUT=$2
  rm -rf "$OUT"; mkdir -p "$OUT"
  t0=$(date +%s.%N)
  prepare "$BB"
  # Every case at bb's defaults; the equivalence set at 1 ms grants and as one-cell references.
  par "$BB" "$OUT" "" "" "${ALL[@]}"
  par "$BB" "$OUT" -1ms "--max-quantum 1ms" "${EQUIV[@]}"
  refs "$BB" "$OUT"
  for c in "${ALL[@]}"; do checks "$OUT" "$c"; done
  for c in "${EQUIV[@]}"; do
    same "$OUT/$c" "$OUT/$c-ref" "$c: defaults equal the one-cell reference except grants" \
      nets.csv nets.vcd non-grant-records
    same "$OUT/$c-1ms" "$OUT/$c-ref" "$c: 1 ms grants equal the one-cell reference except grants" \
      nets.csv nets.vcd non-grant-records
    hashes "$OUT/$c-ref" "$c" all
  done
  for c in "${TESTED[@]}"; do
    run_case "$BB" "$OUT/$c-order" "$c" --engine-order analog-first
    same "$OUT/$c" "$OUT/$c-order" "$c: --engine-order analog-first byte-identical" nets.csv events.jsonl nets.vcd
  done
  # 10-run determinism.
  for c in "${TESTED[@]}"; do
    mkdir -p "$OUT/det-$c"
    n=0
    for k in $(seq 1 "$DET_RUNS"); do
      run_case "$BB" "$OUT/det-$c/$k" "$c" &
      n=$((n + 1))
      if (( n % DET_JOBS == 0 )); then wait; fi
    done
    wait
    first=$(cd "$OUT/det-$c/1" && sha256sum nets.csv events.jsonl nets.vcd)
    ok=1
    for k in $(seq 2 "$DET_RUNS"); do
      [[ $(cd "$OUT/det-$c/$k" && sha256sum nets.csv events.jsonl nets.vcd) == "$first" ]] || ok=0
    done
    if [[ $ok == 1 ]]; then pass "$c: $DET_RUNS runs byte-identical"; else fail "$c: $DET_RUNS runs differ"; fi
  done
  # No promise broken in any run (bb run fails the run on one; here the summary line says it).
  if grep -h "promises broken" "$OUT"/*.log | grep -qv " 0 promises broken"; then
    fail "a promise was broken"; grep -h "promises broken" "$OUT"/*.log | sort | uniq -c
  else
    pass "no promise broken ($(grep -l "promises broken" "$OUT"/*.log | wc -l) runs with lookahead)"
  fi
  echo "dms-w2: gate wall $(echo "$(date +%s.%N) - $t0" | bc) s"
  ;;
*)
  echo "usage: $0 test|gate|ref <bb> <out> | update <out> | prepare <bb>" >&2
  exit 2
  ;;
esac
[[ $status == 0 ]] && echo "dms-w2: $MODE: PASS" || echo "dms-w2: $MODE: FAIL"
exit $status
