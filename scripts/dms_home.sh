#!/usr/bin/env bash
# Phase 1 plan M10: E3, the detent homing H0-H5 of dial-me-spinner docs/firmware.md section 5.4 on the
# hand-written board files of examples/dial-me-spinner/e3 (route A, approver decision 1, 2026-11-14;
# ADR-006 Amendment 6, ADR-008 Amendments 9-11): the Zephyr firmware (firmware/prebuilt/homing.elf,
# homing_step.elf) on the nRF52840 with Breadboard's PWM, TEMP and SAADC, the mechanism and DRV5032 device
# M1 (dms/mechanism@1), and ngspice.
#
# Cases (scripts/dms_home_boards.py writes the boards):
#   step   homing_step.elf on board-step.cir, 45 ms: one full step at 446 PPS through the bridges and coils,
#          against the suite's test_firmware_microstep_pwm[VBAT3.7-25C] (1 MHz samples of the coils)
#   typ    homing.elf on board.cir, 2700 ms: homing from the click end, DRV5032FA typical tS 50 ms
#   worst  homing.elf on board-ts75.cir, 2800 ms: the suite's DRV5032FA worst corner (tS 75 ms, BOP
#          4.8 mT, BRP 0.5 mT) at its worst sample phase
#   park   homing.elf on board-park.cir, 3200 ms: from inside the park zone (H1 runs), typical tS 50 ms,
#          BOP/BRP 4.8/0.5 mT (H1 longest)
# The project route (Phase 2 R1, ADR-013 D1; examples/dial-me-spinner/e3/home and e3/step): the same cases
# on the imported main board, M1 a virtual [[device]], U40/U41/R70/J40 the schematic's parts:
#   project-typ   e3/home (homing.elf, M1 s0 = 500, tS 50 ms), 2700 ms; the bench leaves the MOT nets out
#                 of [analog] nets, so they are digital-only (ADR-014 D4: no glue, no pull-downs)
#   project-step  e3/step (homing_step.elf, M1 s0 = 400), 45 ms; the bench keeps the power stage in scope
# Their decks differ from the board files' (elaborated from the KiCad netlist: element and node names, the
# schematic's capacitor values, rDS(on) 0.28 Ohm, no base-board capacitors), so their nets.csv and nets.vcd
# have their own one-cell references and goldens (tests/golden/dms-home/project-*), and
# scripts/compare_project_run.py gates the documented name mapping against the board-file run: the
# non-grant records byte-identical, every shared digital column identical, the analog columns reported.
#
# The boards are cyclic (U10 -> M1 -> U10, and ngspice -> U10 through VSYS_SENSE), so bb loads the
# lookahead plugin and runs lookahead by default; the kernel finds no quiet grant on them (two producers
# feed U10, ADR-003 D5 allows one; ADR-008 Amendment 11), so bb's defaults are one-cell grants too. The
# one-cell reference is --lookahead off. bb's defaults, --lookahead off and --max-quantum 1ms therefore
# build the SAME partition here (every grant record identical): their equality is a consistency check,
# not a partition-equivalence gate. The device's partition independence is gated by bbdev's G6
# (bbdev/tests/scheduler.rs) and make device-smoke (ADR-008 Amendment 12).
#
# Negative controls (make dms-home; ADR-008 Amendment 12): check_dms_home.py must FAIL
#   neg-fast   homing_fast.elf (the 446 PPS phases at 3125 PPS) on board.cir: exactly the step rate
#              measured on the PWM edges, the park-stop contact before H4 and H3's end 20-108 steps from
#              the stop (the device has no pull-out, so the homing itself "succeeds")
#   neg-fb200  board.cir with M1 at tS 200 ms (the DRV5032FB, typical): exactly the HOME_N latency check
#              (its seeded phase gives the FA run's HOME_N instant, so only the device's tS shows it)
#   neg-fb286  the same at tS 286 ms (the FB's maximum): exactly the HOME_N latency check
#
#   scripts/dms_home.sh test <bb> <out>   the part in `make test`: the boards are the generator's output,
#                                         step at bb's defaults and with --engine-order analog-first,
#                                         check_dms_home.py --step, the committed one-cell hashes; and
#                                         project-step beside them (its hashes, compare_project_run.py
#                                         against step)
#   scripts/dms_home.sh gate <bb> <out>   the M10 gate (make dms-home): every case at bb's defaults and as
#                                         its one-cell reference, check_dms_home.py on each, the defaults
#                                         equal to the references (every record, grants included: one
#                                         partition), the references equal to tests/golden/dms-home,
#                                         --engine-order analog-first byte-identical, --max-quantum 1ms
#                                         equal (typ, step), 10-run determinism (typ), and the three
#                                         negative controls
#   scripts/dms_home.sh project <bb> <out>   the R1 gate (make dms-home-project): the project cases at
#                                         bb's defaults and as their one-cell references (equal: one
#                                         partition), the references equal to their goldens,
#                                         check_dms_home.py on project-typ, compare_project_run.py
#                                         against typ and step (run here too), analog-first on
#                                         project-step, and project-typ's grants and digital-only nets
#                                         reported (ADR-014's long grants need them out of the deck)
#   scripts/dms_home.sh ref <bb> <out>    the one-cell references only (for golden-update)
#   scripts/dms_home.sh update <out>      write tests/golden/dms-home from <out>'s references
#   scripts/dms_home.sh run <bb> <out> <cases...>   only run the named cases at bb's defaults, no checks
#                                         (make docs-plots draws from them)
# DMS_HOME_JOBS (default 5) runs at a time: a homing run holds about 1.5 GB (ngspice keeps every
# solver point) and takes about 4.5 minutes alone, 6-7 with five in parallel (the gate: about 45).
set -euo pipefail
MODE=$1
shift
ROOT=$(cd "$(dirname "$0")/.." && pwd)
E3=$ROOT/examples/dial-me-spinner/e3
FWD=$ROOT/examples/dial-me-spinner/firmware/prebuilt
GOLD=$ROOT/tests/golden/dms-home
JOBS=${DMS_HOME_JOBS:-5}
DET_RUNS=${DET_RUNS:-10}
PY=${PYTHON:-python3}
CASES=(step typ worst park)
PCASES=(project-step project-typ)
NEGS=(neg-fast neg-fb200 neg-fb286)
NEG=
status=0
pass() { echo "dms-home: PASS: $*"; }
fail() { echo "dms-home: FAIL: $*"; status=1; }

