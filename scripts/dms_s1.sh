#!/usr/bin/env bash
# Phase 1 plan S1 (numerics spike): the E1 deck through NgspiceEngine at Phase 0's option set,
# with the S1 view-local fix of the TPS22917 model (ADR-006 Amendment 1).
#
#   scripts/dms_s1.sh [gate]            the S1 gate (make dms-s1); summary in $OUT/summary.json
#   scripts/dms_s1.sh cost [N]          the S1 cost table (make dms-s1-cost), N interleaved rounds (default 5)
#
# Inputs come from a `git archive` export of the reference commit (plan approver decision C) in
# $DMS_S1_DIR (default ~/.cache/bb-dms-s1, off /mnt/c): the dial-me-spinner repository is only read.
# Everything derived from the suite's TPS22917 model (the fixed view, every deck) stays in
# $DMS_S1_DIR: it is TI-derived and LOCAL-ONLY (plan approver decision A3), never in this repository.
# Needs ngspice 42 and the suite's Python packages (the uv venv of scripts/dms_suite.sh).
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
REPO=${DMS_REPO:-$ROOT/../dial-me-spinner}
COMMIT=6854d25e0fde9b5d60da07b2c2b6183fd36dfc5a
LIB=hardware/sim/models/load_switches__tps22917.lib
LIB_SHA256=b487750ff2434ed638e7bcb0f8d4d1ade73f7bb638b87354bf22873641af4e18
WORK=${DMS_S1_DIR:-$HOME/.cache/bb-dms-s1}
OUT=${DMS_S1_OUT:-$ROOT/out/dms-s1}
JOBS=${DMS_S1_JOBS:-8}
MODE=${1:-gate}

NGV=$(ngspice -v 2>&1 | grep -o 'ngspice-[0-9][0-9.]*' | head -1)
[[ $NGV == ngspice-42 ]] || { echo "dms-s1: need ngspice 42, found '$NGV'" >&2; exit 1; }
[[ -d $REPO/.git || -f $REPO/.git ]] || { echo "dms-s1: no dial-me-spinner repository at $REPO (DMS_REPO)" >&2; exit 1; }

# The reference tree, once per commit.
SRC=$WORK/src-${COMMIT:0:12}
if [[ ! -f $SRC/.complete ]]; then
  rm -rf "$SRC" && mkdir -p "$SRC"
  git -C "$REPO" archive "$COMMIT" | tar -x -C "$SRC"
  touch "$SRC/.complete"
fi
got=$(sha256sum < "$SRC/$LIB" | cut -d' ' -f1)
[[ $got == "$LIB_SHA256" ]] || { echo "dms-s1: $LIB sha256 $got, expected $LIB_SHA256" >&2; exit 1; }

# The suite's Python packages (same venv as scripts/dms_suite.sh).
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
S1=$ROOT/scripts/dms_s1.py

# The local-only fixed view (a local package directory outside the repository, ADR-006 D9 rule 4).
VIEW_DIR=$WORK/local-packages/ti/tps22917
mkdir -p "$VIEW_DIR"
FIXED=$VIEW_DIR/load_switches__tps22917.s1.lib
"$PY" -I "$S1" fixview --in "$SRC/$LIB" --out "$FIXED" > "$VIEW_DIR/fixview.json"

RUN=$WORK/run
rm -rf "$RUN" && mkdir -p "$RUN/runs"
DECKS=$RUN/decks
"$PY" -I "$S1" decks --src "$SRC" --lib-fixed "$FIXED" --out "$DECKS"

# deck_run: engines/analog-ngspice/examples/deck_run.rs
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$HOME/.cache/bb-target}
(cd "$ROOT" && cargo build --release -q -p bb-analog-ngspice --example deck_run)
DECK_RUN=$CARGO_TARGET_DIR/release/examples/deck_run

opts() { "$PY" -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["option_sets"][sys.argv[2]])' "$DECKS/manifest.json" "$1"; }
case_args() { # <case>: --edge/--probe/--stop/--voh for deck_run
  "$PY" -I - "$DECKS/manifest.json" "$1" <<'EOF'
import json, sys
c = next(c for c in json.load(open(sys.argv[1]))["cases"] if c["name"] == sys.argv[2])
a = ["--stop", f"{c['stop']}fs", "--voh", f"{c['voh']:g}"]
for n, t, lv in c["edges"]:
    a += ["--edge", f"{n}@{t}fs={lv}"]
for p in c["probes"]:
    a += ["--probe", p]
print(" ".join(a))
EOF
}

# One run: eng.<case>.<view>.<options>.g<grant> or ref.<case>.<view>.<options>.<tran spec>.
run_one() {
  local id=$1 kind case view os spec rest
  # case names hold a '.' (e1_suite_bat3.0_-10, qod_bat4.2, table76_3.6): split from the right
  spec=${id##*.}; rest=${id%.*}; os=${rest##*.}; rest=${rest%.*}; view=${rest##*.}; rest=${rest%.*}
  kind=${rest%%.*}; case=${rest#*.}
  local csv=$RUN/runs/$id.csv log=$RUN/runs/$id.log
  if [[ $kind == eng ]]; then
    local grant=${spec#g} cap=${DMS_S1_CAP:-300}
    # shellcheck disable=SC2046
    timeout "$cap" "$DECK_RUN" --body "$DECKS/$case.$view.cir" --options "$(opts "$os")" --lattice 10us \
      --grant "$grant" $(case_args "$case") --csv "$csv" > "$log" 2>&1 || true
    local js; js=$(tail -n 1 "$log")
    if [[ $js != '{'* ]]; then
      local why; why=$(tail -n 1 "$log" | tr -d '"\\' | cut -c1-200)
      js="{\"status\":\"no summary (killed after ${cap} s, or failed: ${why})\",\"reached_fs\":0,\"wall_s\":$cap}"
    fi
    echo "{\"id\":\"$id\",${js#\{}"
  else
    local tran tmax=
    case $spec in
      tran20us) tran=20u ;;
      tmax10us) tran=10u; tmax=10u ;;
      tmax1us) tran=1u; tmax=1u ;;
    esac
    local js
    js=$("$PY" -I "$S1" ref --manifest "$DECKS/manifest.json" --case "$case" --view "$view" --options "$os" \
      --tran "$tran" --tmax "$tmax" --out "$csv" --timeout "${DMS_S1_REF_CAP:-900}" 2> "$log")
    echo "{\"id\":\"$id\",${js#\{}"
  fi
}
export -f run_one opts case_args
export RUN DECKS PY S1 DECK_RUN

