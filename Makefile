# Breadboard — top-level build entry points. Run inside WSL / Linux.
SHELL := /bin/bash

export CARGO_TARGET_DIR ?= $(HOME)/.cache/bb-target
export PATH := $(HOME)/.cargo/bin:$(HOME)/.local/bin:$(PATH)

CARGO ?= cargo

.PHONY: setup doctor build test cargo-test fmt fmt-check clippy check firmware \
        bb spike spike-verify-every lattice-check partition perf determinism golden \
        golden-update feedback-run feedback feedback-order feedback-determinism \
        feedback-open feedback-open-ref

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

# --- ADR-003 step 4: digital_in thresholding and the feedback boards --------
FEEDBACK_ELF  := examples/feedback/firmware/build/feedback.elf
FEEDBACK_FOR  ?= 500ms
FEEDBACK_OUT  ?= out/feedback
FEEDBACK_ARGS  = --netlist examples/feedback/board.cir --firmware $(FEEDBACK_ELF) \
                 --target U1 --for $(FEEDBACK_FOR) --probe net:SENSE --probe net:LED_B \
                 --probe net:U1.PA5 --probe net:U1.PB0 --probe net:U1.PA6
FEEDBACK_GOLDEN := tests/golden/feedback
FEEDBACK_OPEN_ARGS = --netlist examples/feedback-open/board.cir --firmware $(FEEDBACK_ELF) \
                 --target U1 --for $(FEEDBACK_FOR) --probe net:SENSE --probe net:U1.PB0 \
                 --probe net:U1.PA6
FEEDBACK_OPEN_OUT    ?= out/feedback-open
FEEDBACK_OPEN_HASHES := tests/golden/feedback-open/partition.sha256
FEEDBACK_OPEN_QUANTA ?= 10us,100us,1ms,10ms,50ms,500ms

setup:
	bash scripts/setup.sh

doctor:
	@bash scripts/doctor.sh

# Bare-metal STM32F103 blinky used by the Phase 0 spike and the Renode engine
# tests: examples/spike/firmware/build/blinky.elf
firmware:
	$(MAKE) -C examples/spike/firmware
	$(MAKE) -C examples/feedback/firmware

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
	$(MAKE) feedback
	$(MAKE) feedback-order

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

# Regenerate the snapshots from fresh, checked runs (review the diff!): the
# spike waveform and partition hashes, the feedback waveforms and edge list,
# and the feedback-open partition hashes (from its one-cell reference).
golden-update: spike feedback-run feedback-open-ref
	mkdir -p $(dir $(GOLDEN))
	$(PYTHON) scripts/golden.py write $(SPIKE_OUT)/nets.csv $(GOLDEN)
	cd $(SPIKE_OUT) && grep -v '"type":"grant"' events.jsonl > non-grant-records \
	    && sha256sum nets.csv nets.vcd non-grant-records > $(CURDIR)/$(PARTITION_HASHES) \
	    && rm non-grant-records
	mkdir -p $(FEEDBACK_GOLDEN) $(dir $(FEEDBACK_OPEN_HASHES))
	$(PYTHON) scripts/golden.py write $(FEEDBACK_OUT)/nets.csv $(FEEDBACK_GOLDEN)/sense.csv --net SENSE
	$(PYTHON) scripts/golden.py write $(FEEDBACK_OUT)/nets.csv $(FEEDBACK_GOLDEN)/led_b.csv --net LED_B
	grep '"type":"pin_edge"' $(FEEDBACK_OUT)/events.jsonl > $(FEEDBACK_GOLDEN)/pin_edges.jsonl
	cd $(FEEDBACK_OPEN_OUT)-ref && grep -v '"type":"grant"' events.jsonl > non-grant-records \
	    && sha256sum nets.csv nets.vcd non-grant-records > $(CURDIR)/$(FEEDBACK_OPEN_HASHES) \
	    && rm non-grant-records

