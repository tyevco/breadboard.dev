#!/usr/bin/env bash
# Check examples/dial-me-spinner/SOURCE (Phase 1 plan, gate M0):
#   - every entry names a full 40-hex commit and a path;
#   - every vendored file exists and has the recorded sha256;
#   - with a dial-me-spinner repository as $1 (or DMS_REPO), every file
#     also equals `git show <commit>:<path>` byte for byte;
#   - no file under kicad/ is missing from SOURCE;
#   - every package file migrated from dial-me-spinner (a part.toml `origin`) has an entry.
# Usage: scripts/dms_source_check.sh [<dial-me-spinner repo>]
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
DIR=$ROOT/examples/dial-me-spinner
SRC=$DIR/SOURCE
REPO=${1:-${DMS_REPO:-}}
fail=0 n=0

[[ -f $SRC ]] || { echo "dms-source: $SRC missing" >&2; exit 1; }
grep -q '^# reference   [0-9a-f]\{40\}$' "$SRC" || { echo "dms-source: no reference commit line" >&2; exit 1; }

listed=$(mktemp)
trap 'rm -f "$listed"' EXIT
while read -r sha path origin; do
  [[ -z ${sha:-} || $sha == \#* ]] && continue
  n=$((n + 1))
  echo "$path" >> "$listed"
  commit=${origin%%:*} src=${origin#*:}
  if [[ ! $sha =~ ^[0-9a-f]{64}$ || ! $commit =~ ^[0-9a-f]{40}$ || -z $src ]]; then
    echo "dms-source: malformed entry: $sha $path $origin" >&2; fail=1; continue
  fi
  if [[ ! -f $DIR/$path ]]; then
    echo "dms-source: $path missing" >&2; fail=1; continue
  fi
  got=$(sha256sum "$DIR/$path" | cut -d' ' -f1)
  if [[ $got != "$sha" ]]; then
    echo "dms-source: $path sha256 $got, SOURCE says $sha" >&2; fail=1
  fi
  if [[ -n $REPO ]]; then
    up=$(git -C "$REPO" show "$commit:$src" | sha256sum | cut -d' ' -f1)
    if [[ $up != "$sha" ]]; then
      echo "dms-source: $path differs from $commit:$src ($up)" >&2; fail=1
    fi
  fi
done < "$SRC"

while IFS= read -r f; do
  rel=${f#"$DIR"/}
  grep -qxF "$rel" "$listed" || { echo "dms-source: $rel is not in SOURCE" >&2; fail=1; }
done < <(find "$DIR/kicad" -type f | sort)

# Every package file migrated from dial-me-spinner (part.toml `origin = "dial-me-spinner@<commit>:
# <path>"`) is vendored with a SOURCE entry of that commit and path (ADR-006 Amendment 5).
while IFS= read -r o; do
  oc=${o%%:*} op=${o#*:}
  if ! awk -v c="$oc" -v p="$op" '$1 !~ /^#/ && NF == 3 { split($3, a, ":"); if (index(a[1], c) == 1 && a[2] == p) f = 1 }
       END { exit !f }' "$SRC"; then
    echo "dms-source: a package's origin dial-me-spinner@$o has no SOURCE entry" >&2; fail=1
  else
    n_origin=$((${n_origin:-0} + 1))
  fi
done < <(grep -rhoE --include=part.toml 'origin = "dial-me-spinner@[0-9a-f]+:[^"]+"' "$ROOT/packages" \
           | sed -E 's/^origin = "dial-me-spinner@([0-9a-f]+:[^"]+)"$/\1/' | sort -u)

if [[ $fail -ne 0 ]]; then
  echo "dms-source: FAIL" >&2; exit 1
fi
echo "dms-source: OK ($n files$( [[ -n $REPO ]] && echo ', each equal to its commit:path in the repository'); ${n_origin:-0} package origin(s) listed)"
