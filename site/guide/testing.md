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
7. **`adc`**, **`adc-order`**, **`adc-model`**: the STM32F072 ADC board on
   Breadboard's ADC model, and the Renode-only tests of the ADC model
   (Renode's stock ADC, `adc-stock`, was removed in Phase 2 H6).
8. **`nrf-saadc-model`**, **`nrf-pwm-model`**: the Renode-only tests of
   Breadboard's nRF52840 SAADC and PWM (with the TEMP and NVMC stand-ins),
   then their boards through `bb run` (`examples/nrf-saadc`,
   `examples/nrf-pwm`): the SAADC's charge transfers and codes, and every
   PWM edge at its exact instant, at one cell, with lookahead and in the
   other engine order.
9. **`device-smoke`**: a device part in a loop with the CPU.
10. **`adc-kicad`**: the ADC board drawn in KiCad, run as a `bb.toml` project.
11. **`dms-rail-test`**: the E1 runs (see [the demo](./demo#how-it-is-gated)).
12. **`dms-vsense-test`**: the E2 runs at 50 ms and with C5 at 10 nF, with
    their `load = "none"` twins (see [the E2 demo](./demo-e2)).
13. **`dms-home-test`**: E3's one full step at 446 PPS on dial-me-spinner's
    detent sheet against the suite (see [the E3 demo](./demo-e3)).
14. **`ec-reconnect`**: the one-cell references with the Renode engine
    replacing its External Control connection every 2 000 messages.
15. **`packages`**: every package file, lockfile, project, bench and
    `board.bbm` against `spec/schema`, `bb pkg lint`, `bb pkg test`, current
    `bb.lock` files, and that nothing local-only is committed.
16. **`gdb-test`**: GDB in batch mode against `bb run --gdb` on the spike and
    E1. Attach and detach without a stop give the committed hashes; a
    breakpoint marks the run (see [Debugging with GDB](./debugging)). The
    full gate is `make gdb` (CI job `dms-rail`).
17. **`vscode-templates`**: the VS Code templates in `docs/templates/vscode`
    have the settings cortex-debug needs against `bb run --gdb`, and their
    `endsPattern` matches the line `bb` prints. The full check, in a real
    VS Code, is `make vscode-test` (see [Viewers and VS Code](./viewing)).
18. **`dms-system`**: the dial-me-spinner system import (main and base boards
    and the link harness): the committed `board.bbm` from a fresh import,
    the mated pins, and E1's deck from the system equal to E1's own.

Phase 2 added these steps (the `TEST_STEPS` list in the `Makefile` is the
order they run in):

19. **`measure-lock-test`**: the host measurement lock that timed gates take:
    while one measurement holds it, every measurement target refuses to
    start.
20. **`nrf-power-model`**: the Renode-only register and timing tests of
    Breadboard's nRF52840 POWER model (POFWARN and brown-out, ADR-009 D6,
    D7).
21. **`multi-producer`**: quiet grants with several producers (ADR-014) on
    a synthetic board (an nRF52840 running E3's homing firmware and the
    mechanism device on digital-only nets): bb's defaults equal the one-cell
    reference except grant records, within a grant budget, with the scope
    control.
22. **`bench-test`**: `bb-bench/1` benches (timed actions, playback,
    profiles, faults, the switch view) on `examples/bench`, against their
    one-cell references, with the refusals.
23. **`assert-test`**: the assertion kinds on E1's four cases, `bb assert`'s
    replay, the FW-6.4 negative controls and the refusals (ADR-010).
24. **`pi-static-test`**: the W1 and W2 static-load decks from the `pi`
    package views against the suite, at their tolerances (ADR-009 P1).
25. **`pi-glue-test`**: rail-tracking, tristate `digital_out` at `pi`: every
    Phase 1 board's operating point with every output high-Z, E1 cold at
    `pi`, and LED_PWR_EN's high level under load (ADR-009 D5).
26. **`pi-idd-test`**: the nRF52840's `supply_load` view and CPU state
    outputs against a hand calculation of the duty-weighted current
    (ADR-009 D4).
27. **`dms-e1-system-test`**: E1 on both dial-me-spinner boards in one deck
    (a system run, ADR-013 D2), with E1's checks and its references.
28. **`dms-w1-test`**: W1, POF blanking on a dead battery at `pi`, as
    `tests/w1.toml` through `bb test` (see the README's quickstart), with
    `scripts/check_dms_w1.py` (which reads each run's corner from its
    `run.json`) and the committed hashes.
29. **`dms-w2-test`**: W2, the brown-out reboot loop, as `tests/w2.toml`
    through `bb test`, with `scripts/check_dms_w2.py` and the committed
    hashes.
30. **`registry-test`**: the package registry against local fixtures
    (signed snapshots, tampering, rollback, yanking, routed namespaces;
    ADR-011).
31. **`limits-test`**: package limits as automatic assertions at `pi` and
    with `bb run --limits` (ADR-009 D3): a fixture package that writes E1's
    LSW-3 as a limit fails exactly on the wrong-order firmware.
32. **`docs-sync`**: the docs that copy the code stay in step with it: the
    quickstart page against the README's block, every `bb` subcommand in
    the CLI reference, every `make test` step in this list and in the
    README.

## Pooled or fresh: `POOL=0`

`make test` and the gate targets run on a **warm Renode pool** by default
(ADR-004 Part B): each `bb run` leases a pre-warmed Renode process instead of
starting one. A run that finds no idle process starts Renode fresh, and the
outputs are the same either way (`make pool-equiv` checks this).

```sh
make test              # on a warm pool (POOL_SLOTS, default 5)
make test POOL=0       # every Renode fresh (the nightly job `fresh`)
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

The figures on the [E1](./demo), [E2](./demo-e2), [E3](./demo-e3) and
[introduction](./introduction) pages come from real runs too. `make
docs-plots` reruns make demo's E1 runs, the E2 cases `sag`, `rec`, `s50` and
`s50-none`, E3's `typ` (the homing) and `step` (one full step) and the
spike, and draws `site/public/plots/<name>.{light,dark}.svg` from their
outputs (about 5 minutes, most of it E3's homing). The SVGs are a pure function of the
runs, so `make docs-plots-check` (CI job `dms-rail`) regenerates them and
requires the committed files byte for byte; after a change to results, run
`make docs-plots` and commit the figures. `make vcd-check` (CI job
`dms-rail`) loads make demo's VCDs and a committed fixture with wellen,
Surfer's waveform library (see [Viewers and VS Code](./viewing)).

`make golden-update` regenerates the snapshots and hashes from fresh, checked
runs. A change to an equivalence reference is a change to results and needs a
reason (and, for the protocol, an ADR).

## Speed gates (run on an idle host)

`make perf`, `make lookahead-perf`, `make startup-perf` and
`make pool-perf` measure wall-clock time with the ADR-003 measurement protocol
(interleaved A/B, N runs each, medians, startup separate). They are not in CI.
`make cost-table` only reports; see [Performance](../concepts/performance).

## CI

`.github/workflows/ci.yml` runs on every push and pull request: the PR tier
(ADR-015 D11; its slowest job at most 15 minutes, `make check` at most the
runtime baseline plus 4 minutes). Its jobs: `test` (setup, doctor, fmt,
clippy, packages, `make test` without `cargo-test`), `cargo-test`
(`cargo test --workspace` on a warm pool), `verify-every`, `partition`,
`feedback`, `adc`, `dms-rail` and `fw-rebuild`.

`.github/workflows/nightly.yml` is the nightly tier: once a day on `main`
(and by hand), one job each for `make test POOL=0` (the `fresh` job),
`make ec-long`, `make ngspice-stall`, `make dms-home` and `make dms-vsense`.
`make nightly` runs the same locally. A nightly failure blocks the next
milestone merge.

`.github/workflows/docs.yml` builds this site and deploys it to GitHub Pages
on a push to `main` that touches `site/`, `docs/`, `spec/` or `README.md`.

The full list of targets, with what each one checks, is the table in
[`AGENTS.md`](https://github.com/tyevco/breadboard.dev/blob/main/AGENTS.md).
