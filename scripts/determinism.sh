#!/usr/bin/env bash
# Run the Phase 0 spike N times and require byte-identical outputs.
#
# Usage: scripts/determinism.sh <bb> <out_root> <runs> <jobs> -- <bb run args...>
#   (without --out; each run writes <out_root>/run-NN)
#
# Runs are separate processes (ngspice is a per-process singleton), up to
# <jobs> at a time. Prints the sha256 of nets.csv, events.jsonl and nets.vcd of
# every run and fails unless each file is identical across all runs.
set -euo pipefail

bb=$1 root=$2 runs=$3 jobs=$4
shift 4
[[ ${1:-} == -- ]] && shift

rm -rf "$root"
mkdir -p "$root"
seq -w 1 "$runs" | xargs -P "$jobs" -I{} bash -c '
  set -e
  bb=$1; dir=$2; shift 2
  "$bb" run "$@" --out "$dir" > "$dir.log" 2>&1 || { echo "run $dir failed:"; cat "$dir.log"; exit 1; }
' _ "$bb" "$root/run-{}" "$@"

status=0
for f in nets.csv events.jsonl nets.vcd; do
  echo "== sha256 $f"
  (cd "$root" && sha256sum run-*/"$f")
  n=$(cd "$root" && sha256sum run-*/"$f" | awk '{print $1}' | sort -u | wc -l)
  if [[ $n -eq 1 ]]; then
    echo "determinism: $f identical across $runs runs"
  else
    echo "determinism: FAIL: $f has $n distinct hashes across $runs runs"
    status=1
  fi
done
grep -h 'wall time' "$root"/run-*.log | sed 's/^/determinism: /' || true
grep -h '^bb run: renode:' "$root"/run-*.log | sed 's/ (pid [0-9]*)//' | sort | uniq -c \
  | sed 's/^ */determinism: renode: /' || true
if [[ $status -eq 0 ]]; then echo "determinism: PASS ($runs runs)"; fi
exit $status