PINS="--probe net:VSYS --probe net:HOME_N --probe net:MOT_NSLEEP --probe net:U10.P0.26 --probe net:U10.P0.05 \
--probe net:U10.P0.07 --probe net:U10.P0.11 --probe net:U10.P1.15 --probe net:M1.HOME_N"
args_of() { # <case>: bb run arguments
  case $1 in
    step) echo "--netlist $E3/board-step.cir --firmware $FWD/homing_step.elf --target U10 --for 45ms $PINS \
--probe net:MOT_A_OUT1 --probe net:COILA_L --probe net:MOT_B_OUT1 --probe net:COILB_L --probe net:NB_VBAT \
--probe net:NB_VSYS --sample-rate 1MHz" ;;
    typ) echo "--netlist $E3/board.cir --firmware $FWD/homing.elf --target U10 --for 2700ms $PINS --sample-rate 10kHz" ;;
    worst) echo "--netlist $E3/board-ts75.cir --firmware $FWD/homing.elf --target U10 --for 2800ms $PINS --sample-rate 10kHz" ;;
    park) echo "--netlist $E3/board-park.cir --firmware $FWD/homing.elf --target U10 --for 3200ms $PINS --sample-rate 10kHz" ;;
    neg-fast) echo "--netlist $E3/board.cir --firmware $FWD/homing_fast.elf --target U10 --for 2700ms $PINS --sample-rate 10kHz" ;;
    neg-fb200) echo "--netlist $NEG/fb200.cir --firmware $FWD/homing.elf --target U10 --for 2700ms $PINS --sample-rate 10kHz" ;;
    neg-fb286) echo "--netlist $NEG/fb286.cir --firmware $FWD/homing.elf --target U10 --for 2800ms $PINS --sample-rate 10kHz" ;;
    # The project route (R1); the probes are the projects' [probes].
    project-step) echo "--project $E3/step --bench $E3/step/bench.toml --for 45ms --sample-rate 1MHz" ;;
    project-typ) echo "--project $E3/home --bench $E3/home/bench.toml --for 2700ms --sample-rate 10kHz" ;;
  esac
}
check_args() { # <case>
  case $1 in
    step) echo "--step --board $E3/board-step.cir" ;;
    typ) echo "--corner typ" ;;
    worst) echo "--corner worst" ;;
    park) echo "--corner typ --start park" ;;
    project-typ) echo "--corner typ" ;;
  esac
}

# A project run against the board-file run it stands for (R1, ADR-013 D1 gate G1).
cross() { # <board-file run> <project run>
  if "$PY" "$ROOT/scripts/compare_project_run.py" "$1" "$2" > "$2.cross.txt"; then
    pass "$(basename "$2") equals $(basename "$1") after the name mapping (non-grant records, digital columns)"
  else
    fail "$(basename "$2") differs from $(basename "$1") after the name mapping"
  fi
  grep -v ': ok \|PASS\|FAIL (' "$2.cross.txt" | sed 's/^compare_project_run: /    /'
}

