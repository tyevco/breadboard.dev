#!/usr/bin/env bash
# Milestone B2 (Phase 2 plan section 4, track B; ADR-010 D5-D7, G3, G4, G6, Amendment 3): assertions,
# the `assert` v2 records, the analog monitors at solver points and the windows, against the Phase 1
# check scripts they port (check_dms_rail.py, check_dms_vsense.py, check_dms_home.py), which stay as
# their cross-check for one milestone (D10).
#
#   scripts/assert_gate.sh test <bb> <out>   make assert-test (in make test)
#   scripts/assert_gate.sh gate <bb> <out>   make assert-gate: test, then E2 and E3 and the partitions
#
# test:
#  1. E1 (examples/dial-me-spinner, asserts/e1.toml: LED-DATA-V4, LSW-3, LSW-2, FW-6.4 as three
#     assertions) on its four cases at bb's defaults, each a copy of the project (scripts/dms_rail.sh
#     prepare): the run's assert records against check_dms_rail.py on the same run (same verdicts;
#     the values as scripts/check_asserts.py states: exact where both read the same data); the
#     correct order passes every assertion, the wrong order fails exactly LSW-3; and the assertions
#     are passive: every committed one-cell hash of tests/golden/dms-rail (nets.csv, nets.vcd, the
#     non-grant records without the assert records) holds.
#  2. `bb assert` on each run gives every record that reads only records (edges, logs) again, byte
#     for byte; the solver-point kinds replay as not_evaluable.
#  3. The FW-6.4 negative controls of Phase 1 (dms_rail.sh's: ok-cold's pin_edge records moved, the
#     first P0.06 rise 5 ms after LED_PWR_EN, the second LED_PWR_EN rise 20 ms after its fall):
#     `bb assert` fails exactly FW-6.4-on, resp. FW-6.4-off, as check_dms_rail.py fails exactly
#     FW-6.4 on the same files.
#  4. A bench's own [[assert]] equals the same tables in an --assert file (examples/bench,
#     bench/stimulus.toml), byte for byte; and the assertions change no other output there.
#  5. The refusals (G6 and the file rules), each with its message, on E1's project
#     (--elaborate-only): a check on a net an ideal bench supply forces (a tautology), a branch
#     current, an analog check on an MCU pin, an unknown kind, a stimulus key in an assertion file.
# gate (make assert-gate; about 15 min fresh): test, then
#  6. E1's cases at --max-quantum 10us (the one-cell reference: a true equivalence, E1 is acyclic)
#     and 1ms, and ok-cold with --engine-order analog-first: the assert records and every non-grant
#     record identical to the defaults'; ok-cold 10 runs byte-identical.
#  7. E2's eight cases (asserts/e2/<case>.toml, written by scripts/dms_asserts.py) at bb's defaults:
#     check_dms_vsense.py on them, the assertions against it (the codes exact; the shed timing's
#     crossing in the script's sample period), the committed hashes of tests/golden/dms-vsense hold
#     without the assert records, `bb assert` gives the record kinds again, dms_asserts.py rewrites
#     the committed files byte for byte; s50 and sag as their every-lattice-point references
#     (--lookahead off --adc-push every: other partitions) give the same assert records.
#  8. E3's HOME_N latency (e3/asserts/<corner>.toml) on typ, worst and park, and on dms-home's
#     negative controls: neg-fb200 and neg-fb286 fail exactly HOME_N-tS (check_dms_home.py fails
#     exactly its HOME_N assert latency check on them), neg-fast passes it (the script fails it on
#     its step rate only, which is not ported); the values exact; the committed hashes hold without
#     the assert records. E3's partitions coincide (ADR-008 Amendment 12), so no partition gate here.
set -euo pipefail
MODE=$1
BB=$(realpath "$2")
OUT=$(realpath -m "$3")
ROOT=$(cd "$(dirname "$0")/.." && pwd)
PROJ=$ROOT/examples/dial-me-spinner
E3=$PROJ/e3
FWD=$PROJ/firmware/prebuilt
A=$PROJ/asserts
S=${BB_SCRATCH:-$HOME/.cache}
export DMS_RAIL_WORK=${DMS_RAIL_WORK:-$S/bb-assert-rail}
export DMS_VSENSE_WORK=${DMS_VSENSE_WORK:-$S/bb-assert-vsense}
export BENCH_WORK=${BENCH_WORK:-$S/bb-assert-bench}
PY=${PYTHON:-python3}
DET_RUNS=${DET_RUNS:-10}
DET_JOBS=${DET_JOBS:-5}
JOBS=${ASSERT_JOBS:-6}
E1_CASES=(ok-nominal ok-cold wrong-nominal wrong-cold)
E2_CASES=(s1 s1-none s50 s50-none c5 c5-none sag rec)
E3_CASES=(typ worst park neg-fast neg-fb200 neg-fb286)
status=0
pass() { echo "assert: PASS: $*"; }
fail() { echo "assert: FAIL: $*"; status=1; }

