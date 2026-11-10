#!/usr/bin/env bash
# Phase 1 M6 (ADR-008 D5): the E1 Zephyr firmware of examples/dial-me-spinner,
# built from a pinned west workspace with a pinned Zephyr SDK.
#
#   scripts/dms_zephyr.sh sdk                 install the pinned SDK (minimal + arm-zephyr-eabi)
#   scripts/dms_zephyr.sh ws [<dir>]          create (or update) a workspace from firmware/west.yml
#   scripts/dms_zephyr.sh build <ws> <out>    build both E1 images into <out> (stripped ELFs)
#   scripts/dms_zephyr.sh update [<ws>]       rebuild into firmware/prebuilt (SHA256SUMS, PROVENANCE)
#   scripts/dms_zephyr.sh check               fw-rebuild: a FRESH workspace (a second `west init`
#                                             into a new directory), rebuild, and require the image
#                                             hashes of firmware/prebuilt/SHA256SUMS
#
# Never touches a developer's workspace (~/zephyrproject) or the dial-me-spinner repository.
# Downloads (the SDK from GitHub releases, the west projects, PyPI packages) are the toolchains
# ADR-008 D5 names. The image hash is ADR-008 D5's (PT_LOAD segments and the entry point,
# `bb flash --hash-only`), not the file's sha256: two identical builds differ in their debug info.
set -euo pipefail

ZEPHYR_TAG=v4.5.0
ZEPHYR_SDK_VERSION=1.0.1
SDK_BASE_URL="https://github.com/zephyrproject-rtos/sdk-ng/releases/download/v${ZEPHYR_SDK_VERSION}"
SDK_MINIMAL="zephyr-sdk-${ZEPHYR_SDK_VERSION}_linux-x86_64_minimal.tar.xz"
SDK_MINIMAL_SHA256=ca9bc0ff66fafca1dac9d592a36d953cf16d096a9d09b1c0357f021cf9f6a7eb
SDK_ARM="toolchain_gnu_linux-x86_64_arm-zephyr-eabi.tar.xz"
SDK_ARM_SHA256=21b85981cb5a1818d9bc53d82af80f208946ec038b982ff1907287572ed3a634
WEST_VERSION=1.5.0

repo=$(cd "$(dirname "$0")/.." && pwd)
fw=$repo/examples/dial-me-spinner/firmware
cache=${BB_ZEPHYR_CACHE:-$HOME/.cache/bb-zephyr}
sdk=${ZEPHYR_SDK_INSTALL_DIR:-$HOME/.local/opt/zephyr-sdk-${ZEPHYR_SDK_VERSION}}
default_ws=$cache/ws-${ZEPHYR_TAG}
BB=${BB:-${CARGO_TARGET_DIR:-$HOME/.cache/bb-target}/release/bb}
BOARD=dial_me_spinner/nrf52840
# name -> extra Kconfig fragment (the one switch between the two builds, ADR-008 D5)
IMAGES=(led_rail led_rail_wrong_order)

log() { printf '==> %s\n' "$*" >&2; }

