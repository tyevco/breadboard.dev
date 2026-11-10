#!/usr/bin/env bash
# Phase 1 plan M8: E2, the battery reading and the low-battery LED shed on dial-me-spinner rev A
# (examples/dial-me-spinner), the Zephyr firmware (firmware/prebuilt/vsense.elf, vsense_50ms.elf) on
# the nRF52840's SAADC (Breadboard's BbNrf52Saadc) against the main board's imported netlist, with the
# loading adc_in on VSYS_SENSE (ADR-008 D7-D8, plan section 2 E2).
#
# Cases (each a copy of the project off /mnt/c, $DMS_VSENSE_WORK/proj-<name>, the firmware recorded
# with `bb flash`; the repository's project is never written):
#   s1        vsense.elf (a reading every 1 s), bench/vsense.toml, 2100 ms: three 8x bursts
#   s1-none   the same with load = "none" on the VSYS_SENSE binding: the unloaded reference
#   s50       vsense_50ms.elf (every 50 ms), bench/vsense.toml, 650 ms: thirteen bursts
#   s50-none  the same, load = "none"
#   c5        vsense.elf, bench/vsense.toml, 1100 ms, with C5 fitted one decade low (10 nF; the
#             project's netlist edited in the copy): the suite's C5 / 10 fault
#   c5-none   the same, load = "none"
#   sag       vsense.elf, bench/sag.toml, 4300 ms: the battery ramps down through the LED shed
#   rec       vsense.elf, bench/sag-recover.toml, 2300 ms: the battery dips through the shed and comes
#             back above the restore threshold (the restore half of the policy, ADR-008 Amendment 6)
# Every case runs with --probe net:VSYS_SENSE --sample-rate 10kHz. The board is cyclic (the loading
# adc_in: U10 -> ngspice -> U10), so bb's defaults run lookahead; the every-lattice-point reference
# is --lookahead off --adc-push every (one cell, V(hold) pushed at every lattice point).
#
#   scripts/dms_vsense.sh test <bb> <out>   the part in `make test`: s50, s50-none, c5, c5-none at bb's
#                                           defaults, check_dms_vsense.py on them, their committed
#                                           one-cell hashes (non-grant records, nets.csv, nets.vcd), and
#                                           run.json's record of each binding's load (view saadc_se,
#                                           or "none" for an explicit load = "none")
#   scripts/dms_vsense.sh gate <bb> <out>   the M8 gate (make dms-vsense): every case at bb's defaults
#                                           and as its one-cell reference, check_dms_vsense.py (the
#                                           analog side against load = "none", the firmware-visible
#                                           codes, the C5 / 10 fault, the LED shed timing; no
#                                           stop:ngspice), each default run equal to its reference
#                                           except grant records, each reference equal to the committed
#                                           hashes, --engine-order analog-first byte-identical (s50),
#                                           two other grant partitions (s50 at --max-quantum 1ms, and
#                                           at 3ms with --verify every) equal to the committed hashes
#                                           except grants, run.json's load records,
#                                           the TACQ refusal on the project path, and 10-run
#                                           determinism (sag)
#   scripts/dms_vsense.sh ref <bb> <out>    the one-cell references only (for golden-update)
#   scripts/dms_vsense.sh update <out>      write tests/golden/dms-vsense from <out>'s references
#   scripts/dms_vsense.sh prepare <bb>      only make the project copies (cost-table)
#   scripts/dms_vsense.sh run <bb> <out> <cases...>   only run the named cases at bb's defaults, no
#                                           checks (make docs-plots draws from them)
set -euo pipefail
MODE=$1
shift
ROOT=$(cd "$(dirname "$0")/.." && pwd)
PROJ=$ROOT/examples/dial-me-spinner
GOLD=$ROOT/tests/golden/dms-vsense
WORK=${DMS_VSENSE_WORK:-$HOME/.cache/bb-dms-vsense}
DET_RUNS=${DET_RUNS:-10}
DET_JOBS=${DET_JOBS:-5}
PY=${PYTHON:-python3}
CASES=(s1 s1-none s50 s50-none c5 c5-none sag rec)
TEST_CASES=(s50 s50-none c5 c5-none)
status=0
pass() { echo "dms-vsense: PASS: $*"; }
fail() { echo "dms-vsense: FAIL: $*"; status=1; }