if [[ $MODE == gate ]]; then
  ids=()
  for c in e1_bench_nominal e1_bench_cold; do
    ids+=("eng.$c.fixed.phase0.g1ms" "eng.$c.fixed.phase0.g10us" "eng.$c.fixed.phase0.g10ms"
          "ref.$c.unmod.suite_fallback.tmax10us" "ref.$c.unmod.suite_fallback.tran20us"
          "ref.$c.fixed.phase0.tmax1us" "ref.$c.unmod.suite_fallback.tmax1us")
  done
  for c in e1_suite_bat3.0_-10 e1_suite_bat4.2_25 qod_bat3.0 qod_bat4.2 qod_usb5.0 table76_3.6 table76_5.0; do
    ids+=("eng.$c.fixed.phase0.g1ms")
  done
  echo "dms-s1: $COMMIT ($NGV), ${#ids[@]} runs on $JOBS jobs in $RUN (load $(cut -d' ' -f1-3 /proc/loadavg))"
  start=$(date +%s)
  printf '%s\n' "${ids[@]}" | xargs -P "$JOBS" -I{} bash -c 'run_one "$@"' _ {} > "$RUN/runs.jsonl"
  echo "dms-s1: runs done in $(( $(date +%s) - start )) s (load $(cut -d' ' -f1-3 /proc/loadavg))"
  mkdir -p "$OUT"
  set +e
  "$PY" -I "$S1" gate --dir "$RUN" --summary "$OUT/summary.json" \
    --suite-results "$ROOT/examples/dial-me-spinner/suite/results.json"
  rc=$?
  set -e
  cp "$VIEW_DIR/fixview.json" "$OUT/fixview.json"
  exit $rc
fi

if [[ $MODE == cost ]]; then
  # The S1 cost table (plan §8): wall time of the E1 deck (120 ms) through NgspiceEngine per bench, view,
  # option set and grant, with the ADR-003 protocol: N interleaved rounds, medians, load reported.
  # Run it on an otherwise idle host.
  N=${2:-5}
  configs=()
  for c in e1_bench_nominal e1_bench_cold; do
    configs+=("eng.$c.fixed.phase0.g10ms" "eng.$c.fixed.phase0.g1ms" "eng.$c.fixed.phase0.g10us"
              "eng.$c.fixed.suite_fallback.g10ms" "ref.$c.fixed.phase0.tran20us" "ref.$c.unmod.suite_fallback.tran20us")
  done
  mkdir -p "$OUT"
  raw=$OUT/cost.jsonl
  : > "$raw"
  for k in $(seq 1 "$N"); do
    for id in "${configs[@]}"; do
      load=$(cut -d' ' -f1 /proc/loadavg)
      js=$(run_one "$id")
      echo "{\"round\":$k,\"load1\":$load,${js#\{}" >> "$raw"
      echo "dms-s1-cost: round $k: $(tail -n 1 "$raw" | cut -c1-200)"
    done
  done
  # Once, as the R1 record: the suite's own view through the engine at both option sets.
  for c in e1_bench_nominal e1_bench_cold; do
    for os in phase0 suite_fallback; do
      js=$(DMS_S1_CAP=${DMS_S1_STALL_CAP:-120} run_one "eng.$c.unmod.$os.g10ms")
      echo "{\"round\":0,\"load1\":$(cut -d' ' -f1 /proc/loadavg),${js#\{}" >> "$raw"
    done
  done
  "$PY" -I - "$raw" "$OUT/cost.tsv" <<'EOF'
import json, statistics, sys
rows = [json.loads(l) for l in open(sys.argv[1])]
with open(sys.argv[2], "w") as f:
    f.write("run\tn\tstatus\treached_ms\twall_s_median\twall_s_min\twall_s_max\tsolver_points\tload1_median\n")
    for run in dict.fromkeys(r["id"] for r in rows):
        rs = [r for r in rows if r["id"] == run]
        w = [r["wall_s"] for r in rs]
        line = [run, str(len(rs)), ",".join(sorted({r["status"] for r in rs})),
                f"{max(r.get('reached_fs', 0) for r in rs) / 1e12:g}", f"{statistics.median(w):.2f}",
                f"{min(w):.2f}", f"{max(w):.2f}", str(rs[0].get("solver_points", "")),
                f"{statistics.median(r['load1'] for r in rs):.2f}"]
        f.write("\t".join(line) + "\n")
print(open(sys.argv[2]).read())
EOF
  exit 0
fi
echo "usage: dms_s1.sh [gate | cost [N]]" >&2
exit 2
