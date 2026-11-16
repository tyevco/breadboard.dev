#!/usr/bin/env bash
# The registry's pull-request checks (ADR-011 D8, "On a PR").
#   ci/pr-check.sh <bb> <head dir> <base dir> <author> [approver...]
# 1. schema validation (bb's strict reader; the JSON schemas of breadboard.dev's spec/schema)
# 2. bb pkg lint --publish and bb pkg test on the changed packages
# 3. the licence gate (inside lint --publish and publish, ADR-011 D7)
# 4. the provenance gate (ADR-012): reviewed_by and unverified_claims are CI's; the claim checker
#    is milestone E2 and is not run yet
# 5. pins against the KiCad symbol: milestone E3 (no symbol library in this CI yet)
# 6. the size cap (2 MB per archive), 7. the index diff check, 8. owners.toml
set -euo pipefail
bb=$1 head=$2 base=$3 author=$4
shift 4
approvers=()
for a in "$@"; do approvers+=(--approver "$a"); done
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
cp -a "$head/." "$scratch/"
mapfile -t changed < <(cd "$head" && git diff --name-only "$(git -C "$base" rev-parse HEAD)" -- packages \
    | awk -F/ 'NF >= 4 { print $1 "/" $2 "/" $3 }' | sort -u)
for d in "${changed[@]}"; do
    [ -f "$head/$d/part.toml" ] || { echo "pr-check: $d: removed packages are not deleted here (ADR-011 D2)"; exit 1; }
done
if [ ${#changed[@]} -gt 0 ]; then
    dirs=()
    for d in "${changed[@]}"; do dirs+=("$scratch/$d"); done
    "$bb" pkg lint --publish "${dirs[@]}"
    "$bb" pkg test "${dirs[@]}"
    "$bb" registry publish "$scratch" --pubtime "$(date -u +%F)" "${dirs[@]}"
fi
"$bb" registry check "$scratch" --base "$base" --author "$author" "${approvers[@]}"
echo "pr-check: PASS (${#changed[@]} changed package(s))"
