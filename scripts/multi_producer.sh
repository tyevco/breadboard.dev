#!/usr/bin/env bash
# Phase 2 H5, ADR-014 gate G1 (make multi-producer, in make test): quiet grants with several producers
# on the small synthetic board tests/multi-producer/board.cir (an nRF52840 running homing.elf, the
# device dms/mechanism@2 at tS 5 ms on digital-only nets, ngspice behind sampled adc_in inputs), each
# run with a fresh Renode (--renode-pool off: the board is not in pool-equiv), 100 ms:
#   def    bb's defaults (lookahead on): quiet grants with Q = {devices} and no early stop; at most
#          MP_MAX_GRANTS (500) grants against the reference's 10 000, no promise broken
#   ref    the one-cell reference (--lookahead off): 10 000 grants
#   def must equal ref except grant records (a true equivalence: the partitions differ), and
#   --engine-order analog-first must equal def byte for byte
#   scope  negative control 2 (the scope rule): the board with MOT_NSLEEP glued into ngspice (its
#          `*bb digital` line removed): while U1 is awake, U1 -> ngspice -> U1 is a cycle that M1's
#          promise cannot cut, so no quiet grant has Q = {devices} (bb's "quiet sets" line): M1's
#          promise lifts the cap only together with a promise of U1 (asleep, its edges into ngspice
#          are then cut too: Q = {U1} or {U1, devices}, short grants). At least 90 % of the reference's
#          grant count, against def's few dozen. Its non-grant records equal ref's (the glue loads only
#          U1's output)
# Then the kernel part (bbdev/tests/multi_producer.rs): the same graph with mock engines, and negative
# control 1: a device that declares promise-independent inputs but reacts to one before its promise
# fails with PromiseBroken; set_pin in on_pin_change (rule 1) and in an internal timer are refused.
#
#   scripts/multi_producer.sh <bb> <out>
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
BB=$(realpath "$1")
OUT=$(realpath -m "$2")
MAX_GRANTS=${MP_MAX_GRANTS:-500}
rm -rf "$OUT" && mkdir -p "$OUT"
BOARD=$ROOT/tests/multi-producer/board.cir
FW=$ROOT/examples/dial-me-spinner/firmware/prebuilt/homing.elf
ARGS="--firmware $FW --target U1 --for 100ms --renode-pool off --sample-rate 100kHz \
--probe net:VSYS_SENSE --probe net:HOME_N --probe net:U1.P0.26 --probe net:U1.P1.15 --probe net:M1.HOME_N"
status=0
pass() { echo "multi-producer: PASS: $*"; }
fail() { echo "multi-producer: FAIL: $*"; status=1; }

# The scope control's board: the same file without MOT_NSLEEP's directive (beside a copy of the lock).
mkdir -p "$OUT/scope-board"
cp "$ROOT/tests/multi-producer/bb.lock" "$OUT/scope-board/bb.lock"
grep -vx '\*bb digital MOT_NSLEEP' "$BOARD" > "$OUT/scope-board/board.cir"
[[ $(diff "$BOARD" "$OUT/scope-board/board.cir" | grep -c '^<') == 1 ]] \
  || fail "the scope control's board is not board.cir less one *bb digital line"

run() { # <name> <board> <extra args...>
  local n=$1 b=$2
  shift 2
  # shellcheck disable=SC2086
  if "$BB" run --netlist "$b" $ARGS "$@" --out "$OUT/$n" > "$OUT/$n.log" 2>&1; then
    grep -v '"type":"grant"' "$OUT/$n/events.jsonl" > "$OUT/$n/non-grant-records"
  else
    echo "multi-producer: $n: bb run failed"; tail -8 "$OUT/$n.log"; return 1
  fi
}
run def "$BOARD" & p1=$!
run ref "$BOARD" --lookahead off & p2=$!
run order "$BOARD" --engine-order analog-first & p3=$!
run scope "$OUT/scope-board/board.cir" & p4=$!
for p in $p1 $p2 $p3 $p4; do wait "$p" || status=1; done
[[ $status == 0 ]] || { echo "multi-producer: FAIL"; exit 1; }

