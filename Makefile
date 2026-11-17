# Breadboard — top-level build entry points. Run inside WSL / Linux.
SHELL := /bin/bash

# Phase 2 H0: a track worktree's own target and scratch directories (`make worktree TRACK=<name>`
# writes its git-ignored local.mk; scripts/worktree.sh). The main checkout has none.
-include local.mk
export CARGO_TARGET_DIR ?= $(HOME)/.cache/bb-target
# The parent of the gates' work directories (project copies a run rewrites: bb-dms-rail, ...).
export BB_SCRATCH ?= $(HOME)/.cache
export PATH := $(HOME)/.cargo/bin:$(HOME)/.local/bin:$(PATH)

CARGO ?= cargo

.PHONY: setup doctor build test cargo-test fmt fmt-check clippy check firmware \
        bb spike spike-verify-every lattice-check partition perf determinism golden \
        golden-update feedback-run feedback feedback-order feedback-determinism \
        feedback-open feedback-open-ref lookahead-inert lookahead-startup \
        feedback-ref lookahead-equiv lookahead-perf adc-run adc adc-ref adc-open-ref \
        adc-equiv adc-model adc-order adc-determinism pooled-adc-determinism \
        cost-table cost-table-runs startup-perf ec-reconnect ec-long ec-exposure \
        pool-equiv pool-perf dms-source dms-suite dms-suite-check dms-suite-update \
        packages locks plugin-scope lock-scope dms-values setup-container \
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

# --- ADR-004 Part B: make test and the gates on a warm Renode pool ---------
# Each target below runs its recipe (pooled-<target>) under `bb pool run`
# (scripts/with_pool.sh), so its bb runs and the Renode engine tests lease
# pre-warmed Renode processes; a nested make joins the pool already in
# BB_RENODE_POOL. A run that finds no idle process starts Renode fresh, and
# the outputs are the same either way (make pool-equiv). POOL=0 runs
# everything fresh (the nightly job `fresh`, make nightly-fresh, since Phase 2 H6); POOL_SLOTS
# sets the pool size.
POOL       ?= 1
POOL_SLOTS ?= 5
ifeq ($(POOL),0)
WITH_POOL = env -u BB_RENODE_POOL
else
WITH_POOL = bash scripts/with_pool.sh $(BB) $(POOL_SLOTS) --
endif
POOLED_TARGETS := test partition lattice-check feedback-open lookahead-inert lookahead-equiv adc-equiv spike-verify-every determinism feedback-determinism adc-determinism \
                  nrf-gates nrf-feedback dms-rail dms-rail-test dms-vsense dms-vsense-test dms-home dms-home-test \
                  bench-test bench-gate dms-home-project assert-test assert-gate bb-test-gate \
                  dms-e1-system dms-e1-system-test cargo-test
$(POOLED_TARGETS): firmware bb
	$(WITH_POOL) $(MAKE) --no-print-directory pooled-$@

# Phase 2 H0: every step of `make test` appends its wall time to $(TEST_TIMING) (target, seconds,
# exit status; scripts/step_time.sh), the per-gate breakdown of the runtime baseline.
TEST_TIMING ?= out/test-timing.tsv
STEP = bash scripts/step_time.sh $(TEST_TIMING) $(MAKE)

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
# Phase 2 H6: on a warm pool (a POOLED_TARGETS member, so `make cargo-test` alone builds the
# firmware and bb and leases like make test; ADR-004), and its own CI job (ci.yml `cargo-test`,
# beside `test`, which runs make test with TEST_SKIP=cargo-test; ADR-015 Amendment 5).
pooled-cargo-test:
	BB_REQUIRE_RENODE=1 $(CARGO) test --workspace

# The exit gate: unit/engine tests, then the end-to-end spike, 10-run
# determinism and the golden-waveform diff (golden reuses the spike output).
# Phase 2 H6: TEST_SKIP names steps to leave out (CI's `test` job: TEST_SKIP=cargo-test, which
# its own job runs); make test and make check locally run every step. A step that fails ends the
# run, as before.
TEST_STEPS := measure-lock-test cargo-test spike determinism golden feedback feedback-order \
              nrf-feedback adc adc-order adc-model nrf-saadc-model nrf-pwm-model device-smoke \
              multi-producer adc-kicad dms-rail-test dms-vsense-test dms-home-test bench-test \
              assert-test ec-reconnect packages pi-static-test pi-glue-test pi-idd-test gdb-test \
              vscode-templates dms-system dms-e1-system-test registry-test
TEST_SKIP ?=
pooled-test:
	@mkdir -p $(dir $(TEST_TIMING)) && : > $(TEST_TIMING)
	$(if $(filter-out $(TEST_STEPS),$(TEST_SKIP)),$(error TEST_SKIP: not a make test step: $(filter-out $(TEST_STEPS),$(TEST_SKIP))))
	+@set -e; for s in $(filter-out $(TEST_SKIP),$(TEST_STEPS)); do \
	    echo "$(STEP) $$s"; $(STEP) $$s; \
	done
	$(if $(TEST_SKIP),@echo "test: skipped (TEST_SKIP): $(TEST_SKIP)")

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
measured-perf: firmware bb
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
               adc-kicad-ref nrf-feedback-ref
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
measured-lookahead-perf: firmware bb
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
measured-cost-table: firmware bb
	env -u BB_RENODE_POOL $(MAKE) --no-print-directory cost-table-runs COST_RUN_OUT=$(COST_OUT)/fresh
	$(BB) pool run --slots 1 -- $(MAKE) --no-print-directory cost-table-runs COST_RUN_OUT=$(COST_OUT)/pool
cost-table-runs:
	bash scripts/perf_ab.sh $(BB) $(COST_RUN_OUT)/spike $(PERF_N) "--max-quantum 10us" "" - - - -- $(SPIKE_ARGS)
	bash scripts/perf_ab.sh $(BB) $(COST_RUN_OUT)/feedback $(PERF_N) "--lookahead off" "" - - - -- $(FEEDBACK_ARGS)
	bash scripts/perf_ab.sh $(BB) $(COST_RUN_OUT)/feedback-open $(PERF_N) "--max-quantum 10us" "" - - - \
	    -- $(FEEDBACK_OPEN_ARGS)
	bash scripts/perf_ab.sh $(BB) $(COST_RUN_OUT)/adc $(PERF_N) "--lookahead off" "" - - - -- $(ADC_ARGS)
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
	bash scripts/perf_ab.sh $(BB) $(COST_RUN_OUT)/dms-home $(PERF_N) "--lookahead off" "" - - - \
	    -- $(DMS_HOME_TYP_ARGS)
	bash scripts/perf_ab.sh $(BB) $(COST_RUN_OUT)/dms-home-project $(PERF_N) "--lookahead off" "" - - - \
	    -- $(DMS_HOME_PSTEP_ARGS)
	$(MAKE) --no-print-directory dms-e1-system-prepare
	bash scripts/perf_ab.sh $(BB) $(COST_RUN_OUT)/dms-e1-system $(PERF_N) "--max-quantum 10us" "" - - - \
	    -- $(DMS_E1_SYSTEM_ARGS)

# The Phase 1 M8 row alone (examples/nrf-saadc, the loading adc_in): lookahead off (one cell) vs
# bb's defaults, fresh and on a 1-slot pool, with the protocol. Run it on an otherwise idle host.
measured-cost-table-nrf-saadc: firmware bb
	env -u BB_RENODE_POOL bash scripts/perf_ab.sh $(BB) $(COST_OUT)/fresh/nrf-saadc $(PERF_N) "--lookahead off" "" \
	    - - - -- $(NRF_SAADC_ARGS)
	$(BB) pool run --slots 1 -- bash scripts/perf_ab.sh $(BB) $(COST_OUT)/pool/nrf-saadc $(PERF_N) \
	    "--lookahead off" "" - - - -- $(NRF_SAADC_ARGS)

# The Phase 1 M6 row alone (E1, examples/dial-me-spinner, ok-cold): one-cell vs bb's defaults, fresh
# and on a 1-slot pool, with the protocol. Run it on an otherwise idle host.
measured-cost-table-dms-rail: firmware bb dms-rail-prepare
	env -u BB_RENODE_POOL bash scripts/perf_ab.sh $(BB) $(COST_OUT)/fresh/dms-rail $(PERF_N) "--max-quantum 10us" "" \
	    - - - -- $(DMS_RAIL_ARGS)
	$(BB) pool run --slots 1 -- bash scripts/perf_ab.sh $(BB) $(COST_OUT)/pool/dms-rail $(PERF_N) \
	    "--max-quantum 10us" "" - - - -- $(DMS_RAIL_ARGS)

# The Phase 1 M4 rows alone: examples/adc on board.cir and as the KiCad
# project, each with lookahead off vs on (the same protocol). Run it on an
# otherwise idle host.
measured-cost-table-adc-kicad: firmware bb adc-kicad-prepare
	env -u BB_RENODE_POOL bash scripts/perf_ab.sh $(BB) $(COST_OUT)/fresh/adc $(PERF_N) "--lookahead off" "" - - - -- $(ADC_ARGS)
	env -u BB_RENODE_POOL bash scripts/perf_ab.sh $(BB) $(COST_OUT)/fresh/adc-kicad $(PERF_N) "--lookahead off" "" - - - -- $(ADC_KICAD_ARGS)

