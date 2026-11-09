# AGENTS.md: working on Breadboard

Breadboard is an open hardware simulator ("QEMU for whole circuit boards"). The
repo is in **Phase 0**: a Renode (STM32F103, and STM32F072 for the ADC
board) ↔ ngspice co-simulation spike under a kernel-owned time-grant
protocol, with the speed-ups of ADR-003 (the grant lattice, traces, CPU
lookahead, ADC pushes). Read this file first. Then read
`spec/time-grant.md` before you touch anything that advances time.

## Build and test

Run every command inside WSL (Ubuntu 24.04) or Linux, from the repo root.

| Command | What it does |
| --- | --- |
| `make setup` | Install or verify the pinned toolchain (idempotent): Rust 1.99.1, uv, Renode 1.17.0 portable, arm-none-eabi-gcc, ngspice 42 + libngspice0-dev |
| `make doctor` | Print every tool's version; exits non-zero if anything is missing |
| `make build` | `cargo build --workspace --all-targets` |
| `make test` | The exit gate: `cargo-test`, `spike`, `determinism`, `golden`, `feedback`, `feedback-order`, `adc`, run on a warm Renode pool (ADR-004 Part B; `POOL=0` runs it fresh, `POOL_SLOTS` sets the pool size, default 5). `make check` took 2 min 40 s on the pool against 3 min 10 s fresh before it, under load 6–11, ADR-004 Amendment 4 |
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
| `make adc` | ADR-003 step 9: `examples/adc` (STM32F072; U1.PA4 → RC-filtered divider → ADC_IN → U1.PA1 `adc_in`; TIM3 converts every 999 µs, the analog watchdog drives ALARM on PA5, code bits 4 and 8 on PA6/PA7), 300 ms with lookahead on. `scripts/check_adc.py`: every conversion's edges as the firmware model predicts from ADC_IN; then `nets.csv`, `nets.vcd` and the non-grant records byte-identical to the one-cell reference that pushes at every lattice point (`tests/golden/adc/outputs.sha256`; `make adc-ref`) |
| `make adc-equiv` | ADR-003 step 9 gates (`scripts/adc_equiv.sh`): Part A (one cell) and Part B (lookahead, 1 ms and 10 ms) on `examples/adc`, and `board-open.cir` (acyclic) at 1, 10 and 50 ms, all identical except `grant` records to the every-lattice-point references (`tests/golden/adc*/outputs.sha256`); no promise broken; the watchdog's window crossings stop quiet grants (`stop:ngspice`) (CI job `adc`) |
| `make adc-renode` | The Renode-only ADC model test of ADR-003 Amendment 5 (`make -C examples/adc test`, about 15 s; CI job `adc`) |
| `make lookahead-startup` | ADR-003 step 5 startup-cost gate: Renode engine startup with the cached plugin within 0.3 s of a start without it (interleaved, N = 7, medians; run on an idle host) |
| `make startup-perf` | ADR-004 G1: fresh `bb run` engine startup of the spike and the feedback board at least 12 % and 0.3 s below `BB_RENODE_STOCK_RUNTIME=1` (the stock .NET settings and 100 ms connect poll), interleaved, N = 7 (`STARTUP_N`), medians, outputs identical; `STARTUP_BASE=<bb>` compares against another binary. Run it on an idle host |
| `make pool-equiv` | ADR-004 G4: one pool process serves the spike (default, 1 ms, 500 ms), feedback (on, off, plugin never, polling firmware), adc (on, off) and adc-open twice, in alternating order; every pooled run must be leased and byte-identical to its fresh run and to the committed hashes (`scripts/pool_equiv.sh`) |
| `make pool-perf` | ADR-004 G2: on a 1-slot pool, the first lease starts in at most 0.5 s, median engine startup is at most 0.30 s for spike, feedback and adc (N = 7), and spike total wall at most 1.2 s (`scripts/pool_perf.sh`). Run it on an idle host |
| `bb pool run -- <cmd>` | Run `<cmd>` with a warm Renode pool in `BB_RENODE_POOL` (`--slots N`, `--max-runs K`); `bb pool start` / `stop` / `status` for an interactive shell (`bb pool --help`). `bb run --renode-pool off` ignores a pool |
| `make golden-update` | Regenerate the golden snapshots, the spike output hashes and the partition hashes (spike, feedback, feedback-open), the feedback one-cell reference hashes and the ADC reference hashes from fresh, checked runs. Review the diff before committing it: a change to an equivalence reference is a change to results and needs a reason (and, for the protocol, an ADR) |
| `make cost-table` | The README cost table with the measurement protocol (`scripts/perf_ab.sh`, interleaved, N = `PERF_N`, medians): the spike at 10 µs vs 10 ms grants, `examples/feedback` and `examples/adc` with lookahead off vs on, `examples/feedback-open` at 10 µs vs 10 ms. Reports only; no gate. Run it on an otherwise idle host |
| `make fmt` / `make fmt-check` | `cargo fmt --all` (`-- --check` in CI) |
| `make clippy` | `cargo clippy --workspace --all-targets -- -D warnings` |
| `make check` | `fmt-check`, `clippy` and `test`, which is what CI runs |