# ADR-003 step 4: examples/feedback, a board with a feedback loop (U1.PA5 ->
# RC -> U1.PB0 digital_in; the EXTI0 ISR copies PB0 to PA6 -> LED). The loop
# caps every grant at one Renode quantum. check_feedback.py: PB0 transitions
# at the analytic RC crossings (in their lattice cells), PA6 edges in
# (ceil_L(t_x), ceil_L(t_x) + L], LED_B plateaus.
feedback-run: firmware bb
	$(BB) run $(FEEDBACK_ARGS) --out $(FEEDBACK_OUT)
	$(PYTHON) scripts/check_feedback.py $(FEEDBACK_OUT)

# ... plus the golden snapshot: SENSE and LED_B within 1 mV / 1 sample and
# the exact pin_edge list.
feedback: feedback-run
	$(PYTHON) scripts/golden.py check $(FEEDBACK_OUT)/nets.csv $(FEEDBACK_GOLDEN)/sense.csv --net SENSE
	$(PYTHON) scripts/golden.py check $(FEEDBACK_OUT)/nets.csv $(FEEDBACK_GOLDEN)/led_b.csv --net LED_B
	diff <(grep '"type":"pin_edge"' $(FEEDBACK_OUT)/events.jsonl) $(FEEDBACK_GOLDEN)/pin_edges.jsonl
	@echo "feedback: golden snapshot PASS"

# Swapping the registration order (analog engine first) must give
# byte-identical outputs: the advance order comes from the bindings (ADR-003
# D3). Reuses the `feedback` run.
feedback-order: feedback
	$(BB) run $(FEEDBACK_ARGS) --engine-order analog-first --out $(FEEDBACK_OUT)-swapped
	set -e; for f in nets.csv events.jsonl nets.vcd; do cmp $(FEEDBACK_OUT)/$$f $(FEEDBACK_OUT)-swapped/$$f; done
	@echo "feedback-order: outputs identical with the engines registered in the other order"

# 10 runs of the feedback board (DET_RUNS, DET_JOBS): byte-identical
# outputs. CI job `feedback`.
feedback-determinism: firmware bb
	bash scripts/determinism.sh $(BB) out/feedback-determinism $(DET_RUNS) $(DET_JOBS) -- $(FEEDBACK_ARGS)

# examples/feedback-open, the acyclic variant (ngspice -> U1 only): long
# grants, inputs applied at ceil_L(t_x). Partition gate: every max_quantum
# gives the committed one-cell reference hashes; the 10 ms run passes
# check_feedback.py --open. CI job `feedback`.
feedback-open: firmware bb
	bash scripts/partition.sh $(BB) $(FEEDBACK_OPEN_OUT) $(PARTITION_JOBS) $(FEEDBACK_OPEN_HASHES) \
	    $(FEEDBACK_OPEN_QUANTA) -- $(FEEDBACK_OPEN_ARGS)
	$(PYTHON) scripts/check_feedback.py $(FEEDBACK_OPEN_OUT)/mq-10ms --open
	$(BB) run $(FEEDBACK_OPEN_ARGS) --engine-order analog-first --out $(FEEDBACK_OPEN_OUT)/swapped
	set -e; for f in nets.csv events.jsonl nets.vcd; do cmp $(FEEDBACK_OPEN_OUT)/mq-10ms/$$f $(FEEDBACK_OPEN_OUT)/swapped/$$f; done
	@echo "feedback-open: PASS (partition gate; registration order swapped: identical)"

# The feedback-open one-cell reference (max_quantum = the lattice), for
# golden-update.
feedback-open-ref: firmware bb
	$(BB) run $(FEEDBACK_OPEN_ARGS) --max-quantum 10us --out $(FEEDBACK_OPEN_OUT)-ref
	$(PYTHON) scripts/check_feedback.py $(FEEDBACK_OPEN_OUT)-ref --open

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all -- --check

clippy:
	$(CARGO) clippy --workspace --all-targets -- -D warnings

# Everything CI runs, in CI order.
check: fmt-check clippy test