grants() { grep -c '"type":"grant"' "$OUT/$1/events.jsonl"; }
same() { # <a> <b> <what> <files...>
  local a=$1 b=$2 what=$3 f ok=1
  shift 3
  for f in "$@"; do cmp -s "$OUT/$a/$f" "$OUT/$b/$f" || { ok=0; fail "$what: $f differs"; }; done
  [[ $ok == 1 ]] && pass "$what"
  return 0
}
gd=$(grants def) gr=$(grants ref) gs=$(grants scope)
same def ref "def equals the one-cell reference except grant records ($gd grants against $gr)" \
  nets.csv nets.vcd non-grant-records
same def order "--engine-order analog-first byte-identical" nets.csv nets.vcd events.jsonl
[[ $gr == 10000 ]] && pass "ref: 10000 one-cell grants" || fail "ref: $gr grants, not 10000"
if (( gd <= MAX_GRANTS )); then pass "def: $gd grants <= $MAX_GRANTS"; else fail "def: $gd grants > $MAX_GRANTS"; fi
la=$(grep -h 'bb run: lookahead: [0-9]' "$OUT/def.log" || true)
sets=$(sed -n 's/^bb run: lookahead: quiet sets: //p' "$OUT/def.log")
echo "multi-producer:   def: ${la#bb run: }; quiet sets: ${sets:-none}"
if [[ $la =~ ([0-9]+)\ quiet\ grants,\ 0\ promises\ broken ]] && (( BASH_REMATCH[1] > 0 )); then
  pass "def: ${BASH_REMATCH[1]} quiet grants, no promise broken"
else
  fail "def: no quiet grant, or a promise was broken"
fi
if [[ $sets =~ \{devices\}\ ([0-9]+)\ \(0\ with\ an\ early\ stop\) ]] && (( BASH_REMATCH[1] > 0 )); then
  pass "def: ${BASH_REMATCH[1]} quiet grants with Q = {devices} and no early stop (P empty, ADR-014 D1)"
else
  fail "def: no quiet grant with Q = {devices} and P empty (quiet sets: ${sets:-none})"
fi
# Negative control 2: the scope rule.
ss=$(sed -n 's/^bb run: lookahead: quiet sets: //p' "$OUT/scope.log")
echo "multi-producer:   scope: $gs grants; quiet sets: ${ss:-none}"
if [[ $ss == *"{devices} "* ]]; then
  fail "scope: M1's promise alone lifted the cap with U1's edges glued into ngspice (quiet sets: $ss)"
elif (( gs < 9000 )); then
  fail "scope: $gs grants: expected at least 9000 (one cell while U1 is awake)"
else
  pass "scope: MOT_NSLEEP glued into ngspice: no quiet grant with Q = {devices}, $gs grants (one cell while U1 is awake)"
fi
same scope ref "scope: the CPU's edges and the device's records equal ref's (MOT_NSLEEP's glue loads only U1)" \
  non-grant-records

# The kernel part and negative control 1 (bbdev/tests/multi_producer.rs).
if (cd "$ROOT" && ${CARGO:-cargo} test -q -p bbdev --test multi_producer > "$OUT/kernel.log" 2>&1); then
  pass "kernel: $(grep -o 'test result: ok. [0-9]* passed' "$OUT/kernel.log") (bbdev/tests/multi_producer.rs: \
quiet grants with a device and a sampled analog producer; PromiseBroken for a device that reacts before its \
promise; rule 1 and internal timers refused; the scope rule)"
else
  fail "kernel: bbdev/tests/multi_producer.rs"; tail -20 "$OUT/kernel.log"
fi
if [[ $status == 0 ]]; then echo "multi-producer: PASS"; else echo "multi-producer: FAIL"; fi
exit $status
