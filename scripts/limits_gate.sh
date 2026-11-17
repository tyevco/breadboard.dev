#!/usr/bin/env bash
# ADR-009 D3 (Amendment 11): package limits as automatic range assertions, at pi and with `bb run
# --limits` at fast. make limits-test (in make test).
#
# The fixture: examples/dial-me-spinner's E1 project, copied, with a LOCAL copy of
# worldsemi/ws2812c-2020 that shadows the first-party one (spec/package.md section 1: the first
# directory that provides an id wins). The copy adds, as a gate fixture and not as a first-party
# package fact:
#   - [[limits]] DIN max = "VDD + 0.7V": E1's LSW-3 (the dial-me-spinner suite's rule) written as a
#     package limit, so each of the 48 pixels (D101 the first) gets limit:main:<D>.DIN:max;
#   - the power model on VDD with [defaults.power.default] vmax = "5.3V" and imax = "20mA" (fixture
#     values): limit:main:<D>.VDD:vmax (a range check) and :imax (not_evaluable: no view senses a
#     pin's current from node voltages).
# Checks (60 ms; LSW-3's excursion is at 45 ms):
#   pi-ok     E1 cold at pi (bench/cold-pi.toml), the correct-order firmware: 144 limit records,
#             rung pi, every DIN:max and VDD:vmax passes, every VDD:imax is not_evaluable; run.json
#             lists 144 limits, all asserted.
#   pi-wrong  the wrong-order firmware (the negative control): exactly limit:main:D101.DIN:max fails
#             (value above 0.7 V), as check_dms_rail.py fails exactly LSW-3; the rest as pi-ok.
#   fast      E1 cold at fast with --limits: the same 144 records at rung fast, the wrong order's D1
#             failing again; and the same run without --limits has no limit record, no run.json
#             limits, and nets.csv, nets.vcd and every non-assert record byte-identical (limits are
#             reads).
#   nolimits  the committed project (no fixture) at pi: no limit record (no first-party package
#             declares a limit, so no committed pi result changes).
#
# usage: scripts/limits_gate.sh <bb> <out>
set -euo pipefail
BB=$(realpath "$1")
OUT=$2
ROOT=$(cd "$(dirname "$0")/.." && pwd)
PROJ=$ROOT/examples/dial-me-spinner
FWD=$PROJ/firmware/prebuilt
WORK=${LIMITS_WORK:-${BB_SCRATCH:-$HOME/.cache}/bb-limits}
FOR=${LIMITS_FOR:-60ms}
PY=${PYTHON:-python3}
status=0
pass() { echo "limits: PASS: $*"; }
fail() { echo "limits: FAIL: $*"; status=1; }

rm -rf "$WORK" "$OUT"
mkdir -p "$WORK" "$OUT"
for fw in ok wrong plain; do
  d=$WORK/proj-$fw
  elf=led_rail.elf
  [[ $fw == wrong ]] && elf=led_rail_wrong_order.elf
  mkdir -p "$d/kicad/main" "$d/firmware/prebuilt"
  cp "$PROJ/bb.toml" "$PROJ/board.bbm" "$PROJ/bb.lock" "$d/"
  cp "$PROJ/kicad/main/dial-me-spinner-main.net" "$d/kicad/main/"
  cp -r "$PROJ/bench" "$d/bench"
  cp "$FWD/$elf" "$d/firmware/prebuilt/"
  if [[ $fw != plain ]]; then
    p=$d/lpk/worldsemi/ws2812c-2020
    mkdir -p "$(dirname "$p")"
    cp -r "$ROOT/packages/worldsemi/ws2812c-2020" "$p"
    "$PY" - "$p/pins.toml" <<'EOF'
import sys
p = sys.argv[1]
s = open(p).read()
old = 'name = "VDD"\nrole = "power_in"\n'
assert s.count(old) == 1, "VDD pin"
s = s.replace(old, old + 'models = ["power"]\n')
s += '''
# --- scripts/limits_gate.sh fixture (ADR-009 D3, Amendment 11); not the first-party package ---
[defaults.power.default]
vmax = "5.3V"
imax = "20mA"

[[limits]]
pin = "DIN"
max = "VDD + 0.7V"
source = "gate fixture: E1's LSW-3 (the dial-me-spinner suite's rule) as a package limit"
'''
open(p, "w").write(s)
EOF
    printf '\n[packages]\npaths = ["lpk"]\n' >> "$d/bb.toml"
    "$BB" pkg lint "$d/lpk" > "$OUT/lint-$fw.log" 2>&1 || { cat "$OUT/lint-$fw.log"; fail "bb pkg lint on the fixture"; }
    "$BB" open --update-lock "$d" > "$OUT/open-$fw.log" 2>&1 || { cat "$OUT/open-$fw.log"; exit 1; }
  fi
  "$BB" open --check "$d" > /dev/null 2>&1 || { echo "limits: $d not current"; exit 1; }
  "$BB" flash --project "$d" --target U10 "$d/firmware/prebuilt/$elf" > /dev/null
done

run() { # <name> <project> <bench> [extra]
  local n=$1 d=$2 b=$3
  shift 3
  if "$BB" run --project "$d" --bench "$d/bench/$b" --for "$FOR" --probe net:VLED --probe net:LED_DIN "$@" \
      --out "$OUT/$n" > "$OUT/$n.log" 2>&1; then
    pass "$n: bb run"
  else
    fail "$n: bb run"; tail -5 "$OUT/$n.log"
  fi
}
run pi-ok "$WORK/proj-ok" cold-pi.toml &
run pi-wrong "$WORK/proj-wrong" cold-pi.toml &
run fast-ok "$WORK/proj-ok" cold.toml --limits &
wait
run fast-wrong "$WORK/proj-wrong" cold.toml --limits &
run fast-ok-off "$WORK/proj-ok" cold.toml &
run pi-plain "$WORK/proj-plain" cold-pi.toml &
wait

