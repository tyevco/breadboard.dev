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
        adc-equiv adc-renode adc-model cost-table cost-table-runs startup-perf \
        pool-equiv pool-perf \
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
ADC_ELF     := examples/adc/firmware/build/adc_board_isr.elf
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
# everything fresh (CI job `fresh`); POOL_SLOTS sets the pool size.
POOL       ?= 1
POOL_SLOTS ?= 5
ifeq ($(POOL),0)
WITH_POOL = env -u BB_RENODE_POOL
else
WITH_POOL = bash scripts/with_pool.sh $(BB) $(POOL_SLOTS) --
endif
POOLED_TARGETS := test partition lattice-check feedback-open lookahead-inert lookahead-equiv adc-equiv spike-verify-every determinism feedback-determinism
$(POOLED_TARGETS): firmware bb
	$(WITH_POOL) $(MAKE) --no-print-directory pooled-$@

setup:
	bash scripts/setup.sh

doctor:
	@bash scripts/doctor.sh

# Bare-metal STM32F103 blinky used by the Phase 0 spike and the Renode engine
# tests: examples/spike/firmware/build/blinky.elf
firmware:
	$(MAKE) -C examples/spike/firmware
	$(MAKE) -C examples/feedback/firmware
	$(MAKE) -C engines/cpu-renode/tests/firmware
	$(MAKE) -C examples/adc/firmware

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
	$(MAKE) adc
	$(MAKE) adc-model

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
golden-update: spike feedback-run feedback-ref feedback-open-ref adc-ref adc-open-ref
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
	cd $(ADC_OUT)-open-ref && grep -v '"type":"grant"' events.jsonl > non-grant-records \
	    && sha256sum nets.csv events.jsonl nets.vcd non-grant-records > $(CURDIR)/$(ADC_OPEN_HASHES) \
	    && rm non-grant-records

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

# --- ADR-004 Part B: the warm Renode pool ----------------------------------
# G4 pool equivalence (scripts/pool_equiv.sh): one pool process serves every
# configuration below, in alternating order over POOL_EQUIV_ROUNDS rounds;
# each pooled run must be leased and byte-identical (nets.csv, nets.vcd,
# events.jsonl, non-grant records) to a fresh run of the same configuration
# and to the committed hashes. A new board or platform must pass it before
# it uses the pool.
POOL_EQUIV_OUT    ?= out/pool-equiv
POOL_EQUIV_ROUNDS ?= 2
pool-equiv: firmware bb
	$(BB) pool run --slots 1 -- bash scripts/pool_equiv.sh $(BB) $(POOL_EQUIV_OUT) $(POOL_EQUIV_ROUNDS) -- \
	    "spike;$(SPIKE_HASHES);all;$(SPIKE_ARGS)" \
	    "spike-1ms;$(PARTITION_HASHES);all;$(SPIKE_ARGS) --max-quantum 1ms" \
	    "spike-500ms;$(PARTITION_HASHES);all;$(SPIKE_ARGS) --max-quantum 500ms" \
	    "feedback;$(FEEDBACK_HASHES);nongrant;$(FEEDBACK_ARGS)" \
	    "feedback-off;$(FEEDBACK_HASHES);all;$(FEEDBACK_ARGS) --lookahead off" \
	    "feedback-noplugin;$(FEEDBACK_HASHES);all;$(FEEDBACK_ARGS) --lookahead-plugin never" \
	    "feedback-poll;$(FEEDBACK_HASHES);nongrant;$(FEEDBACK_POLL_ARGS)" \
	    "adc;$(ADC_HASHES);nongrant;$(ADC_ARGS)" \
	    "adc-off;$(ADC_HASHES);nongrant;$(ADC_ARGS) --lookahead off" \
	    "adc-open;$(ADC_OPEN_HASHES);nongrant;$(ADC_OPEN_ARGS)"

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

# The feedback-open one-cell reference (max_quantum = the lattice), for
# golden-update.
feedback-open-ref: firmware bb
	$(BB) run $(FEEDBACK_OPEN_ARGS) --max-quantum 10us --out $(FEEDBACK_OPEN_OUT)-ref
	$(PYTHON) scripts/check_feedback.py $(FEEDBACK_OPEN_OUT)-ref --open

# ADR-003 step 9 (D6): examples/adc, an STM32F072 that reads back through
# its ADC (PA1, adc_in) the RC-filtered divider it drives (PA4); TIM3 starts
# a conversion every 999 us, the analog watchdog drives ALARM (PA5) and the
# code bits 4 and 8 go to PA6/PA7. A feedback loop, so lookahead is on by
# default: ngspice pushes ADC_IN at the lattice points where its whole
# millivolts change and stops early there (the code-change watch).
# check_adc.py: every conversion's edges as the firmware model predicts from
# ADC_IN.
adc-run: firmware bb
	$(BB) run $(ADC_ARGS) --out $(ADC_OUT)
	$(PYTHON) scripts/check_adc.py $(ADC_OUT)

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

# ADR-003 step 9 gates (scripts/adc_equiv.sh): Part A (one cell) and Part B
# (lookahead, 1 ms and 10 ms) on the cyclic board, and the open board in
# long grants, all byte-identical to the every-lattice-point references
# (committed hashes) except grant records; no promise broken; early stops at
# pushes. CI job `adc`.
ADC_EQUIV_OUT ?= out/adc-equiv
pooled-adc-equiv: firmware bb
	bash scripts/adc_equiv.sh $(BB) $(ADC_EQUIV_OUT) $(ADC_HASHES) $(ADC_OPEN_HASHES) \
	    "$(ADC_ARGS)" -- "$(ADC_OPEN_ARGS)"

# The Renode-only ADC platform test of ADR-003 Amendment 5 (about 15 s).
# ADR-005 test plan 1-3: Renode-only tests of Breadboard's STM32F072 ADC
# model and BbStm32Timer TRGO (examples/adc/bb_adc_test.py: pwm, trgo,
# registers, firmware; about 80 s). In `make test`.
adc-model: firmware
	$(MAKE) -C examples/adc model-test

adc-renode:
	$(MAKE) -C examples/adc test

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all -- --check

clippy:
	$(CARGO) clippy --workspace --all-targets -- -D warnings

# Everything CI runs, in CI order.
check: fmt-check clippy test