# ADR-005's speed gate (make adc-speed) and startup gate (make adc-startup) compared Breadboard's
# ADC model with Renode's stock one (--adc-model stock). Both were retired with the stock path in
# Phase 2 H6 (ADR-015 D9, Amendment 5); their last results are in the README and in ADR-005
# Amendment 4 (4.75x less simulation, 1 209 grants; startup +0.08 s warm, +1.17 s cold).

# ADR-003 step 5 startup-cost gate: with the cached plugin, Renode engine
# startup is within 0.3 s of a start without it (interleaved, N =
# BB_LA_STARTUP_N runs each, default 7, medians). Wall-clock: run it on an
# otherwise idle host.
measured-lookahead-startup: firmware
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
measured-startup-perf: firmware bb
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
measured-jit-profile-stress: firmware bb
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
pool-equiv: firmware bb adc-kicad-prepare dms-rail-prepare dms-vsense-prepare bench-prepare \
            dms-e1-system-prepare pi-idd-prepare
	$(BB) pool run --slots 1 -- bash scripts/pool_equiv.sh $(BB) $(POOL_EQUIV_OUT) $(POOL_EQUIV_ROUNDS) -- \
	    "bench-stimulus;$(BENCH_GOLD)/stimulus.sha256;nongrant;$(BENCH_STIM_ARGS)" \
	    "bench-trigger;$(BENCH_GOLD)/trigger.sha256;nongrant;$(BENCH_TRIG_ARGS)" \
	    "bench-set;$(BENCH_GOLD)/set.sha256;nongrant;$(BENCH_SET_ARGS)" \
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
	    "adc-kicad;$(ADC_KICAD_HASHES);nongrant;$(ADC_KICAD_ARGS)" \
	    "nrf-feedback;$(NRF_FEEDBACK_HASHES);nongrant;$(NRF_FEEDBACK_ARGS)" \
	    "nrf-feedback-off;$(NRF_FEEDBACK_HASHES);all;$(NRF_FEEDBACK_ARGS) --lookahead off" \
	    "nrf-feedback-noplugin;$(NRF_FEEDBACK_HASHES);all;$(NRF_FEEDBACK_ARGS) --lookahead-plugin never" \
	    "nrf-saadc;$(NRF_SAADC_HASHES);nongrant;$(NRF_SAADC_ARGS)" \
	    "nrf-saadc-ref;$(NRF_SAADC_HASHES);all;$(NRF_SAADC_ARGS) --lookahead off --adc-push every" \
	    "dms-vsense;$(DMS_VSENSE_GOLD)/s50/outputs.sha256;nongrant;$(DMS_VSENSE_ARGS)" \
	    "dms-vsense-ref;$(DMS_VSENSE_GOLD)/s50/outputs.sha256;all;$(DMS_VSENSE_ARGS) --lookahead off --adc-push every" \
	    "dms-home-step;$(DMS_HOME_GOLD)/step/outputs.sha256;nongrant;$(DMS_HOME_STEP_ARGS)" \
	    "dms-home;$(DMS_HOME_GOLD)/typ/outputs.sha256;nongrant;$(DMS_HOME_TYP_ARGS)" \
	    "dms-home-project-step;$(DMS_HOME_GOLD)/project-step/outputs.sha256;nongrant;$(DMS_HOME_PSTEP_ARGS)" \
	    "dms-home-project;$(DMS_HOME_GOLD)/project-typ/outputs.sha256;nongrant;$(DMS_HOME_PTYP_ARGS)" \
	    "dms-e1-system;$(DMS_E1_SYSTEM_GOLD)/ok-cold/outputs.sha256;nongrant;$(DMS_E1_SYSTEM_ARGS)" \
	    "dms-e1-system-ref;$(DMS_E1_SYSTEM_GOLD)/ok-cold/outputs.sha256;all;$(DMS_E1_SYSTEM_ARGS) --max-quantum 10us" \
	    "pi-idd-duty;$(PI_IDD_GOLD)/duty/outputs.sha256;nongrant;$(PI_IDD_DUTY_ARGS)" \
	    "pi-idd-duty-ref;$(PI_IDD_GOLD)/duty/outputs.sha256;all;$(PI_IDD_DUTY_ARGS) --max-quantum 10us --lookahead off" \
	    "pi-idd-radio;$(PI_IDD_GOLD)/radio/outputs.sha256;nongrant;$(PI_IDD_RADIO_ARGS)" \
	    "pi-idd-feedback;$(PI_IDD_GOLD)/feedback/outputs.sha256;nongrant;$(PI_IDD_FEEDBACK_ARGS)"

# G2 pool startup (scripts/pool_perf.sh): on a 1-slot pool after its
# pre-warm, the first lease starts in <= 0.5 s, then POOL_PERF_N interleaved
# rounds of the spike, the feedback board and the ADC board: median engine
# startup <= 0.30 s each, spike median total wall <= 1.2 s, every run
# leased. Wall-clock: run it on an otherwise idle host.
POOL_PERF_OUT ?= out/pool-perf
POOL_PERF_N   ?= 7
measured-pool-perf: firmware bb
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
measured-nrf-costs: firmware
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
# ADR-015 D5/G4 (Phase 2 H3): the External Control exposure gate. Renode
# connects out to bb's listener (CreateExternalControlClient); 20 engine
# starts (4 fresh, 16 leased) each with a foreign client (a separate bash)
# connected to bb's listener first, again before every reconnect: every one
# refused without a byte, the run's own Renode admitted, Renode listening
# on no port at any check, PA5 edges as the reference
# (engines/cpu-renode/tests/ec_exposure.rs; it is also in make test through
# cargo-test, about 8 s).
ec-exposure: firmware
	@mkdir -p out
	BB_REQUIRE_RENODE=1 $(CARGO) test -p bb-cpu-renode --test ec_exposure -- --nocapture 2>&1 \
	    | tee out/ec-exposure.log | grep -E '^ec-exposure|^test result'; \
	    test $${PIPESTATUS[0]} -eq 0

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

# --- Phase 1 M10 (first step): PWM, TEMP and NVMC (ADR-008 D9, Amendment 9) --
# nrf52840-bb@3: BbNrf52Pwm (PWM0-3), the BbNrf52Temp and BbNrf52Nvmc
# stand-ins and their host port BbNrf52HostPort (the PWM pin ring). make
# nrf-pwm-model (in make test, about 25 s): the Renode-only register, timing
# and sequencer tests and the firmware group (nrf_pwm_step.elf: TEMP, an
# NVMC erase, 4 full steps at 446 PPS; no lookahead promise past a PWM
# edge), examples/nrf-pwm/bb_pwm_test.py, each group twice, byte-identical.
# Then the engine path (M10, ADR-008 Amendment 11): examples/nrf-pwm/board.cir
# runs the same firmware through bb run; the Renode engine reports every PWM
# pin change from the host port's ring at its exact instant, arbitrated with
# the GPIO (scripts/check_nrf_pwm.py: the duty table to the femtosecond). The
# board is cyclic (a digital_in on a constant net), so lookahead is on by
# default: the one-cell reference (--lookahead off), the default, --max-quantum
# 1ms and analog-first give identical nets.csv, nets.vcd and non-grant records,
# with no broken promise (no promise passes a PWM edge). Fresh Renodes: the
# board is not in pool-equiv.
NRF_PWM_BOARD_OUT ?= out/nrf-pwm-board
NRF_PWM_BOARD_ARGS = --netlist examples/nrf-pwm/board.cir \
                     --firmware $(NRF_FW)/build/nrf_pwm_step.elf --target U1 --for 120ms \
                     --renode-pool off --probe net:A1 --probe net:A2 --probe net:U1.P0.26 \
                     --probe net:U1.P0.05
.PHONY: nrf-pwm-model
nrf-pwm-model: firmware bb
	$(PYTHON) examples/nrf-pwm/bb_pwm_test.py --out out/nrf-pwm-model
	rm -rf $(NRF_PWM_BOARD_OUT)
	$(BB) run $(NRF_PWM_BOARD_ARGS) --lookahead off --out $(NRF_PWM_BOARD_OUT)/ref
	$(BB) run $(NRF_PWM_BOARD_ARGS) --out $(NRF_PWM_BOARD_OUT)/default
	$(BB) run $(NRF_PWM_BOARD_ARGS) --max-quantum 1ms --out $(NRF_PWM_BOARD_OUT)/q1ms
	$(BB) run $(NRF_PWM_BOARD_ARGS) --engine-order analog-first --out $(NRF_PWM_BOARD_OUT)/order
	set -e; for r in ref default q1ms order; do \
	    $(PYTHON) scripts/check_nrf_pwm.py $(NRF_PWM_BOARD_OUT)/$$r; \
	    grep -v '"type":"grant"' $(NRF_PWM_BOARD_OUT)/$$r/events.jsonl > $(NRF_PWM_BOARD_OUT)/$$r/non-grant-records; \
	done
	set -e; for r in default q1ms order; do for f in nets.csv nets.vcd non-grant-records; do \
	    cmp $(NRF_PWM_BOARD_OUT)/ref/$$f $(NRF_PWM_BOARD_OUT)/$$r/$$f; done; done
	@echo "nrf-pwm-model: board: grants ref $$(grep -c '"type":"grant"' $(NRF_PWM_BOARD_OUT)/ref/events.jsonl), default $$(grep -c '"type":"grant"' $(NRF_PWM_BOARD_OUT)/default/events.jsonl), q1ms $$(grep -c '"type":"grant"' $(NRF_PWM_BOARD_OUT)/q1ms/events.jsonl)"
	@echo "nrf-pwm-model: PASS"

