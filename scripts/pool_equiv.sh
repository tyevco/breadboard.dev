#!/usr/bin/env bash
# ADR-004 G4: pool equivalence. Every configuration runs once fresh, then
# <rounds> times on the pool in BB_RENODE_POOL (run this under
# `bb pool run --slots 1`, so one process serves every lease), in
# alternating order (forward, then reversed, ...), so boards and platforms
# follow each other in one Renode process. Each pooled run must have been
# leased, must be byte-identical to the fresh run of its configuration in
# nets.csv, nets.vcd, events.jsonl and the non-grant records, and both must
# match the committed hashes.
#
# Usage: scripts/pool_equiv.sh <bb> <out_root> <rounds> -- <config>...
#   <config>  "<name>;<hash file>;<all|nongrant>;<bb run args>"
#             all: every line of the hash file; nongrant: all but events.jsonl
set -euo pipefail

bb=$1 root=$2 rounds=$3
shift 3
[[ ${1:-} == -- ]] && shift
[[ -n ${BB_RENODE_POOL:-} && -S $BB_RENODE_POOL ]] \
  || { echo "pool-equiv: FAIL: no pool (run under bb pool run)"; exit 1; }
rm -rf "$root"
mkdir -p "$root"
repo=$PWD
names=() hashes=() kinds=() argv=()
for c in "$@"; do
  IFS=';' read -r n h k a <<< "$c"
  names+=("$n") hashes+=("$h") kinds+=("$k") argv+=("$a")
done
status=0

run_one() { # <i> <dir> <pooled 0|1>
  local i=$1 dir=$2 pooled=$3
  local -a env_cmd=(env)
  [[ $pooled == 0 ]] && env_cmd+=(-u BB_RENODE_POOL)
  # shellcheck disable=SC2086
  "${env_cmd[@]}" "$bb" run ${argv[$i]} --out "$dir" > "$dir.log" 2>&1 \
    || { echo "pool-equiv: run $dir failed:"; cat "$dir.log"; exit 1; }
  grep -v '"type":"grant"' "$dir/events.jsonl" > "$dir/non-grant-records"
  local hash_lines
  if [[ ${kinds[$i]} == nongrant ]]; then
    hash_lines=$(grep -v ' events.jsonl$' "$repo/${hashes[$i]}")
  else
    hash_lines=$(cat "$repo/${hashes[$i]}")
  fi
  if ! (cd "$dir" && sha256sum --quiet -c <<< "$hash_lines"); then
    echo "pool-equiv: FAIL: $dir does not match ${hashes[$i]}"; status=1
  fi
  local how
  how=$(sed -n 's/^bb run: renode: //p' "$dir.log")
  if [[ $pooled == 1 && $how != leased* ]]; then
    echo "pool-equiv: FAIL: $dir was not leased ($how)"; status=1
  fi
  echo "pool-equiv: ${names[$i]} $(basename "$(dirname "$dir")"): $how;" \
       "$(sed -n 's/^bb run: wall time //p' "$dir.log")"
}

n=${#names[@]}
mkdir -p "$root/fresh"
for i in $(seq 0 $((n - 1))); do
  run_one "$i" "$root/fresh/${names[$i]}" 0
done
pids=()
for r in $(seq 1 "$rounds"); do
  mkdir -p "$root/round-$r"
  if (( r % 2 )); then order=$(seq 0 $((n - 1))); else order=$(seq $((n - 1)) -1 0); fi
  for i in $order; do
    d=$root/round-$r/${names[$i]}
    run_one "$i" "$d" 1
    pids+=("$(sed -n 's/.*leased from the pool (pid \([0-9]*\)).*/\1/p' "$d.log")")
    for f in nets.csv nets.vcd events.jsonl non-grant-records; do
      cmp -s "$root/fresh/${names[$i]}/$f" "$d/$f" \
        || { echo "pool-equiv: FAIL: $d/$f differs from the fresh run"; status=1; }
    done
  done
done
distinct=$(printf '%s\n' "${pids[@]}" | sort -u | grep -c . || true)
echo "pool-equiv: $((n * rounds)) pooled runs served by $distinct Renode process(es): ${pids[*]}"
if [[ $status -eq 0 ]]; then
  echo "pool-equiv: PASS ($n configurations, $rounds rounds in alternating order: every pooled" \
       "run leased and byte-identical to its fresh run and to the committed hashes)"
fi
exit $status
