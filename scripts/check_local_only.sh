#!/usr/bin/env bash
# Phase 1 plan approver decision A3 (ADR-006 D9 rule 4, Amendment 3): nothing LOCAL-ONLY is committed.
#  - no tracked part.toml carries license LicenseRef-local-only;
#  - git tracks nothing under local-packages/, and git ignores that directory;
#  - no tracked file has the bytes of a known TI-derived file (the suite's TPS22917 view, its S1-fixed
#    form, or the local package's view), by sha256.
# Outside a git checkout (a `git archive` export or a tarball) there is nothing "tracked", so the
# same checks run on every file of the tree except local-packages/ (ADR-006 Amendment 5: the check
# used to print "skipped" and exit 0 there, which read as a pass).
# Part of `make packages` (and so of `make test`). Needs only git, or without git, find.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
fail() { echo "local-only-check: FAIL: $*" >&2; exit 1; }
# sha256 of: hardware/sim/models/load_switches__tps22917.lib at 6854d25 (TI-derived), its S1-fixed form
# (scripts/dms_s1.py fixview), and the local package's view (the S1 fix plus the GND -> GNDP rename).
known="b487750ff2434ed638e7bcb0f8d4d1ade73f7bb638b87354bf22873641af4e18
061285e8bb854529fc2c77ec122bcea58e49eaecaf9e9c30f5881b5d2fa4a4ef
f754ea7712096e1b15deb8a726d233174e02d76401ef7595d770b140ad2b9bdf"
MODELS=('*.lib' '*.cir' '*.sp' '*.spi' '*.spice' '*.inc' '*.mod' '*.txt')

if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  scope="tracked"
  list_parts() { git ls-files -z -- '*part.toml'; }
  list_models() { git ls-files -z -- "${MODELS[@]}"; }
  [[ -z $(git ls-files -- local-packages) ]] || fail "git tracks files under local-packages/"
  git check-ignore -q local-packages/x || fail "local-packages/ is not git-ignored"
else
  # No git: every file of the tree is what would be distributed, except local-packages/ (which
  # a tarball must not contain either) and build output.
  scope="in this tree (not a git checkout)"
  [[ ! -e local-packages ]] || [[ -z $(find local-packages -type f -print -quit) ]] \
    || fail "not a git checkout, and local-packages/ holds files: a tree to distribute must not"
  prune=(-path ./local-packages -o -path ./target -o -path ./out -o -path ./.bb)
  list_parts() { find . \( "${prune[@]}" \) -prune -o -type f -name part.toml -print0; }
  list_models() {
    local args=() m
    for m in "${MODELS[@]}"; do args+=(-o -name "$m"); done
    find . \( "${prune[@]}" \) -prune -o -type f \( -false "${args[@]}" \) -print0
  }
fi
bad=$(list_parts | xargs -0 -r grep -l 'license *= *"LicenseRef-local-only"' || true)
[[ -z $bad ]] || fail "$scope part.toml with LicenseRef-local-only: $bad"
hits=$(list_models | xargs -0 -r sha256sum 2>/dev/null | grep -F -f <(echo "$known") || true)
[[ -z $hits ]] || fail "a file $scope is a TI-derived file: $hits"
echo "local-only-check: PASS ($scope: no LicenseRef-local-only package, nothing under local-packages/, no TI-derived file)"
