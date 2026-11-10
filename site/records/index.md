# Design records

Breadboard records its decisions before it implements them. The documents in
this section are rendered from the repository at build time, unchanged except
for a header naming the source file. The repository is the authority, and the
paths they mention are relative to its root.

## Architecture decision records (`docs/adr/`)

An ADR is written before implementation for anything that touches the kernel
protocol or the package schema, and no change to `spec/` lands without one.
Each ADR states its status and, where a human approver decided open
questions, an "Approver decisions" section. Later changes are recorded as
numbered amendments at the end.

| ADR | Decision |
| --- | --- |
| [ADR-001](./adr/001-kernel-language-rust) | The kernel language is Rust, exposed through a C ABI. |
| [ADR-002](./adr/002-time-grant-protocol) | The time-grant protocol: conservative lockstep with lookahead. |
| [ADR-003](./adr/003-cosim-speedup) | Co-simulation speed-up: lattice-aligned grants, edge traces, binding-graph order, CPU lookahead. |
| [ADR-004](./adr/004-startup-time) | Engine startup time: Renode runtime settings and a warm Renode pool. |
| [ADR-005](./adr/005-adc-model) | An in-repo STM32 ADC peripheral model: hardware triggers, conversion timing, sampled inputs. |
| [ADR-006](./adr/006-component-packages) | Component packages v0: `part.toml`, `pins.toml`, model views, local packages and the lockfile. |
| [ADR-007](./adr/007-kicad-import) | KiCad import, the elaborated board (`board.bbm`) and the project file. |
| [ADR-008](./adr/008-nrf52840-target) | The nRF52840 as a CPU target: Renode platform, peripheral models, firmware, GDB. |

## Specifications (`spec/`)

The normative documents. Changing any of them requires an ADR.

| Spec | Covers |
| --- | --- |
| [Time-grant protocol](./spec/time-grant) | The scheduler's state machine, invariants, advance and exchange, quiet grants. |
| [Pin model](./spec/pin-model) | The pin-boundary contract and the glue each pin model generates. |
| [Event stream](./spec/event-stream) | The records of `events.jsonl` and the companion `nets.csv` and `nets.vcd`. |
| [Component packages](./spec/package) | `part.toml`, `pins.toml`, views, values, test vectors, `bb.lock`, lint. |
| [Projects](./spec/project) | `bb.toml`, names, benches, the elaborated deck, the CLI verbs, `run.json`. |
| [The elaborated board](./spec/board-bbm) | `board.bbm`, what the importer reads and the resolution stages. |
| [Canonical JSON](./spec/canonical-json) | The integer-only canonical JSON form. |

## Plans (`docs/`)

- [Phase 1 plan](./plan/phase1-plan): the goal, the worked examples E1–E3,
  scope, milestones M0–M11, the definition of done, risks, the approver
  decisions and the review notes of every milestone.
- [Phase 1 exit checklist](./plan/phase1-exit-checklist): the human run of
  the quickstart.