# The project files are current (bb run refuses them otherwise; this says why first).
projects() { # <bb>
  local d
  for d in "$E3/home" "$E3/step"; do
    if "$1" open --check "$d" > /dev/null 2>&1; then pass "$(basename "$d"): board.bbm and bb.lock are current"
    else fail "$(basename "$d"): board.bbm or bb.lock is stale (bb open --check $d)"; fi
  done
}

# A job list "<dir>|<case>|<extra args>", run JOBS at a time.
run_jobs() { # <bb> <jobs...>
  local bb=$1 j lines=()
  shift
  for j in "$@"; do
    IFS="|" read -r dir c extra <<< "$j"
    [[ -n $c ]] || c=$(basename "$dir")
    lines+=("$dir|$(args_of "$c") $extra")
  done
  printf '%s\n' "${lines[@]}" | xargs -P "$JOBS" -I{} bash -c '
    IFS="|" read -r dir args <<< "$1"
    # shellcheck disable=SC2086
    if "$2" run $args --out "$dir" > "$dir.log" 2>&1; then
      grep -v "\"type\":\"grant\"" "$dir/events.jsonl" > "$dir/non-grant-records"
    else
      echo "dms-home: FAIL: $(basename "$dir"): bb run failed"; tail -8 "$dir.log"; exit 1
    fi
  ' _ {} "$bb" || true
  for j in "$@"; do
    local dir=${j%%|*}
    if [[ -f $dir/non-grant-records ]]; then
      grep -h 'bb run: simulated\|bb run: wall' "$dir.log" | sed "s|^|dms-home:   $(basename "$dir"): |"
    else
      fail "$(basename "$dir"): no outputs"
    fi
  done
}

hashes() { # <dir> <case> <all|nongrant>
  local lines
  [[ -f $GOLD/$2/outputs.sha256 ]] || { fail "$2: no tests/golden/dms-home/$2/outputs.sha256"; return 0; }
  if [[ $3 == nongrant ]]; then lines=$(grep -v ' events.jsonl$' "$GOLD/$2/outputs.sha256")
  else lines=$(cat "$GOLD/$2/outputs.sha256"); fi
  if (cd "$1" && sha256sum --quiet -c <<< "$lines"); then
    pass "$2: $(basename "$1") equals the committed one-cell hashes ($3)"
  else
    fail "$2: $(basename "$1") differs from tests/golden/dms-home/$2/outputs.sha256 ($3)"
  fi
}

same() { # <a> <b> <what> <files...>
  local a=$1 b=$2 what=$3 f ok=1
  shift 3
  for f in "$@"; do cmp -s "$a/$f" "$b/$f" || { ok=0; fail "$what: $f differs"; }; done
  [[ $ok == 1 ]] && pass "$what ($*)"
  return 0
}

check() { # <dir> <case>
  # shellcheck disable=SC2046
  if "$PY" "$ROOT/scripts/check_dms_home.py" "$1" $(check_args "$2") > "$1.check.txt"; then
    pass "check_dms_home.py $(basename "$1") $(check_args "$2" | sed "s|$E3/||")"
  else
    fail "check_dms_home.py $(basename "$1")"
  fi
  grep -v '^check_dms_home: ok' "$1.check.txt" | sed 's/^/    /'
}

# The negative controls' boards: board.cir with M1 as the DRV5032FB (tS 200 ms typical, 286 ms max).
neg_boards() { # <dir>
  mkdir -p "$1"
  cp "$E3/bb.lock" "$1/bb.lock"
  local ts m1="*bb part M1 package=dms/detent-mechanism s0=500"
  for ts in 200 286; do
    sed "s|^\\${m1} ts=50ms\$|${m1} ts=${ts}ms|" "$E3/board.cir" > "$1/fb$ts.cir"
    grep -qxF "${m1} ts=${ts}ms" "$1/fb$ts.cir" \
      || fail "neg-fb$ts: board.cir's M1 line is not the one the control edits"
  done
}