"$PY" - "$OUT" <<'EOF' || status=1
import json, sys
from pathlib import Path
out = Path(sys.argv[1])
ok = True
def check(cond, text):
    global ok
    print(f"limits: {'PASS' if cond else 'FAIL'}: {text}")
    ok = ok and bool(cond)
def recs(run):
    return [json.loads(l) for l in open(out / run / "events.jsonl")]
def limits(run):
    return {r["name"]: r for r in recs(run) if r["type"] == "assert" and r["name"].startswith("limit:")}
for run, rung, bad in (("pi-ok", "pi", []), ("pi-wrong", "pi", ["limit:main:D101.DIN:max"]),
                       ("fast-ok", "fast", []), ("fast-wrong", "fast", ["limit:main:D101.DIN:max"])):
    L = limits(run)
    din = sorted(n for n in L if n.endswith(".DIN:max"))
    vmax = sorted(n for n in L if n.endswith(".VDD:vmax"))
    imax = sorted(n for n in L if n.endswith(".VDD:imax"))
    check(len(L) == 144 and len(din) == len(vmax) == len(imax) == 48,
          f"{run}: {len(L)} limit records (48 pixels x DIN:max, VDD:vmax, VDD:imax; not vacuous)")
    check(all(r["rung"] == rung for r in L.values()), f"{run}: every limit record at rung {rung}")
    failed = sorted(n for n in din + vmax if L[n]["outcome"] != "pass")
    check(failed == bad, f"{run}: failing DIN:max / VDD:vmax limits {failed} (expected {bad})")
    check(all(L[n]["outcome"] == "not_evaluable" and "pass" not in L[n] for n in imax),
          f"{run}: every VDD:imax not_evaluable")
    d1 = L.get("limit:main:D101.DIN:max", {})
    v = d1.get("value")
    if bad:
        check(v is not None and v > 700000 and d1.get("limit") == 700000,
              f"{run}: limit:main:D101.DIN:max = {v} uV against {d1.get('limit')} uV (LSW-3's excursion)")
    print(f"limits: report: {run}: limit:main:D101.DIN:max {d1.get('outcome')} {v} uV: {d1.get('msg', '')}")
    rj = json.load(open(out / run / "run.json"))
    lj = rj.get("limits", [])
    check(len(lj) == 144 and all(x["asserted"] for x in lj), f"{run}: run.json lists {len(lj)} limits, all asserted")
    check(rj.get("limits_flag") is (True if rung == "fast" else None), f"{run}: run.json limits_flag {rj.get('limits_flag')}")
# Reads only: --limits changes no output but its assert records.
a, b = out / "fast-ok", out / "fast-ok-off"
check(not limits("fast-ok-off") and "limits" not in json.load(open(b / "run.json")),
      "fast without --limits: no limit record, no run.json limits")
for f in ("nets.csv", "nets.vcd"):
    check((a / f).read_bytes() == (b / f).read_bytes(), f"fast --limits: {f} byte-identical to the run without")
strip = lambda d: [l for l in open(d / "events.jsonl") if '"type":"assert"' not in l]
check(strip(a) == strip(b), "fast --limits: every non-assert record byte-identical to the run without")
check(not limits("pi-plain") and "limits" not in json.load(open(out / "pi-plain" / "run.json")),
      "the committed packages at pi: no limit record (no first-party package declares a limit)")
sys.exit(0 if ok else 1)
EOF
# The negative control agrees with E1's own check: on pi-wrong check_dms_rail.py fails LSW-3, and
# otherwise only LSW-2, which a 60 ms run cannot evaluate (VLED's decay after the 35 ms fall).
for c in pi-ok pi-wrong; do
  "$PY" "$ROOT/scripts/check_dms_rail.py" --project "$WORK/proj-${c#pi-}" "$OUT/$c" \
    --json "$OUT/$c.rail.json" > "$OUT/$c.rail.txt" 2>&1 || true
done
"$PY" - "$OUT" <<'EOF' || status=1
import json, sys
out = sys.argv[1]
ok = True
for c, want in (("pi-ok", False), ("pi-wrong", True)):
    d = json.load(open(f"{out}/{c}.rail.json"))
    lsw2 = d["checks"]["LSW-2"]
    short = all(f.get("pass") is None for f in lsw2.get("falls", []))
    other = [f for f in d["failed"] if f not in ("LSW-3",) and not (f == "LSW-2" and short)]
    lsw3 = "LSW-3" in d["failed"]
    rec = [json.loads(l) for l in open(f"{out}/{c}/events.jsonl") if '"limit:main:D101.DIN:max"' in l][0]
    good = lsw3 == want and not other and (rec["outcome"] == "fail") == want
    ok = ok and good
    print(f"limits: {'PASS' if good else 'FAIL'}: {c}: check_dms_rail.py LSW-3 {'fails' if lsw3 else 'passes'} "
          f"({d['checks']['LSW-3']['max_din_over_vled_V']:.6f} V from the 10 us samples), "
          f"limit:main:D101.DIN:max {rec['outcome']} ({rec['value'] / 1e6:.6f} V from the solver points); "
          f"other failures {other} (LSW-2 not evaluable in {c}'s 60 ms: {short})")
sys.exit(0 if ok else 1)
EOF
[[ $status == 0 ]] && echo "limits: PASS" || echo "limits: FAIL"
exit $status
