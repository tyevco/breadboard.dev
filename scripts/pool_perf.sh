#!/usr/bin/env bash
# ADR-004 G2: engine startup on a warm pool. Run under `bb pool run
# --slots 1` (after its pre-warm). The first lease must start in at most
# <first_max> s; then N rounds of every board, interleaved: each board's
# median engine startup at most <max> s, and the first board's (the spike)
# median total wall at most <wall_max> s. Every run must be leased.
#
# Usage: scripts/pool_perf.sh <bb> <out_root> <n> <first_max> <max> <wall_max> -- <board>...
#   <board>  "<name>;<bb run args>"
set -euo pipefail

bb=$1 root=$2 n=$3 first_max=$4 max=$5 wall_max=$6
shift 6
[[ ${1:-} == -- ]] && shift
[[ -n ${BB_RENODE_POOL:-} && -S $BB_RENODE_POOL ]] \
  || { echo "pool-perf: FAIL: no pool (run under bb pool run)"; exit 1; }
rm -rf "$root"
mkdir -p "$root"
results=$root/results.tsv
printf 'run\tboard\tstartup_s\tsim_s\ttotal_s\tload1\n' > "$results"
status=0
names=() argv=()
for b in "$@"; do
  IFS=';' read -r nm a <<< "$b"
  names+=("$nm") argv+=("$a")
done

run_one() { # <k> <i>
  local k=$1 i=$2 dir=$root/${names[$2]}-$1 load
  load=$(cut -d' ' -f1 /proc/loadavg)
  # shellcheck disable=SC2086
  "$bb" run ${argv[$i]} --out "$dir" > "$dir.log" 2>&1 \
    || { echo "pool-perf: run $dir failed:"; cat "$dir.log"; exit 1; }
  grep -q '^bb run: renode: leased' "$dir.log" \
    || { echo "pool-perf: FAIL: $dir was not leased"; status=1; }
  local total startup sim
  total=$(sed -n 's/.*wall time \([0-9.]*\) s total.*/\1/p' "$dir.log")
  startup=$(sed -n 's/.*(\([0-9.]*\) s engine startup.*/\1/p' "$dir.log")
  sim=$(sed -n 's/.*, \([0-9.]*\) s simulation).*/\1/p' "$dir.log")
  printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$k" "${names[$i]}" "$startup" "$sim" "$total" "$load" >> "$results"
  echo "pool-perf: run $k ${names[$i]}: startup $startup s, sim $sim s, total $total s, load $load"
}

run_one first 0
first=$(awk -F'\t' '$1 == "first" { print $3 }' "$results")
for k in $(seq 1 "$n"); do
  for i in $(seq 0 $((${#names[@]} - 1))); do
    run_one "$k" "$i"
  done
done
median() { # <board> <column>
  awk -F'\t' -v b="$1" -v col="$2" 'NR > 1 && $1 != "first" && $2 == b { print $col }' "$results" \
    | sort -g | awk '{ a[NR] = $1 } END { if (NR % 2) print a[(NR + 1) / 2]; else print (a[NR / 2] + a[NR / 2 + 1]) / 2 }'
}
le() { awk -v a="$1" -v b="$2" 'BEGIN { exit !(a <= b) }'; }
echo "pool-perf: first lease: startup $first s (<= $first_max)"
le "$first" "$first_max" || { echo "pool-perf: FAIL: first lease $first s > $first_max s"; status=1; }
for nm in "${names[@]}"; do
  s=$(median "$nm" 3)
  echo "pool-perf: median $nm: startup $s s, sim $(median "$nm" 4) s, total $(median "$nm" 5) s, load $(median "$nm" 6)"
  le "$s" "$max" || { echo "pool-perf: FAIL: $nm median startup $s s > $max s"; status=1; }
done
w=$(median "${names[0]}" 5)
le "$w" "$wall_max" || { echo "pool-perf: FAIL: ${names[0]} median wall $w s > $wall_max s"; status=1; }
if [[ $status -eq 0 ]]; then
  echo "pool-perf: PASS (first lease $first s <= $first_max s; median startups <= $max s;" \
       "${names[0]} wall $w s <= $wall_max s; every run leased)"
fi
exit $status
