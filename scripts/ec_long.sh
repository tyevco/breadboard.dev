#!/usr/bin/env bash
# ADR-004 Amendment 8: the long one-cell gate. Renode 1.17.0's External
# Control server crashes ("Stack overflow.") after about 524 000 messages
# on one connection, which the one-cell reference of a long or pin-heavy
# board used to reach (the ADC board's 1500 ms one-cell run died after
# 81 746 grants). The engine now replaces its connection every
# BB_RENODE_EC_MESSAGES (default 100 000) messages while Renode is paused.
#
# usage: ec_long.sh <bb> <out-dir> -- <bb run args of the ADC board, --for included>
#
# 1. The one-cell every-lattice-point reference (--lookahead off --adc-push
#    every, default reconnect interval) must finish, reconnect at least
#    once and pass check_adc.py.
# 2. The defaults (lookahead on) over the same span, in parallel, must pass
#    check_adc.py --require-no-stops and give the reference's nets.csv,
#    nets.vcd and non-grant records byte for byte.
set -euo pipefail
BB=$1
OUT=$2
shift 2
[ "${1:-}" = "--" ] && shift
PY=${PYTHON:-python3}
unset BB_RENODE_EC_MESSAGES
rm -rf "$OUT"
mkdir -p "$OUT"

run() { # name, extra args...
    local name=$1
    shift
    local t0 rc=0
    t0=$(date +%s.%N)
    "$BB" run "$@" --out "$OUT/$name" > "$OUT/$name.log" 2>&1 || rc=$?
    echo "$rc $(awk -v a="$t0" -v b="$(date +%s.%N)" 'BEGIN { printf "%.2f", b - a }')" \
        > "$OUT/$name.rc"
}

run ref "$@" --lookahead off --adc-push every &
p_ref=$!
run b "$@" &
p_b=$!
wait $p_ref $p_b || true

fail=0
for n in ref b; do
    read -r rc wall < "$OUT/$n.rc"
    grants=$(grep -c '"type":"grant"' "$OUT/$n/events.jsonl" 2>/dev/null || echo 0)
    echo "ec-long: $n rc=$rc grants=$grants wall=${wall}s"
    if [ "$rc" != 0 ]; then
        echo "ec-long: FAIL: $n exited $rc:"
        grep -v '^   at ' "$OUT/$n.log" | tail -8
        fail=1
    fi
done
[ $fail = 0 ] || exit 1

if ! grep -q 'External Control reconnected' "$OUT/ref.log"; then
    echo "ec-long: FAIL: the reference never reconnected (the gate needs a run past the interval)"
    exit 1
fi
grep 'External Control reconnected' "$OUT/ref.log" | sed 's/^/ec-long: ref: /'

"$PY" scripts/check_adc.py "$OUT/ref"
"$PY" scripts/check_adc.py "$OUT/b" --require-no-stops
for n in ref b; do
    grep -v '"type":"grant"' "$OUT/$n/events.jsonl" > "$OUT/$n/non-grant-records"
done
for f in nets.csv nets.vcd non-grant-records; do
    cmp "$OUT/ref/$f" "$OUT/b/$f"
done
echo "ec-long: PASS (the one-cell reference finishes across reconnects; lookahead on is identical except grant records)"
