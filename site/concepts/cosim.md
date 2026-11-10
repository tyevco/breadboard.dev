# Co-simulation and time grants

Breadboard runs two very different simulators on one board: Renode executes
firmware instruction by instruction, and ngspice integrates the circuit's
differential equations with its own adaptive time step. Neither may run ahead
of the other. A kernel written in Rust (`bbcore::Scheduler`) owns simulated
time and hands it out in **grants**.

This page is a user-level summary. The normative text is
[`spec/time-grant.md`](../records/spec/time-grant); its rationale is
[ADR-002](../records/adr/002-time-grant-protocol) and
[ADR-003](../records/adr/003-cosim-speedup).

## The loop

Each iteration has four steps:

```text
  Query ──▶ Grant ──▶ Advance ──▶ Exchange ──┐
    ▲                                          │
    └────────────── while now < t_end ─────────┘
```

1. **Query.** The kernel asks every engine for `t_safe`: the latest time up
   to which it can report every output change it makes, with its exact time.
2. **Grant.** The kernel picks the grant `g`: the minimum of every engine's
   `t_safe`, the next sample instant (for probes that need one), the loop cap
   (if the board has a feedback loop), `--max-quantum` and the end of the run,
   rounded down to the **lattice**. It writes a `grant` record.
3. **Advance.** Each engine advances to exactly `g`, one after another, in
   the **plan order** (producers before consumers). Before an engine advances,
   it receives every change that an earlier engine made on its inputs during
   this grant, each at its own time.
4. **Exchange.** At the boundary `g` the kernel delivers what remains,
   samples the probed nets and writes the `pin_edge`, `log` and `net_sample`
   records.

Time is an integer number of **femtoseconds** (`u64`) everywhere a scheduling
decision is made. No engine runs in the background between calls, and no
engine's time ever passes the latest grant ("no engine free-runs",
`AGENTS.md`).

## The lattice

Renode advances in quanta. Its global quantum (`--renode-quantum`, default
10 µs) is the **grant lattice** `L`: every grant boundary must be a multiple of
it, and ngspice steps at most `L` and lands on every multiple of it.

Because every `RUN_FOR` ends on the lattice, Renode's edge times depend on the
lattice, not on how time is cut into grants. The README records that the
spike gives the identical edge list at grants of 10 µs, 70 µs and 100 µs, and
1, 10, 50 and 500 ms.

The lattice is run configuration, like the seed: changing `--renode-quantum`
changes results. `--max-quantum` (default 10 ms) is a pure cap on the length
of one grant and never changes results.

Two rules follow from the lattice:

- An MCU **input** takes effect at the next lattice point after the change
  (`ceil_L(edge_time)`), never earlier.
- A pin change that Renode stamps exactly on a lattice point is reported 1 fs
  later; a timer-driven stamp is reported exactly.

## Traces

A producer reports **every** output change of a grant in a trace, with its
time, not just the value at the end. So a grant may hold any number of edges
per pin, and a long grant loses nothing. When the binding graph has no
feedback loop, the plan order puts every producer before its consumers, so
every change reaches every consumer inside the same grant at its exact time.

## Feedback loops and lookahead

On a board where the MCU reads back a node it drives (`examples/feedback`:
PA5 drives an RC, and PB0 reads it), the binding graph is **cyclic**. A
consumer may then come before a producer in the plan order, so the kernel caps
each grant at one lattice cell: the **one-cell reference**.

**CPU lookahead** (ADR-003 Part B, on by default) lifts that cap while the
MCU sleeps:

- **Quiet promise.** A sleeping MCU with nothing pending promises,
  through `quiet_until`, that its outputs stay constant until its next timer
  event, as long as its inputs do not change.
- **Early stop.** The producer (ngspice) then runs first, and stops early at
  the lattice point after the first change on a pin the MCU reads. The MCU
  then advances to that point and sees the change exactly where a one-cell run
  would have applied it.
- **Audit.** If the MCU wakes early without an output change, or changes an
  output before its promise ends, the run fails with `PromiseBroken`. A
  broken promise is never turned into a fallback.
- **Back-off.** A candidate that keeps making no useful promise is asked less
  often (up to every 64th loop-capped grant), as a function of simulated
  history only.

On the feedback board this gives 117 grants for 500 ms instead of 50 000,
with byte-identical `nets.csv`, `nets.vcd` and non-grant records
(`make lookahead-equiv`). Firmware that never sleeps (the polling variant)
gets no promise and stays in one-cell grants. `--lookahead off` gives the
one-cell reference.

## Sampled inputs

Some inputs matter only at instants the MCU itself schedules. Breadboard's
STM32F072 ADC model reads its channel only at the end of its sampling phase,
so an `adc_in` pin on that model is a **sampled input** (ADR-005 D3):

- ngspice pushes the ADC input at the lattice points where its 12-bit code
  changes, and the Renode engine writes the pushes of a grant to the ADC model
  as timed inputs. A conversion reads the latest push strictly before its
  sample instant.
- A sampled input is not watched for the early stop, so a quiet grant runs to
  the next timer trigger whatever the RC node does.

On `examples/adc` that is about 1 200 grants for 300 ms instead of 30 000,
with byte-identical outputs (`make adc-equiv`). A model that raises an
interrupt or sets a flag when an input changes is not a sampled input.

## Sampled probes

Probes never add grant boundaries: an extra boundary would change Renode's
results. `nets.csv` and `nets.vcd` are sampled at fixed instants
(`--sample-rate`, default 100 kHz). Inside each grant, analog values are read
from ngspice's solver points (one lands on every 10 µs lattice point) and
digital values from the edge trace.

## A worked example

The [README](https://github.com/tyevco/breadboard.dev#feedback-example)
walks through the feedback board: PB0 rises 0.954859 ms after each PA5 rise,
in the 10 µs cell 50.95–50.96 ms. Renode sees the change at the end of that
cell, and PA6 follows 1 fs later. The spec's three sequence examples go
through the protocol grant by grant.
