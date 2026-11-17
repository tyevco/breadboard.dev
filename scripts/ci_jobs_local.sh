#!/usr/bin/env bash
# Phase 2 H6 (ADR-015 D11, Amendment 5): each CI job's local equivalent, timed. CI runs only on a
# push, which this project does not do from an agent, so the PR tier's "slowest CI job at most 15
# minutes" is checked here first and confirmed on the first push. The jobs and their commands are
# read from the workflow itself (default .github/workflows/ci.yml): every `run:` step of each job,
# in order, in one shell; the `uses:` steps (checkout, the caches, the bb-setup action, the JUnit
# reporter) are not run. The setup step's own time is H0's CI median plus make setup-cache-cost's
# measured change, reported apart. A job's commands stop at the first failure (as on CI).
#
#   scripts/ci_jobs_local.sh <out-dir> [<workflow>] [<job> ...]   (default: every job but
#                                                                   CI_JOBS_SKIP="fw-rebuild")
# fw-rebuild is skipped by default: it builds the Zephyr images in a fresh west workspace, which
# fetches Zephyr from the network, and H6 changes nothing in it but its setup step.
# Writes <out>/<job>.log and <out>/results.tsv (job, seconds, exit, load before), and prints them.
set -uo pipefail
out=$1
wf=${2:-.github/workflows/ci.yml}
shift $(( $# >= 2 ? 2 : $# ))
mkdir -p "$out"
rm -f "$out"/*.log "$out"/*.sh "$out/results.tsv"
python3 - "$wf" "$out" "${CI_JOBS_SKIP-fw-rebuild}" "$@" <<'EOF' || exit 1
import sys, yaml
wf, out, skip, only = sys.argv[1], sys.argv[2], sys.argv[3].split(), sys.argv[4:]
jobs = yaml.safe_load(open(wf))["jobs"]
for name, job in jobs.items():
    if (only and name not in only) or (not only and name in skip):
        continue
    runs = [s["run"] for s in job["steps"] if "run" in s]
    with open(f"{out}/{name}.sh", "w") as f:
        f.write("set -e\n")
        for r in runs:
            f.write(r.rstrip("\n") + "\n")
    print(name)
EOF
# On CI the bb-setup action's make setup installs a release `bb` into ~/.local/bin (setup.sh), and
# the `test` job's make doctor requires it on PATH. Here the jobs get this checkout's release build
# the same way, through a directory of their own first on PATH (nothing is installed).
bbdir="$(cd "$out" && pwd)/bin"
mkdir -p "$bbdir"
ln -sfn "$(env -u MAKEFLAGS -u MFLAGS make -s --no-print-directory print-CARGO_TARGET_DIR)/release/bb" "$bbdir/bb"
[ -x "$bbdir/bb" ] || { echo "ci-jobs-local: no release bb at $(readlink "$bbdir/bb") (make bb)" >&2; exit 1; }
export PATH="$bbdir:$PATH"
printf 'job\tseconds\texit\tload_before\n' > "$out/results.tsv"
for sh in "$out"/*.sh; do
  job=$(basename "$sh" .sh)
  load=$(cut -d' ' -f1 /proc/loadavg)
  t0=$(date +%s.%N)
  (unset MAKEFLAGS MFLAGS; bash "$sh") > "$out/$job.log" 2>&1
  rc=$?
  awk -v j="$job" -v a="$t0" -v b="$(date +%s.%N)" -v rc="$rc" -v l="$load" \
    'BEGIN { printf "%s\t%.1f\t%d\t%s\n", j, b - a, rc, l }' >> "$out/results.tsv"
  tail -1 "$out/results.tsv"
done
echo "ci-jobs-local: $(awk -F'\t' 'NR > 1 && $3 != 0' "$out/results.tsv" | wc -l) job(s) failed; results in $out/results.tsv"
! awk -F'\t' 'NR > 1 && $3 != 0 { bad = 1 } END { exit !bad }' "$out/results.tsv"
