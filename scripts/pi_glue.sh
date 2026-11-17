#!/usr/bin/env bash
# Phase 2 milestone P2 (plan section 4 row P2; ADR-009 D5, gates G8 and G9, Amendment 5): the
# rail-tracking, tristate digital_out at pi, with the high-Z leakage, and oe from PIN_CNF.DIR.
#
#   scripts/pi_glue.sh test <bb> <out>   the part in `make test` (pi-glue-test):
#     G9   every Phase 1 board's deck at pi solves its DC operating point with every MCU
#          digital_out high-Z (oe = 0), without the transient-op fallback (`bb run
#          --operating-point --fidelity pi`): the board files with a stand-in rail at voh; the
#          projects (dial-me-spinner at every bench, E3's home and step, adc-kicad) at their
#          benches made pi (scratch copies: fidelity = "pi", and main:U1, the pi-only RT9080, excluded
#          where a bench +3V3 supply stands in for it; adc-kicad gets a +3V3 supply, which its
#          project has none of). Each must have at least one pi output (not vacuous);
#     E1   E1 cold at pi (bench/cold-pi.toml), both firmwares: check_dms_rail.py (the correct order
#          passes; the wrong order fails exactly LSW-3), LSW-3 re-reported with the pessimism of
#          ADR-008 Amendment 3 removed (correct order: max(DIN - VLED) 0 V, at t = 0; while P0.06
#          is high, below 0), the oe edges and the high-Z levels before them;
#     G8   LED_PWR_EN's high level under the stated load (bench/cold-pi-ioh.toml: 0.5 mA, the PS's
#          VOH,SD condition, plus R3 and U2's ON input): VOH >= VDD - 0.4 V, the assertion and
#          the report;
#     the default runs equal the committed one-cell hashes (tests/golden/pi-glue) except grants.
#   scripts/pi_glue.sh gate <bb> <out>   `test`, plus partition independence for every E1 pi case
#          (the one-cell reference --max-quantum 10us --lookahead off and 1 ms grants equal the
#          default run in nets.csv, nets.vcd and the non-grant records; the references equal the
#          committed hashes, grants included), --engine-order analog-first byte-identical, a second
#          default run byte-identical, the fast run of the same case for the side-by-side LSW-3,
#          and --pi-oe fixed's difference (oe = 1 from t = 0) reported.
#   scripts/pi_glue.sh update <bb> <out> run the one-cell references and write tests/golden/pi-glue
#   scripts/pi_glue.sh prepare <bb>      only the project copies (cost measurement)
set -euo pipefail
MODE=$1
shift
ROOT=$(cd "$(dirname "$0")/.." && pwd)
PROJ=$ROOT/examples/dial-me-spinner
E3=$PROJ/e3
FWD=$PROJ/firmware/prebuilt
NRF_FW=$ROOT/engines/cpu-renode/tests/firmware/nrf52840/build
GOLD=$ROOT/tests/golden/pi-glue
WORK=${PI_GLUE_WORK:-${BB_SCRATCH:-$HOME/.cache}/bb-pi-glue}
PY=${PYTHON:-python3}
CASES=(ok-cold wrong-cold ioh-cold)
status=0
pass() { echo "pi-glue: PASS: $*"; }
fail() { echo "pi-glue: FAIL: $*"; status=1; }

