#!/usr/bin/env bash
# Vendor the dial-me-spinner reference KiCad inputs into examples/dial-me-spinner
# (Phase 1 plan M0, approver decisions A and C) and write its SOURCE file.
#
#   scripts/dms_vendor.sh <dial-me-spinner repo>
#
# Reads the repository only through `git archive <commit> <paths>`: it never
# checks out, stashes or touches the working tree (the user works there).
# The reference is 6854d25 (branch kicad10-port, KiCad 10.0.8); dc96515
# (main, KiCad 9.0.9) contributes only its two netlists, for the secondary
# import check of gate M4c. scripts/dms_source_check.sh verifies the result.
set -euo pipefail

REPO=${1:?usage: dms_vendor.sh <dial-me-spinner repo>}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
DEST=$ROOT/examples/dial-me-spinner
REF=6854d25e0fde9b5d60da07b2c2b6183fd36dfc5a
SECONDARY=dc96515affe3c114a7091248ae403ff3d61dc59d

# vendored path (under $DEST) <- path in the dial-me-spinner tree
REF_FILES=(
  "kicad/main/dial-me-spinner-main.net        hardware/kicad/main/dial-me-spinner-main.net"
  "kicad/main/dial-me-spinner-main.kicad_pro  hardware/kicad/main/dial-me-spinner-main.kicad_pro"
  "kicad/main/dial-me-spinner-main.kicad_sch  hardware/kicad/main/dial-me-spinner-main.kicad_sch"
  "kicad/main/detent.kicad_sch                hardware/kicad/main/detent.kicad_sch"
  "kicad/main/display.kicad_sch               hardware/kicad/main/display.kicad_sch"
  "kicad/main/leds.kicad_sch                  hardware/kicad/main/leds.kicad_sch"
  "kicad/main/main_power.kicad_sch            hardware/kicad/main/main_power.kicad_sch"
  "kicad/main/mcu.kicad_sch                   hardware/kicad/main/mcu.kicad_sch"
  "kicad/main/sensors.kicad_sch               hardware/kicad/main/sensors.kicad_sch"
  "kicad/main/sym-lib-table                   hardware/kicad/main/sym-lib-table"
  "kicad/main/lib/dial_spinner.kicad_sym      hardware/kicad/main/lib/dial_spinner.kicad_sym"
  "kicad/base/dial-me-spinner-base.net        hardware/kicad/base/dial-me-spinner-base.net"
  "kicad/base/dial-me-spinner-base.kicad_pro  hardware/kicad/base/dial-me-spinner-base.kicad_pro"
  "kicad/base/dial-me-spinner-base.kicad_sch  hardware/kicad/base/dial-me-spinner-base.kicad_sch"
  "kicad/base/base.kicad_sch                  hardware/kicad/base/base.kicad_sch"
  "kicad/base/sym-lib-table                   hardware/kicad/base/sym-lib-table"
  "kicad/base/lib/dial_spinner.kicad_sym      hardware/kicad/base/lib/dial_spinner.kicad_sym"
)
SECONDARY_FILES=(
  "kicad/kicad9-dc96515/main/dial-me-spinner-main.net  hardware/kicad/main/dial-me-spinner-main.net"
  "kicad/kicad9-dc96515/base/dial-me-spinner-base.net  hardware/kicad/base/dial-me-spinner-base.net"
)

git -C "$REPO" cat-file -e "$REF^{commit}"
git -C "$REPO" cat-file -e "$SECONDARY^{commit}"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

entries=()
vendor() {  # commit, then "dest src" lines
  local commit=$1; shift
  local srcs=() line dst src
  for line in "$@"; do read -r dst src <<<"$line"; srcs+=("$src"); done
  mkdir -p "$TMP/$commit"
  git -C "$REPO" archive "$commit" "${srcs[@]}" | tar -x -C "$TMP/$commit"
  for line in "$@"; do
    read -r dst src <<<"$line"
    mkdir -p "$(dirname "$DEST/$dst")"
    cp "$TMP/$commit/$src" "$DEST/$dst"
    entries+=("$(sha256sum "$DEST/$dst" | cut -d' ' -f1)  $dst  ${commit}:$src")
  done
}
vendor "$REF" "${REF_FILES[@]}"
vendor "$SECONDARY" "${SECONDARY_FILES[@]}"

{
  cat <<EOF
# examples/dial-me-spinner/SOURCE: where every vendored dial-me-spinner file
# comes from (Phase 1 plan M0; approver decisions A and C, 2026-11-09).
# Written by scripts/dms_vendor.sh; checked by scripts/dms_source_check.sh.
#
# repository  dial-me-spinner (the board's author's repository; local, not fetched)
# reference   $REF
#             branch kicad10-port, KiCad 10.0.8 ("Move the toolchain and
#             boards to KiCad 10.0.8, with no change to the design")
# secondary   $SECONDARY
#             main, KiCad 9.0.9: only its two netlists, for the secondary
#             import check of gate M4c (not a gate for E1)
# method      git archive <commit> <paths> | tar -x; bytes unchanged
# licence     vendored by approver decision A (plan Q2): distributed with
#             breadboard.dev under its licence (Apache-2.0). dial-me-spinner
#             itself carries no LICENSE file at either commit.
# suite       hardware/sim at the reference commit is not vendored; it is
#             re-run from a git archive export by scripts/dms_suite.sh, and
#             its results are in suite/results.json
#
# sha256  vendored path (relative to this directory)  commit:path in dial-me-spinner
EOF
  printf '%s\n' "${entries[@]}"
} > "$DEST/SOURCE"
echo "vendored ${#entries[@]} files into $DEST; wrote $DEST/SOURCE"
