#!/usr/bin/env bash
# Phase 2 R2 (ADR-013 D2, gate G2; plan section 4): E1 on the dial-me-spinner system,
# examples/dial-me-spinner/e1-system (simulate = ["main", "base"]): the main board running E1's
# firmware and the base board's power path (a [[battery]] at J2, the BQ25180's `pi` view, the base
# board's capacitors, the link harness) in one deck, at the cold and nominal corners
# (bench/cold.toml, bench/nominal.toml). Each case runs a copy of the project and of what it names
# outside itself (../kicad, ../system/packages, ../firmware/prebuilt) in $DMS_E1_SYSTEM_WORK; the
# repository is never written. The board is E1's: its one-cell reference is --max-quantum 10us.
#
# Cases: ok-cold, ok-nominal (led_rail.elf, as committed); in the gate also wrong-cold
# (led_rail_wrong_order.elf, recorded in a copy with bb flash: must fail exactly LSW-3).
#
#   scripts/dms_e1_system.sh test <bb> <out>   the part in `make test`: ok-cold, ok-nominal and
#                                              ok-cold at --engine-order analog-first, in parallel, at
#                                              bb's defaults: check_system_run.py (it is a system run:
#                                              the base board's path and the harness carry the load),
#                                              check_dms_rail.py (E1's four checks pass), a clean
#                                              PIN_CNF audit, the committed one-cell hashes (nets.csv,
#                                              nets.vcd, non-grant records), analog-first byte-identical
#                                              (grant records included), 120 ms in under
#                                              DMS_E1_SYSTEM_MAX_WALL s (60)
#   scripts/dms_e1_system.sh gate <bb> <out>   the R2 gate (make dms-e1-system): `test`, plus each
#                                              case's one-cell reference (equal to the default run
#                                              except grant records, a true equivalence: 71 grants
#                                              against 12 000; and to the committed hashes, grants
#                                              included), analog-first on ok-nominal, wrong-cold
#                                              (exactly LSW-3 fails), 10-run determinism (ok-cold),
#                                              and THE CROSS-CHECK: each case's pin_edge records
#                                              replayed into the dial-me-spinner suite's own
#                                              led_rail_sequence deck with the suite's battery network
#                                              (vsys_network, at the same corner, with the committed
#                                              fitted TPS22917 view in both; scripts/dms_suite_deck.py
#                                              --battery): VLED within DMS_E1_SYSTEM_LIMIT_MV (10) mV
#                                              at every 10 us sample. Its negative control: ok-cold
#                                              with the cell's R0 doubled (1.0 Ohm) must fail it. The
#                                              cross-check needs DMS_REPO (read with git archive) and
#                                              the suite venv (make dms-suite-check makes it); it
#                                              writes the suite's decks outside the repository
#   scripts/dms_e1_system.sh ref <bb> <out>    the one-cell references only (for update)
#   scripts/dms_e1_system.sh update <out>      write tests/golden/dms-e1-system from <out>'s references
#   scripts/dms_e1_system.sh prepare <bb>      only make the project copies (pool-equiv, cost-table)
set -euo pipefail
MODE=$1
shift
ROOT=$(cd "$(dirname "$0")/.." && pwd)
DMS=$ROOT/examples/dial-me-spinner
GOLD=$ROOT/tests/golden/dms-e1-system
WORK=${DMS_E1_SYSTEM_WORK:-${BB_SCRATCH:-$HOME/.cache}/bb-dms-e1-system}
FOR=${DMS_E1_SYSTEM_FOR:-120ms}
MAX_WALL=${DMS_E1_SYSTEM_MAX_WALL:-60}
LIMIT_MV=${DMS_E1_SYSTEM_LIMIT_MV:-10}
DET_RUNS=${DET_RUNS:-10}
DET_JOBS=${DET_JOBS:-5}
PY=${PYTHON:-python3}
CASES=(ok-cold ok-nominal)
status=0
pass() { echo "dms-e1-system: PASS: $*"; }
fail() { echo "dms-e1-system: FAIL: $*"; status=1; }

