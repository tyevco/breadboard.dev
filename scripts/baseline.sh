#!/usr/bin/env bash
# Phase 2 H0: the runtime baseline (plan section 4 H0, section 5 item 5; ADR-015 D11).
# `make baseline` runs it under the host measurement lock.
#
#   scripts/baseline.sh <out> <N> <warmup: 0|1> -- <make arguments...>
#
# Runs `make <arguments>` (default `check`) N times on this checkout, after one warm-up run that is
# not counted when <warmup> is 1 (a fresh worktree builds a cold target directory first). Records
# each run's wall time, exit status and the load average before it, and the per-step times of
# `make test` (out/test-timing.tsv, scripts/step_time.sh), then the medians in <out>/summary.txt.
# Refuses a checkout that is not on a Linux filesystem (the baseline is an ext4 measurement;
# BASELINE_ANY_FS=1 overrides), and stops at the first failing run.
set -euo pipefail
out=$1 n=$2 warm=$3
shift 3
[[ ${1:-} == -- ]] && shift
(($#)) || set -- check
MAKE=${MAKE:-make}
unset MAKEFLAGS MFLAGS
fs=$(stat -f -c %T .)
case $fs in
  ext2/ext3|xfs|btrfs) ;;
  *) [[ ${BASELINE_ANY_FS:-0} == 1 ]] || { echo "baseline: $PWD is on '$fs'; measure on an ext4 worktree (make worktree), or BASELINE_ANY_FS=1" >&2; exit 2; } ;;
esac
mkdir -p "$out"
rm -f "$out"/*.log "$out"/*.tsv
{
  echo "# scripts/baseline.sh: make $* , N = $n (warm-up $warm)"
  echo "commit   $(git rev-parse HEAD)$(git diff --quiet HEAD -- . ':!out' || echo ' (dirty)')"
  echo "checkout $PWD ($fs)"
  echo "host     $(nproc) CPUs, $(awk '/MemTotal/ {printf "%.1f GiB", $2 / 1048576}' /proc/meminfo), kernel $(uname -r)"
  echo "started  $(date -u +%Y-%m-%dT%H:%M:%SZ)"
} > "$out/timing.txt"
for ((i = 1 - warm; i <= n; i++)); do
  if ((i <= 0)); then label=warmup; else label=run$i; fi
  load=$(cut -d' ' -f1-3 /proc/loadavg)
  rm -f out/test-timing.tsv
  echo "baseline: $label: make $* (load before $load)"
  t0=$(date +%s.%N)
  set +e
  $MAKE --no-print-directory "$@" > "$out/$label.log" 2>&1
  rc=$?
  set -e
  t1=$(date +%s.%N)
  [[ -f out/test-timing.tsv ]] && cp out/test-timing.tsv "$out/$label.tsv"
  secs=$(awk -v a="$t0" -v b="$t1" 'BEGIN { printf "%.1f", b - a }')
  printf '%-7s %8s s  rc=%s  load-before %s\n' "$label" "$secs" "$rc" "$load" | tee -a "$out/timing.txt"
  [[ $rc == 0 ]] || { tail -30 "$out/$label.log" >&2; echo "baseline: $label failed (rc=$rc); see $out/$label.log" >&2; exit 1; }
done
echo "finished $(date -u +%Y-%m-%dT%H:%M:%SZ)" >> "$out/timing.txt"

python3 - "$out" "$n" <<'EOF' | tee "$out/summary.txt"
import pathlib, re, statistics, sys
out, n = pathlib.Path(sys.argv[1]), int(sys.argv[2])
fmt = lambda s: f"{int(s) // 60}:{int(round(s)) % 60:02d}"
runs = {}
for line in (out / "timing.txt").read_text().splitlines():
    m = re.match(r"(run\d+)\s+([\d.]+) s", line)
    if m:
        runs[m[1]] = float(m[2])
tot = [runs[f"run{i}"] for i in range(1, n + 1)]
print(f"total: median {statistics.median(tot):.0f} s ({fmt(statistics.median(tot))}); runs " + ", ".join(f"{t:.0f}" for t in tot))
steps = {}
for i in range(1, n + 1):
    p = out / f"run{i}.tsv"
    if p.exists():
        for line in p.read_text().splitlines():
            name, secs, _rc = line.split("\t")
            steps.setdefault(name, []).append(float(secs))
if steps:
    s_test = sum(statistics.median(v) for v in steps.values())
    print(f"make test steps (median of each; sum {s_test:.0f} s = {fmt(s_test)}):")
    for name, v in steps.items():
        print(f"  {name:20s} {statistics.median(v):7.1f} s   runs " + ", ".join(f"{x:.1f}" for x in v))
    print(f"outside the make test steps (fmt-check, clippy, the bb and firmware builds): "
          f"{statistics.median(tot) - s_test:.0f} s")
EOF
