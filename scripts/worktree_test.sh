#!/usr/bin/env bash
# Phase 2 H0 gate: two concurrent track worktrees build without crossing artifacts.
# `make worktree-test` (not in make test: four release builds of bb, several minutes on ext4).
#
# Two throwaway worktrees A and B made by scripts/worktree.sh (under a temporary root on ext4,
# throwaway branches, each with its own local.mk) from HEAD. A gets one distinguishing change: a
# marker static in bbcore. Then:
#   1. `make bb` runs in A and B AT THE SAME TIME, each through its own Makefile and local.mk;
#   2. the target directories differ, and the crate hashes are compared: bbcore's rlib differs, A's
#      holds the marker and B's does not; bbpkg's rlib and each bb binary carry their own
#      worktree's package path (bbpkg compiles CARGO_MANIFEST_DIR in) and never the other's;
#   3. negative control (the mechanism acted): the same two builds into ONE shared target
#      directory, A then B, leave at A's $(BB) a binary with B's package path, the crossing that
#      AGENTS.md warns about and that separate directories prevent.
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
main=$(dirname "$(git -C "$repo" rev-parse --path-format=absolute --git-common-dir)")
MAKE=${MAKE:-make}
unset MAKEFLAGS MFLAGS CARGO_TARGET_DIR BB_SCRATCH
mkdir -p "$HOME/.cache"
tmp=$(mktemp -d "$HOME/.cache/bb-worktree-test.XXXXXX")
id=${tmp##*.}
id=${id,,}
export WORKTREE_ROOT=$tmp/src WORKTREE_CACHE_ROOT=$tmp/cache WORKTREE_BASE
WORKTREE_BASE=$(git -C "$repo" rev-parse HEAD)
ta=wtt-$id-a tb=wtt-$id-b
A=$WORKTREE_ROOT/bb-$ta B=$WORKTREE_ROOT/bb-$tb
cleanup() {
  for w in "$A" "$B"; do
    git -C "$main" worktree unlock "$w" 2>/dev/null || true
    git -C "$main" worktree remove --force "$w" 2>/dev/null || true
  done
  git -C "$main" branch -D "h0-worktree-test/$ta" "h0-worktree-test/$tb" >/dev/null 2>&1 || true
  rm -rf "$tmp"
}
trap cleanup EXIT
fail() { echo "worktree-test: FAIL: $*" >&2; exit 1; }
log() { echo "worktree-test: $*"; }

WORKTREE_BRANCH=h0-worktree-test/$ta bash "$repo/scripts/worktree.sh" "$ta" > "$tmp/wt-a.log"
WORKTREE_BRANCH=h0-worktree-test/$tb bash "$repo/scripts/worktree.sh" "$tb" > "$tmp/wt-b.log"
[[ $(stat -f -c %T "$A") == ext2/ext3 ]] || log "note: $A is on $(stat -f -c %T "$A"), not ext4"
marker=bb-worktree-test-marker-$id
cat >> "$A/bbcore/src/lib.rs" <<EOF

#[doc(hidden)]
#[used]
pub static BB_WORKTREE_TEST_MARKER: [u8; ${#marker}] = *b"$marker";
EOF

TA=$($MAKE -C "$A" -s --no-print-directory print-CARGO_TARGET_DIR)
TB=$($MAKE -C "$B" -s --no-print-directory print-CARGO_TARGET_DIR)
log "A $A -> $TA"
log "B $B -> $TB"
[[ -n $TA && -n $TB && $TA != "$TB" ]] || fail "the worktrees share a target directory ($TA, $TB)"

# 1. Both at once.
t0=$(date +%s)
$MAKE -C "$A" --no-print-directory bb > "$tmp/build-a.log" 2>&1 & pa=$!
$MAKE -C "$B" --no-print-directory bb > "$tmp/build-b.log" 2>&1 & pb=$!
wait $pa || { tail -20 "$tmp/build-a.log" >&2; fail "A's build failed"; }
wait $pb || { tail -20 "$tmp/build-b.log" >&2; fail "B's build failed"; }
log "concurrent builds done in $(($(date +%s) - t0)) s"

# 2. Crate hashes and contents.
one() { local f; f=$(ls "$1"/release/deps/lib"$2"-*.rlib); [[ $(wc -w <<< "$f") == 1 ]] || fail "$1: expected one $2 rlib: $f"; echo "$f"; }
has() { grep -qaF -- "$2" "$1"; }
for c in bbcore bbpkg; do
  ha=$(sha256sum < "$(one "$TA" "$c")" | cut -c1-16)
  hb=$(sha256sum < "$(one "$TB" "$c")" | cut -c1-16)
  log "crate $c: A $ha  B $hb"
done
[[ $(sha256sum < "$(one "$TA" bbcore)") != $(sha256sum < "$(one "$TB" bbcore)") ]] || fail "bbcore's rlib is the same in A and B"
has "$(one "$TA" bbcore)" "$marker" || fail "A's bbcore rlib lacks A's marker"
has "$(one "$TB" bbcore)" "$marker" && fail "B's bbcore rlib holds A's marker"
has "$(one "$TA" bbpkg)" "$A/bbpkg" || fail "A's bbpkg rlib lacks A's package path"
has "$(one "$TA" bbpkg)" "$B/bbpkg" && fail "A's bbpkg rlib holds B's package path"
has "$(one "$TB" bbpkg)" "$B/bbpkg" || fail "B's bbpkg rlib lacks B's package path"
has "$(one "$TB" bbpkg)" "$A/bbpkg" && fail "B's bbpkg rlib holds A's package path"
has "$TA/release/bb" "$A/bbpkg" || fail "A's bb lacks A's package path"
has "$TA/release/bb" "$B/bbpkg" && fail "A's bb holds B's package path"
has "$TB/release/bb" "$B/bbpkg" || fail "B's bb lacks B's package path"
has "$TB/release/bb" "$A/bbpkg" && fail "B's bb holds A's package path"
log "bb: A $(sha256sum < "$TA/release/bb" | cut -c1-16)  B $(sha256sum < "$TB/release/bb" | cut -c1-16), each with its own package path only"

# 3. Negative control: one shared target directory.
S=$tmp/cache/bb-target-shared
$MAKE -C "$A" --no-print-directory bb CARGO_TARGET_DIR="$S" > "$tmp/shared-a.log" 2>&1 || fail "shared build A failed"
has "$S/release/bb" "$A/bbpkg" || fail "control: A's build into the shared directory lacks A's path"
$MAKE -C "$B" --no-print-directory bb CARGO_TARGET_DIR="$S" > "$tmp/shared-b.log" 2>&1 || fail "shared build B failed"
has "$S/release/bb" "$A/bbpkg" && fail "control: the shared directory kept A's binary (no crossing shown)"
has "$S/release/bb" "$B/bbpkg" || fail "control: the shared binary is not B's"
nrl=$(ls "$S"/release/deps/libbbpkg-*.rlib | wc -l)
log "control: with one shared target directory, A's \$(BB) is B's binary after B builds (bbpkg rlibs in it: $nrl)"
echo "worktree-test: PASS (concurrent builds in separate target directories cross nothing; a shared one crosses)"