prepare() { # <bb>: $WORK/dms with the project as committed (ok), and a copy with the wrong-order firmware
  local bb=$1 d=$WORK/dms
  rm -rf "$d"
  mkdir -p "$d/kicad/main" "$d/kicad/base" "$d/firmware/prebuilt" "$d/system"
  cp "$DMS/kicad/main/dial-me-spinner-main.net" "$d/kicad/main/"
  cp "$DMS/kicad/base/dial-me-spinner-base.net" "$d/kicad/base/"
  cp "$DMS/firmware/prebuilt/led_rail.elf" "$DMS/firmware/prebuilt/led_rail_wrong_order.elf" "$d/firmware/prebuilt/"
  cp -r "$DMS/system/packages" "$d/system/packages"
  for fw in ok wrong; do
    mkdir -p "$d/e1-system-$fw"
    cp "$DMS/e1-system/bb.toml" "$DMS/e1-system/board.bbm" "$DMS/e1-system/bb.lock" "$d/e1-system-$fw/"
    cp -r "$DMS/e1-system/bench" "$d/e1-system-$fw/bench"
    # (The copy sits at another depth than the committed project, so bb open notes that the lock's
    # first-party package paths moved, ADR-015 D7; the tree hashes decide.)
    "$bb" open --check "$d/e1-system-$fw" > "$d/open-$fw.log" 2>&1 || { cat "$d/open-$fw.log"; exit 1; }
  done
  "$bb" flash --project "$d/e1-system-wrong" --target main:U10 "$d/firmware/prebuilt/led_rail_wrong_order.elf" > /dev/null
}

