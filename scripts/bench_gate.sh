#!/usr/bin/env bash
# Milestone B1 (Phase 2 plan section 4, track B; ADR-010 D1-D4 and gates G1, G2, G8): bb-bench/1
# and the bench engine, on examples/bench (a synthetic STM32F103 board running examples/feedback's
# firmware) and on the Phase 1 benches of examples/dial-me-spinner.
#
#   scripts/bench_gate.sh test <bb> <out>     make bench-test (in make test)
#   scripts/bench_gate.sh gate <bb> <out>     make bench-gate: test, then the partition gates
#   scripts/bench_gate.sh perf <bb> <out>     make bench-perf: the playback cost gate (idle host,
#                                             under the measurement lock)
#   scripts/bench_gate.sh update <bb> <out>   rewrite tests/golden/bench from one-cell references
#   scripts/bench_gate.sh prepare <bb>        only the project copy (pool-equiv)
#
# The cases (examples/bench/bench/*.toml):
#   stimulus  300 ms: a current profile, an analog playback, presses (one with bounce), a set and
#             two faults, all tables of the analog engine (no bench engine); lookahead on
#   trigger   70 ms: a press and a set triggered by U1.PA5 (the bench engine drives a switch
#             control and a supply, Drive::Timed); one-cell grants (see the bench)
#   set       100 ms: set actions on U1.PB0, one triggered (the bench engine drives an MCU pin)
#   dpwl      100 ms: an ideal [[pwl]] on BTN, which U1.PB0 reads through digital_in
#   dplay     100 ms: the same edges as a digital playback into U1.PB0 (data/btn.csv)
#   psmall    300 ms: data/ain.csv as a [[playback]] on AIN; ppwl: the same points as a [[pwl]]
#
# test:
#  1. examples/bench is current (bench.net, the data files, board.bbm and bb.lock).
#  2. Each case at bb's defaults: scripts/check_bench.py (the switch view's analytic RC settling
#     and open-state divider, G8; the profile, the playback, the faults, the bounce, the sets; the
#     digital playback's edges equal the [[pwl]]'s to the fs and the MCU applies them at ceil_L
#     with the same response; the triggers' times), and the committed one-cell reference hashes
#     (tests/golden/bench/<case>.sha256: nets.csv, nets.vcd, the non-grant records).
#  3. G2: the 601-point playback and the same points as an inline [[pwl]] give identical nets.csv,
#     nets.vcd and non-grant records.
#  4. The refusals (negative controls; each must fail with its message): a press on an instance
#     without a switch view, a [[pwl]] above 10 000 points, a bb-bench/1 key in a /0 file, a later
#     milestone's key, a playback on a supply's net, a trigger on an MCU input, a set on a net
#     without a supply, overlapping timed actions.
#  5. G1: each Phase 1 bench of examples/dial-me-spinner, as committed and with its schema line
#     rewritten to the other major, elaborates to a byte-identical deck.cir (`bb run
#     --elaborate-only`), and `bb pkg migrate --bench` rewrites a /0 copy to the /1 file.
# gate: test, then every case at --max-quantum 10us, 1ms and 10ms and with --engine-order
#   analog-first (byte-identical in nets.csv, nets.vcd and the non-grant records; analog-first
#   in events.jsonl too), and stimulus and trigger twice more (determinism).
# perf: playback-long (1 s, 100 001 points every 10 us) against playback-base (AIN on a constant
#   supply), interleaved, N = BENCH_PERF_N (default 5): the median added wall time <= 2.0 s.
set -euo pipefail
MODE=$1
BB=$(realpath "$2")
OUT=${3:-out/bench}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
WORK=${BENCH_WORK:-${BB_SCRATCH:-$HOME/.cache}/bb-bench}
P=$WORK/p
ELF=$ROOT/examples/feedback/firmware/build/feedback.elf
GOLD=$ROOT/tests/golden/bench
PY=${PYTHON:-python3}
CHECK="$PY $ROOT/scripts/check_bench.py"
status=0
pass() { echo "bench: PASS: $*"; }
fail() { echo "bench: FAIL: $*"; status=1; }

declare -A BENCH=([stimulus]=stimulus [trigger]=trigger [set]=digital-set [dpwl]=digital-pwl
                  [dplay]=digital-playback [psmall]=playback-small [ppwl]=playback-pwl)
