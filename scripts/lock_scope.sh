#!/usr/bin/env bash
# ADR-015 G5 end to end (Phase 2 H4, D6): editing bb_nrf52_saadc.cs changes
# no STM32 lock.
#
#   scripts/lock_scope.sh <bb> <board or project>...   (make lock-scope)
#
# 1. A second bb ("edited") is built from a copy of this tree in which
#    engines/cpu-renode/platforms/bb_nrf52_saadc.cs has one more comment
#    line, with its own CARGO_TARGET_DIR.
# 2. Every board and project (make's PKG_BOARDS) is copied into scratch,
#    keeping its place under examples/ (so project-local package paths
#    hold). Since milestone S (the schema epoch's relock) the committed locks
#    hold the scoped hashes, so the copies are the committed locks. A second
#    copy is relocked by the stock bb (its renode: platform entries removed,
#    `bb open` adds them again; a project's board.bbm is rewritten with them)
#    and must equal the committed bb.lock and board.bbm byte for byte: the
#    committed entries are what the D6 rule writes.
# 3. The edited bb on the committed copies: every STM32 board and project is
#    current (`bb open --check`), and every nRF52840 one fails with exactly
#    one drift, its renode:nrf52840-bb@3 platform (so the edit took effect).
# 4. Control: the stock bb finds every committed copy current.
#
# Both bbs resolve packages from this tree's packages/
# (BB_FIRST_PARTY_PACKAGES), so only the platform hashes differ. Outputs and
# logs in $BB_SCRATCH/bb-lock-scope.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
BB=$(readlink -f "${1:?usage: lock_scope.sh <bb> <board>...}")
shift
[ $# -gt 0 ] || { echo "lock_scope: no boards given" >&2; exit 2; }
SCR=${BB_SCRATCH:-$HOME/.cache}/bb-lock-scope
EDITED_TARGET=${LOCK_SCOPE_TARGET:-${BB_SCRATCH:-$HOME/.cache}/bb-lock-scope-target}
export BB_FIRST_PARTY_PACKAGES=$ROOT/packages
fail() { echo "lock-scope: FAIL: $*" >&2; exit 1; }
SAADC=engines/cpu-renode/platforms/bb_nrf52_saadc.cs

rm -rf "$SCR"
mkdir -p "$SCR/src" "$SCR/committed" "$SCR/relocked"

# 1. The edited bb (the tree as it is: tracked and untracked, not ignored).
t0=$(date +%s)
git -C "$ROOT" ls-files -z --cached --others --exclude-standard \
    | (cd "$ROOT" && tar --null -T - -cf - 2>/dev/null) | tar -xf - -C "$SCR/src"
printf '// lock-scope: an edit to an nRF-only plugin source (ADR-015 G5)\n' >> "$SCR/src/$SAADC"
cmp -s "$ROOT/$SAADC" "$SCR/src/$SAADC" && fail "the edit did not change $SAADC"
CARGO_TARGET_DIR=$EDITED_TARGET cargo build -q --release -p bb-cli --manifest-path "$SCR/src/Cargo.toml" \
    > "$SCR/build.log" 2>&1 || { cat "$SCR/build.log"; fail "building the edited bb"; }
EDITED=$EDITED_TARGET/release/bb
echo "lock-scope: edited bb built in $(( $(date +%s) - t0 )) s"

# 2. Copies: the committed locks, and a relock by the stock bb.
copy_board() {   # <board or project> <dest root>
    local b=$1 dst=$2 d
    if [ -d "$ROOT/$b" ]; then d=$b; else d=$(dirname "$b"); fi
    mkdir -p "$dst/$d"
    # The directory's files (and, for a project, its subdirectories), so
    # relative package paths and sources inside the project hold.
    (cd "$ROOT" && git ls-files -z --cached --others --exclude-standard -- "$d" \
        | tar --null -T - -cf -) | tar -xf - -C "$dst"
}
# Netlists outside a project directory (dial-me-spinner/system, e3: ../kicad).
mkdir -p "$SCR/committed/examples/dial-me-spinner" "$SCR/relocked/examples/dial-me-spinner"
for b in "$@"; do
    copy_board "$b" "$SCR/committed"
    copy_board "$b" "$SCR/relocked"
done
for dst in "$SCR/committed" "$SCR/relocked"; do
    (cd "$ROOT" && git ls-files -z -- examples/dial-me-spinner/kicad | tar --null -T - -cf -) \
        | tar -xf - -C "$dst"
done

lock_of() { if [ -d "$1" ]; then echo "$1/bb.lock"; else echo "$(dirname "$1")/bb.lock"; fi; }
platform_of() { grep -o 'renode:[a-z0-9-]*@[0-9]*' "$(lock_of "$1")" | sort -u | tr '\n' ' '; }

for b in "$@"; do
    l=$(lock_of "$SCR/relocked/$b")
    grep -q 'schema = "bb-lock/1"' "$(lock_of "$ROOT/$b")" || fail "$b: the committed lock is not bb-lock/1"
    python3 - "$l" <<'EOF'
import sys
p = sys.argv[1]
t = open(p).read()
head, *blocks = t.split("\n[[")
keep = [b for b in blocks if not (b.startswith("platform]]") and 'id = "renode:' in b)]
open(p, "w").write("\n[[".join([head] + keep))
EOF
    "$BB" open "$SCR/relocked/$b" > "$SCR/relock-$(echo "$b" | tr / _).log" 2>&1 \
        || { cat "$SCR/relock-$(echo "$b" | tr / _).log"; fail "$b: relocking the copy"; }
    grep -q 'id = "renode:' "$l" || fail "$b: no renode platform after the relock"
    cmp -s "$l" "$(lock_of "$ROOT/$b")" \
        || { diff "$(lock_of "$ROOT/$b")" "$l" || true; fail "$b: the relock differs from the committed bb.lock"; }
    if [ -d "$ROOT/$b" ]; then
        cmp -s "$SCR/relocked/$b/board.bbm" "$ROOT/$b/board.bbm" \
            || fail "$b: the relock's board.bbm differs from the committed one"
    fi
done

# 3. and 4.
n_stm=0; n_nrf=0
for b in "$@"; do
    tag=$(echo "$b" | tr / _)
    plats=$(platform_of "$SCR/committed/$b")
    "$BB" open --check "$SCR/committed/$b" > "$SCR/stock-$tag.log" 2>&1 \
        || { cat "$SCR/stock-$tag.log"; fail "$b: the stock bb finds the committed lock stale"; }
    if "$EDITED" open --check "$SCR/committed/$b" > "$SCR/edited-$tag.log" 2>&1; then rc=0; else rc=1; fi
    case "$plats" in
        *nrf52840*)
            [ "$rc" = 1 ] || fail "$b ($plats): the edited bb accepts the nRF lock"
            drifts=$(grep -c '^bb open: \(platform\|package\) ' "$SCR/edited-$tag.log" || true)
            grep -q '^bb open: platform renode:nrf52840-bb@3: locked content' "$SCR/edited-$tag.log" \
                && [ "$drifts" = 1 ] \
                || { cat "$SCR/edited-$tag.log"; fail "$b: expected exactly the nRF platform's drift"; }
            n_nrf=$((n_nrf + 1))
            echo "lock-scope: $b ($plats): nRF lock changes (1 drift: renode:nrf52840-bb@3)"
            ;;
        *stm32*)
            [ "$rc" = 0 ] || { cat "$SCR/edited-$tag.log"; fail "$b ($plats): the edit to $SAADC changed an STM32 lock"; }
            n_stm=$((n_stm + 1))
            echo "lock-scope: $b ($plats): unchanged under the edit"
            ;;
        *) fail "$b: no Renode platform ($plats)";;
    esac
done
[ "$n_stm" -gt 0 ] && [ "$n_nrf" -gt 0 ] || fail "need STM32 and nRF boards ($n_stm, $n_nrf)"
echo "lock-scope: PASS ($n_stm STM32 boards and projects unchanged, $n_nrf nRF52840 ones changed; every committed lock is what the D6 relock writes)"