args_of() { # <case>: bb run arguments
  local fw=${1%%-*} bench=${1#*-}
  echo "--project $WORK/dms/e1-system-$fw --bench $WORK/dms/e1-system-$fw/bench/$bench.toml --for $FOR"
}

run_case() { # <bb> <dir> <case> [extra bb run args]
  local bb=$1 dir=$2 c=$3
  shift 3
  # shellcheck disable=SC2046
  if ! "$bb" run $(args_of "$c") "$@" --out "$dir" > "$dir.log" 2>&1; then
    echo "dms-e1-system: FAIL: $c: bb run $* failed"; tail -8 "$dir.log"; return 1
  fi
  grep -v '"type":"grant"' "$dir/events.jsonl" > "$dir/non-grant-records"
}

wall_of() { sed -n 's/^bb run: wall time \([0-9.]*\) s total (.*, \([0-9.]*\) s simulation).*/\1 \2/p' "$1.log"; }

checks() { # <dir> <case>
  local fw=${2%%-*} extra=()
  [[ $fw == wrong ]] && extra=(--expect-fail LSW-3)
  if "$PY" "$ROOT/scripts/check_dms_rail.py" "$1" --project "$WORK/dms/e1-system-$fw" --board main \
      --json "$1/checks.json" "${extra[@]}" > "$1/checks.txt"; then
    pass "$2: check_dms_rail.py$([[ $fw == wrong ]] && echo ' (exactly LSW-3 fails, as expected)')"
  else
    fail "$2: check_dms_rail.py"
  fi
  sed 's/^/    /' "$1/checks.txt"
  if "$PY" "$ROOT/scripts/check_system_run.py" "$1" --json "$1/system.json" > "$1/system.txt"; then
    pass "$2: check_system_run.py (a system run: base board path, harness, grounds, current)"
  else
    fail "$2: check_system_run.py"
  fi
  sed 's/^/    /' "$1/system.txt"
}

audit_clean() { # <dir> <case>: no PIN_CNF finding (ADR-008 D4)
  if grep -q '"type":"log"' "$1/events.jsonl"; then
    fail "$2: PIN_CNF audit not clean:"; grep '"type":"log"' "$1/events.jsonl" | head -5
  else
    pass "$2: PIN_CNF audit clean"
  fi
}

hashes() { # <dir> <case> <all|nongrant>
  local lines
  if [[ $3 == nongrant ]]; then lines=$(grep -v ' events.jsonl$' "$GOLD/$2/outputs.sha256")
  else lines=$(cat "$GOLD/$2/outputs.sha256"); fi
  if (cd "$1" && sha256sum --quiet -c <<< "$lines"); then
    pass "$2: $(basename "$1") equals the committed one-cell hashes ($3)"
  else
    fail "$2: $(basename "$1") differs from tests/golden/dms-e1-system/$2/outputs.sha256 ($3)"
  fi
}

same() { # <a> <b> <what> <files...>
  local a=$1 b=$2 what=$3 f ok=1
  shift 3
  for f in "$@"; do cmp -s "$a/$f" "$b/$f" || { ok=0; fail "$what: $f differs"; }; done
  [[ $ok == 1 ]] && pass "$what ($*)"
  return 0
}

converged() { # <dir> <what>
  local w s
  read -r w s <<< "$(wall_of "$1")"
  if [[ -n $w ]] && awk -v w="$w" -v m="$MAX_WALL" 'BEGIN { exit !(w < m) }' \
      && grep -q "simulated $FOR in" "$1.log"; then
    pass "$2: board-level convergence: $FOR completed in $w s wall ($s s simulation; limit $MAX_WALL s)"
  else
    fail "$2: board-level convergence: wall ${w:-?} s (limit $MAX_WALL s)"
  fi
}

do_test() { # <bb> <out>
  local bb=$1 out=$2 c
  prepare "$bb"
  # Three runs at once (the pool has five slots): ok-cold, ok-nominal, ok-cold analog-first.
  run_case "$bb" "$out/ok-cold" ok-cold & local p1=$!
  run_case "$bb" "$out/ok-nominal" ok-nominal & local p2=$!
  run_case "$bb" "$out/ok-cold-order" ok-cold --engine-order analog-first & local p3=$!
  local r1=0 r2=0 r3=0
  wait $p1 || r1=1
  wait $p2 || r2=1
  wait $p3 || r3=1
  for c in "${CASES[@]}"; do
    if [[ $c == ok-cold && $r1 != 0 ]] || [[ $c == ok-nominal && $r2 != 0 ]]; then
      fail "$c: the run failed"; continue
    fi
    checks "$out/$c" "$c"
    audit_clean "$out/$c" "$c"
    hashes "$out/$c" "$c" nongrant
    converged "$out/$c" "$c (mcu-first)"
  done
  if [[ $r3 == 0 && $r1 == 0 ]]; then
    same "$out/ok-cold" "$out/ok-cold-order" "ok-cold: --engine-order analog-first byte-identical" \
      nets.csv events.jsonl nets.vcd
    converged "$out/ok-cold-order" "ok-cold (analog-first)"
  else
    fail "ok-cold: the analog-first run failed"
  fi
}

do_ref() { # <bb> <out>
  local bb=$1 out=$2 c
  for c in "${CASES[@]}"; do
    run_case "$bb" "$out/$c-ref" "$c" --max-quantum 10us || { status=1; continue; }
    checks "$out/$c-ref" "$c (one-cell reference)"
  done
}

suite_env() { # sets SUITE_PY and SUITE_SRC, or fails
  local repo=${DMS_REPO:-$ROOT/../dial-me-spinner} commit=6854d25e0fde9b5d60da07b2c2b6183fd36dfc5a
  [[ -d $repo/.git || -f $repo/.git ]] || { echo "dms-e1-system: no dial-me-spinner repository at $repo (DMS_REPO)"; return 1; }
  SUITE_SRC=${DMS_PKG_DIR:-${BB_SCRATCH:-$HOME/.cache}/bb-dms-packages}/src-${commit:0:12}
  if [[ ! -f $SUITE_SRC/.complete ]]; then
    rm -rf "$SUITE_SRC" && mkdir -p "$SUITE_SRC"
    git -C "$repo" archive "$commit" | tar -x -C "$SUITE_SRC"
    touch "$SUITE_SRC/.complete"
  fi
  SUITE_PY=${DMS_SUITE_CACHE:-$HOME/.cache/bb-dms-suite}/venv-$(sha256sum "$DMS/suite/requirements.txt" | cut -c1-12)/bin/python
  [[ -x $SUITE_PY ]] || { echo "dms-e1-system: no suite venv ($SUITE_PY; make dms-suite-check makes it)"; return 1; }
}

crosscheck() { # <run dir> <bench name> <label> <gate|control|report> [--battery or nothing]
  local run=$1 bench=$2 label=$3 kind=$4 xc=$WORK/xc
  local gen=$ROOT/scripts/dms_suite_deck.py view="$ROOT/packages/ti/tps22917/analog/lsw_fit.lib:TI_TPS22917_LSW_FIT"
  local name
  name=$(basename "$run")
  mkdir -p "$xc"
  "$SUITE_PY" -I "$gen" deck --src "$SUITE_SRC" --bench "$DMS/e1-system/bench/$bench.toml" --events "$run/events.jsonl" \
    --view "$view" --battery --out "$xc/battery-$name.cir" > /dev/null
  "$SUITE_PY" -I "$gen" run --deck "$xc/battery-$name.cir" --out "$xc/battery-$name.csv" > "$xc/battery-$name.json" \
    || { fail "$label: the suite's battery-network deck did not run: $(cat "$xc/battery-$name.json")"; return; }
  local gate=() r rc
  [[ $kind == report ]] || gate=(--gate)
  if r=$("$SUITE_PY" -I "$gen" compare --replay "$xc/battery-$name.csv" --run "$run/nets.csv" --run-col main:VLED \
      --limit-mv "$LIMIT_MV" "${gate[@]}" --label "$label"); then rc=0; else rc=1; fi
  echo "$r" >> "$OUT_SUMMARY"
  local mv
  mv=$("$PY" -c 'import json, sys; r = json.loads(sys.argv[1]); print("%.3f mV at %.2f ms" % (r["max_dVLED_mV"], r["at_ms"]))' "$r")
  case $kind in
    gate) if [[ $rc == 0 ]]; then pass "$label: VLED within $LIMIT_MV mV of the suite's battery-network deck (max $mv)"
          else fail "$label: VLED differs from the suite's battery-network deck by $mv (limit $LIMIT_MV mV)"; fi ;;
    control) if [[ $rc == 1 ]]; then pass "$label: the cross-check fails it, as it must (max $mv > $LIMIT_MV mV)"
             else fail "$label: the cross-check passed a power path with the cell's R0 doubled ($mv)"; fi ;;
    report) echo "dms-e1-system: REPORT: $label: max |dVLED| $mv" ;;
  esac
}

