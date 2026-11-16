#!/usr/bin/env python3
"""bb test's outputs against the spec (milestone B3; ADR-010 D8-D9, Amendment 5; spec/test.md).

    check_bb_test.py <out> [--expect-ok | --expect-fail]

On a `bb test --out <out>` directory:
  - test.json is canonical JSON (spec/canonical-json.md: keys sorted, no whitespace, integers only,
    one newline) and valid against spec/schema/test-result.schema.json;
  - every output hash in it is the hash of that case's file in <out>/<case>/ (non-grant-records:
    events.jsonl without the grant and the assert records);
  - junit.xml is valid against spec/schema/junit-subset.xsd (xmllint) and says what test.json
    says: one testsuite per case in order, a testcase per reported assertion and per comparison,
    failures and errors exactly where test.json has a case element that is not ok;
  - test.json's ok is --expect-ok's (the default) or --expect-fail's.
Exit status 0 when every check holds.
"""
import hashlib
import json
import subprocess
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FAILS = []


def say(ok, what):
    print(f"check_bb_test: {'ok  ' if ok else 'FAIL'} {what}")
    if not ok:
        FAILS.append(what)


def sha(b):
    return "sha256:" + hashlib.sha256(b).hexdigest()


def no_float(t):
    raise ValueError(f"a float ({t})")


def main(argv):
    if not argv or argv[0].startswith("-"):
        print(__doc__)
        return 2
    out = Path(argv[0])
    want_ok = "--expect-fail" not in argv[1:]
    raw = (out / "test.json").read_bytes()
    tj = json.loads(raw, parse_float=no_float)
    canon = json.dumps(tj, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode() + b"\n"
    say(raw == canon, "test.json is canonical JSON (keys sorted, no whitespace, integers only)")
    try:
        import jsonschema
    except ImportError:
        say(False, "python3-jsonschema is missing (make setup installs it)")
        return 1
    schema = json.loads((ROOT / "spec/schema/test-result.schema.json").read_text())
    jsonschema.Draft202012Validator.check_schema(schema)
    errs = list(jsonschema.Draft202012Validator(schema).iter_errors(tj))
    for e in errs[:5]:
        print(f"    {'/'.join(map(str, e.absolute_path))}: {e.message}")
    say(not errs, "test.json is valid against spec/schema/test-result.schema.json")
    say(tj["ok"] == want_ok, f"test.json ok = {tj['ok']} ({'expected' if want_ok else 'a failing file expected'})")

    # The hashes are the files'.
    bad = []
    for c in tj["cases"]:
        d = out / c["name"]
        for name, h in c["hashes"].items():
            if name == "non-grant-records":
                ev = (d / "events.jsonl").read_bytes().splitlines(keepends=True)
                b = b"".join(x for x in ev if b'"type":"grant"' not in x and b'"type":"assert"' not in x)
            else:
                b = (d / name).read_bytes()
            if sha(b) != h:
                bad.append(f"{c['name']}/{name}")
    say(not bad, f"every output hash in test.json is its file's ({sum(len(c['hashes']) for c in tj['cases'])} files)"
        + (f": {bad}" if bad else ""))

    # junit.xml against the subset, then against test.json.
    xsd = ROOT / "spec/schema/junit-subset.xsd"
    r = subprocess.run(["xmllint", "--noout", "--schema", str(xsd), str(out / "junit.xml")],
                       capture_output=True, text=True)
    say(r.returncode == 0, "junit.xml is valid against spec/schema/junit-subset.xsd (xmllint)"
        + ("" if r.returncode == 0 else ": " + (r.stderr.strip().splitlines() or ["?"])[-1]))
    root = ET.parse(out / "junit.xml").getroot()
    suites = root.findall("testsuite")
    say([s.get("name") for s in suites] == [c["name"] for c in tj["cases"]],
        "junit.xml: one testsuite per case, in the file's order")
    agree = True
    for s, c in zip(suites, tj["cases"]):
        cmps = [k for k in ("golden", "reference", "same_as") if k in c]
        names = [t.get("name") for t in s.findall("testcase")]
        want = ["run"] + [a["name"] for a in c["asserts"]] + cmps
        bad_tc = [t.get("name") for t in s.findall("testcase")
                  if t.find("failure") is not None or t.find("error") is not None]
        want_bad = ([] if c["run"] == "ok" else ["run"]) + [a["name"] for a in c["asserts"] if not a["ok"]] \
            + [k for k in cmps if not c[k]["ok"]]
        n = {k: int(s.get(k)) for k in ("tests", "failures", "errors", "skipped")}
        skipped = sum(1 for t in s.findall("testcase") if t.find("skipped") is not None)
        if names != want or bad_tc != want_bad or n["tests"] != len(names) \
                or n["failures"] + n["errors"] != len(bad_tc) or n["skipped"] != skipped:
            agree = False
            print(f"    {c['name']}: junit {names} / bad {bad_tc} / {n}; test.json {want} / bad {want_bad}")
    tot = {k: int(root.get(k)) for k in ("tests", "failures", "errors", "skipped")}
    sums = {k: sum(int(s.get(k)) for s in suites) for k in tot}
    say(agree and tot == sums, f"junit.xml says what test.json says (a run testcase per case; {tot['tests']} testcases, "
        f"{tot['failures']} failures, {tot['errors']} errors, {tot['skipped']} skipped)")
    print(f"check_bb_test: {'PASS' if not FAILS else 'FAIL'} ({out})")
    return 1 if FAILS else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
