#!/usr/bin/env bash
# ADR-003 plan step 5 gate: Renode's lookahead plugin is inert.
#
#   lookahead_inert.sh <bb> <out-dir> <feedback-hashes> -- <feedback bb run args>
#
# Runs examples/feedback (a feedback loop, so `bb run` loads the plugin by
# default) three ways, in parallel:
#   never  --lookahead-plugin never   (no plugin, so lookahead off: the step 4 run)
#   off    --lookahead off: plugin loaded and armed, no promise asked
#   on     the default since plan step 8: plugin loaded, lookahead on
# `never` must give the committed step 4 hashes (<feedback-hashes>), `off`
# must be byte-identical to `never` in nets.csv, events.jsonl and nets.vcd,
# and `on` must match `never` in nets.csv, nets.vcd and every non-grant
# record (lookahead changes only the grant partition).
set -euo pipefail

bb=$1 out=$2 hashes=$(realpath "$3")
shift 3
[ "${1:-}" = "--" ] && shift
rm -rf "$out"
mkdir -p "$out"

declare -A extra=(
    [never]="--lookahead-plugin never"
    [off]="--lookahead off"
    [on]=""
)
pids=()
for m in never off on; do
    # shellcheck disable=SC2086
    "$bb" run "$@" ${extra[$m]} --out "$out/$m" > "$out/$m.log" 2>&1 &
    pids+=("$!")
done
fail=0
for i in "${!pids[@]}"; do
    wait "${pids[$i]}" || fail=1
done
for m in never off on; do
    echo "--- $m: $(grep 'bb run:' "$out/$m.log" | tr '\n' ' ')"
done
if [ "$fail" != 0 ]; then
    tail -n 30 "$out"/*.log
    echo "lookahead-inert: a run failed" >&2
    exit 1
fi

for m in never off on; do
    grep -v '"type":"grant"' "$out/$m/events.jsonl" > "$out/$m/non-grant-records"
done
(cd "$out/never" && sha256sum -c "$hashes")
for f in nets.csv events.jsonl nets.vcd; do
    cmp "$out/never/$f" "$out/off/$f"
done
for f in nets.csv nets.vcd non-grant-records; do
    cmp "$out/never/$f" "$out/on/$f"
done
grants() { grep -c '"type":"grant"' "$1/events.jsonl"; }
echo "lookahead-inert: plugin armed (off) byte-identical to no plugin; on matches in" \
     "everything but grants ($(grants "$out/never") / $(grants "$out/off") / $(grants "$out/on")" \
     "grants never/off/on); no-plugin run equals the step 4 hashes: PASS"