proj_of() { # <case>: the project copy
  case $1 in
    s1 | sag | rec) echo "$WORK/proj-1s" ;;
    s1-none) echo "$WORK/proj-1s-none" ;;
    s50) echo "$WORK/proj-50ms" ;;
    s50-none) echo "$WORK/proj-50ms-none" ;;
    c5) echo "$WORK/proj-c5" ;;
    c5-none) echo "$WORK/proj-c5-none" ;;
  esac
}

args_of() { # <case>: bb run arguments
  local p bench=vsense for
  p=$(proj_of "$1")
  case $1 in
    s1 | s1-none) for=2100ms ;;
    s50 | s50-none) for=650ms ;;
    c5 | c5-none) for=1100ms ;;
    sag) for=4300ms bench=sag ;;
    rec) for=2300ms bench=sag-recover ;;
  esac
  echo "--project $p --bench $p/bench/$bench.toml --for $for --probe net:VSYS_SENSE --sample-rate 10kHz"
}

prepare() { # <bb>: the project copies, firmware recorded with bb flash
  local bb=$1 name elf d
  for name in 1s 1s-none 50ms 50ms-none c5 c5-none; do
    d=$WORK/proj-$name
    case $name in 50ms*) elf=vsense_50ms.elf ;; *) elf=vsense.elf ;; esac
    rm -rf "$d"
    mkdir -p "$d/kicad/main" "$d/firmware/prebuilt"
    cp "$PROJ/bb.toml" "$PROJ/board.bbm" "$PROJ/bb.lock" "$d/"
    cp "$PROJ/kicad/main/dial-me-spinner-main.net" "$d/kicad/main/"
    cp -r "$PROJ/bench" "$d/bench"
    cp "$PROJ/firmware/prebuilt/$elf" "$d/firmware/prebuilt/"
    if [[ $name == *-none ]]; then
      # The unloaded reference (ADR-008 D7): an explicit load = "none" on the VSYS_SENSE binding.
      "$PY" - "$d/bb.toml" <<'EOF'
import sys
p = sys.argv[1]
s = open(p).read()
old = 'pin = "main:U10.P0.02"\nmodel = "adc_in"\nnet = "VSYS_SENSE"\n'
assert s.count(old) == 1, "the VSYS_SENSE binding"
open(p, "w").write(s.replace(old, old + 'load = "none"\n'))
EOF
    fi
    if [[ $name == c5* ]]; then
      # The suite's fault: C5 fitted one decade low (100n -> 10n), test_fault_c5_decade_low_detected_at_fw_tacq.
      "$PY" - "$d/kicad/main/dial-me-spinner-main.net" <<'EOF'
import sys
p = sys.argv[1]
s = open(p).read()
old = '(ref "C5")\n\t\t\t(value "100n")'
assert s.count(old) == 1, "C5 100n in the netlist"
open(p, "w").write(s.replace(old, '(ref "C5")\n\t\t\t(value "10n")'))
EOF
      "$bb" open "$d" > /dev/null
    else
      "$bb" open --check "$d" > /dev/null
    fi
    "$bb" flash --project "$d" --target U10 "$d/firmware/prebuilt/$elf" > /dev/null
  done
}

run_case() { # <bb> <dir> <case> [extra bb run args]: start in the background
  local bb=$1 dir=$2 c=$3
  shift 3
  # shellcheck disable=SC2046
  "$bb" run $(args_of "$c") "$@" --out "$dir" > "$dir.log" 2>&1
}

finish() { # <dir> <what>: after a run_case
  grep -v '"type":"grant"' "$1/events.jsonl" > "$1/non-grant-records"
}

run_many() { # <bb> <out> <suffix> <extra args> <cases...>: all in parallel
  local bb=$1 out=$2 sfx=$3 extra=$4 c
  shift 4
  local -A pid=()
  for c in "$@"; do
    # shellcheck disable=SC2086
    run_case "$bb" "$out/$c$sfx" "$c" $extra &
    pid[$c]=$!
  done
  for c in "$@"; do
    if wait "${pid[$c]}"; then
      finish "$out/$c$sfx"
      grep -h 'bb run: simulated\|bb run: lookahead\|bb run: wall' "$out/$c$sfx.log" | sed "s/^/dms-vsense:   $c$sfx: /"
    else
      fail "$c$sfx: bb run failed"; tail -8 "$out/$c$sfx.log"
    fi
  done
}