# --- Phase 1 M10 (second step): device parts (ADR-006 D7, Amendment 6) ------
# tests/device-smoke/board.cir: examples/feedback's MCU and firmware with the
# dms/mechanism@2 device in a loop with it (U1.PA5 -> M1.NSLEEP, M1.IN_A1;
# M1.HOME_N -> U1.PB0, both direct digital routes). make device-smoke (in
# make test, about 20 s): the one-cell reference (--lookahead off), the
# default (lookahead on), analog-first and a second default run, each with a
# fresh Renode (--renode-pool off: the board is not in pool-equiv); every run
# passes scripts/check_device_smoke.py (HOME_N rises at exactly 57.04 ms,
# PA6 follows within one cell), and nets.csv, nets.vcd and the non-grant
# records are identical across all four.
DEVICE_SMOKE_OUT ?= out/device-smoke
DEVICE_SMOKE_ARGS = --netlist tests/device-smoke/board.cir --firmware $(FEEDBACK_ELF) \
                    --target U1 --for 200ms --renode-pool off \
                    --probe net:HOME_N --probe net:LED_B --probe net:SENSE
.PHONY: device-smoke
device-smoke: firmware bb
	rm -rf $(DEVICE_SMOKE_OUT)
	$(BB) run $(DEVICE_SMOKE_ARGS) --lookahead off --out $(DEVICE_SMOKE_OUT)/ref
	$(BB) run $(DEVICE_SMOKE_ARGS) --out $(DEVICE_SMOKE_OUT)/on
	$(BB) run $(DEVICE_SMOKE_ARGS) --engine-order analog-first --out $(DEVICE_SMOKE_OUT)/order
	$(BB) run $(DEVICE_SMOKE_ARGS) --out $(DEVICE_SMOKE_OUT)/on2
	set -e; for r in ref on order on2; do \
	    $(PYTHON) scripts/check_device_smoke.py $(DEVICE_SMOKE_OUT)/$$r; \
	    grep -v '"type":"grant"' $(DEVICE_SMOKE_OUT)/$$r/events.jsonl > $(DEVICE_SMOKE_OUT)/$$r/non-grant-records; \
	done
	set -e; for r in on order on2; do for f in nets.csv nets.vcd non-grant-records; do \
	    cmp $(DEVICE_SMOKE_OUT)/ref/$$f $(DEVICE_SMOKE_OUT)/$$r/$$f; done; done
	@echo "device-smoke: grants ref $$(grep -c '"type":"grant"' $(DEVICE_SMOKE_OUT)/ref/events.jsonl), on $$(grep -c '"type":"grant"' $(DEVICE_SMOKE_OUT)/on/events.jsonl)"
	@echo "device-smoke: PASS"

# --- Phase 2 H5: quiet grants with several producers (ADR-014) ------------
# make multi-producer (ADR-014 G1; in make test, about 15 s): the synthetic board
# tests/multi-producer/board.cir (an nRF52840 running homing.elf, dms/mechanism@2 at tS 5 ms on
# digital-only nets, ngspice behind sampled adc_in inputs), 100 ms, fresh Renode (--renode-pool off:
# not in pool-equiv): bb's defaults equal the one-cell reference except grant records with quiet
# grants Q = {devices} and no early stop, at most MP_MAX_GRANTS grants against 10 000;
# analog-first byte-identical; the scope control (MOT_NSLEEP glued into ngspice) never takes
# Q = {devices}; and bbdev/tests/multi_producer.rs (the kernel part, and the PromiseBroken control).
# scripts/multi_producer.sh.
MULTI_PRODUCER_OUT ?= out/multi-producer
.PHONY: multi-producer
multi-producer: firmware bb
	bash scripts/multi_producer.sh $(BB) $(MULTI_PRODUCER_OUT)

# ADR-005 test plan 1-3: Renode-only tests of Breadboard's STM32F072 ADC
# model and BbStm32Timer TRGO (examples/adc/bb_adc_test.py: pwm, trgo,
# registers, firmware; about 80 s). In `make test`.
adc-model: firmware
	$(MAKE) -C examples/adc model-test

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
measured-dms-s1-cost:
	DMS_REPO=$(DMS_REPO) bash scripts/dms_s1.sh cost $(DMS_S1_N)

# Phase 2 plan P0 (ADR-009 D2, gate G3; ADR-009 Amendment 1 is the report, Amendment 2 the approver's
# decision D-W1): the W1 and W2 suite replicas and the first RT9080, BQ25180 and TPS61040 `pi` views
# (examples/dial-me-spinner/pi-spike/views) in static-load mode, through NgspiceEngine (deck_run), at
# phase0 and at the suite's option sets: the option set, the model difference, the tolerances
# (pi-spike/tolerances.toml), W1's feasibility row at its labelled stress corner (55 mA) and the
# 50 mA margin check. Every
# deck is local-only (the TI-derived TPS22917, the suite's netlist extraction) and stays in
# $(BB_SCRATCH)/bb-pi-spike. A spike: not in make test (about 1 min on 8 jobs, PI_SPIKE_JOBS).
.PHONY: pi-spike
pi-spike:
	DMS_REPO=$(DMS_REPO) bash scripts/pi_spike.sh gate

# Phase 2 plan P1 (ADR-009 D1-D3, Amendment 4; decision D-W1, Amendment 2): the W1 and W2 static-load
# decks assembled from the `pi` package views and examples/dial-me-spinner/system/board.bbm
# (scripts/pi_static.py; examples/dial-me-spinner/pi/static_load.toml), run through NgspiceEngine with
# deck_run: W1 at its labelled stress corner (55 mA, RT9080 worst_dropout) against the suite replica
# with the stress corner's tolerances, its feasibility, the 50 mA margin check, the load sweep (30, 50,
# 55, 60 mA), W2 (rule and old rule) with P0's tolerances, and every W1 check's rung (pi). The replica's
# values are P0's recorded measurements, so nothing reads the dial-me-spinner repository.
# pi-static-test (in make test) runs the seven cases on PI_STATIC_JOBS jobs; pi-static adds the stress
# corner at 10 us and 10 ms grants and a second 1 ms run (consistency checks of deck_run's partitions).
.PHONY: pi-static pi-static-test
pi-static:
	$(CARGO) build -q -p bb-analog-ngspice --example deck_run
	DECK_RUN=$(CARGO_TARGET_DIR)/debug/examples/deck_run bash scripts/pi_static.sh gate
pi-static-test:
	$(CARGO) build -q -p bb-analog-ngspice --example deck_run
	DECK_RUN=$(CARGO_TARGET_DIR)/debug/examples/deck_run bash scripts/pi_static.sh test

# Phase 2 plan P2 (ADR-009 D5, gates G8 and G9, Amendment 5): rail-tracking, tristate digital_out
# at pi, with the high-Z leakage rhiz and oe from PIN_CNF.DIR (scripts/pi_glue.sh, pi_glue.py; project
# copies in $(PI_GLUE_WORK)). pi-glue-test (in make test, about 12 s): G9, every Phase 1 deck's DC
# operating point at pi with every MCU digital_out high-Z (`bb run --operating-point`); E1 cold at pi
# (bench/cold-pi.toml) with both firmwares: check_dms_rail.py, LSW-3 re-reported without the voh =
# 3.3 V pessimism, the oe edges; G8, LED_PWR_EN's VOH at the PS's 0.5 mA (bench/cold-pi-ioh.toml); the
# default runs against tests/golden/pi-glue. pi-glue adds the oe prototype's Renode tests (exactness:
# tests/nrf52840.rs nrf_oe_*), partition independence (one-cell, 1 ms, default), analog-first, a
# second run, the fast side-by-side and --pi-oe fixed's report. pi-oe-cost (a measurement, under the
# lock): the PIN_CNF.DIR oe against the fallback oe = 1 on E1 cold pi, N = PI_OE_COST_N (9) interleaved,
# medians; B / A at most 1.05 (ADR-009 D5 5 %).
PI_GLUE_OUT  ?= out/pi-glue
PI_GLUE_WORK ?= $(BB_SCRATCH)/bb-pi-glue
export PI_GLUE_WORK
.PHONY: pi-glue pi-glue-test pi-glue-update
pi-glue: firmware bb
	BB_REQUIRE_RENODE=1 $(CARGO) test --release -p bb-cpu-renode --test nrf52840 -- --nocapture nrf_oe_
	bash scripts/pi_glue.sh gate $(BB) $(PI_GLUE_OUT)
pi-glue-test: firmware bb
	bash scripts/pi_glue.sh test $(BB) $(PI_GLUE_OUT)-test
pi-glue-update: firmware bb
	bash scripts/pi_glue.sh update $(BB) $(PI_GLUE_OUT)-update

