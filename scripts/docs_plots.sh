#!/usr/bin/env bash
# The docs site's waveform plots (site/public/plots), from real runs: `make docs-plots`.
#
#   bash scripts/docs_plots.sh <bb> <out> <svgdir>
#
# Runs, in parallel, every Renode fresh (BB_RENODE_POOL is ignored, as in make demo):
#   <out>/demo     scripts/demo.sh (make demo): E1 on the cold bench, led_rail.elf and
#                  led_rail_wrong_order.elf, with its checks and the committed one-cell hashes
#   <out>/vsense   scripts/dms_vsense.sh run: the E2 cases sag, rec, s50 and s50-none at bb's defaults
#                  (the arguments of make dms-vsense; project copies in $DOCS_PLOTS_WORK/vsense)
#   <out>/spike    the Phase 0 spike with make spike's arguments (SPIKE_ARGS, passed in the environment)
# then scripts/docs_plots.py <out> <svgdir>, which writes <name>.light.svg and <name>.dark.svg.
# The figures are a pure function of the run outputs, so `make docs-plots-check` regenerates them into
# <out>/svg and compares them byte for byte with the committed site/public/plots.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
BB=$(realpath "$1")
OUT=$2
SVG=$3
PY=${PYTHON:-python3}
WORK=${DOCS_PLOTS_WORK:-$HOME/.cache/bb-docs-plots}
unset BB_RENODE_POOL
[[ -n ${SPIKE_ARGS:-} ]] || { echo "docs-plots: SPIKE_ARGS (the bb run arguments of make spike) not set; run make docs-plots" >&2; exit 2; }

rm -rf "$OUT"
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
cd "$ROOT"

DEMO_OUT=$OUT/demo DEMO_WORK=$WORK/demo bash scripts/demo.sh "$BB" > "$OUT/demo.log" 2>&1 &
demo=$!
DMS_VSENSE_WORK=$WORK/vsense bash scripts/dms_vsense.sh run "$BB" "$OUT/vsense" sag rec s50 s50-none \
  > "$OUT/vsense.log" 2>&1 &
vsense=$!
# shellcheck disable=SC2086
"$BB" run $SPIKE_ARGS --out "$OUT/spike" > "$OUT/spike.log" 2>&1 &
spike=$!

status=0
for job in demo:$demo vsense:$vsense spike:$spike; do
  if wait "${job#*:}"; then
    echo "docs-plots: ${job%%:*} done"
  else
    echo "docs-plots: FAIL: ${job%%:*} (log $OUT/${job%%:*}.log)"; tail -15 "$OUT/${job%%:*}.log"; status=1
  fi
done
[[ $status == 0 ]] || exit 1
"$PY" scripts/check_spike.py "$OUT/spike" > /dev/null
"$PY" scripts/docs_plots.py "$OUT" "$SVG"
