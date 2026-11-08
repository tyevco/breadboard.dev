#!/usr/bin/env bash
# ADR-003 plan step 9 (D6) gates for the ADC boards (examples/adc).
#
#   adc_equiv.sh <bb> <out-dir> <hashes> <open-hashes> <cyclic-args> -- <open-args>
#
# <cyclic-args> and <open-args> are each one quoted string of `bb run`
# arguments (board, firmware, probes, duration) without --out,
# --max-quantum, --lookahead and --adc-push. All runs go in parallel:
#
#   ref      cyclic board, --lookahead off --adc-push every: the one-cell
#            reference that pushes the ADC value at every lattice point
#   a        cyclic board, --lookahead off (Part A: one-cell lockstep, the
#            default push on a whole-millivolt change)
#   b-1ms    cyclic board, lookahead on (Part B), --max-quantum 1ms
#   b-10ms   cyclic board, bb's defaults (Part B, 10 ms)
#   open-ref open board (acyclic, ngspice -> U1), --max-quantum 10us
#            --adc-push every: its one-cell every-lattice-point reference
#   open-1ms, open-10ms, open-50ms
#            open board, Part A long grants: Renode splits each RUN_FOR at
#            the pushes in it and sets the value while paused there
#
# Gates: every run exits 0 (a broken quiet promise fails a run with
# PromiseBroken) and passes check_adc.py (every conversion's edges as the
# firmware model predicts from ADC_IN); the Part B runs print bb's
# "0 promises broken" line and stop early at pushes (`stop:ngspice`), among
# them pushes that move ADC_IN across the watchdog window inside a promised
# window (the analog-watchdog wakes, caught by the watch: check_adc.py
# --require-stops); ref
# and open-ref give the committed hashes <hashes> and <open-hashes>
# (nets.csv, events.jsonl, nets.vcd, non-grant-records; `make
# golden-update`); every other run matches its reference byte for byte in
# nets.csv, nets.vcd and every non-grant record.
set -euo pipefail

bb=$1 out=$2 hashes=$(realpath "$3") open_hashes=$(realpath "$4") cyc=$5
shift 5
[[ ${1:-} == -- ]] && shift
open=$1

rm -rf "$out"
mkdir -p "$out"
here=$(cd "$(dirname "$0")" && pwd)
declare -A args=(
    [ref]="$cyc --lookahead off --adc-push every"
    [a]="$cyc --lookahead off"
    [b-1ms]="$cyc --lookahead on --max-quantum 1ms"
    [b-10ms]="$cyc"
    [open-ref]="$open --max-quantum 10us --adc-push every"
    [open-1ms]="$open --max-quantum 1ms"
    [open-10ms]="$open --max-quantum 10ms"
    [open-50ms]="$open --max-quantum 50ms"
)
runs=(ref a b-1ms b-10ms open-ref open-1ms open-10ms open-50ms)
pids=()
for r in "${runs[@]}"; do
    # shellcheck disable=SC2086
    "$bb" run ${args[$r]} --out "$out/$r" > "$out/$r.log" 2>&1 &
    pids+=("$!")
done
fail=0
for i in "${!pids[@]}"; do
    wait "${pids[$i]}" || { echo "adc-equiv: run ${runs[$i]} failed:"; cat "$out/${runs[$i]}.log"; fail=1; }
done
[[ $fail -eq 0 ]] || { echo "adc-equiv: FAIL (a run failed)"; exit 1; }

grants() { grep -c '"type":"grant"' "$1/events.jsonl" || true; }
limits() { grep -o '"limited_by":"[^"]*"' "$1/events.jsonl" | sed 's/"limited_by"://; s/"//g' \
    | sort | uniq -c | awk '{ printf "%s%s %s", (NR > 1 ? ", " : ""), $1, $2 }'; }
status=0
for r in "${runs[@]}"; do
    d=$out/$r
    grep -v '"type":"grant"' "$d/events.jsonl" > "$d/non-grant-records"
    echo "adc-equiv: $r: $(grants "$d") grants ($(limits "$d"))"
    grep -h 'bb run:' "$d.log" | sed 's/^/adc-equiv:   /'
    opt=()
    [[ $r == b-* ]] && opt=(--require-stops)
    python3 "$here/check_adc.py" "$d" "${opt[@]}" | sed "s/^/adc-equiv:   /" \
        || { echo "adc-equiv: FAIL: $r check_adc"; status=1; }
done
check_ref() {
    if ! (cd "$out/$1" && sha256sum --quiet -c "$2"); then
        echo "adc-equiv: FAIL: $1 differs from the committed $2"
        status=1
    fi
}
check_ref ref "$hashes"
check_ref open-ref "$open_hashes"
same() {
    for f in nets.csv nets.vcd non-grant-records; do
        if ! cmp -s "$out/$1/$f" "$out/$2/$f"; then
            echo "adc-equiv: FAIL: $2: $f differs from $1"
            status=1
        fi
    done
}
for r in a b-1ms b-10ms; do same ref "$r"; done
for r in open-1ms open-10ms open-50ms; do same open-ref "$r"; done
for r in b-1ms b-10ms; do
    grep -q '^bb run: lookahead: .* 0 promises broken$' "$out/$r.log" \
        || { echo "adc-equiv: FAIL: $r: no lookahead summary"; status=1; }
    stops=$(grep -c '"limited_by":"stop:ngspice"' "$out/$r/events.jsonl" || true)
    [[ $stops -gt 0 ]] || { echo "adc-equiv: FAIL: $r: no early stop at an ADC push"; status=1; }
done
if [[ $status -eq 0 ]]; then
    echo "adc-equiv: PASS (Part A one-cell, Part B at 1 ms and 10 ms, and the open board at" \
         "1, 10 and 50 ms: nets.csv, nets.vcd and non-grant records identical to the" \
         "every-lattice-point reference; no promise broken; every conversion as predicted)"
else
    echo "adc-equiv: FAIL"
fi
exit $status