# Phase 2 plan P3 (ADR-009 D4, gate G6, Amendment 6): the nRF52840's supply_load view at pi, its CPU
# state outputs #run, #hfxo, #nvmc, #saadc (bb_nrf52_state.cs) and a bench [[activity]], on
# examples/nrf-idd (scripts/pi_idd.sh, pi_idd.py; project copies in $(PI_IDD_WORK)). pi-idd-test (in
# make test): the gate firmware nrf_duty.elf's total VDD current against the hand calculation of the
# duty-weighted average within 1 % (and with a bench-scheduled radio), its negative control at fast,
# the lookahead check (examples/nrf-feedback's firmware with the state outputs at most 2x the board's
# grants), the default runs against tests/golden/pi-idd. pi-idd adds the Renode-only test of the
# state outputs (tests/nrf52840.rs nrf_state_outputs_exact: #run at the WFI instants of nrf_state.elf,
# to the fs), partition independence (one-cell, 1 ms, default), analog-first and a second run.
PI_IDD_OUT  ?= out/pi-idd
PI_IDD_WORK ?= $(BB_SCRATCH)/bb-pi-idd
export PI_IDD_WORK
.PHONY: pi-idd pi-idd-test pi-idd-update pi-idd-prepare
pi-idd: firmware bb
	BB_REQUIRE_RENODE=1 $(CARGO) test --release -p bb-cpu-renode --test nrf52840 -- --nocapture nrf_state_
	bash scripts/pi_idd.sh gate $(BB) $(PI_IDD_OUT)
pi-idd-test: firmware bb
	bash scripts/pi_idd.sh test $(BB) $(PI_IDD_OUT)-test
pi-idd-update: firmware bb
	bash scripts/pi_idd.sh update $(BB) $(PI_IDD_OUT)-update
pi-idd-prepare: firmware bb
	bash scripts/pi_idd.sh prepare $(BB)
# The cost row of the state outputs (ADR-009 D4, risk R5; a measurement, under the lock): nrf_duty.elf
# at pi with the supply_load and its state outputs (B) against static-load mode (A, the same deck with
# the bench load in the view's place and no state output), N = PI_IDD_COST_N interleaved, medians.
PI_IDD_COST_N ?= 5
measured-pi-idd-cost: firmware bb pi-idd-prepare
	PERF_AB_EQUIV=0 bash scripts/perf_ab.sh $(BB) out/pi-idd-cost $(PI_IDD_COST_N) \
	    "--bench $(PI_IDD_WORK)/proj-duty/bench/pi-static.toml" "--bench $(PI_IDD_WORK)/proj-duty/bench/pi.toml" - - - -- \
	    --project $(PI_IDD_WORK)/proj-duty --for 50ms
PI_IDD_GOLD := tests/golden/pi-idd
PI_IDD_DUTY_ARGS = --project $(PI_IDD_WORK)/proj-duty --bench $(PI_IDD_WORK)/proj-duty/bench/pi.toml --for 50ms
PI_IDD_RADIO_ARGS = --project $(PI_IDD_WORK)/proj-duty --bench $(PI_IDD_WORK)/proj-duty/bench/pi-radio.toml --for 50ms
PI_IDD_FEEDBACK_ARGS = --project $(PI_IDD_WORK)/proj-feedback --bench $(PI_IDD_WORK)/proj-feedback/bench/pi.toml --for 500ms
PI_OE_COST_N ?= 9
measured-pi-oe-cost: firmware bb
	bash scripts/pi_glue.sh prepare $(BB)
	PERF_AB_EQUIV=0 bash scripts/perf_ab.sh $(BB) out/pi-oe-cost $(PI_OE_COST_N) "--pi-oe fixed" "" - - 1.05 -- \
	    --project $(PI_GLUE_WORK)/proj-ok --bench $(PI_GLUE_WORK)/proj-ok/bench/cold-pi.toml --for 120ms \
	    --renode-pool off

# --- Phase 1 M4: KiCad import, board.bbm, projects (ADR-007) ---------------
# examples/adc drawn in KiCad (examples/adc/kicad), as a bb.toml project. Its
# runs use a copy off /mnt/c with the firmware recorded by `bb flash`
# ($(ADC_KICAD_WORK)/net; `make adc-kicad-prepare` makes it).
ADC_KICAD_WORK ?= $(BB_SCRATCH)/bb-adc-kicad
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

# --- Phase 1 M11: systems (ADR-007 D5, D7, Amendment 5) --------------------
# examples/dial-me-spinner/system: the main and base boards at 6854d25 mated
# by the link harness ([[connection]] main:J1 - base:J3 through the harness
# view dial-me-spinner/link-harness:wires). scripts/dms_system.sh (about
# 30 s; in `make test`): the committed board.bbm from a fresh import, twice
# byte-identical; base 21 parts, 17 nets; the 15 shared references; the 12
# mated pins; the kicad = route on both boards (when kicad-cli is
# installed, else reported as skipped); dc96515's base netlist (reported);
# and E1's deck from the system equal to E1's own with qualified names. The
# name errors (a bare U1, U2, J1 or TP1) are cli/tests/system.rs.
.PHONY: dms-system
dms-system: bb
	bash scripts/dms_system.sh $(BB) out/dms-system

# --- Phase 2 R2: single-MCU system runs (ADR-013 D2, gate G2) --------------
# examples/dial-me-spinner/e1-system: E1 on the system (simulate = ["main", "base"]), the bench's
# power path replaced by the base board's views (a [[battery]] at J2, ti/bq25180:pp, the base
# board's capacitors, the link harness), at bench/cold.toml and bench/nominal.toml, each run on a
# copy in $(DMS_E1_SYSTEM_WORK) (scripts/dms_e1_system.sh). dms-e1-system-test (in make test):
# ok-cold, ok-nominal and ok-cold analog-first in parallel: check_system_run.py, E1's four checks,
# the committed one-cell hashes, analog-first byte-identical, 120 ms in under 60 s. dms-e1-system
# (local: the cross-check needs DMS_REPO and the suite venv): adds the one-cell references (the
# equivalence), analog-first on nominal, the wrong-order firmware (exactly LSW-3 fails), 10-run
# determinism, and VLED within 10 mV of the suite's led_rail_sequence battery-network deck at both
# corners, with its negative control (the cell's R0 doubled must fail it).
.PHONY: dms-e1-system dms-e1-system-test pooled-dms-e1-system pooled-dms-e1-system-test \
        dms-e1-system-ref dms-e1-system-update dms-e1-system-prepare cost-table-dms-e1-system
DMS_E1_SYSTEM_OUT  ?= out/dms-e1-system
DMS_E1_SYSTEM_WORK ?= $(BB_SCRATCH)/bb-dms-e1-system
export DMS_E1_SYSTEM_WORK
DMS_E1_SYSTEM_GOLD := tests/golden/dms-e1-system
DMS_E1_SYSTEM_ARGS  = --project $(DMS_E1_SYSTEM_WORK)/dms/e1-system-ok \
                      --bench $(DMS_E1_SYSTEM_WORK)/dms/e1-system-ok/bench/cold.toml --for 120ms
pooled-dms-e1-system-test: bb
	bash scripts/dms_e1_system.sh test $(BB) $(DMS_E1_SYSTEM_OUT)-test
pooled-dms-e1-system: bb
	DMS_REPO=$(DMS_REPO) bash scripts/dms_e1_system.sh gate $(BB) $(DMS_E1_SYSTEM_OUT)
dms-e1-system-ref: bb
	bash scripts/dms_e1_system.sh ref $(BB) $(DMS_E1_SYSTEM_OUT)-ref
dms-e1-system-update: dms-e1-system-ref
	bash scripts/dms_e1_system.sh update $(DMS_E1_SYSTEM_OUT)-ref
dms-e1-system-prepare: bb
	bash scripts/dms_e1_system.sh prepare $(BB)
# The R2 cost-table row alone (ok-cold: one-cell vs bb's defaults, fresh and on a 1-slot pool, with
# the protocol; also in make cost-table). Run it on an otherwise idle host.
measured-cost-table-dms-e1-system: firmware bb dms-e1-system-prepare
	env -u BB_RENODE_POOL bash scripts/perf_ab.sh $(BB) $(COST_OUT)/fresh/dms-e1-system $(PERF_N) \
	    "--max-quantum 10us" "" - - - -- $(DMS_E1_SYSTEM_ARGS)
	$(BB) pool run --slots 1 -- bash scripts/perf_ab.sh $(BB) $(COST_OUT)/pool/dms-e1-system $(PERF_N) \
	    "--max-quantum 10us" "" - - - -- $(DMS_E1_SYSTEM_ARGS)

# --- Phase 1 M3: component packages (ADR-006) ------------------------------
# The example boards whose parts come from packages; each has a bb.lock.
PKG_BOARDS := examples/spike/board.cir examples/feedback/board.cir \
              examples/feedback-open/board.cir examples/adc/board.cir examples/adc/board-open.cir \
              examples/adc/kicad examples/dial-me-spinner examples/nrf-saadc/board.cir \
              examples/dial-me-spinner/system tests/device-smoke/board.cir \
              examples/nrf-pwm/board.cir examples/dial-me-spinner/e3/board.cir examples/bench \
              examples/dial-me-spinner/e3/home examples/dial-me-spinner/e3/step \
              examples/dial-me-spinner/e2/none examples/dial-me-spinner/e2/c5 examples/dial-me-spinner/e2/c5-none \
              examples/dial-me-spinner/e1-system tests/multi-producer/board.cir \
              examples/nrf-idd
# Project-local packages (ADR-006 D1), linted and tested with the first-party ones.
PKG_LOCAL := examples/adc/kicad/packages examples/dial-me-spinner/system/packages \
             examples/bench/packages examples/dial-me-spinner/e3/packages

