#!/usr/bin/env bash
# Phase 1 plan M7 (section 5, the exit): the reference demo of the README in one command, `make demo`.
#
#   bash scripts/demo.sh [<bb>]        (default: bb on PATH)
#
# E1 on dial-me-spinner rev A (examples/dial-me-spinner): the README quickstart's commands, run on a
# copy of the project in $DEMO_WORK (default ~/.cache/bb-demo), so the checkout is never written:
#
#   bb open                                           board.bbm from the vendored netlist (unchanged)
#   bb flash --target U10 firmware/prebuilt/led_rail.elf
#   bb run --for 120ms --bench bench/cold.toml --probe net:VLED --probe net:LED_DIN --out <out>/rail
#   check_dms_rail.py <out>/rail                     PASS: LED-DATA-V4, LSW-3, LSW-2, FW-6.4
#   bb flash --target U10 firmware/prebuilt/led_rail_wrong_order.elf
#   bb run ... --out <out>/rail-wrong
#   check_dms_rail.py <out>/rail-wrong               FAIL: LSW-3 (and only LSW-3), as expected
#
# Then, which the README run does not do: both runs' nets.csv, nets.vcd and non-grant events.jsonl
# records against the committed one-cell hashes (tests/golden/dms-rail/{ok,wrong}-cold; plan
# section 5 item 2), and run.json's list of analog views with provenance, confidence, qualification
# and the solver option set (section 5 item 4). Every Renode starts fresh (a stranger has no pool):
# BB_RENODE_POOL is ignored. Outputs in $DEMO_OUT (default out/demo). Needs KiCad, Zephyr and the
# ARM toolchain not at all; `make setup SETUP=demo` is enough. Exit 0 only when every step passes.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
BB=${1:-$(command -v bb || true)}
[[ -n $BB && -x $BB ]] || { echo "demo: no bb (run make setup SETUP=demo, or pass its path)" >&2; exit 2; }
BB=$(cd "$(dirname "$BB")" && pwd)/$(basename "$BB")
PY=${PYTHON:-python3}
SRC=$ROOT/examples/dial-me-spinner
GOLD=$ROOT/tests/golden/dms-rail
WORK=${DEMO_WORK:-$HOME/.cache/bb-demo}
OUT=${DEMO_OUT:-$ROOT/out/demo}
unset BB_RENODE_POOL

status=0
pass() { echo "demo: PASS: $*"; }
fail() { echo "demo: FAIL: $*"; status=1; }
step() { echo; echo "demo: \$ $*"; }

# The project copy: what the demo's commands read (bb.toml names the netlist, the benches and the
# prebuilt images), never the checkout.
rm -rf "$WORK" "$OUT"
mkdir -p "$WORK/kicad/main" "$WORK/firmware/prebuilt" "$OUT"
OUT=$(cd "$OUT" && pwd) # the runs below work in $WORK
cp "$SRC/bb.toml" "$SRC/board.bbm" "$SRC/bb.lock" "$WORK/"
cp "$SRC/kicad/main/dial-me-spinner-main.net" "$WORK/kicad/main/"
cp -r "$SRC/bench" "$WORK/bench"
cp "$SRC/firmware/prebuilt/led_rail.elf" "$SRC/firmware/prebuilt/led_rail_wrong_order.elf" "$WORK/firmware/prebuilt/"
cd "$WORK"
echo "demo: bb $BB, project copy $WORK, outputs $OUT"

step bb open
"$BB" open

run_fw() { # <elf> <outdir>
  step bb flash --target U10 "firmware/prebuilt/$1"
  "$BB" flash --target U10 "firmware/prebuilt/$1"
  step bb run --for 120ms --bench bench/cold.toml --probe net:VLED --probe net:LED_DIN --out "$2"
  "$BB" run --for 120ms --bench bench/cold.toml --probe net:VLED --probe net:LED_DIN --out "$2"
  grep -v '"type":"grant"' "$2/events.jsonl" > "$2/non-grant-records"
}

run_fw led_rail.elf "$OUT/rail"
step python3 scripts/check_dms_rail.py "$OUT/rail"
if "$PY" "$ROOT/scripts/check_dms_rail.py" "$OUT/rail"; then
  pass "the correct order passes LED-DATA-V4, LSW-3, LSW-2 and FW-6.4"
else
  fail "check_dms_rail.py on the correct-order run"
fi

run_fw led_rail_wrong_order.elf "$OUT/rail-wrong"
step python3 scripts/check_dms_rail.py "$OUT/rail-wrong"
set +e
"$PY" "$ROOT/scripts/check_dms_rail.py" "$OUT/rail-wrong" | tee "$OUT/rail-wrong/checks.txt"
rc=${PIPESTATUS[0]}
set -e
if [[ $rc != 0 ]] && "$PY" "$ROOT/scripts/check_dms_rail.py" "$OUT/rail-wrong" \
    --expect-fail LSW-3 > /dev/null; then
  pass "the wrong order fails exactly LSW-3 (check_dms_rail.py exit $rc), as the demo expects"
else
  fail "the wrong-order run does not fail exactly LSW-3 (exit $rc)"
fi

echo
for pair in rail:ok-cold rail-wrong:wrong-cold; do
  d=$OUT/${pair%%:*} c=${pair#*:}
  if (cd "$d" && grep -v ' events.jsonl$' "$GOLD/$c/outputs.sha256" | sha256sum --quiet -c); then
    pass "${pair%%:*}: nets.csv, nets.vcd and the non-grant records equal the committed one-cell hashes ($c)"
  else
    fail "${pair%%:*}: differs from tests/golden/dms-rail/$c/outputs.sha256"
  fi
done

# Section 5 item 4: every analog view in the run, with its provenance, confidence, qualification
# and the option set; bb run's own summary flags the agent_generated and estimated ones.
if "$PY" - "$OUT/rail/run.json" <<'EOF'; then
import collections, json, sys
r = json.load(open(sys.argv[1]))
opts = r.get("bench", {}).get("options")
views = collections.Counter()
for a in r["analog"]:
    p = a["provenance"]
    assert p.get("source") and p.get("confidence") and a.get("qualified") is not None, a["ref"]
    assert a["qualified_for_run"], a["ref"]
    views[(a["package"], a["view"], p["source"], p["confidence"])] += 1
print(f"demo: run.json: {sum(views.values())} analog instances, option set {opts}, every view qualified for the run:")
for (pkg, view, src, conf), n in sorted(views.items()):
    print(f"demo:   {n:4d} x {pkg}:{view}  provenance {src}, confidence {conf}")
EOF
  pass "run.json lists every analog view with provenance, confidence and qualification"
else
  fail "run.json's analog view list"
fi

echo
if [[ $status == 0 ]]; then echo "demo: PASS (outputs in $OUT)"; else echo "demo: FAIL"; fi
exit $status
