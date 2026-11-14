# Breadboard — top-level build entry points. Run inside WSL / Linux.
SHELL := /bin/bash

export CARGO_TARGET_DIR ?= $(HOME)/.cache/bb-target
export PATH := $(HOME)/.cargo/bin:$(HOME)/.local/bin:$(PATH)

CARGO ?= cargo

.PHONY: setup doctor build test cargo-test fmt fmt-check clippy check firmware \
        bb spike spike-verify-every lattice-check partition perf determinism golden \
        golden-update feedback-run feedback feedback-order feedback-determinism \
        feedback-open feedback-open-ref lookahead-inert lookahead-startup \
        feedback-ref lookahead-equiv lookahead-perf adc-run adc adc-ref adc-open-ref \
        adc-equiv adc-renode adc-model adc-startup adc-stock adc-order adc-determinism pooled-adc-determinism adc-stock-ref adc-open-stock-ref adc-speed \
        cost-table cost-table-runs startup-perf ec-reconnect ec-long \
        pool-equiv pool-perf dms-source dms-suite dms-suite-check dms-suite-update \
        packages locks dms-values setup-container \
        adc-kicad adc-kicad-prepare adc-kicad-ref kicad-import cost-table-adc-kicad \
        pooled-test pooled-partition pooled-lattice-check pooled-feedback-open pooled-lookahead-inert pooled-lookahead-equiv pooled-adc-equiv pooled-spike-verify-every pooled-determinism pooled-feedback-determinism

# --- Phase 0 spike ---------------------------------------------------------
BB          := $(CARGO_TARGET_DIR)/release/bb
SPIKE_ELF   := examples/spike/firmware/build/blinky.elf
SPIKE_FOR   ?= 500ms
SPIKE_OUT   ?= out/spike
SPIKE_ARGS   = --netlist examples/spike/board.cir --firmware $(SPIKE_ELF) \
               --target U1 --for $(SPIKE_FOR) --probe net:LED_A --probe net:PA5
GOLDEN      := tests/golden/spike/led_a.csv
PARTITION_HASHES := tests/golden/spike/partition.sha256
# The default spike run's full outputs, grant records included (ADR-003 plan
# step 10): nets.csv, events.jsonl and nets.vcd of `make spike`.
SPIKE_HASHES := tests/golden/spike/outputs.sha256
DET_RUNS    ?= 10
DET_JOBS    ?= 5
PYTHON      ?= python3

# --- ADR-003 step 4: digital_in thresholding and the feedback boards --------
FEEDBACK_ELF  := examples/feedback/firmware/build/feedback.elf
FEEDBACK_FOR  ?= 500ms
FEEDBACK_OUT  ?= out/feedback
# The feedback board without its firmware: plan step 8 runs it with the
# EXTI/WFI firmware and with the polling variant (feedback_poll.elf).
FEEDBACK_BOARD_ARGS = --netlist examples/feedback/board.cir \
                 --target U1 --for $(FEEDBACK_FOR) --probe net:SENSE --probe net:LED_B \
                 --probe net:U1.PA5 --probe net:U1.PB0 --probe net:U1.PA6
FEEDBACK_ARGS  = --firmware $(FEEDBACK_ELF) $(FEEDBACK_BOARD_ARGS)
FEEDBACK_POLL_ELF  := examples/feedback/firmware/build/feedback_poll.elf
FEEDBACK_POLL_ARGS  = --firmware $(FEEDBACK_POLL_ELF) $(FEEDBACK_BOARD_ARGS)
FEEDBACK_GOLDEN := tests/golden/feedback
FEEDBACK_HASHES := $(FEEDBACK_GOLDEN)/outputs.sha256
FEEDBACK_OPEN_ARGS = --netlist examples/feedback-open/board.cir --firmware $(FEEDBACK_ELF) \
                 --target U1 --for $(FEEDBACK_FOR) --probe net:SENSE --probe net:U1.PB0 \
                 --probe net:U1.PA6
FEEDBACK_OPEN_OUT    ?= out/feedback-open
FEEDBACK_OPEN_HASHES := tests/golden/feedback-open/partition.sha256
FEEDBACK_OPEN_QUANTA ?= 10us,100us,1ms,10ms,50ms,500ms

# --- ADR-003 step 9: adc_in and the ADC boards (STM32F072) -----------------
ADC_ELF     := examples/adc/firmware/build/adc_board.elf
# ADR-003's board on Renode's stock ADC model (bb run --adc-model stock).
ADC_STOCK_ELF := examples/adc/firmware/build/adc_board_isr.elf
ADC_FOR     ?= 300ms
ADC_OUT     ?= out/adc-board
ADC_PROBES   = --probe net:ADC_IN --probe net:U1.PA4 --probe net:U1.PA5 \
               --probe net:U1.PA6 --probe net:U1.PA7
ADC_ARGS     = --netlist examples/adc/board.cir --firmware $(ADC_ELF) --target U1 \
               --for $(ADC_FOR) $(ADC_PROBES) --probe net:LED
ADC_OPEN_ARGS = --netlist examples/adc/board-open.cir --firmware $(ADC_ELF) --target U1 \
               --for $(ADC_FOR) $(ADC_PROBES)
ADC_GOLDEN  := tests/golden/adc
ADC_HASHES  := $(ADC_GOLDEN)/outputs.sha256
ADC_OPEN_HASHES := tests/golden/adc-open/outputs.sha256
# The stock-model board (ADR-003 step 9): its own one-cell references.
ADC_STOCK_FLAGS = --adc-model stock --firmware $(ADC_STOCK_ELF)
ADC_STOCK_ARGS  = $(ADC_ARGS) $(ADC_STOCK_FLAGS)
ADC_OPEN_STOCK_ARGS = $(ADC_OPEN_ARGS) $(ADC_STOCK_FLAGS)
ADC_STOCK_HASHES := tests/golden/adc-stock/outputs.sha256
ADC_OPEN_STOCK_HASHES := tests/golden/adc-open-stock/outputs.sha256

# --- ADR-004 Part B: make test and the gates on a warm Renode pool ---------
# Each target below runs its recipe (pooled-<target>) under `bb pool run`
# (scripts/with_pool.sh), so its bb runs and the Renode engine tests lease
# pre-warmed Renode processes; a nested make joins the pool already in
# BB_RENODE_POOL. A run that finds no idle process starts Renode fresh, and
# the outputs are the same either way (make pool-equiv). POOL=0 runs
# everything fresh (CI job `fresh`); POOL_SLOTS sets the pool size.
POOL       ?= 1
POOL_SLOTS ?= 5
ifeq ($(POOL),0)
WITH_POOL = env -u BB_RENODE_POOL
else
WITH_POOL = bash scripts/with_pool.sh $(BB) $(POOL_SLOTS) --
endif
POOLED_TARGETS := test partition lattice-check feedback-open lookahead-inert lookahead-equiv adc-equiv spike-verify-every determinism feedback-determinism adc-determinism \
                  nrf-gates nrf-feedback dms-rail dms-rail-test dms-vsense dms-vsense-test
$(POOLED_TARGETS): firmware bb
	$(WITH_POOL) $(MAKE) --no-print-directory pooled-$@

# `make setup` installs the developer toolchain and bb (into ~/.local/bin);
# `make setup SETUP=demo` skips the ARM toolchain and uv (Phase 1 plan M3,
# R7). Root (a container) runs apt without sudo.
SETUP ?= full
setup:
	bash scripts/setup.sh $(SETUP)

doctor:
	@bash scripts/doctor.sh $(SETUP)

# Bare-metal STM32F103 blinky used by the Phase 0 spike and the Renode engine
# tests: examples/spike/firmware/build/blinky.elf
firmware:
	$(MAKE) -C examples/spike/firmware
	$(MAKE) -C examples/feedback/firmware
	$(MAKE) -C engines/cpu-renode/tests/firmware
	$(MAKE) -C examples/adc/firmware
	$(MAKE) -C engines/cpu-renode/tests/firmware/nrf52840

build:
	$(CARGO) build --workspace --all-targets

# BB_REQUIRE_RENODE=1: the Renode engine tests fail instead of skipping when
# Renode or the firmware toolchain is missing, so the gate really runs them.
cargo-test:
	BB_REQUIRE_RENODE=1 $(CARGO) test --workspace

# The exit gate: unit/engine tests, then the end-to-end spike, 10-run
# determinism and the golden-waveform diff (golden reuses the spike output).
pooled-test:
	$(MAKE) cargo-test
	$(MAKE) spike
	$(MAKE) determinism
	$(MAKE) golden
	$(MAKE) feedback
	$(MAKE) feedback-order
	$(MAKE) nrf-feedback
	$(MAKE) adc
	$(MAKE) adc-order
	$(MAKE) adc-stock
	$(MAKE) adc-model
	$(MAKE) nrf-saadc-model
	$(MAKE) adc-kicad
	$(MAKE) dms-rail-test
	$(MAKE) dms-vsense-test
	$(MAKE) ec-reconnect
	$(MAKE) packages
	$(MAKE) gdb-test

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
pooled-spike-verify-every: spike
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
pooled-lattice-check: spike
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
pooled-partition: firmware bb
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

pooled-determinism: firmware bb
	bash scripts/determinism.sh $(BB) out/determinism $(DET_RUNS) $(DET_JOBS) -- $(SPIKE_ARGS)

