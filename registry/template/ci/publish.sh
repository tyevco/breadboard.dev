#!/usr/bin/env bash
# The registry's publish step (ADR-011 D8, "On merge"): build the archives and append the index
# lines of every new version under packages/, bump the serial and sign the snapshot when the index
# changed (a new version, a yank). The secret key comes from $MINISIGN_SECRET_KEY (the contents of
# a minisign secret key file without a password) and never touches the repository.
#   ci/publish.sh <bb> <registry dir> <pubtime YYYY-MM-DD>
set -euo pipefail
bb=$1 reg=$2 pubtime=$3
"$bb" registry publish "$reg" --pubtime "$pubtime" "$reg/packages"
if "$bb" registry check "$reg" >/dev/null 2>&1 && [ -f "$reg/snapshot.json.minisig" ]; then
    echo "publish: nothing new"
    exit 0
fi
"$bb" registry snapshot "$reg"
key=$(mktemp)
trap 'rm -f "$key"' EXIT
printf '%s\n' "${MINISIGN_SECRET_KEY:?the registry key (an environment secret)}" >"$key"
serial=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["serial"])' "$reg/snapshot.json")
minisign -S -s "$key" -m "$reg/snapshot.json" -t "bb-registry serial $serial"
"$bb" registry check "$reg"
echo "publish: serial $serial"
