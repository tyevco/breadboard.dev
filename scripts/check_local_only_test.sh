#!/usr/bin/env bash
# Regression test of scripts/check_local_only.sh (ADR-006 Amendment 5, post-M7 review): outside a
# git checkout it must still check the tree, never print "skipped" and exit 0. Synthetic trees in a
# temporary directory (outside any git work tree); part of `make packages`.
#   1. a clean tree (no git)                                      -> PASS
#   2. a part.toml with LicenseRef-local-only (no git)            -> FAIL
#   3. a file under local-packages/ (no git)                      -> FAIL
#   4. the local TI-derived view copied into packages/ (no git)   -> FAIL (only where it exists:
#      `make dms-local-packages`; it is never committed, so CI reports this case as not run)
#   5. a git checkout tracking a LicenseRef-local-only part.toml  -> FAIL
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
T=$(mktemp -d "${TMPDIR:-/tmp}/bb-local-only-test.XXXXXX")
trap 'rm -rf "$T"' EXIT
status=0
mk() { # <dir>: a minimal tree with the script, a normal package and a model file
  mkdir -p "$1/scripts" "$1/packages/a/b/analog"
  cp "$ROOT/scripts/check_local_only.sh" "$1/scripts/"
  printf 'schema = "bb-part/0"\n[part]\nid = "a/b"\nlicense = "Apache-2.0"\n' > "$1/packages/a/b/part.toml"
  printf '* a model\n.subckt X 1 2\nR1 1 2 1k\n.ends X\n' > "$1/packages/a/b/analog/x.lib"
}
expect() { # <name> <pass|fail> <dir>
  local got
  if (cd "$3" && GIT_CEILING_DIRECTORIES=$T bash scripts/check_local_only.sh) > "$T/out.txt" 2>&1; then got=pass; else got=fail; fi
  if [[ $got == "$2" ]] && ! grep -q skipped "$T/out.txt"; then
    echo "local-only-test: PASS: $1 ($got: $(tail -1 "$T/out.txt"))"
  else
    echo "local-only-test: FAIL: $1: expected $2, got $got:"; sed 's/^/    /' "$T/out.txt"; status=1
  fi
}
mk "$T/1"; expect "clean tree, no git" pass "$T/1"
mk "$T/2"; sed -i 's/Apache-2.0/LicenseRef-local-only/' "$T/2/packages/a/b/part.toml"
expect "LicenseRef-local-only part.toml, no git" fail "$T/2"
mk "$T/3"; mkdir -p "$T/3/local-packages/ti"; echo x > "$T/3/local-packages/ti/x.lib"
expect "a file under local-packages/, no git" fail "$T/3"
view=$ROOT/local-packages/ti/tps22917/analog/load_switches__tps22917.lib
if [[ -f $view ]]; then
  mk "$T/4"; cp "$view" "$T/4/packages/a/b/analog/copied.lib"
  expect "the local TI-derived view copied into packages/, no git" fail "$T/4"
  rm -f "$T/4/packages/a/b/analog/copied.lib"
else
  echo "local-only-test: not run: the TI-derived view case (no local-packages/ here; make dms-local-packages)"
fi
mk "$T/5"; sed -i 's/Apache-2.0/LicenseRef-local-only/' "$T/5/packages/a/b/part.toml"
printf 'local-packages/\n' > "$T/5/.gitignore"
(cd "$T/5" && git init -q && git add -A) >/dev/null
expect "LicenseRef-local-only part.toml tracked in git" fail "$T/5"
[[ $status == 0 ]] && echo "local-only-test: PASS" || echo "local-only-test: FAIL"
exit $status
