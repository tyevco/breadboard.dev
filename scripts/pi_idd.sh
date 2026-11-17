#!/usr/bin/env bash
# Phase 2 milestone P3 (plan section 4 row P3; ADR-009 D4, gate G6, Amendment 6): the nRF52840's
# supply_load view at pi, its CPU state outputs and a bench [[activity]], on examples/nrf-idd.
#
#   scripts/pi_idd.sh test <bb> <out>   the part in `make test` (pi-idd-test):
#     idd       nrf_duty.elf at bench/pi.toml (50 ms): the total VDD current, (V(VSUP) - V(VDD)) / R1,
#               against the hand calculation of the duty-weighted average on the LDO, the part's
#               default supply mode, which nrf_duty.elf runs on (2.35 uA + 0.249984 x 6.072 mA +
#               0.228 mA HFXO; ADR-009 Amendment 12), within 1 % (scripts/pi_idd.py idd);
#     radio     the same at bench/pi-radio.toml: + #radio_tx from a bench [[activity]] (300 us every
#               1 ms from 0.5 ms) x 10.8 mA, within 1 %; run.json labels the radio bench-scheduled;
#     mode      the negative control of "the mode must match the firmware" (decision D-close 3): the
#               project with [instances."U1"] power_mode = "dcdc" must fail the run, naming
#               POWER.DCDCEN (nrf_duty.elf never enables the DC/DC);
#     control   the negative control: nrf_duty.elf at bench/fast.toml (no supply_load at fast) must
#               fail the same check (the gate is not vacuous);
#     static    static-load mode (bench/pi-static.toml): [[load]] replace = "main:U1" removes the
#               supply_load and its state outputs, and the load (the hand-calculated 1.7483 mA) is the
#               current through R1;
#     lookahead examples/nrf-idd with nrf_feedback.elf (examples/nrf-feedback's loop and firmware) at
#               pi, with its state outputs, takes at most 2x the grants of examples/nrf-feedback's board
#               without them (and of the project at fast), 500 ms, no promise broken; nrf_duty.elf at
#               pi (a wake every 976.6 us) at most 2x its grants at fast;
#     the default pi runs equal the committed one-cell hashes (tests/golden/pi-idd) except grants.
#   scripts/pi_idd.sh gate <bb> <out>   `test`, plus partition independence for each pi case (the
#               one-cell reference --max-quantum 10us --lookahead off and 1 ms grants equal the default
#               run in nets.csv, nets.vcd and the non-grant records; the references equal the committed
#               hashes, grants included), --engine-order analog-first byte-identical and a second
#               default run byte-identical.
#   scripts/pi_idd.sh update <bb> <out> run the one-cell references and write tests/golden/pi-idd
#   scripts/pi_idd.sh prepare <bb>      only the project copies
set -euo pipefail
MODE=$1
shift
ROOT=$(cd "$(dirname "$0")/.." && pwd)
PROJ=$ROOT/examples/nrf-idd
NRF_FW=$ROOT/engines/cpu-renode/tests/firmware/nrf52840/build
GOLD=$ROOT/tests/golden/pi-idd
WORK=${PI_IDD_WORK:-${BB_SCRATCH:-$HOME/.cache}/bb-pi-idd}
PY=${PYTHON:-python3}
CASES=(duty radio feedback)
RADIO=0.0005,0.001,0.0003
status=0
pass() { echo "pi-idd: PASS: $*"; }
fail() { echo "pi-idd: FAIL: $*"; status=1; }

prepare() { # <bb>: one project copy per firmware (bb flash writes bb.toml)
  local bb=$1 fw d
  for fw in duty feedback; do
    d=$WORK/proj-$fw
    rm -rf "$d"
    mkdir -p "$d"
    cp -r "$PROJ/." "$d/"
    rm -rf "$d/.bb"
    cp "$NRF_FW/nrf_$fw.elf" "$d/"
    "$bb" open --check "$d" > /dev/null 2>&1
    "$bb" flash --project "$d" --target U1 "$d/nrf_$fw.elf" > /dev/null
  done
  # The supply mode's negative control (ADR-009 Amendment 12): nrf_duty.elf at power_mode = "dcdc".
  d=$WORK/proj-mode
  rm -rf "$d"
  cp -r "$WORK/proj-duty" "$d"
  printf '\n[instances."U1"]\npower_mode = "dcdc"\n' >> "$d/bb.toml"
}