# --- Phase 2 B1: bb-bench/1 and the bench engine (ADR-010 D1-D4) -----------
# examples/bench, a synthetic STM32F103 board running examples/feedback's firmware, with every B1
# stimulus (scripts/bench_gate.sh describes the cases and checks). bench-test (in make test):
# check_bench.py on each case at bb's defaults (the switch view's analytic RC settling, G8; the
# digital playback's edges equal an equivalent [[pwl]]'s and are applied at ceil_L), the
# committed one-cell reference hashes, G2 (a playback against the same points as a [[pwl]]), the
# refusals, and G1 on the Phase 1 benches (bb-bench/0 and /1 build byte-identical decks).
# bench-gate: that, plus every case at --max-quantum 10us, 1ms and 10ms and analog-first, and
# determinism. bench-perf (idle host, measurement lock): a 1 s, 100 ksps playback adds <= 2 s.
# bench-update rewrites tests/golden/bench from fresh one-cell references (review the diff).
.PHONY: bench-test bench-gate bench-perf bench-update bench-prepare pooled-bench-test pooled-bench-gate
BENCH_OUT  ?= out/bench
BENCH_WORK ?= $(BB_SCRATCH)/bb-bench
export BENCH_WORK
pooled-bench-test: bb
	bash scripts/bench_gate.sh test $(BB) $(BENCH_OUT)-test
pooled-bench-gate: bb
	bash scripts/bench_gate.sh gate $(BB) $(BENCH_OUT)
bench-update: firmware bb
	bash scripts/bench_gate.sh update $(BB) $(BENCH_OUT)-ref
bench-prepare: firmware bb
	bash scripts/bench_gate.sh prepare $(BB)
measured-bench-perf: firmware bb
	env -u BB_RENODE_POOL bash scripts/bench_gate.sh perf $(BB) $(BENCH_OUT)-perf
BENCH_ARGS = --project $(BENCH_WORK)/p --sample-rate 10kHz
BENCH_STIM_ARGS = $(BENCH_ARGS) --bench $(BENCH_WORK)/p/bench/stimulus.toml --for 300ms
BENCH_TRIG_ARGS = $(BENCH_ARGS) --bench $(BENCH_WORK)/p/bench/trigger.toml --for 70ms
BENCH_SET_ARGS  = $(BENCH_ARGS) --bench $(BENCH_WORK)/p/bench/digital-set.toml --for 100ms
BENCH_GOLD := tests/golden/bench

# --- Phase 2 B2: assertions and assert v2 (ADR-010 D5-D7, Amendment 3) -----
# E1's four checks (examples/dial-me-spinner/asserts/e1.toml), E2's codes and shed timing
# (asserts/e2/, written by scripts/dms_asserts.py) and E3's HOME_N latency (e3/asserts/) as
# [[assert]] tables, against check_dms_rail.py, check_dms_vsense.py and check_dms_home.py on the same
# runs (scripts/check_asserts.py; scripts/assert_gate.sh describes every check). assert-test (in make
# test): E1's four cases with the assertions, the cross-check, passivity (the committed hashes), the
# replay by `bb assert`, the FW-6.4 negative controls, a bench's own [[assert]] against an --assert
# file, the refusals (G6). assert-gate (about 15 min fresh): that, E1's partitions and engine order
# and determinism, E2's eight cases and two references, E3's corners and dms-home's negative controls.
.PHONY: assert-test assert-gate pooled-assert-test pooled-assert-gate
ASSERT_OUT ?= out/assert
ASSERT_ENV  = DMS_RAIL_WORK=$(BB_SCRATCH)/bb-assert-rail DMS_VSENSE_WORK=$(BB_SCRATCH)/bb-assert-vsense \
              BENCH_WORK=$(BB_SCRATCH)/bb-assert-bench
pooled-assert-test: bb
	$(ASSERT_ENV) bash scripts/assert_gate.sh test $(BB) $(ASSERT_OUT)-test
pooled-assert-gate: bb
	$(ASSERT_ENV) bash scripts/assert_gate.sh gate $(BB) $(ASSERT_OUT)

# --- Phase 2 B3: bb test, bb-test/0, test.json and JUnit (ADR-010 D8-D9, Amendment 5) ----------
# make dms-rail-test, dms-vsense-test and dms-home-test (in make test) run examples/dial-me-spinner/
# tests/{e1,e2,e3}.toml through `bb test`. bb-test-gate (scripts/bb_test_gate.sh, about 15 min): bb
# test's semantics and negative controls (tests/bb-test), the G9 refusal, 10 runs of each dms-* test
# file with test.json byte-identical (the last with every Renode fresh), check_bb_test.py on every run,
# and the JUnit reporter CI pins reading every junit.xml (scripts/junit_reporter_check.sh: dorny/
# test-reporter's java-junit parser in a node container; the network once, for its clone, image and
# npm ci). bb-test-cost (measurement lock, idle host): the three targets as bb test files against the
# Phase 1 scripts' runs (scripts/<x>.sh test-scripted), interleaved, medians: B3's budget is +0 s.
.PHONY: bb-test-gate pooled-bb-test-gate measured-bb-test-cost
BB_TEST_GATE_OUT ?= out/bb-test-gate
pooled-bb-test-gate: bb
	DMS_RAIL_WORK=$(BB_SCRATCH)/bb-test-rail DMS_VSENSE_WORK=$(BB_SCRATCH)/bb-test-vsense \
	  bash scripts/bb_test_gate.sh $(BB) $(BB_TEST_GATE_OUT)
measured-bb-test-cost: firmware bb
	$(BB) pool run --slots $(POOL_SLOTS) -- bash scripts/bb_test_cost.sh $(BB) out/bb-test-cost

# ADR-006 G1 and the package gates of `make test`: every part.toml, pins.toml,
# test vector and bb.lock validates against spec/schema (python3-jsonschema);
# `bb pkg lint` is clean, for the first-party packages with --publish (the
# view-name rule of ADR-011 D11 an error: gate G10); every vector passes
# through NgspiceEngine at both grant partitions and at every recorded
# qualification (`bb pkg test`); and every example's bb.lock is current
# (`bb open --check`).
packages: bb
	$(PYTHON) scripts/validate_packages.py
	$(BB) pkg lint --publish packages
	$(BB) pkg lint $(PKG_LOCAL)
	$(BB) pkg test packages $(PKG_LOCAL)
	set -e; for b in $(PKG_BOARDS); do $(BB) open --check $$b; done
	bash scripts/check_local_only.sh
	bash scripts/check_local_only_test.sh
	@echo "packages: PASS"

# --- Phase 2 E1: the package registry (ADR-011; local fixtures only, nothing is published) -----
# registry-test (in make test, a few seconds): G1-G5, G9, G10, vendor files, removal, the source
# change and local-first rules against a file:// registry and a local HTTPS server, with
# throwaway keys (scripts/registry_test.sh). registry-gate: the same, then G7 on the local public
# registry repository with every first-party package (scripts/registry_g7.sh; needs the key in
# ~/.config/bb-registry). registry-repo: make or update that repository (~/src/bb-registry,
# BB_REGISTRY_REPO). test-offline: G8, make test and make packages in a network namespace with
# loopback only (scripts/offline.sh).
.PHONY: registry-test registry-gate registry-repo test-offline test-offline-inner
registry-test: firmware bb
	bash scripts/registry_test.sh $(BB) test
registry-gate: firmware bb
	bash scripts/registry_test.sh $(BB) gate
registry-repo: bb
	bash scripts/registry_repo.sh $(BB)
test-offline: firmware bb
	bash scripts/offline.sh $(MAKE) --no-print-directory test-offline-inner
test-offline-inner:
	@if curl -sS --max-time 5 -o /dev/null https://index.crates.io/config.json 2>/dev/null; then \
	  echo "test-offline: the network is reachable: refusing"; exit 1; fi
	@echo "test-offline: no route: $$(ip -o route show | wc -l) routes, interfaces: $$(ip -o -br link | awk '{print $$1}' | tr '\n' ' ')"
	CARGO_NET_OFFLINE=true $(MAKE) --no-print-directory test
	CARGO_NET_OFFLINE=true $(MAKE) --no-print-directory packages
	@echo "test-offline: PASS (make test and make packages without a network)"

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
DMS_RAIL_WORK  ?= $(BB_SCRATCH)/bb-dms-rail
export DMS_RAIL_WORK
DMS_RAIL_GOLD  := tests/golden/dms-rail
DMS_RAIL_ARGS   = --project $(DMS_RAIL_WORK)/proj-ok --bench $(DMS_RAIL_WORK)/proj-ok/bench/cold.toml --for 120ms
DMS_RAIL_WRONG_ARGS = --project $(DMS_RAIL_WORK)/proj-wrong --bench $(DMS_RAIL_WORK)/proj-wrong/bench/nominal.toml \
                 --for 120ms

# In `make test`: both firmwares at both benches at bb's defaults: check_dms_rail.py (the correct
# order passes LED-DATA-V4, LSW-3 and LSW-2; the wrong order fails exactly LSW-3), the committed
# one-cell hashes, the VLED snapshots, a clean PIN_CNF audit, and analog-first byte-identical. Since
# Phase 2 B3 a bb test file (examples/dial-me-spinner/tests/e1.toml: the runs, the assertions, the
# hashes, analog-first) and the checks that are not assertion kinds on its outputs; its test.json and
# junit.xml in out/dms-rail-test (scripts/dms_rail.sh test).
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

