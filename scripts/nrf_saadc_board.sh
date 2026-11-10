#!/usr/bin/env bash
# Phase 1 M8 (ADR-008 D7/D8), the board half of make nrf-saadc-model:
# examples/nrf-saadc (the loading adc_in on AIN0, VDD, UARTE0 lines) with
# nrf_saadc_burst.elf, 30 ms.
#
#   nrf_saadc_board.sh <bb> <out-dir> <hashes> [update]
#
# Runs, in parallel:
#   ref       --lookahead off --adc-push every: the one-cell reference that
#             pushes V(hold) at every lattice point
#   a         --lookahead off (one cell, a push on every uV change)
#   b-1ms     lookahead on, --max-quantum 1ms
#   b-10ms    bb's defaults (lookahead on, 10 ms)
#   order     the defaults with --engine-order analog-first
#   refuse    --renode-quantum 25us: TACQ 40 us < 2 L, so the run must fail
#             with ADR-008 D7's message naming both remedies
#   unloaded  a copy of the board with load=none on AIN0, --renode-quantum
#             25us: runs (the explicit escape), no #smp toggle, the samples
#             read the unloaded pad
# Gates: every run but refuse exits 0 and passes check_nrf_saadc.py (8 #smp
# toggles and 8 charge transfers per 8x burst, exactly; codes; UART lines;
# no stop:ngspice); the lookahead runs print "0 promises broken"; ref gives
# the committed hashes (nets.csv, events.jsonl, nets.vcd,
# non-grant-records; `update` rewrites them); a, b-1ms, b-10ms and order
# equal ref in nets.csv, nets.vcd and every non-grant record (order also
# equals b-10ms byte for byte, grant records included).
set -euo pipefail

bb=$1 out=$2 hashes=$(realpath -m "$3") mode=${4:-check}
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
fw=$root/engines/cpu-renode/tests/firmware/nrf52840/build/nrf_saadc_burst.elf
board=$root/examples/nrf-saadc/board.cir
rm -rf "$out"
mkdir -p "$out/unloaded-board"
# The escape: the same board with an explicit load=none on AIN0 (its own
# bb.lock: the same package and platform).
sed 's/^\*bb pin U1.P0.02 net=AIN model=adc_in vref=3.6 trace_samples=true$/&  load=none/' \
    "$board" > "$out/unloaded-board/board.cir"
grep -q 'load=none' "$out/unloaded-board/board.cir"
cp "$root/examples/nrf-saadc/bb.lock" "$out/unloaded-board/bb.lock"
common="--target U1 --firmware $fw --for 30ms --probe net:AIN --probe net:MARK"
declare -A args=(
    [ref]="--netlist $board $common --lookahead off --adc-push every"
    [a]="--netlist $board $common --lookahead off"
    [b-1ms]="--netlist $board $common --max-quantum 1ms"
    [b-10ms]="--netlist $board $common"
    [order]="--netlist $board $common --engine-order analog-first"
    [refuse]="--netlist $board $common --renode-quantum 25us"
    [unloaded]="--netlist $out/unloaded-board/board.cir $common --renode-quantum 25us"
)
runs=(ref a b-1ms b-10ms order refuse unloaded)
pids=()
for r in "${runs[@]}"; do
    # shellcheck disable=SC2086
    "$bb" run ${args[$r]} --out "$out/$r" > "$out/$r.log" 2>&1 &
    pids+=("$!")
done
status=0
for i in "${!pids[@]}"; do
    r=${runs[$i]}
    if wait "${pids[$i]}"; then
        [[ $r == refuse ]] && { echo "nrf-saadc: FAIL: refuse: the run did not fail"; status=1; }
    elif [[ $r != refuse ]]; then
        echo "nrf-saadc: FAIL: run $r failed:"; tail -20 "$out/$r.log"; status=1
    fi
done
[[ $status -eq 0 ]] || exit 1

# The refusal and its message (ADR-008 D7: both remedies named).
msg=$(cat "$out/refuse.log")
for want in "TACQ 40 us < 2 L = 50 us" "--renode-quantum 20us" 'load = "none"'; do
    grep -qF -- "$want" <<<"$msg" || { echo "nrf-saadc: FAIL: refuse: no '$want' in:"; echo "$msg" | tail -5; status=1; }
done
echo "nrf-saadc: refuse: $(grep -o 'the SAADC acquired[^(]*' "$out/refuse.log" | head -1)... (both remedies named)"

grants() { grep -c '"type":"grant"' "$1/events.jsonl" || true; }
for r in ref a b-1ms b-10ms order unloaded; do
    d=$out/$r
    grep -v '"type":"grant"' "$d/events.jsonl" > "$d/non-grant-records"
    opt=()
    [[ $r == unloaded ]] && opt=(--unloaded)
    echo "nrf-saadc: $r: $(grants "$d") grants"
    grep -h 'bb run: simulated\|bb run: lookahead' "$out/$r.log" | sed 's/^/nrf-saadc:   /'
    python3 "$here/check_nrf_saadc.py" "$d" "${opt[@]}" | sed 's/^/nrf-saadc:   /' \
        || { echo "nrf-saadc: FAIL: $r check"; status=1; }
    if [[ $r == b-* || $r == order ]]; then
        grep -q '0 promises broken' "$out/$r.log" || { echo "nrf-saadc: FAIL: $r: a promise was broken"; status=1; }
    fi
done
if [[ $mode == update ]]; then
    mkdir -p "$(dirname "$hashes")"
    (cd "$out/ref" && sha256sum nets.csv events.jsonl nets.vcd non-grant-records) > "$hashes"
    echo "nrf-saadc: wrote $hashes"
fi
(cd "$out/ref" && sha256sum -c --quiet "$hashes") \
    && echo "nrf-saadc: ref equals the committed hashes $hashes" \
    || { echo "nrf-saadc: FAIL: ref differs from $hashes"; status=1; }
for r in a b-1ms b-10ms order; do
    for f in nets.csv nets.vcd non-grant-records; do
        cmp -s "$out/ref/$f" "$out/$r/$f" || { echo "nrf-saadc: FAIL: $r/$f differs from the one-cell reference"; status=1; }
    done
done
cmp -s "$out/b-10ms/events.jsonl" "$out/order/events.jsonl" \
    || { echo "nrf-saadc: FAIL: --engine-order analog-first changed events.jsonl"; status=1; }
[[ $status -eq 0 ]] && echo "nrf-saadc: a, b-1ms, b-10ms and order equal the one-cell reference (nets.csv, nets.vcd, non-grant records); analog-first byte-identical"
exit $status