args_of() { # <case>
  case $1 in
    duty) echo "--project $WORK/proj-duty --bench $WORK/proj-duty/bench/pi.toml --for 50ms" ;;
    radio) echo "--project $WORK/proj-duty --bench $WORK/proj-duty/bench/pi-radio.toml --for 50ms" ;;
    control) echo "--project $WORK/proj-duty --bench $WORK/proj-duty/bench/fast.toml --for 50ms" ;;
    static) echo "--project $WORK/proj-duty --bench $WORK/proj-duty/bench/pi-static.toml --for 50ms" ;;
    mode) echo "--project $WORK/proj-mode --bench $WORK/proj-mode/bench/pi.toml --for 5ms" ;;
    feedback) echo "--project $WORK/proj-feedback --bench $WORK/proj-feedback/bench/pi.toml --for 500ms" ;;
    feedback-fast) echo "--project $WORK/proj-feedback --bench $WORK/proj-feedback/bench/fast.toml --for 500ms" ;;
    board) echo "--netlist $ROOT/examples/nrf-feedback/board.cir --firmware $NRF_FW/nrf_feedback.elf --target U1 --probe net:SENSE --for 500ms" ;;
  esac
}

run_case() { # <bb> <dir> <case> [extra bb run args]
  local bb=$1 dir=$2 c=$3
  shift 3
  # shellcheck disable=SC2046
  if ! "$bb" run $(args_of "$c") "$@" --out "$dir" > "$dir.log" 2>&1; then
    fail "$c: bb run $* failed"; tail -8 "$dir.log"; return 1
  fi
  grep -v '"type":"grant"' "$dir/events.jsonl" > "$dir/non-grant-records"
}

same() { # <a> <b> <what> <files...>
  local a=$1 b=$2 what=$3 f ok=1
  shift 3
  for f in "$@"; do cmp -s "$a/$f" "$b/$f" || { ok=0; fail "$what: $f differs"; }; done
  [[ $ok == 1 ]] && pass "$what ($*)"
  return 0
}

hashes() { # <dir> <case> <all|nongrant>
  local lines
  if [[ ! -f $GOLD/$2/outputs.sha256 ]]; then fail "$2: no tests/golden/pi-idd/$2 (make pi-idd-update)"; return; fi
  if [[ $3 == nongrant ]]; then lines=$(grep -v ' events.jsonl$' "$GOLD/$2/outputs.sha256")
  else lines=$(cat "$GOLD/$2/outputs.sha256"); fi
  if (cd "$1" && sha256sum --quiet -c <<< "$lines"); then
    pass "$2: $(basename "$1") equals the committed one-cell hashes ($3)"
  else
    fail "$2: $(basename "$1") differs from tests/golden/pi-idd/$2/outputs.sha256 ($3)"
  fi
}

broken() { # <log>: the lookahead line's promises broken (0 expected)
  sed -n 's/.*lookahead: .*, \([0-9]*\) promises broken.*/\1/p' "$1"
}

