#!/usr/bin/env bash
# Run a command under a warm Renode pool (ADR-004 Part B): `bb pool run`,
# unless BB_RENODE_POOL already names a live pool (a nested make), in which
# case the command joins that pool.
#
# Usage: scripts/with_pool.sh <bb> <slots> -- <command> [args...]
set -euo pipefail
bb=$1 slots=$2
shift 2
[[ ${1:-} == -- ]] && shift
if [[ -n ${BB_RENODE_POOL:-} && -S $BB_RENODE_POOL ]]; then
  exec "$@"
fi
exec "$bb" pool run --slots "$slots" -- "$@"