declare -A FOR=([stimulus]=300ms [trigger]=70ms [set]=100ms [dpwl]=100ms [dplay]=100ms
                [psmall]=300ms [ppwl]=300ms)
CASES=(stimulus trigger set dpwl dplay psmall ppwl)

prepare() {
  $PY "$ROOT/examples/bench/make_netlist.py" --check > /dev/null
  $PY "$ROOT/examples/bench/make_data.py" --check
  "$BB" open --check "$ROOT/examples/bench" > /dev/null
  rm -rf "$WORK"
  mkdir -p "$WORK"
  cp -r "$ROOT/examples/bench" "$P"
  rm -rf "$P/.bb"
  cp "$ELF" "$P/feedback.elf"
  "$BB" flash --project "$P" --target U1 "$P/feedback.elf" > /dev/null
}

run_case() { # <dir> <case> [extra bb run args]
  local dir=$1 c=$2
  shift 2
  rm -rf "$dir"
  if ! "$BB" run --project "$P" --bench "$P/bench/${BENCH[$c]}.toml" --for "${FOR[$c]}" \
      --sample-rate 10kHz "$@" --out "$dir" > "$dir.log" 2>&1; then
    fail "$c: bb run failed (${dir}.log):"
    tail -3 "$dir.log" | sed 's/^/bench:   /'
    return 1
  fi
}

nongrant() { grep -v '"type":"grant"' "$1/events.jsonl" > "$1/non-grant-records"; }
hashes_of() { (cd "$1" && nongrant . && sha256sum nets.csv nets.vcd non-grant-records); }

golden() { # <dir> <case>
  if [[ ! -f $GOLD/$2.sha256 ]]; then
    fail "$2: no committed reference hashes ($GOLD/$2.sha256; make bench-update)"
  elif diff <(hashes_of "$1") "$GOLD/$2.sha256" > /dev/null; then
    pass "$2: nets.csv, nets.vcd and the non-grant records equal the committed one-cell reference"
  else
    fail "$2: differs from the committed one-cell reference $GOLD/$2.sha256"
  fi
}

check() { # <check_bench.py args...>
  if $CHECK "$@" | sed 's/^/bench:   /'; [[ ${PIPESTATUS[0]} == 0 ]]; then
    pass "check_bench.py $1"
  else
    fail "check_bench.py $1"
  fi
}

same() { # <a> <b> <what> [all]
  if $CHECK same "$1" "$2" "${4:-nongrant}" > /dev/null; then pass "$3"; else fail "$3"; fi
}

refuse() { # <name> <expected message part> ; the bench text on stdin
  local f=$P/bench/refuse-$1.toml
  cat > "$f"
  if "$BB" run --project "$P" --bench "$f" --for 1ms --elaborate-only --out "$WORK/refuse-$1" \
      > "$WORK/refuse-$1.log" 2>&1; then
    fail "refusal $1: bb run accepted the bench"
  elif grep -qF -- "$2" "$WORK/refuse-$1.log"; then
    pass "refusal $1: \"$2\""
  else
    fail "refusal $1: refused, but without \"$2\": $(tail -1 "$WORK/refuse-$1.log")"
  fi
}

SUPPLIES='[[supply]]
net = "+3V3"
volts = "3.3V"
rs = "1"

[[supply]]
net = "VSW"
volts = "3.3V"
rs = "10m"

[[supply]]
net = "AIN"
volts = "0V"
'

