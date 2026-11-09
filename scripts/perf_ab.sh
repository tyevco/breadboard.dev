#!/usr/bin/env bash
# ADR-003 A/B performance gate with the ADR's measurement protocol:
# interleaved A/B runs, N of each, medians of the simulation time (startup
# reported separately), user+sys CPU, grant counts and the load average.
# Unlike perf.sh, A and B differ by arbitrary bb flags (perf.sh varies only
# --max-quantum).
#
# Usage: scripts/perf_ab.sh <bb> <out_root> <n> <A flags> <B flags> \
#            <max_grants_b> <min_speedup> <max_slowdown> -- <bb run args...>
#   <A flags>, <B flags>  extra bb run flags of each case, one word list
#                         ("" for none)
#   <max_grants_b>        every B run takes at most this many grants (- = no check)
#   <min_speedup>         median(A sim) / median(B sim) >= this (- = no check)
#   <max_slowdown>        median(B sim) / median(A sim) <= this (- = no check)
#   <bb run args>         without --out
#
# Every B run must also match the first A run in nets.csv, nets.vcd and every
# non-grant event record (B may change only the grant partition), unless
# PERF_AB_EQUIV=0 (A and B are different models, e.g. the ADC board on
# Breadboard's ADC model against the stock one; ADR-005 step 6). Wall-clock
# numbers never reach the simulation outputs.
set -euo pipefail

bb=$1 root=$2 n=$3 fa=$4 fb=$5 max_grants=$6 min_speedup=$7 max_slowdown=$8
shift 8
[[ ${1:-} == -- ]] && shift

rm -rf "$root"
mkdir -p "$root"
results=$root/results.tsv
printf 'run\tcase\tgrants\ttotal_s\tstartup_s\tsim_s\tcpu_user_sys_s\tload1\n' > "$results"
echo "perf-ab: A = [${fa}], B = [${fb}], N = $n, interleaved"

run_one() { # <k> <case> <flags>
  local k=$1 c=$2 flags=$3 dir=$root/$2-$1 load cpu
  shift 3
  load=$(cut -d' ' -f1 /proc/loadavg)
  # user+sys CPU of the bb process and its children (Renode, ngspice).
  local TIMEFORMAT='%U %S'
  # shellcheck disable=SC2086
  { time "$bb" run "$@" $flags --out "$dir" > "$dir.log" 2>&1; } 2> "$dir.time" \
    || { echo "perf-ab: run $dir failed:"; cat "$dir.log"; exit 1; }
  cpu=$(awk '{printf "%.2f", $1 + $2}' "$dir.time")
  local grants total startup sim
  grants=$(sed -n 's/.* in \([0-9]*\) grants.*/\1/p' "$dir.log")
  total=$(sed -n 's/.*wall time \([0-9.]*\) s total.*/\1/p' "$dir.log")
  startup=$(sed -n 's/.*(\([0-9.]*\) s engine startup.*/\1/p' "$dir.log")
  sim=$(sed -n 's/.*, \([0-9.]*\) s simulation).*/\1/p' "$dir.log")
  grep -v '"type":"grant"' "$dir/events.jsonl" > "$dir/non-grant-records"
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$k" "$c" "$grants" "$total" "$startup" "$sim" "$cpu" "$load" >> "$results"
  echo "perf-ab: run $k $c: $grants grants, sim $sim s, startup $startup s, total $total s, cpu $cpu s, load $load"
}

for k in $(seq 1 "$n"); do
  run_one "$k" A "$fa" "$@"
  run_one "$k" B "$fb" "$@"
done

median() { # <case> <column>
  awk -F'\t' -v c="$1" -v col="$2" 'NR > 1 && $2 == c { print $col }' "$results" \
    | sort -g | awk '{ a[NR] = $1 } END { if (NR % 2) print a[(NR + 1) / 2]; else print (a[NR / 2] + a[NR / 2 + 1]) / 2 }'
}
column -t -s $'\t' "$results" | sed 's/^/perf-ab: /'
status=0
for c in A B; do
  echo "perf-ab: median $c: sim $(median $c 6) s, startup $(median $c 5) s, total $(median $c 4) s," \
       "cpu $(median $c 7) s, grants $(median $c 3), load $(median $c 8)"
done
for k in $(seq 1 "$n"); do
  [[ ${PERF_AB_EQUIV:-1} == 0 ]] && break
  for f in nets.csv nets.vcd non-grant-records; do
    cmp -s "$root/A-1/$f" "$root/B-$k/$f" \
      || { echo "perf-ab: FAIL: B-$k $f differs from A-1"; status=1; }
  done
done
[[ ${PERF_AB_EQUIV:-1} == 0 ]] && echo "perf-ab: A and B are different models (PERF_AB_EQUIV=0): outputs not compared"
sim_a=$(median A 6) sim_b=$(median B 6)
speedup=$(awk -v a="$sim_a" -v b="$sim_b" 'BEGIN { printf "%.2f", a / b }')
slowdown=$(awk -v a="$sim_a" -v b="$sim_b" 'BEGIN { printf "%.3f", b / a }')
echo "perf-ab: median sim A / median sim B = ${speedup}x (B / A = $slowdown)"
if [[ $max_grants != - ]]; then
  bad=$(awk -F'\t' -v g="$max_grants" 'NR > 1 && $2 == "B" && $3 > g' "$results" | wc -l)
  if [[ $bad -ne 0 ]]; then echo "perf-ab: FAIL: $bad B runs with more than $max_grants grants"; status=1; fi
fi
if [[ $min_speedup != - ]] && ! awk -v r="$speedup" -v m="$min_speedup" 'BEGIN { exit !(r >= m) }'; then
  echo "perf-ab: FAIL: speed-up ${speedup}x < ${min_speedup}x"; status=1
fi
if [[ $max_slowdown != - ]] && ! awk -v r="$slowdown" -v m="$max_slowdown" 'BEGIN { exit !(r <= m) }'; then
  echo "perf-ab: FAIL: B / A = $slowdown > $max_slowdown"; status=1
fi
if [[ $status -eq 0 ]]; then
  echo "perf-ab: PASS (B outputs identical to A except grant records;" \
       "grants <= $max_grants, speed-up ${speedup}x >= $min_speedup, B / A $slowdown <= $max_slowdown)"
fi
exit $status