# --- running -----------------------------------------------------------------------------------
e1_args() { # <case>
  local fw=${1%%-*} bench=${1#*-}
  echo "--project $DMS_RAIL_WORK/proj-$fw --bench $DMS_RAIL_WORK/proj-$fw/bench/$bench.toml --for 120ms --assert $A/e1.toml"
}
e2_proj() {
  case $1 in s1 | sag | rec) echo proj-1s ;; s1-none) echo proj-1s-none ;; s50) echo proj-50ms ;;
    s50-none) echo proj-50ms-none ;; c5) echo proj-c5 ;; c5-none) echo proj-c5-none ;; esac
}
e2_args() { # <case>
  local p bench=vsense f
  p=$DMS_VSENSE_WORK/$(e2_proj "$1")
  case $1 in s1 | s1-none) f=2100ms ;; s50 | s50-none) f=650ms ;; c5 | c5-none) f=1100ms ;;
    sag) f=4300ms bench=sag ;; rec) f=2300ms bench=sag-recover ;; esac
  echo "--project $p --bench $p/bench/$bench.toml --for $f --probe net:VSYS_SENSE --sample-rate 10kHz --assert $A/e2/$1.toml"
}
E3_PINS="--probe net:VSYS --probe net:HOME_N --probe net:MOT_NSLEEP --probe net:U10.P0.26 --probe net:U10.P0.05 \
--probe net:U10.P0.07 --probe net:U10.P0.11 --probe net:U10.P1.15 --probe net:M1.HOME_N --sample-rate 10kHz"
e3_args() { # <case>: dms_home.sh's arguments, and the corner's assertions
  local n=$OUT/e3-neg
  case $1 in
    typ) echo "--netlist $E3/board.cir --firmware $FWD/homing.elf --for 2700ms --assert $E3/asserts/typ.toml" ;;
    worst) echo "--netlist $E3/board-ts75.cir --firmware $FWD/homing.elf --for 2800ms --assert $E3/asserts/worst.toml" ;;
    park) echo "--netlist $E3/board-park.cir --firmware $FWD/homing.elf --for 3200ms --assert $E3/asserts/park.toml" ;;
    neg-fast) echo "--netlist $E3/board.cir --firmware $FWD/homing_fast.elf --for 2700ms --assert $E3/asserts/typ.toml" ;;
    neg-fb200) echo "--netlist $n/fb200.cir --firmware $FWD/homing.elf --for 2700ms --assert $E3/asserts/typ.toml" ;;
    neg-fb286) echo "--netlist $n/fb286.cir --firmware $FWD/homing.elf --for 2800ms --assert $E3/asserts/typ.toml" ;;
  esac
}

