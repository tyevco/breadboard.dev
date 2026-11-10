# Performance

Phase 0 existed to answer one question: is co-simulation sync too slow or
unstable? The answer recorded in the README is that the cost of a
co-simulation is almost all **fixed cost per grant** (Renode's pause and
resume per `RUN_FOR`, 0.4–0.5 ms per grant in the one-cell rows), so the lever
is **fewer grants**, and none of the ways of getting fewer grants may change a
result.

## Where the speed comes from

- **Long grants on a lattice** (ADR-003 Part A). With edge traces and every
  boundary on the 10 µs lattice, the grant length is a pure cap
  (`--max-quantum`, default 10 ms).
- **CPU lookahead** (ADR-003 Part B). On a board with a feedback loop, a
  sleeping MCU's quiet promise lifts the one-cell cap.
- **Sampled inputs** (ADR-005). Breadboard's STM32F072 ADC model reads its
  input only at its sample instants, so a quiet grant runs from one timer
  trigger to the next.
- **Engine startup** (ADR-004). A fresh Renode run uses tuned .NET settings,
  a cached JIT startup profile and a private copy of the firmware. A **warm
  Renode pool** (`bb pool run -- <command>`) lets each run lease a pre-warmed
  Renode instead of starting one.

See [Co-simulation and time grants](./cosim) for how each one keeps results
identical.

## The cost table

These figures are copied from the README. They were measured in **WSL2
Ubuntu 24.04 on the development machine (24 logical CPUs)** with
`make cost-table`, which follows ADR-003's measurement protocol: for each
board, the one-cell run (A) and `bb`'s defaults (B) interleaved, N = 5 of each,
medians, engine startup reported separately from simulation. Every B run gave
the same `nets.csv`, `nets.vcd` and non-grant `events.jsonl` records as A.
"Pool" columns are on a one-slot warm Renode pool.

| Board, run | Simulated | Grants | Simulation (median) | Startup, fresh | Startup, pool | Total wall, fresh | Total wall, pool |
| --- | --- | --- | --- | --- | --- | --- | --- |
| spike, `--max-quantum 10us` (one-cell) | 500 ms | 50 000 | 23.52 s | 2.02 s | 0.15 s | 25.66 s | 24.89 s |
| spike, defaults (10 ms grants) | 500 ms | 50 | **0.81 s** (29×) | 2.04 s | **0.18 s** | 3.02 s | 1.16 s |
| `examples/feedback --lookahead off` | 500 ms | 50 000 | 24.74 s | 2.13 s | 0.30 s | 27.04 s | 24.66 s |
| `examples/feedback`, defaults (lookahead on) | 500 ms | 117 | **1.19 s** (21×) | 2.22 s | **0.39 s** | 3.49 s | 1.78 s |
| `examples/feedback-open`, `--max-quantum 10us` | 500 ms | 50 000 | 24.36 s | 2.25 s | 0.08 s | 26.86 s | 22.63 s |
| `examples/feedback-open`, defaults | 500 ms | 50 | **0.56 s** (44×) | 2.05 s | **0.10 s** | 2.66 s | 0.50 s |
| `examples/adc --lookahead off` | 300 ms | 30 000 | 11.50 s | 1.46 s | 0.09 s | 13.00 s | 11.26 s |
| `examples/adc`, defaults (Breadboard's ADC model) | 300 ms | 1 209 | **1.11 s** (10.4×) | 1.43 s | **0.09 s** | 2.61 s | 0.97 s |
| `examples/adc --adc-model stock` (Renode's ADC, lookahead on) | 300 ms | 6 344 | 5.08 s | 1.38 s | | 6.53 s | |
| `examples/nrf-feedback --lookahead off` | 500 ms | 50 000 | 13.93 s | 1.41 s | 0.05 s | 15.36 s | 14.23 s |
| `examples/nrf-feedback`, defaults | 500 ms | 120 | **0.52 s** (27×) | 1.39 s | **0.05 s** | 1.95 s | 0.42 s |
| `examples/adc/kicad --lookahead off` (as a `bb.toml` project) | 300 ms | 30 000 | 12.66 s | 1.49 s | | 14.17 s | |
| `examples/adc/kicad`, defaults | 300 ms | 1 209 | **1.13 s** (11.2×) | 1.58 s | | 2.64 s | |
| `examples/dial-me-spinner` E1, `--max-quantum 10us` | 120 ms | 12 000 | 6.34 s | 1.38 s | 0.07 s | 7.78 s | 6.02 s |
| `examples/dial-me-spinner` E1, defaults (`make demo`) | 120 ms | 12 | **1.83 s** (3.5×) | 1.42 s | **0.08 s** | 3.29 s | 1.95 s |

### Measurement conditions

The rows were not all measured in one session, and the host load differed.
The README states the conditions; in short:

- **First rows (spike, feedback, feedback-open, adc):** measured 2026-11-09
  (ADR-004 Amendment 7) at load average 2.3–6.6 while another job ran on the
  host, so the figures are noisy. The three `examples/adc` rows were measured
  again later that day (ADR-005 Amendment 4): fresh at load 3.1–7.5, pool at
  load 1.8–2.7, and the stock row on an idle host (load 1.2).
- **`examples/nrf-feedback`** (ADR-008 Amendment 2): fresh at load 2.4–4.0,
  pool at load 1.6–2.0. Not comparable with the rows above, which ran at a
  higher load.
- **`examples/adc/kicad`** (ADR-007 Amendment 1): fresh only, load 1.6–2.2.
  In the same session the hand-written board took 1.09 s at its defaults, so
  the project route costs no measurable simulation time.
- **E1** (Phase 1 M7, `make cost-table-dms-rail`): an idle host, fresh at load
  0.7–1.1 and pool at load 1.0–1.4. The analog deck dominates on this board:
  the same deck through ngspice alone takes about 1.9 s for 120 ms, so long
  grants gain less here than on the Phase 0 boards.

Compare within a row and between the two startup columns, not across
sessions. Per-run numbers are in `out/cost-table/{fresh,pool}/*/results.tsv`
after a run.

## Startup

**Engine startup** is the time `bb run` waits before the first grant; almost
all of it is Renode's own start (.NET, Renode init, the platform load). On an
idle host (load 1.1–1.2) fresh startup was 1.32 s for the spike, 1.39 s for
the feedback board and 1.46 s for the ADC board. On a warm pool it falls to
0.08–0.39 s in the table above. `make test` and the gate targets run on a pool
by default (`POOL=0` runs them fresh).

## Measure it yourself

```sh
make cost-table             # the whole table, fresh and on a 1-slot pool (reports only)
make cost-table-dms-rail    # the E1 rows alone
make perf                   # ADR-003 speed gate (spike)
make lookahead-perf         # ADR-003 step 8 speed gates (feedback)
```

Run them on an otherwise idle host: they measure wall-clock time. A new
cost-table row is always measured with `make cost-table`, never from one run.
