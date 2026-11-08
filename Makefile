# Breadboard — top-level build entry points. Run inside WSL / Linux.
SHELL := /bin/bash

export CARGO_TARGET_DIR ?= $(HOME)/.cache/bb-target
export PATH := $(HOME)/.cargo/bin:$(HOME)/.local/bin:$(PATH)

CARGO ?= cargo

.PHONY: setup doctor build test cargo-test fmt fmt-check clippy check firmware \
        bb spike determinism golden golden-update

# --- Phase 0 spike ---------------------------------------------------------
BB          := $(CARGO_TARGET_DIR)/release/bb
SPIKE_ELF   := examples/spike/firmware/build/blinky.elf
SPIKE_FOR   ?= 500ms
SPIKE_OUT   ?= out/spike
SPIKE_ARGS   = --netlist examples/spike/board.cir --firmware $(SPIKE_ELF) \
               --target U1 --for $(SPIKE_FOR) --probe net:LED_A --probe net:PA5
GOLDEN      := tests/golden/spike/led_a.csv
DET_RUNS    ?= 10
DET_JOBS    ?= 5
PYTHON      ?= python3

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

cargo-test:
	$(CARGO) test --workspace

# The exit gate: unit/engine tests, then the end-to-end spike, 10-run
# determinism and the golden-waveform diff (golden reuses the spike output).
test:
	$(MAKE) cargo-test
	$(MAKE) spike
	$(MAKE) determinism
	$(MAKE) golden

bb:
	$(CARGO) build --release -p bb-cli

# 500 ms of co-simulation: firmware in Renode drives PA5, ngspice solves the
# 330 ohm + red LED circuit. Writes $(SPIKE_OUT)/{events.jsonl,nets.csv,nets.vcd}.
spike: firmware bb
	$(BB) run $(SPIKE_ARGS) --out $(SPIKE_OUT)
	$(PYTHON) scripts/check_spike.py $(SPIKE_OUT)

determinism: firmware bb
	bash scripts/determinism.sh $(BB) out/determinism $(DET_RUNS) $(DET_JOBS) -- $(SPIKE_ARGS)

# Diff LED_A against the committed snapshot (1 mV / 1 sample). Runs the
# spike first if its output is missing.
golden:
	@if [ ! -f $(SPIKE_OUT)/nets.csv ]; then $(MAKE) spike; fi
	$(PYTHON) scripts/golden.py check $(SPIKE_OUT)/nets.csv $(GOLDEN)

# Regenerate the snapshot from a fresh, checked spike run (review the diff!).
golden-update: spike
	mkdir -p $(dir $(GOLDEN))
	$(PYTHON) scripts/golden.py write $(SPIKE_OUT)/nets.csv $(GOLDEN)

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all -- --check

clippy:
	$(CARGO) clippy --workspace --all-targets -- -D warnings

# Everything CI runs, in CI order.
check: fmt-check clippy test