# Diff LED_A against the committed snapshot (1 mV / 1 sample), then check
# the default run's nets.csv, events.jsonl and nets.vcd against their
# committed hashes ($(SPIKE_HASHES); the events.jsonl hash pins the grant
# records of the default 10 ms partition too). Runs the spike first if its
# output is missing.
golden:
	@if [ ! -f $(SPIKE_OUT)/nets.csv ]; then $(MAKE) spike; fi
	$(PYTHON) scripts/golden.py check $(SPIKE_OUT)/nets.csv $(GOLDEN)
	cd $(SPIKE_OUT) && sha256sum -c $(CURDIR)/$(SPIKE_HASHES)

# Regenerate the snapshots from fresh, checked runs (review the diff!): the
# spike waveform, output hashes and partition hashes; the feedback waveforms
# and edge list; the hashes of the feedback board's one-cell reference
# (`--lookahead off`, which the polling firmware shares); the feedback-open
# partition hashes (from its one-cell reference); and the hashes of the ADC
# boards' every-lattice-point references.
golden-update: spike feedback-run feedback-ref feedback-open-ref adc-ref adc-open-ref \
               adc-stock-ref adc-open-stock-ref adc-kicad-ref nrf-feedback-ref
	mkdir -p $(dir $(GOLDEN))
	$(PYTHON) scripts/golden.py write $(SPIKE_OUT)/nets.csv $(GOLDEN)
	cd $(SPIKE_OUT) && sha256sum nets.csv events.jsonl nets.vcd > $(CURDIR)/$(SPIKE_HASHES)
	cd $(SPIKE_OUT) && grep -v '"type":"grant"' events.jsonl > non-grant-records \
	    && sha256sum nets.csv nets.vcd non-grant-records > $(CURDIR)/$(PARTITION_HASHES) \
	    && rm non-grant-records
	mkdir -p $(FEEDBACK_GOLDEN) $(dir $(FEEDBACK_OPEN_HASHES))
	$(PYTHON) scripts/golden.py write $(FEEDBACK_OUT)/nets.csv $(FEEDBACK_GOLDEN)/sense.csv --net SENSE
	$(PYTHON) scripts/golden.py write $(FEEDBACK_OUT)/nets.csv $(FEEDBACK_GOLDEN)/led_b.csv --net LED_B
	grep '"type":"pin_edge"' $(FEEDBACK_OUT)/events.jsonl > $(FEEDBACK_GOLDEN)/pin_edges.jsonl
	cd $(FEEDBACK_OUT)-ref && grep -v '"type":"grant"' events.jsonl > non-grant-records \
	    && sha256sum nets.csv events.jsonl nets.vcd non-grant-records > $(CURDIR)/$(FEEDBACK_HASHES) \
	    && rm non-grant-records
	cd $(FEEDBACK_OPEN_OUT)-ref && grep -v '"type":"grant"' events.jsonl > non-grant-records \
	    && sha256sum nets.csv nets.vcd non-grant-records > $(CURDIR)/$(FEEDBACK_OPEN_HASHES) \
	    && rm non-grant-records
	mkdir -p $(ADC_GOLDEN) $(dir $(ADC_OPEN_HASHES))
	cd $(ADC_OUT)-ref && grep -v '"type":"grant"' events.jsonl > non-grant-records \
	    && sha256sum nets.csv events.jsonl nets.vcd non-grant-records > $(CURDIR)/$(ADC_HASHES) \
	    && rm non-grant-records
	mkdir -p $(dir $(ADC_STOCK_HASHES)) $(dir $(ADC_OPEN_STOCK_HASHES))
	cd $(ADC_OUT)-stock-ref && grep -v '"type":"grant"' events.jsonl > non-grant-records \
	    && sha256sum nets.csv events.jsonl nets.vcd non-grant-records > $(CURDIR)/$(ADC_STOCK_HASHES)
	cd $(ADC_OUT)-open-stock-ref && grep -v '"type":"grant"' events.jsonl > non-grant-records \
	    && sha256sum nets.csv events.jsonl nets.vcd non-grant-records > $(CURDIR)/$(ADC_OPEN_STOCK_HASHES)
	mkdir -p $(dir $(ADC_KICAD_HASHES))
	cd $(ADC_KICAD_OUT)-ref && grep -v '"type":"grant"' events.jsonl > non-grant-records \
	    && sha256sum nets.csv events.jsonl nets.vcd non-grant-records > $(CURDIR)/$(ADC_KICAD_HASHES)
	cd $(ADC_OUT)-open-ref && grep -v '"type":"grant"' events.jsonl > non-grant-records \
	    && sha256sum nets.csv events.jsonl nets.vcd non-grant-records > $(CURDIR)/$(ADC_OPEN_HASHES) \
	    && rm non-grant-records
	mkdir -p $(dir $(NRF_FEEDBACK_HASHES))
	cd $(NRF_FEEDBACK_OUT)-ref && grep -v '"type":"grant"' events.jsonl > non-grant-records \
	    && sha256sum nets.csv events.jsonl nets.vcd non-grant-records > $(CURDIR)/$(NRF_FEEDBACK_HASHES) \
	    && rm non-grant-records
	$(MAKE) --no-print-directory dms-rail-update
	$(MAKE) --no-print-directory dms-vsense-update

# ADR-003 step 4: examples/feedback, a board with a feedback loop (U1.PA5 ->
# RC -> U1.PB0 digital_in; the EXTI0 ISR copies PB0 to PA6 -> LED). The loop
# caps a grant at one Renode quantum unless the sleeping MCU promises to stay
# quiet: since step 8, lookahead is on by default for such boards (ADR-003
# D5). check_feedback.py: PB0 transitions at the analytic RC crossings (in
# their lattice cells), PA6 edges in (ceil_L(t_x), ceil_L(t_x) + L], LED_B
# plateaus.
feedback-run: firmware bb
	$(BB) run $(FEEDBACK_ARGS) --out $(FEEDBACK_OUT)
	$(PYTHON) scripts/check_feedback.py $(FEEDBACK_OUT)

# ... plus the golden snapshot: SENSE and LED_B within 1 mV / 1 sample and
# the exact pin_edge list; and (step 8) nets.csv, nets.vcd and the non-grant
# records byte-identical to the committed one-cell reference
# ($(FEEDBACK_HASHES), `--lookahead off`): lookahead changes only grants.
feedback: feedback-run
	$(PYTHON) scripts/golden.py check $(FEEDBACK_OUT)/nets.csv $(FEEDBACK_GOLDEN)/sense.csv --net SENSE
	$(PYTHON) scripts/golden.py check $(FEEDBACK_OUT)/nets.csv $(FEEDBACK_GOLDEN)/led_b.csv --net LED_B
	diff <(grep '"type":"pin_edge"' $(FEEDBACK_OUT)/events.jsonl) $(FEEDBACK_GOLDEN)/pin_edges.jsonl
	cd $(FEEDBACK_OUT) && grep -v '"type":"grant"' events.jsonl > non-grant-records \
	    && grep -v ' events.jsonl$$' $(CURDIR)/$(FEEDBACK_HASHES) | sha256sum -c
	@echo "feedback: golden snapshot PASS"

# The one-cell reference of examples/feedback (`--lookahead off`): its
# hashes are $(FEEDBACK_HASHES) (make golden-update).
feedback-ref: firmware bb
	$(BB) run $(FEEDBACK_ARGS) --lookahead off --out $(FEEDBACK_OUT)-ref
	$(PYTHON) scripts/check_feedback.py $(FEEDBACK_OUT)-ref

# Swapping the registration order (analog engine first) must give
# byte-identical outputs: the advance order comes from the bindings (ADR-003
# D3). Reuses the `feedback` run.
feedback-order: feedback
	$(BB) run $(FEEDBACK_ARGS) --engine-order analog-first --out $(FEEDBACK_OUT)-swapped
	set -e; for f in nets.csv events.jsonl nets.vcd; do cmp $(FEEDBACK_OUT)/$$f $(FEEDBACK_OUT)-swapped/$$f; done
	@echo "feedback-order: outputs identical with the engines registered in the other order"

# 10 runs of the feedback board (DET_RUNS, DET_JOBS): byte-identical
# outputs. CI job `feedback`.
pooled-feedback-determinism: firmware bb
	bash scripts/determinism.sh $(BB) out/feedback-determinism $(DET_RUNS) $(DET_JOBS) -- $(FEEDBACK_ARGS)

# examples/feedback-open, the acyclic variant (ngspice -> U1 only): long
# grants, inputs applied at ceil_L(t_x). Partition gate: every max_quantum
# gives the committed one-cell reference hashes; the 10 ms run passes
# check_feedback.py --open. CI job `feedback`.
pooled-feedback-open: firmware bb
	bash scripts/partition.sh $(BB) $(FEEDBACK_OPEN_OUT) $(PARTITION_JOBS) $(FEEDBACK_OPEN_HASHES) \
	    $(FEEDBACK_OPEN_QUANTA) -- $(FEEDBACK_OPEN_ARGS)
	$(PYTHON) scripts/check_feedback.py $(FEEDBACK_OPEN_OUT)/mq-10ms --open
	$(BB) run $(FEEDBACK_OPEN_ARGS) --engine-order analog-first --out $(FEEDBACK_OPEN_OUT)/swapped
	set -e; for f in nets.csv events.jsonl nets.vcd; do cmp $(FEEDBACK_OPEN_OUT)/mq-10ms/$$f $(FEEDBACK_OPEN_OUT)/swapped/$$f; done
	@echo "feedback-open: PASS (partition gate; registration order swapped: identical)"

