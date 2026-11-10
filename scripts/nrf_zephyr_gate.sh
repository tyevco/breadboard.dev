#!/usr/bin/env bash
# Phase 1 M2 (ADR-008 D3): rebuild the nrf-zephyr gate image
# (engines/cpu-renode/tests/firmware/nrf52840/zephyr) for nrf52840dk/nrf52840
# in a scratch build directory and compare it with the committed prebuilt
# image, or (--update) replace it. `make test` never runs this: it uses the
# committed ELF, so it needs no Zephyr SDK.
#
# Needs a Zephyr workspace (ZEPHYR_WS, default ~/zephyrproject, with its
# .venv and SDK; never written to: the build directory is outside it).
# PROVENANCE records the workspace's Zephyr, hal_nordic, cmsis and cmsis_6
# commits, the SDK version, and the sha256 of the image's loadable bytes
# (`objcopy -O binary` of the committed ELF). The committed ELF has its debug
# information stripped, because that carries the build path (ADR-008
# Context 5) and is never loaded. This is a provisional identity for a gate
# fixture; the image hash of ADR-008 D5 (PT_LOAD segments and entry point)
# comes with `bb flash` at M6.
#
# Usage: scripts/nrf_zephyr_gate.sh [--update]
set -euo pipefail

ws=${ZEPHYR_WS:-$HOME/zephyrproject}
repo=$(cd "$(dirname "$0")/.." && pwd)
app=$repo/engines/cpu-renode/tests/firmware/nrf52840/zephyr
pre=$app/prebuilt
build=${NRF_ZEPHYR_BUILD:-$HOME/.cache/bb-nrf-zephyr-gate}
sdk=$(ls -d "$ws"/zephyr-sdk-* | head -1)

# shellcheck disable=SC1091
source "$ws/.venv/bin/activate"
export ZEPHYR_SDK_INSTALL_DIR=$sdk ZEPHYR_BASE=$ws/zephyr
rm -rf "$build"
(cd "$ws" && west build -p always -b nrf52840dk/nrf52840 -d "$build" "$app") \
  > "$build.log" 2>&1 || { tail -40 "$build.log"; exit 1; }
arm-none-eabi-strip --strip-debug -o "$build/gate.elf" "$build/zephyr/zephyr.elf"

image() { # <elf>: sha256 of its loadable bytes
  arm-none-eabi-objcopy -O binary "$1" "$build/image.bin"
  sha256sum < "$build/image.bin" | cut -d' ' -f1
}
rev() { git -C "$1" rev-parse HEAD; }
provenance() {
  echo "# nrf-zephyr gate image, built by scripts/nrf_zephyr_gate.sh --update"
  echo "board nrf52840dk/nrf52840"
  echo "zephyr $(rev "$ws/zephyr") ($(git -C "$ws/zephyr" describe --tags --always 2>/dev/null))"
  echo "hal_nordic $(rev "$ws/modules/hal/nordic")"
  echo "cmsis $(rev "$ws/modules/hal/cmsis")"
  echo "cmsis_6 $(rev "$ws/modules/hal/cmsis_6")"
  echo "sdk $(basename "$sdk") $(cat "$sdk/sdk_version")"
  echo "app $(cd "$app" && cat CMakeLists.txt prj.conf src/main.c | sha256sum | cut -d' ' -f1)"
  echo "image $(image "$build/gate.elf")"
}

if [[ ${1:-} == --update ]]; then
  mkdir -p "$pre"
  cp "$build/gate.elf" "$pre/nrf_zephyr_gate.elf"
  provenance > "$pre/PROVENANCE"
  echo "nrf_zephyr_gate: wrote $pre"
  cat "$pre/PROVENANCE"
else
  want=$(sed -n 's/^image //p' "$pre/PROVENANCE")
  got=$(image "$build/gate.elf")
  committed=$(image "$pre/nrf_zephyr_gate.elf")
  echo "nrf_zephyr_gate: image rebuilt $got, PROVENANCE $want, committed ELF $committed"
  if [[ $got == "$want" && $committed == "$want" ]]; then
    echo "nrf_zephyr_gate: PASS (identical image)"
  else
    echo "nrf_zephyr_gate: FAIL"; exit 1
  fi
fi