prepare() { # <bb>: the project copies (bb flash writes bb.toml) and the pi benches
  local bb=$1 fw d elf
  for fw in ok wrong; do
    d=$WORK/proj-$fw
    elf=led_rail.elf
    [[ $fw == wrong ]] && elf=led_rail_wrong_order.elf
    rm -rf "$d"
    mkdir -p "$d/kicad/main" "$d/firmware/prebuilt"
    cp "$PROJ/bb.toml" "$PROJ/board.bbm" "$PROJ/bb.lock" "$d/"
    cp "$PROJ/kicad/main/dial-me-spinner-main.net" "$d/kicad/main/"
    cp -r "$PROJ/bench" "$d/bench"
    cp "$FWD/$elf" "$d/firmware/prebuilt/"
    "$bb" open --check "$d" > /dev/null 2>&1
    "$bb" flash --project "$d" --target U10 "$d/firmware/prebuilt/$elf" > /dev/null
  done
  d=$WORK/adc-kicad
  rm -rf "$d"
  mkdir -p "$d"
  cp -r "$ROOT/examples/adc/kicad/." "$d/"
  rm -rf "$d/.bb"
  cp "$ROOT/examples/adc/firmware/build/adc_board.elf" "$d/"
  "$bb" open --check "$d" > /dev/null 2>&1
  "$bb" flash --project "$d" --target U1 "$d/adc_board.elf" > /dev/null
  # G9's pi benches: each Phase 1 bench with fidelity = "pi" and, where a bench supply stands in for
  # the RT9080 on +3V3, main:U1 excluded (spec/project.md section 3: two drivers otherwise).
  mkdir -p "$WORK/g9"
  "$PY" - "$WORK/g9" "$PROJ/bench/nominal.toml" "$PROJ/bench/cold.toml" "$PROJ/bench/vsense.toml" \
      "$PROJ/bench/sag.toml" "$PROJ/bench/sag-recover.toml" "$E3/home/bench.toml" "$E3/step/bench.toml" <<'EOF'
import os, sys
out = sys.argv[1]
for src in sys.argv[2:]:
    name = os.path.basename(src)
    if name == "bench.toml":
        name = os.path.basename(os.path.dirname(src)) + ".toml"
    lines = open(src).read().split("\n")
    i = next(k for k, l in enumerate(lines) if l.startswith("schema = "))
    lines.insert(i + 1, 'fidelity = "pi"')
    a = next(k for k, l in enumerate(lines) if l.strip() == "[analog]")
    j = next(k for k in range(a + 1, len(lines)) if lines[k].startswith("["))
    lines.insert(j, 'exclude = ["main:U1"]')
    open(os.path.join(out, name), "w").write("\n".join(lines))
open(os.path.join(out, "adc-kicad.toml"), "w").write(
    '# examples/adc/kicad at pi (milestone P2, G9): its outputs drive from VDD on +3V3, which the\n'
    '# project has no source for; 3.3 V, the Phase 0 voh.\n'
    'schema = "bb-bench/1"\nfidelity = "pi"\n\n[[supply]]\nnet = "+3V3"\nvolts = "3.3V"\n')
EOF
}

