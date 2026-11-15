#!/usr/bin/env bash
# Phase 2 E4 (ADR-016 D1, G1): the viewers of the docs page read bb's VCD as
# bb wrote it. tools/vcd-check (wellen 0.25.7, Surfer's waveform library,
# outside the Cargo workspace) loads:
#   - every nets.vcd that `make demo` wrote (<demo-out>/*/nets.vcd; the
#     repository commits no VCD), with the run's nets.csv beside it;
#   - the committed fixture tests/fixtures/vcd/mixed.vcd with mixed.csv
#     (digital and real variables, the 1 fs timescale, the $comment name
#     map), so the check has an input when the demo changes.
# Then the negative controls, each on a copy of the fixture, must fail with
# their own message (the check acted): the timescale changed to 1 ps; one
# CSV value changed by 1 uV; one value-change line of a real variable
# dropped; a $comment entry naming an undeclared variable; a real variable
# declared as a 1-bit wire.
#
#   scripts/vcd_check.sh <vcd-check> <demo-out> <out>       (make vcd-check)
#   scripts/vcd_check.sh <vcd-check> --fixture-only <out>   (make vcd-check-fixture)
set -uo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
[[ $# == 3 ]] || { sed -n '2,20p' "$0"; exit 2; }
TOOL=$1 DEMO=$2 OUT=$3
FIX=$ROOT/tests/fixtures/vcd
rm -rf "$OUT" && mkdir -p "$OUT"
status=0
pass() { echo "vcd-check: PASS: $*"; }
fail() { echo "vcd-check: FAIL: $*"; status=1; }

load() { # <name> <vcd> <csv>
  if "$TOOL" "$2" --csv "$3" > "$OUT/$1.txt" 2>&1; then
    pass "$1: $(grep -c '^  board\.' "$OUT/$1.txt") variables, $(tail -2 "$OUT/$1.txt" | head -1 | sed 's/^ *//')"
  else
    cat "$OUT/$1.txt"; fail "$1: $2"
  fi
}

if [[ $DEMO != --fixture-only ]]; then
  n=0
  for v in "$DEMO"/*/nets.vcd; do
    [[ -f $v ]] || continue
    load "demo-$(basename "$(dirname "$v")")" "$v" "$(dirname "$v")/nets.csv"
    n=$((n + 1))
  done
  if [[ $n -ge 2 ]]; then pass "make demo wrote $n VCDs, all loaded"; else fail "make demo wrote $n VCDs in $DEMO (expected its two E1 runs)"; fi
fi
load fixture "$FIX/mixed.vcd" "$FIX/mixed.csv"

neg() { # <name> <expected message> : the files in $OUT/neg-<name>/
  local d=$OUT/neg-$1
  if "$TOOL" "$d/mixed.vcd" --csv "$d/mixed.csv" > "$d/out.txt" 2>&1; then
    fail "negative control $1: vcd-check passed"
  elif grep -q -- "$2" "$d/out.txt"; then
    pass "negative control $1: fails with \"$2\""
  else
    cat "$d/out.txt"; fail "negative control $1: failed, but not with \"$2\""
  fi
}
mk() { mkdir -p "$OUT/neg-$1" && cp "$FIX/mixed.vcd" "$FIX/mixed.csv" "$OUT/neg-$1/"; }

mk timescale && sed -i 's/^\$timescale 1 fs \$end$/$timescale 1 ps $end/' "$OUT/neg-timescale/mixed.vcd"
neg timescale "timescale is not 1 fs"
# The last data row, first real column: its last digit changed by 1 (1 uV).
mk csv-value && python3 - "$OUT/neg-csv-value/mixed.csv" << 'EOF'
import sys
p = sys.argv[1]
rows = open(p).read().splitlines()
f = rows[-1].split(",")
d = int(f[1][-1])
f[1] = f[1][:-1] + str((d + 1) % 10)
rows[-1] = ",".join(f)
open(p, "w").write("\n".join(rows) + "\n")
EOF
neg csv-value "CSV values differ from wellen's"
# The last value-change line of the first real variable (id `!`, VLED).
mk drop-change && python3 - "$OUT/neg-drop-change/mixed.vcd" << 'EOF'
import sys
p = sys.argv[1]
rows = open(p).read().splitlines()
i = max(k for k, l in enumerate(rows) if l.startswith("r") and l.endswith(" !") and not rows[k - 1] == "#0")
del rows[i]
open(p, "w").write("\n".join(rows) + "\n")
EOF
neg drop-change "CSV values differ from wellen's"
mk comment && sed -i 's/^  LED_ring_LED_DIN = /  LED_ring_LED_DIN_X = /' "$OUT/neg-comment/mixed.vcd"
neg comment "which is not declared"
mk real-as-wire && sed -i 's/^\$var real 64 % VSYS \$end$/$var wire 1 % VSYS $end/' "$OUT/neg-real-as-wire/mixed.vcd"
neg real-as-wire "wellen could not load it"
# A 1-bit wire declared as a reg: wellen reads VarType::Reg, not the Wire bb writes.
mk wire-as-reg && sed -i 's/^\$var wire 1 & U10\.P1\.04 \$end$/$var reg 1 \& U10.P1.04 $end/' "$OUT/neg-wire-as-reg/mixed.vcd"
neg wire-as-reg "bb writes only"
for n in timescale csv-value drop-change comment real-as-wire wire-as-reg; do
  if cmp -s "$OUT/neg-$n/mixed.vcd" "$FIX/mixed.vcd" && cmp -s "$OUT/neg-$n/mixed.csv" "$FIX/mixed.csv"; then
    fail "negative control $n: the copy equals the fixture (the edit did nothing)"
  fi
done

if [[ $status == 0 ]]; then echo "vcd-check: PASS"; else echo "vcd-check: FAIL"; fi
exit $status
