#!/usr/bin/env bash
# Phase 1 plan M6, the local E1 cross-check (plan section 2 E1, R2): the run's pin_edge records replayed
# as the PWL sources of the dial-me-spinner suite's own led_rail_sequence deck, with the power path
# replaced by the same bench supply (scripts/dms_suite_deck.py); VLED from that deck and from the
# co-simulation must agree within 5 mV at every 10 us sample.
#
# The gate compares like with like: the co-simulation with the LOCAL-ONLY vendor-derived TPS22917
# (local-packages/, `make dms-local-packages`, on the project's package search path) against the suite
# deck with the same view. The committed datasheet-fitted view's runs (examples/dial-me-spinner as
# committed) are compared too, reported; and the bench deck against the suite's own battery-network deck
# at the same corner is reported, not gated (it measures what the bench leaves out).
#
# Runs locally only: it needs the dial-me-spinner repository (DMS_REPO; read with git archive) and the
# local package. Everything it writes is outside the repository ($DMS_XC_DIR, ~/.cache/bb-dms-crosscheck)
# except the summary out/dms-crosscheck/summary.jsonl, which holds numbers only.
#
# usage: scripts/dms_crosscheck.sh <bb>
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
BB=$(realpath "${1:?usage: dms_crosscheck.sh <bb>}")
REPO=${DMS_REPO:-$ROOT/../dial-me-spinner}
COMMIT=6854d25e0fde9b5d60da07b2c2b6183fd36dfc5a
LOCAL=${DMS_LOCAL_PACKAGES:-$ROOT/local-packages}
XC=${DMS_XC_DIR:-$HOME/.cache/bb-dms-crosscheck}
PROJ=$ROOT/examples/dial-me-spinner
LIMIT_MV=${DMS_XC_LIMIT_MV:-5}
SUM=$ROOT/out/dms-crosscheck/summary.jsonl

[[ -d $REPO/.git || -f $REPO/.git ]] || { echo "dms-crosscheck: no dial-me-spinner repository at $REPO (DMS_REPO)" >&2; exit 1; }
VENDOR=$LOCAL/ti/tps22917
grep -q 'LicenseRef-local-only' "$VENDOR/part.toml" 2>/dev/null \
  || { echo "dms-crosscheck: no LOCAL-ONLY ti/tps22917 at $VENDOR (make dms-local-packages)" >&2; exit 1; }
SRC=${DMS_PKG_DIR:-$HOME/.cache/bb-dms-packages}/src-${COMMIT:0:12}
if [[ ! -f $SRC/.complete ]]; then
  rm -rf "$SRC" && mkdir -p "$SRC"
  git -C "$REPO" archive "$COMMIT" | tar -x -C "$SRC"
  touch "$SRC/.complete"
fi
REQ=$PROJ/suite/requirements.txt
PY=${DMS_SUITE_CACHE:-$HOME/.cache/bb-dms-suite}/venv-$(sha256sum "$REQ" | cut -c1-12)/bin/python
[[ -x $PY ]] || { echo "dms-crosscheck: no suite venv ($PY; make dms-local-packages makes it)" >&2; exit 1; }
GEN=$ROOT/scripts/dms_suite_deck.py

rm -rf "$XC" && mkdir -p "$XC" "$(dirname "$SUM")"
: > "$SUM"
status=0

# The co-simulation runs: the committed project (fitted view) and a copy with the local package first
# on its search path (vendor-derived view; bb open rewrites that copy's board.bbm and bb.lock).
DMS_RAIL_WORK=$XC/work bash "$ROOT/scripts/dms_rail.sh" prepare "$BB"
cp -r "$XC/work/proj-ok" "$XC/work/proj-ok-local"
sed -i "s|^\[\[board\]\]|[packages]\npaths = [\"$LOCAL\"]\n\n[[board]]|" "$XC/work/proj-ok-local/bb.toml"
"$BB" open --update-lock "$XC/work/proj-ok-local" > "$XC/open-local.log" 2>&1 \
  || { cat "$XC/open-local.log"; exit 1; }
for view in fit vendor; do
  p=$XC/work/proj-ok
  [[ $view == fit ]] || p=$XC/work/proj-ok-local
  for bench in nominal cold; do
    o=$XC/cosim-$view-$bench
    "$BB" run --project "$p" --bench "$p/bench/$bench.toml" --for 120ms --out "$o" > "$o.log" 2>&1 \
      || { echo "dms-crosscheck: FAIL: co-simulation $view $bench"; tail -5 "$o.log"; exit 1; }
    grep -q '"view":"lsw"' "$o/run.json"
    lic=$("$PY" -I -c 'import json,sys
r=json.load(open(sys.argv[1]))
print([x["provenance"].get("source") for x in r["analog"] if x["ref"]=="main:U2"][0])' "$o/run.json")
    echo "dms-crosscheck: co-simulation $view $bench: U2 provenance $lic, $(sed -n 's/^bb run: wall time //p' "$o.log")"
  done
done

declare -A VIEW=(
  [vendor]="$VENDOR/analog/load_switches__tps22917.lib:LSW_TPS22917"
  [fit]="$ROOT/packages/ti/tps22917/analog/lsw_fit.lib:TPS22917_LSW_FIT"
)
replay() { # <name> <view> <bench> [--battery] [--options x]
  local name=$1 view=$2 bench=$3
  shift 3
  "$PY" -I "$GEN" deck --src "$SRC" --bench "$PROJ/bench/$bench.toml" --events "$XC/cosim-$view-$bench/events.jsonl" \
    --view "${VIEW[$view]}" "$@" --out "$XC/$name.cir" > /dev/null
  "$PY" -I "$GEN" run --deck "$XC/$name.cir" --out "$XC/$name.csv" | tee -a "$XC/runs.jsonl"
}
for bench in nominal cold; do
  for view in vendor fit; do
    replay "replay-$view-$bench" "$view" "$bench" || { status=1; continue; }
    gate=()
    if [[ $view == vendor ]]; then gate=(--gate); fi
    if r=$("$PY" -I "$GEN" compare --replay "$XC/replay-$view-$bench.csv" --run "$XC/cosim-$view-$bench/nets.csv" \
        --limit-mv "$LIMIT_MV" "${gate[@]}" --label "$view $bench: co-simulation vs the suite deck replay"); then :
    else status=1; fi
    echo "$r" >> "$SUM"
    echo "dms-crosscheck: $([[ $view == vendor ]] && { [[ $r == *'"pass": true'* ]] && echo PASS || echo FAIL; } \
      || echo REPORT): $r"
  done
  # what the bench leaves out: the same replay with the suite's own battery network (vendor view)
  if replay "battery-vendor-$bench" vendor "$bench" --battery; then
    r=$("$PY" -I "$GEN" compare --replay "$XC/battery-vendor-$bench.csv" --run "$XC/replay-vendor-$bench.csv" \
      --label "vendor $bench: the suite battery-network deck vs the bench deck (reported)")
    echo "$r" >> "$SUM"
    echo "dms-crosscheck: REPORT: $r"
  else
    echo "dms-crosscheck: REPORT: the battery-network deck at $bench did not run at phase0"
  fi
done
if [[ $status == 0 ]]; then echo "dms-crosscheck: PASS (summary $SUM)"; else echo "dms-crosscheck: FAIL"; fi
exit $status
