#!/usr/bin/env bash
# ADR-005 D6 startup gate for Breadboard's plugin assembly on the ADC board.
#
#   plugin_startup.sh <bb> <out_root> <n> <max_warm_s> <max_cold_s> -- <bb run args...>
#
# Interleaved rounds, N of each, fresh Renode processes only (no pool):
#   stock  the board with $STOCK_FLAGS (default --adc-model stock: the stock
#          STM32F072 plus the lookahead plugin), warm caches
#   warm   the board on Breadboard's STM32F072 (the overlay, BbStm32F0Adc,
#          BbStm32Timer), warm caches
#   cold   the same, with an empty plugin cache: the run compiles the
#          plugin assembly (the JIT profile cache is copied in, so only the
#          compile differs)
# Gates on the medians of `bb run`'s engine startup: warm - stock <=
# <max_warm_s> and cold - warm <= <max_cold_s>. Wall-clock: run it on an
# otherwise idle host.
set -euo pipefail

bb=$1 root=$2 n=$3 max_warm=$4 max_cold=$5
shift 5
[[ ${1:-} == -- ]] && shift

rm -rf "$root"
mkdir -p "$root"
# Absolute: BB_CACHE_DIR reaches Renode, whose working directory differs.
root=$(cd "$root" && pwd)
base=${BB_CACHE_DIR:-${XDG_CACHE_HOME:+$XDG_CACHE_HOME/bb}}
base=${base:-$HOME/.cache/bb}
results=$root/results.tsv
printf 'run\tcase\tstartup_s\tload1\n' > "$results"

startup() { sed -n 's/.*(\([0-9.]*\) s engine startup.*/\1/p' "$1"; }

run_case() { # <k> <case>
  local k=$1 c=$2 dir=$root/$2-$1 cache=$base load
  shift 2
  local extra=()
  case $c in
    stock) read -r -a extra <<< "${STOCK_FLAGS:---adc-model stock}" ;;
    cold)
      cache=$root/cache-$k
      mkdir -p "$cache"
      [[ -d $base/renode-jit ]] && cp -r "$base/renode-jit" "$cache/"
      ;;
  esac
  load=$(cut -d' ' -f1 /proc/loadavg)
  BB_CACHE_DIR=$cache env -u BB_RENODE_POOL "$bb" run "$@" "${extra[@]}" --out "$dir" > "$dir.log" 2>&1 \
    || { echo "plugin-startup: run $dir failed:"; cat "$dir.log"; exit 1; }
  if [[ $c == cold ]] && ! ls "$cache/renode-plugins"/bb_plugins-*.dll > /dev/null 2>&1; then
    echo "plugin-startup: FAIL: cold run $k did not compile the plugin assembly"; exit 1
  fi
  printf '%s\t%s\t%s\t%s\n' "$k" "$c" "$(startup "$dir.log")" "$load" >> "$results"
  echo "plugin-startup: run $k $c: startup $(startup "$dir.log") s, load $load"
}

# One warm-up run fills the caches the warm rounds rely on.
BB_CACHE_DIR=$base env -u BB_RENODE_POOL "$bb" run "$@" --out "$root/warmup" > "$root/warmup.log" 2>&1
# shellcheck disable=SC2086
BB_CACHE_DIR=$base env -u BB_RENODE_POOL "$bb" run "$@" ${STOCK_FLAGS:---adc-model stock} --out "$root/warmup-stock" \
  > "$root/warmup-stock.log" 2>&1
for k in $(seq 1 "$n"); do
  run_case "$k" stock "$@"
  run_case "$k" warm "$@"
  run_case "$k" cold "$@"
done

median() {
  awk -F'\t' -v c="$1" 'NR > 1 && $2 == c { print $3 }' "$results" \
    | sort -g | awk '{ a[NR] = $1 } END { if (NR % 2) print a[(NR + 1) / 2]; else print (a[NR / 2] + a[NR / 2 + 1]) / 2 }'
}
s=$(median stock) w=$(median warm) c=$(median cold)
dw=$(awk -v a="$w" -v b="$s" 'BEGIN { printf "%.3f", a - b }')
dc=$(awk -v a="$c" -v b="$w" 'BEGIN { printf "%.3f", a - b }')
echo "plugin-startup: medians (N = $n): stock $s s, warm $w s, cold $c s; warm - stock $dw s, cold - warm $dc s"
status=0
awk -v d="$dw" -v m="$max_warm" 'BEGIN { exit !(d <= m) }' \
  || { echo "plugin-startup: FAIL: warm - stock $dw s > $max_warm s"; status=1; }
awk -v d="$dc" -v m="$max_cold" 'BEGIN { exit !(d <= m) }' \
  || { echo "plugin-startup: FAIL: cold - warm $dc s > $max_cold s"; status=1; }
[[ $status -eq 0 ]] && echo "plugin-startup: PASS (warm - stock <= $max_warm s, cold - warm <= $max_cold s)"
exit $status