# ADR-003 step 5: Renode's lookahead plugin is inert. examples/feedback
# without the plugin (--lookahead-plugin never, so lookahead off) must give
# the committed hashes ($(FEEDBACK_HASHES)); with the plugin loaded and armed
# (the default for a feedback loop) and --lookahead off it must be
# byte-identical; with lookahead on (the default since step 8) only the
# grant records may differ. CI job `feedback`.
LOOKAHEAD_INERT_OUT ?= out/lookahead-inert
pooled-lookahead-inert: firmware bb
	bash scripts/lookahead_inert.sh $(BB) $(LOOKAHEAD_INERT_OUT) $(FEEDBACK_HASHES) -- $(FEEDBACK_ARGS)
	$(BB) run $(SPIKE_ARGS) --lookahead-plugin always --out $(LOOKAHEAD_INERT_OUT)/spike
	cd $(LOOKAHEAD_INERT_OUT)/spike && grep -v '"type":"grant"' events.jsonl > non-grant-records \
	    && sha256sum -c $(CURDIR)/$(PARTITION_HASHES)
	@echo "lookahead-inert: the spike with the plugin loaded gives the committed partition hashes"

# ADR-003 step 8 equivalence gate: examples/feedback with lookahead on at
# max_quantum 1 ms and 10 ms must give its one-cell reference (--lookahead
# off, the committed $(FEEDBACK_HASHES)) byte for byte in nets.csv, nets.vcd
# and every non-grant record, with no broken promise and (10 ms) at most
# LOOKAHEAD_MAX_GRANTS grants per 500 ms; the same for the polling-firmware
# variant (feedback_poll.elf), whose one-cell outputs are those of the EXTI
# firmware byte for byte (both copy PB0 to PA6 in the slice that opens at
# ceil_L(t_x)), so it is held to the same hashes. CI job `feedback`.
LOOKAHEAD_EQUIV_OUT  ?= out/lookahead-equiv
LOOKAHEAD_MAX_GRANTS ?= 200
pooled-lookahead-equiv: firmware bb
	bash scripts/lookahead_equiv.sh $(BB) $(LOOKAHEAD_EQUIV_OUT) $(LOOKAHEAD_MAX_GRANTS) \
	    feedback=$(FEEDBACK_ELF):$(FEEDBACK_HASHES) poll=$(FEEDBACK_POLL_ELF):$(FEEDBACK_HASHES) \
	    -- $(FEEDBACK_BOARD_ARGS)

# ADR-003 step 8 performance gates (measurement protocol: interleaved A/B,
# PERF_N runs each, medians, startup separate, CPU time and load recorded):
#  1. examples/feedback, lookahead on (bb's defaults) against step 4 (one-cell
#     lockstep, no plugin): every run <= LOOKAHEAD_MAX_GRANTS grants and
#     >= 10x less simulation time;
#  2. the polling firmware, lookahead on against --lookahead off: at most
#     10 % more simulation time (the Query back-off, D5.5).
# Both also require identical outputs except grant records. Wall-clock: run
# it on an otherwise idle host; not in CI (like `make perf`).
LOOKAHEAD_PERF_OUT ?= out/lookahead-perf
lookahead-perf: firmware bb
	bash scripts/perf_ab.sh $(BB) $(LOOKAHEAD_PERF_OUT)/feedback $(PERF_N) \
	    "--lookahead off --lookahead-plugin never" "" $(LOOKAHEAD_MAX_GRANTS) 10 - -- $(FEEDBACK_ARGS)
	bash scripts/perf_ab.sh $(BB) $(LOOKAHEAD_PERF_OUT)/poll $(PERF_N) \
	    "--lookahead off" "" - - 1.10 -- $(FEEDBACK_POLL_ARGS)

# ADR-003 plan step 10: the README cost table, measured with the protocol
# (scripts/perf_ab.sh: interleaved A/B, PERF_N runs each, medians, startup
# separate, CPU time and load recorded). B is always bb's defaults; A is the
# one-cell run (the spike and feedback-open at --max-quantum 10us, the
# cyclic boards with --lookahead off). Every B run must match A in nets.csv,
# nets.vcd and the non-grant records. No speed gate (that is `make perf` and
# `make lookahead-perf`). Wall-clock: run it on an otherwise idle host.
# ADR-004: the table is measured twice, every Renode fresh ($(COST_OUT)/fresh)
# and on a 1-slot warm pool ($(COST_OUT)/pool, every run leased), for the
# startup columns.
COST_OUT ?= out/cost-table
cost-table: firmware bb
	env -u BB_RENODE_POOL $(MAKE) --no-print-directory cost-table-runs COST_RUN_OUT=$(COST_OUT)/fresh
	$(BB) pool run --slots 1 -- $(MAKE) --no-print-directory cost-table-runs COST_RUN_OUT=$(COST_OUT)/pool
cost-table-runs:
	bash scripts/perf_ab.sh $(BB) $(COST_RUN_OUT)/spike $(PERF_N) "--max-quantum 10us" "" - - - -- $(SPIKE_ARGS)
	bash scripts/perf_ab.sh $(BB) $(COST_RUN_OUT)/feedback $(PERF_N) "--lookahead off" "" - - - -- $(FEEDBACK_ARGS)
	bash scripts/perf_ab.sh $(BB) $(COST_RUN_OUT)/feedback-open $(PERF_N) "--max-quantum 10us" "" - - - \
	    -- $(FEEDBACK_OPEN_ARGS)
	bash scripts/perf_ab.sh $(BB) $(COST_RUN_OUT)/adc $(PERF_N) "--lookahead off" "" - - - -- $(ADC_ARGS)
	PERF_AB_EQUIV=0 bash scripts/perf_ab.sh $(BB) $(COST_RUN_OUT)/adc-stock $(PERF_N) "$(ADC_STOCK_FLAGS)" "" \
	    - - - -- $(ADC_ARGS)
	$(MAKE) --no-print-directory adc-kicad-prepare
	bash scripts/perf_ab.sh $(BB) $(COST_RUN_OUT)/adc-kicad $(PERF_N) "--lookahead off" "" - - - -- $(ADC_KICAD_ARGS)
	bash scripts/perf_ab.sh $(BB) $(COST_RUN_OUT)/nrf-feedback $(PERF_N) "--lookahead off" "" - - - \
	    -- $(NRF_FEEDBACK_ARGS)
	$(MAKE) --no-print-directory dms-rail-prepare
	bash scripts/perf_ab.sh $(BB) $(COST_RUN_OUT)/dms-rail $(PERF_N) "--max-quantum 10us" "" - - - \
	    -- $(DMS_RAIL_ARGS)
	bash scripts/perf_ab.sh $(BB) $(COST_RUN_OUT)/nrf-saadc $(PERF_N) "--lookahead off" "" - - - \
	    -- $(NRF_SAADC_ARGS)
	$(MAKE) --no-print-directory dms-vsense-prepare
	bash scripts/perf_ab.sh $(BB) $(COST_RUN_OUT)/dms-vsense $(PERF_N) "--lookahead off --adc-push every" "" \
	    - - - -- $(DMS_VSENSE_ARGS)

# The Phase 1 M8 row alone (examples/nrf-saadc, the loading adc_in): lookahead off (one cell) vs
# bb's defaults, fresh and on a 1-slot pool, with the protocol. Run it on an otherwise idle host.
cost-table-nrf-saadc: firmware bb
	env -u BB_RENODE_POOL bash scripts/perf_ab.sh $(BB) $(COST_OUT)/fresh/nrf-saadc $(PERF_N) "--lookahead off" "" \
	    - - - -- $(NRF_SAADC_ARGS)
	$(BB) pool run --slots 1 -- bash scripts/perf_ab.sh $(BB) $(COST_OUT)/pool/nrf-saadc $(PERF_N) \
	    "--lookahead off" "" - - - -- $(NRF_SAADC_ARGS)

# The Phase 1 M6 row alone (E1, examples/dial-me-spinner, ok-cold): one-cell vs bb's defaults, fresh
# and on a 1-slot pool, with the protocol. Run it on an otherwise idle host.
cost-table-dms-rail: firmware bb dms-rail-prepare
	env -u BB_RENODE_POOL bash scripts/perf_ab.sh $(BB) $(COST_OUT)/fresh/dms-rail $(PERF_N) "--max-quantum 10us" "" \
	    - - - -- $(DMS_RAIL_ARGS)
	$(BB) pool run --slots 1 -- bash scripts/perf_ab.sh $(BB) $(COST_OUT)/pool/dms-rail $(PERF_N) \
	    "--max-quantum 10us" "" - - - -- $(DMS_RAIL_ARGS)

# The Phase 1 M4 rows alone: examples/adc on board.cir and as the KiCad
# project, each with lookahead off vs on (the same protocol). Run it on an
# otherwise idle host.
cost-table-adc-kicad: firmware bb adc-kicad-prepare
	env -u BB_RENODE_POOL bash scripts/perf_ab.sh $(BB) $(COST_OUT)/fresh/adc $(PERF_N) "--lookahead off" "" - - - -- $(ADC_ARGS)
	env -u BB_RENODE_POOL bash scripts/perf_ab.sh $(BB) $(COST_OUT)/fresh/adc-kicad $(PERF_N) "--lookahead off" "" - - - -- $(ADC_KICAD_ARGS)

