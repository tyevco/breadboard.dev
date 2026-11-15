#!/usr/bin/env bash
# Phase 2 H0: run one step of `make test` and append its wall time to a TSV (target, seconds,
# exit status), so the runtime baseline and the gate budgets have a per-gate breakdown
# (out/test-timing.tsv; scripts/baseline.sh collects it). Wall time is a report here, never an
# output of a run.
#
#   scripts/step_time.sh <tsv> <make> <target> [make args...]
set -euo pipefail
log=$1 make=$2 target=$3
shift 3
t0=$(date +%s.%N)
set +e
"$make" "$target" "$@"
rc=$?
set -e
t1=$(date +%s.%N)
awk -v t="$target" -v a="$t0" -v b="$t1" -v rc="$rc" 'BEGIN { printf "%s\t%.1f\t%d\n", t, b - a, rc }' >> "$log"
exit $rc
