#!/usr/bin/env bash
# Phase 1 plan M6: E1, LED rail sequencing on dial-me-spinner rev A (examples/dial-me-spinner), the
# Zephyr firmware on the nRF52840 against the main board's imported netlist, at the nominal and cold
# benches (ADR-006, ADR-007, ADR-008; plan section 2 E1).
#
# Cases: <firmware>-<bench>, firmware ok (firmware/prebuilt/led_rail.elf) or wrong
# (led_rail_wrong_order.elf), bench nominal or cold. Each case runs a copy of the project off
# /mnt/c ($DMS_RAIL_WORK/proj-<firmware>, the firmware recorded with `bb flash`; the repository's
# project is never written). The board is acyclic (the MCU drives, nothing reads back), so its
# one-cell reference is --max-quantum 10us.
#
#   scripts/dms_rail.sh test <bb> <out>     the part in `make test`: every case at bb's defaults,
#                                           check_dms_rail.py (ok passes; wrong fails exactly LSW-3),
#                                           the committed one-cell hashes (non-grant records, nets.csv,
#                                           nets.vcd), the VLED snapshots, a clean PIN_CNF audit, and
#                                           --engine-order analog-first byte-identical on ok-cold, and
#                                           check_dms_rail.py's two FW-6.4 negative controls (ok-cold's
#                                           pin_edge records moved; each fails exactly FW-6.4)
#   scripts/dms_rail.sh gate <bb> <out>     the M6 gate (make dms-rail): `test`, plus every case's
#                                           one-cell reference run (equal to the default run except
#                                           grant records, and to the committed hashes, grant records
#                                           included), analog-first on every case, the board-level
#                                           convergence gate (each run completes 120 ms in under
#                                           DMS_RAIL_MAX_WALL s, at both engine orders), the PIN_CNF
#                                           audit's negative control, and 10-run determinism
#   scripts/dms_rail.sh ref <bb> <out>      the one-cell references only (for golden-update)
#   scripts/dms_rail.sh update <out>        write tests/golden/dms-rail from <out>'s references
#   scripts/dms_rail.sh prepare <bb>        only make the project copies (pool-equiv, cost-table)
set -euo pipefail
MODE=$1
shift
ROOT=$(cd "$(dirname "$0")/.." && pwd)
PROJ=$ROOT/examples/dial-me-spinner
GOLD=$ROOT/tests/golden/dms-rail
WORK=${DMS_RAIL_WORK:-$HOME/.cache/bb-dms-rail}
FOR=${DMS_RAIL_FOR:-120ms}
MAX_WALL=${DMS_RAIL_MAX_WALL:-30}
DET_RUNS=${DET_RUNS:-10}
DET_JOBS=${DET_JOBS:-5}
PY=${PYTHON:-python3}
CASES=(ok-nominal ok-cold wrong-nominal wrong-cold)
status=0
pass() { echo "dms-rail: PASS: $*"; }
fail() { echo "dms-rail: FAIL: $*"; status=1; }

elf_of() { [[ $1 == ok ]] && echo led_rail.elf || echo led_rail_wrong_order.elf; }

prepare() { # <bb>: the two project copies, firmware recorded with bb flash
  local bb=$1
  for fw in ok wrong; do
    local d=$WORK/proj-$fw elf
    elf=$(elf_of "$fw")
    rm -rf "$d"
    mkdir -p "$d/kicad/main" "$d/firmware/prebuilt"
    cp "$PROJ/bb.toml" "$PROJ/board.bbm" "$PROJ/bb.lock" "$d/"
    cp "$PROJ/kicad/main/dial-me-spinner-main.net" "$d/kicad/main/"
    cp -r "$PROJ/bench" "$d/bench"
    cp "$PROJ/firmware/prebuilt/$elf" "$d/firmware/prebuilt/"
    "$bb" open --check "$d" > /dev/null
    "$bb" flash --project "$d" --target U10 "$d/firmware/prebuilt/$elf" > /dev/null
  done
}