# ADR-005 step 6 speed gates (scripts/perf_ab.sh, interleaved, N = PERF_N,
# medians): examples/adc on Breadboard's ADC model (bb defaults) against the
# ADR-003 default (--adc-model stock, its firmware), in the same protocol
# run: at most 2000 grants per 300 ms and at least 3x less simulation time.
# The two models give different (each self-consistent) outputs, so the A/B
# equivalence is not checked here (it is make adc / make adc-stock). Run it
# on an otherwise idle host.
# ADR-005 D6 startup gate (scripts/plugin_startup.sh, fresh Renode,
# interleaved, N = ADC_STARTUP_N, medians of the engine startup, 20 ms runs):
# the ADC board on Breadboard's STM32F072 with a warm plugin cache within
# 0.3 s of the stock STM32F072 with the lookahead plugin, and a cold start
# (the plugin assembly compiled) within 3 s of a warm one. Run it on an
# otherwise idle host.
ADC_STARTUP_N ?= 5
adc-startup: firmware bb
	STOCK_FLAGS="$(ADC_STOCK_FLAGS)" bash scripts/plugin_startup.sh $(BB) out/adc-startup $(ADC_STARTUP_N) \
	    0.3 3 -- $(ADC_ARGS) --for 20ms

ADC_SPEED_OUT ?= out/adc-speed
adc-speed: firmware bb
	env -u BB_RENODE_POOL PERF_AB_EQUIV=0 bash scripts/perf_ab.sh $(BB) $(ADC_SPEED_OUT) $(PERF_N) \
	    "$(ADC_STOCK_FLAGS)" "" 2000 3 - -- $(ADC_ARGS)

# ADR-003 step 5 startup-cost gate: with the cached plugin, Renode engine
# startup is within 0.3 s of a start without it (interleaved, N =
# BB_LA_STARTUP_N runs each, default 7, medians). Wall-clock: run it on an
# otherwise idle host.
lookahead-startup: firmware
	BB_REQUIRE_RENODE=1 $(CARGO) test --release -p bb-cpu-renode --test lookahead -- \
	    --ignored --nocapture --exact plugin_startup_overhead

# ADR-004 G1 (Part A): fresh `bb run` engine startup of the spike and the
# feedback board at least STARTUP_MIN_FRAC (12 %) and STARTUP_MIN_S (0.3 s)
# below the same-session baseline, BB_RENODE_STOCK_RUNTIME=1 (the stock .NET
# settings, no JIT profile, the 100 ms connect poll); interleaved, N = STARTUP_N (7),
# medians, outputs identical. STARTUP_BASE=<path to another bb> compares
# against that binary instead (e.g. one built from the commit before
# ADR-004). Wall-clock: run it on an otherwise idle host.
STARTUP_OUT      ?= out/startup-perf
STARTUP_N        ?= 7
STARTUP_MIN_FRAC ?= 0.12
STARTUP_MIN_S    ?= 0.3
STARTUP_BASE     ?= stock
startup-perf: firmware bb
	bash scripts/startup_perf.sh $(BB) $(STARTUP_OUT)/spike $(STARTUP_N) $(STARTUP_MIN_FRAC) \
	    $(STARTUP_MIN_S) $(STARTUP_BASE) -- $(SPIKE_ARGS)
	bash scripts/startup_perf.sh $(BB) $(STARTUP_OUT)/feedback $(STARTUP_N) $(STARTUP_MIN_FRAC) \
	    $(STARTUP_MIN_S) $(STARTUP_BASE) -- $(FEEDBACK_ARGS)

# ADR-004 Amendment 9: the startup JIT profile's gather under load
# (scripts/jit_profile_stress.sh): JIT_STRESS_ROUNDS rounds of JIT_STRESS_PAR
# parallel gathering spike runs (each with an empty BB_CACHE_DIR) while
# JIT_STRESS_LOAD busy loops (default 2 x nproc) load every CPU; every run must
# leave exactly one profile. About 3 min at the defaults (32 runs). Not in
# make test: it loads the host on purpose.
.PHONY: jit-profile-stress
JIT_STRESS_ROUNDS ?= 4
JIT_STRESS_PAR    ?= 8
JIT_STRESS_LOAD   ?=
jit-profile-stress: firmware bb
	bash scripts/jit_profile_stress.sh $(BB) $(JIT_STRESS_ROUNDS) $(JIT_STRESS_PAR) $(JIT_STRESS_LOAD)

# --- ADR-004 Part B: the warm Renode pool ----------------------------------
# G4 pool equivalence (scripts/pool_equiv.sh): one pool process serves every
# configuration below, in alternating order over POOL_EQUIV_ROUNDS rounds;
# each pooled run must be leased and byte-identical (nets.csv, nets.vcd,
# events.jsonl, non-grant records) to a fresh run of the same configuration
# and to the committed hashes. A new board or platform must pass it before
# it uses the pool.
POOL_EQUIV_OUT    ?= out/pool-equiv
POOL_EQUIV_ROUNDS ?= 2
pool-equiv: firmware bb adc-kicad-prepare dms-rail-prepare dms-vsense-prepare
	$(BB) pool run --slots 1 -- bash scripts/pool_equiv.sh $(BB) $(POOL_EQUIV_OUT) $(POOL_EQUIV_ROUNDS) -- \
	    "dms-rail;$(DMS_RAIL_GOLD)/ok-cold/outputs.sha256;nongrant;$(DMS_RAIL_ARGS)" \
	    "dms-rail-ref;$(DMS_RAIL_GOLD)/ok-cold/outputs.sha256;all;$(DMS_RAIL_ARGS) --max-quantum 10us" \
	    "dms-rail-wrong;$(DMS_RAIL_GOLD)/wrong-nominal/outputs.sha256;nongrant;$(DMS_RAIL_WRONG_ARGS)" \
	    "spike;$(SPIKE_HASHES);all;$(SPIKE_ARGS)" \
	    "spike-1ms;$(PARTITION_HASHES);all;$(SPIKE_ARGS) --max-quantum 1ms" \
	    "spike-500ms;$(PARTITION_HASHES);all;$(SPIKE_ARGS) --max-quantum 500ms" \
	    "feedback;$(FEEDBACK_HASHES);nongrant;$(FEEDBACK_ARGS)" \
	    "feedback-off;$(FEEDBACK_HASHES);all;$(FEEDBACK_ARGS) --lookahead off" \
	    "feedback-noplugin;$(FEEDBACK_HASHES);all;$(FEEDBACK_ARGS) --lookahead-plugin never" \
	    "feedback-poll;$(FEEDBACK_HASHES);nongrant;$(FEEDBACK_POLL_ARGS)" \
	    "adc;$(ADC_HASHES);nongrant;$(ADC_ARGS)" \
	    "adc-off;$(ADC_HASHES);nongrant;$(ADC_ARGS) --lookahead off" \
	    "adc-open;$(ADC_OPEN_HASHES);nongrant;$(ADC_OPEN_ARGS)" \
	    "adc-stock;$(ADC_STOCK_HASHES);nongrant;$(ADC_STOCK_ARGS)" \
	    "adc-open-stock;$(ADC_OPEN_STOCK_HASHES);nongrant;$(ADC_OPEN_STOCK_ARGS)" \
	    "adc-kicad;$(ADC_KICAD_HASHES);nongrant;$(ADC_KICAD_ARGS)" \
	    "nrf-feedback;$(NRF_FEEDBACK_HASHES);nongrant;$(NRF_FEEDBACK_ARGS)" \
	    "nrf-feedback-off;$(NRF_FEEDBACK_HASHES);all;$(NRF_FEEDBACK_ARGS) --lookahead off" \
	    "nrf-feedback-noplugin;$(NRF_FEEDBACK_HASHES);all;$(NRF_FEEDBACK_ARGS) --lookahead-plugin never" \
	    "nrf-saadc;$(NRF_SAADC_HASHES);nongrant;$(NRF_SAADC_ARGS)" \
	    "nrf-saadc-ref;$(NRF_SAADC_HASHES);all;$(NRF_SAADC_ARGS) --lookahead off --adc-push every" \
	    "dms-vsense;$(DMS_VSENSE_GOLD)/s50/outputs.sha256;nongrant;$(DMS_VSENSE_ARGS)" \
	    "dms-vsense-ref;$(DMS_VSENSE_GOLD)/s50/outputs.sha256;all;$(DMS_VSENSE_ARGS) --lookahead off --adc-push every"

# G2 pool startup (scripts/pool_perf.sh): on a 1-slot pool after its
# pre-warm, the first lease starts in <= 0.5 s, then POOL_PERF_N interleaved
# rounds of the spike, the feedback board and the ADC board: median engine
# startup <= 0.30 s each, spike median total wall <= 1.2 s, every run
# leased. Wall-clock: run it on an otherwise idle host.
POOL_PERF_OUT ?= out/pool-perf
POOL_PERF_N   ?= 7
pool-perf: firmware bb
	$(BB) pool run --slots 1 -- bash scripts/pool_perf.sh $(BB) $(POOL_PERF_OUT) $(POOL_PERF_N) \
	    0.5 0.30 1.2 -- "spike;$(SPIKE_ARGS)" "feedback;$(FEEDBACK_ARGS)" "adc;$(ADC_ARGS)"