# A job list "<dir>|<bb run args>", JOBS at a time; then each dir's non-grant records without the
# assert records (for the committed hashes) and its assert records.
run_jobs() {
  printf '%s\n' "$@" | xargs -P "$JOBS" -I{} bash -c '
    IFS="|" read -r dir args <<< "$1"
    # shellcheck disable=SC2086
    "$2" run $args --out "$dir" > "$dir.log" 2>&1 || { echo "assert: FAIL: $(basename "$dir"): bb run failed"; tail -6 "$dir.log"; }
  ' _ {} "$BB" || true
  local j d
  for j in "$@"; do
    d=${j%%|*}
    if [[ -f $d/events.jsonl ]] && grep -q "bb run: wall time" "$d.log"; then
      grep -v '"type":"grant"' "$d/events.jsonl" | grep -v '"type":"assert"' > "$d/non-grant-records" || true
      grep '"type":"assert"' "$d/events.jsonl" > "$d/assert-records" || true
      grep -h 'bb run: assertions:\|bb run: wall' "$d.log" | sed "s|^|assert:   $(basename "$d"): |"
    else
      fail "$(basename "$d"): no outputs"
    fi
  done
}

hashes() { # <dir> <golden dir>: nets.csv, nets.vcd and the non-grant records (asserts removed)
  local lines
  lines=$(grep -v ' events.jsonl$' "$2/outputs.sha256")
  if (cd "$1" && sha256sum --quiet -c <<< "$lines"); then
    pass "$(basename "$1"): the committed one-cell hashes ($(basename "$2")) hold with the assertions on (non-grant records without the assert records, nets.csv, nets.vcd): the assertions are passive"
  else
    fail "$(basename "$1"): differs from $2/outputs.sha256 (asserts removed)"
  fi
}

failed_set() { # <assert-records file>: the names of the failing assertions, sorted, comma-separated
  "$PY" -c 'import json,sys; print(",".join(sorted(json.loads(l)["name"] for l in open(sys.argv[1]) if json.loads(l)["outcome"]=="fail")))' "$1"
}

xcheck() { # <what> <check_asserts.py args...>
  local what=$1 log
  shift
  log=$(mktemp)
  if "$PY" "$ROOT/scripts/check_asserts.py" "$@" > "$log"; then
    pass "$what: $(grep -c '^check_asserts: ok' "$log") comparisons with the script hold"
  else
    fail "$what"; grep -v '^check_asserts: ok' "$log"
  fi
  grep -E '^check_asserts: ok   (LSW-3|LSW-2: settle|E2-shed-timing|HOME_N-latency|HOME_N-release): (value|settle)' "$log" \
    | sed 's/^check_asserts: ok  /assert:  /' || true
  rm -f "$log"
}

replayed() { # <dir> <assert files...>: bb assert gives the record kinds again
  local d=$1 x=()
  shift
  for f in "$@"; do x+=(--assert "$f"); done
  if "$BB" assert "$d" "${x[@]}" --out "$d/replay.jsonl" 2> "$d/replay.txt" \
      && "$PY" "$ROOT/scripts/check_asserts.py" replay "$d" "$d/replay.jsonl" > "$d/replay-check.txt"; then
    pass "$(basename "$d"): bb assert gives the run's records again ($(sed 's/^bb assert: //' "$d/replay.txt"))"
  else
    fail "$(basename "$d"): bb assert"; cat "$d/replay.txt"; grep -v ': ok' "$d/replay-check.txt" || true
  fi
}

same_asserts() { # <a> <b> <what>: identical assert records and non-grant records
  if cmp -s "$1/assert-records" "$2/assert-records" && cmp -s "$1/non-grant-records" "$2/non-grant-records" \
      && cmp -s "$1/nets.csv" "$2/nets.csv" && cmp -s "$1/nets.vcd" "$2/nets.vcd"; then
    pass "$3: the same $(grep -c . "$1/assert-records") assert records and every non-grant record, nets.csv, nets.vcd"
  else
    fail "$3: the verdicts or outputs differ"
    diff <(cut -c1-160 "$1/assert-records") <(cut -c1-160 "$2/assert-records") | head -6 || true
  fi
}

