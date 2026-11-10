#!/usr/bin/env bash
# Phase 1 plan M5/M5b (ADR-006 D9, Amendment 3): the E1 packages' vectors and the LOCAL-ONLY TPS22917.
#
#   scripts/dms_packages.sh vectors <bb>    regenerate the committed vectors of packages/ti/tps22917 and
#                                           packages/worldsemi/ws2812c-2020 from the suite (make dms-vectors)
#   scripts/dms_packages.sh check <bb>      regenerate them into a scratch copy and require the committed
#                                           files byte for byte, and nordic/nrf52840-qiaa's GPIO sets to be
#                                           the suite's resistances (make dms-vectors-check)
#   scripts/dms_packages.sh m5b <bb>        plan M5b's gate against the local package (make dms-m5b)
#   scripts/dms_packages.sh local <bb>      build the LOCAL-ONLY vendor-derived ti/tps22917 in
#                                           $DMS_LOCAL_PACKAGES (default <repo>/local-packages, git-ignored),
#                                           qualify it (bb pkg test --record) and run gate G8 (make
#                                           dms-local-packages)
#
# Inputs come from a `git archive` export of the reference commit in $DMS_PKG_DIR (default
# ~/.cache/bb-dms-packages, off /mnt/c); the dial-me-spinner repository is only read (DMS_REPO). Needs
# ngspice 42 and the suite's Python packages (the uv venv of scripts/dms_suite.sh). Nothing TI-derived
# is ever written into the repository: the local package lives in a git-ignored path (approver
# decision A3), and this script refuses to write it anywhere git tracks.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
MODE=${1:?usage: dms_packages.sh vectors|check|local <bb>}
BB=${2:?usage: dms_packages.sh vectors|check|local <bb>}
BB=$(cd "$(dirname "$BB")" && pwd)/$(basename "$BB")
REPO=${DMS_REPO:-$ROOT/../dial-me-spinner}
COMMIT=6854d25e0fde9b5d60da07b2c2b6183fd36dfc5a
LIB=hardware/sim/models/load_switches__tps22917.lib
LIB_SHA256=b487750ff2434ed638e7bcb0f8d4d1ade73f7bb638b87354bf22873641af4e18
WORK=${DMS_PKG_DIR:-$HOME/.cache/bb-dms-packages}
LOCAL=${DMS_LOCAL_PACKAGES:-$ROOT/local-packages}
RESULTS=$ROOT/examples/dial-me-spinner/suite/results.json

NGV=$(ngspice -v 2>&1 | grep -o 'ngspice-[0-9][0-9.]*' | head -1)
[[ $NGV == ngspice-42 ]] || { echo "dms-packages: need ngspice 42, found '$NGV'" >&2; exit 1; }
[[ -d $REPO/.git || -f $REPO/.git ]] || { echo "dms-packages: no dial-me-spinner repository at $REPO (DMS_REPO)" >&2; exit 1; }

SRC=$WORK/src-${COMMIT:0:12}
if [[ ! -f $SRC/.complete ]]; then
  rm -rf "$SRC" && mkdir -p "$SRC"
  git -C "$REPO" archive "$COMMIT" | tar -x -C "$SRC"
  touch "$SRC/.complete"
fi
got=$(sha256sum < "$SRC/$LIB" | cut -d' ' -f1)
[[ $got == "$LIB_SHA256" ]] || { echo "dms-packages: $LIB sha256 $got, expected $LIB_SHA256" >&2; exit 1; }

REQ=$ROOT/examples/dial-me-spinner/suite/requirements.txt
VENV=${DMS_SUITE_CACHE:-$HOME/.cache/bb-dms-suite}/venv-$(sha256sum "$REQ" | cut -c1-12)
if [[ ! -x $VENV/bin/python ]]; then
  mkdir -p "$(dirname "$VENV")"
  uv venv -q --python python3.12 "$VENV.tmp"
  uv pip install -q --offline --python "$VENV.tmp/bin/python" -r "$REQ" 2>/dev/null \
    || uv pip install -q --python "$VENV.tmp/bin/python" -r "$REQ"
  mv "$VENV.tmp" "$VENV"
fi
PY=$VENV/bin/python
GEN=$ROOT/scripts/dms_packages.py