# A negative control: check_dms_home.py --corner typ must fail exactly the checks <patterns> name
# (grep -E, one failed check each), as E1's FW-6.4 controls fail exactly FW-6.4.
negcheck() { # <dir> <patterns...>
  local d=$1 p ok=1 fails
  shift
  if "$PY" "$ROOT/scripts/check_dms_home.py" "$d" --corner typ > "$d.check.txt"; then
    fail "$(basename "$d"): check_dms_home.py passed a negative control"; return 0
  fi
  fails=$(grep '^check_dms_home: FAIL ' "$d.check.txt" | grep -v '^check_dms_home: FAIL (' || true)
  for p in "$@"; do grep -qE "$p" <<< "$fails" || { ok=0; echo "    missing failure: $p"; }; done
  [[ $(grep -c . <<< "$fails") == "$#" ]] || ok=0
  sed 's/^/    /' <<< "$fails" | cut -c1-260
  if [[ $ok == 1 ]]; then pass "$(basename "$d"): check_dms_home.py fails exactly $# check(s): $*"
  else fail "$(basename "$d"): the negative control does not fail exactly: $*"; fi
}

boards() {
  if "$PY" "$ROOT/scripts/dms_home_boards.py" --check; then pass "the E3 board files are the generator's output"
  else fail "the E3 board files differ from scripts/dms_home_boards.py's output"; fi
}

do_test() { # <bb> <out>
  local bb=$1 out=$2
  boards
  projects "$bb"
  run_jobs "$bb" "$out/step||" "$out/step-order|step|--engine-order analog-first" "$out/project-step||"
  check "$out/step" step
  hashes "$out/step" step nongrant
  same "$out/step" "$out/step-order" "step: --engine-order analog-first byte-identical" nets.csv events.jsonl nets.vcd
  # The project route (R1): its own one-cell hashes, and the board file's run after the name mapping.
  hashes "$out/project-step" project-step nongrant
  cross "$out/step" "$out/project-step"
}

# The R1 gate (Phase 2 plan R1, ADR-013 gate G1): E3's step and typ on the project route.
do_project() { # <bb> <out>
  local bb=$1 out=$2 c
  boards
  projects "$bb"
  run_jobs "$bb" "$out/step||" "$out/typ||" "$out/project-step||" "$out/project-step-ref|project-step|--lookahead off" \
    "$out/project-step-order|project-step|--engine-order analog-first" "$out/project-typ||" \
    "$out/project-typ-ref|project-typ|--lookahead off" "$out/neg-seed|project-step|--seed 2"
  hashes "$out/step" step all
  hashes "$out/typ" typ all
  for c in "${PCASES[@]}"; do
    [[ -d $out/$c && -d $out/$c-ref ]] || continue
    same "$out/$c" "$out/$c-ref" "$c: bb's defaults equal the one-cell reference (grant records included)" \
      nets.csv nets.vcd events.jsonl
    hashes "$out/$c-ref" "$c" all
    grep -q '0 promises broken' "$out/$c.log" && pass "$c: 0 promises broken" \
      || fail "$c: a lookahead promise was broken"
  done
  check "$out/project-typ" project-typ
  same "$out/project-step" "$out/project-step-order" "project-step: --engine-order analog-first byte-identical" \
    nets.csv events.jsonl nets.vcd
  cross "$out/step" "$out/project-step"
  cross "$out/typ" "$out/project-typ"
  # The negative control (plan section 7: the mechanism must act): project-step at another seed moves
  # M1's seeded sample phase, so its device records differ and the comparison must fail on them.
  if "$PY" "$ROOT/scripts/compare_project_run.py" "$out/step" "$out/neg-seed" > "$out/neg-seed.cross.txt"; then
    fail "neg-seed: compare_project_run.py passed a run at --seed 2"
  elif grep -q 'FAIL non-grant records differ' "$out/neg-seed.cross.txt" \
       && grep -q 'first window at' "$out/neg-seed.cross.txt"; then
    pass "neg-seed: compare_project_run.py fails a run at --seed 2 on M1's seeded sample phase"
  else
    fail "neg-seed: compare_project_run.py failed a run at --seed 2, but not on M1's first record"
  fi
  # ADR-014's long grants (H5) need the MOT nets out of the analog engine: report what the project does.
  "$PY" - "$out/project-typ" "$out/typ" <<'EOF'
import json, sys
from pathlib import Path
p, t = Path(sys.argv[1]), Path(sys.argv[2])
rj = json.loads((p / "run.json").read_text())
g = lambda d: sum(1 for ln in (d / "events.jsonl").open() if '"type":"grant"' in ln)
print(f"dms-home:   project-typ: digital-only nets (no glue, ADR-014 D4): {', '.join(rj.get('digital_only', [])) or 'none'}")
print(f"dms-home:   project-typ: {g(p)} grants (the board file's typ: {g(t)}); quiet grants need H5 (ADR-014 D1-D3)")
EOF
  grep -h 'bb run: lookahead' "$out/project-typ.log" | sed 's/^/dms-home:   project-typ: /'
}

