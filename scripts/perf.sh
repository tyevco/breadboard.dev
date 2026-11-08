#!/usr/bin/env bash
# ADR-003 performance gate, measured with the ADR's protocol: interleaved A/B
# runs (A = baseline --max-quantum, B = candidate), N of each, medians of the
# simulation time (startup reported separately), user+sys CPU, grant counts
# and the load average.
#
# Usage: scripts/perf.sh <bb> <out_root> <n> <qA> <qB> <grantsB> <max_sim_s> <min_ratio> -- <bb run args...>
#   (without --out / --max-quantum)
#
# Passes when every B run has exactly <grantsB> grants, the median B
# simulation time is <= <max_sim_s> and median(A sim) / median(B sim) >=
# <min_ratio>. Wall-clock numbers never reach the simulation outputs.
set -euo pipefail

bb=$1 root=$2 n=$3 qa=$4 qb=$5 grants_b=$6 max_sim=$7 min_ratio=$8
shift 8
[[ ${1:-} == -- ]] && shift

rm -rf "$root"
mkdir -p "$root"
results=$root/results.tsv
printf 'run\tcase\tmax_quantum\tgrants\ttotal_s\tstartup_s\tsim_s\tcpu_user_sys_s\tload1\n' > "$results"

run_one() { # <k> <case> <quantum>
  local k=$1 c=$2 q=$3 dir=$root/$2-$1 load cpu
  shift 3
  load=$(cut -d' ' -f1 /proc/loadavg)
  # user+sys CPU of the bb process and its children (Renode, ngspice).
  local TIMEFORMAT='%U %S'
  { time "$bb" run "$@" --max-quantum "$q" --out "$dir" > "$dir.log" 2>&1; } 2> "$dir.time" \
    || { echo "perf: run $dir failed:"; cat "$dir.log"; exit 1; }
  cpu=$(awk '{printf "%.2f", $1 + $2}' "$dir.time")
  local grants total startup sim
  grants=$(sed -n 's/.* in \([0-9]*\) grants.*/\1/p' "$dir.log")
  total=$(sed -n 's/.*wall time \([0-9.]*\) s total.*/\1/p' "$dir.log")
  startup=$(sed -n 's/.*(\([0-9.]*\) s engine startup.*/\1/p' "$dir.log")
  sim=$(sed -n 's/.*, \([0-9.]*\) s simulation).*/\1/p' "$dir.log")
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$k" "$c" "$q" "$grants" "$total" "$startup" "$sim" "$cpu" "$load" >> "$results"
  echo "perf: run $k $c (max_quantum $q): $grants grants, sim $sim s, startup $startup s, total $total s, cpu $cpu s, load $load"
}

for k in $(seq 1 "$n"); do
  run_one "$k" A "$qa" "$@"
  run_one "$k" B "$qb" "$@"
done

median() { # <case> <column>
  awk -F'\t' -v c="$1" -v col="$2" 'NR > 1 && $2 == c { print $col }' "$results" \
    | sort -g | awk '{ a[NR] = $1 } END { if (NR % 2) print a[(NR + 1) / 2]; else print (a[NR / 2] + a[NR / 2 + 1]) / 2 }'
}
column -t -s $'\t' "$results" | sed 's/^/perf: /'
status=0
for c in A B; do
  echo "perf: median $c: sim $(median $c 7) s, startup $(median $c 6) s, total $(median $c 5) s, cpu $(median $c 8) s, grants $(median $c 4), load $(median $c 9)"
done
sim_a=$(median A 7) sim_b=$(median B 7)
bad_grants=$(awk -F'\t' -v g="$grants_b" 'NR > 1 && $2 == "B" && $4 != g' "$results" | wc -l)
ratio=$(awk -v a="$sim_a" -v b="$sim_b" 'BEGIN { printf "%.1f", a / b }')
echo "perf: speed-up (median sim A / median sim B) = ${ratio}x"
if [[ $bad_grants -ne 0 ]]; then echo "perf: FAIL: $bad_grants B runs without exactly $grants_b grants"; status=1; fi
if ! awk -v b="$sim_b" -v m="$max_sim" 'BEGIN { exit !(b <= m) }'; then
  echo "perf: FAIL: median B sim $sim_b s > $max_sim s"; status=1
fi
if ! awk -v r="$ratio" -v m="$min_ratio" 'BEGIN { exit !(r >= m) }'; then
  echo "perf: FAIL: speed-up ${ratio}x < ${min_ratio}x"; status=1
fi
if [[ $status -eq 0 ]]; then
  echo "perf: PASS (B: exactly $grants_b grants, median sim $sim_b s <= $max_sim s, ${ratio}x >= ${min_ratio}x)"
fi
exit $status