hashes() { # <dir> <case> <all|nongrant>
  local lines
  [[ -f $GOLD/$2/outputs.sha256 ]] || { fail "$2: no tests/golden/dms-vsense/$2/outputs.sha256"; return 0; }
  if [[ $3 == nongrant ]]; then lines=$(grep -v ' events.jsonl$' "$GOLD/$2/outputs.sha256")
  else lines=$(cat "$GOLD/$2/outputs.sha256"); fi
  if (cd "$1" && sha256sum --quiet -c <<< "$lines"); then
    pass "$2: $(basename "$1") equals the committed one-cell hashes ($3)"
  else
    fail "$2: $(basename "$1") differs from tests/golden/dms-vsense/$2/outputs.sha256 ($3)"
  fi
}

same() { # <a> <b> <what> <files...>
  local a=$1 b=$2 what=$3 f ok=1
  shift 3
  for f in "$@"; do cmp -s "$a/$f" "$b/$f" || { ok=0; fail "$what: $f differs"; }; done
  [[ $ok == 1 ]] && pass "$what ($*)"
  return 0
}

loads() { # <dir> <case>: run.json records the binding's load under the bound pin (ADR-008 D7)
  local want=saadc_se
  [[ $2 == *-none ]] && want=none
  if "$PY" - "$1/run.json" "$want" <<'EOF'; then
import json, sys
r = json.load(open(sys.argv[1]))
v = [a["view"] for a in r["analog"] if a.get("kind") == "pin_load" and a["ref"] == "main:U10.P0.02"]
sys.exit(0 if v == [sys.argv[2]] else f"pin_load records of main:U10.P0.02: {v}")
EOF
    pass "$2: run.json records the VSYS_SENSE binding's load as $want"
  else
    fail "$2: run.json's load record"
  fi
}

checks() { # <out> <json> [--ref] [--cases ...]
  local out=$1 json=$2
  shift 2
  if "$PY" "$ROOT/scripts/check_dms_vsense.py" "$out" --suite "$PROJ/suite/results.json" --json "$json" "$@" \
      > "$json.txt"; then
    pass "check_dms_vsense.py $*"
  else
    fail "check_dms_vsense.py $*"
  fi
  sed 's/^/    /' "$json.txt"
}

do_test() { # <bb> <out>
  local bb=$1 out=$2 c
  prepare "$bb"
  run_many "$bb" "$out" "" "" "${TEST_CASES[@]}"
  checks "$out" "$out/checks.json" --cases "$(IFS=,; echo "${TEST_CASES[*]}")"
  for c in "${TEST_CASES[@]}"; do
    [[ -d $out/$c ]] || continue
    hashes "$out/$c" "$c" nongrant
    loads "$out/$c" "$c"
  done
}

do_ref() { # <bb> <out> <cases...>
  local bb=$1 out=$2
  shift 2
  run_many "$bb" "$out" "-ref" "--lookahead off --adc-push every" "$@"
}