# ADR-008 D5: rebuild the committed E1-E3, W1 and W2 images in a FRESH west workspace (pinned
# manifest examples/dial-me-spinner/firmware/west.yml, Zephyr SDK 1.0.1) and require the image
# hashes of firmware/prebuilt/SHA256SUMS (CI job fw-rebuild, where a workspace can be created;
# downloads about 400 MB the first time). dms-firmware rebuilds the prebuilt images (review the diff).
fw-rebuild: bb
	bash scripts/dms_zephyr.sh check
dms-firmware: bb
	bash scripts/dms_zephyr.sh ws
	bash scripts/dms_zephyr.sh update

# Phase 2 W-fw (plan section 4; ADR-009 D6, Amendment 7): the Renode-only test of the W1 and W2
# images' POWER and GPIO configuration (examples/dial-me-spinner/firmware/bb_wfw_test.py, about
# 30 s on 4 jobs, WFW_JOBS): the PIN_CNF of the enables, POFCON at V28, the blanking's start and
# end against the LED_PWR_EN edge in each W1 build, the cue held or shed with the stress corner's
# 2.94 V window as the comparator input, and W2's RESETREAS read and its OLED rule; every case run
# twice, byte-identical. The POWER registers are power_double.py's until P4's BbNrf52Power
# replaces Renode's NRF_CLOCK. Needs only Renode and the prebuilt images (CI job fw-rebuild).
.PHONY: dms-wfw-test
dms-wfw-test:
	$(PYTHON) examples/dial-me-spinner/firmware/bb_wfw_test.py --out out/dms-wfw-test

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
DMS_VSENSE_WORK ?= $(BB_SCRATCH)/bb-dms-vsense
DMS_VSENSE_GOLD := tests/golden/dms-vsense
export DMS_VSENSE_WORK
DMS_VSENSE_ARGS  = --project $(DMS_VSENSE_WORK)/proj-50ms --bench $(DMS_VSENSE_WORK)/proj-50ms/bench/vsense.toml \
                   --for 650ms --probe net:VSYS_SENSE --sample-rate 10kHz

# In `make test` (since Phase 2 B3 the bb test file examples/dial-me-spinner/tests/e2.toml, on the
# project and its variants e2/none, e2/c5, e2/c5-none, then the scripts' checks on its outputs):
# s50, s50-none, c5 and c5-none at bb's defaults, check_dms_vsense.py on them (the
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
measured-cost-table-dms-vsense: firmware bb dms-vsense-prepare
	env -u BB_RENODE_POOL bash scripts/perf_ab.sh $(BB) $(COST_OUT)/fresh/dms-vsense $(PERF_N) \
	    "--lookahead off --adc-push every" "" - - - -- $(DMS_VSENSE_ARGS)
	$(BB) pool run --slots 1 -- bash scripts/perf_ab.sh $(BB) $(COST_OUT)/pool/dms-vsense $(PERF_N) \
	    "--lookahead off --adc-push every" "" - - - -- $(DMS_VSENSE_ARGS)

# --- Phase 1 M10: E3, the detent homing H0-H5 on dial-me-spinner (route A) ---------------------
# examples/dial-me-spinner/e3: hand-written board files (scripts/dms_home_boards.py writes them)
# carrying the main board's detent sheet, the nRF52840 (PWM0, TEMP, SAADC) and the mechanism and
# DRV5032 device M1 (dms/mechanism@2), with firmware/prebuilt/homing.elf (the procedure) and
# homing_step.elf (one full step at 446 PPS). scripts/dms_home.sh; ADR-008 Amendments 9-11.
.PHONY: pooled-dms-home pooled-dms-home-test pooled-dms-home-project dms-home-ref dms-home-update \
        cost-table-dms-home cost-table-dms-home-project dms-home-rss
DMS_HOME_OUT  ?= out/dms-home
DMS_HOME_GOLD := tests/golden/dms-home
DMS_HOME_E3   := examples/dial-me-spinner/e3
DMS_HOME_FW   := examples/dial-me-spinner/firmware/prebuilt
# The step board keeps MOT_NSLEEP analog (the bridges load it); on the homing boards it is digital-only
# (Phase 2 H5, ADR-014 D4), and U10.P1.15 is its level.
DMS_HOME_STEP_PINS = --probe net:VSYS --probe net:HOME_N --probe net:MOT_NSLEEP --probe net:U10.P0.26 \
                 --probe net:U10.P0.05 --probe net:U10.P0.07 --probe net:U10.P0.11 --probe net:U10.P1.15 \
                 --probe net:M1.HOME_N
DMS_HOME_PINS  = --probe net:VSYS --probe net:HOME_N --probe net:U10.P0.26 \
                 --probe net:U10.P0.05 --probe net:U10.P0.07 --probe net:U10.P0.11 --probe net:U10.P1.15 \
                 --probe net:M1.HOME_N
DMS_HOME_TYP_ARGS  = --netlist $(DMS_HOME_E3)/board.cir --firmware $(DMS_HOME_FW)/homing.elf --target U10 \
                     --for 2700ms $(DMS_HOME_PINS) --sample-rate 10kHz
DMS_HOME_STEP_ARGS = --netlist $(DMS_HOME_E3)/board-step.cir --firmware $(DMS_HOME_FW)/homing_step.elf \
                     --target U10 --for 45ms $(DMS_HOME_STEP_PINS) --probe net:MOT_A_OUT1 --probe net:COILA_L \
                     --probe net:MOT_B_OUT1 --probe net:COILB_L --probe net:NB_VBAT --probe net:NB_VSYS \
                     --sample-rate 1MHz
# Phase 2 R1 (ADR-013 D1): the same two cases on the project route, examples/dial-me-spinner/e3/home
# (typ) and e3/step: the imported main board with M1 as a virtual [[device]] (their probes are the
# projects' [probes]).
DMS_HOME_PSTEP_ARGS = --project $(DMS_HOME_E3)/step --bench $(DMS_HOME_E3)/step/bench.toml --for 45ms \
                      --sample-rate 1MHz
DMS_HOME_PTYP_ARGS  = --project $(DMS_HOME_E3)/home --bench $(DMS_HOME_E3)/home/bench.toml --for 2700ms \
                      --sample-rate 10kHz

# In `make test` (about 40 s; since Phase 2 B3 the runs, the step test's UART phases, the hashes and
# analog-first are the bb test file examples/dial-me-spinner/tests/e3.toml, and check_dms_home.py
# --step and compare_project_run.py read its outputs): the E3 boards are the generator's output, the
# step case at bb's
# defaults and with --engine-order analog-first (byte-identical), check_dms_home.py --step (one full
# step at 446 PPS against the suite's test_firmware_microstep_pwm), its committed one-cell hashes;
# beside them (Phase 2 R1) the step case on the project route, e3/step: its committed one-cell
# hashes, and scripts/compare_project_run.py against the board file's run (non-grant records and
# digital columns byte-identical after the name mapping).
pooled-dms-home-test: bb
	bash scripts/dms_home.sh test $(BB) $(DMS_HOME_OUT)-test

# The Phase 2 R1 gate (ADR-013 D1, gate G1; about 5 minutes): E3's step and typ on the project route
# (e3/step, e3/home) at bb's defaults and as their one-cell references (--lookahead off; equal: one
# partition, a consistency check), the references against tests/golden/dms-home/project-*,
# check_dms_home.py on project-typ, compare_project_run.py against the board files' step and typ (run
# beside them, and equal to their own goldens), analog-first on project-step, and project-typ's
# digital-only MOT nets and grant count reported (ADR-014's long grants, H5).
pooled-dms-home-project: bb
	bash scripts/dms_home.sh project $(BB) $(DMS_HOME_OUT)-project

# The plan's M10 gate (E3, about 55 minutes on 5 jobs): step, typ (tS 50 ms), worst (the suite's
# DRV5032FA corner: tS 75 ms, BOP 4.8 mT, BRP 0.5 mT, at its worst sample phase) and park (H1) at bb's
# defaults and as their one-cell references (--lookahead off), check_dms_home.py on each (H0-H5, step
# counts, every phase's step rate from the PWM edges, the park stall, the HOME_N latency gate of
# ADR-008 Amendments 10 and 12), defaults equal to the references, the references equal to
# tests/golden/dms-home, analog-first byte-identical and --max-quantum 1ms equal (typ, step; on E3 the
# defaults, the reference and 1 ms are one partition: a consistency check), 10-run determinism (typ),
# and three negative controls (homing_fast.elf; M1 as the DRV5032FB at 200 and 286 ms) that must fail
# exactly their checks (ADR-008 Amendment 12). make pool-equiv has step and typ.
pooled-dms-home: bb
	bash scripts/dms_home.sh gate $(BB) $(DMS_HOME_OUT)

dms-home-ref: bb
	bash scripts/dms_home.sh ref $(BB) $(DMS_HOME_OUT)-ref
dms-home-update: dms-home-ref
	bash scripts/dms_home.sh update $(DMS_HOME_OUT)-ref

