#!/usr/bin/env bash
# Phase 2 H1, ADR-015 gate G1 (D1, the progress watchdog): the SAADC stall repro ends in an error,
# not a hang.
#
#   scripts/ngspice_stall.sh <bb> <out>
#
# The repro (ADR-008 Amendment 11 Finding 2, ADR-015 Context): examples/dial-me-spinner/e3/board.cir
# with P0.02 bound through the loading saadc_se view (its `load=none` removed), homing_step.elf (one
# full step, then quiet), 100 ms. Without the watchdog ngspice stalls 39.65 ms into the quiet
# stretch at 77.43 ms on the lattice point, prints `Reference value : 7.74300e-02` forever and grows
# without bound (2.3 GB after 10 minutes). The gate requires, at bb's default watchdog
# (200 000 points per cell, 10 000 points without progress):
#   - bb run fails with `NoProgress:ngspice at 77.43 ms` (the fs time of the tripping point rounds
#     to 77.430 ms) in the lattice cell (77430us, 77440us], with ngspice's reference value
#     7.74300e-02 in the message;
#   - within STALL_MAX_S (default 120 s) of wall time and STALL_MAX_RSS_KB (default 500 MB) of bb's
#     own peak RSS (ADR-015 G1);
#   - the outputs written before the error are those of a run that ends at the last completed
#     grant (--for 77430us): nets.csv, nets.vcd and the non-grant records byte for byte (the grant
#     records add the failed grant's own record);
#   - deterministic: at a lower limit (--ngspice-watchdog 20000,10000, which trips in the same cell)
#     two runs give the same message;
#   - the negative control: the committed board (load=none) runs the same 100 ms with no error, so
#     the trip is the loading view's stall and the default watchdog does not fire on a healthy run.
# Every run on a fresh Renode (BB_RENODE_POOL unset): a timing gate (`make ngspice-stall` takes the
# host measurement lock).
set -uo pipefail
BB=$1
OUT=$2
ROOT=$(cd "$(dirname "$0")/.." && pwd)
E3=$ROOT/examples/dial-me-spinner/e3
FWD=$ROOT/examples/dial-me-spinner/firmware/prebuilt
MAX_S=${STALL_MAX_S:-120}
MAX_RSS_KB=${STALL_MAX_RSS_KB:-500000}
W=$OUT/board
PINS="--probe net:VSYS --probe net:HOME_N --probe net:MOT_NSLEEP --probe net:U10.P0.26 --probe net:U10.P0.05
      --probe net:U10.P0.07 --probe net:U10.P0.11 --probe net:U10.P1.15 --probe net:M1.HOME_N"
status=0
pass() { echo "ngspice-stall: PASS: $*"; }
fail() { echo "ngspice-stall: FAIL: $*"; status=1; }
unset BB_RENODE_POOL

rm -rf "$OUT" && mkdir -p "$W"
# The repro board: a copy of the e3 directory (its bb.lock pins the same packages and platform) with
# P0.02 on the loading view.
cp "$E3"/board.cir "$E3"/bb.lock "$W/"
sed 's/^\(\*bb pin U10\.P0\.02 .*\) load=none$/\1/' "$E3/board.cir" > "$W/stall.cir"
if cmp -s "$W/board.cir" "$W/stall.cir" || grep -q '^\*bb pin U10\.P0\.02 .*load=none' "$W/stall.cir"; then
  fail "could not remove load=none from P0.02 in $E3/board.cir"; exit 1
fi

run() { # <name> <board> <for> [extra bb run args...]: wall seconds into $OUT/<name>.wall
  local name=$1 board=$2 for=$3
  shift 3
  local t0 t1
  t0=$(date +%s.%N)
  "$BB" run --netlist "$W/$board" --firmware "$FWD/homing_step.elf" --target U10 --for "$for" $PINS \
    --sample-rate 10kHz --out "$OUT/$name" "$@" > "$OUT/$name.log" 2>&1
  echo $? > "$OUT/$name.rc"
  t1=$(date +%s.%N)
  echo "$t1 - $t0" | bc > "$OUT/$name.wall"
}

