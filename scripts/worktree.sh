#!/usr/bin/env bash
# Phase 2 H0 (plan section 7 rule 1): one git worktree per track, on ext4, with its own build and
# scratch directories. `make worktree TRACK=<name>` runs it.
#
#   scripts/worktree.sh <track>
#
# Creates (or, if it exists, re-configures) the worktree $WORKTREE_ROOT/bb-<track> (default
# ~/src/bb-<track>) on the branch p2/<track> ($WORKTREE_BRANCH), starting from $WORKTREE_BASE
# (default main) when the branch is new. It writes the worktree's git-ignored local.mk, which the
# Makefile includes first:
#
#   CARGO_TARGET_DIR  $WORKTREE_CACHE_ROOT/bb-target-wt-<track>  (two checkouts sharing a target
#                     directory link each other's crates; AGENTS.md, WSL notes)
#   BB_SCRATCH        $WORKTREE_CACHE_ROOT/bb-wt-<track>  (the project copies and work directories
#                     of the gates, e.g. bb-dms-rail, that a run rewrites)
#   DMS_REPO          the main checkout's ../dial-me-spinner, if it exists (only read, as ever)
#
# WORKTREE_CACHE_ROOT defaults to ~/.cache. Content-keyed caches stay shared between checkouts:
# ~/.cache/bb (the Renode plugin assembly and JIT profiles, keyed by content), bb-dms-suite (the
# suite venvs, keyed by the requirements hash) and bb-zephyr (the SDK downloads).
#
# The worktree is locked (`git worktree lock`): the Windows checkout cannot see an ext4 path, and a
# `git worktree prune` there would otherwise delete the track's metadata. `make worktree-remove
# TRACK=<name>` unlocks and removes it (the branch is kept).
set -euo pipefail

usage() { echo "usage: $0 <track> | --remove <track>" >&2; exit 2; }
remove=0
if [[ ${1:-} == --remove ]]; then remove=1; shift; fi
track=${1:-}
[[ -n $track ]] || usage
[[ $track =~ ^[a-z0-9][a-z0-9-]*$ ]] || { echo "worktree: TRACK must match [a-z0-9][a-z0-9-]* (got '$track')" >&2; exit 2; }

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# The main checkout (the common git directory's parent), also when run from a track worktree.
common=$(git -C "$repo" rev-parse --path-format=absolute --git-common-dir)
main=$(dirname "$common")
root=${WORKTREE_ROOT:-$HOME/src}
cache_root=${WORKTREE_CACHE_ROOT:-$HOME/.cache}
branch=${WORKTREE_BRANCH:-p2/$track}
base=${WORKTREE_BASE:-main}
wt=$root/bb-$track

if ((remove)); then
  git -C "$main" worktree unlock "$wt" 2>/dev/null || true
  git -C "$main" worktree remove "$wt"
  echo "worktree: removed $wt (branch $branch kept; target $cache_root/bb-target-wt-$track and scratch $cache_root/bb-wt-$track kept)"
  exit 0
fi

case $(stat -f -c %T "$(dirname "$root")" 2>/dev/null || echo unknown) in
  v9fs|9p|drvfs|fuseblk) echo "worktree: $root is on a Windows drive; a track worktree belongs on ext4 (plan section 7)" >&2
                         exit 2 ;;
esac

mkdir -p "$root"
if git -C "$main" worktree list --porcelain | grep -qxF "worktree $wt"; then
  echo "worktree: $wt exists; rewriting its local.mk"
else
  if git -C "$main" show-ref --verify --quiet "refs/heads/$branch"; then
    git -C "$main" worktree add "$wt" "$branch"
  else
    git -C "$main" worktree add -b "$branch" "$wt" "$base"
  fi
fi
git -C "$main" worktree lock --reason "bb track worktree on ext4 ($track); make worktree-remove TRACK=$track" "$wt" \
  2>/dev/null || true

dms=$(cd "$main/.." && pwd)/dial-me-spinner
{
  echo "# Written by make worktree TRACK=$track (Phase 2 H0); git-ignored. Included first by the Makefile."
  echo "BB_TRACK := $track"
  echo "CARGO_TARGET_DIR := $cache_root/bb-target-wt-$track"
  echo "BB_SCRATCH := $cache_root/bb-wt-$track"
  if [[ -d $dms/.git || -f $dms/.git ]]; then
    echo "DMS_REPO := $dms"
    echo "export DMS_REPO"
  fi
} > "$wt/local.mk"
mkdir -p "$cache_root/bb-wt-$track"

echo "worktree: $wt on branch $(git -C "$wt" rev-parse --abbrev-ref HEAD) at $(git -C "$wt" rev-parse --short HEAD)"
sed -n '2,$p' "$wt/local.mk" | sed 's/^/worktree:   /'
echo "worktree: for cargo outside make there: export CARGO_TARGET_DIR=$cache_root/bb-target-wt-$track"
