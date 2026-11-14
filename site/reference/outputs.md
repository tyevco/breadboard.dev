# Outputs and the event stream

`bb run` writes everything under `--out`. The three core files are
deterministic: the same inputs and seed give byte-identical files.

| File | Written for | Contents |
| --- | --- | --- |
| `events.jsonl` | every run | The event stream (below). |
| `nets.csv` | every run | `t_fs` and one column per probe, one row per sample instant. |
| `nets.vcd` | every run | The probes as a VCD (timescale 1 fs), for GTKWave. |
| `deck.cir` | projects | The elaborated SPICE deck. |
| `run.json` | projects | The run's provenance record. |

## `events.jsonl`

The normative description is
[`spec/event-stream.md`](../records/spec/event-stream) (schema version 1).

- UTF-8, one JSON object per line, no blank lines.
- Every record begins with `"type"`; the other keys follow in a fixed order,
  which is part of the schema. Optional keys are omitted, never `null`.
- Times are **integer femtoseconds** (`t_fs`, `until_fs`); voltages are
  **integer microvolts** (`uv`). Records contain no floats.
- `t_fs` never decreases along the stream. Records of one exchange are sorted
  by time, then by the producing engine's registration index, then by pin.
- Consumers must ignore unknown keys and unknown `type` values.

### Record types

| `type` | Meaning | Keys |
| --- | --- | --- |
| `grant` | The kernel granted every engine the interval `(t_fs, until_fs]`. | `t_fs`, `until_fs`, `limited_by` |
| `pin_edge` | A digital net changed level. One record per change. | `t_fs`, `net`, `driver`, `level` |
| `net_sample` | A probed net's value at a fixed-rate sample instant. | `t_fs`, `net`, `uv` or `level` (optional) |
| `log` | A line from an engine (for example firmware UART) or the kernel. | `t_fs`, `source`, `level`, `msg` |
| `assert` | The result of a check (emitted by benches and tests, not by the scheduler). | `t_fs`, `name`, `pass`, `msg` |

`limited_by` names what bounded the grant: `engine:<name>`, `sample`,
`loop_quantum` (the board has a feedback loop), `max_quantum`, `t_end`, or
`stop:<engine>` (with lookahead, that engine stopped the grant early).

```json
{"type":"grant","t_fs":0,"until_fs":10000000000000,"limited_by":"max_quantum"}
{"type":"pin_edge","t_fs":50000000000000,"net":"PA5","driver":"renode.PA5","level":1}
```

`grant` records describe how time was partitioned, which is not part of the
result. To compare two runs, compare the records without them:

```sh
grep -v '"type":"grant"' out/rail/events.jsonl
```

## `nets.csv`

A header `t_fs,<net>,...` (display names, quoted per RFC 4180 when needed),
then one row per sample instant `k × period` (`--sample-rate`, default
100 kHz, so every 10 µs). Digital values are `0`/`1`; analog values are volts
with exactly 6 decimals, printed through integer µV; an undriven net is an
empty field.

## `nets.vcd`

`$timescale 1 fs`, no `$date`. Digital nets are `wire 1`, one change per
transition, stamped at the edge time. Analog nets are `real`, written at the
fixed sample instants and only when the µV-rounded value changes, so the file
does not depend on the grant partition. In a project, analog nets use their
SPICE names, and a `$comment` block maps each to its display name when they
differ.

## `run.json` (`bb-run/0`)

Canonical JSON, with no absolute paths and no wall-clock times. It records:

- the sha256 of `board.bbm`, the import, and the bench (file name, text,
  sha256, option set and its card);
- the targets (image kinds and hashes);
- **every in-scope analog instance**: package, version, view, the requested
  fidelity and the rung it was chosen at, provenance `source` and
  `confidence`, its qualifications and whether one covers the run;
- the out-of-scope and not-simulated instances, with reasons;
- the locked package and platform hashes, the deck's sha256, every run option
  that can change results, the probes, and the tool versions;
- `"debug": true` when a debugger was attached (`bb run --gdb`, with
  `options.gdb` and `options.gdb_wait`). The key is absent otherwise.

A debug run also carries a `warn` `log` record from the MCU's engine, "debug
run: GDB: ...", at the first operation that can have changed the results
(a breakpoint or watchpoint, a step, a write, a peripheral-register read).
Never compare such a run against a reference. See
[Debugging with GDB](../guide/debugging).

See [Fidelity and honesty](../concepts/fidelity) for why.