# --- Phase 1 M2: the nRF52840 platform (ADR-008 D1-D4) ----------------------
# nrf52840-bb@1 (engines/cpu-renode/platforms/nrf52840-bb.repl), the
# bare-metal gate firmware in $(NRF_FW) and the prebuilt Zephyr gate image
# in $(NRF_FW)/zephyr/prebuilt. examples/nrf-feedback is examples/feedback's
# circuit on the nRF52840 (RTC0 drives U1.P0.13; GPIOTE IN on U1.P0.11 copies
# to U1.P0.15).
.PHONY: nrf-gates nrf-feedback nrf-feedback-ref nrf-costs pooled-nrf-gates pooled-nrf-feedback
NRF_FW            := engines/cpu-renode/tests/firmware/nrf52840
NRF_FEEDBACK_ELF  := $(NRF_FW)/build/nrf_feedback.elf
NRF_FEEDBACK_FOR  ?= 500ms
NRF_FEEDBACK_OUT  ?= out/nrf-feedback
NRF_FEEDBACK_BOARD_ARGS = --netlist examples/nrf-feedback/board.cir --target U1 \
                 --for $(NRF_FEEDBACK_FOR) --probe net:SENSE --probe net:LED_B \
                 --probe net:U1.P0.13 --probe net:U1.P0.11 --probe net:U1.P0.15
NRF_FEEDBACK_ARGS  = --firmware $(NRF_FEEDBACK_ELF) $(NRF_FEEDBACK_BOARD_ARGS)
NRF_FEEDBACK_HASHES := tests/golden/nrf-feedback/outputs.sha256
NRF_MAX_GRANTS    ?= 200

# The board-level nrf-feedback gate (ADR-008 D3; in make test): with
# lookahead on at max_quantum 1 ms and 10 ms, nets.csv, nets.vcd and every
# non-grant record are byte-identical to the one-cell --lookahead off
# reference (the committed $(NRF_FEEDBACK_HASHES)), with no broken promise
# and at most NRF_MAX_GRANTS grants per 500 ms at 10 ms; the reference passes
# check_feedback.py --nrf (the analytic RC crossings); and the run with the
# engines registered in the other order is byte-identical, grant records
# included.
pooled-nrf-feedback: firmware bb
	CHECK_FEEDBACK_ARGS=--nrf bash scripts/lookahead_equiv.sh $(BB) $(NRF_FEEDBACK_OUT) \
	    $(NRF_MAX_GRANTS) nrf-feedback=$(NRF_FEEDBACK_ELF):$(NRF_FEEDBACK_HASHES) \
	    -- $(NRF_FEEDBACK_BOARD_ARGS)
	$(BB) run $(NRF_FEEDBACK_ARGS) --engine-order analog-first --out $(NRF_FEEDBACK_OUT)/order
	set -e; for f in nets.csv events.jsonl nets.vcd; do \
	    cmp $(NRF_FEEDBACK_OUT)/nrf-feedback/on-10ms/$$f $(NRF_FEEDBACK_OUT)/order/$$f; done
	@echo "nrf-feedback: --engine-order analog-first byte-identical to mcu-first"

# Phase 1 M2 gate (ADR-008 D3, the plan's M2 row): the Renode engine tests
# of engines/cpu-renode/tests/nrf52840.rs (nrf-blinky identical at one-cell
# grants, 10 ms grants, plugin armed and lookahead on, self-test status 0x3;
# nrf-ppi; nrf-busywait; the D6 TIMER-compare-then-long-sleep promise
# clamp; the PIN_CNF audit; nrf-zephyr) and the board-level nrf-feedback.
# `make pool-equiv` covers the nRF configurations.
pooled-nrf-gates: firmware bb
	mkdir -p out
	BB_REQUIRE_RENODE=1 $(CARGO) test -p bb-cpu-renode --test nrf52840 -- --nocapture 2>&1 \
	    | tee out/nrf-gates.log | grep -E '^(nrf-|cost:|test |test result)'; \
	    test $${PIPESTATUS[0]} -eq 0
	$(MAKE) --no-print-directory pooled-nrf-feedback
	@echo "nrf-gates: PASS"

# The nrf-feedback one-cell reference, for golden-update.
nrf-feedback-ref: firmware bb
	$(BB) run $(NRF_FEEDBACK_ARGS) --lookahead off --out $(NRF_FEEDBACK_OUT)-ref
	$(PYTHON) scripts/check_feedback.py $(NRF_FEEDBACK_OUT)-ref --nrf

# ADR-008 D1: fresh startup and per-grant cost of the nRF52840 blinky at 64
# MIPS against the STM32F103 blinky (interleaved, N = BB_NRF_COST_N, default
# 5, medians). Reports only. Run it on an otherwise idle host.
nrf-costs: firmware
	BB_REQUIRE_RENODE=1 $(CARGO) test --release -p bb-cpu-renode --test nrf52840 -- \
	    --ignored --nocapture --exact nrf_startup_and_grant_costs

# The feedback-open one-cell reference (max_quantum = the lattice), for
# golden-update.
feedback-open-ref: firmware bb
	$(BB) run $(FEEDBACK_OPEN_ARGS) --max-quantum 10us --out $(FEEDBACK_OPEN_OUT)-ref
	$(PYTHON) scripts/check_feedback.py $(FEEDBACK_OPEN_OUT)-ref --open

# ADR-005 (D7; ADR-003 step 9): examples/adc, an STM32F072 that reads back
# through its ADC (PA1, adc_in) the RC-filtered divider it drives (PA4), on
# Breadboard's ADC model: TIM3 TRGO triggers a conversion every 999 us (no
# TIM3 interrupt), the ADC interrupt drives ALARM (PA5) from the analog
# watchdog, the code bits 4 and 8 on PA6/PA7 and PA4 every 50 conversions.
# A feedback loop, so lookahead is on by default: ngspice pushes ADC_IN at
# the lattice points where its 12-bit code changes, as timed inputs of the
# model; the ADC pin is a sampled input, so quiet grants do not stop at a
# push (ADR-005 D3). check_adc.py: every conversion's edges as the firmware
# and the model's timing (s_k = T_k + 1063 ns, c_k = T_k + 4750 ns) predict
# from ADC_IN.
adc-run: firmware bb
	$(BB) run $(ADC_ARGS) --out $(ADC_OUT)
	$(PYTHON) scripts/check_adc.py $(ADC_OUT) --require-no-stops

# ... and nets.csv, nets.vcd and the non-grant records byte-identical to the
# committed one-cell reference that pushes at every lattice point
# ($(ADC_HASHES), `make adc-ref`). In `make test`.
adc: adc-run
	cd $(ADC_OUT) && grep -v '"type":"grant"' events.jsonl > non-grant-records \
	    && grep -v ' events.jsonl$$' $(CURDIR)/$(ADC_HASHES) | sha256sum -c
	@echo "adc: PASS (identical to the one-cell every-lattice-point reference except grant records)"

# The references: one cell, the ADC value pushed at every lattice point
# (their hashes: make golden-update).
adc-ref: firmware bb
	$(BB) run $(ADC_ARGS) --lookahead off --adc-push every --out $(ADC_OUT)-ref
	$(PYTHON) scripts/check_adc.py $(ADC_OUT)-ref
adc-open-ref: firmware bb
	$(BB) run $(ADC_OPEN_ARGS) --max-quantum 10us --adc-push every --out $(ADC_OUT)-open-ref
	$(PYTHON) scripts/check_adc.py $(ADC_OUT)-open-ref

# Swapping the registration order (analog engine first) must give
# byte-identical outputs (ADR-003 D3). Reuses the `adc` run. In `make test`.
adc-order: adc
	$(BB) run $(ADC_ARGS) --engine-order analog-first --out $(ADC_OUT)-swapped
	set -e; for f in nets.csv events.jsonl nets.vcd; do cmp $(ADC_OUT)/$$f $(ADC_OUT)-swapped/$$f; done
	@echo "adc-order: outputs identical with the engines registered in the other order"

# 10 runs of the ADC board (DET_RUNS, DET_JOBS): byte-identical outputs.
pooled-adc-determinism: firmware bb
	bash scripts/determinism.sh $(BB) out/adc-determinism $(DET_RUNS) $(DET_JOBS) -- $(ADC_ARGS)

# ADR-003's board on Renode's stock ADC (`--adc-model stock`, approver
# decision 4 of ADR-005: kept for comparisons): the defaults must give its
# committed one-cell references ($(ADC_STOCK_HASHES), the ADR-003 hashes).
adc-stock: firmware bb
	$(BB) run $(ADC_STOCK_ARGS) --out $(ADC_OUT)-stock
	$(PYTHON) scripts/check_adc.py $(ADC_OUT)-stock --model stock
	cd $(ADC_OUT)-stock && grep -v '"type":"grant"' events.jsonl > non-grant-records \
	    && grep -v ' events.jsonl$$' $(CURDIR)/$(ADC_STOCK_HASHES) | sha256sum -c
	@echo "adc-stock: PASS (identical to the stock model's one-cell reference except grant records)"
adc-stock-ref: firmware bb
	$(BB) run $(ADC_STOCK_ARGS) --lookahead off --adc-push every --out $(ADC_OUT)-stock-ref
	$(PYTHON) scripts/check_adc.py $(ADC_OUT)-stock-ref --model stock
adc-open-stock-ref: firmware bb
	$(BB) run $(ADC_OPEN_STOCK_ARGS) --max-quantum 10us --adc-push every --out $(ADC_OUT)-open-stock-ref
	$(PYTHON) scripts/check_adc.py $(ADC_OUT)-open-stock-ref --model stock

# ADR-005 step 5 gates (scripts/adc_equiv.sh): Part A (one cell) and Part B
# (lookahead, 1 ms and 10 ms) on the cyclic board, and the open board in
# long grants (1, 10, 50 ms), all byte-identical to the every-lattice-point
# references (committed hashes) except grant records; no promise broken; no
# early stop at a push (the ADC pin is a sampled input). CI job `adc`.
ADC_EQUIV_OUT ?= out/adc-equiv
pooled-adc-equiv: firmware bb
	bash scripts/adc_equiv.sh $(BB) $(ADC_EQUIV_OUT) $(ADC_HASHES) $(ADC_OPEN_HASHES) \
	    "$(ADC_ARGS)" -- "$(ADC_OPEN_ARGS)"