run stall stall.cir 100ms
rc=$(cat "$OUT/stall.rc")
wall=$(cat "$OUT/stall.wall")
msg=$(grep '^bb: ' "$OUT/stall.log" | tail -1)
rss=$(sed -n 's/^bb run: peak RSS \([0-9]*\) kB.*/\1/p' "$OUT/stall.log" | tail -1)
echo "ngspice-stall: repro: rc $rc, wall ${wall} s, bb peak RSS ${rss:-?} kB"
echo "ngspice-stall: repro: ${msg}"
if [[ $rc -ne 0 ]] && grep -q 'NoProgress:ngspice at 77430[0-9]*fs' <<<"$msg" \
   && grep -q 'lattice cell (77430us, 77440us\]' <<<"$msg" && grep -q 'Reference value : 7.74300e-02' <<<"$msg"; then
  at=$(sed -n 's/.*NoProgress:ngspice at \([0-9]*\)fs.*/\1/p' <<<"$msg")
  pass "the repro ends in NoProgress:ngspice at ${at} fs ($(echo "scale=3; $at / 1000000000000" | bc) ms) in the cell (77.43 ms, 77.44 ms]"
else
  fail "the repro did not end in NoProgress:ngspice at 77.43 ms (rc $rc)"; tail -5 "$OUT/stall.log"
fi
if (( $(echo "$wall < $MAX_S" | bc) )); then pass "within ${MAX_S} s of wall time (${wall} s)"; else fail "wall ${wall} s >= ${MAX_S} s"; fi
if [[ -n $rss ]] && (( rss < MAX_RSS_KB )); then pass "bounded memory: bb's peak RSS ${rss} kB < ${MAX_RSS_KB} kB"; else fail "bb's peak RSS ${rss:-unknown} kB (limit ${MAX_RSS_KB} kB)"; fi

# The outputs up to the last completed grant: those of a run that ends there.
run upto stall.cir 77430us
if [[ $(cat "$OUT/upto.rc") -eq 0 ]]; then
  ok=1
  for f in nets.csv nets.vcd; do cmp -s "$OUT/stall/$f" "$OUT/upto/$f" || { ok=0; echo "ngspice-stall:   $f differs"; }; done
  cmp -s <(grep -v '"type":"grant"' "$OUT/stall/events.jsonl") <(grep -v '"type":"grant"' "$OUT/upto/events.jsonl") \
    || { ok=0; echo "ngspice-stall:   the non-grant records differ"; }
  # The grant records differ by design: the stalled run also records the grant it failed in
  # (77.43 -> 77.44 ms), and the shorter run's last grant is limited by its end.
  gs=$(grep -c '"type":"grant"' "$OUT/stall/events.jsonl")
  gu=$(grep -c '"type":"grant"' "$OUT/upto/events.jsonl")
  grep '"type":"grant"' "$OUT/stall/events.jsonl" | tail -1 | grep -q '"t_fs":77430000000000,"until_fs":77440000000000' \
    && [[ $gs -eq $((gu + 1)) ]] || { ok=0; echo "ngspice-stall:   grants: $gs in the stalled run, $gu in the shorter one"; }
  [[ $ok -eq 1 ]] && pass "the outputs written before the error equal a --for 77430us run (nets.csv, nets.vcd and the non-grant records byte for byte; $gu completed grants, then the failed one's record)" \
    || fail "the outputs before the error differ from a run that ends at 77.43 ms"
else
  fail "the run to 77430us failed"; tail -3 "$OUT/upto.log"
fi

# Deterministic: a lower limit trips in the same cell, twice with the same message.
run low1 stall.cir 100ms --ngspice-watchdog 20000,10000
run low2 stall.cir 100ms --ngspice-watchdog 20000,10000
m1=$(grep '^bb: ' "$OUT/low1.log" | tail -1)
m2=$(grep '^bb: ' "$OUT/low2.log" | tail -1)
if [[ -n $m1 && $m1 == "$m2" ]] && grep -q 'NoProgress:ngspice at 77430[0-9]*fs: 20001 accepted solver points in the lattice cell (77430us, 77440us\]' <<<"$m1"; then
  pass "deterministic: at 20000 points per cell two runs fail identically, in the same cell ($(cat "$OUT/low1.wall") s, $(cat "$OUT/low2.wall") s)"
else
  fail "the low-limit runs differ or did not trip in the cell: [$m1] [$m2]"
fi

# The negative control: the committed board (load=none) is healthy over the same 100 ms.
run healthy board.cir 100ms
if [[ $(cat "$OUT/healthy.rc") -eq 0 ]]; then
  pass "negative control: the committed board (load=none) runs 100 ms without a watchdog error ($(cat "$OUT/healthy.wall") s)"
else
  fail "negative control: the committed board failed"; tail -3 "$OUT/healthy.log"
fi
[[ $status -eq 0 ]] && echo "ngspice-stall: all checks passed" || echo "ngspice-stall: FAILED"
exit $status
