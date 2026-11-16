#!/usr/bin/env bash
# Phase 2 plan P0 (numerics and feasibility spike, ADR-009 D2, gate G3): the W1 and W2 suite replicas
# and the first RT9080, BQ25180 and TPS61040 `pi` views in static-load mode, through NgspiceEngine
# (deck_run), at Breadboard's option sets and the suite's; W1 at its labelled stress corner and the
# suite's 50 mA margin check (decision D-W1, ADR-009 Amendment 2).
#
#   scripts/pi_spike.sh [gate]          the P0 gate (make pi-spike); summary in $OUT/summary.json
#   scripts/pi_spike.sh runs <id>...    run single configurations (for exploration), results on stdout
#
# Run ids: <case>.<variant>.<option set>.<g<grant> | tmax1us> (cases, variants and option sets in
# scripts/pi_spike.py). Inputs come from a `git archive` export of the reference commit in $WORK
# (default $BB_SCRATCH/bb-pi-spike, off /mnt/c): the dial-me-spinner repository is only read. Every
# deck is LOCAL-ONLY (the replicas hold the suite's TI-derived TPS22917; every deck holds the suite's
# netlist extraction): they stay in $WORK, never in this repository. The `pi` views are this
# repository's own (examples/dial-me-spinner/pi-spike/views). Needs ngspice 42 and the suite's Python
# packages (the uv venv of scripts/dms_suite.sh).
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
REPO=${DMS_REPO:-$ROOT/../dial-me-spinner}
COMMIT=6854d25e0fde9b5d60da07b2c2b6183fd36dfc5a
LIB=hardware/sim/models/load_switches__tps22917.lib
LIB_SHA256=b487750ff2434ed638e7bcb0f8d4d1ade73f7bb638b87354bf22873641af4e18
WORK=${PI_SPIKE_DIR:-${BB_SCRATCH:-$HOME/.cache}/bb-pi-spike}
OUT=${PI_SPIKE_OUT:-$ROOT/out/pi-spike}
JOBS=${PI_SPIKE_JOBS:-8}
MODE=${1:-gate}
VIEWS=$ROOT/examples/dial-me-spinner/pi-spike/views
TOL=$ROOT/examples/dial-me-spinner/pi-spike/tolerances.toml

NGV=$(ngspice -v 2>&1 | grep -o 'ngspice-[0-9][0-9.]*' | head -1)
[[ $NGV == ngspice-42 ]] || { echo "pi-spike: need ngspice 42, found '$NGV'" >&2; exit 1; }
[[ -d $REPO/.git || -f $REPO/.git ]] || { echo "pi-spike: no dial-me-spinner repository at $REPO (DMS_REPO)" >&2; exit 1; }

SRC=$WORK/src-${COMMIT:0:12}
if [[ ! -f $SRC/.complete ]]; then
  rm -rf "$SRC" && mkdir -p "$SRC"
  git -C "$REPO" archive "$COMMIT" | tar -x -C "$SRC"
  touch "$SRC/.complete"
fi
got=$(sha256sum < "$SRC/$LIB" | cut -d' ' -f1)
[[ $got == "$LIB_SHA256" ]] || { echo "pi-spike: $LIB sha256 $got, expected $LIB_SHA256" >&2; exit 1; }

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
P0=$ROOT/scripts/pi_spike.py