# The Phase 1 M10 E3 row (typ: homing.elf, 2700 ms): the one-cell reference vs bb's defaults, fresh
# and on a 1-slot pool, with the protocol. Run it on an otherwise idle host (about 50 minutes at
# PERF_N = 5).
measured-cost-table-dms-home: firmware bb
	env -u BB_RENODE_POOL bash scripts/perf_ab.sh $(BB) $(COST_OUT)/fresh/dms-home $(PERF_N) \
	    "--lookahead off" "" - - - -- $(DMS_HOME_TYP_ARGS)
	$(BB) pool run --slots 1 -- bash scripts/perf_ab.sh $(BB) $(COST_OUT)/pool/dms-home $(PERF_N) \
	    "--lookahead off" "" - - - -- $(DMS_HOME_TYP_ARGS)

# The Phase 2 R1 row (the project route, e3/step: homing_step.elf, 45 ms): the one-cell reference vs
# bb's defaults (one partition on E3), fresh and on a 1-slot pool, with the protocol. Run it on an
# otherwise idle host.
measured-cost-table-dms-home-project: firmware bb
	env -u BB_RENODE_POOL bash scripts/perf_ab.sh $(BB) $(COST_OUT)/fresh/dms-home-project $(PERF_N) \
	    "--lookahead off" "" - - - -- $(DMS_HOME_PSTEP_ARGS)
	$(BB) pool run --slots 1 -- bash scripts/perf_ab.sh $(BB) $(COST_OUT)/pool/dms-home-project $(PERF_N) \
	    "--lookahead off" "" - - - -- $(DMS_HOME_PSTEP_ARGS)

# --- Phase 2 H1: ngspice robustness (ADR-015 D1-D4) --------------------------------------------
# ADR-015 G1, the progress watchdog (D1), with the lattice snap off (--ngspice-lattice-snap off,
# the step clamp before H2): the SAADC stall repro (e3/board-glued.cir, E3's typ board before H5,
# with P0.02 on the loading saadc_se view, homing_step.elf, 100 ms) ends in NoProgress:ngspice at
# 77.43 ms in the cell (77.43 ms, 77.44 ms] within STALL_MAX_S (120 s) and with bb's peak RSS under
# STALL_MAX_RSS_KB (500 MB); the outputs before the error equal a run that ends at the last
# completed grant (67.94 ms since H5's quiet grants, ADR-014 Amendment 1); a lower limit trips
# identically twice; the committed board (load=none) is the negative control. Then Phase 2 H2
# (ADR-015 Amendment 4, the lattice snap, bb's default): the repro completes 300 ms at
# --renode-quantum 5us, 10us and 20us and 1 s, and the committed E3 board completes 1 s (with the
# snap off it stalls at 502.47 ms). Fresh Renode, under the host measurement lock (a timing gate;
# about 5 minutes, so not in make test: its tier is nightly, ADR-015 D11). scripts/ngspice_stall.sh.
NGSPICE_STALL_OUT ?= out/ngspice-stall
measured-ngspice-stall: firmware bb
	bash scripts/ngspice_stall.sh $(BB) $(NGSPICE_STALL_OUT)

# Phase 2 H5, ADR-014 gate G0 (the prototype measurement): E3 typ three ways with the ADR-003 protocol
# (G0_N interleaved rounds, default 3, medians of the simulation time, fresh Renode): the committed run
# before H5 (G0_BASE_BB on the checkout G0_BASE_TREE, e.g. a detached worktree of the commit before
# H5 and its bb; skipped when unset), the model change alone (D4: the digital-only MOT nets at one-cell
# grants, --lookahead off) and D1-D4 (bb's defaults). D1-D4 must equal D4 alone except grant records,
# and the protocol gain (D4 alone / D1-D4) must be at least G0_MIN (3), else ADR-014 is withdrawn.
# scripts/dms_home_g0.sh; about 20 minutes with the base case. Run it on an otherwise idle host.
DMS_HOME_G0_OUT ?= out/dms-home-g0
measured-dms-home-g0: firmware bb
	bash scripts/dms_home_g0.sh $(BB) $(DMS_HOME_G0_OUT) $(G0_BASE_BB) $(G0_BASE_TREE)

# ADR-015 G2, engine-owned .save (D2): E3 typ (homing.elf, 2700 ms, bb's defaults) with bb's peak RSS
# at most DMS_HOME_RSS_MAX_KB: the full-length measurement with .save plus 20 % (ADR-015 Amendment 1;
# 1.5 GB without .save), and its outputs equal to tests/golden/dms-home/typ (one partition: a
# consistency check of .save). About 4 minutes; make dms-home runs the same check on its typ run.
DMS_HOME_RSS_OUT ?= out/dms-home-rss
dms-home-rss: firmware bb
	bash scripts/dms_home.sh rss $(BB) $(DMS_HOME_RSS_OUT)

# After an intended change to a package, a platform's .repl or a Renode
# plugin source the platform declares: rewrite the changed bb.lock entries
# (review, then commit). A platform's hash covers its .repl and its declared
# plugin sources only (ADR-015 D6). A bb-lock/0 file is rewritten as
# bb-lock/1, its legacy platform hashes as the scoped ones (ADR-011 D5).
locks: bb
	set -e; for b in $(PKG_BOARDS); do $(BB) open --update-lock $$b; done

# --- Phase 2 H4: lock scope and the package path (ADR-015 D6, D7) ----------
# plugin-scope (G5, about 6 s; not in make test, H4's budget is none; CI job
# adc): with Renode's own compiler, each platform's declared plugin subset
# compiles alone, the whole assembly has no module initialiser, and two
# negative controls (a hidden dependency, a [ModuleInitializer]) are caught
# (engines/cpu-renode/tests/plugin_scope.rs).
plugin-scope:
	BB_REQUIRE_RENODE=1 $(CARGO) test -p bb-cpu-renode --test plugin_scope -- --ignored --nocapture
# lock-scope (G5 end to end, about 1 min: a second bb is built): a bb with
# one edit to bb_nrf52_saadc.cs on scratch copies of every board and
# project (the committed locks, which milestone S relocked under the D6 rule:
# a stock relock of each copy gives them byte for byte) changes no STM32 lock
# and exactly the nRF52840 platform of every nRF lock (scripts/lock_scope.sh).
lock-scope: bb
	bash scripts/lock_scope.sh $(BB) $(PKG_BOARDS)

# --- Phase 2 S: the schema epoch (ADR-009..013) ------------------------------
# schema-epoch (about 3-5 min, most of it building the base commit's bb; not
# in make test): every committed board file and every project with each of its
# benches elaborates (`bb run --elaborate-only`) to the same deck from the base
# commit's tree and from this one: the base bb and this bb on the base tree
# byte for byte, and this bb on the base tree and on this tree byte for byte up
# to the first-party view renames of ADR-011 D11 (scripts/schema_epoch.sh,
# scripts/schema_epoch_decks.py). EPOCH_BASE is main before the epoch.
EPOCH_BASE ?= 61a3845
.PHONY: schema-epoch
schema-epoch: firmware bb
	bash scripts/schema_epoch.sh $(BB) $(EPOCH_BASE) out/schema-epoch

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
measured-setup-container:
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
DEMO_WORK ?= $(BB_SCRATCH)/bb-demo
demo: bb
	DEMO_OUT=$(DEMO_OUT) DEMO_WORK=$(DEMO_WORK) bash scripts/demo.sh $(BB)
measured-stranger-test-container:
	bash scripts/stranger_test.sh container $(if $(STRANGER_URL),--url $(STRANGER_URL)) $(STRANGER_ARGS)
measured-stranger-test-wsl:
	bash scripts/stranger_test.sh wsl $(if $(STRANGER_URL),--url $(STRANGER_URL)) $(STRANGER_ARGS)

# The docs site's waveform plots (site/public/plots, committed: the Pages build has no Renode).
# `make docs-plots` runs make demo's E1 runs, the E2 cases sag, rec, s50 and s50-none of make
# dms-vsense, the E3 cases typ and step of make dms-home and the spike, every Renode fresh, then
# draws <name>.{light,dark}.svg from their outputs with plain Python (scripts/docs_plots.sh,
# scripts/docs_plots.py; about 5 min alone, most of it the E3 homing).
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
GDB_WORK ?= $(BB_SCRATCH)/bb-gdb-gate
gdb gdb-test: firmware bb
	@test -n "$(GDB)" || { echo "$@: no GDB found (make setup installs gdb-multiarch; or pass GDB=<path>)"; exit 1; }
	DMS_RAIL_WORK=$(GDB_WORK) bash scripts/dms_rail.sh prepare $(BB)
	$(PYTHON) scripts/gdb_gate.py $(if $(filter gdb,$@),gate,test) --bb $(BB) --gdb $(GDB) \
	    --out $(GDB_OUT)/$(if $(filter gdb,$@),gate,test) --work $(GDB_WORK)

