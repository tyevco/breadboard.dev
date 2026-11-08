# AGENTS.md: working on Breadboard

Breadboard is an open hardware simulator ("QEMU for whole circuit boards"). The
repo is in **Phase 0**: a Renode (STM32F103) ↔ ngspice co-simulation spike under
a kernel-owned time-grant protocol. Read this file first. Then read
`spec/time-grant.md` before you touch anything that advances time.

## Build and test

Run every command inside WSL (Ubuntu 24.04) or Linux, from the repo root.

| Command | What it does |
| --- | --- |
| `make setup` | Install or verify the pinned toolchain (idempotent): Rust 1.99.1, uv, Renode 1.17.0 portable, arm-none-eabi-gcc, ngspice 42 + libngspice0-dev |
| `make doctor` | Print every tool's version; exits non-zero if anything is missing |
| `make build` | `cargo build --workspace --all-targets` |
| `make test` | The exit gate: `cargo-test`, `spike`, `determinism`, `golden` (about 5 min) |
| `make cargo-test` | `cargo test --workspace` only |
| `make spike` | Build firmware + `bb`, run 500 ms of co-simulation into `out/spike`, check it (`scripts/check_spike.py`) |
| `make spike-verify-every` | The spike again with `--verify every` (Renode cross-check after every grant, ADR-003 D1.2); outputs must be byte-identical to `out/spike` (CI job `verify-every`) |
| `make determinism` | 10 spike runs (`DET_RUNS`, `DET_JOBS` in parallel); sha256 of `nets.csv`, `events.jsonl`, `nets.vcd` must match |
| `make golden` | Diff `out/spike` LED_A against `tests/golden/spike/led_a.csv` (1 mV / 1 sample) |
| `make golden-update` | Regenerate the golden snapshot from a fresh, checked spike run. Review the diff before committing it |
| `make fmt` / `make fmt-check` | `cargo fmt --all` (`-- --check` in CI) |
| `make clippy` | `cargo clippy --workspace --all-targets -- -D warnings` |
| `make check` | `fmt-check`, `clippy` and `test`, which is what CI runs |

`make firmware` builds the blinky ELF on its own.

## Repo map

```
bbcore/                 kernel: Time, PinSignal, Engine trait, Scheduler, EventSink, CsvWriter, VcdWriter
engines/analog-ngspice/ ngspice shared-lib engine (libngspice.so.0)
engines/cpu-renode/     Renode engine over the External Control API
cli/                    `bb` binary (`bb run`, board-file parser)
examples/spike/         blinky firmware + board.cir (SPICE body + *bb pin bindings)
tests/golden/spike/     led_a.csv waveform snapshot
spec/                   normative protocol and schema docs; changing them needs an ADR
docs/adr/               architecture decision records
scripts/                setup.sh, doctor.sh, check_spike.py, golden.py, determinism.sh
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
   latest grant.

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
- Git is local only: never push and never add remotes. Commit messages end with
  the co-author trailer given by the orchestrator.

## Known bad ideas (do not do these)

- **Stepping ngspice per instruction.** ngspice advances one *grant* at a
  time, to a breakpoint at `g`.
- **Letting Renode's virtual time run ahead of the grant.** Drive Renode only
  with `RunFor(g - now)`. Never use `start`/free-running emulation and then try
  to catch up.
- **Parsing KiCad files with regex.** Use a real S-expression parser.
- Using float seconds for scheduling. Convert at the engine's FFI boundary only.
- Registering a consumer engine before its producer. The Advance phase is
  Gauss-Seidel (ADR-002 Amendment 2), so only engines registered *after* a
  producer see its edges inside the same grant, at their true time.
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
- Renode is installed at `~/.local/opt/renode-1.17.0` (symlink
  `~/.local/bin/renode`). `libngspice.so.0` is in `/usr/lib/x86_64-linux-gnu`.

## Definition of done

A change is done when **`make test` is green** and, for anything that affects
waveforms, the **golden-waveform diff is within its stated tolerance**
(Phase 0: `tests/golden/spike/led_a.csv`, within 1 mV / 1 sample). Waveform
diffs decide correctness, not prose. `make clippy` and `make fmt-check` must
also pass, and determinism must hold: running twice gives identical bytes.
