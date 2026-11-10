# SPDX-License-Identifier: Apache-2.0
# E1 firmware (ADR-008 D5, Phase 1 plan M6): the LED rail sequencing rule of dial-me-spinner
# docs/firmware.md section 6.4. A fixed BUILD_VERSION and no boot banner keep the workspace's
# `git describe` out of the image, so the image hash does not depend on the workspace.
cmake_minimum_required(VERSION 3.20.0)
set(BUILD_VERSION bb-dms-led-rail-1)
# The out-of-tree board (../boards/dial/dial_me_spinner) and its bindings (../dts/bindings).
list(APPEND BOARD_ROOT ${CMAKE_CURRENT_LIST_DIR}/..)
list(APPEND DTS_ROOT ${CMAKE_CURRENT_LIST_DIR}/..)
find_package(Zephyr REQUIRED HINTS $ENV{ZEPHYR_BASE})
project(led_rail)
target_sources(app PRIVATE src/main.c)
