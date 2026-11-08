# Breadboard — top-level build entry points. Run inside WSL / Linux.
SHELL := /bin/bash

export CARGO_TARGET_DIR ?= $(HOME)/.cache/bb-target
export PATH := $(HOME)/.cargo/bin:$(HOME)/.local/bin:$(PATH)

CARGO ?= cargo

.PHONY: setup doctor build test cargo-test fmt fmt-check clippy check firmware \
        bb spike spike-verify-every lattice-check partition perf determinism golden \
        golden-update

# --- Phase 0 spike ---------------------------------------------------------
BB          := $(CARGO_TARGET_DIR)/release/bb
SPIKE_ELF   := examples/spike/firmware/build/blinky.elf
SPIKE_FOR   ?= 500ms
SPIKE_OUT   ?= out/spike
SPIKE_ARGS   = --netlist examples/spike/board.cir --firmware $(SPIKE_ELF) \
               --target U1 --for $(SPIKE_FOR) --probe net:LED_A --probe net:PA5
GOLDEN      := tests/golden/spike/led_a.csv
PARTITION_HASHES := tests/golden/spike/partition.sha256
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

# BB_REQUIRE_RENODE=1: the Renode engine tests fail instead of skipping when
# Renode or the firmware toolchain is missing, so the gate really runs them.
cargo-test:
	BB_REQUIRE_RENODE=1 $(CARGO) test --workspace

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

# The spike with the Renode cross-check after every grant (ADR-003 D1.2; CI
# job `verify-every`). The checks are pure reads, so the outputs must be
# byte-identical to the default (`--verify edges`) spike run.
VERIFY_EVERY_OUT ?= out/spike-verify-every
spike-verify-every: spike
	$(BB) run $(SPIKE_ARGS) --verify every --out $(VERIFY_EVERY_OUT)
	$(PYTHON) scripts/check_spike.py $(VERIFY_EVERY_OUT)
	set -e; for f in nets.csv events.jsonl nets.vcd; do cmp $(SPIKE_OUT)/$$f $(VERIFY_EVERY_OUT)/$$f; done
	@echo "spike-verify-every: outputs identical to $(SPIKE_OUT)"

# ADR-003 D2 (step 2) gate: on the fixed 10 us lattice, 1 ms grants give the
# spike's nets.csv byte for byte and the same non-grant records, and a 1 kHz
# CSV samples exactly the 100 kHz waveform (TMAX no longer follows the grant
# or the sample rate). check_spike is not run on the 1 kHz CSV: its LED_A
# crossing check interpolates between samples and needs samples much closer
# than its 0.1 ms spacing tolerance (ADR-003 Amendment 2).
LATTICE_OUT ?= out/lattice
lattice-check: spike
	$(BB) run $(SPIKE_ARGS) --max-quantum 1ms --out $(LATTICE_OUT)/mq1ms
	$(PYTHON) scripts/check_spike.py $(LATTICE_OUT)/mq1ms
	cmp $(SPIKE_OUT)/nets.csv $(LATTICE_OUT)/mq1ms/nets.csv
	diff <(grep -v '"type":"grant"' $(SPIKE_OUT)/events.jsonl) \
	     <(grep -v '"type":"grant"' $(LATTICE_OUT)/mq1ms/events.jsonl)
	$(BB) run $(SPIKE_ARGS) --max-quantum 1ms --sample-rate 1kHz --out $(LATTICE_OUT)/mq1ms-1khz
	$(PYTHON) scripts/check_resample.py $(SPIKE_OUT) $(LATTICE_OUT)/mq1ms-1khz
	@echo "lattice-check: PASS"

# ADR-003 step 3 partition-equivalence gate: the spike at every max_quantum
# below (10 us is the one-cell reference) must give nets.csv, nets.vcd and
# the non-grant event records of the committed baseline hashes
# ($(PARTITION_HASHES)) byte for byte. CI job `partition`.
PARTITION_OUT    ?= out/partition
PARTITION_QUANTA ?= 10us,100us,10ms,50ms,500ms
PARTITION_JOBS   ?= 5
partition: firmware bb
	bash scripts/partition.sh $(BB) $(PARTITION_OUT) $(PARTITION_JOBS) $(PARTITION_HASHES) \
	    $(PARTITION_QUANTA) -- $(SPIKE_ARGS)

# ADR-003 step 3 performance gate (measurement protocol: interleaved A/B,
# N runs each, medians, startup separate, CPU time and load recorded): the
# spike at the default 10 ms grants (B) against 10 us grants (A) must take
# exactly 50 grants, a median simulation time <= 2 s and be >= 15x faster.
PERF_OUT ?= out/perf
PERF_N   ?= 5
perf: firmware bb
	bash scripts/perf.sh $(BB) $(PERF_OUT) $(PERF_N) 10us 10ms 50 2.0 15 -- $(SPIKE_ARGS)

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
	cd $(SPIKE_OUT) && grep -v '"type":"grant"' events.jsonl > non-grant-records \
	    && sha256sum nets.csv nets.vcd non-grant-records > $(CURDIR)/$(PARTITION_HASHES) \
	    && rm non-grant-records

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all -- --check

clippy:
	$(CARGO) clippy --workspace --all-targets -- -D warnings

# Everything CI runs, in CI order.
check: fmt-check clippy test
