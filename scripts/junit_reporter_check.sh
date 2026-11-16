#!/usr/bin/env bash
# Milestone B3's prototype check of the JUnit reporter (ADR-010 D9, G5; Amendment 5): bb test's
# junit.xml read by the parser of the reporter action CI uses, dorny/test-reporter at v3.2.0 (commit
# d1b5dff021aa968b288b02285a8bf47471c5928a, the commit .github/workflows/ci.yml pins), reporter
# java-junit. A GitHub run of the action needs a push (and writes a check run); this runs the action's
# own parser and report renderer, from its source at that commit, locally, in a node container (podman
# or docker), so nothing is published.
#
#   scripts/junit_reporter_check.sh <junit.xml>...
#
# For each file: the parser's test run (JavaJunitParser.parse) must have exactly bb test's testsuites
# (names, in order), their testcases (names, in order) and their counts (tests, failed = failures +
# errors, skipped), and getReport must render a non-empty report naming every suite (written next to
# the copies, <cache>/run/<k>.md). Needs the network once: the clone, the image and `npm ci` are cached
# in $BB_JUNIT_REPORTER_CACHE (default ~/.cache/bb-junit-reporter).
set -euo pipefail
[[ $# -ge 1 ]] || { sed -n '2,20p' "$0"; exit 2; }
SHA=d1b5dff021aa968b288b02285a8bf47471c5928a
IMAGE=${BB_JUNIT_REPORTER_IMAGE:-docker.io/library/node:24-bookworm-slim}
CACHE=${BB_JUNIT_REPORTER_CACHE:-$HOME/.cache/bb-junit-reporter}
ENGINE=$(command -v podman || command -v docker || true)
[[ -n $ENGINE ]] || { echo "junit-reporter: FAIL: no podman or docker"; exit 1; }
src=$CACHE/test-reporter-$SHA
mkdir -p "$CACHE"
if [[ ! -d $src/.git ]]; then
  git clone --quiet https://github.com/dorny/test-reporter "$src"
fi
git -C "$src" checkout --quiet --detach "$SHA"
[[ $(git -C "$src" rev-parse HEAD) == "$SHA" ]] || { echo "junit-reporter: FAIL: not at $SHA"; exit 1; }
run=$CACHE/run
rm -rf "$run" && mkdir -p "$run"
k=0
for f in "$@"; do
  k=$((k + 1))
  cp "$f" "$run/$k.xml"
  echo "$k $f" >> "$run/files.txt"
done
# What bb test wrote, read with Python's parser: the expectation the action's parser must meet.
python3 - "$run" <<'EOF'
import json, sys, xml.etree.ElementTree as ET
from pathlib import Path
run = Path(sys.argv[1])
exp = []
for x in sorted(run.glob("*.xml"), key=lambda p: int(p.stem)):
    root = ET.parse(x).getroot()
    suites = []
    for s in root.findall("testsuite"):
        tcs = s.findall("testcase")
        suites.append({"name": s.get("name"), "tests": [t.get("name") for t in tcs]})
    tcs = root.iter("testcase")
    tc = list(root.iter("testcase"))
    exp.append({"file": x.name, "suites": suites, "tests": len(tc),
                "failed": sum(1 for t in tc if t.find("failure") is not None or t.find("error") is not None),
                "skipped": sum(1 for t in tc if t.find("skipped") is not None)})
(run / "expected.json").write_text(json.dumps(exp, indent=1))
EOF
cat > "$src/__tests__/bb-junit.test.ts" <<'EOF'
// Written by breadboard's scripts/junit_reporter_check.sh (not part of dorny/test-reporter).
import * as fs from 'fs'
import {JavaJunitParser} from '../src/parsers/java-junit/java-junit-parser.js'
import {getReport} from '../src/report/get-report.js'

const run = '/bbrun'
const expected = JSON.parse(fs.readFileSync(`${run}/expected.json`, {encoding: 'utf8'}))

describe('bb test junit.xml through java-junit', () => {
  for (const e of expected) {
    it(e.file, async () => {
      const content = fs.readFileSync(`${run}/${e.file}`, {encoding: 'utf8'})
      const result = await new JavaJunitParser({parseErrors: true, trackedFiles: []}).parse(e.file, content)
      const got = {
        suites: result.suites.map(s => ({name: s.name, tests: s.groups.flatMap(g => g.tests.map(t => t.name))})),
        tests: result.tests,
        failed: result.failed,
        skipped: result.skipped
      }
      fs.writeFileSync(`${run}/${e.file}.parsed.json`, JSON.stringify(got, null, 1))
      expect(got).toEqual({suites: e.suites, tests: e.tests, failed: e.failed, skipped: e.skipped})
      const md = getReport([result])
      fs.writeFileSync(`${run}/${e.file}.md`, md)
      expect(md.length).toBeGreaterThan(0)
      for (const s of e.suites) expect(md).toContain(s.name)
    })
  }
})
EOF
# shellcheck disable=SC2016
if "$ENGINE" run --rm -v "$src:/src" -v "$run:/bbrun" -w /src "$IMAGE" sh -c '
    if [ ! -d node_modules ]; then npm ci --no-audit --no-fund --loglevel=error >/bbrun/npm.log 2>&1 || { tail -20 /bbrun/npm.log; exit 1; }; fi
    NODE_OPTIONS=--experimental-vm-modules npx jest --ci __tests__/bb-junit.test.ts' > "$run/jest.log" 2>&1; then
  ok=1
else
  ok=0
fi
grep -E '✓|✕|Tests:' "$run/jest.log" | sed 's/^/junit-reporter:   /'
while read -r n f; do
  python3 - "$run/$n.xml.parsed.json" "$f" <<'EOF' 2>/dev/null || true
import json, sys
g = json.load(open(sys.argv[1]))
print(f"junit-reporter:   {sys.argv[2]}: {len(g['suites'])} suites, {g['tests']} tests, {g['failed']} failed, "
      f"{g['skipped']} skipped (dorny/test-reporter java-junit)")
EOF
done < "$run/files.txt"
if [[ $ok == 1 ]]; then
  echo "junit-reporter: PASS: dorny/test-reporter@${SHA:0:7} (v3.2.0) java-junit reads $# junit.xml as bb test wrote them"
else
  echo "junit-reporter: FAIL (see $run/jest.log)"; tail -30 "$run/jest.log"; exit 1
fi