do_gate() { # <bb> <out>
  local bb=$1 out=$2 c
  OUT_SUMMARY=$out/crosscheck.jsonl
  : > "$OUT_SUMMARY"
  do_test "$bb" "$out"
  do_ref "$bb" "$out"
  for c in "${CASES[@]}"; do
    [[ -d $out/$c-ref && -d $out/$c ]] || continue
    same "$out/$c" "$out/$c-ref" "$c: default (lookahead, 10 ms grants) equals the one-cell reference except grants" \
      nets.csv nets.vcd non-grant-records
    hashes "$out/$c-ref" "$c" all
    converged "$out/$c-ref" "$c (one-cell reference)"
    echo "dms-e1-system: REPORT: $c: grants $(grep -c '"type":"grant"' "$out/$c/events.jsonl") (default) vs \
$(grep -c '"type":"grant"' "$out/$c-ref/events.jsonl") (one-cell reference)"
  done
  if run_case "$bb" "$out/ok-nominal-order" ok-nominal --engine-order analog-first; then
    same "$out/ok-nominal" "$out/ok-nominal-order" "ok-nominal: --engine-order analog-first byte-identical" \
      nets.csv events.jsonl nets.vcd
    converged "$out/ok-nominal-order" "ok-nominal (analog-first)"
  else fail "ok-nominal: the analog-first run failed"; fi
  if run_case "$bb" "$out/wrong-cold" wrong-cold; then
    checks "$out/wrong-cold" wrong-cold
    converged "$out/wrong-cold" "wrong-cold"
  else fail "wrong-cold: the run failed"; fi
  # 10-run determinism (ok-cold).
  # shellcheck disable=SC2046
  if bash "$ROOT/scripts/determinism.sh" "$bb" "$out/determinism" "$DET_RUNS" "$DET_JOBS" -- \
      $(args_of ok-cold) > "$out/determinism.log" 2>&1; then
    pass "ok-cold: $DET_RUNS runs byte-identical"
    if cmp -s "$out/determinism/run-01/events.jsonl" "$out/ok-cold/events.jsonl"; then
      pass "ok-cold: the determinism runs equal the gate's run (events.jsonl)"
    else fail "ok-cold: determinism runs differ from the gate's run"; fi
  else
    fail "ok-cold: determinism"; tail -12 "$out/determinism.log"
  fi
  # The cross-check (gate G2), its negative control, and what the system changes against E1's bench.
  if suite_env; then
    for c in "${CASES[@]}"; do
      [[ -d $out/$c ]] || continue
      crosscheck "$out/$c" "${c#*-}" "$c: the system run vs the suite's battery-network deck" gate
      # Reported: what E1's bench leaves out, at the same edges: the suite's deck with E1's bench
      # supply (Phase 1's replay) against its battery network, both with the fitted view.
      local gen=$ROOT/scripts/dms_suite_deck.py xc=$WORK/xc r
      if "$SUITE_PY" -I "$gen" deck --src "$SUITE_SRC" --bench "$DMS/bench/${c#*-}.toml" --events "$out/$c/events.jsonl" \
            --view "$ROOT/packages/ti/tps22917/analog/lsw_fit.lib:TI_TPS22917_LSW_FIT" --out "$xc/bench-$c.cir" > /dev/null \
          && "$SUITE_PY" -I "$gen" run --deck "$xc/bench-$c.cir" --out "$xc/bench-$c.csv" > /dev/null \
          && r=$("$SUITE_PY" -I "$gen" compare --replay "$xc/battery-$c.csv" --run "$xc/bench-$c.csv" \
                 --label "$c: E1's bench supply vs the suite's battery network (the suite's deck, reported)"); then
        echo "$r" >> "$OUT_SUMMARY"
        echo "dms-e1-system: REPORT: $c: the suite's deck with E1's bench supply vs with its battery network: max |dVLED| $("$PY" -c 'import json, sys; r = json.loads(sys.argv[1]); print("%.3f mV at %.2f ms" % (r["max_dVLED_mV"], r["at_ms"]))' "$r")"
      else
        echo "dms-e1-system: REPORT: $c: the bench-supply replay did not run"
      fi
    done
    local d=$WORK/dms/e1-system-r0x2
    rm -rf "$d" && cp -r "$WORK/dms/e1-system-ok" "$d"
    sed -i 's/^r0 = "0.5"$/r0 = "1.0"/' "$d/bench/cold.toml"
    grep -q '^r0 = "1.0"$' "$d/bench/cold.toml" || fail "negative control: the bench edit did not apply"
    if "$bb" run --project "$d" --bench "$d/bench/cold.toml" --for "$FOR" --out "$out/control-r0x2" \
        > "$out/control-r0x2.log" 2>&1; then
      crosscheck "$out/control-r0x2" cold "negative control: ok-cold with the cell's R0 doubled (1.0 Ohm)" control
    else fail "negative control: the run failed"; tail -5 "$out/control-r0x2.log"; fi
  else
    fail "the cross-check needs DMS_REPO and the suite venv (gate G2)"
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
    echo "dms-e1-system: wrote $GOLD"; exit 0 ;;
esac
BB=$(realpath "$1")
OUT=$2
rm -rf "$OUT" && mkdir -p "$OUT"
case $MODE in
  test) do_test "$BB" "$OUT" ;;
  gate) do_gate "$BB" "$OUT" ;;
  ref) prepare "$BB"; do_ref "$BB" "$OUT" ;;
  *) sed -n '2,45p' "$0"; exit 2 ;;
esac
if [[ $status == 0 ]]; then echo "dms-e1-system: $MODE PASS"; else echo "dms-e1-system: $MODE FAIL"; fi
exit $status
