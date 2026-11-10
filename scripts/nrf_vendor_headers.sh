#!/usr/bin/env bash
# Phase 1 M2 (ADR-008 D5): vendor the headers the nRF52840 bare-metal gate
# firmware builds against into engines/cpu-renode/tests/firmware/nrf52840/vendor/,
# unmodified, and write vendor/SOURCE (upstream commits and the sha256 of
# every file). Needs a Zephyr workspace with hal_nordic and cmsis_6
# (ZEPHYR_WS, default ~/zephyrproject); `make test` never runs this, it
# builds against the committed copies.
#
#   nrfx MDK (BSD-3-Clause, the licence is in each file's header):
#     nrf52840.h, nrf52840_bitfields.h, system_nrf52840.h
#   CMSIS-Core (Apache-2.0, vendor/cmsis/LICENSE): core_cm4.h and the five
#     headers it includes for arm-none-eabi-gcc
#
# Usage: scripts/nrf_vendor_headers.sh [--check]
#   --check  compare the committed copies against the workspace instead.
set -euo pipefail

ws=${ZEPHYR_WS:-$HOME/zephyrproject}
nordic=$ws/modules/hal/nordic
cmsis=$ws/modules/hal/cmsis_6
repo=$(cd "$(dirname "$0")/.." && pwd)
dest=$repo/engines/cpu-renode/tests/firmware/nrf52840/vendor
mdk_files=(nrf52840.h nrf52840_bitfields.h system_nrf52840.h)
cmsis_files=(core_cm4.h cmsis_version.h cmsis_compiler.h cmsis_gcc.h
             m-profile/cmsis_gcc_m.h m-profile/armv7m_mpu.h)

out=$dest
if [[ ${1:-} == --check ]]; then
  out=$(mktemp -d)
  trap 'rm -rf "$out"' EXIT
fi
rm -rf "$out/nrfx" "$out/cmsis"
mkdir -p "$out/nrfx" "$out/cmsis/m-profile"
for f in "${mdk_files[@]}"; do
  cp "$nordic/nrfx/bsp/stable/mdk/nrf52/nrf52840/$f" "$out/nrfx/$f"
done
for f in "${cmsis_files[@]}"; do
  cp "$cmsis/CMSIS/Core/Include/$f" "$out/cmsis/$f"
done
cp "$cmsis/LICENSE" "$out/cmsis/LICENSE"
{
  echo "# Headers for the nRF52840 bare-metal gate firmware (ADR-008 D5), copied"
  echo "# unmodified by scripts/nrf_vendor_headers.sh. <sha256>  <path> per file."
  echo "# nrfx: https://github.com/zephyrproject-rtos/hal_nordic $(git -C "$nordic" rev-parse HEAD) nrfx/bsp/stable/mdk/nrf52/nrf52840 (BSD-3-Clause)"
  echo "# cmsis: https://github.com/zephyrproject-rtos/CMSIS_6 $(git -C "$cmsis" rev-parse HEAD) CMSIS/Core/Include (Apache-2.0)"
  (cd "$out" && find nrfx cmsis -type f | LC_ALL=C sort | xargs sha256sum)
} > "$out/SOURCE"
if [[ ${1:-} == --check ]]; then
  diff -r "$dest" "$out" && echo "nrf_vendor_headers: committed copies match $ws"
else
  echo "nrf_vendor_headers: wrote $dest"
  cat "$dest/SOURCE"
fi