refusals() {
  refuse press-resistor "R1 has no switch view in the analog deck (its views: ideal (circuit)" <<EOF
schema = "bb-bench/1"
$SUPPLIES
[[action]]
press = "R1"
at = "1ms"
EOF
  { echo 'schema = "bb-bench/1"'; echo "$SUPPLIES"; echo '[[pwl]]'; echo 'net = "BTN"';
    echo -n 'points = ['
    for ((k = 0; k <= 10000; k++)); do echo -n "[\"${k}us\", \"1V\"], "; done; echo ']'; } \
    | refuse pwl-long "[[pwl]] takes at most 10000"
  refuse v0-action "action: unknown key" <<EOF
schema = "bb-bench/0"
[[supply]]
net = "+3V3"
volts = "3.3V"
[[action]]
press = "SW1"
at = "1ms"
EOF
  # A key a later milestone implements ([[battery]] is P1's since ADR-009 Amendment 4).
  refuse later-key "activity: a bb-bench/1 key for bench-scheduled internal states" <<EOF
schema = "bb-bench/1"
$SUPPLIES
[[activity]]
state = "U1#radio_tx"
EOF
  refuse playback-on-supply "another bench source drives it" <<EOF
schema = "bb-bench/1"
$SUPPLIES
[[playback]]
net = "VSW"
file = "../data/ain.csv"
EOF
  refuse trigger-on-input "a trigger edge is an MCU pin bound as digital_out" <<EOF
schema = "bb-bench/1"
$SUPPLIES
[[action]]
press = "SW1"
after = { edge = "U1.PB0", level = 1 }
EOF
  refuse set-no-supply "has no [[supply]] (set drives a bench source)" <<EOF
schema = "bb-bench/1"
$SUPPLIES
[[action]]
set = "BTN"
to = "1V"
at = "1ms"
EOF
  refuse overlap "starts at or before" <<EOF
schema = "bb-bench/1"
$SUPPLIES
[[action]]
press = "SW1"
at = "1ms"
for = "2ms"
[[action]]
press = "SW1"
at = "2ms"
EOF
}

migration() { # G1 on the Phase 1 benches of examples/dial-me-spinner
  local d=$WORK/dms elf=led_rail.elf src=$ROOT/examples/dial-me-spinner
  rm -rf "$d"
  mkdir -p "$d/kicad/main" "$d/firmware/prebuilt"
  cp "$src/bb.toml" "$src/board.bbm" "$src/bb.lock" "$d/"
  cp "$src/kicad/main/dial-me-spinner-main.net" "$d/kicad/main/"
  cp -r "$src/bench" "$d/bench"
  cp "$src/firmware/prebuilt/$elf" "$d/firmware/prebuilt/"
  "$BB" flash --project "$d" --target U10 "$d/firmware/prebuilt/$elf" > /dev/null
  local n=0
  for b in "$d"/bench/*.toml; do
    local name cur other
    name=$(basename "$b" .toml)
    cur=$(sed -n 's/^schema = "bb-bench\/\([01]\)".*/\1/p' "$b")
    other=$((1 - cur))
    sed "s/^schema = \"bb-bench\/$cur\"/schema = \"bb-bench\/$other\"/" "$b" > "$d/bench/$name.other.toml"
    for v in cur other; do
      local f=$b
      [[ $v == other ]] && f=$d/bench/$name.other.toml
      if ! "$BB" run --project "$d" --bench "$f" --for 10us --elaborate-only --out "$WORK/g1/$name-$v" \
          > "$WORK/g1-$name-$v.log" 2>&1; then
        fail "G1 $name (bb-bench/${!v}): elaboration failed: $(tail -1 "$WORK/g1-$name-$v.log")"
        continue 2
      fi
    done
    if cmp -s "$WORK/g1/$name-cur/deck.cir" "$WORK/g1/$name-other/deck.cir"; then
      n=$((n + 1))
    else
      fail "G1 $name: bb-bench/0 and bb-bench/1 build different decks"
    fi
    # bb pkg migrate --bench: a /0 copy becomes the /1 file.
    local zero=$d/bench/$name.zero.toml
    if [[ $cur == 0 ]]; then cp "$b" "$zero"; else cp "$d/bench/$name.other.toml" "$zero"; fi
    "$BB" pkg migrate --bench "$zero" > /dev/null
    if ! grep -q '^schema = "bb-bench/1"$' "$zero"; then
      fail "G1 $name: bb pkg migrate --bench did not write bb-bench/1"
    fi
    if [[ $cur == 1 ]] && ! cmp -s "$zero" "$b"; then
      fail "G1 $name: bb pkg migrate --bench of its /0 form differs from the committed /1 file"
    fi
  done
  [[ $n -gt 0 ]] && pass "G1: $n Phase 1 benches build byte-identical decks as bb-bench/0 and bb-bench/1; bb pkg migrate --bench writes the committed files"
}

