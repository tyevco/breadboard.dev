# nrf-zephyr gate image (ADR-008 D3). A fixed BUILD_VERSION and no boot
# banner keep the workspace's `git describe` out of the image (ADR-008 D5).
cmake_minimum_required(VERSION 3.20.0)
set(BUILD_VERSION bb-nrf-zephyr-gate-1)
find_package(Zephyr REQUIRED HINTS $ENV{ZEPHYR_BASE})
project(nrf_zephyr_gate)
target_sources(app PRIVATE src/main.c)