# --- test --------------------------------------------------------------------------------------
e1_runs() { # [<suffix> <extra args>]
  local sfx=${1:-} extra=${2:-} jobs=() c
  for c in "${E1_CASES[@]}"; do jobs+=("$OUT/e1-$c$sfx|$(e1_args "$c") $extra"); done
  run_jobs "${jobs[@]}"
}

do_test() {
  bash "$ROOT/scripts/dms_rail.sh" prepare "$BB" > /dev/null
  e1_runs
  local c d x
  for c in "${E1_CASES[@]}"; do
    d=$OUT/e1-$c
    [[ -f $d/assert-records ]] || continue
    x=()
    [[ $c == wrong-* ]] && x=(--expect-fail LSW-3)
    "$PY" "$ROOT/scripts/check_dms_rail.py" "$d" --project "$PROJ" --json "$d/checks.json" "${x[@]}" > "$d/checks.txt" \
      || fail "$c: check_dms_rail.py"
    xcheck "e1-$c: assertions against check_dms_rail.py" e1 "$d" "$d/checks.json"
    local want="" got
    [[ $c == wrong-* ]] && want=LSW-3
    got=$(failed_set "$d/assert-records")
    if [[ $got == "$want" && $(grep -c . "$d/assert-records") == 6 ]]; then
      pass "e1-$c: 6 assertions, failing exactly: ${got:-none} (the script: $(sed -n 's/^check_dms_rail: \(PASS\|FAIL\): \(expected\|LED-DATA-V4, \)/\2/p' "$d/checks.txt" | tail -1 | cut -c1-60))"
    else
      fail "e1-$c: failing assertions ${got:-none}, expected ${want:-none}"
    fi
    hashes "$d" "$ROOT/tests/golden/dms-rail/$c"
    replayed "$d" "$A/e1.toml"
  done
  fw_controls
  bench_embedded
  refusals
}

fw_controls() { # 3.
  local src=$OUT/e1-ok-cold k d got
  [[ -f $src/events.jsonl ]] || { fail "FW-6.4 controls: no ok-cold run"; return; }
  for k in on-wait off-wait; do
    d=$OUT/fw-rule-$k
    rm -rf "$d" && mkdir -p "$d"
    cp "$src/nets.csv" "$src/run.json" "$d/"
    "$PY" - "$src/events.jsonl" "$d/events.jsonl" "$k" <<'EOF'
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
        if r["type"] != "assert":
            f.write(json.dumps(r, separators=(",", ":")) + "\n")
EOF
    local want=FW-6.4-on
    [[ $k == off-wait ]] && want=FW-6.4-off
    if "$BB" assert "$d" --assert "$A/e1.toml" --out "$d/replay.jsonl" 2> "$d/replay.txt"; then
      got=$(failed_set "$d/replay.jsonl")
      "$PY" "$ROOT/scripts/check_dms_rail.py" "$d" --project "$PROJ" --json "$d/checks.json" > "$d/checks.txt" || true
      local sf
      sf=$("$PY" -c 'import json,sys; print(",".join(json.load(open(sys.argv[1]))["failed"]))' "$d/checks.json")
      if [[ $got == "$want" && $sf == FW-6.4 ]]; then
        pass "FW-6.4 negative control ($k): bb assert fails exactly $got ($(sed 's/^bb assert: //' "$d/replay.txt")); check_dms_rail.py fails exactly $sf"
      else
        fail "FW-6.4 negative control ($k): bb assert fails ${got:-none} (want $want), the script ${sf:-none}"
      fi
    else
      fail "FW-6.4 negative control ($k): bb assert"; cat "$d/replay.txt"
    fi
  done
}

