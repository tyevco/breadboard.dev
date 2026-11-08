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
make test    # cargo tests + 500 ms spike + 10-run determinism + golden diff
```

From Windows: `wsl -d Ubuntu-24.04 -- bash -lc 'cd /mnt/c/source/tyevco/breadboard.dev && make setup && make test'`.

Run the simulation by hand:

```sh
make firmware bb
~/.cache/bb-target/release/bb run --netlist examples/spike/board.cir \
    --firmware examples/spike/firmware/build/blinky.elf --target U1 --for 500ms \
    --probe net:LED_A --probe net:PA5 --out out/spike   # [--max-quantum 10us] [--sample-rate 100kHz]
```

The run writes these files to `out/spike/`:

| File | Contents |
| --- | --- |
| `events.jsonl` | One `grant` record per grant, plus `pin_edge` records with Renode's exact edge times (`spec/event-stream.md`) |
| `nets.csv` | `t_fs,LED_A,PA5` sampled every 10 µs at exchange boundaries, in volts with 6 decimals |
| `nets.vcd` | The probed analog nets as `real`, and the MCU pin net `U1.PA5` as a wire (timescale 1 fs) |

`bb` prints the wall-clock time to the terminal, never into the outputs.

## What `make test` checks

1. `cargo test --workspace`: scheduler mock tests, the ngspice RC test against
   the analytic answer, the Renode blinky edge-timing tests and the CLI's
   unit tests.
2. `make spike`: the 500 ms run, then `scripts/check_spike.py`. The LED_A high
   plateau must be in [1.7, 2.2] V and the low plateau below 0.1 V. There must
   be at least 8 PA5 edges and at least 8 LED_A transitions, each spaced
   50 ms ± 0.1 ms. LED_A must have switched by the first sample after each PA5
   edge.
3. `make determinism`: 10 runs. The sha256 of `nets.csv`, `events.jsonl` and
   `nets.vcd` must be identical across all of them.
4. `make golden`: LED_A from the spike run is diffed against
   `tests/golden/spike/led_a.csv` (a 10 kHz snapshot) within 1 mV / 1 sample.

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
  5 ns pin ramp is applied at Renode's exact edge timestamp: the CPU engine
  advances first in every grant, and its edges reach ngspice before ngspice
  advances through the same grant (ADR-002 Amendment 2).

`out/spike/nets.vcd` opens in GTKWave. Plot `LED_A` and `PA5` (real) together
with `U1.PA5` (wire) to see this.

## Measured cost (the "co-sim is slow" risk)

These figures were measured in WSL2 Ubuntu 24.04 on the development machine,
for 500 ms of simulated time at the default 10 µs quantum. That is 50 000
grants with the per-grant Renode time/GPIO cross-check enabled.

| Run | Wall clock |
| --- | --- |
| `make spike`, a single run | **39.1 s** total: 2.8 s engine startup plus 36.3 s simulation, about 0.73 ms per grant. An earlier run took 45.7 s |
| `make determinism`, 5 runs in parallel | 80–87 s per run |
| `make test`, the whole gate | about 4 minutes |

Almost all of the time goes to Renode's pause/resume per `RUN_FOR` grant. The
lever is lookahead, a larger `t_safe` while the MCU is idle (see
`spec/time-grant.md`, example 2). Phase 0 does not use it yet. A coarser
setting is much cheaper: `--max-quantum 1ms --sample-rate 1kHz` runs 500
grants in **3.7 s** total (0.9 s of simulation), with the same 10 edges at the
same exact times. The CSV is then sampled only every 1 ms. Edge timing does not
depend on the quantum, because edges carry exact times. The quantum only
bounds how many edges can fall into one grant, at most one per pin.

## Layout

See `AGENTS.md` for the repo map, the invariants (determinism, the
pin-boundary contract, no engine free-runs) and the rules for changing
`spec/`. The decisions are recorded in `docs/adr/`.
