#!/usr/bin/env bash
# Phase 2 E1 (ADR-011 G7, approver decision: local only): make or update the LOCAL public registry
# repository, a git repository with every first-party package published and signed, ready for
# the human to push (docs/registry-publish.md). Nothing here pushes, adds a remote or touches
# GitHub.
#
#   scripts/registry_repo.sh <bb> [<dir>]     default dir: $BB_REGISTRY_REPO or ~/src/bb-registry
#
# - The files of registry/template/ (README, owners.toml, the CI workflows and scripts) are
#   copied in, with BB_REV = this checkout's HEAD.
# - packages/ becomes a copy of breadboard.dev's first-party packages/ (the PR source layout).
# - ci/publish.sh, the registry's own merge step, publishes every new version and signs the
#   snapshot with the key in $BB_REGISTRY_KEYS (default ~/.config/bb-registry: bb-registry.key,
#   bb-registry.pub; made once with `minisign -G -W`, never committed). The public key must be
#   the one compiled into bb (cli/src/registry.rs PUBLIC_KEY).
# - The result is committed on the repository's main branch (local, as user "breadboard").
set -euo pipefail
BB=$(readlink -f "$1")
REPO=$(cd "$(dirname "$0")/.." && pwd)
D=${2:-${BB_REGISTRY_REPO:-$HOME/src/bb-registry}}
KEYS=${BB_REGISTRY_KEYS:-$HOME/.config/bb-registry}
[ -f "$KEYS/bb-registry.key" ] && [ -f "$KEYS/bb-registry.pub" ] || {
    echo "registry-repo: no key in $KEYS; make one: mkdir -p $KEYS && minisign -G -W -p $KEYS/bb-registry.pub -s $KEYS/bb-registry.key"
    exit 1
}
PUB=$(tail -1 "$KEYS/bb-registry.pub")
COMPILED=$(sed -n 's/^pub const PUBLIC_KEY: &str = "\(.*\)";/\1/p' "$REPO/cli/src/registry.rs")
if [ "$PUB" != "$COMPILED" ]; then
    echo "registry-repo: $KEYS/bb-registry.pub ($PUB) is not the key compiled into bb ($COMPILED)"
    exit 1
fi
mkdir -p "$D"
[ -d "$D/.git" ] || git -C "$D" init -q -b main
cp -a "$REPO/registry/template/." "$D/"
git -C "$REPO" rev-parse HEAD >"$D/BB_REV"
[ -f "$D/config.json" ] || "$BB" registry init "$D" --key "$KEYS/bb-registry.pub"
rm -rf "$D/packages"
mkdir -p "$D/packages"
cp -a "$REPO/packages/." "$D/packages/"
find "$D/packages" -path '*/tests/out' -prune -exec rm -rf {} +
PUBTIME=$(git -C "$REPO" log -1 --format=%cs)
MINISIGN_SECRET_KEY=$(cat "$KEYS/bb-registry.key") bash "$D/ci/publish.sh" "$BB" "$D" "$PUBTIME"
git -C "$D" add -A
if ! git -C "$D" diff --cached --quiet; then
    serial=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["serial"])' "$D/snapshot.json")
    git -C "$D" -c user.name=breadboard -c user.email=tyler@MSI.localdomain commit -q \
        -m "publish: serial $serial (breadboard.dev $(cut -c1-7 "$D/BB_REV"), every first-party package)"
fi
echo "registry-repo: $D at $(git -C "$D" log -1 --format='%h %s')"
