# Running the tests

Breadboard's definition of done is that **`make test` is green** and, for
anything that affects waveforms, the golden-waveform diff is within its stated
tolerance. Waveform diffs decide correctness, not prose.

## `make check`

`make check` runs `fmt-check`, `clippy` and `test`: what CI runs.

```sh
make check       # cargo fmt --check, cargo clippy -D warnings, make test
make test        # the exit gate alone
```

## What `make test` runs

1. **`cargo-test`**: `cargo test --workspace` (scheduler mock tests, the
   ngspice RC test against the analytic answer, the Renode blinky edge-timing
   tests, the CLI's unit tests). `make test` sets `BB_REQUIRE_RENODE=1`, so the
   Renode tests fail rather than skip when Renode or the ARM toolchain is
   missing.
2. **`spike`**: 500 ms of the Phase 0 spike, then `scripts/check_spike.py`
   (LED_A plateaus, at least 8 PA5 edges spaced 50 ms ± 0.1 ms, LED_A switched
   by the first sample after each edge).
3. **`determinism`**: 10 spike runs; the sha256 of `nets.csv`, `events.jsonl`
   and `nets.vcd` must be identical across all of them.
4. **`golden`**: LED_A diffed against `tests/golden/spike/led_a.csv` (within
   1 mV / 1 sample), and the outputs against `tests/golden/spike/outputs.sha256`
   byte for byte.
5. **`feedback`** and **`feedback-order`**: the feedback board, its golden
   snapshot and its one-cell reference hashes; then the same run with the
   engines registered in the other order, byte-identical.
6. **`nrf-feedback`**: the feedback circuit on the nRF52840, lookahead on,
   against its one-cell reference.
7. **`adc`**, **`adc-order`**, **`adc-stock`**, **`adc-model`**: the STM32F072
   ADC board on Breadboard's ADC model and on Renode's stock one, and the
   Renode-only tests of the ADC model.
8. **`adc-kicad`**: the ADC board drawn in KiCad, run as a `bb.toml` project.
9. **`dms-rail-test`**: the E1 runs (see [the demo](./demo#how-it-is-gated)).
10. **`ec-reconnect`**: the one-cell references with the Renode engine
    replacing its External Control connection every 2 000 messages.
11. **`packages`**: every package file, lockfile, project, bench and
    `board.bbm` against `spec/schema`, `bb pkg lint`, `bb pkg test`, current
    `bb.lock` files, and that nothing local-only is committed.

## Pooled or fresh: `POOL=0`

`make test` and the gate targets run on a **warm Renode pool** by default
(ADR-004 Part B): each `bb run` leases a pre-warmed Renode process instead of
starting one. A run that finds no idle process starts Renode fresh, and the
outputs are the same either way (`make pool-equiv` checks this).

```sh
make test              # on a warm pool (POOL_SLOTS, default 5)
make test POOL=0       # every Renode fresh (CI job `fresh`)
```

## Golden, partition and determinism gates

The gates compare outputs, not prose. They share one rule: **the grant
partition is never part of the result.** `--max-quantum` and `--lookahead`
change only `grant` records, so a gate compares `nets.csv`, `nets.vcd` and the
non-grant records of `events.jsonl` against the one-cell reference
(`--max-quantum 10us`, or `--lookahead off` on a feedback board).

| Target | Checks |
| --- | --- |
| `make golden` | Spike LED_A against the committed snapshot (1 mV / 1 sample) and output hashes. |
| `make partition` | The spike at `--max-quantum` 10 µs, 100 µs, 10 ms, 50 ms and 500 ms gives the committed baseline byte for byte. CI job `partition`. |
| `make lattice-check` | 1 ms grants on the 10 µs lattice give the spike's `nets.csv` and non-grant records byte for byte; a 1 kHz CSV samples the 100 kHz waveform exactly. |
| `make determinism` | 10 spike runs, byte-identical. Also `feedback-determinism`, `adc-determinism`. |
| `make lookahead-equiv` | The feedback board with lookahead on at 1 ms and 10 ms equals its one-cell reference, with no broken promise and at most 200 grants per 500 ms. |
| `make adc-equiv` | The ADC board's one-cell, lookahead and open-loop runs equal the every-lattice-point references. |
| `make pool-equiv` | Every pooled run is leased and byte-identical to its fresh run and to the committed hashes. |

`make golden-update` regenerates the snapshots and hashes from fresh, checked
runs. A change to an equivalence reference is a change to results and needs a
reason (and, for the protocol, an ADR).

## Speed gates (run on an idle host)

`make perf`, `make lookahead-perf`, `make adc-speed`, `make startup-perf` and
`make pool-perf` measure wall-clock time with the ADR-003 measurement protocol
(interleaved A/B, N runs each, medians, startup separate). They are not in CI.
`make cost-table` only reports; see [Performance](../concepts/performance).

## CI

`.github/workflows/ci.yml` runs on every push and pull request. Its jobs:
`test` (setup, doctor, fmt, clippy, packages, `make test`), `fresh`
(`make test POOL=0`), `verify-every`, `partition`, `feedback`, `adc`,
`dms-rail` and `fw-rebuild`.

The full list of targets, with what each one checks, is the table in
[`AGENTS.md`](https://github.com/tyevco/breadboard.dev/blob/main/AGENTS.md).