# ADR-009 D2's rules on the spike's views (no branch-current unknown, two-argument min/max)
"$PY" -I "$P0" lint "$VIEWS"/*.lib

# The S1-fixed TPS22917 (local-only), as scripts/dms_s1.sh builds it
FIXED=$WORK/local-packages/ti/tps22917/load_switches__tps22917.s1.lib
mkdir -p "$(dirname "$FIXED")"
"$PY" -I "$ROOT/scripts/dms_s1.py" fixview --in "$SRC/$LIB" --out "$FIXED" > /dev/null

RUN=$WORK/run
DECKS=$RUN/decks
if [[ $MODE == gate ]]; then rm -rf "$RUN"; fi
mkdir -p "$RUN/runs" "$DECKS"
"$PY" -I "$P0" decks --src "$SRC" --views "$VIEWS" --packages "$ROOT/packages" --lib-fixed "$FIXED" --out "$DECKS"

export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$HOME/.cache/bb-target}
(cd "$ROOT" && cargo build --release -q -p bb-analog-ngspice --example deck_run)
DECK_RUN=$CARGO_TARGET_DIR/release/examples/deck_run

# deck_run arguments of <case> <variant> from the manifest
case_args() {
  "$PY" -I - "$DECKS/manifest.json" "$1" "$2" <<'EOF'
import json, sys
c = next(c for c in json.load(open(sys.argv[1]))["cases"] if c["name"] == sys.argv[2])
v = c["variants"][sys.argv[3]]
n, t, lv = v["edge"]
a = ["--body", v["body"], "--stop", f"{c['stop']}fs", "--voh", f"{v['voh']:g}", "--edge", f"{n}@{t}fs={lv}"]
for p in c["probes"]:
    a += ["--probe", p]
print(" ".join(a))
EOF
}
opts() { "$PY" -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["option_sets"][sys.argv[2]])' "$DECKS/manifest.json" "$1"; }

run_one() {
  local id=$1 case variant os spec
  IFS=. read -r case variant os spec <<< "$id"
  local csv=$RUN/runs/$id.csv log=$RUN/runs/$id.log cap=${PI_SPIKE_CAP:-300} extra=()
  case $spec in
    g*) extra=(--grant "${spec#g}") ;;
    tmax1us) extra=(--grant 1ms --tmax 1us) ;;
    *) echo "{\"id\":\"$id\",\"status\":\"bad run id\"}"; return ;;
  esac
  local args; args=$(case_args "$case" "$variant")
  # shellcheck disable=SC2086
  (cd "$DECKS" && timeout "$cap" "$DECK_RUN" $args --options "$(opts "$os")" --lattice 10us "${extra[@]}" \
    --csv "$csv") > "$log" 2>&1 || true
  local js; js=$(tail -n 1 "$log")
  if [[ $js != '{'* ]]; then
    local why; why=$(tail -n 1 "$log" | tr -d '"\\' | cut -c1-200)
    js="{\"status\":\"no summary (killed after ${cap} s, or failed: ${why})\",\"reached_fs\":0,\"wall_s\":$cap}"
  fi
  echo "{\"id\":\"$id\",${js#\{}"
}
export -f run_one case_args opts
export RUN DECKS PY DECK_RUN

if [[ $MODE == runs ]]; then
  shift
  printf '%s\n' "$@" | xargs -P "$JOBS" -I{} bash -c 'run_one "$@"' _ {} | tee -a "$RUN/runs.jsonl"
  exit 0
fi

if [[ $MODE == gate ]]; then
  Q=$("$PY" -I -c 'import tomllib,sys; print(tomllib.load(open(sys.argv[1],"rb"))["option_set"]["name"])' "$TOL")
  RO=$("$PY" -I -c 'import tomllib,sys; t=tomllib.load(open(sys.argv[1],"rb"))["replica"]; print(t["variant"], t["options"])' "$TOL")
  read -r RV ROS <<< "$RO"
  ids=()
  for c in w1 w2rule w2old; do
    # the replica, the static-load deck (grant partitions, converged reference), the attribution
    # variants, the other option sets, and the suite's unfixed view on the suite's fallback ladder
    ids+=("$c.$RV.$ROS.g1ms" "$c.pi.$Q.g1ms" "$c.pi.$Q.g10us" "$c.pi.$Q.g10ms" "$c.pi.$Q.tmax1us")
    for v in pi_typ pi_no3v3c pi_c3half suite_lswfit pi_lswfix; do ids+=("$c.$v.$Q.g1ms"); done
    for os in phase0 phase0_gear suite_fallback suite_l1; do
      [[ $os == "$Q" ]] || ids+=("$c.pi.$os.g1ms")
    done
    for os in suite_default suite_l1 suite_fallback; do ids+=("$c.suite.$os.g1ms"); done
  done
  for c in w1_l30 w1_l40 w1_l60; do ids+=("$c.pi.$Q.g1ms" "$c.$RV.$ROS.g1ms"); done
  for ma in 51 52 53 54 55 56 57 58 59 60; do ids+=("w1_l$ma.pi.$Q.g1ms" "w1_l$ma.pi_typ.$Q.g1ms" "w1_l$ma.$RV.$ROS.g1ms"); done
  # W1's stress corner (decision D-W1, tolerances.toml [w1]): the option set qualified on it as well
  S=$("$PY" -I -c 'import tomllib,sys; print(tomllib.load(open(sys.argv[1],"rb"))["w1"]["stress_load_mA"])' "$TOL")
  ids+=("w1_l$S.pi.$Q.g10us" "w1_l$S.pi.$Q.g10ms" "w1_l$S.pi.$Q.tmax1us")
  mapfile -t ids < <(printf '%s\n' "${ids[@]}" | sort -u)
  echo "pi-spike: $COMMIT ($NGV), ${#ids[@]} runs on $JOBS jobs in $RUN (load $(cut -d' ' -f1-3 /proc/loadavg))"
  start=$(date +%s)
  printf '%s\n' "${ids[@]}" | xargs -P "$JOBS" -I{} bash -c 'run_one "$@"' _ {} > "$RUN/runs.jsonl"
  echo "pi-spike: runs done in $(( $(date +%s) - start )) s (load $(cut -d' ' -f1-3 /proc/loadavg))"
  mkdir -p "$OUT"
  set +e
  "$PY" -I "$P0" gate --dir "$RUN" --summary "$OUT/summary.json" --tolerances "$TOL" \
    --suite-results "$ROOT/examples/dial-me-spinner/suite/results.json"
  rc=$?
  set -e
  exit $rc
fi
echo "usage: pi_spike.sh [gate | runs <id>...]" >&2
exit 2
