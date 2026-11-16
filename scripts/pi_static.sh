#!/usr/bin/env bash
# Phase 2 P1 (plan section 4; ADR-009 D1-D3, Amendment 4; decision D-W1, ADR-009 Amendment 2): the W1 and
# W2 static-load decks on the `pi` package views, against the suite replica with P0's tolerances, and
# W1's load sweep.
#
#   scripts/pi_static.sh [test|gate]
#
# The decks are assembled by scripts/pi_static.py from the packages and the dial-me-spinner system
# import (examples/dial-me-spinner/system/board.bbm), as examples/dial-me-spinner/pi/static_load.toml
# says, and run through NgspiceEngine with deck_run (lattice 10 us, 1 ms grants, phase0): the project
# route needs milestone R2. `test` (make pi-static-test, in make test) runs the seven cases; `gate`
# (make pi-static) adds the stress corner at 10 us and 10 ms grants and a second 1 ms run. Nothing here
# reads the dial-me-spinner repository: the replica's values are P0's recorded measurements.
# Outputs in $PI_STATIC_OUT (default out/pi-static); runs on $PI_STATIC_JOBS jobs (default 8).
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
MODE=${1:-gate}
OUT=${PI_STATIC_OUT:-$ROOT/out/pi-static}
JOBS=${PI_STATIC_JOBS:-8}
SPEC=$ROOT/examples/dial-me-spinner/pi/static_load.toml
TOL=$ROOT/examples/dial-me-spinner/pi-spike/tolerances.toml
BBM=$ROOT/examples/dial-me-spinner/system/board.bbm
PY=${PYTHON:-python3}

NGV=$(ngspice -v 2>&1 | grep -o 'ngspice-[0-9][0-9.]*' | head -1)
[[ $NGV == ngspice-42 ]] || { echo "pi-static: need ngspice 42, found '$NGV'" >&2; exit 1; }

rm -rf "$OUT" && mkdir -p "$OUT/runs"
"$PY" -I "$ROOT/scripts/pi_static.py" decks --bbm "$BBM" --packages "$ROOT/packages" --spec "$SPEC" \
  --out "$OUT/decks"

# deck_run: the Makefile passes the debug build that `make build` makes (as fast as the release build
# here: ngspice does the work); run directly, the script builds it.
if [[ -z ${DECK_RUN:-} ]]; then
  export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$HOME/.cache/bb-target}
  (cd "$ROOT" && cargo build -q -p bb-analog-ngspice --example deck_run)
  DECK_RUN=$CARGO_TARGET_DIR/debug/examples/deck_run
fi

run_one() { # <case>.<grant>: grant g1ms, g10us, g10ms or g1ms_again
  local id=$1 case grant
  IFS=. read -r case grant <<< "$id"
  local g=${grant#g}; g=${g%_again}
  local args
  args=$("$PY" -I - "$OUT/decks/manifest.json" "$case" <<'EOF'
import json, sys
c = json.load(open(sys.argv[1]))["cases"][sys.argv[2]]
n, t, lv = c["edge"]
a = ["--body", f"{sys.argv[2]}.cir", "--stop", f"{c['stop_fs']}fs", "--voh", "3.3", "--edge", f"{n}@{t}fs={lv}"]
for p in sorted(set(c["probes"].values())):
    a += ["--probe", p]
print(" ".join(a))
EOF
)
  # shellcheck disable=SC2086
  (cd "$OUT/decks" && timeout "${PI_STATIC_CAP:-300}" "$DECK_RUN" $args \
     --options '.options RELTOL=1e-5 VNTOL=1e-8 ABSTOL=1e-13' --lattice 10us --grant "$g" \
     --csv "$OUT/runs/$id.csv") > "$OUT/runs/$id.log" 2>&1 || true
  local js; js=$(tail -n 1 "$OUT/runs/$id.log")
  if [[ $js != '{'* ]]; then
    js="{\"status\":\"no summary: $(tail -n 1 "$OUT/runs/$id.log" | tr -d '"\\' | cut -c1-200)\"}"
  fi
  echo "{\"id\":\"$id\",${js#\{}"
}
export -f run_one
export OUT PY DECK_RUN

ids=()
for c in w1_l30 w1_l50 w1_l50_c3half w1_l55 w1_l60 w2rule w2old; do ids+=("$c.g1ms"); done
FULL=()
if [[ $MODE == gate ]]; then
  ids+=(w1_l55.g10us w1_l55.g10ms w1_l55.g1ms_again)
  FULL=(--full)
elif [[ $MODE != test ]]; then
  echo "usage: pi_static.sh [test|gate]" >&2; exit 2
fi
echo "pi-static: ${#ids[@]} runs on $JOBS jobs (load $(cut -d' ' -f1-3 /proc/loadavg))"
start=$(date +%s)
printf '%s\n' "${ids[@]}" | xargs -P "$JOBS" -I{} bash -c 'run_one "$@"' _ {} | sort > "$OUT/runs.jsonl"
echo "pi-static: runs done in $(( $(date +%s) - start )) s"
"$PY" -I "$ROOT/scripts/pi_static.py" gate --dir "$OUT" --spec "$SPEC" --tolerances "$TOL" \
  --summary "$OUT/summary.json" "${FULL[@]}"
