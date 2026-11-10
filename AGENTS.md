# AGENTS.md: working on Breadboard

Breadboard is an open hardware simulator ("QEMU for whole circuit boards"). The
repo is in **Phase 0**: a Renode (STM32F103, and STM32F072 for the ADC
board) ↔ ngspice co-simulation spike under a kernel-owned time-grant
protocol, with the speed-ups of ADR-003 (the grant lattice, traces, CPU
lookahead, ADC pushes), ADR-004 (startup) and ADR-005 (Breadboard's own
STM32F072 ADC model and sampled inputs). Read this file first. Then read
`spec/time-grant.md` before you touch anything that advances time.

## Build and test

Run every command inside WSL (Ubuntu 24.04) or Linux, from the repo root.

| Command | What it does |
| --- | --- |
| `make setup` | Install or verify the pinned toolchain (idempotent): Rust 1.99.1, uv, Renode 1.17.0 portable, arm-none-eabi-gcc, ngspice 42 + libngspice0-dev, python3-jsonschema; then build `bb` and install it into `~/.local/bin`. `make setup SETUP=demo` skips the ARM toolchain, uv and jsonschema (what the demo needs: Rust, Renode, ngspice, `bb`). As root (a container) it runs apt without sudo (Phase 1 M3) |
| `make setup-container` | Phase 1 M3 gate (R7): `make setup SETUP=demo` timed in a clean `ubuntu:24.04` container (podman, or docker), as root, from `git archive HEAD` (`SETUP_REV=`, or `SETUP_WORKTREE=1`); timing in `out/setup-container/timing.txt` (`scripts/setup_container_test.sh`) |
| `make doctor` | Print every tool's version; exits non-zero if anything is missing |
| `make build` | `cargo build --workspace --all-targets` |
| `make test` | The exit gate: `cargo-test`, `spike`, `determinism`, `golden`, `feedback`, `feedback-order`, `adc`, `adc-order`, `adc-stock`, `adc-model`, `ec-reconnect`, run on a warm Renode pool (ADR-004 Part B; `POOL=0` runs it fresh, `POOL_SLOTS` sets the pool size, default 5). `make check` took 2 min 40 s on the pool against 3 min 10 s fresh before it, under load 6–11, ADR-004 Amendment 4 |
| `make cargo-test` | `cargo test --workspace` only |
| `make spike` | Build firmware + `bb`, run 500 ms of co-simulation into `out/spike` (50 grants of the default 10 ms `--max-quantum`), check it (`scripts/check_spike.py`) |
| `make spike-verify-every` | The spike again with `--verify every` (Renode cross-check after every grant, ADR-003 D1.2); outputs must be byte-identical to `out/spike` (CI job `verify-every`) |
| `make lattice-check` | ADR-003 D2 gate: the spike in 1 ms grants on the 10 µs lattice must give `out/spike`'s `nets.csv` and non-grant records byte for byte, and a 1 kHz CSV must sample the 100 kHz waveform exactly (`scripts/check_resample.py`) |
| `make partition` | ADR-003 step 3 gate: the spike at `--max-quantum` 10 µs, 100 µs, 10 ms, 50 ms and 500 ms must give the committed baseline `nets.csv`, `nets.vcd` and non-grant records (`tests/golden/spike/partition.sha256`) byte for byte (`scripts/partition.sh`; CI job `partition`) |
| `make perf` | ADR-003 performance gate with the measurement protocol: interleaved 10 µs vs 10 ms grant runs, N = 5 (`PERF_N`), medians; 10 ms must take exactly 50 grants, ≤ 2 s simulation and be ≥ 15× faster (`scripts/perf.sh`). Run it on an otherwise idle host |
| `make determinism` | 10 spike runs (`DET_RUNS`, `DET_JOBS` in parallel); sha256 of `nets.csv`, `events.jsonl`, `nets.vcd` must match |
| `make golden` | Diff `out/spike` LED_A against `tests/golden/spike/led_a.csv` (1 mV / 1 sample), then check `out/spike`'s `nets.csv`, `events.jsonl` and `nets.vcd` against `tests/golden/spike/outputs.sha256` (the default 10 ms partition, grant records included) |
| `make feedback` | ADR-003 step 4: `examples/feedback` (U1.PA5 → RC → U1.PB0 `digital_in`; the EXTI0 ISR copies PB0 to PA6 → LED_B), 500 ms with CPU lookahead on (the default since ADR-003 step 8: 117 grants; with `--lookahead off` a feedback loop runs 50 000 one-cell grants). `scripts/check_feedback.py`: PB0 transitions at the analytic RC crossings in their lattice cells, PA6 edges in `(ceil_L(t_x), ceil_L(t_x) + L]`, LED_B plateaus; then the golden snapshot `tests/golden/feedback/` (SENSE, LED_B within 1 mV / 1 sample, exact `pin_edge` list) and `nets.csv`, `nets.vcd` and the non-grant records byte-identical to the one-cell reference hashes `tests/golden/feedback/outputs.sha256`. `make feedback-run` skips the golden; `make feedback-ref` is the `--lookahead off` reference run |
| `make feedback-order` | The feedback run with the engines registered in the other order (`bb run --engine-order analog-first`) must be byte-identical (ADR-003 D3) |
| `make feedback-determinism` | 10 runs of `examples/feedback`, byte-identical (CI job `feedback`) |
| `make feedback-open` | The acyclic variant `examples/feedback-open` (ngspice → U1 only, long grants) at `--max-quantum` 10 µs … 500 ms must give the committed one-cell hashes (`tests/golden/feedback-open/partition.sha256`), pass `check_feedback.py --open` and not depend on the registration order (CI job `feedback`) |
| `make lookahead-inert` | ADR-003 step 5: Renode's lookahead plugin is inert. `examples/feedback` without the plugin (`--lookahead-plugin never`, so lookahead off) must give the committed hashes (`tests/golden/feedback/outputs.sha256`), with the plugin armed and `--lookahead off` byte-identical, with lookahead on (the default) identical except `grant` records; the spike with the plugin forced on must give its partition hashes (CI job `feedback`) |
| `make lookahead-equiv` | ADR-003 step 8 equivalence gate: `examples/feedback` and its polling-firmware variant (`feedback_poll.elf`) with lookahead on at `--max-quantum` 1 ms and 10 ms must give the one-cell `--lookahead off` reference (the committed `tests/golden/feedback/outputs.sha256`) byte for byte in `nets.csv`, `nets.vcd` and every non-grant record, with no broken promise and at most 200 grants per 500 ms at 10 ms (`scripts/lookahead_equiv.sh`; CI job `feedback`) |
| `make lookahead-perf` | ADR-003 step 8 performance gates with the measurement protocol (`scripts/perf_ab.sh`, interleaved, N = `PERF_N`, medians): `examples/feedback` with lookahead on vs step 4 (one-cell, no plugin) at most 200 grants and >= 10x less simulation time; the polling firmware with lookahead on vs off at most 10 % slower. Run it on an otherwise idle host |
| `make adc` | ADR-005 (ADR-003 step 9): `examples/adc` on Breadboard's STM32F072 (`BbStm32F0Adc`, `BbStm32Timer`; U1.PA4 → RC-filtered divider → ADC_IN → U1.PA1 `adc_in`; TIM3 TRGO triggers a conversion every 999 µs, sampled at T + 1063 ns, ended at T + 4750 ns; the ADC interrupt drives ALARM on PA5 from the analog watchdog, code bits 4 and 8 on PA6/PA7, PA4 every 50 conversions), 300 ms with lookahead on (about 1 200 grants). `scripts/check_adc.py`: every conversion's edges as the firmware and the model's timing predict from ADC_IN, and no `stop:ngspice` (the ADC pin is a sampled input); then `nets.csv`, `nets.vcd` and the non-grant records byte-identical to the one-cell reference that pushes at every lattice point (`tests/golden/adc/outputs.sha256`; `make adc-ref`) |
| `make adc-order` | The `adc` run with `--engine-order analog-first` must be byte-identical |
| `make adc-stock` | The ADR-003 board on Renode's stock ADC (`bb run --adc-model stock`, `adc_board_isr.elf`): its committed one-cell references (`tests/golden/adc-stock`, the ADR-003 hashes; `make adc-stock-ref`, `make adc-open-stock-ref`) |
| `make adc-model` | ADR-005 test plan 1-3 (`examples/adc/bb_adc_test.py`, about 80 s): TIM3 PWM through the overlay's AF wiring, TRGO, the ADC's registers and exact conversion instants for every SMP × RES × CKMODE, AWD, the host port, and the board firmware (TRGO conversions, the AWD interrupt, no wake on a push, timed inputs = `SetValue`) |
| `make adc-equiv` | ADR-005 step 5 gates (`scripts/adc_equiv.sh`): Part A (one cell) and Part B (lookahead, 1 ms and 10 ms) on `examples/adc`, and `board-open.cir` (acyclic) at 1, 10 and 50 ms, all identical except `grant` records to the every-lattice-point references (`tests/golden/adc*/outputs.sha256`); no promise broken; no `stop:ngspice` (CI job `adc`) |
| `make adc-determinism` | 10 runs of `examples/adc`, byte-identical |
| `make adc-speed` | ADR-005 step 6 speed gates (`scripts/perf_ab.sh`, interleaved, N = `PERF_N`, medians): `examples/adc` on the model takes at most 2 000 grants per 300 ms and at least 3× less simulation than the ADR-003 default (`--adc-model stock`) in the same run. Run it on an idle host |
| `make adc-startup` | ADR-005 D6 startup gate (`scripts/plugin_startup.sh`, fresh Renode, interleaved, N = `ADC_STARTUP_N`, medians): the ADC board's engine startup on Breadboard's STM32F072 with a warm plugin cache within 0.3 s of the stock STM32F072 with the lookahead plugin, and a cold start (the plugin assembly compiled) within 3 s of a warm one. Run it on an idle host |
| `make adc-renode` | The Renode-only test of the stock ADC model of ADR-003 Amendment 5 (`make -C examples/adc test`, about 15 s; CI job `adc`) |
| `make ec-reconnect` | ADR-004 Amendment 8: the one-cell references of `examples/adc` (`--lookahead off --adc-push every`) and `examples/feedback` (`--lookahead off`), with the Renode engine replacing its External Control connection every 2 000 messages (`EC_MESSAGES`, passed as `BB_RENODE_EC_MESSAGES`), must give their committed hashes (about 40 s; in `make test`) |
| `make ec-long` | ADR-004 Amendment 8 long one-cell gate (`scripts/ec_long.sh`, about 2.5 min): the ADC board's every-lattice-point reference over 1 500 ms (`EC_LONG_FOR`; Renode used to crash after 81 746 grants) must finish, reconnect at least once, pass `check_adc.py` and equal the lookahead run except grant records (CI job `adc`) |
| `make nrf-gates` | Phase 1 M2 gate (ADR-008 D1-D4, D6; Amendment 2): the Renode engine tests `engines/cpu-renode/tests/nrf52840.rs` on Breadboard's nRF52840 (`Platform::Nrf52840Bb`, `nrf52840-bb@1`: no SVD download, 64 MIPS) with the bare-metal gate firmware in `engines/cpu-renode/tests/firmware/nrf52840/` (nrf-blinky edges identical at one-cell grants, 10 ms grants, plugin armed and lookahead on, self-test status exactly 0x3; nrf-ppi; nrf-busywait: a 10 ms nrfx busy-wait takes 6.66 ms of virtual time; the D6 TIMER-compare-then-long-sleep promise clamp; the PIN_CNF audit, mid-run and end-of-run) and the prebuilt Zephyr gate image (nrf-zephyr), then `make nrf-feedback` (about 2 min) |
| `make nrf-feedback` | ADR-008 D3 board-level gate (in `make test`): `examples/nrf-feedback` (examples/feedback's circuit on the nRF52840: RTC0 drives `U1.P0.13`, GPIOTE IN on `U1.P0.11` copies to `U1.P0.15`) with lookahead on at 1 ms and 10 ms byte-identical to its one-cell `--lookahead off` reference (`tests/golden/nrf-feedback/outputs.sha256`; `make nrf-feedback-ref`), no promise broken, `check_feedback.py --nrf`, and `--engine-order analog-first` byte-identical |
| `make nrf-costs` | ADR-008 D1: fresh startup and per-grant cost of the nRF52840 blinky at 64 MIPS against the STM32F103 blinky (interleaved, N = `BB_NRF_COST_N`, medians); reports only. Run it on an idle host |
| `make lookahead-startup` | ADR-003 step 5 startup-cost gate: Renode engine startup with the cached plugin within 0.3 s of a start without it (interleaved, N = 7, medians; run on an idle host) |
| `make startup-perf` | ADR-004 G1: fresh `bb run` engine startup of the spike and the feedback board at least 12 % and 0.3 s below `BB_RENODE_STOCK_RUNTIME=1` (the stock .NET settings, no JIT profile and the 100 ms connect poll), interleaved, N = 7 (`STARTUP_N`), medians, outputs identical; `STARTUP_BASE=<bb>` compares against another binary. Run it on an idle host |
| `make pool-equiv` | ADR-004 G4: one pool process serves the spike (default, 1 ms, 500 ms), feedback (on, off, plugin never, polling firmware), adc (on, off) and adc-open twice, in alternating order; every pooled run must be leased and byte-identical to its fresh run and to the committed hashes (`scripts/pool_equiv.sh`) |
| `make pool-perf` | ADR-004 G2: on a 1-slot pool, the first lease starts in at most 0.5 s, median engine startup is at most 0.30 s for spike, feedback and adc (N = 7), and spike total wall at most 1.2 s (`scripts/pool_perf.sh`). Run it on an idle host |
| `bb pool run -- <cmd>` | Run `<cmd>` with a warm Renode pool in `BB_RENODE_POOL` (`--slots N`, `--max-runs K`); `bb pool start` / `stop` / `status` for an interactive shell (`bb pool --help`). `bb run --renode-pool off` ignores a pool |
| `make golden-update` | Regenerate the golden snapshots, the spike output hashes and the partition hashes (spike, feedback, feedback-open), the feedback one-cell reference hashes and the ADC reference hashes from fresh, checked runs. Review the diff before committing it: a change to an equivalence reference is a change to results and needs a reason (and, for the protocol, an ADR) |
| `make cost-table` | The README cost table with the measurement protocol (`scripts/perf_ab.sh`, interleaved, N = `PERF_N`, medians): the spike at 10 µs vs 10 ms grants, `examples/feedback` and `examples/adc` with lookahead off vs on, `examples/feedback-open` at 10 µs vs 10 ms. Reports only; no gate. Run it on an otherwise idle host |
| `make dms-source` | Phase 1 M0: the vendored dial-me-spinner KiCad inputs (`examples/dial-me-spinner/kicad/`) against the sha256s in `examples/dial-me-spinner/SOURCE`, and against `git show <commit>:<path>` when `DMS_REPO` (default `../dial-me-spinner`) exists (`scripts/dms_source_check.sh`) |
| `make dms-suite-check` | Phase 1 M0 gate: re-run the dial-me-spinner SPICE suite (`-m "not slow"`) at the reference commit `6854d25` from a `git archive` export in `~/.cache/bb-dms-suite` (`scripts/dms_suite.sh`, about 4-5 min) and require `out/dms-suite/results.json` to equal the committed `examples/dial-me-spinner/suite/results.json` byte for byte (checks, test outcomes, every deck's numeric fallback level, ngspice version). `make dms-suite-update` regenerates it. Never check out, stash or write in the dial-me-spinner repository |
| `make dms-s1` | Phase 1 plan S1 (ADR-006 Amendment 2): the E1 deck (two LED rail on/off cycles, 120 ms, both benches) through `NgspiceEngine` at `phase0`, with the S1 fix of the TPS22917 view. The view and every deck are built in `~/.cache/bb-dms-s1` from a `git archive` export of `6854d25`; they are TI-derived, so they stay local-only and never enter the repository (`scripts/dms_s1.sh`, `scripts/dms_s1.py`, `engines/analog-ngspice/examples/deck_run.rs`). Its 18 gates: 120 ms at both benches; grants of 10 µs, 1 ms and 10 ms byte-identical; VLED at the check instants and the LSW-2 time against the suite's `suite_fallback` run; VLED at every 10 µs against the converged run; the fix's result neutrality. About 1.5 min on 8 jobs (`DMS_S1_JOBS`). `make dms-s1-cost` is the S1 cost table (plan §8; `DMS_S1_N` interleaved rounds, medians; run it on an idle host) |
| `make packages` | ADR-006 (Phase 1 M3; in `make test`): every `part.toml`, `pins.toml`, test vector, `bb.lock`, project `bb.toml`, bench and `board.bbm` against `spec/schema/*.schema.json` (`scripts/validate_packages.py`, python3-jsonschema), `bb pkg lint` clean, `bb pkg test` (every vector through `NgspiceEngine` at one cell and 1 ms, identical, and at every recorded qualification; the first-party packages and `examples/adc/kicad/packages`), and every example's `bb.lock` and every example project's `board.bbm` current (`bb open --check`); `scripts/check_local_only.sh` (no `LicenseRef-local-only` package, nothing under `local-packages/` and no TI-derived model file is tracked) |
| `make dms-vectors` / `make dms-vectors-check` | Phase 1 M5 (ADR-006 Amendment 3): regenerate the test vectors of `packages/ti/tps22917` and `packages/worldsemi/ws2812c-2020` from the dial-me-spinner suite's own deck builders at `6854d25` (`scripts/dms_packages.sh`, `scripts/dms_packages.py`; `git archive` export, suite venv), or require the committed ones byte for byte and `nordic/nrf52840-qiaa`'s GPIO sets to equal the suite's resistances (needs `DMS_REPO`) |
| `make dms-local-packages` | Phase 1 M5: build the LOCAL-ONLY vendor-derived `ti/tps22917` (`LicenseRef-local-only`: the suite's TI-derived view with the S1 fix and the `GND` -> `GNDP` rename) in the git-ignored `local-packages/` (`DMS_LOCAL_PACKAGES`), qualify it with `bb pkg test --record`, and run gate G8 (the rename gives byte-identical `bb pkg test --report` lines). Never commit `local-packages/` |
| `make dms-m5b` | Phase 1 M5b gate (after `dms-local-packages`): the committed datasheet-fitted TPS22917 view against the local vendor-derived one on the E1 checks through `NgspiceEngine` at `phase0`, both benches (VLED at each P0.06 rise and the LSW-3 maximum within 10 mV, LSW-2 within 1 ms, the wrong-order excursion within 10 mV; `scripts/dms_m5b.py`, `out/dms-m5b/summary.json`, about 40 s) |
| `make locks` | `bb open --update-lock` on every example board and project: after an intended change to a package, a platform `.repl` or **any Renode plugin source** (they are in every Renode platform's content hash, ADR-006 D8); review and commit the `bb.lock` and `board.bbm` diffs |
| `make adc-kicad` | Phase 1 M4 gate M4a (ADR-007 Amendment 1; in `make test`): `examples/adc/kicad`, the ADC board drawn in KiCad, imported and run as a `bb.toml` project (a copy in `~/.cache/bb-adc-kicad`, firmware recorded with `bb flash`; `scripts/adc_kicad.sh`): `board.bbm` current; equal to its one-cell reference and to `tests/golden/adc-kicad/outputs.sha256`; the `.net` and `kicad-cli` routes byte-identical (when `kicad-cli` is installed); board.cir's committed non-grant records byte for byte and `nets.csv`/`nets.vcd` within 1 mV (2 samples differ by 1 µV: C1's `470n` against `4.7e-7`, which the script checks byte for byte); `bb run` writes nothing into the project. `make adc-kicad-prepare` makes the copy that `pool-equiv` and `cost-table` run |
| `make kicad-import` | Phase 1 M4 gates M4b-M4f on the dial-me-spinner main board at 6854d25 (`scripts/kicad_import.sh`, about 20 s; needs KiCad 10 in WSL and the Windows `kicad-cli.exe` unless `KICAD_WINDOWS=0`): 201 parts, 142 nets, 657 pin nodes; the resolution counts; one `board.bbm` from the committed netlist, fresh Linux and Windows exports and a partial KiCad 10 re-save (dc96515's KiCad 9 netlist as the secondary check); byte-identical re-imports; unchanged sources; the version refusals. Not in `make test` (no KiCad in CI); `cli/tests/kicad_import.rs` (the two-board fixture) is |
| `make dms-values` | ADR-006 G3: `bb pkg value --kicad` on the 29 distinct passive values of the dial-me-spinner main board equals the suite's `spicelib.parse_value` (re-derived from `git show 6854d25:hardware/sim/spicelib.py` when `DMS_REPO` exists, else the committed fixture `tests/fixtures/values/dms-main.json`) |
| `make fmt` / `make fmt-check` | `cargo fmt --all` (`-- --check` in CI) |
| `make clippy` | `cargo clippy --workspace --all-targets -- -D warnings` |
| `make check` | `fmt-check`, `clippy` and `test`, which is what CI runs |

`make firmware` builds the firmware ELFs (spike, feedback, ADC and the Renode
lookahead gate firmware in `engines/cpu-renode/tests/firmware`, and the
nRF52840 gate firmware in its `nrf52840/` subdirectory, built against the
vendored nrfx MDK and CMSIS-Core headers in `nrf52840/vendor/`) on their own.
The nRF52840's Zephyr gate image is committed prebuilt
(`nrf52840/zephyr/prebuilt/`, stripped of debug information);
`scripts/nrf_zephyr_gate.sh` rebuilds it where a Zephyr workspace exists and
checks the loadable image is unchanged.

## Repo map

```
bbcore/                 kernel: Time, PinSignal, Engine trait, Scheduler, EventSink, CsvWriter, VcdWriter;
                        board.rs (board.bbm types, ADR-007 D6), cjson.rs (integer-only canonical JSON)
importers/kicad/        bb-kicad (ADR-007 D1-D3): S-expression reader, the netlist model and its version
                        gates, schematic gates, the kicad-cli runner (on a temporary copy, never in the project)
bbpkg/                  component packages (ADR-006, spec/package.md): value grammar and integer units,
                        TOML reader, part.toml/pins.toml, lint, view files and deck assembly, test
                        vectors through NgspiceEngine, tree hashes and bb.lock, schema versions, SHA-256
packages/               first-party packages: generic/{resistor,capacitor,inductor}, st/stm32f103c8,
                        st/stm32f072rb; Phase 1 M5: ti/tps22917 (datasheet-fitted view), worldsemi/
                        ws2812c-2020, nordic/nrf52840-qiaa, raytac/mdbt50q-u1mv2 (a module)
local-packages/         git-ignored, LOCAL-ONLY packages (make dms-local-packages: the vendor-derived
                        ti/tps22917); never committed
engines/analog-ngspice/ ngspice shared-lib engine (libngspice.so.0)
                        examples/deck_run.rs: a SPICE body through NgspiceEngine with digital-glue edges (S1)
engines/cpu-renode/     Renode engine over the External Control API; platforms/: the plugin
                        assembly's sources (bb_lookahead.cs, the lookahead block, ADR-003 D5;
                        bb_stm32_timer.cs, STM32_Timer + TRGO; bb_stm32f0_adc.cs, the STM32F0
                        ADC model and its host port, ADR-005) and stm32f072-bb.repl (the F072
                        overlay); tests/firmware (gate firmware); console.rs, pool.rs,
                        manager.rs (the warm Renode pool, ADR-004)
cli/                    `bb` binary (`bb run`, `bb pool`, `bb open`, `bb pkg`, `bb flash`, `bb probe`,
                        board-file parser, platforms.rs: the Renode platform identities of ADR-006 D8;
                        Phase 1 M4: project.rs (bb.toml, benches), names.rs, import.rs (resolution, D4),
                        elab.rs (bindings, analog scope, the deck, run.json))
examples/spike/         blinky firmware + board.cir (SPICE body + *bb pin bindings; U1 from the package
                        st/stm32f103c8 since M3, pinned in examples/*/bb.lock like every example)
examples/feedback/      feedback board (digital_in PB0 reads the RC that PA5 drives) + EXTI firmware and a polling variant
examples/feedback-open/ the same with the loop opened (acyclic, long grants)
examples/adc/           STM32F072 ADC board (adc_in PA1 reads the divider PA4 drives; board.cir,
                        board-open.cir), its firmware (adc_board.elf on Breadboard's ADC model,
                        adc_board_isr.elf on the stock one, test firmware), bb_adc_test.py (the
                        ADR-005 model tests) and renode_adc_test.py (the stock model's test)
examples/adc/kicad/     the ADC board drawn in KiCad (Phase 1 M4): build_schematic.py, the KiCad project,
                        adc.net, bb.toml, board.bbm, bb.lock, packages/example/led-red (a local package)
examples/dial-me-spinner/ Phase 1 reference board (dial-me-spinner 6854d25): SOURCE (commits and
                        sha256 of every vendored file), kicad/ (main and base projects; kicad9-dc96515/
                        the secondary KiCad 9 netlists), suite/results.json (the suite re-run, M0),
                        bb.toml with its board.bbm and bb.lock (M4)
examples/nrf-feedback/  examples/feedback's circuit on the nRF52840 (Phase 1 M2; firmware
                        engines/cpu-renode/tests/firmware/nrf52840/nrf_feedback.c)
engines/cpu-renode/platforms/nrf52840-bb.repl  Breadboard's nRF52840 (ADR-008 D1); its gate
                        firmware, vendored headers and Zephyr gate image are in
                        engines/cpu-renode/tests/firmware/nrf52840/, its tests in tests/nrf52840.rs
tests/golden/spike/     led_a.csv waveform snapshot, default-run output hashes, partition hashes
tests/golden/feedback/  SENSE / LED_B snapshots, exact pin_edge list, one-cell reference hashes
tests/golden/feedback-open/ partition hashes of the one-cell reference
tests/golden/adc/, adc-open/ hashes of the every-lattice-point one-cell ADC references (ADR-005)
tests/golden/adc-stock/, adc-open-stock/ the same on the stock ADC model (ADR-003)
tests/golden/nrf-feedback/ one-cell reference hashes of examples/nrf-feedback (Phase 1 M2)
tests/golden/adc-kicad/ one-cell reference hashes of the examples/adc/kicad project (Phase 1 M4)
tests/fixtures/kicad/   the synthetic two-board system of ADR-007 gate M4b (cli/tests/kicad_import.rs)
spec/                   normative protocol and schema docs (package.md and schema/*.schema.json: ADR-006;
                        board-bbm.md, canonical-json.md, project.md: ADR-007); changing them needs an ADR
docs/adr/               architecture decision records
scripts/                setup.sh, doctor.sh, check_spike.py, golden.py, determinism.sh,
                        partition.sh, perf.sh, perf_ab.sh (also make cost-table),
                        check_resample.py, check_feedback.py, lookahead_inert.sh,
                        lookahead_equiv.sh, check_adc.py, adc_equiv.sh, startup_perf.sh,
                        with_pool.sh, pool_equiv.sh, pool_perf.sh, dms_vendor.sh,
                        dms_source_check.sh, dms_suite.sh (+ dms_suite_plugin.py,
                        dms_suite_collect.py), validate_packages.py, dms_values.py,
                        check_pins_kicad.py, setup_container_test.sh,
                        nrf_vendor_headers.sh, nrf_zephyr_gate.sh, adc_kicad.sh,
                        kicad_import.sh, compare_outputs.py, dms_packages.sh (+ dms_packages.py,
                        dms_m5b.py), check_local_only.sh
```

## The three invariants

1. **Determinism.** The same inputs and seed must produce byte-identical outputs
   (`events.jsonl`, `nets.csv`, `nets.vcd`). That means integer femtosecond
   `Time` for every scheduling decision, ordered collections (`BTreeMap`/`Vec`,
   never `HashMap` iteration into an output), no wall-clock reads, no unseeded
   randomness, and no thread-timing-dependent results. Samples are taken at
   fixed sim-time instants, not at solver points, and must not add grant
   boundaries (that changes Renode's results; ADR-002 Amendment 3).
2. **Pin-boundary contract.** Engines interact only through pins bound to nets.
   Each pin has exactly one pin model (`spec/pin-model.md`) and each net has
   exactly one driver. Values cross the boundary as `PinSignal`
   (digital level plus `edge_time`, or analog volts) and only at exchange
   boundaries. No engine reaches into another engine's state.
3. **No engine free-runs.** Engines advance only on kernel grants:
   `advance_to(g)` with `now < g <= t_safe`, stopping at exactly `g`. No engine
   runs in the background between calls, and no engine's time ever passes the
   latest grant. `t_safe = Time::MAX` means "I report every change in my
   trace", not "I am idle"; `--max-quantum` (default 10 ms) caps every grant.
   Every boundary lies on the grant lattice (`--renode-quantum`, default
   10 µs). A feedback loop caps grants at one lattice cell unless a sleeping
   MCU promises quiet outputs (`quiet_until`, CPU lookahead); then its
   producer runs first and stops early (`advance_until`) at the lattice
   point after the first change the MCU reads, and the MCU's wake is
   audited (`PromiseBroken`).

The grant partition is never part of the result: `--max-quantum` and
`--lookahead` change only `grant` records. Every speed-up is gated by
byte-identical `nets.csv`, `nets.vcd` and non-grant records against the
one-cell reference (`--max-quantum 10us`, or `--lookahead off` on a
feedback board).

## Rules

- **No PR changes `spec/` without an ADR.** Add or amend one in `docs/adr/`
  in the same change. ADR-002 explicitly expects an "Amendment" section from
  the ngspice engine agent, which resolves sub-decision D5.
- ADRs are written *before* implementation for anything that touches the kernel
  protocol or the package schema. Everything else is your call; record the
  reasoning in the commit message.
- Verify external API facts (ngspice `sharedspice.h`, the Renode External Control
  API in `~/.local/opt/renode-1.17.0/tools/external_control_client`, monitor
  commands) against installed headers, docs or upstream sources. Do not guess.
- Renode stays pinned to 1.17.0. The lookahead plugin
  (`engines/cpu-renode/platforms/bb_lookahead.cs`) reads 1.17.0 internals by
  reflection (`NVIC.pendingIRQs`, `TimeSourceBase.delayedActions`) and relies
  on its WFI hook (ADR-003 approver decision 3, Amendment 8). Its startup
  self-test falls back to no promise if they are missing, but a Renode
  upgrade still needs an ADR and must pass `make lookahead-inert`,
  `make lookahead-equiv`, `make adc-equiv` and the gate firmware in
  `engines/cpu-renode/tests/lookahead.rs` again.
- A new speed-up or a new board gets an equivalence gate against its
  one-cell reference before it is on by default, and its cost-table row is
  measured with `make cost-table` (the ADR-003 measurement protocol), never
  from one run.
- **The Renode pool is not an engine** (ADR-004 Part B). It holds no
  simulated time and, between leases, no machine. A lease runs the same
  `session.resc` a fresh run does, and nothing about a run's results may
  depend on whether its Renode was leased or spawned. A new board, a new
  platform or a new peripheral with process-wide state must pass
  `make pool-equiv` (added to its configuration list) before it runs on
  the pool. A pool process is never reused after a failed lease. Only
  `BB_RENODE_POOL` names a pool; nothing discovers one.
- **No orphans.** Every Renode is spawned through
  `bb_cpu_renode::renode_command` (kill on the death of the spawning
  thread). Spawn it from a thread that lives as long as the process is
  used.
- **Packages and platforms are pinned** (ADR-006 D8). A board file whose
  parts come from packages (`*bb part U1 package=...`) needs a current
  `bb.lock` next to it; `bb run` refuses a mismatch and never writes the
  lock. A change to a package's files, to a platform's `.repl` or to any
  Renode plugin source in `PLUGIN_SOURCES` changes those hashes: run
  `make locks`, review, and commit the `bb.lock` files with the change. A
  new platform adds a row to `cli/src/platforms.rs`. A package value is a
  string in the value grammar (`spec/package.md` section 6), never a SPICE
  suffix: write `Meg`, never a bare `M`.
- **Local-only models stay local** (Phase 1 approver decision A3, ADR-006
  D9 rule 4). A view derived from a vendor model whose redistribution terms
  were not checked (the TI-derived TPS22917) lives only in a
  `LicenseRef-local-only` package under the git-ignored `local-packages/`
  (or outside the repository), built from a `git archive` export. Never
  `git add` it, never copy its text into a committed file; `make packages`
  fails if one is tracked. The committed default is a datasheet-fitted view.
- **Projects (ADR-007, `spec/project.md`).** `bb open` is the only command
  that writes into a project (`board.bbm`, `bb.lock`, `.bb/`; `bb.toml`
  only when it creates a minimal one), `bb flash` and `bb probe` edit
  `bb.toml` in place, and `bb run` is read-only on the project: it refuses
  a stale `board.bbm` or `bb.lock` ("run `bb open`") and writes only under
  `--out`. `kicad-cli` always runs on a temporary copy. `board.bbm` is
  content-only and canonical: never put paths, hashes of sources, tool
  strings or dates into it (they go to `.bb/import.json`, which is not
  committed). Example projects commit `board.bbm` and `bb.lock`; a package
  change means `make locks` and a reviewed `board.bbm` diff.
- Git is local only: never push and never add remotes. Commit messages end with
  the co-author trailer given by the orchestrator.

## Known bad ideas (do not do these)

- **Stepping ngspice per instruction.** ngspice advances one *grant* at a
  time, to a breakpoint at `g`.
- **Letting Renode's virtual time run ahead of the grant.** Drive Renode only
  with `RunFor` calls that end on the grant lattice at or before `g`. Never use
  `start`/free-running emulation and then try to catch up.
- **A `RUN_FOR` that ends off the lattice** (ADR-003 D2). Renode's results
  depend on its sync-point sequence; every boundary must be a multiple of the
  Renode quantum (`--renode-quantum`), which is run configuration, not a
  tuning knob to couple to `--max-quantum`.
- **Applying an MCU input before `ceil_L(edge_time)`.** Renode sees an input
  only at a sync point; earlier is acausal, later than that is a skew. The
  same holds for an ADC push: ngspice sends it at a lattice point `t` (where
  the 12-bit code changes, ADR-005 D5). On Breadboard's ADC model the engine
  writes it to the model as a timed input before Renode runs past `t`, and a
  conversion reads the latest push strictly before its sample instant; on
  the stock model (`--adc-model stock`, whole millivolts) Renode applies it
  with `ADC SetValue` while paused at `t`. Never at the engine's `now`
  before `t` (`RenodeEngine::apply_input_trace`).
- **Declaring an input sampled (`Engine::input_is_sampled`) that is not.**
  A sampled input may change only the engine's behaviour at sample instants
  its `quiet_until` never passes (ADR-005 D3). A model that raises an
  interrupt, sets a flag or arms a timer when an input changes is not
  sampled; the promise audit then fails the run with `PromiseBroken`. A new
  peripheral that wants the exemption needs the D3 argument and a gate like
  `la_adc` (`engines/cpu-renode/tests/lookahead.rs`).
- **A `quiet_until` promise the engine cannot guarantee, or one with side
  effects.** It is a pure read, it may be skipped by the back-off, and a
  sleeping CPU's promise ends at its next clock limit. Never weaken the
  wake audit or turn a `PromiseBroken` into a fallback: a broken promise
  means the outputs may already differ from the one-cell run.
- **Calling a run equivalent from `events.jsonl` alone, or from
  `nets.csv` alone.** Compare `nets.csv`, `nets.vcd` and the non-grant
  records (`grep -v '"type":"grant"'`); grant records differ by design
  between partitions and lookahead settings.
- **Sending more than about 500 000 messages on one External Control
  connection.** Renode 1.17.0's server leaks 16 bytes of its receive
  thread's stack per message received (`stackalloc` inside the
  `RxThreadBody` loop) and dies with `Stack overflow.` after about 524 000
  (8 MiB stack). The Renode engine replaces its connection every 100 000
  messages while Renode is paused (`RenodeConfig::ec_messages_per_connection`,
  `BB_RENODE_EC_MESSAGES`), and `Client` refuses a request past 400 000
  (`protocol::EC_MESSAGE_LIMIT`). A new client of the API (a test, a
  script) that sends many requests must reconnect too. A reconnect must not
  register the GPIO events again: the hooks of the first connection stay
  on the pins and would report every edge twice (ADR-004 Amendment 8).
- **Sending `Clear` to a Renode right after its External Control client
  disconnected.** Renode 1.17.0 restarts the server then, and a `Clear`
  that races the restart leaks the listener or crashes Renode (ADR-004
  Amendment 2). The pool waits for the re-listen first; keep it that way.
- **Parsing KiCad files with regex.** Use a real S-expression parser.
- **A branch-current unknown in series with a milliohm path in an analog
  view.** Under `phase0` (`ABSTOL=1e-13`) ngspice converges every branch
  current to 1e-13 A + 1e-5·|i|. That includes 0 V ammeters, CCVS and CCCS
  sense sources, and V-type B and E sources. Through a few milliohms
  between nodes at about 3 V, that asks for the node voltages to about
  1e-14 V, below double resolution, and the step collapses (ADR-006
  Amendment 2: the TPS22917 view's `RVAR` and `H1` blocks). Write such an
  element as a B conductance, and compute a sensed current from node
  voltages.
- Using float seconds for scheduling. Convert at the engine's FFI boundary only.
- Assuming engines advance in registration order. They advance in the plan
  order the kernel builds from the bindings (producers first, ADR-003 D3);
  registration order only breaks ties and orders the event records.
- Collapsing an engine's output changes to one per grant, or dropping
  intermediate input transitions. Producers report every change in their
  trace; consumers apply every transition, in order (ADR-003 D3).
- Iterating a `HashMap` into any output, or putting `$date`/timestamps/PIDs/
  absolute paths in output files.
- Silently clamping a protocol violation. The scheduler reports `Causality` /
  `NoProgress` errors; fix the engine's `t_safe` instead.

## WSL notes

- The repo lives on the Windows drive (`/mnt/c/source/tyevco/breadboard.dev`).
  Building there is slow, so the Makefile defaults
  `CARGO_TARGET_DIR=$HOME/.cache/bb-target`. Keep that setting.
- From Windows, run `wsl -d Ubuntu-24.04 -- bash -lc 'cd /mnt/c/source/tyevco/breadboard.dev && make test'`.
  Use single quotes so that PowerShell does not expand `$`. Avoid Git Bash for
  WSL calls, because it mangles `$` and `/mnt` paths.
- Use git from WSL (`git -C /mnt/c/...`). `core.autocrlf=false`, and
  `.gitattributes` forces LF. Check with `git ls-files --eol` (expect `i/lf w/lf`).
- A second checkout or `git worktree` must use its own `CARGO_TARGET_DIR`.
  Cargo names workspace-member artifacts by their workspace-relative path,
  so two checkouts sharing `~/.cache/bb-target` overwrite each other's
  crates and a build can silently link the other checkout's code.
- Breadboard's Renode plugins (the lookahead block, `BbStm32Timer`,
  `BbStm32F0Adc` and its host port) are compiled once, together, into one
  assembly cached in `~/.cache/bb/renode-plugins` (`$BB_CACHE_DIR` or
  `$XDG_CACHE_HOME/bb` override it); deleting the directory only costs one
  compile (about 2 s). The loader runs before `LoadPlatformDescription`,
  because the STM32F072 overlay names the plugin's types.
- A fresh Renode plays back a .NET multi-core JIT startup profile cached in
  `~/.cache/bb/renode-jit` (same overrides), one per Renode install,
  platform and plugin setting (ADR-004 Amendment 6). The first run for a key
  gathers it and stops Renode with `SIGTERM` instead of `SIGKILL` so the
  runtime writes it. Deleting the directory only costs that one gather.
  `BB_RENODE_JIT_PROFILE=off` disables it. It changes JIT timing only; a
  new board must still give its committed hashes with and without it.
- The repository lives on WSL's `/mnt/c`, where every open or stat can
  cost 10-130 ms while Windows is busy (ADR-004 Amendment 7). Keep files
  that Renode or a hot loop reads off it: the Renode engine hands Renode a
  copy of the firmware in its temp workdir, and `bb run` writes outputs
  through 1 MiB buffers. Do not reintroduce per-run reads of repository
  files from inside Renode or small unbuffered writes.
- Renode is installed at `~/.local/opt/renode-1.17.0` (symlink
  `~/.local/bin/renode`). `libngspice.so.0` is in `/usr/lib/x86_64-linux-gnu`.

## Definition of done

A change is done when **`make test` is green** and, for anything that affects
waveforms, the **golden-waveform diff is within its stated tolerance**
(Phase 0: `tests/golden/spike/led_a.csv`, within 1 mV / 1 sample). Waveform
diffs decide correctness, not prose. `make clippy` and `make fmt-check` must
also pass, and determinism must hold: running twice gives identical bytes.