args_of() { # <case>: bb run arguments
  local fw=${1%%-*} bench=${1#*-}
  echo "--project $WORK/proj-$fw --bench $WORK/proj-$fw/bench/$bench.toml --for $FOR"
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

wall_of() { sed -n 's/^bb run: wall time \([0-9.]*\) s total (.*, \([0-9.]*\) s simulation).*/\1 \2/p' "$1.log"; }

checks() { # <dir> <case>
  local fw=${2%%-*} extra=()
  [[ $fw == wrong ]] && extra=(--expect-fail LSW-3)
  if "$PY" "$ROOT/scripts/check_dms_rail.py" "$1" --project "$PROJ" --json "$1/checks.json" "${extra[@]}" \
      > "$1/checks.txt"; then
    pass "$2: check_dms_rail.py$([[ $fw == wrong ]] && echo ' (exactly LSW-3 fails, as expected)')"
  else
    fail "$2: check_dms_rail.py"
  fi
  sed 's/^/    /' "$1/checks.txt"
}

audit_clean() { # <dir> <case>: no PIN_CNF finding (ADR-008 D4)
  if grep -q '"type":"log"' "$1/events.jsonl"; then
    fail "$2: PIN_CNF audit not clean:"; grep '"type":"log"' "$1/events.jsonl" | head -5
  else
    pass "$2: PIN_CNF audit clean (P1.04 S0S1, P0.06 H0H1, unbound GPIOs on scope nets not outputs)"
  fi
}

hashes() { # <dir> <case> <all|nongrant>
  local lines
  if [[ $3 == nongrant ]]; then lines=$(grep -v ' events.jsonl$' "$GOLD/$2/outputs.sha256")
  else lines=$(cat "$GOLD/$2/outputs.sha256"); fi
  if (cd "$1" && sha256sum --quiet -c <<< "$lines"); then
    pass "$2: $(basename "$1") equals the committed one-cell hashes ($3)"
  else
    fail "$2: $(basename "$1") differs from tests/golden/dms-rail/$2/outputs.sha256 ($3)"
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
    pass "$2: convergence: $FOR completed in $w s wall ($s s simulation; limit $MAX_WALL s)"
  else
    fail "$2: convergence: wall ${w:-?} s (limit $MAX_WALL s)"
  fi
}

snapshots() { # <dir> <case>
  [[ ${2%%-*} == ok ]] || return 0
  if "$PY" "$ROOT/scripts/golden.py" check "$1/nets.csv" "$GOLD/$2/vled.csv" --net VLED > "$1/golden.txt"; then
    pass "$2: VLED snapshot within 1 mV / 1 sample"
  else
    fail "$2: VLED snapshot"; cat "$1/golden.txt"
  fi
}

fw_rule_controls() { # <out>: check_dms_rail.py's FW-6.4 negative controls (post-M7 review)
  # ok-cold's outputs with its pin_edge records moved: (a) the first P0.06 rise 5 ms after
  # LED_PWR_EN (VLED has settled by then, so only FW-6.4 can see it), (b) the second LED_PWR_EN
  # rise 20 ms after the fall. Each must fail exactly FW-6.4.
  local out=$1 k d
  [[ -d $out/ok-cold ]] || return 0
  for k in on-wait off-wait; do
    d=$out/fw-rule-$k
    rm -rf "$d" && mkdir -p "$d"
    cp "$out/ok-cold/nets.csv" "$out/ok-cold/run.json" "$d/"
    "$PY" - "$out/ok-cold/events.jsonl" "$d/events.jsonl" "$k" <<'EOF'
import json, sys
src, dst, kind = sys.argv[1:]
recs = [json.loads(l) for l in open(src)]
edge = lambda r, net, lv: r.get("type") == "pin_edge" and r["net"] == net and r["level"] == lv
if kind == "on-wait":
    en = next(r for r in recs if edge(r, "U10.P1.04", 1))
    din = next(r for r in recs if edge(r, "U10.P0.06", 1))
    din["t_fs"] = en["t_fs"] + 5_000_000_000_000
else:
    fall = next(r for r in recs if edge(r, "U10.P1.04", 0))
    rise = [r for r in recs if edge(r, "U10.P1.04", 1)][1]
    rise["t_fs"] = fall["t_fs"] + 20_000_000_000_000
with open(dst, "w") as f:
    for r in recs:
        f.write(json.dumps(r, separators=(",", ":")) + "\n")
EOF
    if ! "$PY" "$ROOT/scripts/check_dms_rail.py" "$d" --project "$PROJ" --json "$d/checks.json" > "$d/checks.txt" \
        && "$PY" -c 'import json, sys; sys.exit(json.load(open(sys.argv[1]))["failed"] != ["FW-6.4"])' "$d/checks.json"; then
      pass "FW-6.4 negative control ($k): $(grep -o 'FAIL: FW-6.4: [^(]*' "$d/checks.txt")"
    else
      fail "FW-6.4 negative control ($k): expected exactly FW-6.4 to fail"; cat "$d/checks.txt"
    fi
  done
}

do_test() { # <bb> <out>
  local bb=$1 out=$2 c
  prepare "$bb"
  for c in "${CASES[@]}"; do
    run_case "$bb" "$out/$c" "$c" || continue
    checks "$out/$c" "$c"
    audit_clean "$out/$c" "$c"
    hashes "$out/$c" "$c" nongrant
    snapshots "$out/$c" "$c"
    converged "$out/$c" "$c (mcu-first)"
  done
  fw_rule_controls "$out"
  if run_case "$bb" "$out/ok-cold-order" ok-cold --engine-order analog-first; then
    same "$out/ok-cold" "$out/ok-cold-order" "ok-cold: --engine-order analog-first byte-identical" \
      nets.csv events.jsonl nets.vcd
    converged "$out/ok-cold-order" "ok-cold (analog-first)"
  fi
}

do_ref() { # <bb> <out>
  local bb=$1 out=$2 c
  for c in "${CASES[@]}"; do
    run_case "$bb" "$out/$c-ref" "$c" --max-quantum 10us || continue
    checks "$out/$c-ref" "$c (one-cell reference)"
  done
}

do_gate() { # <bb> <out>
  local bb=$1 out=$2 c
  do_test "$bb" "$out"
  do_ref "$bb" "$out"
  for c in "${CASES[@]}"; do
    [[ -d $out/$c-ref ]] || continue
    same "$out/$c" "$out/$c-ref" "$c: default (10 ms grants) equals the one-cell reference except grants" \
      nets.csv nets.vcd non-grant-records
    hashes "$out/$c-ref" "$c" all
    if [[ $c != ok-cold ]] && run_case "$bb" "$out/$c-order" "$c" --engine-order analog-first; then
      same "$out/$c" "$out/$c-order" "$c: --engine-order analog-first byte-identical" nets.csv events.jsonl nets.vcd
      converged "$out/$c-order" "$c (analog-first)"
    fi
  done
  # The PIN_CNF audit's negative control: P0.06 bound with a standard-drive set while the
  # firmware configures H0H1 must give a warn record (so a clean audit above means something).
  local d=$WORK/proj-audit
  rm -rf "$d" && cp -r "$WORK/proj-ok" "$d"
  sed -i 's/^set = "H0H1_max"$/set = "S0S1"/' "$d/bb.toml"
  if "$bb" run --project "$d" --bench "$d/bench/cold.toml" --for 40ms --out "$out/audit-control" \
      > "$out/audit-control.log" 2>&1 \
      && grep -q '"type":"log".*"level":"warn".*P0.06.*H0H1' "$out/audit-control/events.jsonl"; then
    pass "PIN_CNF audit negative control: P0.06 bound S0S1, firmware H0H1: $(grep -o '"msg":"[^"]*"' \
      "$out/audit-control/events.jsonl" | head -1)"
  else
    fail "PIN_CNF audit negative control: no warn record"; tail -5 "$out/audit-control.log"
  fi
  # ... and P0.06 left unbound (its [[bind]] removed): the firmware makes it an output on an
  # analog-scope net that gets no glue, which the audit reports (ADR-008 D4, unbound pins).
  d=$WORK/proj-audit-unbound
  rm -rf "$d" && cp -r "$WORK/proj-ok" "$d"
  awk 'BEGIN { RS = ""; ORS = "\n\n" } !/pin = "main:U10.P0.06"/' "$WORK/proj-ok/bb.toml" > "$d/bb.toml"
  sed -i 's/"LED_DIN_MCU", //' "$d/bb.toml"
  if "$bb" run --project "$d" --bench "$d/bench/cold.toml" --for 40ms --out "$out/audit-unbound" \
      > "$out/audit-unbound.log" 2>&1 \
      && grep -q '"type":"log".*"level":"warn".*P0.06' "$out/audit-unbound/events.jsonl"; then
    pass "PIN_CNF audit negative control: P0.06 unbound: $(grep -o '"msg":"[^"]*"' \
      "$out/audit-unbound/events.jsonl" | head -1)"
  else
    fail "PIN_CNF audit negative control (unbound): no warn record"; tail -5 "$out/audit-unbound.log"
  fi
  # 10-run determinism (ok-cold).
  # shellcheck disable=SC2046
  if bash "$ROOT/scripts/determinism.sh" "$bb" "$out/determinism" "$DET_RUNS" "$DET_JOBS" -- \
      $(args_of ok-cold) > "$out/determinism.log" 2>&1; then
    pass "ok-cold: $DET_RUNS runs byte-identical ($(grep -c . <<< "$(ls -d "$out"/determinism/run-*/)") runs)"
    if cmp -s "$out/determinism/run-01/events.jsonl" "$out/ok-cold/events.jsonl"; then
      pass "ok-cold: the determinism runs equal the gate's run (events.jsonl)"
    else fail "ok-cold: determinism runs differ from the gate's run"; fi
  else
    fail "ok-cold: determinism"; tail -12 "$out/determinism.log"
  fi
}

case $MODE in
  prepare) prepare "$(realpath "$1")"; exit 0 ;;
  update)
    out=$1
    for c in "${CASES[@]}"; do
      mkdir -p "$GOLD/$c"
      (cd "$out/$c-ref" && sha256sum nets.csv events.jsonl nets.vcd non-grant-records) > "$GOLD/$c/outputs.sha256"
      [[ ${c%%-*} == ok ]] && "$PY" "$ROOT/scripts/golden.py" write "$out/$c-ref/nets.csv" "$GOLD/$c/vled.csv" --net VLED
    done
    echo "dms-rail: wrote $GOLD"; exit 0 ;;
esac
BB=$(realpath "$1")
OUT=$2
rm -rf "$OUT" && mkdir -p "$OUT"
case $MODE in
  test) do_test "$BB" "$OUT" ;;
  gate) do_gate "$BB" "$OUT" ;;
  ref) prepare "$BB"; do_ref "$BB" "$OUT" ;;
  *) sed -n '2,30p' "$0"; exit 2 ;;
esac
if [[ $status == 0 ]]; then echo "dms-rail: $MODE PASS"; else echo "dms-rail: $MODE FAIL"; fi
exit $status
