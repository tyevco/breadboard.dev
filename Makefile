# Breadboard — top-level build entry points. Run inside WSL / Linux.
SHELL := /bin/bash

export CARGO_TARGET_DIR ?= $(HOME)/.cache/bb-target
export PATH := $(HOME)/.cargo/bin:$(HOME)/.local/bin:$(PATH)

CARGO ?= cargo

.PHONY: setup doctor build test fmt fmt-check clippy check firmware

setup:
	bash scripts/setup.sh

doctor:
	@bash scripts/doctor.sh

# Bare-metal STM32F103 blinky used by the Phase 0 spike and the Renode engine
# tests: examples/spike/firmware/build/blinky.elf
firmware:
	$(MAKE) -C examples/spike/firmware

build:
	$(CARGO) build --workspace --all-targets

test:
	$(CARGO) test --workspace

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all -- --check

clippy:
	$(CARGO) clippy --workspace --all-targets -- -D warnings

# Everything CI runs, in CI order.
check: fmt-check clippy test