# --- Phase 2 E4: viewers and VS Code (ADR-016) ----------------------------------------------------
# make vcd-check (G1; CI job dms-rail, about 30 s after the demo): tools/vcd-check (wellen 0.25.7,
# Surfer's waveform library; its own crate OUTSIDE the workspace, built into its own target
# directory) loads make demo's nets.vcd files and the committed fixture tests/fixtures/vcd/mixed.vcd:
# timescale 1 fs, every $var present with its type (real as Real), every change, the $comment name
# map, and every real net equal to the run's nets.csv at every row; then six negative controls on
# copies of the fixture each fail with their own message (scripts/vcd_check.sh).
# make vcd-check-fixture: the fixture and the controls only (no demo).
# make vscode-templates (in make test, under 1 s): the static check of docs/templates/vscode.
# make vscode-test (G2; local, not in make test or CI: it downloads a pinned VS Code and the pinned
# cortex-debug VSIXs once into ~/.cache/bb/vscode-test, and needs xvfb and unprivileged user
# namespaces; about 1 min): the templates, copied verbatim into a copy of the dial-me-spinner
# project, attach cortex-debug to bb run --gdb inside a headless VS Code with no network; attach,
# reads and detach give all four committed hashes of the run without GDB, and the negative control
# (a breakpoint through cortex-debug) marks the run and changes its outputs (scripts/vscode_test.sh).
.PHONY: vcd-check vcd-check-fixture vcd-check-tool vscode-templates vscode-test
VCD_CHECK_DIR ?= $(CARGO_TARGET_DIR)/vcd-check
VCD_CHECK     := $(VCD_CHECK_DIR)/release/vcd-check
VCD_CHECK_OUT ?= out/vcd-check
vcd-check-tool:
	CARGO_TARGET_DIR=$(VCD_CHECK_DIR) $(CARGO) build --release --locked --manifest-path tools/vcd-check/Cargo.toml
vcd-check: vcd-check-tool demo
	bash scripts/vcd_check.sh $(VCD_CHECK) $(DEMO_OUT) $(VCD_CHECK_OUT)
vcd-check-fixture: vcd-check-tool
	bash scripts/vcd_check.sh $(VCD_CHECK) --fixture-only $(VCD_CHECK_OUT)
vscode-templates:
	PYTHON=$(PYTHON) bash scripts/vscode_test.sh --static
VSCODE_OUT  ?= out/vscode
VSCODE_WORK ?= $(BB_SCRATCH)/bb-vscode
vscode-test: firmware bb
	PYTHON=$(PYTHON) bash scripts/vscode_test.sh $(BB) $(VSCODE_OUT) $(VSCODE_WORK)

# --- Phase 2 H0: track worktrees, the measurement lock, the runtime baseline ----------------------
# make worktree TRACK=<name>: the git worktree ~/src/bb-<name> (WORKTREE_ROOT) on the branch
# p2/<name>, from main for a new branch, locked against a prune from Windows, with its own
# git-ignored local.mk: CARGO_TARGET_DIR ~/.cache/bb-target-wt-<name>, BB_SCRATCH
# ~/.cache/bb-wt-<name> and DMS_REPO (scripts/worktree.sh). make worktree-remove TRACK=<name>
# removes it (the branch stays). make worktree-test (H0 gate, several minutes): two throwaway
# worktrees build bb at the same time without crossing artifacts, and one shared target
# directory crosses them (the negative control; scripts/worktree_test.sh).
.PHONY: worktree worktree-remove worktree-test measure-lock-test
worktree:
	@test -n "$(TRACK)" || { echo "usage: make worktree TRACK=<name>"; exit 2; }
	bash scripts/worktree.sh $(TRACK)
worktree-remove:
	@test -n "$(TRACK)" || { echo "usage: make worktree-remove TRACK=<name>"; exit 2; }
	bash scripts/worktree.sh --remove $(TRACK)
worktree-test:
	bash scripts/worktree_test.sh

# The host measurement lock (plan section 7 rule 3): each target in MEASURE_TARGETS takes
# ~/.cache/bb/measure.lock (BB_MEASURE_LOCK) before its recipe (measured-<target>) and its
# prerequisites, and REFUSES (exit 75 from scripts/measure_lock.sh) while another measurement
# holds it, from any checkout or worktree. A new perf, cost, startup or timing target joins the
# list. make measure-lock-test (in make test): on a private lock, every member refuses while a
# probe holds it (scripts/measure_lock_test.sh).
MEASURE_TARGETS := perf lookahead-perf cost-table cost-table-nrf-saadc cost-table-dms-rail \
                   cost-table-adc-kicad cost-table-dms-vsense cost-table-dms-home cost-table-dms-home-project \
                   cost-table-dms-e1-system \
                   lookahead-startup startup-perf jit-profile-stress pool-perf nrf-costs dms-s1-cost \
                   setup-container stranger-test-container stranger-test-wsl baseline measure-lock-probe \
                   ngspice-stall bench-perf bb-test-cost pi-oe-cost dms-home-g0 pi-idd-cost \
                   setup-cache-cost ci-jobs-local
.PHONY: $(MEASURE_TARGETS) $(addprefix measured-,$(MEASURE_TARGETS))
$(MEASURE_TARGETS):
	@bash scripts/measure_lock.sh $@ $(MAKE) --no-print-directory measured-$@
measure-lock-test:
	bash scripts/measure_lock_test.sh
measured-measure-lock-probe:
	@echo "measure-lock-probe: ran"
	@if [ -n "$(PROBE_HOLD)" ]; then while [ ! -e "$(PROBE_HOLD)" ]; do sleep 0.1; done; fi

# The runtime baseline (plan H0; ADR-015 D11's PR-tier targets are relative to it): make check
# (BASELINE_ARGS) BASELINE_N times after one uncounted warm-up, on an idle ext4 worktree, under the
# measurement lock; wall times, load, and make test's per-step times in $(BASELINE_OUT)
# (scripts/baseline.sh). The result goes into AGENTS.md's gate-budget table.
BASELINE_OUT    ?= out/baseline
BASELINE_N      ?= 3
BASELINE_WARMUP ?= 1
BASELINE_ARGS   ?= check
measured-baseline:
	bash scripts/baseline.sh $(BASELINE_OUT) $(BASELINE_N) $(BASELINE_WARMUP) -- $(BASELINE_ARGS)

# Phase 2 H6 (ADR-015 D11, Amendment 5): the CI side of the tiers, measured locally (CI runs only
# on a push). make ci-jobs-local: every job of ci.yml (CI_JOBS_WORKFLOW) with its `run:` steps,
# timed (scripts/ci_jobs_local.sh; fw-rebuild skipped, CI_JOBS_SKIP). make setup-cache-cost: what
# the bb-setup action's cache of the installed toolchain saves (scripts/setup_cache_cost.sh,
# SETUP_CACHE_N rounds, medians). Both under the measurement lock; run them on an idle host.
CI_JOBS_OUT      ?= out/ci-jobs-local
CI_JOBS_WORKFLOW ?= .github/workflows/ci.yml
measured-ci-jobs-local: firmware bb
	bash scripts/ci_jobs_local.sh $(CI_JOBS_OUT) $(CI_JOBS_WORKFLOW) $(CI_JOBS)
SETUP_CACHE_OUT ?= out/setup-cache-cost
SETUP_CACHE_N   ?= 3
measured-setup-cache-cost:
	bash scripts/setup_cache_cost.sh $(SETUP_CACHE_OUT) $(SETUP_CACHE_N)
# make workflows-check: the local structural check of .github/workflows and the bb-setup action
# (scripts/check_workflows.py: YAML, jobs and steps, the matrix/steps/inputs names in expressions,
# and every `make <target>` a Makefile target), the stand-in for actionlint, which is not
# installed here. A few seconds; not in make test.
.PHONY: workflows-check
workflows-check:
	$(PYTHON) scripts/check_workflows.py

# Phase 2 H6 (ADR-015 D11, Amendment 5): the nightly tier, by name. The `fresh` job (make test
# POOL=0, the slowest CI PR job before H6), make ec-long, make ngspice-stall (H1's timing gate),
# the full make dms-home (E3) and the full make dms-vsense (E2); .github/workflows/nightly.yml runs
# the same targets, one job each. Every step runs even when an earlier one failed; their wall times
# go to $(NIGHTLY_TIMING) and make nightly fails if any step did. A nightly failure blocks the
# next milestone merge (D11). make test's dms-*-test targets stay in the PR tier.
NIGHTLY_TIMING ?= out/nightly-timing.tsv
NIGHTLY_STEPS  ?= nightly-fresh ec-long ngspice-stall dms-home dms-vsense
.PHONY: nightly nightly-fresh
nightly:
	@mkdir -p $(dir $(NIGHTLY_TIMING)) && : > $(NIGHTLY_TIMING)
	@rc=0; for s in $(NIGHTLY_STEPS); do \
	    bash scripts/step_time.sh $(NIGHTLY_TIMING) $(MAKE) $$s || rc=1; \
	done; \
	echo "nightly: step, seconds, exit status ($(NIGHTLY_TIMING)):"; cat $(NIGHTLY_TIMING); \
	if [ $$rc = 0 ]; then echo "nightly: PASS"; else echo "nightly: FAIL"; fi; exit $$rc
# The `fresh` job: make test with every Renode started fresh (its per-step times in
# out/nightly-fresh-timing.tsv, so a nightly run keeps make test's own out/test-timing.tsv).
nightly-fresh:
	$(MAKE) --no-print-directory test POOL=0 TEST_TIMING=out/nightly-fresh-timing.tsv

# make print-VAR: a variable's value (scripts use it, e.g. print-CARGO_TARGET_DIR).
print-%:
	@echo $($*)

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all -- --check

clippy:
	$(CARGO) clippy --workspace --all-targets -- -D warnings

# Everything CI runs, in CI order.
check: fmt-check clippy test
