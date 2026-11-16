#!/usr/bin/env bash
# Run a command with no network (Phase 2 E1, ADR-011 G8): in a new network namespace that has
# only a loopback interface (brought up, so Renode's External Control and the registry tests'
# local HTTPS server work), as the calling user (a nested user namespace maps the uid and gid
# back, so files and permission checks are the caller's, not root's).
#
#   scripts/offline.sh <command...>
#
# Needs unprivileged user namespaces (WSL2 and most Linux hosts; GitHub's ubuntu-24.04 runners
# restrict them through AppArmor). Exits 2 when they are not available.
set -euo pipefail
if ! unshare -rn true 2>/dev/null; then
    echo "offline: unprivileged user namespaces are not available here" >&2
    exit 2
fi
exec unshare -rn sh -c '
    u=$0 g=$1
    shift
    ip link set lo up
    exec unshare -U --map-user="$u" --map-group="$g" -- "$@"
' "$(id -u)" "$(id -g)" "$@"