# ADR-004 Amendment 8: Renode 1.17.0's External Control server crashes after
# about 524 000 messages on one connection, so the engine reconnects every
# BB_RENODE_EC_MESSAGES (default 100 000) while Renode is paused.
# ec-reconnect (in `make test`): the one-cell references of examples/adc
# (sampled ADC input, host port, GPIO outputs) and examples/feedback (GPIO
# input SET_STATE) reconnecting every $(EC_MESSAGES) messages must give their
# committed hashes. ec-long: the ADC board's one-cell reference over
# $(EC_LONG_FOR) (it used to crash after 81 746 grants) must finish and equal
# the lookahead run except grant records (scripts/ec_long.sh, about 2.5 min).
EC_OUT      ?= out/ec-reconnect
EC_MESSAGES ?= 2000
EC_LONG_FOR ?= 1500ms
EC_LONG_OUT ?= out/ec-long
ec-reconnect: firmware bb
	rm -rf $(EC_OUT) && mkdir -p $(EC_OUT)
	BB_RENODE_EC_MESSAGES=$(EC_MESSAGES) $(BB) run $(ADC_ARGS) --lookahead off --adc-push every \
	    --out $(EC_OUT)/adc > $(EC_OUT)/adc.log 2>&1 || { tail -8 $(EC_OUT)/adc.log; exit 1; }
	BB_RENODE_EC_MESSAGES=$(EC_MESSAGES) $(BB) run $(FEEDBACK_ARGS) --lookahead off \
	    --out $(EC_OUT)/feedback > $(EC_OUT)/feedback.log 2>&1 || { tail -8 $(EC_OUT)/feedback.log; exit 1; }
	grep 'External Control reconnected' $(EC_OUT)/adc.log $(EC_OUT)/feedback.log
	cd $(EC_OUT)/adc && grep -v '"type":"grant"' events.jsonl > non-grant-records \
	    && grep -v ' events.jsonl$$' $(CURDIR)/$(ADC_HASHES) | sha256sum -c
	cd $(EC_OUT)/feedback && grep -v '"type":"grant"' events.jsonl > non-grant-records \
	    && grep -v ' events.jsonl$$' $(CURDIR)/$(FEEDBACK_HASHES) | sha256sum -c
	@echo "ec-reconnect: PASS (one-cell references identical with a reconnect every $(EC_MESSAGES) messages)"
ec-long: firmware bb
	bash scripts/ec_long.sh $(BB) $(EC_LONG_OUT) -- $(subst --for $(ADC_FOR),--for $(EC_LONG_FOR),$(ADC_ARGS))

# --- Phase 1 M8 (first half): Breadboard's nRF52840 SAADC (ADR-008 D7/D8) --
# nrf52840-bb@2: BbNrf52Saadc (BURST/oversampling, TACQ, EasyDMA, the TACQ
# >= 2 L refusal) with its host port, the loading adc_in (one #smp toggle
# per acquisition into the package's pin_load view saadc_se), per-sample
# log records, the VDD input and the UARTE tap. make nrf-saadc-model (in
# make test, about 25 s): the Renode-only register and timing tests
# (examples/nrf-saadc/bb_saadc_test.py, ADR-005 test-plan shape; twice,
# byte-identical), then the board gates of scripts/nrf_saadc_board.sh on
# examples/nrf-saadc (8 #smp toggles and 8 charge transfers per 8x burst,
# against the analytic charge sharing; every partition, lookahead and
# engine order equal to the every-lattice-point one-cell reference
# $(NRF_SAADC_HASHES); the refusal and its message; the load=none escape).
.PHONY: nrf-saadc-model nrf-saadc-update cost-table-nrf-saadc
NRF_SAADC_ELF    := $(NRF_FW)/build/nrf_saadc_burst.elf
NRF_SAADC_OUT    ?= out/nrf-saadc
NRF_SAADC_HASHES := tests/golden/nrf-saadc/outputs.sha256
NRF_SAADC_ARGS    = --netlist examples/nrf-saadc/board.cir --target U1 --firmware $(NRF_SAADC_ELF) \
                    --for 30ms --probe net:AIN --probe net:MARK
nrf-saadc-model: firmware bb
	$(PYTHON) examples/nrf-saadc/bb_saadc_test.py --out out/nrf-saadc-model
	bash scripts/nrf_saadc_board.sh $(BB) $(NRF_SAADC_OUT) $(NRF_SAADC_HASHES)
	@echo "nrf-saadc-model: PASS"
# Rewrite the reference hashes from a fresh, checked run (review the diff).
nrf-saadc-update: firmware bb
	bash scripts/nrf_saadc_board.sh $(BB) $(NRF_SAADC_OUT) $(NRF_SAADC_HASHES) update

# ADR-005 test plan 1-3: Renode-only tests of Breadboard's STM32F072 ADC
# model and BbStm32Timer TRGO (examples/adc/bb_adc_test.py: pwm, trgo,
# registers, firmware; about 80 s). In `make test`.
adc-model: firmware
	$(MAKE) -C examples/adc model-test

# The Renode-only ADC platform test of ADR-003 Amendment 5 (the stock
# model; about 15 s).
adc-renode:
	$(MAKE) -C examples/adc test

# --- Phase 1 M0: dial-me-spinner reference inputs and its SPICE suite -----
# The reference board is dial-me-spinner 6854d25 (plan, approver decision C).
# DMS_REPO is a local dial-me-spinner repository; it is only read with
# `git archive` / `git show`, never checked out.
DMS_REPO          ?= $(abspath $(CURDIR)/../dial-me-spinner)
DMS_SUITE_OUT     ?= out/dms-suite
DMS_SUITE_RESULTS := examples/dial-me-spinner/suite/results.json

# The vendored KiCad inputs against examples/dial-me-spinner/SOURCE (and,
# when DMS_REPO exists, each file against git show <commit>:<path>).
dms-source:
	bash scripts/dms_source_check.sh $(wildcard $(DMS_REPO))

# Re-run the suite (`-m "not slow"`) at the reference commit from a git
# archive export in ~/.cache/bb-dms-suite (about 4-5 min on 12 jobs).
dms-suite:
	bash scripts/dms_suite.sh --repo $(DMS_REPO) --out $(DMS_SUITE_OUT)/results.json

# M0 gate: a fresh run reproduces the committed results.json byte for byte
# (checks, test outcomes, decks and their fallback levels, tool versions).
dms-suite-check: dms-suite
	@if cmp -s $(DMS_SUITE_OUT)/results.json $(DMS_SUITE_RESULTS); then \
	    echo "dms-suite-check: PASS ($(DMS_SUITE_OUT)/results.json identical to $(DMS_SUITE_RESULTS), sha256 $$(sha256sum < $(DMS_SUITE_RESULTS) | cut -c1-16))"; \
	else diff -u $(DMS_SUITE_RESULTS) $(DMS_SUITE_OUT)/results.json | head -60; \
	    echo "dms-suite-check: FAIL (results differ)"; exit 1; fi

# Regenerate the committed results.json (review the diff: it is the source
# of the package test vectors, ADR-006 D9 rule 5).
dms-suite-update: dms-suite
	cp $(DMS_SUITE_OUT)/results.json $(DMS_SUITE_RESULTS)

# Phase 1 plan S1 (ADR-006 Amendment 2): the E1 deck through NgspiceEngine
# at phase0 with the S1 fix of the TPS22917 view. The fixed view and every
# deck are TI-derived: they stay in ~/.cache/bb-dms-s1 (local-only, never
# in the repository), and the reference is read with git archive. About
# 1.5 min on 8 jobs.
.PHONY: dms-s1 dms-s1-cost
dms-s1:
	DMS_REPO=$(DMS_REPO) bash scripts/dms_s1.sh gate

# The S1 cost table (plan §8): DMS_S1_N interleaved rounds (default 5),
# medians, load reported. Run it on an otherwise idle host.
DMS_S1_N ?= 5
dms-s1-cost:
	DMS_REPO=$(DMS_REPO) bash scripts/dms_s1.sh cost $(DMS_S1_N)

# --- Phase 1 M4: KiCad import, board.bbm, projects (ADR-007) ---------------
# examples/adc drawn in KiCad (examples/adc/kicad), as a bb.toml project. Its
# runs use a copy off /mnt/c with the firmware recorded by `bb flash`
# ($(ADC_KICAD_WORK)/net; `make adc-kicad-prepare` makes it).
ADC_KICAD_WORK ?= $(HOME)/.cache/bb-adc-kicad
ADC_KICAD_OUT  ?= out/adc-kicad
ADC_KICAD_ARGS  = --project $(ADC_KICAD_WORK)/net --for $(ADC_FOR)
ADC_KICAD_HASHES := tests/golden/adc-kicad/outputs.sha256

# Gate M4a (scripts/adc_kicad.sh; in `make test`): board.bbm current; the
# project run equal to its one-cell reference and to the committed
# $(ADC_KICAD_HASHES); the .net and kicad-cli routes byte-identical (when
# kicad-cli is installed; KICAD_REQUIRED=1 makes it a failure otherwise);
# against board.cir's committed reference the non-grant records byte for
# byte and nets.csv/nets.vcd within 1 mV with digital values exact
# (ADR-007 D9's fallback; the cause, C1's 470n against 4.7e-7, is checked
# byte for byte); bb run writes nothing into the project.
adc-kicad: firmware bb
	ADC_KICAD_WORK=$(ADC_KICAD_WORK) bash scripts/adc_kicad.sh $(BB) $(ADC_KICAD_OUT)