# --- G9: the operating point of every Phase 1 board at pi with every oe = 0.
# Since H5 the E3 homing boards' five driver nets are digital-only (`*bb digital`, ADR-014
# D4), so, like e3-project-typ, they have no MCU digital_out on a circuit net and no pi glue;
# e3-step keeps the bridges in scope and its five pi outputs.
g9_cases() {
  cat <<EOF
spike|1|--netlist $ROOT/examples/spike/board.cir --firmware $ROOT/examples/spike/firmware/build/blinky.elf --target U1 --probe net:LED_A
feedback|2|--netlist $ROOT/examples/feedback/board.cir --firmware $ROOT/examples/feedback/firmware/build/feedback.elf --target U1 --probe net:SENSE
feedback-open|0|--netlist $ROOT/examples/feedback-open/board.cir --firmware $ROOT/examples/feedback/firmware/build/feedback.elf --target U1 --probe net:SENSE
adc|2|--netlist $ROOT/examples/adc/board.cir --firmware $ROOT/examples/adc/firmware/build/adc_board.elf --target U1 --probe net:ADC_IN
adc-open|0|--netlist $ROOT/examples/adc/board-open.cir --firmware $ROOT/examples/adc/firmware/build/adc_board.elf --target U1 --probe net:ADC_IN
nrf-feedback|2|--netlist $ROOT/examples/nrf-feedback/board.cir --firmware $NRF_FW/nrf_feedback.elf --target U1 --probe net:SENSE
nrf-saadc|1|--netlist $ROOT/examples/nrf-saadc/board.cir --firmware $NRF_FW/nrf_saadc_burst.elf --target U1 --probe net:AIN
nrf-pwm|4|--netlist $ROOT/examples/nrf-pwm/board.cir --firmware $NRF_FW/nrf_pwm_step.elf --target U1 --probe net:A1
e3-typ|0|--netlist $E3/board.cir --firmware $FWD/homing.elf --target U10 --probe net:VSYS
e3-step|5|--netlist $E3/board-step.cir --firmware $FWD/homing_step.elf --target U10 --probe net:VSYS
e3-worst|0|--netlist $E3/board-ts75.cir --firmware $FWD/homing.elf --target U10 --probe net:VSYS
e3-park|0|--netlist $E3/board-park.cir --firmware $FWD/homing.elf --target U10 --probe net:VSYS
adc-kicad|2|--project $WORK/adc-kicad --bench $WORK/g9/adc-kicad.toml
dms-nominal|2|--project $WORK/proj-ok --bench $WORK/g9/nominal.toml
dms-cold|2|--project $WORK/proj-ok --bench $WORK/g9/cold.toml
dms-vsense|2|--project $WORK/proj-ok --bench $WORK/g9/vsense.toml
dms-sag|2|--project $WORK/proj-ok --bench $WORK/g9/sag.toml
dms-sag-recover|2|--project $WORK/proj-ok --bench $WORK/g9/sag-recover.toml
e3-project-typ|0|--project $E3/home --bench $WORK/g9/home.toml
e3-project-step|5|--project $E3/step --bench $WORK/g9/step.toml
EOF
}

g9() { # <bb> <out>
  local bb=$1 out=$2 name want args n k=0
  mkdir -p "$out/op"
  while IFS='|' read -r name want args; do
    # shellcheck disable=SC2086
    if "$bb" run $args --fidelity pi --operating-point --for 10us --out "$out/op/$name" \
        > "$out/op/$name.log" 2>&1; then
      n=$(sed -n 's/.*operating point solved at pi (\([0-9]*\) pi digital_out.*/\1/p' "$out/op/$name.log")
      # The expected number of pi outputs is stated per deck, so a deck whose outputs stop being
      # glued (a vacuous pass) fails. A deck with none drives no circuit net from the MCU.
      if [[ ${n:-x} == "$want" ]]; then
        k=$((k + 1))
        pass "G9 $name: $(grep -o 'operating point solved.*' "$out/op/$name.log")$([[ $want == 0 ]] \
          && echo ' (no MCU digital_out on a circuit net)')"
      else
        fail "G9 $name: ${n:-no} pi digital_out(s) in the deck, expected $want"
      fi
    else
      fail "G9 $name: the operating point at pi with every oe = 0"; tail -4 "$out/op/$name.log"
    fi
  done < <(g9_cases)
  echo "pi-glue: G9: $k of $(g9_cases | wc -l) Phase 1 decks solve at pi with every oe = 0"
}

args_of() { # <case>
  case $1 in
    ok-cold) echo "--project $WORK/proj-ok --bench $WORK/proj-ok/bench/cold-pi.toml --for 120ms" ;;
    wrong-cold) echo "--project $WORK/proj-wrong --bench $WORK/proj-wrong/bench/cold-pi.toml --for 120ms" ;;
    ioh-cold) echo "--project $WORK/proj-ok --bench $WORK/proj-ok/bench/cold-pi-ioh.toml --for 32ms" ;;
    fast-ok-cold) echo "--project $WORK/proj-ok --bench $WORK/proj-ok/bench/cold.toml --for 120ms" ;;
  esac
}
PROBES=(--probe net:+3V3 --probe net:LED_PWR_EN)

