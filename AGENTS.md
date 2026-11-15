# AGENTS.md: working on Breadboard

Breadboard is an open hardware simulator ("QEMU for whole circuit boards"). The
repo is in **Phase 1** (`docs/phase1-plan.md`), with every milestone
(M0-M11) done. M10 (E3, the detent homing) brought the nRF52840's PWM,
TEMP and NVMC models and the PWM engine path (ADR-008 Amendments 9, 11),
the device route (device parts in board files, the `bbdev` crate and its
`DeviceEngine`, the `dms/mechanism@1` device; ADR-006 Amendments 6-7,
approver decisions 2026-11-14), the `ti/drv8837` ideal-switch bridge view,
the Zephyr homing firmware and E3's board files
(`examples/dial-me-spinner/e3`).

- **Phase 0** (done) was a Renode (STM32F103, and STM32F072 for the ADC
  board) ↔ ngspice co-simulation spike under a kernel-owned time-grant
  protocol, with the speed-ups of ADR-003 (the grant lattice, traces, CPU
  lookahead, ADC pushes), ADR-004 (startup and the warm Renode pool) and
  ADR-005 (Breadboard's own STM32F072 ADC model and sampled inputs). Its
  boards (`examples/spike`, `feedback`, `feedback-open`, `adc`) still run
  in `make test`.
- **Phase 1, M0-M8** (done) runs a real board: component packages
  (ADR-006, `packages/`), the KiCad importer, `board.bbm` and `bb.toml`
  projects with `bb open`/`flash`/`run`/`probe` (ADR-007), Breadboard's
  nRF52840 platform with Zephyr firmware and its SAADC model (ADR-008),
  and the worked examples on dial-me-spinner rev A: E1, the LED rail power
  sequencing (M6), and E2, the battery reading and low-battery LED shed
  (M8). The README quickstart, `make demo` and the stranger test are M7.
- **Phase 1, M9** (done): `bb run --gdb`, GDB passthrough behind a proxy
  (`engines/cpu-renode/src/gdb.rs`). D10's gate does not hold for a
  breakpoint stop, so it uses D10's fallback: a debug-only mode whose
  outputs carry a mark (ADR-008 D10, Amendment 7). Only the proxy gets
  Renode's stub; any other client of its internal port is refused
  (Amendment 8).
- The repository is public at https://github.com/tyevco/breadboard.dev,
  with a documentation site (`site/`, VitePress) published to GitHub Pages
  at https://tyevco.github.io/breadboard.dev/.

Read this file first. Then read `spec/time-grant.md` before you touch
anything that advances time.

## Build and test

Run every command inside WSL (Ubuntu 24.04) or Linux, from the repo root.

| Command | What it does |
| --- | --- |
| `make setup` | Install or verify the pinned toolchain (idempotent): Rust 1.99.1, uv, Renode 1.17.0 portable, arm-none-eabi-gcc, ngspice 42 + libngspice0-dev, python3-jsonschema; then build `bb` and install it into `~/.local/bin`. `make setup SETUP=demo` skips the ARM toolchain, uv and jsonschema (what the demo needs: Rust, Renode, ngspice, `bb`). As root (a container) it runs apt without sudo (Phase 1 M3). `make setup SETUP=zephyr` (optional, M6) adds the Zephyr host tools and the pinned Zephyr SDK 1.0.1 for `make fw-rebuild` |
| `make setup-container` | Phase 1 M3 gate (R7): `make setup SETUP=demo` timed in a clean `ubuntu:24.04` container (podman, or docker), as root, from `git archive HEAD` (`SETUP_REV=`, or `SETUP_WORKTREE=1`); timing in `out/setup-container/timing.txt` (`scripts/setup_container_test.sh`) |
| `make demo` | Phase 1 M7 (plan section 5): the README's reference demo in one command, on a copy of `examples/dial-me-spinner` in `~/.cache/bb-demo` (`DEMO_WORK`), every Renode fresh: `bb open`, `bb flash` and `bb run` at the cold bench with the correct and the wrong-order firmware, `check_dms_rail.py` (pass; exactly LSW-3 fails), both runs against `tests/golden/dms-rail/{ok,wrong}-cold`, and `run.json`'s view list (`scripts/demo.sh`, about 15 s; outputs in `out/demo`). Needs only `make setup SETUP=demo` |
| `make stranger-test-container` / `make stranger-test-wsl` | Phase 1 M7 exit measurement (approver decision B measured it from a local clone; the repository has since been published): the README's quickstart block of the commit at HEAD (between the `stranger-test` markers; its `git clone https://github.com/tyevco/breadboard.dev` URL replaced by `STRANGER_URL`, default the local repository, so `STRANGER_URL=https://github.com/tyevco/breadboard.dev` tests the published repository itself) run line by line and timed, as root, in a clean `ubuntu:24.04` container (rootless podman, the repository mounted read-only, cloned from `file:///repo`) or a fresh WSL distro imported from the same image and unregistered afterwards; then the committed hashes and `make demo` (`scripts/stranger_test.sh`; `out/stranger/<mode>/timing.txt`; limit 900 s). The human run: `docs/phase1-exit-checklist.md`. Keep the README block runnable as written |
| `make docs-plots` / `make docs-plots-check` | The docs site's waveform plots, `site/public/plots/<name>.{light,dark}.svg` (committed: the Pages build has no Renode): make demo's two E1 runs, the E2 cases `sag`, `rec`, `s50` and `s50-none` of `make dms-vsense`, the E3 cases `typ` and `step` of `make dms-home` and the spike, every Renode fresh and in parallel, drawn with plain Python (`scripts/docs_plots.sh`, `scripts/docs_plots.py`; outputs in `out/docs-plots`, about 5 min alone, most of it the E3 homing: 305 s in the M10 review, 450 s in M10 beside other gates). The SVGs are a pure function of the runs; `docs-plots-check` regenerates them into `out/docs-plots/svg` and requires the committed files byte for byte (CI job `dms-rail`). After a change to results, run `make docs-plots` and commit the figures |
| `make doctor` | Print every tool's version; exits non-zero if anything is missing |
| `make build` | `cargo build --workspace --all-targets` |
| `make test` | The exit gate: `cargo-test`, `spike`, `determinism`, `golden`, `feedback`, `feedback-order`, `adc`, `adc-order`, `adc-stock`, `adc-model`, `ec-reconnect` (since Phase 1 also `nrf-feedback`, `adc-kicad`, `dms-rail-test`, `packages`, from M8 `nrf-saadc-model` and `dms-vsense-test`, from M9 `gdb-test`, from M11 `dms-system`, and from M10 `nrf-pwm-model`, `device-smoke` and `dms-home-test`; from Phase 2 H0 `measure-lock-test`, from E4 `vscode-templates`, and every step appends its wall time to `out/test-timing.tsv`), run on a warm Renode pool (ADR-004 Part B; `POOL=0` runs it fresh, `POOL_SLOTS` sets the pool size, default 5). `make check` took about 14 minutes in M10, 833 s pooled and 832 s with `POOL=0` beside other gates (ADR-008 Amendment 11; the pool's own gain was measured in Phase 0: 2 min 40 s against 3 min 10 s fresh, ADR-004 Amendment 4) |
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
| `make gdb` / `make gdb-test` | Phase 1 M9 (ADR-008 D10, Amendments 7 and 8): `bb run --gdb <port>` (`--gdb-wait on\|off`) driven by a real GDB in batch mode (`scripts/gdb_gate.py`; GDB: `gdb-multiarch` from `make setup`, else `arm-none-eabi-gdb` or the Zephyr SDK's, `GDB=` overrides) on the spike and E1 (a copy of the project in `~/.cache/bb-gdb-gate`). `gdb-test` (in `make test`, about 45 s): attach and detach without a stop give all four committed hashes of the run without GDB on both boards, a breakpoint hit and continued gives exactly one `warn` debug mark (and run.json `debug: true` on E1), a run without `--gdb` has no debug key, `monitor` is refused, and a client of Renode's internal stub port, first (`BB_GDB_STUB_DELAY_MS`, a test knob) or while the proxy holds it, gets no reply and changes nothing (Amendment 8). `gdb` (CI job `dms-rail`, about 5 min) adds attach mid-run, the scripted breakpoint session 21 times (5 on E1) byte-identical, steps and reads at a breakpoint, a 5 s hold past a 2 s External Control timeout (`BB_RENODE_IO_TIMEOUT`) with no grant boundary during it, Ctrl-C then detach or continue, 1 500 steps across grants and WFI, a second debugger, GDB or bb killed during a halt, the loopback listener, the pool and the internal-port race 5 times without the knob. A marked run is never compared against a reference |
| `make ec-long` | ADR-004 Amendment 8 long one-cell gate (`scripts/ec_long.sh`, about 2.5 min): the ADC board's every-lattice-point reference over 1 500 ms (`EC_LONG_FOR`; Renode used to crash after 81 746 grants) must finish, reconnect at least once, pass `check_adc.py` and equal the lookahead run except grant records (CI job `adc`) |
| `make nrf-gates` | Phase 1 M2 gate (ADR-008 D1-D4, D6; Amendment 2): the Renode engine tests `engines/cpu-renode/tests/nrf52840.rs` on Breadboard's nRF52840 (`Platform::Nrf52840Bb`, `nrf52840-bb@1`: no SVD download, 64 MIPS) with the bare-metal gate firmware in `engines/cpu-renode/tests/firmware/nrf52840/` (nrf-blinky edges identical at one-cell grants, 10 ms grants, plugin armed and lookahead on, self-test status exactly 0x3; nrf-ppi; nrf-busywait: a 10 ms nrfx busy-wait takes 6.66 ms of virtual time; the D6 TIMER-compare-then-long-sleep promise clamp; the PIN_CNF audit, mid-run and end-of-run) and the prebuilt Zephyr gate image (nrf-zephyr), then `make nrf-feedback` (about 2 min) |
| `make nrf-saadc-model` | Phase 1 M8, first half (ADR-008 D7, D8, Amendment 4; in `make test`, about 25 s): Breadboard's nRF52840 SAADC (`BbNrf52Saadc` on `nrf52840-bb@2`). The Renode-only register and timing tests in ADR-005's test-plan shape (`examples/nrf-saadc/bb_saadc_test.py`, `nrf_saadc_idle.elf` / `nrf_saadc_burst.elf`: reset values, every TACQ x RESOLUTION x OVERSAMPLE x BURST instant, the 8x BURST, transfer and rounding, VDD, EasyDMA, LIMIT, timer mode, the IRQ, the TACQ < 2 L refusal, the UART tap; run twice, byte-identical), then `scripts/nrf_saadc_board.sh` on `examples/nrf-saadc` (the loading `adc_in`: 8 `#smp` toggles and exactly 8 charge transfers per 8x burst against the analytic charge sharing, codes, UART lines; lookahead at 1 ms and 10 ms, push-on-change and `--engine-order analog-first` equal to the every-lattice-point one-cell reference `tests/golden/nrf-saadc/outputs.sha256`; the refusal at `--renode-quantum 25us` and its message; the `load=none` escape). `make nrf-saadc-update` rewrites the hashes; `make cost-table-nrf-saadc` is the board's cost-table row (also in `make cost-table`; idle host) |
| `make nrf-pwm-model` | Phase 1 M10, first step (ADR-008 D9, Amendment 9; in `make test`, about 25 s): Breadboard's nRF52840 PWM0-PWM3 (`BbNrf52Pwm`), the TEMP and NVMC stand-ins (`BbNrf52Temp`, `BbNrf52Nvmc`) and their host port (`BbNrf52HostPort` at 0x5FFC0000: the PWM pin ring, TEMP's value) on `nrf52840-bb@3`. `examples/nrf-pwm/bb_pwm_test.py`, Renode-only, each group twice and byte-identical: `registers` (`nrf_saadc_idle.elf`; reset values; every pin change exact to 0.5 ns for up and up-and-down x PRESCALER x COUNTERTOP x COMP x POLARITY; REFRESH, ENDDELAY, LOOP, the SHORTS, NextStep, the Grouped, Common and WaveForm decoders, PSEL and ENABLE while running, every event instant, the IRQs, the ring overflow; TEMP's DATARDY at 36 us and STOP; NVMC erase timing, CONFIG gating, partial erase, ERASEALL) and `firmware` (`nrf_pwm_step.elf`: TEMP, an NVMC erase polled on READY, 4 full steps at 446 PPS as `nrfx_pwm_complex_playback` runs them, every pin change against the firmware's duty table, the UART lines, and no lookahead promise past the next PWM edge). Then the engine path (M10, third step; ADR-008 Amendment 11): `examples/nrf-pwm/board.cir` (cyclic through a constant `digital_in`) runs the same firmware through `bb run`, fresh: `scripts/check_nrf_pwm.py` requires the four pins' `pin_edge` records to be the duty table's changes to the femtosecond, and the one-cell reference (`--lookahead off`), the default (lookahead on), `--max-quantum 1ms` and analog-first give identical `nets.csv`, `nets.vcd` and non-grant records |
| `make dms-home` / `make dms-home-test` | Phase 1 M10 gate (plan section 2 E3; ADR-008 Amendments 9-12, ADR-006 Amendments 6-7): E3, the detent homing H0-H5 on the hand-written board files of `examples/dial-me-spinner/e3` (written by `scripts/dms_home_boards.py`; the detent sheet, the nRF52840, the device `M1`, and the suite's power path as the bench) with `firmware/prebuilt/homing.elf` and `homing_step.elf` (`scripts/dms_home.sh`, `scripts/check_dms_home.py`). Cases: `step` (one full step at 446 PPS through the bridges and coils, against the suite's `test_firmware_microstep_pwm[VBAT3.7-25C]`, tolerances 2.5 % / 1 % per unit duty), `typ` (tS 50 ms), `worst` (the suite's DRV5032FA corner: tS 75 ms, BOP 4.8 mT, BRP 0.5 mT, at its worst sample phase), `park` (H1, BOP/BRP 4.8/0.5 mT). `dms-home` (about 55 min on `DMS_HOME_JOBS` 5; a homing run is 270 000 one-cell grants, about 4.5 min alone and 6-7 min with five in parallel, 1.5 GB): every case and its one-cell reference (`--lookahead off`), the checks (H0-H5, step counts, every phase's step rate from the PWM edges, no park-stop contact before H4, H3 ending 20-108 steps from the stop, the park stall, the HOME_N latency of Amendment 10 as a worst-phase bound: the device's tS must be the FA's, tS + tACTIVE and the run's latency within 75 ms typical, 97.4 ms worst), the references against `tests/golden/dms-home`, analog-first and `--max-quantum 1ms` (typ, step; on E3 the defaults, the reference and 1 ms are one partition, so that is a consistency check, Amendment 12), 10-run determinism (typ), and three negative controls that must fail exactly their checks (`homing_fast.elf`: step rate, stop contact, H3; M1 at tS 200 and 286 ms, the DRV5032FB: the latency). `dms-home-test` (in `make test`, about 40 s): the boards are the generator's output, `step` with its checks, hashes and analog-first. `make dms-home-update` rewrites the goldens; `make cost-table-dms-home` is its cost-table row (idle host) |
| `make device-smoke` | Phase 1 M10, second step (ADR-006 D7, Amendment 6; in `make test`, about 20 s): `tests/device-smoke/board.cir`, examples/feedback's STM32F103 and firmware with the `dms/mechanism@1` device (`packages/dms/detent-mechanism`) in a loop with the CPU through the direct digital routes of `spec/package.md` section 12 (U1.PA5 -> M1.NSLEEP, M1.IN_A1; M1.HOME_N -> U1.PB0). Four fresh runs (`--renode-pool off`: the board is not in `pool-equiv`): the one-cell reference (`--lookahead off`), the default, `--engine-order analog-first` and the default again; `scripts/check_device_smoke.py` on each (HOME_N rises at exactly 57.04 ms, PA6 follows within one cell); `nets.csv`, `nets.vcd` and the non-grant records identical across all four |
| `make nrf-feedback` | ADR-008 D3 board-level gate (in `make test`): `examples/nrf-feedback` (examples/feedback's circuit on the nRF52840: RTC0 drives `U1.P0.13`, GPIOTE IN on `U1.P0.11` copies to `U1.P0.15`) with lookahead on at 1 ms and 10 ms byte-identical to its one-cell `--lookahead off` reference (`tests/golden/nrf-feedback/outputs.sha256`; `make nrf-feedback-ref`), no promise broken, `check_feedback.py --nrf`, and `--engine-order analog-first` byte-identical |
| `make nrf-costs` | ADR-008 D1: fresh startup and per-grant cost of the nRF52840 blinky at 64 MIPS against the STM32F103 blinky (interleaved, N = `BB_NRF_COST_N`, medians); reports only. Run it on an idle host |
| `make lookahead-startup` | ADR-003 step 5 startup-cost gate: Renode engine startup with the cached plugin within 0.3 s of a start without it (interleaved, N = 7, medians; run on an idle host) |
| `make startup-perf` | ADR-004 G1: fresh `bb run` engine startup of the spike and the feedback board at least 12 % and 0.3 s below `BB_RENODE_STOCK_RUNTIME=1` (the stock .NET settings, no JIT profile and the 100 ms connect poll), interleaved, N = 7 (`STARTUP_N`), medians, outputs identical; `STARTUP_BASE=<bb>` compares against another binary. Run it on an idle host |
| `make jit-profile-stress` | ADR-004 Amendment 9: the startup JIT profile's gather under load (`scripts/jit_profile_stress.sh`): `JIT_STRESS_ROUNDS` (4) rounds of `JIT_STRESS_PAR` (8) parallel gathering spike runs, each with an empty `BB_CACHE_DIR`, while `JIT_STRESS_LOAD` busy loops (default 2 x nproc) load every CPU; every run must leave exactly one profile (about 3 min). Not in `make test`: it loads the host on purpose |
| `make pool-equiv` | ADR-004 G4: one pool process serves, twice each in alternating order (`POOL_EQUIV_ROUNDS`), the spike (default, 1 ms, 500 ms), feedback (on, off, plugin never, polling firmware), adc (on, off), adc-open, adc-stock, adc-open-stock, adc-kicad (Phase 1 M4), nrf-feedback (on, off, plugin never; M2), E1 (`dms-rail` ok-cold default and one-cell, wrong-nominal; M6), nrf-saadc (default and its every-lattice-point reference; M8), E2 (`dms-vsense` s50 default and reference; M8) and E3 (`dms-home-step`, `dms-home` typ; M10); every pooled run must be leased and byte-identical to its fresh run and to the committed hashes (`scripts/pool_equiv.sh`) |
| `make pool-perf` | ADR-004 G2: on a 1-slot pool, the first lease starts in at most 0.5 s, median engine startup is at most 0.30 s for spike, feedback and adc (N = 7), and spike total wall at most 1.2 s (`scripts/pool_perf.sh`). Run it on an idle host |
| `bb pool run -- <cmd>` | Run `<cmd>` with a warm Renode pool in `BB_RENODE_POOL` (`--slots N`, `--max-runs K`); `bb pool start` / `stop` / `status` for an interactive shell (`bb pool --help`). `bb run --renode-pool off` ignores a pool |
| `make golden-update` | Regenerate the golden snapshots, the spike output hashes and the partition hashes (spike, feedback, feedback-open), the feedback one-cell reference hashes and the ADC reference hashes from fresh, checked runs. Review the diff before committing it: a change to an equivalence reference is a change to results and needs a reason (and, for the protocol, an ADR) |
| `make cost-table` | The README cost table with the measurement protocol (`scripts/perf_ab.sh`, interleaved, N = `PERF_N`, medians): the spike at 10 µs vs 10 ms grants, `examples/feedback` and `examples/adc` with lookahead off vs on, `examples/feedback-open` at 10 µs vs 10 ms. Reports only; no gate. Run it on an otherwise idle host |
| `make dms-source` | Phase 1 M0: the vendored dial-me-spinner KiCad inputs (`examples/dial-me-spinner/kicad/`) against the sha256s in `examples/dial-me-spinner/SOURCE`, and against `git show <commit>:<path>` when `DMS_REPO` (default `../dial-me-spinner`) exists, and that every package file migrated from dial-me-spinner (a `part.toml` `origin`, the WS2812C view) has an entry (`scripts/dms_source_check.sh`) |
| `make dms-suite-check` | Phase 1 M0 gate: re-run the dial-me-spinner SPICE suite (`-m "not slow"`) at the reference commit `6854d25` from a `git archive` export in `~/.cache/bb-dms-suite` (`scripts/dms_suite.sh`, about 4-5 min) and require `out/dms-suite/results.json` to equal the committed `examples/dial-me-spinner/suite/results.json` byte for byte (checks, test outcomes, every deck's numeric fallback level, ngspice version). `make dms-suite-update` regenerates it. Never check out, stash or write in the dial-me-spinner repository |
| `make dms-s1` | Phase 1 plan S1 (ADR-006 Amendment 2): the E1 deck (two LED rail on/off cycles, 120 ms, both benches) through `NgspiceEngine` at `phase0`, with the S1 fix of the TPS22917 view. The view and every deck are built in `~/.cache/bb-dms-s1` from a `git archive` export of `6854d25`; they are TI-derived, so they stay local-only and never enter the repository (`scripts/dms_s1.sh`, `scripts/dms_s1.py`, `engines/analog-ngspice/examples/deck_run.rs`). Its 18 gates: 120 ms at both benches; grants of 10 µs, 1 ms and 10 ms byte-identical; VLED at the check instants and the LSW-2 time against the suite's `suite_fallback` run; VLED at every 10 µs against the converged run; the fix's result neutrality. About 1.5 min on 8 jobs (`DMS_S1_JOBS`). `make dms-s1-cost` is the S1 cost table (plan §8; `DMS_S1_N` interleaved rounds, medians; run it on an idle host) |
| `make packages` | ADR-006 (Phase 1 M3; in `make test`): every `part.toml`, `pins.toml`, test vector, `bb.lock`, project `bb.toml`, bench and `board.bbm` against `spec/schema/*.schema.json` (`scripts/validate_packages.py`, python3-jsonschema), `bb pkg lint` clean, `bb pkg test` (every vector through `NgspiceEngine` at one cell and 1 ms, identical, and at every recorded qualification; the first-party packages and `examples/adc/kicad/packages`), and every example's `bb.lock` and every example project's `board.bbm` current (`bb open --check`); `scripts/check_local_only.sh` (no `LicenseRef-local-only` package, nothing under `local-packages/` and no TI-derived model file is tracked; outside a git checkout it checks every file of the tree), and its regression test `scripts/check_local_only_test.sh` |
| `make dms-vectors` / `make dms-vectors-check` | Phase 1 M5 (ADR-006 Amendment 3): regenerate the test vectors of `packages/ti/tps22917` and `packages/worldsemi/ws2812c-2020` from the dial-me-spinner suite's own deck builders at `6854d25` (`scripts/dms_packages.sh`, `scripts/dms_packages.py`; `git archive` export, suite venv), or require the committed ones byte for byte and `nordic/nrf52840-qiaa`'s GPIO sets to equal the suite's resistances (needs `DMS_REPO`) |
| `make dms-local-packages` | Phase 1 M5: build the LOCAL-ONLY vendor-derived `ti/tps22917` (`LicenseRef-local-only`: the suite's TI-derived view with the S1 fix and the `GND` -> `GNDP` rename) in the git-ignored `local-packages/` (`DMS_LOCAL_PACKAGES`), qualify it with `bb pkg test --record`, and run gate G8 (the rename gives byte-identical `bb pkg test --report` lines). Never commit `local-packages/` |
| `make dms-m5b` | Phase 1 M5b gate (after `dms-local-packages`): the committed datasheet-fitted TPS22917 view against the local vendor-derived one on the E1 checks through `NgspiceEngine` at `phase0`, both benches (VLED at each P0.06 rise and the LSW-3 maximum within 10 mV, LSW-2 within 1 ms, the wrong-order excursion within 10 mV; `scripts/dms_m5b.py`, `out/dms-m5b/summary.json`, about 40 s) |
| `make locks` | `bb open --update-lock` on every example board and project: after an intended change to a package, a platform `.repl` or **any Renode plugin source** (they are in every Renode platform's content hash, ADR-006 D8); review and commit the `bb.lock` and `board.bbm` diffs |
| `make adc-kicad` | Phase 1 M4 gate M4a (ADR-007 Amendment 1; in `make test`): `examples/adc/kicad`, the ADC board drawn in KiCad, imported and run as a `bb.toml` project (a copy in `~/.cache/bb-adc-kicad`, firmware recorded with `bb flash`; `scripts/adc_kicad.sh`): `board.bbm` current; equal to its one-cell reference and to `tests/golden/adc-kicad/outputs.sha256`; the `.net` and `kicad-cli` routes byte-identical (when `kicad-cli` is installed); board.cir's committed non-grant records byte for byte and `nets.csv`/`nets.vcd` within 1 mV (2 samples differ by 1 µV: C1's `470n` against `4.7e-7`, which the script checks byte for byte); `bb run` writes nothing into the project. `make adc-kicad-prepare` makes the copy that `pool-equiv` and `cost-table` run |
| `make kicad-import` | Phase 1 M4 gates M4b-M4f on the dial-me-spinner main board at 6854d25 (`scripts/kicad_import.sh`, about 20 s; needs KiCad 10 in WSL and the Windows `kicad-cli.exe` unless `KICAD_WINDOWS=0`): 201 parts, 142 nets, 657 pin nodes; the resolution counts; one `board.bbm` from the committed netlist, fresh Linux and Windows exports and a partial KiCad 10 re-save (dc96515's KiCad 9 netlist as the secondary check); byte-identical re-imports; unchanged sources; the version refusals. Not in `make test` (no KiCad in CI); `cli/tests/kicad_import.rs` (the two-board fixture) is |
| `make dms-system` | Phase 1 M11 gate (ADR-007 D5, D7, Amendment 5; in `make test`, about 30 s): `examples/dial-me-spinner/system` (`scripts/dms_system.sh`, copies in `~/.cache/bb-dms-system`): a fresh import gives the committed `board.bbm`, twice byte-identical; main 201/142/657 and base 21 parts, 17 nets, 81 pin nodes; the 15 shared references; each board its own ground; the 12 pins mated through `dial-me-spinner/link-harness@0.1.0:wires`, the harness locked; the `kicad =` route on both boards (skipped and reported without `kicad-cli`); dc96515's base netlist (reported: J1's S1/SH); E1's targets, bindings and benches on the system elaborate to E1's own deck with qualified names, the base side of J1 warned as open. `cli/tests/system.rs` (cargo test): a bare U1, U2, J1 or TP1 is an error listing both candidates in every place a name is taken; the connection and harness errors |
| `make dms-rail` | Phase 1 M6 gate (plan section 2 E1; ADR-008 Amendment 3, ADR-007 Amendment 3): `examples/dial-me-spinner` with the prebuilt Zephyr firmware (`led_rail.elf`, `led_rail_wrong_order.elf`) at `bench/nominal.toml` and `bench/cold.toml`, each run on a copy of the project in `~/.cache/bb-dms-rail` (`scripts/dms_rail.sh gate`, about 3 min fresh): `scripts/check_dms_rail.py` (the correct order passes LED-DATA-V4, LSW-3, LSW-2 and FW-6.4, the firmware rule's timing from the `pin_edge` records; the wrong order fails exactly LSW-3), every case equal to its one-cell reference (`--max-quantum 10us`) except grants and to `tests/golden/dms-rail`, analog-first byte-identical, 120 ms in under 30 s at both engine orders (board-level convergence), the `PIN_CNF` audit clean and its two negative controls, 10-run determinism. `make dms-rail-test` (in `make test`) is the four default runs with the checks, hashes, VLED snapshots, audit and one analog-first run, and two FW-6.4 negative controls (ok-cold's `pin_edge` records moved: each fails exactly FW-6.4); `make dms-rail-update` rewrites the goldens |
| `make dms-crosscheck` | Phase 1 M6, local only (needs `DMS_REPO` and `make dms-local-packages`): the run's `pin_edge` records replayed into the suite's own `led_rail_sequence` deck with the same bench supply (`scripts/dms_crosscheck.sh`, `scripts/dms_suite_deck.py`); VLED within 5 mV of the co-simulation with the LOCAL-ONLY vendor-derived view, both benches; the fitted view and the suite's battery-network deck reported. Decks stay in `~/.cache/bb-dms-crosscheck` |
| `make fw-rebuild` / `make dms-firmware` | ADR-008 D5 (M6, M8, M10): rebuild the E1, E2 and E3 images (with E3's negative control `homing_fast.elf`) in a FRESH west workspace from `examples/dial-me-spinner/firmware/west.yml` (Zephyr v4.5.0, SDK 1.0.1 in `~/.local/opt`) and require the image hashes of `firmware/prebuilt/SHA256SUMS`; `dms-firmware` rebuilds the prebuilt images (`scripts/dms_zephyr.sh`; `make setup SETUP=zephyr` installs the SDK and host tools). Never uses a developer's workspace |
| `make cost-table-dms-rail` | The M6 cost-table row alone (E1 cold, one-cell vs defaults, fresh and on a 1-slot pool); `make cost-table` includes it. Run on an idle host |
| `make dms-vsense` | Phase 1 M8 gate, second half (plan section 2 E2; ADR-008 Amendment 5): `examples/dial-me-spinner` with `vsense.elf` (a reading every 1 s) and `vsense_50ms.elf` reading VSYS_SENSE through the SAADC with the loading `adc_in`, at `bench/vsense.toml` (the suite's sampling worst case) and `bench/sag.toml` (the battery ramps down through the LED shed), each run on a copy of the project in `~/.cache/bb-dms-vsense` (`scripts/dms_vsense.sh gate`, about 14 min fresh): seven cases (s1, s50, c5 with C5 at 10 nF, their `load = "none"` twins, sag) at bb's defaults and as their every-lattice-point references (`--lookahead off --adc-push every`); `scripts/check_dms_vsense.py` (the per-sample error against the `load = "none"` run and the suite's charge-sharing arithmetic, which must also reproduce the suite's 356/761 uV and 3.47 mV at its corner; codes; C5 / 10; the shed within one burst period of the threshold crossing; no `stop:`); defaults equal to the references except grants, the references equal to `tests/golden/dms-vsense`; analog-first byte-identical; the TACQ refusal; 10-run determinism (sag). `make dms-vsense-test` (in `make test`, about 1 min) is s50, c5 and their twins with the checks and hashes; `make dms-vsense-update` rewrites the goldens; `make cost-table-dms-vsense` is its cost-table row (idle host) |
| `make dms-values` | ADR-006 G3: `bb pkg value --kicad` on the 29 distinct passive values of the dial-me-spinner main board equals the suite's `spicelib.parse_value` (re-derived from `git show 6854d25:hardware/sim/spicelib.py` when `DMS_REPO` exists, else the committed fixture `tests/fixtures/values/dms-main.json`) |
| `make fmt` / `make fmt-check` | `cargo fmt --all` (`-- --check` in CI) |
| `make clippy` | `cargo clippy --workspace --all-targets -- -D warnings` |
| `make check` | `fmt-check`, `clippy` and `test`, which is what CI runs |
| `make worktree TRACK=<name>` | Phase 2 H0 (plan section 7 rule 1): the track's git worktree `~/src/bb-<name>` (`WORKTREE_ROOT`) on the branch `p2/<name>`, from `main` when the branch is new, locked against a `git worktree prune` from Windows, with its own git-ignored `local.mk` (included first by the Makefile): `CARGO_TARGET_DIR` `~/.cache/bb-target-wt-<name>`, `BB_SCRATCH` `~/.cache/bb-wt-<name>` (the gates' project copies and work directories) and `DMS_REPO`. Idempotent; `make worktree-remove TRACK=<name>` removes it and keeps the branch (`scripts/worktree.sh`) |
| `make worktree-test` | H0 gate (not in `make test`; four release builds of `bb`): two throwaway worktrees build `bb` at the same time, and their crate hashes are compared (bbcore's rlib differs by a marker only A has; bbpkg's rlib and each `bb` carry their own package path, never the other's); negative control: built one after the other into one shared target directory, B's `make bb` compiles nothing and leaves B a binary with A's code and package path (cargo names sources by their workspace-relative path) (`scripts/worktree_test.sh`) |
| `make measure-lock-test` | H0 gate (in `make test`, a few seconds): on a private lock file, every target of `MEASURE_TARGETS` refuses while a probe holds the measurement lock (the hold checked from outside), before building anything; the probe runs before and after (`scripts/measure_lock_test.sh`) |
| `make baseline` | H0: the runtime baseline. `make check` (`BASELINE_ARGS`) `BASELINE_N` = 3 times after one uncounted warm-up, on an idle ext4 worktree, under the measurement lock; wall times, load before each run and `make test`'s per-step times (`out/test-timing.tsv`, written by every `make test`) in `out/baseline` (`scripts/baseline.sh`). Its result is the table in "Gate budgets" |
| `make vcd-check` / `make vcd-check-fixture` | Phase 2 E4 gate G1 (ADR-016 D1, Amendment 1; CI job `dms-rail`, 16 s with the demo): `tools/vcd-check` (wellen 0.25.7, Surfer's waveform library, outside the workspace) loads `make demo`'s two `nets.vcd` and the committed fixture `tests/fixtures/vcd/mixed.vcd`: timescale 1 fs, every `$var` present with its type (`real` as Real, `wire 1` as a 1-bit Wire), every change, the `$comment` name map, and every real net equal to the run's `nets.csv` at every row; then six negative controls on edited copies of the fixture each fail with their own message (`scripts/vcd_check.sh`). `vcd-check-fixture` skips the demo. The docs page is `site/guide/viewing.md` |
| `make vscode-templates` / `make vscode-test` | Phase 2 E4 gate G2 (ADR-016 D2, Amendment 1). `vscode-templates` (in `make test`, 0.1 s): `docs/templates/vscode/{tasks,launch}.json` statically against D2 (attach, `servertype: external`, `overrideAttachCommands: []`, no `svdFile`, the port, the `endsPattern` against bb's listening line in `cli/src/main.rs`, a `beginsPattern` that does not swallow it). `vscode-test` (local only: downloads a pinned VS Code 1.145.3 and cortex-debug 1.12.1 with its four dependencies from Open VSX once into `~/.cache/bb/vscode-test`, sha256-checked; needs `xvfb` and unprivileged user namespaces; about 2 min): the templates copied verbatim into a copy of the dial-me-spinner project, run in a headless VS Code in a network namespace with only loopback; attach, reads and stop give all four `tests/golden/dms-rail/ok-cold` hashes, and the negative control (a breakpoint through cortex-debug) marks the run and changes its outputs (`scripts/vscode_test.sh`) |

`make firmware` builds the firmware ELFs (spike, feedback, ADC and the Renode
lookahead gate firmware in `engines/cpu-renode/tests/firmware`, and the
nRF52840 gate firmware in its `nrf52840/` subdirectory, built against the
vendored nrfx MDK and CMSIS-Core headers in `nrf52840/vendor/`) on their own.
The nRF52840's Zephyr gate image is committed prebuilt
(`nrf52840/zephyr/prebuilt/`, stripped of debug information);
`scripts/nrf_zephyr_gate.sh` rebuilds it where a Zephyr workspace exists and
checks the loadable image is unchanged.

## Gate budgets

Phase 2 plan section 7 rule 2 and ADR-015 D11. There are two tiers:

- The **PR tier** is `make check` and the CI jobs on a pull request.
  - `make check` on an idle ext4 worktree: at most **the baseline below
    + 4 minutes**.
  - The slowest CI PR job: at most **15 minutes**.
- The **nightly tier** (H6) takes, by name, the `fresh` job (`make test
  POOL=0`), `make ec-long`, `make dms-home`, the full `make dms-vsense`,
  and any later gate over its budget.

How the budgets are kept:

- A milestone records its measured `make test` addition and its full
  gate's wall time in the milestone table below, in the commit that adds
  the gate.
- A gate over its budget moves to the nightly tier; the target does not
  move.
- An overrun of more than 25 % is a review finding.
- Savings (H6) are measured against this baseline with `make baseline`,
  under the measurement lock.

**Runtime baseline (H0).** `make baseline` measures `make check` N = 3
times on an idle ext4 worktree, after one uncounted warm-up. The local
table below is filled from that run. The CI numbers are the wall times
of each job in the last three CI runs on `main`. They come from GitHub
Actions' job records (read-only), on three consecutive commits: fd63d3a
(Phase 1 M10, run 43683405952), f1f0abb (run 43611129210) and faaf816
(run 42905856531). They are not three runs of one commit, because
re-running CI would act on GitHub. faaf816 predates the GDB step of
`dms-rail`.

The local baseline was measured on 2026-11-15 at 83e48af, in
`~/src/bb-baseline` (ext4, made by `make worktree`), on 24 CPUs and
14.5 GiB under WSL2 kernel 6.18.58.1. The pool was on (the default), and
nothing else ran. The 1-minute load before each run was 1.2-1.7; that is
the previous run's tail.

| Local (idle ext4 worktree) | Median | Runs |
| --- | --- | --- |
| **`make check`** (the PR-tier reference) | **470 s (7:50)** | 466, 470, 474 s (warm-up from a cold target directory: 519 s) |
| `make test POOL=0` (the `fresh` tier, for H6) | 554 s (9:14) | N = 1 |

So the PR-tier target of plan section 5 item 5 is `make check` ≤ 710 s
(11:50) on this host. `make test`'s steps (medians of the three runs; the
sum is 461 s, and fmt-check, clippy and the up-to-date builds add about
10 s):

| Step | s | Step | s | Step | s |
| --- | --- | --- | --- | --- | --- |
| `cargo-test` | 152.4 | `nrf-pwm-model` | 43.0 | `ec-reconnect` | 37.0 |
| `gdb-test` | 36.8 | `dms-vsense-test` | 36.1 | `adc-model` | 32.3 |
| `adc-kicad` | 26.7 | `dms-home-test` | 18.3 | `nrf-feedback` | 17.3 |
| `device-smoke` | 13.5 | `nrf-saadc-model` | 13.2 | `dms-rail-test` | 9.2 |
| `packages` | 7.2 | `adc-stock` | 5.5 | `dms-system` | 4.1 |
| `adc-order` | 2.1 | `determinism` | 1.7 | `adc` | 1.2 |
| `feedback-order` | 1.2 | `measure-lock-test` | 0.7 | `feedback` | 0.7 |
| `spike` | 0.5 | `golden` | 0.1 | | |

| CI job (ubuntu-24.04) | Median | Runs (fd63d3a, f1f0abb, faaf816) | Its `make setup` step (median) |
| --- | --- | --- | --- |
| `fresh` (`make test POOL=0`) | **11:30** | 13:58, 10:31, 11:30 | 0:53 |
| `test` (setup, fmt, clippy, packages, `make test`) | 8:51 | 8:51, 6:51, 9:25 | 0:50 |
| `dms-rail` (E1 gate, GDB gate, docs plots) | 7:39 | 7:48, 7:39, 5:21 | 0:49 |
| `adc` | 3:58 | 3:58, 3:29, 3:59 | 0:53 |
| `feedback` | 3:08 | 3:08, 3:12, 2:35 | 0:53 |
| `fw-rebuild` | 2:53 | 3:05, 2:53, 2:39 | 1:12 (with the SDK) |
| `partition` | 1:37 | 1:37, 1:37, 1:44 | 0:56 |
| `verify-every` | 1:14 | 1:22, 1:14, 1:06 | 0:52 |

The jobs run in parallel, so a CI run's wall time is its slowest job:
`fresh`, which H6 moves to the nightly tier. H0 caches the Renode tarball
and the apt packages (`.github/actions/bb-setup`). Checked locally in a
clean container: the second `make setup SETUP=demo` downloaded no `.deb`
and not the Renode tarball. Its saving on CI is measured in H6, because
CI only runs on a push.

**Milestone budgets** (plan section 4). The budget is the most a
milestone may add to `make test` (wall time, on the baseline host). The
"measured" columns are filled by the milestone's commit.

| Milestone | `make test` budget | Measured `make test` addition | Full gate (measured) |
| --- | --- | --- | --- |
| H0 | — (`measure-lock-test`, a few seconds) | 0.7 s (`measure-lock-test`) | `make worktree-test` 76 s; `make baseline` 32 min |
| H1 | +30 s | | |
| H2 | — | | |
| H3 | +10 s | | |
| H4 | — | | |
| H5 | +30 s (`multi-producer`) | | |
| H6 | — (removes: `fresh` to nightly, `adc-stock`, split `cargo-test`) | | |
| S | — | | |
| B1 | +40 s | | |
| B2 | +20 s | | |
| B3 | +0 s (replaces scripts) | | |
| R1 | +15 s | | |
| R2 | +30 s | | |
| P0 | — (spike) | | |
| P1 | +30 s | | |
| P2 | +15 s | | |
| P3 | +30 s | | |
| W-fw | — | | |
| P4 | +40 s | | |
| P5 | +40 s | | |
| E1 | +20 s (local registry fixture) | | |
| E4 | — (`vscode-templates`, under 1 s) | 0.1 s (`vscode-templates`) | `make vcd-check` 16 s with the demo (CI `dms-rail`); `make vscode-test` 116 s, local (ADR-016 Amendment 1) |

This build adds at most 350 s to `make test` (plan section 8, without
R3's 10 s; B4, R3, P6, E2, E3 and E5 are not in this build). Section 5
item 5's target allows the baseline + 4 minutes (240 s), so the build
needs at least 110 s of H6's named savings.

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
                        ws2812c-2020, nordic/nrf52840-qiaa, raytac/mdbt50q-u1mv2 (a module);
                        Phase 1 M10: ti/drv8837 (the ideal-switch bridge view), dms/detent-mechanism
                        (a device package: `[behavioral] engine = "device"`, `dms/mechanism@1`)
local-packages/         git-ignored, LOCAL-ONLY packages (make dms-local-packages: the vendor-derived
                        ti/tps22917); never committed
bbdev/                  the Phase 1 device API (ADR-006 D7, Amendment 6): `Device`, `Ctx`, the seeded
                        `Rng`, `DeviceEngine` (engine.rs; t_safe MAX, quiet_until = the earliest
                        timer, advance_until), the device table `DEVICES` and dms_mechanism.rs
                        (the detent mechanism and DRV5032, `dms/mechanism@1`); tests/scheduler.rs (G6)
engines/analog-ngspice/ ngspice shared-lib engine (libngspice.so.0)
                        examples/deck_run.rs: a SPICE body through NgspiceEngine with digital-glue edges (S1)
engines/cpu-renode/     Renode engine over the External Control API; platforms/: the plugin
                        assembly's sources (bb_lookahead.cs, the lookahead block, ADR-003 D5;
                        bb_stm32_timer.cs, STM32_Timer + TRGO; bb_stm32f0_adc.cs, the STM32F0
                        ADC model and its host port, ADR-005) and stm32f072-bb.repl (the F072
                        overlay); tests/firmware (gate firmware); console.rs, pool.rs,
                        manager.rs (the warm Renode pool, ADR-004); gdb.rs (the GDB proxy of
                        `bb run --gdb` in front of Renode's stub, ADR-008 D10, Amendment 7)
cli/                    `bb` binary (`bb run`, `bb pool`, `bb open`, `bb pkg`, `bb flash`, `bb probe`,
                        board-file parser, platforms.rs: the Renode platform identities of ADR-006 D8;
                        Phase 1 M4: project.rs (bb.toml, benches), names.rs, import.rs (resolution, D4),
                        elab.rs (bindings, analog scope, the deck, run.json);
                        Phase 1 M10: devices.rs (device parts' direct digital routes, the engine graph))
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
                        bb.toml with its board.bbm and bb.lock (M4); M6: the E1 [[bind]]s and
                        target, bench/ (nominal, cold), firmware/ (west.yml, the out-of-tree
                        Zephyr board, the led_rail app, prebuilt/ ELFs with SHA256SUMS of their
                        image hashes and PROVENANCE); M8: the E2 [[bind]]s (VSYS_SENSE on AIN0,
                        VDD), bench/vsense.toml, bench/sag.toml and bench/sag-recover.toml,
                        firmware/vsense (vsense.elf,
                        vsense_50ms.elf); M10: firmware/homing (homing.elf, homing_step.elf;
                        dts/bindings/dial,detent-stepper.yaml)
examples/dial-me-spinner/e3/ Phase 1 M10 (E3, route A): board.cir (typ), board-ts75.cir (worst),
                        board-park.cir (H1), board-step.cir (the whole detent sheet: the bridge
                        view copied verbatim, the coils, the suite's back-EMF), one bb.lock; all
                        written by scripts/dms_home_boards.py (make dms-home-test checks them)
examples/dial-me-spinner/system/ Phase 1 M11 (ADR-007 Amendment 5): main + base as one project
                        (both vendored netlists, qualified names), [[connection]] main:J1 - base:J3
                        through the harness view of packages/dial-me-spinner/link-harness (a
                        project-local package); board.bbm, bb.lock (import only: simulate = main)
examples/nrf-feedback/  examples/feedback's circuit on the nRF52840 (Phase 1 M2; firmware
                        engines/cpu-renode/tests/firmware/nrf52840/nrf_feedback.c)
examples/nrf-saadc/     Phase 1 M8: the loading adc_in board (AIN0 through the package's
                        saadc_se view, VDD, UARTE0 lines; firmware nrf_saadc_burst.c) and
                        bb_saadc_test.py (the SAADC model's Renode-only tests)
engines/cpu-renode/platforms/bb_nrf52_saadc.cs  BbNrf52Saadc, BbSaadcHostPort (0x5FFF0100),
                        BbUartLog (0x5FFF8000; ADR-008 D7, D8), loaded by nrf52840-bb.repl (@2)
engines/cpu-renode/platforms/bb_nrf52_pwm.cs  BbNrf52Pwm (PWM0-3), BbNrf52Temp, BbNrf52Nvmc and
                        BbNrf52HostPort (0x5FFC0000: the PWM pin ring; ADR-008 D9, Amendment 9),
                        loaded by nrf52840-bb.repl (@3)
examples/nrf-pwm/       bb_pwm_test.py (the PWM, TEMP and NVMC models' Renode-only tests;
                        firmware engines/cpu-renode/tests/firmware/nrf52840/nrf_pwm_step.c) and
                        board.cir with bb.lock (the PWM engine path through bb run; M10)
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
tests/golden/dms-rail/  one-cell reference hashes of E1's four cases and VLED snapshots (Phase 1 M6)
tests/golden/dms-vsense/ every-lattice-point reference hashes of E2's seven cases (Phase 1 M8)
tests/golden/dms-home/  one-cell reference hashes of E3's four cases (Phase 1 M10)
tests/device-smoke/     the device route's smoke board (board.cir, bb.lock; make device-smoke)
tests/fixtures/kicad/   the synthetic two-board system of ADR-007 gate M4b (cli/tests/kicad_import.rs)
spec/                   normative protocol and schema docs (package.md and schema/*.schema.json: ADR-006;
                        board-bbm.md, canonical-json.md, project.md: ADR-007); changing them needs an ADR
docs/adr/               architecture decision records
site/                   the documentation site (VitePress; published to GitHub Pages by
                        .github/workflows/docs.yml): guide/, concepts/, reference/, records/
                        (docs/ and spec/ rendered by site/scripts/sync.mjs); public/plots/ holds
                        the committed waveform figures that `make docs-plots` draws
                        (scripts/docs_plots.sh, scripts/docs_plots.py) and `make
                        docs-plots-check` requires byte for byte
.github/workflows/      ci.yml (CI: make check's parts and the gate jobs) and docs.yml (the site)
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
                        dms_m5b.py), check_local_only.sh (+ check_local_only_test.sh),
                        check_dms_rail.py, dms_rail.sh,
                        dms_zephyr.sh, dms_crosscheck.sh (+ dms_suite_deck.py), demo.sh,
                        stranger_test.sh, docs_plots.sh (+ docs_plots.py),
                        jit_profile_stress.sh, gdb_gate.py, dms_system.sh, check_nrf_pwm.py,
                        dms_home.sh (+ dms_home_boards.py, check_dms_home.py); Phase 2 H0:
                        worktree.sh (+ worktree_test.sh), measure_lock.sh (+ measure_lock_test.sh),
                        baseline.sh, step_time.sh; Phase 2 E4: vcd_check.sh, vscode_test.sh
                        (+ vscode_templates.py, vscode_test/: the extension test run inside VS Code)
tools/vcd-check/        Phase 2 E4 (ADR-016 D1): the VCD loader of make vcd-check (wellen 0.25.7,
                        Surfer's waveform library); its own crate and Cargo.lock, OUTSIDE the
                        workspace (an empty [workspace]), built into $CARGO_TARGET_DIR/vcd-check
docs/templates/vscode/  Phase 2 E4 (ADR-016 D2): tasks.json and launch.json, copied by hand into a
                        project's .vscode/ (cortex-debug attached to bb run --gdb)
tests/fixtures/vcd/     the viewer check's committed input (mixed.vcd, mixed.csv: a real E1 run)
.github/actions/bb-setup/ make setup with the Renode tarball and the apt packages cached (Phase 2 H0)
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
- **Measurements hold the host lock** (Phase 2 plan section 7 rule 3).
  Every perf, cost, startup and timing target is in the Makefile's
  `MEASURE_TARGETS`: it takes `~/.cache/bb/measure.lock` before anything
  else and refuses to run while another measurement holds it, from any
  checkout or worktree. A new such target joins the list; a measurement
  is never started by hand around the lock.
- **Gate budgets** (section "Gate budgets"). Every milestone states and
  records the wall time of its `make test` additions and of its full gate.
  A gate over its budget goes to the nightly tier (ADR-015 D11), not into
  `make test`; an overrun of more than 25 % is a review finding.
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
- **Devices** (ADR-006 D7, Amendment 6). A device is Rust in `bbdev`,
  pinned by the content hash of `bbdev/src/lib.rs`, `engine.rs` and its own
  file (`device:<id>@<major>` in `bb.lock`), so any edit there needs `make
  locks`. Three rules keep its outputs partition-independent: outputs
  change only in `on_timer` (never in `on_pin_change`; a reaction is a
  timer strictly later), timers fire before inputs at one instant, and a
  device's state advances only at its own events, never at a grant
  boundary. Its `t_safe` is `Time::MAX` (a timer off the lattice must not
  bound a grant) and its promise is its earliest timer. Device parts exist
  only in board files; `[[device]]` in `bb.toml` is Phase 2.
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
- **Publication.** The repository is public at
  https://github.com/tyevco/breadboard.dev (`origin`). Agents commit locally
  and never push, change remotes or change GitHub settings; only the
  orchestrating session pushes, and only after the human approves. CI
  (`.github/workflows/ci.yml`) runs on every push, so on every push to
  `main`, and on every pull request; a push to `main` that touches `site/`,
  `docs/`, `spec/` or `README.md` also deploys the docs site to GitHub
  Pages (`.github/workflows/docs.yml`). Commit messages end
  with the co-author trailer given by the orchestrator.

## Known bad ideas (do not do these)

- **Stepping ngspice per instruction.** ngspice advances one *grant* at a
  time, to a breakpoint at `g`.
- **Handing Renode's GDB stub to a debugger, or forwarding a resume
  unchecked.** A GDB step while no `RUN_FOR` runs calls
  `emulation.StartAll()` (a free-run), Ctrl-C pauses the CPU mid-slice
  and can hang a `RUN_FOR`, `monitor` reaches Renode's time control, and
  the stub's park replies race resumes. `bb run --gdb` keeps the stub
  behind its proxy (`engines/cpu-renode/src/gdb.rs`): a `c` or `s` goes to
  Renode only after a state read shows the CPU parked inside a running
  `RUN_FOR` before its end, and Ctrl-C becomes a single-step-mode switch
  between `RUN_FOR`s (ADR-008 Amendment 7). A breakpoint stop or a step
  changes results; such a run carries a `warn` debug mark and is never a
  reference.
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
- **Sending `Clear` or `SIGTERM` to a Renode right after its External
  Control client disconnected.** Renode 1.17.0 restarts the server then, and
  a `Clear` that races the restart leaks the listener or crashes Renode
  (ADR-004 Amendment 2); a `SIGTERM` that races it aborts Renode
  (`SIGABRT`) before the runtime writes the JIT profile (Amendment 9).
  The pool waits for the re-listen first, and a gathering engine waits until
  the server's receive thread sleeps in `accept`
  (`ec_server_accepting`); keep it that way.
- **Parsing KiCad files with regex.** Use a real S-expression parser.
- **A branch-current unknown in series with a milliohm path in an analog
  view.** Under `phase0` (`ABSTOL=1e-13`) ngspice converges every branch
  current to 1e-13 A + 1e-5·|i|. That includes 0 V ammeters, CCVS and CCCS
  sense sources, and V-type B and E sources. Through a few milliohms
  between nodes at about 3 V, that asks for the node voltages to about
  1e-14 V, below double resolution, and the step collapses (ADR-006
  Amendment 2: the TPS22917 view's `RVAR` and `H1` blocks). Write such an
  element as a B conductance, and compute a sensed current from node
  voltages. The same holds for a bench: an ideal (tracking) voltage
  source across a rail's capacitors stalled the E1 deck at `phase0`, with
  or without 10 mOhm in series; give such a `[[supply]]` an `rs`, which
  `bb run` writes as a Norton B conductance (ADR-007 Amendment 3).
- **Timing a firmware rule with Zephyr's `startup-delay-us`.** In Zephyr
  v4.5.0 `regulator_enable()` sleeps `off-on-delay-us`, and
  `startup-delay-us` applies only to a regulator enabled at boot (ADR-008
  Amendment 3). Read the driver before relying on a devicetree delay.
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
  `make worktree TRACK=<name>` sets that up (and a scratch directory of
  its own for the gates' project copies); `make worktree-test` shows it.
- **Phase 2 tracks work in their own worktree on ext4** (plan section 7
  rule 1): `~/src/bb-<track>` on the branch `p2/<track>`, made by `make
  worktree TRACK=<track>`, never in the Windows checkout, which only the
  merger edits. Before each milestone a track merges `main` into its
  branch; it commits only on its branch. Its gates run there (`/mnt/c`
  multiplies file-heavy steps 30-300 times), and so does every
  measurement, under the lock.
- Breadboard's Renode plugins (the lookahead block, `BbStm32Timer`,
  `BbStm32F0Adc` and its host port, `BbNrf52Saadc`, its host port,
  `BbUartLog`, and `BbNrf52Pwm`, `BbNrf52Temp`, `BbNrf52Nvmc` with their
  host port) are compiled once, together, into one
  assembly cached in `~/.cache/bb/renode-plugins` (`$BB_CACHE_DIR` or
  `$XDG_CACHE_HOME/bb` override it); deleting the directory only costs one
  compile (about 2 s). The loader runs before `LoadPlatformDescription`,
  because the STM32F072 and nRF52840 overlays name the plugin's types.
- A fresh Renode plays back a .NET multi-core JIT startup profile cached in
  `~/.cache/bb/renode-jit` (same overrides), one per Renode install,
  platform and plugin setting (ADR-004 Amendment 6). The first run for a key
  gathers it and stops Renode with `SIGTERM` instead of `SIGKILL` so the
  runtime writes it, once the External Control server is back in `accept`
  (Amendment 9; `make jit-profile-stress` checks the gather under load).
  Deleting the directory only costs that one gather.
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
