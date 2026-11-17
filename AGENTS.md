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
| `make setup` | Install or verify the pinned toolchain (idempotent): Rust 1.99.1, uv, Renode 1.17.0 portable, arm-none-eabi-gcc, ngspice 42 + libngspice0-dev, python3-jsonschema, minisign and openssl (the registry tests, Phase 2 E1); then build `bb` and install it into `~/.local/bin`. `make setup SETUP=demo` skips the ARM toolchain, uv and jsonschema (what the demo needs: Rust, Renode, ngspice, `bb`). As root (a container) it runs apt without sudo (Phase 1 M3). `make setup SETUP=zephyr` (optional, M6) adds the Zephyr host tools and the pinned Zephyr SDK 1.0.1 for `make fw-rebuild` |
| `make setup-container` | Phase 1 M3 gate (R7): `make setup SETUP=demo` timed in a clean `ubuntu:24.04` container (podman, or docker), as root, from `git archive HEAD` (`SETUP_REV=`, or `SETUP_WORKTREE=1`); timing in `out/setup-container/timing.txt` (`scripts/setup_container_test.sh`) |
| `make demo` | Phase 1 M7 (plan section 5): the README's reference demo in one command, on a copy of `examples/dial-me-spinner` in `~/.cache/bb-demo` (`DEMO_WORK`), every Renode fresh: `bb open`, `bb flash` and `bb run` at the cold bench with the correct and the wrong-order firmware, `check_dms_rail.py` (pass; exactly LSW-3 fails), both runs against `tests/golden/dms-rail/{ok,wrong}-cold`, and `run.json`'s view list (`scripts/demo.sh`, about 15 s; outputs in `out/demo`). Needs only `make setup SETUP=demo` |
| `make stranger-test-container` / `make stranger-test-wsl` | Phase 1 M7 exit measurement (approver decision B measured it from a local clone; the repository has since been published): the README's quickstart block of the commit at HEAD (between the `stranger-test` markers; its `git clone https://github.com/tyevco/breadboard.dev` URL replaced by `STRANGER_URL`, default the local repository, so `STRANGER_URL=https://github.com/tyevco/breadboard.dev` tests the published repository itself) run line by line and timed, as root, in a clean `ubuntu:24.04` container (rootless podman, the repository mounted read-only, cloned from `file:///repo`) or a fresh WSL distro imported from the same image and unregistered afterwards; then the committed hashes and `make demo` (`scripts/stranger_test.sh`; `out/stranger/<mode>/timing.txt`; limit 900 s). The human run: `docs/phase1-exit-checklist.md`. Keep the README block runnable as written |
| `make docs-plots` / `make docs-plots-check` | The docs site's waveform plots, `site/public/plots/<name>.{light,dark}.svg` (committed: the Pages build has no Renode): make demo's two E1 runs, the E2 cases `sag`, `rec`, `s50` and `s50-none` of `make dms-vsense`, the E3 cases `typ` and `step` of `make dms-home` and the spike, every Renode fresh and in parallel, drawn with plain Python (`scripts/docs_plots.sh`, `scripts/docs_plots.py`; outputs in `out/docs-plots`, about 5 min alone, most of it the E3 homing: 305 s in the M10 review, 450 s in M10 beside other gates). The SVGs are a pure function of the runs; `docs-plots-check` regenerates them into `out/docs-plots/svg` and requires the committed files byte for byte (CI job `dms-rail`). After a change to results, run `make docs-plots` and commit the figures |
| `make doctor` | Print every tool's version; exits non-zero if anything is missing |
| `make build` | `cargo build --workspace --all-targets` |
| `make test` | The exit gate: `cargo-test`, `spike`, `determinism`, `golden`, `feedback`, `feedback-order`, `adc`, `adc-order`, `adc-model`, `ec-reconnect` (since Phase 1 also `nrf-feedback`, `adc-kicad`, `dms-rail-test`, `packages`, from M8 `nrf-saadc-model` and `dms-vsense-test`, from M9 `gdb-test`, from M11 `dms-system`, and from M10 `nrf-pwm-model`, `device-smoke` and `dms-home-test`; from Phase 2 H0 `measure-lock-test`, from B1 `bench-test`, from B2 `assert-test` (from B3 `dms-rail-test`, `dms-vsense-test` and `dms-home-test` are `bb test` files), from E4 `vscode-templates`, from E1 `registry-test`, from P1 `pi-static-test`, from P2 `pi-glue-test`, from R2 `dms-e1-system-test`, from H5 `multi-producer`, from P3 `pi-idd-test`, from P4 `nrf-power-model` and `dms-w1-test`, from P5 `dms-w2-test`; H6 removed `adc-stock`; every step appends its wall time to `out/test-timing.tsv`, and `TEST_SKIP` leaves steps out, as CI's `test` job does with `cargo-test`), run on a warm Renode pool (ADR-004 Part B; `POOL=0` runs it fresh, `POOL_SLOTS` sets the pool size, default 5). `make check` took about 14 minutes in M10, 833 s pooled and 832 s with `POOL=0` beside other gates (ADR-008 Amendment 11; the pool's own gain was measured in Phase 0: 2 min 40 s against 3 min 10 s fresh, ADR-004 Amendment 4) |
| `make cargo-test` | `cargo test --workspace` only (`BB_REQUIRE_RENODE=1`), on a warm pool like `make test` (Phase 2 H6; `POOL=0` fresh). Its own CI job `cargo-test` since H6: CI's `test` job runs `make test TEST_SKIP=cargo-test` |
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
| `make adc-model` | ADR-005 test plan 1-3 (`examples/adc/bb_adc_test.py`, about 80 s): TIM3 PWM through the overlay's AF wiring, TRGO, the ADC's registers and exact conversion instants for every SMP × RES × CKMODE, AWD, the host port, and the board firmware (TRGO conversions, the AWD interrupt, no wake on a push, timed inputs = `SetValue`) |
| `make adc-equiv` | ADR-005 step 5 gates (`scripts/adc_equiv.sh`): Part A (one cell) and Part B (lookahead, 1 ms and 10 ms) on `examples/adc`, and `board-open.cir` (acyclic) at 1, 10 and 50 ms, all identical except `grant` records to the every-lattice-point references (`tests/golden/adc*/outputs.sha256`); no promise broken; no `stop:ngspice` (CI job `adc`) |
| `make adc-determinism` | 10 runs of `examples/adc`, byte-identical |
| `make ec-reconnect` | ADR-004 Amendment 8: the one-cell references of `examples/adc` (`--lookahead off --adc-push every`) and `examples/feedback` (`--lookahead off`), with the Renode engine replacing its External Control connection every 2 000 messages (`EC_MESSAGES`, passed as `BB_RENODE_EC_MESSAGES`), must give their committed hashes (about 40 s; in `make test`) |
| `make gdb` / `make gdb-test` | Phase 1 M9 (ADR-008 D10, Amendments 7 and 8): `bb run --gdb <port>` (`--gdb-wait on\|off`) driven by a real GDB in batch mode (`scripts/gdb_gate.py`; GDB: `gdb-multiarch` from `make setup`, else `arm-none-eabi-gdb` or the Zephyr SDK's, `GDB=` overrides) on the spike and E1 (a copy of the project in `~/.cache/bb-gdb-gate`). `gdb-test` (in `make test`, about 45 s): attach and detach without a stop give all four committed hashes of the run without GDB on both boards, a breakpoint hit and continued gives exactly one `warn` debug mark (and run.json `debug: true` on E1), a run without `--gdb` has no debug key, `monitor` is refused, and a client of Renode's internal stub port, first (`BB_GDB_STUB_DELAY_MS`, a test knob) or while the proxy holds it, gets no reply and changes nothing (Amendment 8). `gdb` (CI job `dms-rail`, about 5 min) adds attach mid-run, the scripted breakpoint session 21 times (5 on E1) byte-identical, steps and reads at a breakpoint, a 5 s hold past a 2 s External Control timeout (`BB_RENODE_IO_TIMEOUT`) with no grant boundary during it, Ctrl-C then detach or continue, 1 500 steps across grants and WFI, a second debugger, GDB or bb killed during a halt, the loopback listener, the pool and the internal-port race 5 times without the knob. A marked run is never compared against a reference |
| `make ec-long` | ADR-004 Amendment 8 long one-cell gate (`scripts/ec_long.sh`, about 2.5 min): the ADC board's every-lattice-point reference over 1 500 ms (`EC_LONG_FOR`; Renode used to crash after 81 746 grants) must finish, reconnect at least once, pass `check_adc.py` and equal the lookahead run except grant records (nightly tier since Phase 2 H6: `make nightly`, `nightly.yml`) |
| `make ec-exposure` | Phase 2 H3 gate (ADR-015 D5, G4, Amendment 2; in `make test` through `cargo-test`, about 8 s): Renode connects out to `bb`'s `127.0.0.1` listener (`CreateExternalControlClient`), so it listens on nothing. 20 engine starts (4 fresh in parallel, 16 leased from a 2-process pool), each with a foreign client (a separate `bash` with `/dev/tcp`) connected to `bb`'s listener before Renode, and again before each of its reconnects (every 8 messages): every foreign client refused without reading a byte, the run's own Renode admitted (its socket inode among the Renode process's fds), Renode listening on no TCP port after start, after every grant and at the end, and the PA5 edges equal to a run without foreign clients (`engines/cpu-renode/tests/ec_exposure.rs`) |
| `make nrf-gates` | Phase 1 M2 gate (ADR-008 D1-D4, D6; Amendment 2): the Renode engine tests `engines/cpu-renode/tests/nrf52840.rs` on Breadboard's nRF52840 (`Platform::Nrf52840Bb`, `nrf52840-bb@1`: no SVD download, 64 MIPS) with the bare-metal gate firmware in `engines/cpu-renode/tests/firmware/nrf52840/` (nrf-blinky edges identical at one-cell grants, 10 ms grants, plugin armed and lookahead on, self-test status exactly 0x3; nrf-ppi; nrf-busywait: a 10 ms nrfx busy-wait takes 6.66 ms of virtual time; the D6 TIMER-compare-then-long-sleep promise clamp; the PIN_CNF audit, mid-run and end-of-run) and the prebuilt Zephyr gate image (nrf-zephyr), then `make nrf-feedback` (about 2 min) |
| `make nrf-saadc-model` | Phase 1 M8, first half (ADR-008 D7, D8, Amendment 4; in `make test`, about 25 s): Breadboard's nRF52840 SAADC (`BbNrf52Saadc` on `nrf52840-bb@2`). The Renode-only register and timing tests in ADR-005's test-plan shape (`examples/nrf-saadc/bb_saadc_test.py`, `nrf_saadc_idle.elf` / `nrf_saadc_burst.elf`: reset values, every TACQ x RESOLUTION x OVERSAMPLE x BURST instant, the 8x BURST, transfer and rounding, VDD, EasyDMA, LIMIT, timer mode, the IRQ, the TACQ < 2 L refusal, the UART tap; run twice, byte-identical), then `scripts/nrf_saadc_board.sh` on `examples/nrf-saadc` (the loading `adc_in`: 8 `#smp` toggles and exactly 8 charge transfers per 8x burst against the analytic charge sharing, codes, UART lines; lookahead at 1 ms and 10 ms, push-on-change and `--engine-order analog-first` equal to the every-lattice-point one-cell reference `tests/golden/nrf-saadc/outputs.sha256`; the refusal at `--renode-quantum 25us` and its message; the `load=none` escape). `make nrf-saadc-update` rewrites the hashes; `make cost-table-nrf-saadc` is the board's cost-table row (also in `make cost-table`; idle host) |
| `make nrf-pwm-model` | Phase 1 M10, first step (ADR-008 D9, Amendment 9; in `make test`, about 25 s): Breadboard's nRF52840 PWM0-PWM3 (`BbNrf52Pwm`), the TEMP and NVMC stand-ins (`BbNrf52Temp`, `BbNrf52Nvmc`) and their host port (`BbNrf52HostPort` at 0x5FFC0000: the PWM pin ring, TEMP's value) on `nrf52840-bb@3`. `examples/nrf-pwm/bb_pwm_test.py`, Renode-only, each group twice and byte-identical: `registers` (`nrf_saadc_idle.elf`; reset values; every pin change exact to 0.5 ns for up and up-and-down x PRESCALER x COUNTERTOP x COMP x POLARITY; REFRESH, ENDDELAY, LOOP, the SHORTS, NextStep, the Grouped, Common and WaveForm decoders, PSEL and ENABLE while running, every event instant, the IRQs, the ring overflow; TEMP's DATARDY at 36 us and STOP; NVMC erase timing, CONFIG gating, partial erase, ERASEALL) and `firmware` (`nrf_pwm_step.elf`: TEMP, an NVMC erase polled on READY, 4 full steps at 446 PPS as `nrfx_pwm_complex_playback` runs them, every pin change against the firmware's duty table, the UART lines, and no lookahead promise past the next PWM edge). Then the engine path (M10, third step; ADR-008 Amendment 11): `examples/nrf-pwm/board.cir` (cyclic through a constant `digital_in`) runs the same firmware through `bb run`, fresh: `scripts/check_nrf_pwm.py` requires the four pins' `pin_edge` records to be the duty table's changes to the femtosecond, and the one-cell reference (`--lookahead off`), the default (lookahead on), `--max-quantum 1ms` and analog-first give identical `nets.csv`, `nets.vcd` and non-grant records |
| `make dms-home` / `make dms-home-test` | Phase 1 M10 gate (plan section 2 E3; ADR-008 Amendments 9-12, ADR-006 Amendments 6-7): E3, the detent homing H0-H5 on the hand-written board files of `examples/dial-me-spinner/e3` (written by `scripts/dms_home_boards.py`; the detent sheet, the nRF52840, the device `M1`, and the suite's power path as the bench) with `firmware/prebuilt/homing.elf` and `homing_step.elf` (`scripts/dms_home.sh`, `scripts/check_dms_home.py`). Cases: `step` (one full step at 446 PPS through the bridges and coils, against the suite's `test_firmware_microstep_pwm[VBAT3.7-25C]`, tolerances 2.5 % / 1 % per unit duty), `typ` (tS 50 ms), `worst` (the suite's DRV5032FA corner: tS 75 ms, BOP 4.8 mT, BRP 0.5 mT, at its worst sample phase), `park` (H1, BOP/BRP 4.8/0.5 mT). Since Phase 2 H5 the homing boards' five driver nets are digital-only (`*bb digital`, ADR-014 D4, a recorded model change; MOT_NSLEEP is probed as `U10.P1.15`), so with `dms/mechanism@2`'s promise the kernel takes quiet grants with several producers (ADR-014 D1-D3): `typ` runs 433 grants at bb's defaults against 270 000 one-cell grants. `dms-home` (the one-cell references dominate: a homing reference is about 4.5 min alone and 6-10 min beside other jobs, a homing run at bb's defaults under half a minute; about 330 MB with the engine's `.save`, 1.5 GB before H1): every case and its one-cell reference (`--lookahead off`; on the homing boards a true equivalence, the partitions differ; on `step` U10's own short promises, ADR-014 G3), the checks (H0-H5, step counts, every phase's step rate from the PWM edges, no park-stop contact before H4, H3 ending 20-108 steps from the stop, the park stall, the HOME_N latency of Amendment 10 as a worst-phase bound: the device's tS must be the FA's, tS + tACTIVE and the run's latency within 75 ms typical, 97.4 ms worst), the references against `tests/golden/dms-home`, `typ`'s peak RSS (ADR-015 G2, `make dms-home-rss`), `typ` at most `DMS_HOME_TYP_MAX_GRANTS` (5 000) grants and at least `DMS_HOME_TYP_MIN_GAIN` (3x) less simulation than its one-cell reference (beside the gate's jobs; the protocol figure is `make dms-home-g0`), analog-first and `--max-quantum 1ms` (typ, step), 10-run determinism (typ), and three negative controls that must fail exactly their checks (`homing_fast.elf`: step rate, stop contact, H3; M1 at tS 200 and 286 ms, the DRV5032FB: the latency). `dms-home-test` (in `make test`, about 40 s): the boards are the generator's output, `step` with its checks, hashes and analog-first, and beside it (Phase 2 R1) `project-step`, the step case on the project route, with its hashes and `scripts/compare_project_run.py` against `step`. `make dms-home-update` rewrites the goldens (the project cases' too); `make cost-table-dms-home` is its cost-table row (idle host) |
| `make multi-producer` | Phase 2 H5 gate (ADR-014 G1; in `make test`, about 15 s): `tests/multi-producer/board.cir`, a small synthetic board (an nRF52840 running `homing.elf`, `dms/mechanism@2` at tS 5 ms on digital-only nets, ngspice behind two sampled `adc_in` inputs), 100 ms, fresh Renode (`--renode-pool off`: not in `pool-equiv`): bb's defaults equal the one-cell reference except grant records with quiet grants `Q = {devices}` and no early stop (`P` empty, ADR-014 D1; bb's `quiet sets` line), at most `MP_MAX_GRANTS` (500) grants against 10 000; analog-first byte-identical; the scope control (MOT_NSLEEP's directive removed, so U1 -> ngspice -> U1) never takes `Q = {devices}`; then `bbdev/tests/multi_producer.rs`: the kernel part with mock engines, a declaring device that reacts before its promise fails with `PromiseBroken`, `set_pin` in `on_pin_change` and in an internal timer refused, the scope rule, and a device change exactly at `G` reaching a later consumer as the one-cell plan delivers it (`scripts/multi_producer.sh`) |
| `make dms-home-g0` | Phase 2 H5, ADR-014 G0 (under the measurement lock; about 25 min at N = 3 with the base case): E3 `typ` three ways with the ADR-003 protocol (`G0_N` interleaved rounds, medians of the simulation time, fresh Renode): the committed run before H5 (`G0_BASE_BB` on the checkout `G0_BASE_TREE`, both optional), the model change alone (D4 at one cell, `--lookahead off`) and D1-D4 (bb's defaults); D1-D4 must equal D4 alone except grant records and the protocol gain must be at least `G0_MIN` (3x) (`scripts/dms_home_g0.sh`; measured 32.3x, ADR-014 Amendment 1). Run it on an idle host |
| `make dms-home-project` / `make cost-table-dms-home-project` | Phase 2 R1 gate (ADR-013 D1, gate G1; about 9 min under load): E3's `step` and `typ` on the project route, `examples/dial-me-spinner/e3/step` and `e3/home` (the imported main board, `M1` a virtual `[[device]]`, J40 the motor's phases from the project-local `e3/packages`), at bb's defaults and as their one-cell references (`--lookahead off`; since H5 `project-typ` takes ADR-014's long grants, a true equivalence, and `project-step` keeps the power stage in scope), the references against `tests/golden/dms-home/project-*`, `check_dms_home.py` on `project-typ`, `scripts/compare_project_run.py` against the board files' `step` and `typ` (run beside them, and equal to their goldens: the non-grant records and every shared digital column byte-identical after the documented name mapping, the analog columns reported), analog-first on `project-step`, and `project-typ`'s digital-only MOT nets (ADR-014 D4: `e3/home/bench.toml` leaves them out of `[analog] nets`) and grant count reported (`scripts/dms_home.sh project`), and a `--seed 2` negative control that the comparison must fail on M1's seeded phase. `cost-table-dms-home-project` is its cost-table row (`project-step`, one-cell vs defaults; also in `make cost-table`; idle host) |
| `make dms-e1-system` / `make dms-e1-system-test` / `make cost-table-dms-e1-system` | Phase 2 R2 gate (ADR-013 D2, gates G2 and G3, Amendment 3; about 90 s pooled, 170 s fresh): E1 on the system, `examples/dial-me-spinner/e1-system` (`simulate = ["main", "base"]`: the base board's power path, a `[[battery]]` at `base:VBAT`, `ti/bq25180` at `suite_compat`, its capacitors and the link harness, in the deck in place of E1's VSYS supply), at `bench/cold.toml` and `bench/nominal.toml`, on copies in `~/.cache/bb-dms-e1-system` (`scripts/dms_e1_system.sh gate`). `scripts/check_system_run.py` (it is a system run: `simulated`, base:U1 and the harness in the deck, each board's ground, the path carries the load), E1's four checks (`check_dms_rail.py --board main`), the default runs equal to their one-cell references (`--max-quantum 10us`; 71 against 12 000 grants, a true equivalence) and to `tests/golden/dms-e1-system`, analog-first byte-identical at both corners, 120 ms in under 60 s at both engine orders, the wrong-order firmware failing exactly LSW-3, 10-run determinism, and **the cross-check**: each case's `pin_edge` records replayed into the suite's own `led_rail_sequence` deck with its battery network (`dms_suite_deck.py --battery`, the fitted TPS22917 view in both), VLED within 10 mV at every 10 µs, with a negative control (the cell's R0 doubled must fail it). The cross-check needs `DMS_REPO` and the suite venv (local, like `dms-crosscheck`). `dms-e1-system-test` (in `make test`, about 16 s pooled): ok-cold, ok-nominal and ok-cold analog-first in parallel with the checks, the committed hashes and convergence. `make dms-e1-system-update` rewrites the goldens; `cost-table-dms-e1-system` is its cost-table row (ok-cold, one-cell vs defaults; also in `make cost-table`; idle host). `cli/tests/system_run.rs` (cargo test): the system deck, and G3's refusals (a second target, an ambiguous bench name, a harness wire half in the analog scope) |
| `make device-smoke` | Phase 1 M10, second step (ADR-006 D7, Amendment 6; in `make test`, about 20 s): `tests/device-smoke/board.cir`, examples/feedback's STM32F103 and firmware with the `dms/mechanism@2` device (since H5 the run at bb's defaults takes 21 grants with `Q = {devices}`: M1 samples on lattice points, so it also gates ADR-014 Amendment 1's plan-order tie-break) (`packages/dms/detent-mechanism`) in a loop with the CPU through the direct digital routes of `spec/package.md` section 12 (U1.PA5 -> M1.NSLEEP, M1.IN_A1; M1.HOME_N -> U1.PB0). Four fresh runs (`--renode-pool off`: the board is not in `pool-equiv`): the one-cell reference (`--lookahead off`), the default, `--engine-order analog-first` and the default again; `scripts/check_device_smoke.py` on each (HOME_N rises at exactly 57.04 ms, PA6 follows within one cell); `nets.csv`, `nets.vcd` and the non-grant records identical across all four |
| `make nrf-feedback` | ADR-008 D3 board-level gate (in `make test`): `examples/nrf-feedback` (examples/feedback's circuit on the nRF52840: RTC0 drives `U1.P0.13`, GPIOTE IN on `U1.P0.11` copies to `U1.P0.15`) with lookahead on at 1 ms and 10 ms byte-identical to its one-cell `--lookahead off` reference (`tests/golden/nrf-feedback/outputs.sha256`; `make nrf-feedback-ref`), no promise broken, `check_feedback.py --nrf`, and `--engine-order analog-first` byte-identical |
| `make nrf-costs` | ADR-008 D1: fresh startup and per-grant cost of the nRF52840 blinky at 64 MIPS against the STM32F103 blinky (interleaved, N = `BB_NRF_COST_N`, medians); reports only. Run it on an idle host |
| `make lookahead-startup` | ADR-003 step 5 startup-cost gate: Renode engine startup with the cached plugin within 0.3 s of a start without it (interleaved, N = 7, medians; run on an idle host) |
| `make startup-perf` | ADR-004 G1: fresh `bb run` engine startup of the spike and the feedback board at least 12 % and 0.3 s below `BB_RENODE_STOCK_RUNTIME=1` (the stock .NET settings, no JIT profile and the 100 ms connect poll), interleaved, N = 7 (`STARTUP_N`), medians, outputs identical; `STARTUP_BASE=<bb>` compares against another binary. Run it on an idle host |
| `make jit-profile-stress` | ADR-004 Amendment 9: the startup JIT profile's gather under load (`scripts/jit_profile_stress.sh`): `JIT_STRESS_ROUNDS` (4) rounds of `JIT_STRESS_PAR` (8) parallel gathering spike runs, each with an empty `BB_CACHE_DIR`, while `JIT_STRESS_LOAD` busy loops (default 2 x nproc) load every CPU; every run must leave exactly one profile (about 3 min). Not in `make test`: it loads the host on purpose |
| `make pool-equiv` | ADR-004 G4: one pool process serves, twice each in alternating order (`POOL_EQUIV_ROUNDS`), the spike (default, 1 ms, 500 ms), feedback (on, off, plugin never, polling firmware), adc (on, off), adc-open, adc-kicad (Phase 1 M4; H6 removed adc-stock and adc-open-stock), nrf-feedback (on, off, plugin never; M2), E1 (`dms-rail` ok-cold default and one-cell, wrong-nominal; M6), nrf-saadc (default and its every-lattice-point reference; M8), E2 (`dms-vsense` s50 default and reference; M8) E3 (`dms-home-step`, `dms-home` typ; M10) E3 on the project route (`dms-home-project-step`, `dms-home-project`; Phase 2 R1) and E1 on the system (`dms-e1-system` default and one-cell; R2); every pooled run must be leased and byte-identical to its fresh run and to the committed hashes (`scripts/pool_equiv.sh`) |
| `make pool-perf` | ADR-004 G2: on a 1-slot pool, the first lease starts in at most 0.5 s, median engine startup is at most 0.30 s for spike, feedback and adc (N = 7), and spike total wall at most 1.2 s (`scripts/pool_perf.sh`). Run it on an idle host |
| `bb pool run -- <cmd>` | Run `<cmd>` with a warm Renode pool in `BB_RENODE_POOL` (`--slots N`, `--max-runs K`); `bb pool start` / `stop` / `status` for an interactive shell (`bb pool --help`). `bb run --renode-pool off` ignores a pool |
| `make golden-update` | Regenerate the golden snapshots, the spike output hashes and the partition hashes (spike, feedback, feedback-open), the feedback one-cell reference hashes and the ADC reference hashes from fresh, checked runs. Review the diff before committing it: a change to an equivalence reference is a change to results and needs a reason (and, for the protocol, an ADR) |
| `make cost-table` | The README cost table with the measurement protocol (`scripts/perf_ab.sh`, interleaved, N = `PERF_N`, medians): the spike at 10 µs vs 10 ms grants, `examples/feedback` and `examples/adc` with lookahead off vs on, `examples/feedback-open` at 10 µs vs 10 ms. Reports only; no gate. Run it on an otherwise idle host |
| `make dms-source` | Phase 1 M0: the vendored dial-me-spinner KiCad inputs (`examples/dial-me-spinner/kicad/`) against the sha256s in `examples/dial-me-spinner/SOURCE`, and against `git show <commit>:<path>` when `DMS_REPO` (default `../dial-me-spinner`) exists, and that every package file migrated from dial-me-spinner (a `part.toml` `origin`, the WS2812C view) has an entry (`scripts/dms_source_check.sh`) |
| `make dms-suite-check` | Phase 1 M0 gate: re-run the dial-me-spinner SPICE suite (`-m "not slow"`) at the reference commit `6854d25` from a `git archive` export in `~/.cache/bb-dms-suite` (`scripts/dms_suite.sh`, about 4-5 min) and require `out/dms-suite/results.json` to equal the committed `examples/dial-me-spinner/suite/results.json` byte for byte (checks, test outcomes, every deck's numeric fallback level, ngspice version). `make dms-suite-update` regenerates it. Never check out, stash or write in the dial-me-spinner repository |
| `make dms-s1` | Phase 1 plan S1 (ADR-006 Amendment 2): the E1 deck (two LED rail on/off cycles, 120 ms, both benches) through `NgspiceEngine` at `phase0`, with the S1 fix of the TPS22917 view. The view and every deck are built in `~/.cache/bb-dms-s1` from a `git archive` export of `6854d25`; they are TI-derived, so they stay local-only and never enter the repository (`scripts/dms_s1.sh`, `scripts/dms_s1.py`, `engines/analog-ngspice/examples/deck_run.rs`). Its 18 gates: 120 ms at both benches; grants of 10 µs, 1 ms and 10 ms byte-identical; VLED at the check instants and the LSW-2 time against the suite's `suite_fallback` run; VLED at every 10 µs against the converged run; the fix's result neutrality. About 1.5 min on 8 jobs (`DMS_S1_JOBS`). `make dms-s1-cost` is the S1 cost table (plan §8; `DMS_S1_N` interleaved rounds, medians; run it on an idle host) |
| `make pi-spike` | Phase 2 P0 (ADR-009 D2, G3; Amendment 1 is its report, Amendment 2 the approver's decision D-W1): the W1 and W2 suite replicas (the suite's decks with the S1-fixed TPS22917 at `phase0`) against `results.json`, and the static-load decks with the first RT9080, BQ25180 and TPS61040 `pi` views (`examples/dial-me-spinner/pi-spike/views`) at `phase0` and the suite's option sets: grant partitions, the converged reference, the model difference and its attribution, the tolerances of `pi-spike/tolerances.toml` (each equal to the stated rule on the measured difference), W1 at its labelled stress corner (`[w1]`: 55 mA, RT9080 worst, re-derived from the 50-60 mA sweep; feasibility: at least 0.5 ms below 2.94 V, recovered by 8 ms) and the 50 mA margin check (no POFWARN at +5 %; margin reported at nominal and halved +3V3 capacitance) (`scripts/pi_spike.sh`, `scripts/pi_spike.py`; decks local-only in `$BB_SCRATCH/bb-pi-spike`; 85 runs, about 85 s on 8 jobs, `PI_SPIKE_JOBS`). Not in `make test`; needs `DMS_REPO` and the suite venv |
| `make pi-static` / `make pi-static-test` | Phase 2 P1 (ADR-009 D1-D3, Amendment 4; decision D-W1, Amendment 2): W1 and W2 in static-load mode on the `pi` package views. `scripts/pi_static.py` assembles each deck from the packages (the ladder at `pi`, RT9080 `worst_dropout`, BQ25180 `suite_compat`) and `examples/dial-me-spinner/system/board.bbm`, with the bench of static-load mode (`examples/dial-me-spinner/pi/static_load.toml`), and runs it through `deck_run` at `phase0` (`scripts/pi_static.sh`; outputs in `out/pi-static`, `PI_STATIC_JOBS`). Gates: W1 at its labelled stress corner (55 mA) against P0's recorded replica within `[static_load.w1_stress]` and its feasibility (0.5 ms below 2.94 V, recovered by 8 ms), the 50 mA margin check (no POFWARN; halved capacitance reported), the load sweep (30 / 50 / 55 / 60 mA), W2 rule and old rule within P0's tolerances, every W1 check's rung `pi`. `pi-static-test` (in `make test`, 14-21 s: W2 old is the long pole) runs the seven cases; `pi-static` adds the stress corner at 10 µs and 10 ms grants and a second run (consistency checks). Needs no dial-me-spinner checkout |
| `make pi-glue` / `make pi-glue-test` | Phase 2 P2 (ADR-009 D5, G8, G9, Amendment 5): rail-tracking, tristate `digital_out` at `pi` (`spec/pin-model.md`: two B conductances of `zout` to the rail, the package's `[power] io_rail` or the binding's `rail`, and to the ground pins' net, gated by the level `s` and the output enable `oe`, and the high-Z leakage `rhiz`, default 100 MΩ); `oe` is `PIN_CNF.DIR` on the nRF52840 (the CPU engine's `<pin>#oe` from the `PIN_CNF` write hook, the GPIO clock and lattice rule; `bb run --pi-oe fixed`: 1, the only source on the STM32). `pi-glue-test` (in `make test`; `scripts/pi_glue.sh`, `scripts/pi_glue.py`, copies in `$(BB_SCRATCH)/bb-pi-glue`): G9, twenty Phase 1 decks solve their DC operating point at `pi` with every MCU output high-Z (`bb run --operating-point --fidelity pi`, each with its expected number of `pi` outputs; board files with a stand-in rail, project benches made `pi` with `main:U1` excluded); E1 cold at `pi` (`examples/dial-me-spinner/bench/cold-pi.toml`), both firmwares: `check_dms_rail.py`, LSW-3 without the `voh` = 3.3 V pessimism, the `#oe` edges and the high-Z levels before them; G8, LED_PWR_EN's VOH against VDD − 0.4 V at the PS's 0.5 mA (`bench/cold-pi-ioh.toml`, assertion `PS-VOH-SD`); the runs against `tests/golden/pi-glue`. `pi-glue` adds the `oe` prototype's Renode tests (`tests/nrf52840.rs` `nrf_oe_*`, firmware `nrf_oe.c`), one-cell / 1 ms / default equality, analog-first, a second run, the `fast` side-by-side and `--pi-oe fixed`'s report; `make pi-glue-update` rewrites the goldens; `make pi-oe-cost` (measurement lock) is `PIN_CNF.DIR` against `oe = 1`, at most 5 % slower |
| `make packages` | ADR-006 (Phase 1 M3; in `make test`): every `part.toml`, `pins.toml`, test vector, `bb.lock`, project `bb.toml`, bench and `board.bbm` against `spec/schema/*.schema.json` (`scripts/validate_packages.py`, python3-jsonschema), `bb pkg lint` clean, `bb pkg test` (every vector through `NgspiceEngine` at one cell and 1 ms, identical, and at every recorded qualification; the first-party packages and `examples/adc/kicad/packages`), and every example's `bb.lock` and every example project's `board.bbm` current (`bb open --check`); `scripts/check_local_only.sh` (no `LicenseRef-local-only` package, nothing under `local-packages/` and no TI-derived model file is tracked; outside a git checkout it checks every file of the tree), and its regression test `scripts/check_local_only_test.sh` |
| `make registry-test` | Phase 2 E1 (ADR-011 G1-G5, G9, G10; in `make test`, about 5 s): the registry end to end against local fixtures only (`scripts/registry_test.sh`): throwaway minisign keys, a `file://` registry and a local static HTTPS server on 127.0.0.1 (`scripts/registry_https.py`, a self-signed certificate trusted through `CURL_CA_BUNDLE`). Every first-party package published (the D7 licence gate and the D11 names in `bb pkg lint --publish`), `bb registry check`, the index lines, snapshot and config against `spec/schema`; G5 (two `bb pkg pack` byte-identical, the extracted tree hash = the source's, the ustar form); G1 (the spike from the registry with no first-party packages, its lock's trees = the committed lock's, then `bb run` with the registry moved away, a recording `curl` and in a network namespace: `tests/golden/spike/outputs.sha256`; the same over HTTPS, `--offline` with the server down); G2 (a modified archive byte or header, a modified index line, a modified, unsigned or foreign-signed snapshot, a rolled-back serial: each refused); G3 (yank: a new resolve skips it, a lock keeps it with a warning in `bb open` and `bb run`; legal removal); the registry PR diff checks and `owners.toml`; G4 (a routed namespace never resolves on the public registry; a local package of it is used); G9 (`bb-part/0` and `/1` archives resolve with their trees, an unknown major says "update bb"); vendor files (`[behavioral] svd`: fetched over HTTPS, locked, a changed sha256 refused); local sources first and the source-change rule. Needs minisign, openssl and curl (`make setup`) |
| `make registry-gate` / `make registry-repo` | Phase 2 E1 full gate: `registry-test`, then G7 (`scripts/registry_g7.sh`): `registry-repo` (`scripts/registry_repo.sh`: the LOCAL public registry repository `~/src/bb-registry`, `BB_REGISTRY_REPO`, from `registry/template/`, every first-party package, signed with the key in `~/.config/bb-registry` through the registry's own `ci/publish.sh`, committed locally, never pushed), verified over local HTTPS with the key compiled into `bb`; dial-me-spinner's E1 project with no first-party packages and a `registry+` lock: cold cache, then offline with a warm cache, each run `check_dms_rail.py` and `tests/golden/dms-rail/ok-cold`; the registry's `ci/pr-check.sh` on a local pull request (an owner passes, a non-owner is refused). `docs/registry-publish.md` is the human's publishing step |
| `make test-offline` | Phase 2 E1 G8: `make test` and `make packages` in a network namespace with loopback only, as the calling user (`scripts/offline.sh`: `unshare -rn`, then a nested user namespace mapping the uid back); refuses to start if `https://index.crates.io` answers. Needs unprivileged user namespaces |
| `make dms-vectors` / `make dms-vectors-check` | Phase 1 M5 (ADR-006 Amendment 3): regenerate the test vectors of `packages/ti/tps22917` and `packages/worldsemi/ws2812c-2020` from the dial-me-spinner suite's own deck builders at `6854d25` (`scripts/dms_packages.sh`, `scripts/dms_packages.py`; `git archive` export, suite venv), or require the committed ones byte for byte and `nordic/nrf52840-qiaa`'s GPIO sets to equal the suite's resistances (needs `DMS_REPO`) |
| `make dms-local-packages` | Phase 1 M5: build the LOCAL-ONLY vendor-derived `ti/tps22917` (`LicenseRef-local-only`: the suite's TI-derived view with the S1 fix and the `GND` -> `GNDP` rename) in the git-ignored `local-packages/` (`DMS_LOCAL_PACKAGES`), qualify it with `bb pkg test --record`, and run gate G8 (the rename gives byte-identical `bb pkg test --report` lines). Never commit `local-packages/` |
| `make dms-m5b` | Phase 1 M5b gate (after `dms-local-packages`): the committed datasheet-fitted TPS22917 view against the local vendor-derived one on the E1 checks through `NgspiceEngine` at `phase0`, both benches (VLED at each P0.06 rise and the LSW-3 maximum within 10 mV, LSW-2 within 1 ms, the wrong-order excursion within 10 mV; `scripts/dms_m5b.py`, `out/dms-m5b/summary.json`, about 40 s) |
| `make locks` | `bb open --update-lock` on every example board and project: after an intended change to a package, a platform `.repl` or a Renode plugin source a platform declares (ADR-006 D8, ADR-015 D6); review and commit the `bb.lock` and `board.bbm` diffs. A `bb-lock/0` file is rewritten as `bb-lock/1` (ADR-011 D5) |
| `make adc-kicad` | Phase 1 M4 gate M4a (ADR-007 Amendment 1; in `make test`): `examples/adc/kicad`, the ADC board drawn in KiCad, imported and run as a `bb.toml` project (a copy in `~/.cache/bb-adc-kicad`, firmware recorded with `bb flash`; `scripts/adc_kicad.sh`): `board.bbm` current; equal to its one-cell reference and to `tests/golden/adc-kicad/outputs.sha256`; the `.net` and `kicad-cli` routes byte-identical (when `kicad-cli` is installed); board.cir's committed non-grant records byte for byte and `nets.csv`/`nets.vcd` within 1 mV (2 samples differ by 1 µV: C1's `470n` against `4.7e-7`, which the script checks byte for byte); `bb run` writes nothing into the project. `make adc-kicad-prepare` makes the copy that `pool-equiv` and `cost-table` run |
| `make kicad-import` | Phase 1 M4 gates M4b-M4f on the dial-me-spinner main board at 6854d25 (`scripts/kicad_import.sh`, about 20 s; needs KiCad 10 in WSL and the Windows `kicad-cli.exe` unless `KICAD_WINDOWS=0`): 201 parts, 142 nets, 657 pin nodes; the resolution counts; one `board.bbm` from the committed netlist, fresh Linux and Windows exports and a partial KiCad 10 re-save (dc96515's KiCad 9 netlist as the secondary check); byte-identical re-imports; unchanged sources; the version refusals. Not in `make test` (no KiCad in CI); `cli/tests/kicad_import.rs` (the two-board fixture) is |
| `make dms-system` | Phase 1 M11 gate (ADR-007 D5, D7, Amendment 5; in `make test`, about 30 s): `examples/dial-me-spinner/system` (`scripts/dms_system.sh`, copies in `~/.cache/bb-dms-system`): a fresh import gives the committed `board.bbm`, twice byte-identical; main 201/142/657 and base 21 parts, 17 nets, 81 pin nodes; the 15 shared references; each board its own ground; the 12 pins mated through `dial-me-spinner/link-harness@0.1.0:wires`, the harness locked; the `kicad =` route on both boards (skipped and reported without `kicad-cli`); dc96515's base netlist (reported: J1's S1/SH); E1's targets, bindings and benches on the system elaborate to E1's own deck with qualified names, the base side of J1 warned as open. `cli/tests/system.rs` (cargo test): a bare U1, U2, J1 or TP1 is an error listing both candidates in every place a name is taken; the connection and harness errors |
| `make dms-rail` | Phase 1 M6 gate (plan section 2 E1; ADR-008 Amendment 3, ADR-007 Amendment 3): `examples/dial-me-spinner` with the prebuilt Zephyr firmware (`led_rail.elf`, `led_rail_wrong_order.elf`) at `bench/nominal.toml` and `bench/cold.toml`, each run on a copy of the project in `~/.cache/bb-dms-rail` (`scripts/dms_rail.sh gate`, about 3 min fresh): `scripts/check_dms_rail.py` (the correct order passes LED-DATA-V4, LSW-3, LSW-2 and FW-6.4, the firmware rule's timing from the `pin_edge` records; the wrong order fails exactly LSW-3), every case equal to its one-cell reference (`--max-quantum 10us`) except grants and to `tests/golden/dms-rail`, analog-first byte-identical, 120 ms in under 30 s at both engine orders (board-level convergence), the `PIN_CNF` audit clean and its two negative controls, 10-run determinism. `make dms-rail-test` (in `make test`) is the four default runs with the checks, hashes, VLED snapshots, audit and one analog-first run, and two FW-6.4 negative controls (ok-cold's `pin_edge` records moved: each fails exactly FW-6.4); `make dms-rail-update` rewrites the goldens |
| `make dms-crosscheck` | Phase 1 M6, local only (needs `DMS_REPO` and `make dms-local-packages`): the run's `pin_edge` records replayed into the suite's own `led_rail_sequence` deck with the same bench supply (`scripts/dms_crosscheck.sh`, `scripts/dms_suite_deck.py`); VLED within 5 mV of the co-simulation with the LOCAL-ONLY vendor-derived view, both benches; the fitted view and the suite's battery-network deck reported. Decks stay in `~/.cache/bb-dms-crosscheck` |
| `make dms-wfw-test` | Phase 2 W-fw (ADR-009 Amendment 7; not in `make test`, CI job `fw-rebuild`, about 30 s on 4 jobs): the Renode-only test of the W1 and W2 images' POWER and GPIO configuration (`examples/dial-me-spinner/firmware/bb_wfw_test.py`): PIN_CNF of the enables, POFCON at V28, each W1 build's blanking against the LED_PWR_EN edge, the cue held or shed with the stress corner's 2.94 V window as the comparator input (the 4 ms control and blanking off shed it, 10 and 5 ms hold it), W2's RESETREAS read and OLED rule; every case twice, byte-identical. Since P4 it runs on Breadboard's POWER model `BbNrf52Power` (journalled by `power_journal.py`; the comparator input is the model's `#pofwarn`; ADR-009 Amendment 8) |
| `make nrf-power-model` | Phase 2 P4 (ADR-009 D6, G7, Amendment 8; in `make test`, about 15 s): the Renode-only register and timing tests of Breadboard's nRF52840 POWER model `BbNrf52Power` (`nrf52840-bb@4`; `examples/nrf-power/bb_power_test.py`, ADR-005's test-plan shape): reset values, every POFCON.THRESHOLD code and its `#pofen`/`#pofthr0..3` outputs on the state block's ring, the refused codes (0-3 with POF, THRESHOLDVDDH), EVENTS_POFWARN on a rising `#pofwarn` and its interrupt, the "already below at enable" rule, RESETREAS, GPREGRET, the wrapped `NRF_CLOCK`, NVMC erases blocked by POF; every group twice, byte-identical. The comparator's thresholds and hysteresis are the package view `nordic/nrf52840-qiaa:pof` (`bb pkg test`, in `make packages`) |
| `make dms-w1` / `make dms-w1-test` | Phase 2 P4 gate (plan section 2 W1; ADR-009 D6, G4, G5, decision D-W1, Amendments 2 and 8; `dms-w1-test` in `make test`, about 10 s): W1 with the firmware in the loop on `examples/dial-me-spinner/w1` (`simulate = ["main", "base"]`, the BQ25180, RT9080 and U2 as `pi` views, U10's POWER model and comparator) at the LABELLED STRESS CORNER (`bench/w1-stress.toml`: 55 mA static +3V3 load, suite budget 50 mA, RT9080 worst_dropout, VPOFTOL +5 %), each case on a copy in `~/.cache/bb-dms-w1` with its image recorded by `bb flash` (`scripts/dms_w1.sh`, `scripts/check_dms_w1.py`, `asserts/w1.toml`, `asserts/w1-margin.toml`): the rule firmware passes cue-held, pof-dip, flash-vcc, no-reset with no POFWARN and the static-load tolerances against the suite at 55 mA; the 4 ms and blanking-off controls fail exactly cue-held (POFWARN at ceil_L of the re-enable, and within one lattice cell of the window's start); the margin check at 50 mA (`bench/w1-margin.toml`, no POFWARN, both builds); reported: 5 ms, the nominal and -5 % corners, the two co-simulations. `make dms-w1` adds partition independence (10 us, 1 ms, 10 ms grants), the references against `tests/golden/dms-w1`, analog-first, 10-run determinism, `dms-wfw-test` and `nrf-power-model`; `make dms-w1-update` rewrites the goldens |
| `make dms-w2` / `make dms-w2-test` | Phase 2 P5 gate (plan section 2 W2; ADR-009 D7, G4, G5, Amendment 9; `dms-w2-test` in `make test`, about 3 s): the S4 brown-out on `examples/dial-me-spinner/w2` (`simulate = ["main", "base"]`, the power path, U21 and the averaged TPS61040 as `pi` views, U10's brownout reset detector: the `bor` output of `nordic/nrf52840-qiaa:pof`) in static-load mode (USB at ILIM 100 mA, dead cell, the suite's 50 mA +3V3 load), each case on a copy in `~/.cache/bb-dms-w2` with its image recorded by `bb flash` (`scripts/dms_w2.sh`, `scripts/check_dms_w2.py`, `asserts/w2-old.toml`, `asserts/w2-rule.toml`): the old rule's hold and release at `ceil_L` of #bor's crossings with their `reset` records, every output and `#oe` low while held, OLED_VCC_EN through R25, RESETREAS 0 at the reboot, the VBOR crossing against the suite at 1.57, 1.60 and 1.63 V (500 µs); the rule: no reset, +3V3 >= 2.7 V, the suite's minima (10 mV, 20 mV); the plan's 200 ms of each with the brown-outs counted (one: the loop does not repeat on these views, a finding for the approver). `make dms-w2` adds partition independence (10 us, 1 ms, 10 ms grants), the references against `tests/golden/dms-w2`, analog-first, 10-run determinism; `make dms-w2-update` rewrites the goldens. The brown-out's engine gate is `nrf_bor_hold_and_release` in `engines/cpu-renode/tests/nrf52840.rs` (`cargo-test`) |
| `make fw-rebuild` / `make dms-firmware` | ADR-008 D5 (M6, M8, M10; Phase 2 W-fw): rebuild the E1, E2 and E3 images (with E3's negative control `homing_fast.elf`) and the W1 and W2 images (`w1_rule`, `w1_blank4`, `w1_blank5`, `w1_blank_off`, `w2_rule`, `w2_old_rule`) in a FRESH west workspace from `examples/dial-me-spinner/firmware/west.yml` (Zephyr v4.5.0, SDK 1.0.1 in `~/.local/opt`) and require the image hashes of `firmware/prebuilt/SHA256SUMS`; `dms-firmware` rebuilds the prebuilt images (`scripts/dms_zephyr.sh`; `make setup SETUP=zephyr` installs the SDK and host tools). Never uses a developer's workspace |
| `make cost-table-dms-rail` | The M6 cost-table row alone (E1 cold, one-cell vs defaults, fresh and on a 1-slot pool); `make cost-table` includes it. Run on an idle host |
| `make dms-vsense` | Phase 1 M8 gate, second half (plan section 2 E2; ADR-008 Amendment 5): `examples/dial-me-spinner` with `vsense.elf` (a reading every 1 s) and `vsense_50ms.elf` reading VSYS_SENSE through the SAADC with the loading `adc_in`, at `bench/vsense.toml` (the suite's sampling worst case) and `bench/sag.toml` (the battery ramps down through the LED shed), each run on a copy of the project in `~/.cache/bb-dms-vsense` (`scripts/dms_vsense.sh gate`, about 14 min fresh): seven cases (s1, s50, c5 with C5 at 10 nF, their `load = "none"` twins, sag) at bb's defaults and as their every-lattice-point references (`--lookahead off --adc-push every`); `scripts/check_dms_vsense.py` (the per-sample error against the `load = "none"` run and the suite's charge-sharing arithmetic, which must also reproduce the suite's 356/761 uV and 3.47 mV at its corner; codes; C5 / 10; the shed within one burst period of the threshold crossing; no `stop:`); defaults equal to the references except grants, the references equal to `tests/golden/dms-vsense`; analog-first byte-identical; the TACQ refusal; 10-run determinism (sag). `make dms-vsense-test` (in `make test`, about 1 min) is s50, c5 and their twins with the checks and hashes; `make dms-vsense-update` rewrites the goldens; `make cost-table-dms-vsense` is its cost-table row (idle host) |
| `make pi-idd` / `make pi-idd-test` | Phase 2 P3 (ADR-009 D4, gate G6, Amendment 6; `pi-idd-test` in `make test`, about 6 s): the nRF52840's `supply_load` view `idd` (`[power] idd` of `nordic/nrf52840-qiaa` 0.3.0) and the CPU state outputs `#run`, `#hfxo`, `#nvmc`, `#saadc` (`bb_nrf52_state.cs`, mapped at 0x5FFE0000 only when bound) on `examples/nrf-idd` (`scripts/pi_idd.sh`, `pi_idd.py`): `nrf_duty.elf`'s total VDD current against the hand calculation of the duty-weighted average within 1 % (and with a bench `[[activity]]` radio), its negative control at fast, the nRF feedback firmware with the state outputs at most 2x the board's grants, the defaults against `tests/golden/pi-idd`. `pi-idd-test` also runs static-load mode (`bench/pi-static.toml`, `[[load]] replace`). `make pi-idd` adds `nrf_state_outputs_exact` (`#run` at the WFI instants of `nrf_state.elf`, to the fs), one-cell / 1 ms / default equality, analog-first and a second run; `make pi-idd-update` rewrites the hashes; `make pi-idd-cost` (measurement lock) is the state outputs' cost row against static-load mode |
| `make dms-values` | ADR-006 G3: `bb pkg value --kicad` on the 29 distinct passive values of the dial-me-spinner main board equals the suite's `spicelib.parse_value` (re-derived from `git show 6854d25:hardware/sim/spicelib.py` when `DMS_REPO` exists, else the committed fixture `tests/fixtures/values/dms-main.json`) |
| `make fmt` / `make fmt-check` | `cargo fmt --all` (`-- --check` in CI) |
| `make clippy` | `cargo clippy --workspace --all-targets -- -D warnings` |
| `make check` | `fmt-check`, `clippy` and `test`, which is what CI runs |
| `make worktree TRACK=<name>` | Phase 2 H0 (plan section 7 rule 1): the track's git worktree `~/src/bb-<name>` (`WORKTREE_ROOT`) on the branch `p2/<name>`, from `main` when the branch is new, locked against a `git worktree prune` from Windows, with its own git-ignored `local.mk` (included first by the Makefile): `CARGO_TARGET_DIR` `~/.cache/bb-target-wt-<name>`, `BB_SCRATCH` `~/.cache/bb-wt-<name>` (the gates' project copies and work directories) and `DMS_REPO`. Idempotent; `make worktree-remove TRACK=<name>` removes it and keeps the branch (`scripts/worktree.sh`) |
| `make worktree-test` | H0 gate (not in `make test`; four release builds of `bb`): two throwaway worktrees build `bb` at the same time, and their crate hashes are compared (bbcore's rlib differs by a marker only A has; bbpkg's rlib and each `bb` carry their own package path, never the other's); negative control: built one after the other into one shared target directory, B's `make bb` compiles nothing and leaves B a binary with A's code and package path (cargo names sources by their workspace-relative path) (`scripts/worktree_test.sh`) |
| `make plugin-scope` / `make lock-scope` | Phase 2 H4 gates (ADR-015 G5, D6; not in `make test`, H4's budget is none). `plugin-scope` (about 6 s; CI job `adc`): with Renode's own compiler, each platform's declared plugin subset (`bb_cpu_renode::PLATFORM_PLUGINS` plus `SHARED_PLUGINS`) compiles alone, the whole assembly has no module initialiser (its metadata read), and two negative controls (a source using another's type, a `[ModuleInitializer]`) are caught (`engines/cpu-renode/tests/plugin_scope.rs`). `lock-scope` (about 50 s): a second `bb` built from a copy of the tree with one edit to `bb_nrf52_saadc.cs`, on scratch copies of every `PKG_BOARDS` entry on the committed locks (relocked under the D6 rule in milestone S, so a stock relock of each copy gives the committed `bb.lock` and `board.bbm` byte for byte): every STM32 board and project stays current, every nRF52840 one has exactly its platform's drift (`scripts/lock_scope.sh`). The unit form is `platforms::tests` in `cargo-test`; G6 (the package path) is `cli/tests/lock.rs` |
| `make schema-epoch` | Phase 2 S gate (the schema epoch; not in `make test`, about 4 min, most of it building the base commit's `bb`): every committed board file and every project with each of its benches elaborates (`bb run --elaborate-only`; a board file writes the analog engine's netlist) to the same deck from `EPOCH_BASE` (main before the epoch, `git archive`) and from this tree: the base `bb` and this `bb` on the base tree byte for byte, and this `bb` on the base tree and on this tree byte for byte up to the first-party view renames of ADR-011 D11 (`scripts/schema_epoch.sh`, `scripts/schema_epoch_decks.py`, with two negative controls and a count of every rename compared) |
| `make measure-lock-test` | H0 gate (in `make test`, a few seconds): on a private lock file, every target of `MEASURE_TARGETS` refuses while a probe holds the measurement lock (the hold checked from outside), before building anything; the probe runs before and after (`scripts/measure_lock_test.sh`) |
| `make ngspice-stall` | Phase 2 H1 and H2 gate (ADR-015 G1, D1, Amendment 4; nightly tier (`make nightly`, `nightly.yml` since H6), not in `make test`; under the measurement lock, fresh Renode). Part 1 (H1), with the lattice snap off (`--ngspice-lattice-snap off`, the step clamp before H2): the SAADC stall repro, `e3/board-glued.cir` (E3's typ board as it was before H5, the MOT nets glued; ADR-014 Amendment 1) with `U10.P0.02` on the loading `saadc_se` view and `homing_step.elf`, 100 ms, ends in `NoProgress:ngspice at 77430000059213fs` (77.43 ms) in the cell (77.43 ms, 77.44 ms] within `STALL_MAX_S` (120 s) and with bb's peak RSS under `STALL_MAX_RSS_KB` (500 MB); the outputs written before the error equal a run that ends at the last completed grant (`nets.csv`, `nets.vcd` and the non-grant records byte for byte; the grant records add the failed grant's own; since H5 that grant is the quiet grant (67.94 ms, 77.94 ms], so the run is `--for 67940us`, read from the stalled run); `--ngspice-watchdog 20000,10000` trips identically twice; negative control: the committed board (`load=none`) runs the same 100 ms clean. Part 2 (H2), with the snap on (bb's default): the repro completes 300 ms at `--renode-quantum` 5, 10 and 20 us and 1 s at 10 us; the committed E3 board (`board-glued.cir`) completes 1 s; its negative control, with the snap off, stalls at 502.47 ms (`scripts/ngspice_stall.sh`; the untimed runs `STALL_JOBS` = 3 at a time; 1 min 46 s on an idle host) |
| `make dms-home-rss` | Phase 2 H1 gate (ADR-015 G2, D2): E3 `typ` (2.7 s) at bb's defaults with bb's peak RSS (`bb run: peak RSS`, `VmHWM`) at most `DMS_HOME_RSS_MAX_KB` = 403 627 kB, the full-length measurement with the engine's `.save` (336 356 kB, against 1.5 GB without) plus 20 %, and its committed hashes (`scripts/dms_home.sh rss`, about 4 min alone); `make dms-home` checks the same on its `typ` run |
| `make baseline` | H0: the runtime baseline. `make check` (`BASELINE_ARGS`) `BASELINE_N` = 3 times after one uncounted warm-up, on an idle ext4 worktree, under the measurement lock; wall times, load before each run and `make test`'s per-step times (`out/test-timing.tsv`, written by every `make test`) in `out/baseline` (`scripts/baseline.sh`). Its result is the table in "Gate budgets" |
| `make nightly` | Phase 2 H6 (ADR-015 D11, Amendment 5): the nightly tier, by name, the same targets as `.github/workflows/nightly.yml` (one job each, daily on `main` and by hand): `nightly-fresh` (`make test POOL=0`, CI's former `fresh` job; its step times in `out/nightly-fresh-timing.tsv`), `make ec-long`, `make ngspice-stall`, the full `make dms-home` and the full `make dms-vsense`. Every step runs even after a failure; wall times in `out/nightly-timing.tsv` (`NIGHTLY_STEPS`, `NIGHTLY_TIMING`). About 27-30 min alone. A nightly failure blocks the next milestone merge |
| `make ci-jobs-local` / `make setup-cache-cost` / `make workflows-check` | Phase 2 H6 (ADR-015 D11, Amendment 5): CI runs only on a push, so the PR tier is measured here first. `ci-jobs-local` (measurement lock): every job of `ci.yml` (`CI_JOBS_WORKFLOW`) with its `run:` steps in one shell, timed, a release `bb` of this checkout first on `PATH` as `make setup` installs it (`scripts/ci_jobs_local.sh`; `CI_JOBS` names jobs, `CI_JOBS_SKIP` defaults to `fw-rebuild`, which needs the network; outputs in `out/ci-jobs-local`). `setup-cache-cost` (measurement lock): what the bb-setup action's cache of the installed toolchain saves, each part of `scripts/setup.sh` it restores (Rust, Renode, uv) installed both ways, `SETUP_CACHE_N` interleaved rounds, medians (`scripts/setup_cache_cost.sh`). `workflows-check` (a few seconds): the workflows and the bb-setup action parsed and checked structurally, every `make <target>` a Makefile target (`scripts/check_workflows.py`; actionlint is not installed). None is in `make test` |
| `make bench-test` / `make bench-gate` | Phase 2 B1 (ADR-010 D1-D4, Amendment 1): `bb-bench/1` and the bench engine on `examples/bench` (a synthetic STM32F103 project with examples/feedback's firmware; a copy in `$(BB_SCRATCH)/bb-bench`, `scripts/bench_gate.sh`). `bench-test` (in `make test`): seven cases at bb's defaults through `scripts/check_bench.py` (the switch view's analytic RC settling and open-state divider, G8; the current profile; the analog playback equal to its CSV; the faults; the seeded bounce; the sets; the triggers' times; a digital playback into U1.PB0 with the edge times of an equivalent `[[pwl]]` gives the same edges to the fs and the same response, applied at `ceil_L`), the committed one-cell reference hashes (`tests/golden/bench`), G2 (a 601-point playback equals the same points as an inline `[[pwl]]`), eight refusals, and G1 (the Phase 1 benches as `bb-bench/0` and `/1` build byte-identical decks; `bb pkg migrate --bench`). `bench-gate`: that, plus every case at `--max-quantum` 10 µs, 1 ms and 10 ms and `--engine-order analog-first`, and determinism (about 6.5 min). `make bench-update` rewrites the goldens from one-cell references |
| `make bench-perf` | Phase 2 B1 cost gate (measurement lock; idle host): `bench/playback-long.toml` (1 s, 100 001 points every 10 µs) against `bench/playback-base.toml` (AIN on a constant supply), interleaved, N = `BENCH_PERF_N` (5), medians: the playback adds at most 2.0 s of wall time |
| `bb test <file> --out <dir>` / `make bb-test-gate` / `make bb-test-cost` | Phase 2 B3 (ADR-010 D8-D9, G4, G5, G9, Amendment 5; `spec/test.md`): a `bb-test/0` file of cases, each a `bb run` (a project's image by `bb run --image <instance>=<elf>`, a case's assertions from its bench, `assert_files` and inline `[[case.assert]]`), with `expect` (negative controls; G9: one needs a `count` or `event` assertion expected to pass), `golden` (`nets.csv`, `nets.vcd` and the records without grant and assert records), `reference = true` (the one-cell run: "equivalence", or "consistency" when the partitions coincide) and `same_as`; cases in parallel (`--jobs`) on the pool in `BB_RENODE_POOL` (`--pool auto` starts one, `env`, `off`); writes `test.json` (canonical, deterministic: `spec/schema/test-result.schema.json`) and `junit.xml` (`spec/schema/junit-subset.xsd`, wall times only here); exit 0 iff every outcome is as expected. `make dms-rail-test`, `dms-vsense-test` and `dms-home-test` run `examples/dial-me-spinner/tests/{e1,e2,e3}.toml` (E2's one-edit variants are the committed projects `e2/{none,c5,c5-none}`, written by `scripts/dms_e2_projects.py`) and then, on `bb test`'s outputs, what is not a kind (`scripts/dms_<x>.sh test`; the Phase 1 runs stay as `test-scripted`, used by the full gates). `bb-test-gate` (`scripts/bb_test_gate.sh`, about 15 min): bb test's semantics and negative controls on the Phase 0 boards (`tests/bb-test`), the G9 refusal, 10 runs of each dms-* file with `test.json` byte-identical (the tenth fresh), `scripts/check_bb_test.py` on every run, and `scripts/junit_reporter_check.sh` (dorny/test-reporter v3.2.0's java-junit parser and report, the commit CI pins, in a node container; podman, the network once) on every `junit.xml`. `bb-test-cost` (measurement lock, idle host): the three targets against their Phase 1 scripted runs, interleaved, medians (`scripts/bb_test_cost.sh`) |
| `make assert-test` / `make assert-gate` | Phase 2 B2 (ADR-010 D5-D7, G3, G4, G6, Amendment 3): `[[assert]]` (`spec/project.md` section 3.4), evaluated by the kernel over the engines' accepted solver points (`bbcore::assert`, `Engine::solver_points`) and the records, one `assert` record per assertion at its window's close; `bb run --assert <file>` (an assertion file, for projects and board files) and `bb assert <run>` (the record-only kinds again on a finished run). The ported checks are assertion files: `examples/dial-me-spinner/asserts/e1.toml`, `asserts/e2/<case>.toml` (`scripts/dms_asserts.py` writes them from `check_dms_vsense.py`'s results) and `e3/asserts/<corner>.toml`; `scripts/check_asserts.py` compares every record with the script it ports on the same run (verdicts equal; values exact where both read the same data, the stated bounds for an extremum and a crossing between the scripts' samples, Amendment 3 A3.6). `assert-test` (in `make test`; copies in `$(BB_SCRATCH)/bb-assert-*`, `scripts/assert_gate.sh`): E1's four cases with the assertions against `check_dms_rail.py` (the wrong order fails exactly LSW-3), the committed hashes hold with the assertions on (passive), `bb assert` gives the run's records again, the FW-6.4 negative controls (moved `pin_edge` records) fail exactly FW-6.4-on and -off, a bench's own `[[assert]]` equals an `--assert` file, five refusals (G6: an ideal-source net, a branch current; an analog check on a pin, an unknown kind, a stimulus key in an assertion file); `dms-vsense-test` replays E2's code assertions on its four runs. `assert-gate` (about 20 min): that, E1 at `--max-quantum` 10 µs and 1 ms and analog-first and 10-run determinism (the same verdicts), E2's eight cases with the assertions (and s50, sag as their every-lattice-point references: the same verdicts; `dms_asserts.py` rewrites `asserts/e2` byte for byte), E3's typ, worst and park and dms-home's negative controls (neg-fb200 fails exactly HOME_N-tS, neg-fb286 HOME_N-tS and HOME_N-latency, neg-fast none) |
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
  and any later gate over its budget. Since H6 it is `make nightly` and
  `.github/workflows/nightly.yml` (with `make ngspice-stall`, nightly since
  H1); see "H6: the tiers, measured" below.

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
| H1 | +30 s | under 1 s (the new unit and lint tests); `packages`' operating-point checks not measurable (`bb pkg test` A/B: median 31.4 s against 34.0 s before H1, N = 5, at load 23-60) | `make ngspice-stall` 3.4 min (the repro 77.9-120.6 s under load: 2-minute bound on an idle host; ADR-015 Amendment 1); `make dms-home-rss` 8.6 min; `make dms-home` 62 min (both beside other gates) |
| H2 | — | 0.6 s (the `lattice` engine tests with `quiet_tmax_steps_take_no_micro_step`); no gate's runtime changes measurably (`make check` green pooled 1 049 s and `POOL=0` 1 313 s, both beside `make dms-home` / `make dms-vsense`) | `make ngspice-stall` 1 min 46 s idle (H1's repro 23.9 s, H2's completion runs 3 at a time); `make dms-vsense` 20.8 min and `make dms-home` 37.3 min beside other gates (ADR-015 Amendment 4) |
| H3 | +10 s | 8.9 s (`ec_exposure` in `cargo-test`: 8.93 s pooled, 8.44 s `POOL=0`, in idle `make check` runs of 481 s and 581 s; unit tests under 0.1 s) | `make ec-exposure` 8.2 s; `make ec-long` 130 s; `make pool-equiv` 26.7 min; `make jit-profile-stress` 194 s (ADR-015 Amendment 2) |
| H4 | — | under 1 s (`platforms::tests`, `packages_directive`, three tests in `cli/tests/lock.rs`: 0.17 s for the file); `plugin-scope` and `lock-scope` are outside `make test` | `make plugin-scope` about 6 s (4.2 s of Renode compiles); `make lock-scope` 47 s (20 s of it the second build) |
| H5 | +30 s (`multi-producer`) | `multi-producer` 19.6 s in a pooled `make check` (18 min 16 s in all) and 18.8 s with `POOL=0` (20 min 54 s), both beside other gates (load 5-20); `bbdev/tests/multi_producer.rs` about 4 s of it (also in `cargo-test`); `device-smoke` takes 21 grants instead of 37, no measurable change | `make dms-home` 11 min 34 s beside other gates (37-62 min before H5: `typ` 433 grants at bb's defaults, the one-cell references are the rest); `make dms-home-project` 9 min 17 s; `make dms-home-g0` about 25 min (N = 3 with the base case); ADR-014 Amendment 1 |
| H6 | — (removes: `fresh` to nightly, `adc-stock`, split `cargo-test`) | -5.5 s (`adc-stock` gone; no step added): `make check` 475 s idle (N = 3, `make baseline`) against H0's 470 s; green pooled 756 s and `POOL=0` 729 s after merging W-fw, beside another track (load 6-25) | `make nightly` 1 775 s and 1 627 s (green twice); `make ci-jobs-local` 1 341 s (all seven jobs green, slowest `dms-rail` 528 s under load 14); `make setup-cache-cost` 44 s; `make pool-equiv` 902 s (34 configurations); `make workflows-check` under 1 s (ADR-015 Amendment 5) |
| S | — | under 1 s (`bbpkg/tests/epoch.rs`: 0.01 s; the `packages` step 8.4 s against 7.2 s at the baseline, with `--publish` lint and the new schemas) in a pooled `make check` of 453 s | `make schema-epoch` 39 s with the base `bb` built, about 4 min from a cold base target |
| B1 | +40 s | `bench-test` 5.0 s in an idle pooled `make check` (456 s in all, load 1.2-5.5); alone 10.6 s pooled (with the pool's start), 15.2 s `POOL=0`; in a `make check` beside other tracks (load 4-26) 20.6 s and 27.1 s | `make bench-gate` 6.5 min (load 30-60); `make bench-perf` 31 s (idle: the 1 s, 100 ksps playback adds −0.01 s) |
| B2 | +20 s | `assert-test` 9.8 s in a pooled `make check` (908 s in all) and 18.1 s with `POOL=0` (1021 s in all), both beside other tracks (load 8-50, so to be re-measured idle); the E2 code replay in `dms-vsense-test` about 1 s; the unit tests (`bbcore::assert`, `cli` `asserts`) under 0.1 s | `make assert-gate` 23 min 52 s pooled (load 8-50; E3's six homing runs are most of it) |
| B3 | +0 s (replaces scripts) | -14.4 s (`make bb-test-cost`: the three dms-*-test targets as bb test files 108.2 s against their Phase 1 scripted runs 122.6 s, N = 3 interleaved medians on a 5-slot pool, load 12-19 from other tracks, so to be re-measured idle; in a pooled `make check` of 1045 s: dms-rail-test 4.6 s, dms-vsense-test 30.3 s, dms-home-test 36.0 s); `cli` unit tests under 0.1 s | `make bb-test-gate` 860 s pooled (load 6-18); `make bb-test-cost` 751 s |
| R1 | +15 s | +4.8 s (`project-step` beside `step` in `dms-home-test`; N = 3 under load 16-48 from other tracks, so to be re-measured idle) | `make dms-home-project` 9 min 0 s (same load) |
| R2 | +30 s | `dms-e1-system-test` 3.4 s in a pooled `make check` (1 234 s in all) and 38.7 s in a `POOL=0` one (1 485 s), both beside another track's `make check` (load 7-46, so to be re-measured idle); 15.8 s alone with its own pool's start; `cli/tests/system_run.rs` 1.3 s | `make dms-e1-system` 93 s pooled, 169 s `POOL=0`; `make pool-equiv` 57 min (32 configurations); `make cost-table-dms-e1-system` 5 min (ADR-013 Amendment 3) |
| P0 | — (spike) | 0 s (`make pi-spike` is not in `make test`) | `make pi-spike` 84 s (85 runs, 8 jobs, host load 0.3-3.9; before D-W1 102 s, 82 runs, load 8-11) |
| P1 | +30 s | `pi-static-test` 13.9 s in a `POOL=0` `make check` of 557 s (load 5), 20.6 s in a pooled one of 837 s (load 21-25); the new `bb pkg test` vectors and corner operating points 0.6 s; `cli/tests/fidelity.rs` 0.8 s | `make pi-static` 18 s (10 runs on 8 jobs, W2 old the long pole); `make dms-vsense` 12.9 min (E2's sag and rec re-recorded, ADR-009 Amendment 4) |
| P2 | +15 s | `pi-glue-test` 5.9 s pooled, 11.9 s `POOL=0` (`make check` runs of 1 009 s and 1 076 s beside other tracks, load 7-15; earlier 8.0 s and 9.7 s); the two `nrf_oe_*` Renode tests in `cargo-test` 17-21 s of their own, in parallel with the `nrf52840` binary's others; the `cli` unit test under 0.1 s | `make pi-glue` 2.0 min (load 8-15); `make pi-oe-cost` B / A 1.000 (N = 9, load 7-12; ADR-009 Amendment 5) |
| P3 | +30 s | `pi-idd-test` 4.3 s pooled, 7.2 s `POOL=0` (`make check` runs of 490 s and 582 s, load 6-7); `nrf_state_outputs_exact` in `cargo-test` about 17 s of its own, in parallel with the `nrf52840` binary's others (31.4 s and 52.9 s for the binary) | `make pi-idd` 69 s; `make pi-glue` 60 s; `make lock-scope` 63 s; `make pool-equiv` 23.4 min (34 configurations); `make pi-idd-cost` B / A 1.34 (N = 5, load 1; ADR-009 Amendment 6) |
| W-fw | — | 0 s (nothing added to `make test`) | `make fw-rebuild` 419 s (thirteen images, fresh workspace); `make dms-wfw-test` 30.5 s (24 runs, 4 jobs; CI `fw-rebuild`; ADR-009 Amendment 7) |
| P4 | +40 s | +13.2 s pooled, +17.9 s `POOL=0` (`nrf-power-model` 11.3 s / 11.3 s, `dms-w1-test` 1.9 s / 6.6 s; `make check` 8 min 13 s pooled and 10 min 5 s `POOL=0`, load 1.7-6, with H6 merged); the `pof` vectors in `packages` about 0.5 s | `make dms-w1` 61 s pooled (with `dms-wfw-test` 22.5 s and `nrf-power-model` 11.1 s); `make pool-equiv` 11 min 0 s (38 configurations, the four W1 ones among them: every pooled run leased and byte-identical to its fresh run and to the committed hashes, 76 leases by one Renode process) (ADR-009 Amendment 8) |
| P5 | +40 s | +5.5 s pooled, +10 s `POOL=0` (`dms-w2-test` 2.0 s / 5.8 s; `nrf_bor_hold_and_release` +3.1-4.3 s on the `nrf52840` binary, 8.8 s alone; `make check` 502 s pooled and 604 s `POOL=0`, load 1.2-7); the `bor` vectors in `packages` about 0.3 s | `make dms-w2` 39 s pooled; `make pool-equiv` 11 min 13 s (41 configurations, W2's old rule with its brown-out and the rule among them) (ADR-009 Amendment 9) |
| E1 | +20 s (local registry fixture) | 7.1 s pooled, 9.6 s `POOL=0` (`registry-test` in `make check` runs of 1096 s and 1161 s beside other tracks, load 4-36) | `make registry-gate` 34 s; `make test-offline` 978 s (ADR-011 Amendment 2) |
| E4 | — (`vscode-templates`, under 1 s) | 0.1 s (`vscode-templates`) | `make vcd-check` 16 s with the demo (CI `dms-rail`); `make vscode-test` 116 s, local (ADR-016 Amendment 1) |

This build adds at most 350 s to `make test` (plan section 8, without
R3's 10 s; B4, R3, P6, E2, E3 and E5 are not in this build). Section 5
item 5's target allows the baseline + 4 minutes (240 s), so the build
needs at least 110 s of H6's named savings.

**H6: the tiers, measured** (ADR-015 Amendment 5). The milestones'
measured additions were far below their budgets, so the named savings
were not needed to meet the local target:

- **`make check` on an idle ext4 worktree: 475 s (7:55)** after H6
  (`make baseline`, N = 3: 475.0, 475.4, 474.6 s, load 1.7-1.9 before
  each; on `7bb202f`, W-fw since added nothing to it), against H0's 470 s
  and the target of **710 s**. Phase 2's new `make test` steps add 48.1 s,
  the dms-*-test steps take 22.9 s less than at H0, H6 removed
  `adc-stock` (5.5 s).
- **The PR tier's CI jobs, by their local equivalents** (`make
  ci-jobs-local` on `19dbea1` with H6, setup aside, beside another
  track's `make test`): `dms-rail` 528 s (380 s at lower load), `test`
  410 s (without `cargo-test`), `cargo-test` 179 s, `feedback` 135 s,
  `adc` 55 s, `partition` 27 s, `verify-every` 7 s; `fw-rebuild` not run
  (network). H0's slowest CI job was `fresh` (13:58), now nightly; the
  slowest left is expected at about 8-11 minutes on CI, under 15.
- **The savings against H0:** `fresh` (CI 11:30 median) out of the PR
  tier; `cargo-test` (147-152 s) off the `test` job's critical path onto
  its own runner; `ec-long` (about 100-130 s) and `adc-renode` (about
  15 s) off the `adc` job (H0: 3:58); `adc-stock` 5.5 s off `make test`;
  `make setup`'s Rust, Renode and uv restored from a cache, 8.89 s to
  1.35 s locally per job (`make setup-cache-cost`, N = 3). The full
  `dms-home`, `dms-vsense` and `ngspice-stall` were in no PR-tier job at
  H0 and save nothing; the nightly tier gives them a CI run.
- **The nightly tier green twice** (`make nightly`, locally): 1 775 s and
  1 627 s (`nightly-fresh` 681 / 577 s, `ec-long` 99 / 97 s,
  `ngspice-stall` 51 / 49 s, `dms-home` 309 / 293 s, `dms-vsense` 636 /
  612 s).
- **The first push must confirm** each CI job's time on GitHub, the
  setup cache's miss then hit, and `nightly.yml` green (by hand, then
  twice on its schedule). Until then these are local equivalents.

## Repo map

```
bbcore/                 kernel: Time, PinSignal, Engine trait, Scheduler, EventSink, CsvWriter, VcdWriter;
                        board.rs (board.bbm types, ADR-007 D6), cjson.rs (integer-only canonical JSON);
                        Phase 2 B2: assert.rs (ADR-010 D5-D7: the assertion kinds, windows, the
                        kernel's evaluator over Engine::solver_points, and bb assert's replay)
importers/kicad/        bb-kicad (ADR-007 D1-D3): S-expression reader, the netlist model and its version
                        gates, schematic gates, the kicad-cli runner (on a temporary copy, never in the project)
bbpkg/                  component packages (ADR-006, spec/package.md): value grammar and integer units,
                        TOML reader, part.toml/pins.toml, lint, view files and deck assembly, test
                        vectors through NgspiceEngine, tree hashes and bb.lock, schema versions, SHA-256;
                        bexpr.rs (ADR-015 D3: B/E/G expressions and .func against ngspice 42's tables);
                        Phase 2 E1 (ADR-011): archive.rs (deterministic ustar, the strict reader),
                        registry.rs (index lines, the signed snapshot and serial, the cache, curl and
                        file:// transport, the Resolver), semver.rs (caret requirements)
packages/               first-party packages: generic/{resistor,capacitor,inductor}, st/stm32f103c8,
                        st/stm32f072rb; Phase 1 M5: ti/tps22917 (datasheet-fitted view), worldsemi/
                        ws2812c-2020, nordic/nrf52840-qiaa, raytac/mdbt50q-u1mv2 (a module);
                        Phase 1 M10: ti/drv8837 (the ideal-switch bridge view), dms/detent-mechanism
                        (a device package: `[behavioral] engine = "device"`, `dms/mechanism@1`)
                        Phase 2 P1: richtek/rt9080, ti/bq25180, ti/tps61040 (`pi` power views,
                        corners), cj/b5819w; ti/tps22917 restated to ADR-009 D2's rules; the
                        generic R/C/L, WS2812C and TPS22917 views valid at fast and pi
local-packages/         git-ignored, LOCAL-ONLY packages (make dms-local-packages: the vendor-derived
                        ti/tps22917); never committed
bbdev/                  the Phase 1 device API (ADR-006 D7, Amendment 6): `Device`, `Ctx`, the seeded
                        `Rng`, `DeviceEngine` (engine.rs; t_safe MAX, quiet_until = the earliest
                        timer, advance_until), the device table `DEVICES` and dms_mechanism.rs
                        (the detent mechanism and DRV5032, `dms/mechanism@1`); tests/scheduler.rs (G6)
bbbench/                Phase 2 B1 (ADR-010 D3, D4): the canonical bench CSV (csv.rs) and the bench
                        engine (engine.rs: digital outputs into MCU pins and triggered actions;
                        t_safe MAX, exact quiet_until, advance_until; identity bench@1)
engines/analog-ngspice/ ngspice shared-lib engine (libngspice.so.0); Phase 2 H1: the progress watchdog,
                        the engine-owned .save and operating_point (ADR-015 D1, D2, D4); bench tables
                        (TableSource: EXTERNAL V/I sources from a table with a cursor) and
                        Drive::Timed (B1)
                        examples/deck_run.rs: a SPICE body through NgspiceEngine with digital-glue edges (S1)
engines/cpu-renode/     Renode engine over the External Control API; platforms/: the plugin
                        assembly's sources (bb_lookahead.cs, the lookahead block, ADR-003 D5;
                        bb_stm32_timer.cs, STM32_Timer + TRGO; bb_stm32f0_adc.cs, the STM32F0
                        ADC model and its host port, ADR-005) and stm32f072-bb.repl (the F072
                        overlay); tests/firmware (gate firmware); console.rs, pool.rs,
                        manager.rs (the warm Renode pool, ADR-004); gdb.rs (the GDB proxy of
                        `bb run --gdb` in front of Renode's stub, ADR-008 D10, Amendment 7);
                        ec.rs (Phase 2 H3: the reverse External Control connection, bb's
                        owner-checked listener and the reconnect, ADR-015 D5, Amendment 2)
cli/                    `bb` binary (`bb run`, `bb pool`, `bb open`, `bb pkg`, `bb flash`, `bb probe`,
                        board-file parser, platforms.rs: the Renode platform identities of ADR-006 D8;
                        Phase 1 M4: project.rs (bb.toml, benches), names.rs, import.rs (resolution, D4),
                        elab.rs (bindings, analog scope, the deck, run.json);
                        Phase 1 M10: devices.rs (device parts' direct digital routes, the engine graph);
                        Phase 2 B2: asserts.rs; B3: test_cmd.rs (`bb test`, test.json, junit.xml))
examples/spike/         blinky firmware + board.cir (SPICE body + *bb pin bindings; U1 from the package
                        st/stm32f103c8 since M3, pinned in examples/*/bb.lock like every example)
examples/feedback/      feedback board (digital_in PB0 reads the RC that PA5 drives) + EXTI firmware and a polling variant
examples/feedback-open/ the same with the loop opened (acyclic, long grants)
examples/adc/           STM32F072 ADC board (adc_in PA1 reads the divider PA4 drives; board.cir,
                        board-open.cir), its firmware (adc_board.elf on Breadboard's ADC model,
                        test firmware), bb_adc_test.py (the ADR-005 model tests) and
                        renode_adc_test.py (the stock model's test, by hand since H6, and the
                        External Control client the model tests import)
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
                        dts/bindings/dial,detent-stepper.yaml); Phase 2 W-fw: firmware/w1_cue
                        (w1_rule.elf and the blanking builds w1_blank4/5/off.elf), firmware/w2_oled
                        (w2_rule.elf, w2_old_rule.elf), bb_wfw_test.py with power_journal.py
                        (make dms-wfw-test; ADR-009 Amendments 7, 8); Phase 2 P4: w1/ (W1 on the
                        system, simulate = ["main", "base"]: bench/w1-stress.toml, the labelled
                        stress corner, its -typ/-min comparator corners, w1-margin.toml, the two
                        co-simulation benches; make dms-w1) and asserts/w1.toml, w1-margin.toml;
                        Phase 2 P5: w2/ (W2 on the system: bench/w2-old.toml with its -min/-max
                        VBOR corners, w2-rule.toml; make dms-w2) and asserts/w2-old.toml,
                        w2-rule.toml
examples/dial-me-spinner/e3/ Phase 1 M10 (E3, route A): board.cir (typ), board-ts75.cir (worst),
                        board-park.cir (H1), board-step.cir (the whole detent sheet: the bridge
                        view copied verbatim, the coils, the suite's back-EMF), one bb.lock; all
                        written by scripts/dms_home_boards.py (make dms-home-test checks them);
                        Phase 2 H5: the homing boards' MOT nets digital-only (*bb digital,
                        ADR-014 D4), and board-glued.cir, typ as it was before H5 (make
                        ngspice-stall's deck);
                        Phase 2 R1 (ADR-013 D1): home/ and step/, the typ and step cases as
                        projects (bb.toml with M1 a virtual [[device]], bench.toml, board.bbm,
                        bb.lock), and packages/dial-me-spinner/detent-stepper (J40's motor
                        phases, views coils and step_emf; a project-local package)
examples/dial-me-spinner/asserts/ Phase 2 B2 (ADR-010 D10, Amendment 3): the ported checks as
                        assertion files (bb run --assert): e1.toml (E1's four checks), e2/<case>.toml
                        (E2's codes, shed and restore; written by scripts/dms_asserts.py); E3's
                        HOME_N latency is in e3/asserts/ (typ, worst, park); B3: e1-run.toml (the
                        PIN_CNF audit, the firmware's drive), e3/asserts/step.toml (the step's phases)
examples/dial-me-spinner/tests/ Phase 2 B3 (ADR-010 D8, Amendment 5): e1.toml, e2.toml, e3.toml, the
                        dms-*-test targets as bb test files
examples/dial-me-spinner/e2/ Phase 2 B3: E2's one-edit variant projects for tests/e2.toml (none:
                        load = "none"; c5: C5 at 10 nF, its netlist; c5-none), written by
                        scripts/dms_e2_projects.py; board.bbm, bb.lock
examples/dial-me-spinner/system/ Phase 1 M11 (ADR-007 Amendment 5): main + base as one project
                        (both vendored netlists, qualified names), [[connection]] main:J1 - base:J3
                        through the harness view of packages/dial-me-spinner/link-harness (a
                        project-local package); board.bbm, bb.lock (import only: simulate = main)
examples/dial-me-spinner/e1-system/ Phase 2 R2 (ADR-013 D2, Amendment 3): E1 on the system,
                        simulate = ["main", "base"] (E1's target and bindings, qualified);
                        bench/cold.toml and nominal.toml put the base board's power path in the
                        deck ([[battery]] at base:VBAT, ti/bq25180 at suite_compat, the harness)
                        in place of E1's VSYS supply; board.bbm, bb.lock
examples/dial-me-spinner/pi-spike/ Phase 2 P0 (ADR-009 Amendment 1): views/ (the first `pi` views:
                        RT9080_LDO_PI, BQ25180_PP_PI, TPS61040_BOOST_PI, restated from their
                        datasheets) and tolerances.toml (the option set and the static-load
                        tolerances that make pi-spike checks)
examples/dial-me-spinner/pi/ Phase 2 P1: static_load.toml, the W1/W2 static-load cases, scope, bench
                        and P0's recorded replica references (make pi-static)
examples/dial-me-spinner/bench/cold-pi.toml, cold-pi-ioh.toml (+ data/led_pwr_en_ioh.csv) Phase 2 P2:
                        E1's cold bench at pi (main:U1 excluded), and with the PS's 0.5 mA VOH
                        load on LED_PWR_EN (make pi-glue)
examples/nrf-feedback/  examples/feedback's circuit on the nRF52840 (Phase 1 M2; firmware
                        engines/cpu-renode/tests/firmware/nrf52840/nrf_feedback.c)
examples/nrf-idd/       Phase 2 P3: the nRF52840 supply_load board (U1 behind R1 10 Ohm from the
                        bench's VSUP; examples/nrf-feedback's loop), make_netlist.py, bench/
                        (fast, pi, pi-radio with a [[activity]])
examples/nrf-saadc/     Phase 1 M8: the loading adc_in board (AIN0 through the package's
                        saadc_se view, VDD, UARTE0 lines; firmware nrf_saadc_burst.c) and
                        bb_saadc_test.py (the SAADC model's Renode-only tests)
engines/cpu-renode/platforms/bb_nrf52_state.cs  BbNrf52State (0x5FFE0000): the CPU state outputs
                        #run, #hfxo, #nvmc, #saadc (ADR-009 D4, Phase 2 P3), mapped by the engine;
                        since P4 also BbNrf52Power's #pofen, #pofthr0..3 and its events
engines/cpu-renode/platforms/bb_nrf52_power.cs  BbNrf52Power (ADR-009 D6, Phase 2 P4): the POWER
                        registers (POFCON, EVENTS_POFWARN, RESETREAS, GPREGRET) around a private
                        NRF_CLOCK at 0x40000000 (`clock`), the #pofwarn input, host registers at
                        0x40000F00; nrf52840-bb.repl (@4) inlines the stock file's entries
examples/nrf-power/     Phase 2 P4: bb_power_test.py, the POWER model's Renode-only tests
                        (make nrf-power-model)
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
tests/golden/nrf-feedback/ one-cell reference hashes of examples/nrf-feedback (Phase 1 M2)
tests/golden/adc-kicad/ one-cell reference hashes of the examples/adc/kicad project (Phase 1 M4)
tests/golden/dms-rail/  one-cell reference hashes of E1's four cases and VLED snapshots (Phase 1 M6)
tests/golden/dms-vsense/ every-lattice-point reference hashes of E2's seven cases (Phase 1 M8)
tests/golden/dms-w1/    one-cell reference hashes of W1's gated cases and co-simulations (Phase 2 P4)
tests/golden/dms-w2/    one-cell reference hashes of W2's old rule and rule, 20 ms and 200 ms (Phase 2 P5)
tests/golden/dms-home/  one-cell reference hashes of E3's four cases (Phase 1 M10) and of its two
                        project cases, project-step and project-typ (Phase 2 R1)
tests/golden/dms-e1-system/ one-cell reference hashes of E1 on the system, ok-cold and ok-nominal
                        (Phase 2 R2)
examples/bench/         Phase 2 B1: a synthetic STM32F103 project (bench.net by make_netlist.py;
                        examples/feedback's firmware), local switch packages (example/pushbutton,
                        example/switch-1k), bench/*.toml (every bb-bench/1 stimulus), data/ (CSVs
                        by make_data.py; data/btn.csv from the [[pwl]] run)
tests/golden/bench/     one-cell reference hashes of examples/bench's seven cases (Phase 2 B1)
tests/device-smoke/     the device route's smoke board (board.cir, bb.lock; make device-smoke)
tests/bb-test/          bb test's own semantics and negative controls on the Phase 0 boards
                        (make bb-test-gate; Phase 2 B3)
tests/multi-producer/   Phase 2 H5: the multi-producer quiet-grant board (board.cir, bb.lock; make
                        multi-producer, ADR-014 G1)
tests/fixtures/kicad/   the synthetic two-board system of ADR-007 gate M4b (cli/tests/kicad_import.rs)
spec/                   normative protocol and schema docs (package.md and schema/*.schema.json: ADR-006;
                        board-bbm.md, canonical-json.md, project.md: ADR-007; test.md, test.schema.json,
                        test-result.schema.json, junit-subset.xsd: ADR-010 D8-D9); changing them needs an ADR
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
                        stranger_test.sh, lock_scope.sh, docs_plots.sh (+ docs_plots.py),
                        jit_profile_stress.sh, gdb_gate.py, dms_system.sh, check_nrf_pwm.py,
                        dms_home.sh (+ dms_home_boards.py, check_dms_home.py; Phase 2 R1:
                        compare_project_run.py; R2: dms_e1_system.sh, check_system_run.py);
                        Phase 2 H0:
                        worktree.sh (+ worktree_test.sh), measure_lock.sh (+ measure_lock_test.sh),
                        baseline.sh, step_time.sh; H6: ci_jobs_local.sh, setup_cache_cost.sh,
                        check_workflows.py; B1: bench_gate.sh (+ check_bench.py); B2:
                        assert_gate.sh (+ check_asserts.py, dms_asserts.py);
                        P0: pi_spike.sh (+ pi_spike.py); P1: pi_static.sh (+ pi_static.py);
                        P2: pi_glue.sh (+ pi_glue.py);
                        Phase 2 E4: vcd_check.sh, vscode_test.sh
                        (+ vscode_templates.py, vscode_test/: the extension test run inside VS Code);
                        E1: registry_test.sh (+ registry_https.py), registry_g7.sh, registry_repo.sh,
                        offline.sh
registry/template/      Phase 2 E1: the public registry repository's own files (README, owners.toml, the
                        pr and publish workflows, ci/bb.sh, ci/pr-check.sh, ci/publish.sh), copied
                        into ~/src/bb-registry by make registry-repo (spec/registry.md section 9)
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
  lock. A change to a package's files, to a platform's `.repl` or to a
  Renode plugin source that platform declares changes those hashes: run
  `make locks`, review, and commit the `bb.lock` files with the change. A
  platform's hash covers its `.repl` and its declared plugin sources only
  (`bb_cpu_renode::PLATFORM_PLUGINS` and `SHARED_PLUGINS`, ADR-015 D6), so
  an nRF model edit re-locks no STM32 board; a model a `.repl` starts
  using must be added to its row, and `make plugin-scope` checks that each
  row compiles alone. Milestone S relocked every committed lock as
  `bb-lock/1` with the scoped hashes (ADR-011 Amendment 1); a `bb-lock/0`
  file is read, and `bb open` or `bb pkg migrate --lock` rewrites it. A
  board file names extra package directories with
  `*bb packages path=<dir>`; `bb open` and `bb run` warn when a locked
  package resolved away from its recorded `path+` source (D7). Every
  top-level name a first-party view defines starts with its package prefix
  (`TI_TPS22917_...`, ADR-011 D11) and no view has a top-level `.param`:
  `make packages` lints `packages/` with `--publish`. A new platform adds
  a row to `cli/src/platforms.rs`. A package value is a
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
- **Registries (ADR-011, `spec/registry.md`).** Local sources always resolve
  first; a registry is asked only for an id no local source provides, and
  only `bb open` fetches (`bb run` reads a locked registry package from the
  cache by its tree hash). Every committed example's lock keeps `path+`
  sources, so `make test` and `make packages` never need the network
  (`make test-offline`). The registry key lives only in
  `~/.config/bb-registry` and the registry's CI secret: never commit it, and
  never push `~/src/bb-registry` or create its GitHub repository (that is
  the human's step, `docs/registry-publish.md`). A vendor file with
  restrictive terms (an SVD) is only referenced, by `[behavioral] svd`, and
  fetched at use: never put it into a package.
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
  `bb-project/1` and `bb-board/1` (ADR-013 D1, Phase 2 R1): a
  `[[device]]` takes a board file's device rules (`spec/package.md`
  section 12); a virtual one is board content (`virtual: true` in
  `board.bbm`), its parameters are not. A bound digital net outside the
  bench's `[analog] nets` is digital-only (ADR-014 D4): no glue, kernel
  routing only. Without `[analog] nets` nothing changes.
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

- **Trusting Renode's WFI-entry hook time.** Renode 1.17.0 calls the hook
  (`AddHookAtWfiStateChange`, entered) only when the CPU thread next runs,
  at the next slice's start, with every clock already there, and a WFI in
  a RUN_FOR's last slice is hooked in the next RUN_FOR. `bb_nrf52_state.cs`
  times the entry by the instructions executed since the last wake and
  records it at the engine's SYNC read after every advance (ADR-009
  Amendment 6). The wake hook's time is exact.
- **Timing a firmware delay by polling a Renode timer.** A running CPU sees
  RTC0 `COUNTER` (and the other timers' values) advance at its sync points,
  so a poll overshoots by up to one lattice cell; count instructions for a
  known duty (`nrf_duty.c`).

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
  conversion reads the latest push strictly before its sample instant (the
  stock model's `ADC SetValue` path, `--adc-model stock`, was removed in
  Phase 2 H6, ADR-015 D9). Never at the engine's `now` before `t`
  (`RenodeEngine::apply_input_trace`).
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
- **An assertion that is not passive** (ADR-010 D6, Amendment 3): one that
  adds an element, an input or a grant boundary, or that reads anything but
  the records and the engines' accepted solver points. A run's outputs
  other than its `assert` records must be those of the same run without
  the assertions (`make assert-test` checks the committed hashes with the
  `assert` records removed). And a check that does not fit a kind stays a
  script: no expression language (D5).
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
- **Letting Renode listen for External Control in a `bb` run, or
  admitting a connection without the owner check.** Renode 1.17.0's server
  binds `IPAddress.Any` with no accept hook, so any local process could
  take the slot (ADR-015 D5). Renode is the client
  (`CreateExternalControlClient`, `bb_cpu_renode::ec`): `bb` listens on
  `127.0.0.1` and admits only a connection whose socket is among its own
  Renode process's fds; anything else is closed unread. A Renode client
  does not reconnect by itself when `bb` closes: the engine restarts the
  *same* object (`ec::RECONNECT_PY`, reflection on the private
  `RestartConnection`); a second `CreateExternalControlClient` would leave
  the first object's GPIO hooks sending through disposed handlers
  (ADR-015 Amendment 2). The Renode-only model tests
  (`bb_adc_test.py`, `bb_saadc_test.py`, `renode_adc_test.py`,
  `tests/console.rs`) still open a server for their own clients.
- **Sending `Clear`, `SIGTERM` or the reconnect to a Renode before its
  External Control client has closed.** After `bb` closes the connection
  Renode disposes the client on its receive thread
  (`Disconnect(State.Disposed)`); a `Clear`, an exit or a
  `RestartConnection` racing that is the class of race of ADR-004
  Amendments 2 and 9 (a leaked listener, `SIGABRT` before the JIT profile
  is written). The engine, the pool and a gathering teardown wait until
  the client's threads are gone (`ec::wait_ec_closed`); keep it that way.
- **Parsing KiCad files with regex.** Use a real S-expression parser.
- **Feeding recorded data through a `[[pwl]]`.** An inline PWL card is
  O(n²) in ngspice (100 000 points took 103 s); `bb run` warns above 1 000
  points and refuses above 10 000. A `[[playback]]` (a bench table served
  with a cursor, ADR-010 D3) is the route, and its CSV is the only runtime
  format: never read a capture format in a run. A table never crosses the
  kernel boundary per solver point; only a triggered action's change does,
  as one `Drive::Timed` transition (ADR-010 Amendment 1).
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
- **A wall-clock timeout for an ngspice stall, or raising the watchdog to
  get past one** (ADR-015 D1). The watchdog counts accepted solver points
  per lattice cell, so a stall fails the same way in every run
  (`NoProgress:ngspice`); find the cause and fix it (the SAADC stall's was
  the engine's step clamp, ADR-015 Amendment 4). `--ngspice-watchdog` is
  for a board that legitimately needs more points per cell, and `run.json`
  records it.
- **A sub-picosecond solver step to a lattice point, or turning the
  lattice snap off outside `make ngspice-stall`** (ADR-015 Amendment 4).
  ngspice's TMAX = L steps drift off the lattice by rounding; a clamp that
  then adds a sub-femtosecond step to `kL` restarts the integration at
  about 1e-16 s, where trapezoidal Newton iterations can limit-cycle (the
  SAADC stall, and the committed E3 board at 502.47 ms of a 1 s run).
  `NgspiceEngine` stretches such a step onto the point instead (at most
  min(1 ps, 1 % of the step)). `--ngspice-lattice-snap off` exists only to
  reproduce the stall; its results are not the committed references.
- **A function call ngspice 42's table does not have, or with another
  arity** (ADR-015 D3). ngspice does not check arity: a three-argument
  `min()` in a B source segfaults libngspice 42 (and the whole `bb`
  process), and inside braces numparam silently computes `min(a, c)`.
  `min`/`max` take exactly two arguments; `bb pkg lint`, `bb pkg test` and
  `bb run` refuse the rest (`bbpkg::bexpr`).
- **A `.save` card in a board, view or test circuit** (ADR-015 D2). The
  engine owns `.save` and saves exactly the vectors it reads; a deck with
  its own is refused. Without it ngspice kept every vector at every solver
  point (E3 `typ`: 1.5 GB; with it, 330 MB).
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
  runtime writes it, once Renode has disposed of its External Control
  client (Amendment 9, ADR-015 Amendment 2; `make jit-profile-stress`
  checks the gather under load).
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
