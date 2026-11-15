#!/usr/bin/env bash
# Phase 2 H0 (plan section 7 rule 3): the host measurement lock. Every perf, cost, startup and
# timing target of the Makefile runs under it (MEASURE_TARGETS), so no two measurements share the
# host, whichever checkout or worktree they run from.
#
#   scripts/measure_lock.sh <name> <command> [args...]
#
# A measurement that finds the lock held REFUSES to run (exit 75, EX_TEMPFAIL) instead of waiting:
# a queued measurement would start on a host whose load it did not choose. The lock is
# `flock` on $BB_MEASURE_LOCK (default ~/.cache/bb/measure.lock, one per host); <lock>.owner names
# the holder. The command runs as a child that does not inherit the lock's descriptor, so nothing it
# spawns (a Renode pool) can keep the lock after it ends. A command already under the same lock
# (BB_MEASURE_LOCK_HELD, set here) runs directly, so a measurement may call another.
set -euo pipefail
[[ $# -ge 2 ]] || { echo "usage: $0 <name> <command> [args...]" >&2; exit 2; }
name=$1
shift
lock=${BB_MEASURE_LOCK:-$HOME/.cache/bb/measure.lock}
if [[ ${BB_MEASURE_LOCK_HELD:-} == "$lock" ]]; then
  exec "$@"
fi
mkdir -p "$(dirname "$lock")"
exec 9>>"$lock"
if ! flock -n 9; then
  echo "measure-lock: refusing '$name': the host measurement lock $lock is held by:" >&2
  sed 's/^/measure-lock:   /' "$lock.owner" >&2 2>/dev/null || echo "measure-lock:   (unknown)" >&2
  echo "measure-lock: run it when that measurement has finished (plan section 7 rule 3)" >&2
  exit 75
fi
printf '%s (pid %s) in %s since %s\n' "$name" "$$" "$PWD" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" > "$lock.owner"
export BB_MEASURE_LOCK_HELD=$lock
set +e
"$@" 9>&-
rc=$?
set -e
: > "$lock.owner"
exit $rc