run_case() { # <bb> <dir> <case> [extra bb run args]
  local bb=$1 dir=$2 c=$3
  shift 3
  # shellcheck disable=SC2046
  if ! "$bb" run $(args_of "$c") "${PROBES[@]}" "$@" --out "$dir" > "$dir.log" 2>&1; then
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
  if [[ ! -f $GOLD/$2/outputs.sha256 ]]; then fail "$2: no tests/golden/pi-glue/$2 (make pi-glue-update)"; return; fi
  if [[ $3 == nongrant ]]; then lines=$(grep -v ' events.jsonl$' "$GOLD/$2/outputs.sha256")
  else lines=$(cat "$GOLD/$2/outputs.sha256"); fi
  if (cd "$1" && sha256sum --quiet -c <<< "$lines"); then
    pass "$2: $(basename "$1") equals the committed one-cell hashes ($3)"
  else
    fail "$2: $(basename "$1") differs from tests/golden/pi-glue/$2/outputs.sha256 ($3)"
  fi
}

report() { # <dir> <case>: the checks of one E1 pi case
  local d=$1 c=$2
  case $c in
    ok-cold|wrong-cold)
      local extra=()
      [[ $c == wrong-cold ]] && extra=(--expect-fail LSW-3)
      if "$PY" "$ROOT/scripts/check_dms_rail.py" "$d" --project "$PROJ" --json "$d/checks.json" "${extra[@]}" \
          > "$d/checks.txt"; then
        pass "$c (pi): check_dms_rail.py$([[ $c == wrong-cold ]] && echo ' (exactly LSW-3 fails, as at fast)')"
      else
        fail "$c (pi): check_dms_rail.py"
      fi
      sed 's/^/    /' "$d/checks.txt"
      if [[ $c == ok-cold ]]; then
        if "$PY" "$ROOT/scripts/pi_glue.py" lsw3 "$d" "lsw3_V<=0.000001" "lsw3_p006_high_V<=0" \
            "p006_high_minus_rail_max_V<=0.01" > "$d/lsw3.txt"; then
          pass "ok-cold (pi): LSW-3 with the pessimism removed (fast, voh = 3.3 V: 0.320 V, ADR-008 Amendment 3)"
        else
          fail "ok-cold (pi): LSW-3 re-report"
        fi
      else
        "$PY" "$ROOT/scripts/pi_glue.py" lsw3 "$d" > "$d/lsw3.txt" || true
      fi
      sed 's/^/    /' "$d/lsw3.txt"
      if "$PY" "$ROOT/scripts/pi_glue.py" rail "$d" > "$d/rail.txt" \
          && "$PY" - "$d/rail.txt" <<'EOF'
import json, sys
r = json.loads(open(sys.argv[1]).read().split("json: ")[1])
ok = True
for pin in ("P1.04", "P0.06"):
    e = r[pin]
    ok &= len(e["oe_edges_fs"]) >= 1 and e["oe_edges_fs"][0][1] == 1   # made an output by firmware
    ok &= e["before_oe_max_V"] < 1e-3                                 # high-Z before it
    ok &= e["high_drop_from_rail_max_V"] is None or 0 <= e["high_drop_from_rail_max_V"] < 0.05
sys.exit(0 if ok else 1)
EOF
      then
        pass "$c (pi): oe from PIN_CNF.DIR: high-Z until the firmware makes each pin an output, then high at the rail"
      else
        fail "$c (pi): the oe edges / rail tracking"
      fi
      sed 's/^/    /' "$d/rail.txt"
      ;;
    ioh-cold)
      if "$PY" "$ROOT/scripts/pi_glue.py" ioh "$d" "ioh_mA>=0.5" "voh_minus_vdd_V>=-0.4" "assert=pass" \
          > "$d/ioh.txt"; then
        pass "G8: LED_PWR_EN's high level under the stated load against the PS VOH figure"
      else
        fail "G8: LED_PWR_EN's high level"
      fi
      sed 's/^/    /' "$d/ioh.txt"
      ;;
  esac
}