adc-kicad-prepare: firmware bb
	ADC_KICAD_WORK=$(ADC_KICAD_WORK) bash scripts/adc_kicad.sh --prepare $(BB)
# The project's one-cell reference (lookahead off, ADC pushed at every
# lattice point), for golden-update.
adc-kicad-ref: adc-kicad-prepare
	$(BB) run $(ADC_KICAD_ARGS) --lookahead off --adc-push every --out $(ADC_KICAD_OUT)-ref
	$(PYTHON) scripts/check_adc.py $(ADC_KICAD_OUT)-ref

# Gates M4b-M4f on the dial-me-spinner main board at 6854d25
# (scripts/kicad_import.sh, about 20 s): counts, resolution, one board.bbm
# from the committed netlist, fresh Linux and Windows kicad-cli exports and
# a partial KiCad 10 re-save (dc96515's KiCad 9 netlist as the secondary
# check), byte-identical re-imports, unchanged sources, and the version
# refusals. Needs KiCad 10 in WSL and, unless KICAD_WINDOWS=0, the Windows
# kicad-cli.exe. Not in `make test` (CI has no KiCad); the synthetic
# two-board fixture (cli/tests/kicad_import.rs) is.
kicad-import: bb
	bash scripts/kicad_import.sh $(BB) out/kicad-import

# --- Phase 1 M3: component packages (ADR-006) ------------------------------
# The example boards whose parts come from packages; each has a bb.lock.
PKG_BOARDS := examples/spike/board.cir examples/feedback/board.cir \
              examples/feedback-open/board.cir examples/adc/board.cir examples/adc/board-open.cir \
              examples/adc/kicad examples/dial-me-spinner examples/nrf-saadc/board.cir
# Project-local packages (ADR-006 D1), linted and tested with the first-party ones.
PKG_LOCAL := examples/adc/kicad/packages

# ADR-006 G1 and the package gates of `make test`: every part.toml, pins.toml,
# test vector and bb.lock validates against spec/schema (python3-jsonschema);
# `bb pkg lint` is clean; every vector passes through NgspiceEngine at both
# grant partitions and at every recorded qualification (`bb pkg test`); and
# every example's bb.lock is current (`bb open --check`).
packages: bb
	$(PYTHON) scripts/validate_packages.py
	$(BB) pkg lint packages $(PKG_LOCAL)
	$(BB) pkg test packages $(PKG_LOCAL)
	set -e; for b in $(PKG_BOARDS); do $(BB) open --check $$b; done
	bash scripts/check_local_only.sh
	bash scripts/check_local_only_test.sh
	@echo "packages: PASS"

# --- Phase 1 M5/M5b: the E1 packages (ADR-006 D9, Amendment 3) ------------
# The committed vectors of packages/ti/tps22917 and packages/worldsemi/
# ws2812c-2020 are generated from the dial-me-spinner suite at 6854d25 (its
# own deck builders, git archive export; scripts/dms_packages.sh).
# dms-vectors rewrites them; dms-vectors-check requires the committed ones
# byte for byte (needs DMS_REPO and the suite's Python, like dms-s1).
.PHONY: dms-vectors dms-vectors-check dms-local-packages dms-m5b
dms-vectors: bb
	DMS_REPO=$(DMS_REPO) bash scripts/dms_packages.sh vectors $(BB)
dms-vectors-check: bb
	DMS_REPO=$(DMS_REPO) bash scripts/dms_packages.sh check $(BB)
# The LOCAL-ONLY vendor-derived ti/tps22917 (approver decision A3): built in
# the git-ignored local-packages/ (DMS_LOCAL_PACKAGES) from git archive with
# the S1 fix and the GND -> GNDP rename, qualified by `bb pkg test --record`
# (its vectors reproduce results.json), and gate G8 (the rename gives
# byte-identical vector results). Never commit local-packages/.
dms-local-packages: bb
	DMS_REPO=$(DMS_REPO) bash scripts/dms_packages.sh local $(BB)
# Plan M5b's gate: the committed datasheet-fitted TPS22917 view against the
# local vendor-derived one on the E1 checks (VLED at each P0.06 rise and the
# LSW-3 maximum within 10 mV, LSW-2 within 1 ms, the wrong-order excursion
# within 10 mV), both benches, through NgspiceEngine at phase0. About 30 s.
dms-m5b: dms-local-packages
	DMS_REPO=$(DMS_REPO) bash scripts/dms_packages.sh m5b $(BB)

# --- Phase 1 M6: E1, LED rail sequencing on dial-me-spinner rev A ----------
# examples/dial-me-spinner: the imported main board, the Zephyr E1 firmware
# (firmware/prebuilt/led_rail.elf and led_rail_wrong_order.elf, ADR-008 D5)
# on U10's nRF52840, bench/nominal.toml and bench/cold.toml. Runs use copies
# of the project off /mnt/c (scripts/dms_rail.sh; $(DMS_RAIL_WORK)).
.PHONY: dms-rail dms-rail-test pooled-dms-rail pooled-dms-rail-test dms-rail-ref dms-rail-update \
        dms-rail-prepare dms-crosscheck fw-rebuild dms-firmware cost-table-dms-rail
DMS_RAIL_OUT   ?= out/dms-rail
DMS_RAIL_WORK  ?= $(HOME)/.cache/bb-dms-rail
export DMS_RAIL_WORK
DMS_RAIL_GOLD  := tests/golden/dms-rail
DMS_RAIL_ARGS   = --project $(DMS_RAIL_WORK)/proj-ok --bench $(DMS_RAIL_WORK)/proj-ok/bench/cold.toml --for 120ms
DMS_RAIL_WRONG_ARGS = --project $(DMS_RAIL_WORK)/proj-wrong --bench $(DMS_RAIL_WORK)/proj-wrong/bench/nominal.toml \
                 --for 120ms

# In `make test`: both firmwares at both benches at bb's defaults: check_dms_rail.py (the correct
# order passes LED-DATA-V4, LSW-3 and LSW-2; the wrong order fails exactly LSW-3), the committed
# one-cell hashes, the VLED snapshots, a clean PIN_CNF audit, and analog-first byte-identical.
pooled-dms-rail-test: bb
	bash scripts/dms_rail.sh test $(BB) $(DMS_RAIL_OUT)-test

# The plan's M6 gate: the above, plus every case's one-cell reference (--max-quantum 10us; equal
# to the default run except grants and to the committed hashes), analog-first on every case, the
# board-level convergence gate (120 ms in under 30 s at both engine orders), the PIN_CNF audit's
# negative controls and 10-run determinism. About 3 min fresh.
pooled-dms-rail: bb
	bash scripts/dms_rail.sh gate $(BB) $(DMS_RAIL_OUT)

# The one-cell references and, from them, tests/golden/dms-rail (golden-update runs it).
dms-rail-ref: bb
	bash scripts/dms_rail.sh ref $(BB) $(DMS_RAIL_OUT)-ref
dms-rail-update: dms-rail-ref
	bash scripts/dms_rail.sh update $(DMS_RAIL_OUT)-ref
dms-rail-prepare: bb
	bash scripts/dms_rail.sh prepare $(BB)

# Local only (needs DMS_REPO and the LOCAL-ONLY vendor-derived TPS22917): the plan's PWL replay of
# the run's pin_edge records into the suite's own led_rail_sequence deck with the same bench
# supply; VLED within 5 mV of the co-simulation with the vendor-derived view (R2), both benches.
dms-crosscheck: dms-local-packages
	DMS_REPO=$(DMS_REPO) bash scripts/dms_crosscheck.sh $(BB)

# ADR-008 D5: rebuild the committed E1 and E2 images in a FRESH west workspace (pinned manifest
# examples/dial-me-spinner/firmware/west.yml, Zephyr SDK 1.0.1) and require the image hashes of
# firmware/prebuilt/SHA256SUMS (CI job fw-rebuild, where a workspace can be created; downloads
# about 400 MB the first time). dms-firmware rebuilds the prebuilt images (review the diff).
fw-rebuild: bb
	bash scripts/dms_zephyr.sh check
dms-firmware: bb
	bash scripts/dms_zephyr.sh ws
	bash scripts/dms_zephyr.sh update

# --- Phase 1 M8 (second half): E2, battery reading and LED shed on dial-me-spinner ---------
# examples/dial-me-spinner with firmware/prebuilt/vsense.elf (a reading every 1 s) and
# vsense_50ms.elf (every 50 ms) on U10's SAADC, the loading adc_in on VSYS_SENSE (ADR-008 D7-D8),
# bench/vsense.toml (the suite's sample-rate worst case), bench/sag.toml (the battery ramps
# down through the LED shed) and bench/sag-recover.toml (it dips through the shed and comes back
# above the restore threshold). Runs use copies of the project off /mnt/c (scripts/dms_vsense.sh;
# $(DMS_VSENSE_WORK)), one with load = "none" per loaded case and one with C5 fitted at 10 nF.
.PHONY: pooled-dms-vsense pooled-dms-vsense-test dms-vsense-ref dms-vsense-update dms-vsense-prepare \
        cost-table-dms-vsense