# Is a path tracked by (or not ignored by) git? The local package must never be either.
not_in_git() {
  local p=$1
  if git -C "$ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    case $(cd "$(dirname "$p")" 2>/dev/null && pwd -P)/ in
      "$(cd "$ROOT" && pwd -P)"/*)
        git -C "$ROOT" check-ignore -q "$p" || { echo "dms-packages: $p is inside the repository and not git-ignored" >&2; return 1; }
        [[ -z $(git -C "$ROOT" ls-files -- "$p") ]] || { echo "dms-packages: git tracks files under $p" >&2; return 1; } ;;
    esac
  fi
}

case $MODE in
vectors)
  "$PY" -I "$GEN" vectors --src "$SRC" --results "$RESULTS" --out "$ROOT"
  ;;
check)
  T=$WORK/check
  rm -rf "$T" && mkdir -p "$T/packages/ti" "$T/packages/worldsemi"
  cp -r "$ROOT/packages/ti/tps22917" "$T/packages/ti/"
  cp -r "$ROOT/packages/worldsemi/ws2812c-2020" "$T/packages/worldsemi/"
  "$PY" -I "$GEN" vectors --src "$SRC" --results "$RESULTS" --out "$T"
  bad=0
  for p in ti/tps22917 worldsemi/ws2812c-2020; do
    if ! diff -r "$ROOT/packages/$p/tests" "$T/packages/$p/tests" > "$T/diff.txt"; then
      head -40 "$T/diff.txt"; bad=1
    fi
  done
  "$PY" -I "$GEN" gpio --src "$SRC" --pins "$ROOT/packages/nordic/nrf52840-qiaa/pins.toml" || bad=1
  [[ $bad == 0 ]] || { echo "dms-vectors-check: FAIL (regenerated vectors or GPIO sets differ from the suite)"; exit 1; }
  echo "dms-vectors-check: PASS (the committed vectors are what the suite at ${COMMIT:0:7} gives)"
  ;;
local)
  mkdir -p "$LOCAL"
  not_in_git "$LOCAL"
  FIXED=$WORK/load_switches__tps22917.s1.lib
  "$PY" -I "$ROOT/scripts/dms_s1.py" fixview --in "$SRC/$LIB" --out "$FIXED" > "$WORK/fixview.json"
  G8=$WORK/g8-packages
  "$PY" -I "$GEN" local --src "$SRC" --results "$RESULTS" --fixed "$FIXED" --out "$LOCAL" --g8 "$G8" \
    | tee "$WORK/local.json"
  "$BB" pkg lint "$LOCAL/ti/tps22917"
  "$BB" pkg test "$LOCAL/ti/tps22917" --record --report "$WORK/report-local.jsonl"
  # Gate G8 (ADR-006): the GND -> GNDP rename gives byte-identical vector results (every measured value
  # and every sample, at both partitions).
  "$BB" pkg test "$G8/ti/tps22917" --report "$WORK/report-g8.jsonl" > "$WORK/g8.log" || true
  if [[ -s $WORK/report-g8.jsonl ]] && cmp -s "$WORK/report-local.jsonl" "$WORK/report-g8.jsonl"; then
    echo "dms-local-packages: G8 PASS: the GND -> GNDP rename gives byte-identical vector results ($(wc -l < "$WORK/report-g8.jsonl") vector runs)"
  else
    diff "$WORK/report-local.jsonl" "$WORK/report-g8.jsonl" | head -20
    echo "dms-local-packages: G8 FAIL"; exit 1
  fi
  echo "dms-local-packages: PASS (LOCAL-ONLY ti/tps22917 in $LOCAL/ti/tps22917, tree $("$BB" pkg tree "$LOCAL/ti/tps22917"))"
  ;;
m5b)
  # Plan M5b's gate: the committed datasheet-fitted view against the local vendor-derived one on the E1
  # checks (10 mV / 1 ms), both through NgspiceEngine at phase0. The decks hold the vendor view, so they
  # stay in $WORK (off the repository).
  VENDOR=$LOCAL/ti/tps22917
  [[ -f $VENDOR/part.toml ]] || { echo "dms-m5b: no local vendor-derived package at $VENDOR (run make dms-local-packages)" >&2; exit 1; }
  grep -q 'LicenseRef-local-only' "$VENDOR/part.toml" || { echo "dms-m5b: $VENDOR is not the local-only package" >&2; exit 1; }
  RUN=$WORK/m5b
  rm -rf "$RUN" && mkdir -p "$RUN/runs"
  "$PY" -I "$ROOT/scripts/dms_m5b.py" decks --src "$SRC" --out "$RUN/decks" \
    --view "vendor=$VENDOR/analog/load_switches__tps22917.lib:LSW_TPS22917" \
    --view "fit=$ROOT/packages/ti/tps22917/analog/lsw_fit.lib:TPS22917_LSW_FIT"
  export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$HOME/.cache/bb-target}
  (cd "$ROOT" && cargo build --release -q -p bb-analog-ngspice --example deck_run)
  DECK_RUN=$CARGO_TARGET_DIR/release/examples/deck_run
  OPTS=$("$PY" -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["options"])' "$RUN/decks/manifest.json")
  "$PY" -I - "$RUN/decks/manifest.json" > "$RUN/cases.txt" <<'PYEOF'
import json, sys
for c in json.load(open(sys.argv[1]))["cases"]:
    a = [c["name"], "--stop", f"{c['stop']}fs", "--voh", f"{c['voh']:g}"]
    for n, t, lv in c["edges"]:
        a += ["--edge", f"{n}@{t}fs={lv}"]
    for p in c["probes"]:
        a += ["--probe", p]
    print(" ".join(a))
PYEOF
  run_case() {
    local name=$1; shift
    timeout "${DMS_M5B_CAP:-300}" "$DECK_RUN" --body "$RUN/decks/$name.cir" --options "$OPTS" --lattice 10us \
      --grant 1ms "$@" --csv "$RUN/runs/$name.csv" > "$RUN/runs/$name.log" 2>&1 || true
    local js; js=$(tail -n 1 "$RUN/runs/$name.log")
    [[ $js == '{'* ]] || js='{"status":"no summary (killed or failed)","reached_fs":0,"wall_s":0}'
    echo "{\"id\":\"$name\",${js#\{}"
  }
  export -f run_case
  export RUN DECK_RUN OPTS
  echo "dms-m5b: $(wc -l < "$RUN/cases.txt") runs on ${DMS_M5B_JOBS:-8} jobs (load $(cut -d' ' -f1-3 /proc/loadavg))"
  xargs -P "${DMS_M5B_JOBS:-8}" -L 1 bash -c 'run_case "$@"' _ < "$RUN/cases.txt" > "$RUN/runs.jsonl"
  mkdir -p "$ROOT/out/dms-m5b"
  "$PY" -I "$ROOT/scripts/dms_m5b.py" gate --dir "$RUN" --summary "$ROOT/out/dms-m5b/summary.json"
  ;;
*)
  echo "usage: dms_packages.sh vectors|check|local|m5b <bb>" >&2; exit 2 ;;
esac