cmd_sdk() {
  if [[ -x $sdk/gnu/arm-zephyr-eabi/bin/arm-zephyr-eabi-gcc && "$(cat "$sdk/sdk_version" 2>/dev/null)" == "$ZEPHYR_SDK_VERSION" ]]; then
    log "sdk: Zephyr SDK $ZEPHYR_SDK_VERSION present at $sdk"
    return
  fi
  local dl=$cache/dl
  mkdir -p "$dl" "$(dirname "$sdk")"
  for f in "$SDK_MINIMAL:$SDK_MINIMAL_SHA256" "$SDK_ARM:$SDK_ARM_SHA256"; do
    local name=${f%%:*} want=${f##*:}
    if ! echo "$want  $dl/$name" | sha256sum -c --status - 2>/dev/null; then
      log "sdk: downloading $name"
      curl -fL --retry 3 -o "$dl/$name" "$SDK_BASE_URL/$name"
    fi
    echo "$want  $dl/$name" | sha256sum -c -
  done
  local tmp
  tmp=$(mktemp -d "$cache/sdk.XXXXXX")
  tar -xJf "$dl/$SDK_MINIMAL" -C "$tmp"
  local root
  root=$(find "$tmp" -mindepth 1 -maxdepth 1 -type d | head -1)
  mkdir -p "$root/gnu"
  tar -xJf "$dl/$SDK_ARM" -C "$root/gnu"
  rm -rf "$sdk"
  mv "$root" "$sdk"
  rmdir "$tmp"
  "$sdk/gnu/arm-zephyr-eabi/bin/arm-zephyr-eabi-gcc" --version | head -1
  log "sdk: installed Zephyr SDK $(cat "$sdk/sdk_version") (minimal + arm-zephyr-eabi) at $sdk"
}

venv_of() { echo "$1/.venv"; }

cmd_ws() {
  local ws=${1:-$default_ws}
  mkdir -p "$ws/manifest"
  cp "$fw/west.yml" "$ws/manifest/west.yml"
  local venv
  venv=$(venv_of "$ws")
  if [[ ! -x $venv/bin/west ]]; then
    log "ws: python venv in $venv (west $WEST_VERSION)"
    python3 -m venv "$venv"
    "$venv/bin/pip" install -q "west==$WEST_VERSION"
  fi
  if [[ ! -d $ws/.west ]]; then
    (cd "$ws" && "$venv/bin/west" init -l manifest)
  fi
  log "ws: west update in $ws"
  (cd "$ws" && "$venv/bin/west" update --narrow -o=--depth=1 2>&1 | grep -v '^$' >&2)
  # Zephyr's Python build requirements, at the versions of firmware/requirements.lock.
  "$venv/bin/pip" install -q -c "$fw/requirements.lock" -r "$ws/zephyr/scripts/requirements-base.txt"
  for p in zephyr modules/hal/cmsis modules/hal/cmsis_6 modules/hal/nordic; do
    printf '%s %s\n' "$p" "$(git -C "$ws/$p" rev-parse HEAD)" >&2
  done
}

image_hash() { "$BB" flash --hash-only "$1"; }

cmd_build() {
  local ws=$1 out=$2
  [[ -d $ws/zephyr ]] || { echo "dms_zephyr: no workspace at $ws (scripts/dms_zephyr.sh ws $ws)" >&2; exit 1; }
  cmd_sdk
  local venv
  venv=$(venv_of "$ws")
  # shellcheck disable=SC1091
  source "$venv/bin/activate"
  export ZEPHYR_SDK_INSTALL_DIR=$sdk ZEPHYR_BASE=$ws/zephyr ZEPHYR_TOOLCHAIN_VARIANT=zephyr
  mkdir -p "$out"
  for name in "${IMAGES[@]}"; do
    local bdir=$out/build-$name extra=()
    [[ $name == led_rail_wrong_order ]] && extra=(-DEXTRA_CONF_FILE=wrong_order.conf)
    rm -rf "$bdir"
    log "build: $name ($BOARD) in $bdir"
    (cd "$ws" && west build -p always -b "$BOARD" -d "$bdir" "$fw/led_rail" -- "${extra[@]}") \
      > "$bdir.log" 2>&1 || { tail -40 "$bdir.log"; exit 1; }
    "$sdk/gnu/arm-zephyr-eabi/bin/arm-zephyr-eabi-strip" --strip-debug \
      -o "$out/$name.elf" "$bdir/zephyr/zephyr.elf"
    printf '%s  %s\n' "$(image_hash "$out/$name.elf")" "$name.elf"
  done
}

provenance() {
  local ws=$1
  echo "# examples/dial-me-spinner/firmware/prebuilt: built by scripts/dms_zephyr.sh update (ADR-008 D5)"
  echo "board $BOARD (firmware/boards/dial/dial_me_spinner)"
  echo "zephyr $(git -C "$ws/zephyr" rev-parse HEAD) ($ZEPHYR_TAG)"
  echo "hal_nordic $(git -C "$ws/modules/hal/nordic" rev-parse HEAD)"
  echo "cmsis $(git -C "$ws/modules/hal/cmsis" rev-parse HEAD)"
  echo "cmsis_6 $(git -C "$ws/modules/hal/cmsis_6" rev-parse HEAD)"
  echo "sdk zephyr-sdk $(cat "$sdk/sdk_version") ($("$sdk/gnu/arm-zephyr-eabi/bin/arm-zephyr-eabi-gcc" --version | head -1))"
  echo "west $WEST_VERSION"
  echo "sources $(cd "$fw" && find boards dts led_rail west.yml requirements.lock -type f | LC_ALL=C sort \
    | xargs sha256sum | sha256sum | cut -d' ' -f1)"
}

cmd_update() {
  local ws=${1:-$default_ws}
  local out=$cache/build
  cmd_build "$ws" "$out" > "$out.sums"
  mkdir -p "$fw/prebuilt"
  for name in "${IMAGES[@]}"; do cp "$out/$name.elf" "$fw/prebuilt/$name.elf"; done
  {
    echo "# ELF image hashes (ADR-008 D5: sha256 over the PT_LOAD segments and the entry point,"
    echo "# \`bb flash --hash-only\`), not sha256sum of the files."
    cat "$out.sums"
  } > "$fw/prebuilt/SHA256SUMS"
  provenance "$ws" > "$fw/prebuilt/PROVENANCE"
  cat "$fw/prebuilt/SHA256SUMS" "$fw/prebuilt/PROVENANCE"
}

cmd_check() {
  local ws
  mkdir -p "$cache"
  ws=$(mktemp -d "$cache/fresh-ws.XXXXXX")
  log "check: fresh workspace $ws"
  cmd_ws "$ws"
  local out=$ws/out
  cmd_build "$ws" "$out" > "$out.sums"
  local ok=1
  while read -r h name; do
    local want
    want=$(awk -v n="$name" '$2 == n {print $1}' "$fw/prebuilt/SHA256SUMS")
    local committed
    committed=$(image_hash "$fw/prebuilt/$name")
    echo "fw-rebuild: $name rebuilt $h, SHA256SUMS $want, committed ELF $committed"
    [[ $h == "$want" && $committed == "$want" ]] || ok=0
  done < "$out.sums"
  if [[ $ok == 1 ]]; then
    echo "fw-rebuild: PASS (a fresh workspace gives the committed image hashes)"
    [[ ${BB_ZEPHYR_KEEP:-0} == 1 ]] || rm -rf "$ws"
  else
    echo "fw-rebuild: FAIL (workspace kept at $ws)"; exit 1
  fi
}

case ${1:-} in
  sdk) cmd_sdk ;;
  ws) shift; cmd_ws "$@" ;;
  build) shift; cmd_build "$@" ;;
  update) shift; cmd_update "$@" ;;
  check) cmd_check ;;
  *) sed -n '2,16p' "$0" >&2; exit 2 ;;
esac
