#!/usr/bin/env bash
# Phase 2 H0 gate (in make test, a few seconds): measurement targets refuse to run while another
# measurement holds the host measurement lock (scripts/measure_lock.sh). On a private lock file,
# so a real measurement elsewhere on the host is neither disturbed nor able to disturb this test.
#
#   1. a free lock runs the command, passes its exit status, and the command does not inherit the
#      lock's descriptor; a measurement under the lock can run a nested one;
#   2. `make measure-lock-probe` (a MEASURE_TARGETS member) runs when the lock is free;
#   3. while a probe holds the lock (checked: an outside `flock -n` fails, so the mechanism acted),
#      EVERY member of MEASURE_TARGETS refuses with exit status != 0 and the refusal message, within
#      a few seconds and before building anything;
#   4. after the holder releases it, the probe runs again.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
MAKE=${MAKE:-make}
tmp=$(mktemp -d)
holder=
cleanup() {
  [[ -z $holder ]] || { touch "$tmp/release"; wait "$holder" 2>/dev/null || true; }
  rm -rf "$tmp"
}
trap cleanup EXIT
export BB_MEASURE_LOCK=$tmp/measure.lock
unset BB_MEASURE_LOCK_HELD MAKEFLAGS MFLAGS
fail() { echo "measure-lock-test: FAIL: $*" >&2; exit 1; }

# 1. The helper itself.
bash scripts/measure_lock.sh t1 true || fail "a free lock did not run the command"
set +e
bash scripts/measure_lock.sh t2 sh -c 'exit 3'
rc=$?
set -e
[[ $rc == 3 ]] || fail "the command's exit status 3 came back as $rc"
bash scripts/measure_lock.sh t3 sh -c '! [ -e /proc/self/fd/9 ]' || fail "the command inherited the lock descriptor"
bash scripts/measure_lock.sh outer bash scripts/measure_lock.sh inner true || fail "a nested measurement was refused"

# 2. The probe target with the lock free.
out=$($MAKE -s --no-print-directory measure-lock-probe)
[[ $out == *"measure-lock-probe: ran"* ]] || fail "the probe did not run with the lock free: $out"

# 3. A probe holds the lock until $tmp/release exists.
$MAKE -s --no-print-directory measure-lock-probe PROBE_HOLD="$tmp/release" > "$tmp/holder.log" 2>&1 &
holder=$!
for _ in $(seq 100); do
  if ! flock -n "$BB_MEASURE_LOCK" true 2>/dev/null; then break; fi
  sleep 0.1
done
flock -n "$BB_MEASURE_LOCK" true 2>/dev/null && fail "the holder never took the lock (the control did not act)"
grep -q "measure-lock-probe" "$BB_MEASURE_LOCK.owner" || fail "the lock's owner file does not name the holder"

targets=$($MAKE -s --no-print-directory print-MEASURE_TARGETS)
n=0
for t in $targets; do
  t0=$(date +%s%N)
  set +e
  $MAKE --no-print-directory "$t" > "$tmp/$t.log" 2>&1
  rc=$?
  set -e
  ms=$((($(date +%s%N) - t0) / 1000000))
  [[ $rc != 0 ]] || fail "$t ran while the lock was held (exit 0)"
  grep -q "measure-lock: refusing '$t'" "$tmp/$t.log" || { cat "$tmp/$t.log" >&2; fail "$t failed without the refusal"; }
  if grep -qE 'Compiling|cargo (build|test)|arm-none-eabi-gcc' "$tmp/$t.log"; then
    cat "$tmp/$t.log" >&2
    fail "$t built something before refusing"
  fi
  ((ms < 15000)) || fail "$t took $ms ms to refuse"
  n=$((n + 1))
done
((n >= 10)) || fail "only $n measurement targets listed"

# 4. Release; the probe runs again.
touch "$tmp/release"
wait "$holder" || fail "the holder probe failed: $(cat "$tmp/holder.log")"
holder=
out=$($MAKE -s --no-print-directory measure-lock-probe)
[[ $out == *"measure-lock-probe: ran"* ]] || fail "the probe did not run after the release: $out"
echo "measure-lock-test: PASS ($n measurement targets refused while the lock was held; the lock was really held; free before and after)"