do_ref() { # <bb> <out> <cases...>
  local bb=$1 out=$2 c jobs=()
  shift 2
  for c in "$@"; do jobs+=("$out/$c-ref|$c|--lookahead off"); done
  run_jobs "$bb" "${jobs[@]}"
}

do_gate() { # <bb> <out>
  local bb=$1 out=$2 c k jobs=()
  boards
  NEG=$out/neg-boards
  neg_boards "$NEG"
  for c in "${CASES[@]}"; do jobs+=("$out/$c|$c|" "$out/$c-ref|$c|--lookahead off"); done
  for c in "${NEGS[@]}"; do jobs+=("$out/$c|$c|"); done
  for c in typ step; do
    jobs+=("$out/$c-order|$c|--engine-order analog-first" "$out/$c-q1ms|$c|--max-quantum 1ms")
  done
  # The determinism runs of typ (with the default run above, DET_RUNS in all).
  for k in $(seq -w 2 "$DET_RUNS"); do jobs+=("$out/typ-det-$k|typ|"); done
  run_jobs "$bb" "${jobs[@]}"
  for c in "${CASES[@]}"; do
    [[ -d $out/$c && -d $out/$c-ref ]] || continue
    check "$out/$c" "$c"
    same "$out/$c" "$out/$c-ref" "$c: bb's defaults equal the one-cell reference (grant records included)" \
      nets.csv nets.vcd events.jsonl
    hashes "$out/$c-ref" "$c" all
    grep -q '0 promises broken' "$out/$c.log" && pass "$c: 0 promises broken" \
      || fail "$c: a lookahead promise was broken"
  done
  for c in typ step; do
    same "$out/$c" "$out/$c-order" "$c: --engine-order analog-first byte-identical" nets.csv events.jsonl nets.vcd
    hashes "$out/$c-q1ms" "$c" nongrant
    # One partition (ADR-008 Amendment 12): say so instead of calling it partition equivalence.
    if cmp -s <(grep '"type":"grant"' "$out/$c/events.jsonl") <(grep '"type":"grant"' "$out/$c-q1ms/events.jsonl"); then
      echo "dms-home:   $c: --max-quantum 1ms builds the same partition as the defaults ($(grep -c '"type":"grant"' "$out/$c/events.jsonl") grants, no quiet grant)"
    fi
  done
  negcheck "$out/neg-fast" "step rate from the PWM edges" "no park-stop contact before H4" "H3 ends"
  negcheck "$out/neg-fb200" "HOME_N assert latency"
  negcheck "$out/neg-fb286" "HOME_N assert latency"
  local ok=1 n=1
  for k in $(seq -w 2 "$DET_RUNS"); do
    [[ -d $out/typ-det-$k ]] || { ok=0; continue; }
    n=$((n + 1))
    for f in nets.csv events.jsonl nets.vcd; do cmp -s "$out/typ/$f" "$out/typ-det-$k/$f" || ok=0; done
  done
  if [[ $ok == 1 && $n == "$((10#$DET_RUNS))" ]]; then
    pass "typ: $n runs byte-identical (nets.csv, events.jsonl, nets.vcd: $(sha256sum "$out/typ/events.jsonl" | cut -c1-16))"
  else
    fail "typ: determinism ($n runs)"
  fi
}

case $MODE in
  update)
    out=$1
    for c in "${CASES[@]}" "${PCASES[@]}"; do
      mkdir -p "$GOLD/$c"
      (cd "$out/$c-ref" && sha256sum nets.csv events.jsonl nets.vcd non-grant-records) > "$GOLD/$c/outputs.sha256"
    done
    echo "dms-home: wrote $GOLD"; exit 0 ;;
esac
BB=$(realpath "$1")
OUT=$(realpath -m "$2")
rm -rf "$OUT" && mkdir -p "$OUT"
case $MODE in
  test) do_test "$BB" "$OUT" ;;
  gate) do_gate "$BB" "$OUT" ;;
  project) do_project "$BB" "$OUT" ;;
  ref) boards; do_ref "$BB" "$OUT" "${CASES[@]}" "${PCASES[@]}" ;;
  run) shift 2; jobs=(); for c in "$@"; do jobs+=("$OUT/$c|$c|"); done; run_jobs "$BB" "${jobs[@]}" ;;
  *) sed -n '2,/^set -euo/{/^set -euo/!p}' "$0"; exit 2 ;;
esac
if [[ $status == 0 ]]; then echo "dms-home: $MODE PASS"; else echo "dms-home: $MODE FAIL"; fi
exit $status