do_test() { # <bb> <out>
  local bb=$1 out=$2 c
  prepare "$bb"
  g9 "$bb" "$out"
  for c in "${CASES[@]}"; do run_case "$bb" "$out/$c" "$c" & done
  wait
  for c in "${CASES[@]}"; do
    [[ -f $out/$c/non-grant-records ]] || { fail "$c: no run"; continue; }
    report "$out/$c" "$c"
    hashes "$out/$c" "$c" nongrant
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
    same "$out/$c" "$out/$c-ref" "$c: default grants equal the one-cell reference except grants" \
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
  # Side by side: the same case at fast (Phase 1's glue, voh = 3.3 V).
  if run_case "$bb" "$out/fast-ok-cold" fast-ok-cold; then
    "$PY" "$ROOT/scripts/pi_glue.py" lsw3 "$out/fast-ok-cold" > "$out/fast-ok-cold/lsw3.txt" || true
    echo "pi-glue: ok-cold at fast (bench/cold.toml):"
    sed 's/^/    /' "$out/fast-ok-cold/lsw3.txt"
  fi
  # --pi-oe fixed (the fallback, oe = 1 from t = 0): what PIN_CNF.DIR changes.
  if run_case "$bb" "$out/ok-cold-oefixed" ok-cold --pi-oe fixed; then
    "$PY" "$ROOT/scripts/pi_glue.py" rail "$out/ok-cold-oefixed" > "$out/ok-cold-oefixed/rail.txt" || true
    "$PY" "$ROOT/scripts/pi_glue.py" lsw3 "$out/ok-cold-oefixed" > "$out/ok-cold-oefixed/lsw3.txt" || true
    echo "pi-glue: ok-cold with --pi-oe fixed (report):"
    sed 's/^/    /' "$out/ok-cold-oefixed/rail.txt" "$out/ok-cold-oefixed/lsw3.txt"
    if cmp -s "$out/ok-cold/nets.csv" "$out/ok-cold-oefixed/nets.csv"; then
      echo "pi-glue:   nets.csv identical to the PIN_CNF.DIR run"
    else
      "$PY" - "$out/ok-cold/nets.csv" "$out/ok-cold-oefixed/nets.csv" <<'EOF'
import csv, sys
a = list(csv.reader(open(sys.argv[1]))); b = list(csv.reader(open(sys.argv[2])))
h = a[0]; worst = {}
for ra, rb in zip(a[1:], b[1:]):
    for i in range(1, len(h)):
        d = abs(float(ra[i]) - float(rb[i]))
        if d > worst.get(h[i], (0, 0))[0]:
            worst[h[i]] = (d, int(ra[0]))
for n, (d, t) in sorted(worst.items()):
    print(f"pi-glue:   {n}: largest difference {d * 1e3:.3f} mV at {t / 1e12:.3f} ms")
EOF
    fi
  fi
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
      echo "pi-glue: wrote tests/golden/pi-glue/$c/outputs.sha256"
    done
    exit 0 ;;
  test|gate)
    bb=$(realpath "$1"); out=$2
    rm -rf "$out" && mkdir -p "$out"
    t0=$(date +%s.%N)
    "do_$MODE" "$bb" "$out"
    echo "pi-glue: $MODE wall $(awk -v a="$t0" -v b="$(date +%s.%N)" 'BEGIN { printf "%.1f", b - a }') s"
    if [[ $status == 0 ]]; then echo "pi-glue: $MODE: PASS"; else echo "pi-glue: $MODE: FAIL"; fi
    exit $status ;;
  *) echo "usage: scripts/pi_glue.sh test|gate|update|prepare <bb> [<out>]" >&2; exit 2 ;;
esac
