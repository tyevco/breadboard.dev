# SPDX-License-Identifier: Apache-2.0
# W2 firmware (Phase 2 plan section 2 W2, milestone W-fw; ADR-009 D7): the S4 brown-out reboot
# loop of dial-me-spinner docs/firmware.md, the old rule and the NEW-1 rule. A fixed BUILD_VERSION
# and no boot banner keep the workspace's `git describe` out of the image, so the image hash does
# not depend on the workspace.
cmake_minimum_required(VERSION 3.20.0)
set(BUILD_VERSION bb-dms-w2-oled-1)
# The out-of-tree board (../boards/dial/dial_me_spinner) and its bindings (../dts/bindings).
list(APPEND BOARD_ROOT ${CMAKE_CURRENT_LIST_DIR}/..)
list(APPEND DTS_ROOT ${CMAKE_CURRENT_LIST_DIR}/..)
find_package(Zephyr REQUIRED HINTS $ENV{ZEPHYR_BASE})
project(w2_oled)
target_sources(app PRIVATE src/main.c)