do_test() { # <bb> <out>
  local bb=$1 out=$2 c g_pi g_board g_fast
  prepare "$bb"
  for c in duty radio control static feedback feedback-fast board; do run_case "$bb" "$out/$c" "$c" & done
  wait
  for c in duty radio control static feedback feedback-fast board; do
    [[ -f $out/$c/non-grant-records ]] || fail "$c: no run"
  done
  # The supply mode must match the firmware's regulator (approver decision D-close 3, ADR-009
  # Amendment 12): nrf_duty.elf never sets POWER.DCDCEN, so the run at mode dcdc must fail, naming it;
  # the duty run (mode ldo, the default) passed the same check.
  # shellcheck disable=SC2046
  if "$bb" run $(args_of mode) --out "$out/mode" > "$out/mode.log" 2>&1; then
    fail "supply mode: nrf_duty.elf at power_mode = \"dcdc\" ran (the firmware never enables the DC/DC)"
  elif grep -q 'DCDCEN = 0 at the run.s end' "$out/mode.log" && grep -q '"ldo"' "$out/mode.log"; then
    pass "supply mode: nrf_duty.elf at power_mode = \"dcdc\" fails: $(grep -o 'the run.s idd view draws the DC/DC[^(]*' "$out/mode.log" | head -1)"
  else
    fail "supply mode: the dcdc run failed for another reason"; tail -5 "$out/mode.log"
  fi
  if "$PY" -c 'import json,sys; s=json.load(open(sys.argv[1]))["supply_load"][0]; sys.exit(0 if s.get("mode") == "ldo" else 1)' "$out/duty/run.json"; then
    pass "supply mode: run.json records the duty run's idd at mode ldo (the part's default)"
  else
    fail "supply mode: run.json's supply_load has no mode ldo"
  fi
  # G6, idd: the hand calculation.
  if "$PY" "$ROOT/scripts/pi_idd.py" idd "$out/duty" > "$out/duty/idd.txt"; then
    pass "idd: nrf_duty.elf's total VDD current equals the hand calculation of the duty-weighted average within 1 %"
  else
    fail "idd: nrf_duty.elf's total VDD current against the hand calculation"
  fi
  sed 's/^/    /' "$out/duty/idd.txt"
  if "$PY" "$ROOT/scripts/pi_idd.py" idd "$out/radio" --radio "$RADIO" > "$out/radio/idd.txt"; then
    pass "idd + radio: with a bench [[activity]] on #radio_tx, within 1 %"
  else
    fail "idd + radio: against the hand calculation"
  fi
  sed 's/^/    /' "$out/radio/idd.txt"
  if "$PY" - "$out/duty/run.json" "$out/radio/run.json" <<'EOF'
import json, sys
duty, radio = (json.load(open(f))["supply_load"][0]["states"] for f in sys.argv[1:3])
cpu = {k: "cpu" for k in ("run", "hfxo", "saadc")}
ok = duty == dict(cpu, radio_tx="0", radio_rx="0")
ok &= radio == dict(cpu, radio_tx="bench [[activity]] (bench-scheduled)", radio_rx="0")
sys.exit(0 if ok else 1)
EOF
  then
    pass "run.json: #run, #hfxo, #saadc from the CPU engine; #radio_tx labelled bench-scheduled"
  else
    fail "run.json supply_load states"
  fi
  if "$PY" "$ROOT/scripts/pi_idd.py" idd "$out/control" --expect-fail > "$out/control/idd.txt"; then
    pass "negative control: the same check fails at fast (no supply_load in the deck)"
  else
    fail "negative control: the check passed at fast"
  fi
  sed 's/^/    /' "$out/control/idd.txt"
  # Static-load mode: the bench load stands in for U1's supply_load.
  if "$PY" "$ROOT/scripts/pi_idd.py" static "$out/static" 1.7483mA > "$out/static/static.txt"; then
    pass "static-load mode: [[load]] replace = \"main:U1\" removed the supply_load and its state outputs; the load is the current"
  else
    fail "static-load mode (bench/pi-static.toml)"
  fi
  sed 's/^/    /' "$out/static/static.txt"
  # G6, lookahead: the feedback board with state outputs.
  g_pi=$("$PY" "$ROOT/scripts/pi_idd.py" grants "$out/feedback")
  g_board=$("$PY" "$ROOT/scripts/pi_idd.py" grants "$out/board")
  g_fast=$("$PY" "$ROOT/scripts/pi_idd.py" grants "$out/feedback-fast")
  local n_run
  n_run=$(grep -c '"net":"U1.#run"' "$out/feedback/events.jsonl" || true)
  if (( n_run >= 20 && g_pi <= 2 * g_board && g_pi <= 2 * g_fast )) \
      && [[ $(broken "$out/feedback.log") == 0 ]]; then
    pass "lookahead: nrf_feedback at pi with its state outputs ($n_run #run edges) took $g_pi grants per 500 ms, examples/nrf-feedback's board $g_board, the project at fast $g_fast (limit 2x); no promise broken"
  else
    fail "lookahead: $g_pi grants at pi ($n_run #run edges, $(broken "$out/feedback.log") promises broken) against $g_board (board) and $g_fast (fast)"
  fi
  # The 1 ms waking firmware (nrf_duty.elf: awake a quarter of each RTC period, in a feedback
  # loop, so one-cell grants while awake): the state outputs add no grants either.
  local g_duty g_ctl
  g_duty=$("$PY" "$ROOT/scripts/pi_idd.py" grants "$out/duty")
  g_ctl=$("$PY" "$ROOT/scripts/pi_idd.py" grants "$out/control")
  if (( g_duty <= 2 * g_ctl )) && [[ $(broken "$out/duty.log") == 0 ]]; then
    pass "lookahead: nrf_duty (a wake every 976.6 us) at pi with its state outputs took $g_duty grants per 50 ms, at fast without them $g_ctl (limit 2x); no promise broken"
  else
    fail "lookahead: nrf_duty took $g_duty grants at pi against $g_ctl at fast ($(broken "$out/duty.log") promises broken)"
  fi
  for c in "${CASES[@]}"; do
    [[ -f $out/$c/non-grant-records ]] && hashes "$out/$c" "$c" nongrant
  done
}

