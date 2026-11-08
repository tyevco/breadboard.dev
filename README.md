# Breadboard

Breadboard is an open hardware simulator, aiming to be "QEMU for whole circuit boards". You give
it firmware and a board, and it simulates both together. CPU engines run the
firmware, analog engines solve the circuit, and a kernel that owns
simulated time couples them at the pin boundary.

The repo is in **Phase 0**, a spike that tests the project's top risk: is
co-simulation sync too slow or unstable? Firmware runs on an STM32F103 in
[Renode](https://renode.io) and toggles PA5. The pin drives a 330 Ω resistor
and a red LED, and [ngspice](https://ngspice.sourceforge.io) solves that
circuit. Both engines advance only on time grants from the Rust kernel
(`bbcore`, `spec/time-grant.md`). The output must be byte-identical from run
to run.

```
U1 (STM32F103, Renode)        generated glue                    board.cir
  PA5 digital_out  ──edge──▶  V_bb_U1_PA5 (EXTERNAL) ─ 25 Ω ─▶ PA5 ─ R1 330 Ω ─ LED_A ─ D1 (red) ─ GND
  {3.3 V, 5 ns, 25 Ω}         (from the pin model)
```

## Quickstart

Run these inside WSL Ubuntu 24.04 or Linux, from the repo root:

```sh
make setup   # pinned toolchain: Rust, Renode 1.17.0 portable, arm-none-eabi-gcc, ngspice 42
make test    # cargo tests + 500 ms spike + 10-run determinism + golden diff + feedback board
```

From Windows: `wsl -d Ubuntu-24.04 -- bash -lc 'cd /mnt/c/source/tyevco/breadboard.dev && make setup && make test'`.

Run the simulation by hand:

```sh
make firmware bb
~/.cache/bb-target/release/bb run --netlist examples/spike/board.cir \
    --firmware examples/spike/firmware/build/blinky.elf --target U1 --for 500ms \
    --probe net:LED_A --probe net:PA5 --out out/spike   # [--max-quantum 10ms] [--renode-quantum 10us] [--sample-rate 100kHz] [--verify edges]
```

The run writes these files to `out/spike/`:

| File | Contents |
| --- | --- |
| `events.jsonl` | One `grant` record per grant, plus `pin_edge` records with Renode's exact edge times (`spec/event-stream.md`, schema version 1) |
| `nets.csv` | `t_fs,LED_A,PA5` sampled every 10 µs, in volts with 6 decimals. Samples never add grant boundaries: inside each grant, analog values are read from ngspice's solver points (one lands on every 10 µs lattice point) and digital values from the edge trace (ADR-002 Amendment 3, ADR-003 D3) |
| `nets.vcd` | The probed analog nets as `real`, sampled at `--sample-rate`, and the MCU pin net `U1.PA5` as a wire with every edge (timescale 1 fs) |

`bb` prints the wall-clock time to the terminal, never into the outputs.

## What `make test` checks

1. `cargo test --workspace`: scheduler mock tests, the ngspice RC test against
   the analytic answer, the Renode blinky edge-timing tests and the CLI's
   unit tests. `make test` sets `BB_REQUIRE_RENODE=1`, so the Renode tests
   fail rather than skip when Renode or the ARM toolchain is missing.
2. `make spike`: the 500 ms run, then `scripts/check_spike.py`. The LED_A high
   plateau must be in [1.7, 2.2] V and the low plateau below 0.1 V. There must
   be at least 8 PA5 edges and at least 8 LED_A transitions, each spaced
   50 ms ± 0.1 ms. LED_A must have switched by the first sample after each PA5
   edge.
3. `make determinism`: 10 runs. The sha256 of `nets.csv`, `events.jsonl` and
   `nets.vcd` must be identical across all of them.
4. `make golden`: LED_A from the spike run is diffed against
   `tests/golden/spike/led_a.csv` (a 10 kHz snapshot) within 1 mV / 1 sample.
5. `make feedback`: 500 ms of `examples/feedback`, a board with a feedback
   loop (below), then `scripts/check_feedback.py` and the golden snapshot
   `tests/golden/feedback/`.
6. `make feedback-order`: the same run with the engines registered in the
   other order must be byte-identical.

`make feedback-determinism` (10 runs) and `make feedback-open` (the partition
gate of the open-loop variant) run in their own CI job.

## Feedback example

`examples/feedback` is a board on which the MCU reads back a node it drives
(ADR-003 "Feedback-path correctness"):

```
U1.PA5 ─ 25 Ω ─ R2 1 kΩ ─ SENSE ─ C1 1 µF ─ GND
                          SENSE ──▶ U1.PB0 digital_in {vih 2.0 V, vil 0.8 V, cin 5 pF}
U1.PA6 ─ 25 Ω ─ R3 330 Ω ─ LED_B ─ D2 (red) ─ GND
```

The firmware toggles PA5 every 50 ms (SysTick). An EXTI0 interrupt on both
edges of PB0 copies PB0 to PA6. ngspice thresholds SENSE with the
`digital_in` hysteresis at every solver point and reports each crossing,
interpolated between solver points, as a PB0 edge. PB0 rises 0.954859 ms
after each PA5 rise, in the 10 µs cell 50.95–50.96 ms, and falls 1.452500 ms
after each fall, in 101.45–101.46 ms. Renode sees the change at the end of
that cell, `ceil_L(t_x)`, and PA6 follows 1 fs later. The loop
U1 → ngspice → U1 keeps every grant at one Renode quantum (10 µs), so this
board is correct but slow until ADR-003 Part B. Opening the loop
(`examples/feedback-open`, SENSE driven by a circuit source) gives long
grants again with byte-identical outputs.

## Result

LED_A in `nets.csv` is a clean 10 Hz square wave:

- **Before the first edge:** 0 V until the firmware's first SysTick toggle at
  50 ms.
- **High plateau:** a flat **1.866945 V**, the red LED's forward voltage at
  (3.199 − 1.867) V / 330 Ω ≈ **4.0 mA**.
- **PA5 when high:** sits at 3.199 V, because 25 Ω × 4 mA is dropped in the
  pin's output impedance.
- **Low plateau:** exactly 0 V.
- **Edges:** PA5 changes level at 50, 100, …, 450 ms and 499.99987 ms. The
  later edges drift by about 20 ns per period because of the timer model
  rounding in Renode. That drift is deterministic and is well inside the
  ±0.1 ms tolerance.
- **LED_A transitions:** each one has completed by the next 10 µs sample. The
  5 ns pin ramp is applied at Renode's exact edge timestamp: the kernel
  advances the CPU engine first in every grant because it drives ngspice's
  input (the plan order, ADR-003 D3), and every edge of the grant reaches
  ngspice before ngspice advances through it (ADR-002 Amendment 2).

`out/spike/nets.vcd` opens in GTKWave. Plot `LED_A` and `PA5` (real) together
with `U1.PA5` (wire) to see this.

## Measured cost (the "co-sim is slow" risk)

These figures were measured in WSL2 Ubuntu 24.04 on the development machine,
for 500 ms of simulated time, with `make perf` (ADR-003's measurement
protocol: interleaved runs, N = 5, medians, startup reported separately; load
average 3–12 from other work on the host).

| Run | Grants | Simulation (median) | Total wall (median) |
| --- | --- | --- | --- |
| `--max-quantum 10us` (the Phase 0 setting: one-cell lockstep) | 50 000 | 45.9 s | 54.0 s |
| default `--max-quantum 10ms` (`make spike`) | 50 | **1.42 s** (32×) | 9.2 s, mostly 4.9 s of engine startup |
| `examples/feedback` (a feedback loop: one-cell grants whatever `--max-quantum` is) | 50 000 | 31.8 s (0.64 ms per grant) | 36.0 s |
| `examples/feedback-open` (the loop opened), default 10 ms grants | 50 | 2.1 s | 7.4 s |

The cost of a co-simulation is almost all fixed cost per grant (Renode's
pause/resume per `RUN_FOR`), so the lever is fewer grants. Since ADR-003 the
grant length is a pure cap and changes no output: Renode reports every edge
of a grant in a trace, its quantum (the lattice) stays 10 µs, and ngspice
lands on every lattice point. The spike gives byte-identical `nets.csv`,
`nets.vcd` and `pin_edge` records at 10 µs, 100 µs, 10 ms, 50 ms and 500 ms
grants (`make partition`). A board with a feedback loop (an analog net read
back by the MCU, `examples/feedback`) runs in one-cell grants; ADR-003 Part B
lifts that while the MCU sleeps. The `examples/feedback` row is the median of
5 interleaved runs (ADR-003 Amendment 4), and the `examples/feedback-open` row
is one run of `make feedback-open`. Neither is `make perf`. The full cost table
is refreshed in ADR-003 plan step 10.

**Edge times depend on the Renode quantum, not on the grant partition**
(ADR-003 D2). Renode's quantum (`--renode-quantum`, default 10 µs) is a grid
that every grant boundary must sit on; it is run configuration, like the seed.
Renode stamps a pin change with the sync point that opens the slice it ran
in; a stamp on the grid is reported 1 fs later, a timer-driven stamp exactly.
With every `RUN_FOR` ending on the grid, 10 µs, 70 µs and 100 µs, and 1, 10,
50 and 500 ms grants give the identical edge list
(`engines/cpu-renode/tests/blinky.rs`), and the spike's outputs do not depend
on `--max-quantum` (a multiple of the quantum; `make lattice-check`,
`make partition`). A grant may hold any number of edges per pin. MCU inputs
take effect at the next grid point, never earlier.

## Layout

See `AGENTS.md` for the repo map, the invariants (determinism, the
pin-boundary contract, no engine free-runs) and the rules for changing
`spec/`. The decisions are recorded in `docs/adr/`.
