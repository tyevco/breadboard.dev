#!/usr/bin/env bash
# ADR-003 plan step 8 equivalence gate: CPU lookahead on a board with a
# feedback loop changes only the grant partition.
#
#   lookahead_equiv.sh <bb> <out-dir> <max-grants> <name>=<elf>:<hash-file> ... -- <bb run args>
#
# <bb run args> are the board's arguments without --firmware, --out,
# --max-quantum and --lookahead. For every firmware <name>, three runs, all
# in parallel:
#   off     --lookahead off: the one-cell reference (the loop cap binds in
#           every grant whatever --max-quantum is)
#   on-1ms  --lookahead on --max-quantum 1ms
#   on-10ms --lookahead on --max-quantum 10ms (bb's defaults)
# Each must exit 0 (a broken quiet promise fails the run with
# PromiseBroken, so this is the "0 promise violations" check) and print
# bb's "0 promises broken" lookahead line. The reference must give the
# committed hashes in <hash-file> (nets.csv, events.jsonl, nets.vcd and
# non-grant-records, `make golden-update`) and pass check_feedback.py; both
# `on` runs must match it byte for byte in nets.csv, nets.vcd and every
# non-grant record. For the first firmware (the WFI board) the on-10ms run
# must take at most <max-grants> grants.
set -euo pipefail

bb=$1 out=$2 max_grants=$3
shift 3
cases=()
while [[ $# -gt 0 && $1 != -- ]]; do
    cases+=("$1")
    shift
done
[[ ${1:-} == -- ]] && shift
[[ ${#cases[@]} -gt 0 ]] || { echo "lookahead-equiv: no firmware given" >&2; exit 2; }

rm -rf "$out"
mkdir -p "$out"
here=$(cd "$(dirname "$0")" && pwd)
modes=(off on-1ms on-10ms)
declare -A extra=(
    [off]="--lookahead off"
    [on-1ms]="--lookahead on --max-quantum 1ms"
    [on-10ms]="--lookahead on --max-quantum 10ms"
)

pids=() names=()
for c in "${cases[@]}"; do
    name=${c%%=*} rest=${c#*=}
    elf=${rest%%:*}
    for m in "${modes[@]}"; do
        d=$out/$name/$m
        mkdir -p "$(dirname "$d")"
        # shellcheck disable=SC2086
        "$bb" run "$@" --firmware "$elf" ${extra[$m]} --out "$d" > "$d.log" 2>&1 &
        pids+=("$!") names+=("$name/$m")
    done
done
fail=0
for i in "${!pids[@]}"; do
    wait "${pids[$i]}" || { echo "lookahead-equiv: run ${names[$i]} failed:"; cat "$out/${names[$i]}.log"; fail=1; }
done
[[ $fail -eq 0 ]] || { echo "lookahead-equiv: FAIL (a run failed)"; exit 1; }

grants() { grep -c '"type":"grant"' "$1/events.jsonl" || true; }
limits() { grep -o '"limited_by":"[^"]*"' "$1/events.jsonl" | sed 's/"limited_by"://; s/"//g' \
    | sort | uniq -c | awk '{ printf "%s%s %s", (NR > 1 ? ", " : ""), $1, $2 }'; }
status=0
first=1
for c in "${cases[@]}"; do
    name=${c%%=*} rest=${c#*=}
    hashes=$(realpath "${rest#*:}")
    for m in "${modes[@]}"; do
        d=$out/$name/$m
        grep -v '"type":"grant"' "$d/events.jsonl" > "$d/non-grant-records"
        echo "lookahead-equiv: $name $m: $(grants "$d") grants ($(limits "$d"))"
        grep -h 'bb run:' "$d.log" | sed 's/^/lookahead-equiv:   /'
    done
    ref=$out/$name/off
    if ! (cd "$ref" && sha256sum --quiet -c "$hashes"); then
        echo "lookahead-equiv: FAIL: $name one-cell reference differs from $hashes"
        status=1
    fi
    python3 "$here/check_feedback.py" "$ref" | sed "s/^/lookahead-equiv: $name off: /" || status=1
    for m in on-1ms on-10ms; do
        d=$out/$name/$m
        grep -q '^bb run: lookahead: .* 0 promises broken$' "$d.log" \
            || { echo "lookahead-equiv: FAIL: $name $m: no lookahead summary"; status=1; }
        for f in nets.csv nets.vcd non-grant-records; do
            if ! cmp -s "$ref/$f" "$d/$f"; then
                echo "lookahead-equiv: FAIL: $name $m: $f differs from the one-cell reference"
                status=1
            fi
        done
    done
    if [[ $first -eq 1 ]]; then
        g=$(grants "$out/$name/on-10ms")
        if [[ $g -gt $max_grants ]]; then
            echo "lookahead-equiv: FAIL: $name on-10ms took $g grants > $max_grants"
            status=1
        fi
        first=0
    fi
done
if [[ $status -eq 0 ]]; then
    echo "lookahead-equiv: PASS (lookahead on at max_quantum 1 ms and 10 ms: nets.csv, nets.vcd" \
         "and non-grant records identical to the one-cell reference; no promise broken;" \
         "${cases[0]%%=*} on-10ms <= $max_grants grants)"
else
    echo "lookahead-equiv: FAIL"
fi
exit $status
