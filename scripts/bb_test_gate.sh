#!/usr/bin/env bash
# Milestone B3's gate (Phase 2 plan section 4 B3; ADR-010 D8-D9, G4, G5, G9; Amendment 5): `bb test`,
# bb-test/0, test.json and junit.xml. make dms-rail-test, dms-vsense-test and dms-home-test (in make test)
# are the dms-* targets re-expressed as bb test files (examples/dial-me-spinner/tests/e1.toml, e2.toml,
# e3.toml) with the same outcomes; this gate adds:
#
#  1. bb test's semantics (tests/bb-test/semantics.toml): exit 0, every case ok; reference = true says
#     "equivalence" where the case's grants are longer than one cell (feedback with lookahead, the
#     spike's 10 ms grants) and "consistency" where they are one cell already (feedback at --lookahead
#     off); a not_evaluable assertion expected so is a skipped testcase.
#  2. Its negative controls (tests/bb-test/negative.toml): exit 1, and test.json lists exactly the four
#     failing cases, each failing for its own reason: an unexpected pass of a negative control (the
#     assertion, not the run), a golden mismatch (only nets.csv), a same_as mismatch (events.jsonl),
#     a run bb refuses (an error testcase); a negative control that holds passes.
#  3. G9: a negative control without a count or event assertion expected to pass is refused before
#     anything runs, naming G9.
#  4. Determinism: each of the three dms-* test files run DET_RUNS (10) times, test.json byte-identical
#     across them; the last run with --pool off (every Renode fresh), the others on the pool
#     (ADR-004: results never depend on leasing). check_bb_test.py on every run (test.json canonical
#     and valid against spec/schema/test-result.schema.json, its hashes the files', junit.xml valid
#     against spec/schema/junit-subset.xsd and saying what test.json says).
#  5. The JUnit reporter (D9, G5): scripts/junit_reporter_check.sh reads every junit.xml of 1, 2 and 4
#     (the first run of each) with dorny/test-reporter's java-junit parser at the commit CI pins.
#
#   scripts/bb_test_gate.sh <bb> <out>
set -euo pipefail
BB=$(realpath "$1")
OUT=$(realpath -m "$2")
ROOT=$(cd "$(dirname "$0")/.." && pwd)
PY=${PYTHON:-python3}
DET_RUNS=${DET_RUNS:-10}
JOBS=${BB_TEST_JOBS:-5}
status=0
pass() { echo "bb-test-gate: PASS: $*"; }
fail() { echo "bb-test-gate: FAIL: $*"; status=1; }
rm -rf "$OUT" && mkdir -p "$OUT"

bbtest() { # <file> <out> [args...]: bb test, its exit status
  local f=$1 o=$2
  shift 2
  "$BB" test "$f" --out "$o" --jobs "$JOBS" "$@" > "$o.log" 2>&1
}

checked() { # <out> [--expect-fail]
  if "$PY" "$ROOT/scripts/check_bb_test.py" "$@" > "$1.check.txt"; then
    pass "$(basename "$1"): test.json and junit.xml against the spec ($(grep -c '^check_bb_test: ok' "$1.check.txt") checks)"
  else
    fail "$(basename "$1"): check_bb_test.py"; grep -v '^check_bb_test: ok' "$1.check.txt"
  fi
}

# --- 1. semantics -------------------------------------------------------------------------------
if bbtest "$ROOT/tests/bb-test/semantics.toml" "$OUT/semantics" --pool env; then
  pass "semantics.toml: bb test exits 0"
else
  fail "semantics.toml: bb test failed"; grep '^bb test' "$OUT/semantics.log" | tail -20
fi
checked "$OUT/semantics"
if "$PY" - "$OUT/semantics/test.json" <<'EOF'; then
import json, sys
t = json.load(open(sys.argv[1]))
c = {x["name"]: x for x in t["cases"]}
want = {"feedback-lookahead": "equivalence", "spike": "equivalence", "feedback-one-cell": "consistency"}
got = {n: c[n]["reference"]["kind"] for n in want}
assert got == want, got
assert all(c[n]["reference"]["ok"] for n in want)
a = {x["name"]: x for x in c["feedback"]["asserts"]}
assert a["SENSE-settle"]["outcome"] == "not_evaluable" and a["SENSE-settle"]["ok"], a["SENSE-settle"]
assert c["feedback"]["golden"]["ok"] and c["feedback"]["golden"]["checked"] == ["nets.csv", "nets.vcd", "non-grant-records"]
print(f"bb-test-gate:   references: {got}; SENSE-settle not_evaluable as expected")
EOF
  pass "semantics.toml: reference kinds (equivalence on longer grants, consistency on one-cell grants), the golden, an expected not_evaluable"
else
  fail "semantics.toml: test.json"
fi
grep -q '<skipped message="not_evaluable (expected)' "$OUT/semantics/junit.xml" \
  && pass "semantics.toml: the expected not_evaluable is a skipped testcase in junit.xml" \
  || fail "semantics.toml: no skipped testcase"

# --- 2. negative controls -------------------------------------------------------------------------
if bbtest "$ROOT/tests/bb-test/negative.toml" "$OUT/negative" --pool env; then
  fail "negative.toml: bb test exited 0"
