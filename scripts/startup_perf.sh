#!/usr/bin/env bash
# ADR-004 G1: fresh `bb run` engine startup with Part A against a baseline,
# with the ADR-003 measurement protocol: interleaved A/B runs, N of each,
# medians, load average recorded.
#
# Usage: scripts/startup_perf.sh <bb> <out_root> <n> <min_frac> <min_s> <baseline> \
#            -- <bb run args...>
#   <baseline>  "stock": the same <bb> with BB_RENODE_STOCK_RUNTIME=1 (the
#               runtime settings, no JIT profile and the 100 ms poll before Part A);
#               otherwise the path of another bb binary (e.g. one built from
#               the commit before ADR-004)
#   <min_frac>  median(A) - median(B) >= min_frac * median(A)  (- = no check)
#   <min_s>     median(A) - median(B) >= min_s seconds          (- = no check)
#   <bb run args> without --out
#
# Both cases run fresh (BB_RENODE_POOL is removed from their environment),
# except that with STARTUP_B_POOL=1 case B keeps BB_RENODE_POOL (run under
# `bb pool run`: the pool against the baseline; every B run must be leased).
# Every B run must match the first A run in nets.csv, nets.vcd and the
# non-grant event records: Part A changes no result.
set -euo pipefail

bb=$1 root=$2 n=$3 min_frac=$4 min_s=$5 base=$6
shift 6
[[ ${1:-} == -- ]] && shift

rm -rf "$root"
mkdir -p "$root"
results=$root/results.tsv
printf 'run\tcase\tstartup_s\tsim_s\ttotal_s\tload1\n' > "$results"
if [[ $base == stock ]]; then
  echo "startup-perf: A = BB_RENODE_STOCK_RUNTIME=1 $bb, B = $bb (ADR-004 Part A), N = $n, interleaved"
else
  echo "startup-perf: A = $base, B = $bb (ADR-004 Part A), N = $n, interleaved"
fi

run_one() { # <k> <case>
  local k=$1 c=$2 dir=$root/$2-$1 load
  shift 2
  load=$(cut -d' ' -f1 /proc/loadavg)
  local -a cmd=(env -u BB_RENODE_POOL)
  if [[ $c == B && ${STARTUP_B_POOL:-0} == 1 ]]; then
    cmd=(env)
  fi
  if [[ $c == A && $base == stock ]]; then
    cmd+=(BB_RENODE_STOCK_RUNTIME=1 "$bb")
  elif [[ $c == A ]]; then
    cmd+=(-u BB_RENODE_STOCK_RUNTIME "$base")
  else
    cmd+=(-u BB_RENODE_STOCK_RUNTIME "$bb")
  fi
  "${cmd[@]}" run "$@" --out "$dir" > "$dir.log" 2>&1 \
    || { echo "startup-perf: run $dir failed:"; cat "$dir.log"; exit 1; }
  local total startup sim
  total=$(sed -n 's/.*wall time \([0-9.]*\) s total.*/\1/p' "$dir.log")
  startup=$(sed -n 's/.*(\([0-9.]*\) s engine startup.*/\1/p' "$dir.log")
  sim=$(sed -n 's/.*, \([0-9.]*\) s simulation).*/\1/p' "$dir.log")
  grep -v '"type":"grant"' "$dir/events.jsonl" > "$dir/non-grant-records"
  if [[ $c == B && ${STARTUP_B_POOL:-0} == 1 ]] && ! grep -q '^bb run: renode: leased' "$dir.log"; then
    echo "startup-perf: FAIL: $dir was not leased from the pool"; exit 1
  fi
  printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$k" "$c" "$startup" "$sim" "$total" "$load" >> "$results"
  echo "startup-perf: run $k $c: startup $startup s, sim $sim s, total $total s, load $load"
}

for k in $(seq 1 "$n"); do
  run_one "$k" A "$@"
  run_one "$k" B "$@"
done

median() { # <case> <column>
  awk -F'\t' -v c="$1" -v col="$2" 'NR > 1 && $2 == c { print $col }' "$results" \
    | sort -g | awk '{ a[NR] = $1 } END { if (NR % 2) print a[(NR + 1) / 2]; else print (a[NR / 2] + a[NR / 2 + 1]) / 2 }'
}
status=0
for k in $(seq 1 "$n"); do
  for f in nets.csv nets.vcd non-grant-records; do
    cmp -s "$root/A-1/$f" "$root/B-$k/$f" \
      || { echo "startup-perf: FAIL: B-$k $f differs from A-1"; status=1; }
  done
done
a=$(median A 3) b=$(median B 3)
for c in A B; do
  echo "startup-perf: median $c: startup $(median $c 3) s, sim $(median $c 4) s, total $(median $c 5) s, load $(median $c 6)"
done
d=$(awk -v a="$a" -v b="$b" 'BEGIN { printf "%.3f", a - b }')
frac=$(awk -v a="$a" -v b="$b" 'BEGIN { printf "%.3f", (a - b) / a }')
echo "startup-perf: median startup A - B = $d s ($(awk -v f="$frac" 'BEGIN { printf "%.1f", 100 * f }') % of A)"
if [[ $min_frac != - ]] && ! awk -v f="$frac" -v m="$min_frac" 'BEGIN { exit !(f >= m) }'; then
  echo "startup-perf: FAIL: startup cut $frac of A < $min_frac"; status=1
fi
if [[ $min_s != - ]] && ! awk -v d="$d" -v m="$min_s" 'BEGIN { exit !(d >= m) }'; then
  echo "startup-perf: FAIL: startup cut $d s < $min_s s"; status=1
fi
if [[ $status -eq 0 ]]; then
  echo "startup-perf: PASS (outputs identical; cut $d s, fraction $frac; needs >= $min_s s and >= $min_frac)"
fi
exit $status