bench_embedded() { # 4.
  bash "$ROOT/scripts/bench_gate.sh" prepare "$BB" > /dev/null
  local p=$BENCH_WORK/p elf=$ROOT/examples/feedback/firmware/build/feedback.elf af=$OUT/bench-asserts.toml
  cat > "$af" <<'EOF'
schema = "bb-bench/1"

[[assert]]
name = "AFILT-range"
kind = "range"
net = "AFILT"
min = "-0.1V"
max = "3.4V"

[[assert]]
name = "PA5-rises"
kind = "count"
net = "U1.PA5"
level = 1
min = 1
EOF
  { cat "$p/bench/stimulus.toml"; echo; sed -n '/^\[\[assert\]\]/,$p' "$af"; } > "$p/bench/stimulus-asserts.toml"
  local args="--project $p --for 300ms"
  # shellcheck disable=SC2086
  if "$BB" run $args --bench "$p/bench/stimulus.toml" --out "$OUT/bench-plain" > "$OUT/bench-plain.log" 2>&1 \
      && "$BB" run $args --bench "$p/bench/stimulus.toml" --assert "$af" --out "$OUT/bench-file" > "$OUT/bench-file.log" 2>&1 \
      && "$BB" run $args --bench "$p/bench/stimulus-asserts.toml" --out "$OUT/bench-own" > "$OUT/bench-own.log" 2>&1; then
    local n
    n=$(grep -c '"type":"assert"' "$OUT/bench-own/events.jsonl" || true)
    if [[ $n == 2 ]] && cmp -s "$OUT/bench-own/events.jsonl" "$OUT/bench-file/events.jsonl" \
        && cmp -s "$OUT/bench-own/nets.csv" "$OUT/bench-file/nets.csv" \
        && cmp -s <(grep -v '"type":"assert"' "$OUT/bench-file/events.jsonl") "$OUT/bench-plain/events.jsonl" \
        && cmp -s "$OUT/bench-file/nets.csv" "$OUT/bench-plain/nets.csv" && cmp -s "$OUT/bench-file/nets.vcd" "$OUT/bench-plain/nets.vcd"; then
      pass "examples/bench stimulus: a bench's own [[assert]] equals the same tables in an --assert file (events.jsonl byte for byte, $n assert records: $(grep -o '"name":"[^"]*","outcome":"[a-z_]*"' "$OUT/bench-own/events.jsonl" | tr '\n' ' ')); without them every other output is the same"
    else
      fail "examples/bench stimulus: the embedded and the file assertions differ, or they change another output ($n records)"
    fi
  else
    fail "examples/bench stimulus: a run failed"; tail -4 "$OUT"/bench-*.log
  fi
}

refusals() { # 5.
  local p=$DMS_RAIL_WORK/proj-ok k f
  local -A body=(
    [tautology]='name = "T"\nkind = "range"\nnet = "VSYS"\nmin = "2V"'
    [current]='name = "I"\nkind = "range"\nnet = "i(V_bench_VSYS)"\nmax = "1V"'
    [pin]='name = "P"\nkind = "range"\nnet = "U10.P1.04"\nmax = "1V"'
    [kind]='name = "K"\nkind = "maximum"\nnet = "VLED"\nmax = "1V"'
  )
  local -A want=(
    [tautology]="a tautology, refused: ADR-010 D6"
    [current]="an assertion on a current through a 0 V source is refused (ADR-010 D6)"
    [pin]="is a pin (a digital level); an analog check reads a net"
    [kind]="the closed set of ADR-010 D5"
    [stimulus]="an assertion file holds [[assert]] tables only"
  )
  # The nominal bench's VSYS is an ideal supply (rs = 0)? Use a copy with an ideal VSYS for the
  # tautology: the cold bench's VSYS has rs, so the rule must name an ideal one.
  sed 's/^rs = "0.5692"$/rs = "0"/' "$p/bench/cold.toml" > "$p/bench/cold-ideal.toml"
  for k in tautology current pin kind stimulus; do
    f=$OUT/refuse-$k.toml
    if [[ $k == stimulus ]]; then
      printf 'schema = "bb-bench/1"\n\n[[supply]]\nnet = "VSYS"\nvolts = "3V"\n' > "$f"
    else
      printf 'schema = "bb-bench/1"\n\n[[assert]]\n%b\n' "${body[$k]}" > "$f"
    fi
    if "$BB" run --project "$p" --bench "$p/bench/cold-ideal.toml" --for 10ms --elaborate-only --assert "$f" \
        --out "$OUT/refuse-$k" > "$OUT/refuse-$k.log" 2>&1; then
      fail "refusal $k: elaborated"
    elif grep -qF "${want[$k]}" "$OUT/refuse-$k.log"; then
      pass "refusal $k: $(grep -o 'assertion [^:]*: .\{0,90\}\|[^:]*: an assertion file.\{0,40\}\|kind "maximum".\{0,40\}' "$OUT/refuse-$k.log" | head -1)..."
    else
      fail "refusal $k: message"; tail -3 "$OUT/refuse-$k.log"
    fi
  done
  rm -f "$p/bench/cold-ideal.toml"
}

# --- gate --------------------------------------------------------------------------------------
do_gate() {
  do_test
  # 6. E1 partitions and engine order.
  e1_runs -ref "--max-quantum 10us"
  e1_runs -q1ms "--max-quantum 1ms"
  run_jobs "$OUT/e1-ok-cold-order|$(e1_args ok-cold) --engine-order analog-first"
  local c
  for c in "${E1_CASES[@]}"; do
    same_asserts "$OUT/e1-$c" "$OUT/e1-$c-ref" "e1-$c: the one-cell reference (--max-quantum 10us)"
    same_asserts "$OUT/e1-$c" "$OUT/e1-$c-q1ms" "e1-$c: --max-quantum 1ms"
  done
  if cmp -s "$OUT/e1-ok-cold/events.jsonl" "$OUT/e1-ok-cold-order/events.jsonl"; then
    pass "e1-ok-cold: --engine-order analog-first byte-identical (events.jsonl, assert records included)"
  else fail "e1-ok-cold: --engine-order analog-first differs"; fi
  # shellcheck disable=SC2046
  if bash "$ROOT/scripts/determinism.sh" "$BB" "$OUT/e1-determinism" "$DET_RUNS" "$DET_JOBS" -- \
      $(e1_args ok-cold) > "$OUT/e1-determinism.log" 2>&1 \
      && cmp -s "$OUT/e1-determinism/run-01/events.jsonl" "$OUT/e1-ok-cold/events.jsonl"; then
    pass "e1-ok-cold: $DET_RUNS runs byte-identical, assert records included"
  else
    fail "e1-ok-cold: determinism"; tail -5 "$OUT/e1-determinism.log"
  fi
  # 7. E2.
  bash "$ROOT/scripts/dms_vsense.sh" prepare "$BB" > /dev/null
  local jobs=()
  for c in "${E2_CASES[@]}"; do jobs+=("$OUT/e2/$c|$(e2_args "$c")"); done
  jobs+=("$OUT/e2/s50-ref|$(e2_args s50) --lookahead off --adc-push every")
  jobs+=("$OUT/e2/sag-ref|$(e2_args sag) --lookahead off --adc-push every")
  mkdir -p "$OUT/e2"
  run_jobs "${jobs[@]}"
  if "$PY" "$ROOT/scripts/check_dms_vsense.py" "$OUT/e2" --suite "$PROJ/suite/results.json" --json "$OUT/e2/checks.json" \
      > "$OUT/e2/checks.txt"; then pass "e2: check_dms_vsense.py on the eight cases"
  else fail "e2: check_dms_vsense.py"; tail -5 "$OUT/e2/checks.txt"; fi
  for c in "${E2_CASES[@]}"; do
    [[ -f $OUT/e2/$c/assert-records ]] || continue
    xcheck "e2-$c: assertions against check_dms_vsense.py" e2 "$OUT/e2/$c" "$OUT/e2/checks.json" --case "$c"
    hashes "$OUT/e2/$c" "$ROOT/tests/golden/dms-vsense/$c"
    replayed "$OUT/e2/$c" "$A/e2/$c.toml"
  done
  same_asserts "$OUT/e2/s50" "$OUT/e2/s50-ref" "e2-s50: the every-lattice-point reference (--lookahead off --adc-push every)"
  same_asserts "$OUT/e2/sag" "$OUT/e2/sag-ref" "e2-sag: the every-lattice-point reference (--lookahead off --adc-push every)"
  "$PY" "$ROOT/scripts/dms_asserts.py" "$OUT/e2/checks.json" "$OUT/e2/asserts" > /dev/null
  if diff -r "$OUT/e2/asserts" "$A/e2" > /dev/null; then
    pass "e2: scripts/dms_asserts.py rewrites asserts/e2 byte for byte from this gate's runs"
  else fail "e2: asserts/e2 differs from dms_asserts.py's output"; diff -r "$OUT/e2/asserts" "$A/e2" | head -8; fi
  # 8. E3.
  mkdir -p "$OUT/e3-neg"
  cp "$E3/bb.lock" "$OUT/e3-neg/bb.lock"
  local m1="*bb part M1 package=dms/detent-mechanism s0=500" ts
  for ts in 200 286; do
    sed "s|^\\${m1} ts=50ms\$|${m1} ts=${ts}ms|" "$E3/board.cir" > "$OUT/e3-neg/fb$ts.cir"
    grep -qxF "${m1} ts=${ts}ms" "$OUT/e3-neg/fb$ts.cir" || fail "neg-fb$ts: board.cir's M1 line"
  done
  jobs=()
  for c in "${E3_CASES[@]}"; do jobs+=("$OUT/e3/$c|$(e3_args "$c") --target U10 $E3_PINS"); done
  mkdir -p "$OUT/e3"
  run_jobs "${jobs[@]}"
  local corner start
  for c in "${E3_CASES[@]}"; do
    local d=$OUT/e3/$c
    [[ -f $d/assert-records ]] || continue
    corner=typ start=click
    [[ $c == worst ]] && corner=worst
    [[ $c == park ]] && start=park
    "$PY" "$ROOT/scripts/check_dms_home.py" "$d" --corner "$corner" --start "$start" > "$d/check.txt" || true
    local x=()
    [[ $c == park ]] && x=(--park)
    xcheck "e3-$c: assertions against check_dms_home.py" e3 "$d" "$d/check.txt" "${x[@]}"
    local af=$corner
    [[ $c == park ]] && af=park
    replayed "$d" "$E3/asserts/$af.toml"
    local got want="" sf
    got=$(failed_set "$d/assert-records")
    sf=$(grep -c '^check_dms_home: FAIL HOME_N assert latency' "$d/check.txt" || true)
    # neg-fb200 (tS 200 ms): its seeded phase gives the FA run's HOME_N instant, so only the device's
    # tS shows it; neg-fb286 (tS 286 ms): its latency too (108.3 ms). Either way the script's one
    # HOME_N assert latency check fails, and every failing assertion is one of the three it became.
    case $c in neg-fb200) want=HOME_N-tS ;; neg-fb286) want=HOME_N-latency,HOME_N-tS ;; esac
    if [[ $got == "$want" ]]; then
      pass "e3-$c: failing exactly: ${got:-none} (check_dms_home.py's HOME_N assert latency check: $([[ $sf == 1 ]] && echo FAIL || echo ok))"
    else
      fail "e3-$c: failing ${got:-none}, expected ${want:-none}"
    fi
    case $c in typ | worst | park) hashes "$d" "$ROOT/tests/golden/dms-home/$c" ;; esac
  done
}

rm -rf "$OUT" && mkdir -p "$OUT"
case $MODE in
  test) do_test ;;
  gate) do_gate ;;
  *) sed -n '2,/^set -euo/{/^set -euo/!p}' "$0"; exit 2 ;;
esac
if [[ $status == 0 ]]; then echo "assert: $MODE PASS"; else echo "assert: $MODE FAIL"; fi
exit $status