else
  pass "negative.toml: bb test exits non-zero"
fi
checked "$OUT/negative" --expect-fail
if "$PY" - "$OUT/negative/test.json" <<'EOF'; then
import json, sys
t = json.load(open(sys.argv[1]))
assert t["failed"] == ["unexpected-pass", "golden-mismatch", "same-as-differs", "run-fails"], t["failed"]
c = {x["name"]: x for x in t["cases"]}
u = {a["name"]: a for a in c["unexpected-pass"]["asserts"]}
assert u["PB0-edges"]["outcome"] == "pass" and u["PB0-edges"]["expected"] == "fail" and not u["PB0-edges"]["ok"]
assert u["PA5-edges"]["ok"] and c["unexpected-pass"]["run"] == "ok"
g = c["golden-mismatch"]["golden"]
assert not g["ok"] and g["differs"] == ["nets.csv"] and g["checked"] == ["nets.csv", "nets.vcd", "non-grant-records"], g
s = c["same-as-differs"]["same_as"]
assert not s["ok"] and s["case"] == "unexpected-pass"
assert c["run-fails"]["run"] == "failed" and c["run-fails"]["asserts"] == []
k = {a["name"]: a for a in c["control-ok"]["asserts"]}
assert c["control-ok"]["ok"] and k["PB0-many"]["outcome"] == "fail" and k["PB0-many"]["expected"] == "fail"
print("bb-test-gate:   failed: " + ", ".join(t["failed"]) + "; control-ok passes")
EOF
  pass "negative.toml: exactly the four controls fail, each for its own reason"
else
  fail "negative.toml: test.json"
fi
grep -q '<error message="bb: --max-quantum 10ms must be a positive multiple' "$OUT/negative/junit.xml" \
  && pass "negative.toml: the refused run is an error testcase in junit.xml" || fail "negative.toml: no error testcase"

# --- 3. G9 ----------------------------------------------------------------------------------------
"$PY" - "$ROOT/tests/bb-test/negative.toml" "$OUT/g9.toml" <<'EOF'
import sys
s = open(sys.argv[1]).read()
old = '[[case.assert]]\nname = "PA5-edges"\nkind = "count"\nnet = "U1.PA5"\nmin = 1\n\n[[case]]\nname = "golden-mismatch"'
assert s.count(old) == 1
open(sys.argv[2], "w").write(s.replace(old, '[[case]]\nname = "golden-mismatch"'))
EOF
if "$BB" test "$OUT/g9.toml" --out "$OUT/g9" --pool off > "$OUT/g9.log" 2>&1; then
  fail "G9: a negative control without a count or event assertion ran"
elif grep -q 'ADR-010 G9' "$OUT/g9.log" && [[ ! -e $OUT/g9/test.json ]]; then
  pass "G9: refused before any run: $(grep -o 'case unexpected-pass: [^(]*' "$OUT/g9.log" | head -1)"
else
  fail "G9: not refused with its message"; cat "$OUT/g9.log"
fi

# --- 4. determinism -------------------------------------------------------------------------------
for t in e1 e2 e3; do
  f=$ROOT/examples/dial-me-spinner/tests/$t.toml
  ok=1
  for k in $(seq -w 1 "$DET_RUNS"); do
    d=$OUT/$t-$k
    pool=env
    [[ $((10#$k)) == "$DET_RUNS" ]] && pool=off
    if ! bbtest "$f" "$d" --pool "$pool"; then
      ok=0; fail "$t run $k (--pool $pool): bb test failed"; grep '^bb test' "$d.log" | tail -8
    fi
    "$PY" "$ROOT/scripts/check_bb_test.py" "$d" > "$d.check.txt" || { ok=0; fail "$t run $k: check_bb_test.py"; grep -v ': ok' "$d.check.txt"; }
    if [[ $k != 01 ]] && ! cmp -s "$OUT/$t-01/test.json" "$d/test.json"; then
      ok=0; fail "$t run $k (--pool $pool): test.json differs from run 01"
    fi
  done
  [[ $ok == 1 ]] && pass "$t.toml: $DET_RUNS bb test runs, test.json byte-identical (the last with --pool off, every Renode fresh): $(sha256sum "$OUT/$t-01/test.json" | cut -c1-16), every run valid (check_bb_test.py)"
done

# --- 5. the JUnit reporter ------------------------------------------------------------------------
if bash "$ROOT/scripts/junit_reporter_check.sh" "$OUT/semantics/junit.xml" "$OUT/negative/junit.xml" \
    "$OUT/e1-01/junit.xml" "$OUT/e2-01/junit.xml" "$OUT/e3-01/junit.xml" > "$OUT/junit-reporter.txt" 2>&1; then
  pass "$(grep '^junit-reporter: PASS' "$OUT/junit-reporter.txt" | sed 's/^junit-reporter: PASS: //')"
else
  fail "the JUnit reporter"
fi
sed -n 's/^junit-reporter:   /    /p' "$OUT/junit-reporter.txt"

if [[ $status == 0 ]]; then echo "bb-test-gate: PASS"; else echo "bb-test-gate: FAIL"; fi
exit $status
