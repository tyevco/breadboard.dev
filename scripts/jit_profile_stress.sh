#!/usr/bin/env bash
# ADR-004 Amendment 9: the startup JIT profile's gather under load (`make jit-profile-stress`).
#
#   bash scripts/jit_profile_stress.sh <bb> [rounds] [parallel] [burners]
#
# Each round starts <parallel> gathering `bb run`s of the spike (20 ms) at once, each with its own
# empty BB_CACHE_DIR, while <burners> busy loops load every CPU (default 2 x nproc). Every run must
# succeed and leave exactly one `renode-*.prof` and nothing else in its cache's renode-jit/: a
# Renode that aborted at the SIGTERM (the race of the External Control server's restart with the
# shutdown) or was killed leaves none. Prints one line per failure and a summary; exits non-zero on
# any failure. Before the fix, 2 of 32 runs at 4 x 8 with 48 burners left no profile.
set -uo pipefail
BB=$(realpath "${1:?usage: jit_profile_stress.sh <bb> [rounds] [parallel] [burners]}")
ROUNDS=${2:-4}
PAR=${3:-8}
LOAD=${4:-$((2 * $(nproc)))}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
make -s -C examples/spike/firmware
ELF=$ROOT/examples/spike/firmware/build/blinky.elf
W=$(mktemp -d "${TMPDIR:-/tmp}/bb-jit-stress.XXXXXX")
burners=()
cleanup() {
  ((${#burners[@]})) && kill "${burners[@]}" 2> /dev/null
  rm -rf "$W"
}
trap cleanup EXIT
for ((i = 0; i < LOAD; i++)); do
  (while :; do :; done) &
  burners+=($!)
done
pass=0 fail=0
for ((r = 0; r < ROUNDS; r++)); do
  pids=()
  for ((p = 0; p < PAR; p++)); do
    d=$W/$r-$p
    mkdir -p "$d"
    (
      unset BB_RENODE_POOL BB_RENODE_STOCK_RUNTIME BB_RENODE_JIT_PROFILE
      export BB_CACHE_DIR=$d/cache
      exec "$BB" run --netlist examples/spike/board.cir --firmware "$ELF" --target U1 --for 20ms \
        --probe net:LED_A --out "$d/out" > "$d/log" 2>&1
    ) &
    pids+=($!)
  done
  for ((p = 0; p < PAR; p++)); do
    d=$W/$r-$p
    if ! wait "${pids[$p]}"; then
      fail=$((fail + 1))
      echo "jit-profile-stress: FAIL: round $r run $p: bb run failed"
      tail -5 "$d/log"
      continue
    fi
    left=$(ls -A "$d/cache/renode-jit" 2> /dev/null | tr '\n' ' ')
    if [[ $left =~ ^renode-[0-9a-f]{16}_[^\ ]*\.prof\ $ ]]; then
      pass=$((pass + 1))
    else
      fail=$((fail + 1))
      echo "jit-profile-stress: FAIL: round $r run $p: renode-jit/ holds [${left% }], want one profile"
      grep 'wall time' "$d/log"
    fi
    rm -rf "$d"
  done
done
echo "jit-profile-stress: $pass of $((ROUNDS * PAR)) gathers left one profile ($ROUNDS rounds x $PAR parallel, $LOAD burners, load $(cut -d' ' -f1-3 /proc/loadavg))"
[[ $fail == 0 ]] && echo "jit-profile-stress: PASS" || { echo "jit-profile-stress: FAIL"; exit 1; }