do_test() {
  mkdir -p "$OUT"
  prepare
  pass "examples/bench current (bench.net, data, board.bbm, bb.lock)"
  for c in "${CASES[@]}"; do
    run_case "$OUT/$c" "$c" || continue
    grep -E 'simulated' "$OUT/$c.log" | sed "s/^bb run:/bench:   $c:/"
  done
  check stimulus "$OUT/stimulus" "$P/bench"
  check trigger "$OUT/trigger"
  check set "$OUT/set"
  check digital "$OUT/dpwl" "$OUT/dplay" "$P/data/btn.csv"
  for c in "${CASES[@]}"; do golden "$OUT/$c" "$c"; done
  same "$OUT/psmall" "$OUT/ppwl" "G2: the 601-point playback equals the same points as an inline [[pwl]] (nets.csv, nets.vcd, non-grant records)"
  refusals
  migration
}

do_gate() {
  do_test
  for c in "${CASES[@]}"; do
    for q in 10us 1ms 10ms; do
      run_case "$OUT/$c-$q" "$c" --max-quantum "$q" || continue
      same "$OUT/$c" "$OUT/$c-$q" "$c: --max-quantum $q equals the default run (partition independence)"
    done
    run_case "$OUT/$c-order" "$c" --engine-order analog-first || continue
    same "$OUT/$c" "$OUT/$c-order" "$c: --engine-order analog-first byte-identical" all
  done
  for c in stimulus trigger; do
    for k in 2 3; do
      run_case "$OUT/$c-det$k" "$c" || continue
      same "$OUT/$c" "$OUT/$c-det$k" "$c: run $k byte-identical (determinism)" all
    done
  done
}

do_update() {
  mkdir -p "$OUT" "$GOLD"
  prepare
  for c in "${CASES[@]}"; do
    # The one-cell reference: --lookahead off on a feedback board; --max-quantum 10us otherwise.
    run_case "$OUT/$c-ref" "$c" --lookahead off --max-quantum 10us
    hashes_of "$OUT/$c-ref" > "$GOLD/$c.sha256"
    echo "bench: wrote $GOLD/$c.sha256"
  done
}

do_perf() {
  mkdir -p "$OUT"
  prepare
  $PY "$ROOT/examples/bench/make_data.py" --long "$P/bench" > /dev/null
  local n=${BENCH_PERF_N:-5} k c
  printf 'run\tcase\ttotal_s\tsim_s\tload1\n' > "$OUT/results.tsv"
  for ((k = 1; k <= n; k++)); do
    for c in base long; do
      local dir=$OUT/$c-$k load
      load=$(cut -d' ' -f1 /proc/loadavg)
      rm -rf "$dir"
      "$BB" run --project "$P" --bench "$P/bench/playback-$c.toml" --for 1s --out "$dir" > "$dir.log" 2>&1
      local tot sim
      tot=$(sed -n 's/.*wall time \([0-9.]*\) s total.*/\1/p' "$dir.log")
      sim=$(sed -n 's/.*startup, \([0-9.]*\) s simulation.*/\1/p' "$dir.log")
      printf '%s\t%s\t%s\t%s\t%s\n' "$k" "$c" "$tot" "$sim" "$load" >> "$OUT/results.tsv"
      echo "bench-perf: $c $k: $tot s total, $sim s simulation (load $load)"
    done
  done
  $PY - "$OUT/results.tsv" <<'EOF' || status=1
import csv, statistics, sys
rows = list(csv.DictReader(open(sys.argv[1]), delimiter="\t"))
med = lambda c, k: statistics.median(float(r[k]) for r in rows if r["case"] == c)
add, add_sim = med("long", "total_s") - med("base", "total_s"), med("long", "sim_s") - med("base", "sim_s")
print(f"bench-perf: medians: base {med('base', 'total_s'):.2f} s, playback {med('long', 'total_s'):.2f} s total; "
      f"added {add:.2f} s total, {add_sim:.2f} s simulation (limit 2.0 s)")
sys.exit(0 if add <= 2.0 else 1)
EOF
  if [[ $status == 0 ]]; then pass "playback of 1 s at 100 ksps adds <= 2 s of wall time"; else fail "playback cost"; fi
}

case $MODE in
  prepare) prepare; echo "bench: prepared $P"; exit 0 ;;
  test) do_test ;;
  gate) do_gate ;;
  update) do_update ;;
  perf) do_perf ;;
  *) echo "usage: $0 test|gate|perf|update <bb> <out>" >&2; exit 2 ;;
esac
if [[ $status == 0 ]]; then echo "bench: $MODE: PASS"; else echo "bench: $MODE: FAIL"; fi
exit $status
