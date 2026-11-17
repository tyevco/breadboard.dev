#!/usr/bin/env bash
# Regression test of scripts/validate_packages.py (make packages): a bb-bench file is validated
# wherever it lives, not only under bench/ or asserts/ (examples/dial-me-spinner/e3/home/bench.toml
# was outside both globs). A root holding an invalid bb-bench/1 file at <root>/e3/home/bench.toml
# must fail and name it; the same file made valid must pass.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
PY=${PYTHON:-python3}
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
mkdir -p "$T/e3/home"
printf 'schema = "bb-bench/1"\ntemp_c = "25"\nfrobnicate = 1\n' > "$T/e3/home/bench.toml"
if "$PY" "$ROOT/scripts/validate_packages.py" "$T" > "$T/out.txt" 2>&1; then
  cat "$T/out.txt"
  echo "validate_packages_test: FAIL: an invalid bb-bench file outside bench/ and asserts/ passed"
  exit 1
fi
if ! grep -q "FAIL .*e3/home/bench.toml" "$T/out.txt"; then
  cat "$T/out.txt"
  echo "validate_packages_test: FAIL: the failure does not name e3/home/bench.toml"
  exit 1
fi
printf 'schema = "bb-bench/1"\ntemp_c = "25"\n' > "$T/e3/home/bench.toml"
if ! "$PY" "$ROOT/scripts/validate_packages.py" "$T" > "$T/out.txt" 2>&1; then
  cat "$T/out.txt"
  echo "validate_packages_test: FAIL: a valid bb-bench file outside bench/ failed"
  exit 1
fi
grep -q "1 files, 0 failed" "$T/out.txt" || { cat "$T/out.txt"; echo "validate_packages_test: FAIL: not checked"; exit 1; }
echo "validate_packages_test: PASS (a bb-bench file outside bench/ and asserts/ is validated)"