DMS_VSENSE_OUT  ?= out/dms-vsense
DMS_VSENSE_WORK ?= $(HOME)/.cache/bb-dms-vsense
DMS_VSENSE_GOLD := tests/golden/dms-vsense
export DMS_VSENSE_WORK
DMS_VSENSE_ARGS  = --project $(DMS_VSENSE_WORK)/proj-50ms --bench $(DMS_VSENSE_WORK)/proj-50ms/bench/vsense.toml \
                   --for 650ms --probe net:VSYS_SENSE --sample-rate 10kHz

# In `make test`: s50, s50-none, c5 and c5-none at bb's defaults, check_dms_vsense.py on them (the
# analog side against load = "none", the codes, the C5 / 10 fault), their committed one-cell
# hashes (non-grant records, nets.csv, nets.vcd) and run.json's record of each binding's load.
pooled-dms-vsense-test: bb
	bash scripts/dms_vsense.sh test $(BB) $(DMS_VSENSE_OUT)-test

# The plan's M8 gate (E2): every case (s1, s1-none, s50, s50-none, c5, c5-none, sag, rec) at bb's
# defaults and as its every-lattice-point reference (--lookahead off --adc-push every),
# check_dms_vsense.py on both, the references equal to tests/golden/dms-vsense, the defaults equal
# to them except grant records, analog-first byte-identical, s50 at two other grant partitions
# (--max-quantum 1ms; 3ms with --verify every), run.json's load records, the TACQ refusal, 10-run
# determinism. make pool-equiv has s50 (defaults and reference).
pooled-dms-vsense: bb
	bash scripts/dms_vsense.sh gate $(BB) $(DMS_VSENSE_OUT)

dms-vsense-ref: bb
	bash scripts/dms_vsense.sh ref $(BB) $(DMS_VSENSE_OUT)-ref
dms-vsense-update: dms-vsense-ref
	bash scripts/dms_vsense.sh update $(DMS_VSENSE_OUT)-ref
dms-vsense-prepare: bb
	bash scripts/dms_vsense.sh prepare $(BB)

# The Phase 1 M8 E2 row alone (s50: vsense_50ms.elf, 650 ms, the loading adc_in on the imported
# board): the every-lattice-point reference vs bb's defaults, fresh and on a 1-slot pool, with the
# protocol. Run it on an otherwise idle host.
cost-table-dms-vsense: firmware bb dms-vsense-prepare
	env -u BB_RENODE_POOL bash scripts/perf_ab.sh $(BB) $(COST_OUT)/fresh/dms-vsense $(PERF_N) \
	    "--lookahead off --adc-push every" "" - - - -- $(DMS_VSENSE_ARGS)
	$(BB) pool run --slots 1 -- bash scripts/perf_ab.sh $(BB) $(COST_OUT)/pool/dms-vsense $(PERF_N) \
	    "--lookahead off --adc-push every" "" - - - -- $(DMS_VSENSE_ARGS)

# After an intended change to a package, a platform's .repl or a Renode
# plugin source: rewrite the changed bb.lock entries (review, then commit).
locks: bb
	set -e; for b in $(PKG_BOARDS); do $(BB) open --update-lock $$b; done

# ADR-006 G3: bb's KiCad value parser against the suite's spicelib on the 29
# distinct passive values of the dial-me-spinner main board (with DMS_REPO
# present the fixture tests/fixtures/values/dms-main.json is re-derived from
# spicelib at 6854d25 and must be unchanged).
dms-values: bb
	$(PYTHON) scripts/dms_values.py --bb $(BB) $(if $(wildcard $(DMS_REPO)),--repo $(DMS_REPO))

# Phase 1 plan M3 / R7: `make setup SETUP=demo` timed in a clean
# ubuntu:24.04 container (podman or docker), from `git archive HEAD`
# (SETUP_REV=<commit>, or SETUP_WORKTREE=1 for the working tree).
SETUP_REV ?= HEAD
setup-container:
	bash scripts/setup_container_test.sh $(if $(SETUP_WORKTREE),--worktree,--rev $(SETUP_REV)) --out out/setup-container

# Phase 1 plan M7 (section 5, the exit). `make demo`: the README's reference demo in one command
# (E1 on dial-me-spinner rev A: bb open, bb flash, bb run at the cold bench with the correct and
# the wrong-order firmware, check_dms_rail.py), on a copy of the project in $(DEMO_WORK), every
# Renode fresh, plus the committed one-cell hashes and run.json's view list (scripts/demo.sh).
# Needs only `make setup SETUP=demo`. `make stranger-test-container` / `stranger-test-wsl`: the
# README's quickstart block run line by line, timed, in a clean ubuntu:24.04 container (rootless
# podman) or a fresh WSL distro imported for the run and removed afterwards, cloning from the local
# repository by default (approver decision B) or from STRANGER_URL, e.g. the published
# https://github.com/tyevco/breadboard.dev (scripts/stranger_test.sh; out/stranger/<mode>/timing.txt).
.PHONY: demo stranger-test-container stranger-test-wsl
DEMO_OUT  ?= out/demo
DEMO_WORK ?= $(HOME)/.cache/bb-demo
demo: bb
	DEMO_OUT=$(DEMO_OUT) DEMO_WORK=$(DEMO_WORK) bash scripts/demo.sh $(BB)
stranger-test-container:
	bash scripts/stranger_test.sh container $(if $(STRANGER_URL),--url $(STRANGER_URL)) $(STRANGER_ARGS)
stranger-test-wsl:
	bash scripts/stranger_test.sh wsl $(if $(STRANGER_URL),--url $(STRANGER_URL)) $(STRANGER_ARGS)

# The docs site's waveform plots (site/public/plots, committed: the Pages build has no Renode).
# `make docs-plots` runs make demo's E1 runs, the E2 cases sag, rec, s50 and s50-none of make
# dms-vsense and the spike, every Renode fresh, then draws <name>.{light,dark}.svg from their
# outputs with plain Python (scripts/docs_plots.sh, scripts/docs_plots.py; about 2.5 min).
# `make docs-plots-check` does the same into $(DOCS_PLOTS_OUT)/svg and requires the committed
# files byte for byte (CI job dms-rail).
.PHONY: docs-plots docs-plots-check
DOCS_PLOTS_OUT ?= out/docs-plots
DOCS_PLOTS_DIR := site/public/plots
docs-plots: firmware bb
	SPIKE_ARGS="$(SPIKE_ARGS)" bash scripts/docs_plots.sh $(BB) $(DOCS_PLOTS_OUT) $(DOCS_PLOTS_DIR)
docs-plots-check: firmware bb
	SPIKE_ARGS="$(SPIKE_ARGS)" bash scripts/docs_plots.sh $(BB) $(DOCS_PLOTS_OUT) $(DOCS_PLOTS_OUT)/svg
	@if diff -r $(DOCS_PLOTS_DIR) $(DOCS_PLOTS_OUT)/svg > $(DOCS_PLOTS_OUT)/svg.diff; then \
	    echo "docs-plots-check: PASS ($(DOCS_PLOTS_DIR) identical to the regenerated figures)"; \
	else head -40 $(DOCS_PLOTS_OUT)/svg.diff; \
	    echo "docs-plots-check: FAIL: the committed plots differ from the runs (make docs-plots, review, commit)"; exit 1; fi

# Phase 1 M9 (ADR-008 D10, Amendment 7): `bb run --gdb` driven by a real GDB in batch mode
# (scripts/gdb_gate.py), on the spike and on E1 (led_rail.elf, cold bench; a copy of the project in
# $(GDB_WORK) made by `dms_rail.sh prepare`). `make gdb-test` (in `make test`, about 1 min): attach
# and detach without a stop give the committed hashes of the run without GDB, on both boards; a
# breakpoint hit and continued marks the run (one warn record; run.json debug: true); monitor is
# refused; a run without --gdb has no debug key; a client of Renode's internal stub port gets no reply
# and changes nothing (Amendment 8). `make gdb` (CI job dms-rail, about 5 min) adds
# attach mid-run, the scripted breakpoint session 20 times (5 on E1), steps and reads at the
# breakpoint, a 5 s hold past a 2 s External Control timeout, Ctrl-C then detach or continue,
# 1 500 steps across grants and WFI, a second debugger, GDB or bb killed during a halt, the
# loopback listener, the pool and the internal-port race 5 times. GDB: gdb-multiarch (make setup), arm-none-eabi-gdb or the
# Zephyr SDK's arm-zephyr-eabi-gdb.
.PHONY: gdb gdb-test
GDB      ?= $(shell command -v gdb-multiarch 2>/dev/null || command -v arm-none-eabi-gdb 2>/dev/null || \
              ls $(HOME)/.local/opt/zephyr-sdk-1.0.1/gnu/arm-zephyr-eabi/bin/arm-zephyr-eabi-gdb 2>/dev/null)
GDB_OUT  ?= out/gdb
GDB_WORK ?= $(HOME)/.cache/bb-gdb-gate
gdb gdb-test: firmware bb
	@test -n "$(GDB)" || { echo "$@: no GDB found (make setup installs gdb-multiarch; or pass GDB=<path>)"; exit 1; }
	DMS_RAIL_WORK=$(GDB_WORK) bash scripts/dms_rail.sh prepare $(BB)
	$(PYTHON) scripts/gdb_gate.py $(if $(filter gdb,$@),gate,test) --bb $(BB) --gdb $(GDB) \
	    --out $(GDB_OUT)/$(if $(filter gdb,$@),gate,test) --work $(GDB_WORK)

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all -- --check

clippy:
	$(CARGO) clippy --workspace --all-targets -- -D warnings

# Everything CI runs, in CI order.
check: fmt-check clippy test
