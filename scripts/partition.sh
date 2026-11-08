#!/usr/bin/env bash
# ADR-003 partition-equivalence gate: the same run at several --max-quantum
# values must give byte-identical nets.csv and nets.vcd, equal to the
# committed baseline hashes, and identical non-grant event records.
#
# Usage: scripts/partition.sh <bb> <out_root> <jobs> <hash_file> <q1,q2,...> -- <bb run args...>
#   (without --out / --max-quantum; each run writes <out_root>/mq-<q>)
#
# <hash_file> holds sha256sum lines for `nets.csv`, `nets.vcd` and
# `non-grant-records` (events.jsonl without its `grant` lines), taken from
# the one-cell reference run (max_quantum = the lattice). Regenerate it with
# `make golden-update` after a reviewed waveform change.
set -euo pipefail

bb=$1 root=$2 jobs=$3 hashes=$4 quanta=$5
shift 5
[[ ${1:-} == -- ]] && shift

want() { awk -v f="$1" '$2 == f { print $1 }' "$hashes"; }
for f in nets.csv nets.vcd non-grant-records; do
  [[ -n $(want "$f") ]] || { echo "partition: FAIL: no hash for $f in $hashes"; exit 1; }
done

rm -rf "$root"
mkdir -p "$root"
tr ',' '\n' <<<"$quanta" | xargs -P "$jobs" -I{} bash -c '
  set -e
  bb=$1; dir=$2; q=$3; shift 3
  "$bb" run "$@" --max-quantum "$q" --out "$dir" > "$dir.log" 2>&1 \
    || { echo "run $dir failed:"; cat "$dir.log"; exit 1; }
' _ "$bb" "$root/mq-{}" {} "$@"

status=0
for q in $(tr ',' ' ' <<<"$quanta"); do
  d=$root/mq-$q
  grep -v '"type":"grant"' "$d/events.jsonl" > "$d/non-grant-records"
  grants=$(grep -c '"type":"grant"' "$d/events.jsonl" || true)
  line="partition: max_quantum $q: $grants grants;"
  for f in nets.csv nets.vcd non-grant-records; do
    got=$(sha256sum "$d/$f" | awk '{print $1}')
    if [[ $got == "$(want "$f")" ]]; then
      line+=" $f ok"
    else
      line+=" $f MISMATCH ($got)"
      status=1
    fi
  done
  echo "$line"
  grep -h 'wall time' "$d.log" | sed 's/^/partition:   /' || true
done
if [[ $status -eq 0 ]]; then
  echo "partition: PASS (max_quantum in {$quanta}: identical to the baseline hashes)"
else
  echo "partition: FAIL"
fi
exit $status