`make firmware` builds the firmware ELFs (spike, feedback, ADC and the Renode
lookahead gate firmware in `engines/cpu-renode/tests/firmware`) on their own.

## Repo map

```
bbcore/                 kernel: Time, PinSignal, Engine trait, Scheduler, EventSink, CsvWriter, VcdWriter
engines/analog-ngspice/ ngspice shared-lib engine (libngspice.so.0)
engines/cpu-renode/     Renode engine over the External Control API; platforms/bb_lookahead.cs
                        (the lookahead plugin, ADR-003 D5); tests/firmware (its gate firmware);
                        console.rs, pool.rs, manager.rs (the warm Renode pool, ADR-004)
cli/                    `bb` binary (`bb run`, `bb pool`, board-file parser)
examples/spike/         blinky firmware + board.cir (SPICE body + *bb pin bindings)
examples/feedback/      feedback board (digital_in PB0 reads the RC that PA5 drives) + EXTI firmware and a polling variant
examples/feedback-open/ the same with the loop opened (acyclic, long grants)
examples/adc/           STM32F072 ADC board (adc_in PA1 reads the divider PA4 drives; board.cir,
                        board-open.cir), its firmware, and the Renode-only ADC model test
tests/golden/spike/     led_a.csv waveform snapshot, default-run output hashes, partition hashes
tests/golden/feedback/  SENSE / LED_B snapshots, exact pin_edge list, one-cell reference hashes
tests/golden/feedback-open/ partition hashes of the one-cell reference
tests/golden/adc/, adc-open/ hashes of the every-lattice-point one-cell ADC references
spec/                   normative protocol and schema docs; changing them needs an ADR
docs/adr/               architecture decision records
scripts/                setup.sh, doctor.sh, check_spike.py, golden.py, determinism.sh,
                        partition.sh, perf.sh, perf_ab.sh (also make cost-table),
                        check_resample.py, check_feedback.py, lookahead_inert.sh,
                        lookahead_equiv.sh, check_adc.py, adc_equiv.sh, startup_perf.sh,
                        with_pool.sh, pool_equiv.sh, pool_perf.sh
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
  same holds for an ADC push: ngspice sends it at a lattice point (when the
  whole millivolts change, ADR-003 Amendment 5) and Renode applies it with
  `ADC SetValue` while paused there, never at the engine's `now` before it
  (`RenodeEngine::apply_input_trace`).
- **A `quiet_until` promise the engine cannot guarantee, or one with side
  effects.** It is a pure read, it may be skipped by the back-off, and a
  sleeping CPU's promise ends at its next clock limit. Never weaken the
  wake audit or turn a `PromiseBroken` into a fallback: a broken promise
  means the outputs may already differ from the one-cell run.
- **Calling a run equivalent from `events.jsonl` alone, or from
  `nets.csv` alone.** Compare `nets.csv`, `nets.vcd` and the non-grant
  records (`grep -v '"type":"grant"'`); grant records differ by design
  between partitions and lookahead settings.
- **Sending `Clear` to a Renode right after its External Control client
  disconnected.** Renode 1.17.0 restarts the server then, and a `Clear`
  that races the restart leaks the listener or crashes Renode (ADR-004
  Amendment 2). The pool waits for the re-listen first; keep it that way.
- **Parsing KiCad files with regex.** Use a real S-expression parser.
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
- The Renode lookahead plugin is compiled once and cached in
  `~/.cache/bb/renode-plugins` (`$BB_CACHE_DIR` or `$XDG_CACHE_HOME/bb`
  override it); deleting the directory only costs one compile (about 1.6 s).
- Renode is installed at `~/.local/opt/renode-1.17.0` (symlink
  `~/.local/bin/renode`). `libngspice.so.0` is in `/usr/lib/x86_64-linux-gnu`.

## Definition of done

A change is done when **`make test` is green** and, for anything that affects
waveforms, the **golden-waveform diff is within its stated tolerance**
(Phase 0: `tests/golden/spike/led_a.csv`, within 1 mV / 1 sample). Waveform
diffs decide correctness, not prose. `make clippy` and `make fmt-check` must
also pass, and determinism must hold: running twice gives identical bytes.