do_ref() { # <bb> <out>
  local bb=$1 out=$2 c
  for c in "${CASES[@]}"; do run_case "$bb" "$out/$c-ref" "$c" --max-quantum 10us --lookahead off & done
  wait
}

do_gate() { # <bb> <out>
  local bb=$1 out=$2 c
  do_test "$bb" "$out"
  do_ref "$bb" "$out"
  for c in "${CASES[@]}"; do
    [[ -d $out/$c-ref ]] || continue
    same "$out/$c" "$out/$c-ref" "$c: the default run equals the one-cell reference except grants" \
      nets.csv nets.vcd non-grant-records
    hashes "$out/$c-ref" "$c" all
    run_case "$bb" "$out/$c-1ms" "$c" --max-quantum 1ms \
      && same "$out/$c" "$out/$c-1ms" "$c: 1 ms grants equal the default except grants" \
        nets.csv nets.vcd non-grant-records
    run_case "$bb" "$out/$c-order" "$c" --engine-order analog-first \
      && same "$out/$c" "$out/$c-order" "$c: --engine-order analog-first byte-identical" \
        nets.csv events.jsonl nets.vcd
    run_case "$bb" "$out/$c-again" "$c" \
      && same "$out/$c" "$out/$c-again" "$c: a second run byte-identical" nets.csv events.jsonl nets.vcd
    grep -h 'bb run: simulated\|lookahead:' "$out/$c.log" "$out/$c-ref.log" | sed "s/^/    $c: /"
  done
}

case $MODE in
  prepare) prepare "$(realpath "$1")"; exit 0 ;;
  update)
    bb=$(realpath "$1"); out=$2
    rm -rf "$out" && mkdir -p "$out"
    prepare "$bb"
    do_ref "$bb" "$out"
    for c in "${CASES[@]}"; do
      mkdir -p "$GOLD/$c"
      (cd "$out/$c-ref" && sha256sum nets.csv events.jsonl nets.vcd non-grant-records) > "$GOLD/$c/outputs.sha256"
      echo "pi-idd: wrote tests/golden/pi-idd/$c/outputs.sha256"
    done
    exit 0 ;;
  test|gate)
    bb=$(realpath "$1"); out=$2
    rm -rf "$out" && mkdir -p "$out"
    t0=$(date +%s.%N)
    "do_$MODE" "$bb" "$out"
    echo "pi-idd: $MODE wall $(awk -v a="$t0" -v b="$(date +%s.%N)" 'BEGIN { printf "%.1f", b - a }') s"
    if [[ $status == 0 ]]; then echo "pi-idd: $MODE: PASS"; else echo "pi-idd: $MODE: FAIL"; fi
    exit $status ;;
  *) echo "usage: scripts/pi_idd.sh test|gate|update|prepare <bb> [<out>]" >&2; exit 2 ;;
esac