do_gate() { # <bb> <out>
  local bb=$1 out=$2 c
  prepare "$bb"
  run_many "$bb" "$out" "" "" "${CASES[@]}"
  do_ref "$bb" "$out" "${CASES[@]}"
  checks "$out" "$out/checks.json"
  checks "$out" "$out/checks-ref.json" --suffix=-ref
  for c in "${CASES[@]}"; do
    [[ -d $out/$c-ref && -d $out/$c ]] || continue
    same "$out/$c" "$out/$c-ref" "$c: bb's defaults equal the every-lattice-point reference except grants" \
      nets.csv nets.vcd non-grant-records
    hashes "$out/$c-ref" "$c" all
    loads "$out/$c" "$c"
    grep -q '0 promises broken' "$out/$c.log" && pass "$c: 0 promises broken" \
      || fail "$c: a lookahead promise was broken"
  done
  # Engine order: s50 with --engine-order analog-first, byte-identical (grant records included).
  if run_case "$bb" "$out/s50-order" s50 --engine-order analog-first; then
    finish "$out/s50-order"
    same "$out/s50" "$out/s50-order" "s50: --engine-order analog-first byte-identical" nets.csv events.jsonl nets.vcd
  else
    fail "s50-order: bb run failed"; tail -5 "$out/s50-order.log"
  fi
  # Two other grant partitions (s50): every non-grant record, nets.csv and nets.vcd equal the
  # committed one-cell hashes, with no broken promise.
  run_many "$bb" "$out" "-q1ms" "--max-quantum 1ms" s50
  run_many "$bb" "$out" "-q3ms-verify" "--max-quantum 3ms --verify every" s50
  for c in s50-q1ms s50-q3ms-verify; do
    [[ -d $out/$c ]] || continue
    hashes "$out/$c" s50 nongrant
    grep -q '0 promises broken' "$out/$c.log" && pass "$c: 0 promises broken" \
      || fail "$c: a lookahead promise was broken"
  done
  # The TACQ refusal on the project path (ADR-008 D7, approver decision A4): TACQ 40 us with
  # --renode-quantum 25us (2 L = 50 us) fails the run, naming both remedies. The views are
  # qualified at L = 10 us only, so the run names them --allow-unqualified to reach the SAADC.
  local unq=()
  for c in generic/capacitor:ideal generic/resistor:ideal worldsemi/ws2812c-2020:pixel ti/tps22917:lsw \
      nordic/nrf52840-qiaa:saadc_se; do unq+=(--allow-unqualified "$c"); done
  if run_case "$bb" "$out/refuse" s50 --renode-quantum 25us "${unq[@]}"; then
    fail "refuse: the run did not fail"
  elif grep -qF "TACQ 40 us < 2 L = 50 us" "$out/refuse.log" && grep -qF -- "--renode-quantum 20us" "$out/refuse.log" \
      && grep -qF 'load = "none"' "$out/refuse.log"; then
    pass "refuse: $(grep -o 'the SAADC acquired[^(]*' "$out/refuse.log" | head -1)... (both remedies named)"
  else
    fail "refuse: message"; tail -5 "$out/refuse.log"
  fi
  # 10-run determinism (sag).
  # shellcheck disable=SC2046
  if bash "$ROOT/scripts/determinism.sh" "$bb" "$out/determinism" "$DET_RUNS" "$DET_JOBS" -- \
      $(args_of sag) > "$out/determinism.log" 2>&1; then
    pass "sag: $DET_RUNS runs byte-identical ($(find "$out/determinism" -mindepth 1 -maxdepth 1 -type d -name 'run-*' | wc -l) run directories)"
    if cmp -s "$out/determinism/run-01/events.jsonl" "$out/sag/events.jsonl"; then
      pass "sag: the determinism runs equal the gate's run (events.jsonl)"
    else fail "sag: determinism runs differ from the gate's run"; fi
  else
    fail "sag: determinism"; tail -12 "$out/determinism.log"
  fi
}

case $MODE in
  prepare) prepare "$(realpath "$1")"; exit 0 ;;
  update)
    out=$1
    for c in "${CASES[@]}"; do
      mkdir -p "$GOLD/$c"
      (cd "$out/$c-ref" && sha256sum nets.csv events.jsonl nets.vcd non-grant-records) > "$GOLD/$c/outputs.sha256"
    done
    echo "dms-vsense: wrote $GOLD"; exit 0 ;;
esac
BB=$(realpath "$1")
OUT=$2
rm -rf "$OUT" && mkdir -p "$OUT"
case $MODE in
  test) do_test "$BB" "$OUT" ;;
  gate) do_gate "$BB" "$OUT" ;;
  ref) prepare "$BB"; do_ref "$BB" "$OUT" "${CASES[@]}" ;;
  run) shift 2; prepare "$BB"; run_many "$BB" "$OUT" "" "" "$@" ;;
  *) sed -n '2,44p' "$0"; exit 2 ;;
esac
if [[ $status == 0 ]]; then echo "dms-vsense: $MODE PASS"; else echo "dms-vsense: $MODE FAIL"; fi
exit $status
