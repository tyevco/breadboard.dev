# Introduction

Breadboard is an open hardware simulator, aiming to be "QEMU for whole circuit
boards". You give it firmware and a board, and it simulates both together:

- **CPU engines** run the firmware. Today that is [Renode](https://renode.io)
  1.17.0, on an STM32F103, an STM32F072 and Breadboard's own nRF52840 platform.
- **Analog engines** solve the circuit. Today that is
  [ngspice](https://ngspice.sourceforge.io) 42, loaded as a shared library.
- **A kernel** written in Rust (`bbcore`) owns simulated time. Engines advance
  only when the kernel grants them time, and they exchange values only
  through pins bound to nets.

The command-line tool is `bb`. It imports a board, records which firmware runs
on which MCU, runs the co-simulation and writes waveforms and an event log.

## What it does today

Breadboard is in **Phase 1**. Phase 1 runs a real board that nobody wrote for
Breadboard:

> Import a real KiCad project, resolve its parts to packages, load real
> firmware on the real MCU, and reproduce, in a co-simulation, a result that
> the board's own SPICE test suite established by hand: a firmware rule that
> protects the hardware. ("Goal", `docs/phase1-plan.md`)

The reference board is **dial-me-spinner rev A**, pinned at commit `6854d25`
(KiCad 10.0.8). Its KiCad inputs are vendored in
`examples/dial-me-spinner/kicad/`, so you do not need KiCad to run it. The
reference demo, [E1](./demo), checks the board's LED rail power sequencing.

Phase 0 came first. It was a spike that tested the project's top risk ("is
co-simulation sync too slow or unstable?") on hand-written boards: an
STM32F103 blinky driving an LED, a feedback board on which the MCU reads back
a node it drives, and an STM32F072 that reads back a divider through its ADC.
Those boards are still in `examples/` and still run in `make test`.

## How a run fits together

```text
KiCad netlist ──bb open──▶ board.bbm + bb.lock      (parts resolved to packages)
firmware ELF  ──bb flash─▶ bb.toml [targets]        (image hash recorded)
bench.toml    ──bb run───▶ Renode (MCU) ⇄ kernel ⇄ ngspice (analog scope)
                             │
                             ▼
              nets.csv, nets.vcd, events.jsonl, deck.cir, run.json
```

- `bb open` imports the netlist, resolves every part to a
  [component package](../reference/packages), and writes the elaborated board
  `board.bbm` and the lockfile `bb.lock`.
- `bb flash` records a firmware image and its hash in the project's `bb.toml`.
  No device is touched.
- `bb run` builds the analog deck from the parts in the bench's analog scope,
  starts the engines and runs them for the requested simulated time. It is
  read-only on the project and writes only under `--out`.

## Where to go next

- [Quickstart](./quickstart): run the reference demo on a clean Ubuntu 24.04.
- [The E1 demo](./demo): what the demo checks and why the wrong-order
  firmware fails.
- [Co-simulation and time grants](../concepts/cosim): how the kernel keeps
  two very different simulators in step.
- [Status and roadmap](./roadmap): what is done, and what is planned but not
  built.
