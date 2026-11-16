# Event stream (`events.jsonl`)

Status: normative, schema version 2 (ADR-009 D7 and ADR-010 D7, landed with
the Phase 2 schema epoch, milestone S; version 1 was ADR-003 D3/D4, version
0 Phase 0). Implementation: `bbcore::event` (`EVENT_STREAM_VERSION`).
Changing a record's fields, their order, or their meaning requires an ADR.

Version 2 adds the `reset` record and gives `assert` its version 2 keys
(`outcome`, `value`, `limit`, `unit`, `rung`). No other record changes,
and the stream carries no version field, so every version 1 file is a
version 2 file. `assert` records are emitted since milestone B2, for a
run's assertions (`spec/project.md` section 3.4; ADR-010 Amendment 3);
nothing in this build emits `reset` (milestone P5). A run without
assertions gives exactly the stream it gave before.

Net names (ADR-007 D6, Phase 1 M4): `net` in `pin_edge` and `net_sample`
is the net's **display name** (a project's KiCad net name, or the kernel
net `<ref>.<pin>` of an MCU pin); Phase 0 names are unchanged. No record
gained or lost a field, so the schema version stays 1.

Version 1 changes, all from `docs/adr/003-cosim-speedup.md`: `limited_by`
gains `loop_quantum` and `stop:<engine>` (the early stop of ADR-003 D5,
in use since plan step 6, Amendment 6); `pin_edge` may repeat a net within
one grant; ties in the boundary records are ordered by registration index,
never by the advance (plan) order. No record gains or loses a field.

## Format

- UTF-8, one JSON object per line, each line ending in `\n`. There is no
  enclosing array, no blank lines and no trailing whitespace.
- Every record begins with `"type"`. The remaining keys follow, in the
  **exact order listed below**, and that order is part of the schema.
- Optional keys are omitted, never written as `null`.
- All times are **integer femtoseconds** (`t_fs`, `until_fs`).
- Voltages are **integer microvolts** (`uv`). Records contain no floats, so the
  text never depends on float formatting.
- Records appear in emission order, and `t_fs` never decreases along the
  stream. A `grant` is emitted before the boundary records of its interval.
  The boundary records of one exchange (`log`, `pin_edge`, `net_sample`) are
  sorted by `(t_fs, registration index of the producing engine, pin)`: at
  equal `t_fs`, engines in registration order, within an engine its `log`
  records (in emission order) before its `pin_edge` records (in
  output-binding order), and the kernel's records last: its `net_sample`
  records (in configured net order), then its `assert` records (in the
  order of the run's assertions: the bench's, then each `--assert` file's).
  The order never depends on the order in which engines advance.
- Consumers must ignore unknown keys and unknown `type` values, which keeps
  forward compatibility. Producers must not emit them without an ADR.

Determinism requirement: the same inputs and seed produce a byte-identical file.

## Records

### `grant`
The kernel granted every engine the interval `(t_fs, until_fs]`.

| key | type | meaning |
| --- | --- | --- |
| `t_fs` | int | start of the interval (the previous boundary) |
| `until_fs` | int | the grant `g` |
| `limited_by` | string | `engine:<name>` (its `t_safe`, or with lookahead its `quiet_until` promise), `sample`, `loop_quantum` (the binding graph has a feedback loop), `max_quantum`, `t_end`, or `stop:<engine>`: with lookahead (ADR-003 D5) that engine stopped the grant early, at the first lattice point at or after a change on a pin feeding a quiet engine, so `until_fs` is that point |

```json
{"type":"grant","t_fs":0,"until_fs":10000000000000,"limited_by":"max_quantum"}
```

Emitted once per grant unless `SchedulerConfig.log_grants` is false.

### `pin_edge`
A digital net changed level. One record per change: a net may have any number
of `pin_edge` records within one grant (an engine trace, ADR-003 D3), with
strictly increasing `t_fs` and alternating `level`.

| key | type | meaning |
| --- | --- | --- |
| `t_fs` | int | `edge_time` reported by the driver, in `(previous boundary, g]` |
| `net` | string | net name |
| `driver` | string | `<engine>.<pin>` |
| `level` | int | new level, `0` or `1` |

```json
{"type":"pin_edge","t_fs":50000000000000,"net":"PA5","driver":"renode.PA5","level":1}
```

### `net_sample`
A probed net's value at a fixed-rate sample instant. That instant is always an
exchange boundary, because the scheduler aligns grants to it.

| key | type | meaning |
| --- | --- | --- |
| `t_fs` | int | sample instant, a multiple of the sample period |
| `net` | string | net name |
| `uv` | int, optional | analog value in µV, rounded half away from zero |
| `level` | int, optional | digital level 0/1 |

At most one of `uv` and `level` is present. Neither is present if the net is
not driven yet.

```json
{"type":"net_sample","t_fs":60000000000000,"net":"LED_A","uv":1900000}
```

### `log`
A line from an engine (for example firmware UART) or from the kernel.

| key | type | meaning |
| --- | --- | --- |
| `t_fs` | int | sim time it was produced, clamped up to the grant start |
| `source` | string | engine name, or `kernel` |
| `level` | string | `debug`, `info`, `warn` or `error` |
| `msg` | string | the text, JSON-escaped |

```json
{"type":"log","t_fs":1000000000000,"source":"renode","level":"info","msg":"boot ok"}
```

### `assert` (version 2)
Result of a check (ADR-010 D5-D7): an assertion of a bench or a test case,
evaluated deterministically. The scheduler itself never emits one; the
kernel emits the verdict at the verdict time, after the `net_sample`
records of equal `t_fs` (the kernel's records come last). It is part of
the non-grant equivalence comparison. One record per assertion of the
run, at its window's close (`spec/project.md` section 3.4). In this build
`rung` is the run's rung, `fast`, and `nA` and `udegC` are not used yet.

| key | type | meaning |
| --- | --- | --- |
| `t_fs` | int | sim time of the verdict |
| `name` | string | stable check identifier |
| `outcome` | string | `pass`, `fail` or `not_evaluable` |
| `pass` | bool, optional | the outcome as version 1 read it: present unless `not_evaluable` |
| `value` | int, optional | the measured value, an integer in the base unit `unit` |
| `limit` | int, optional | the limit it was compared with, in the same unit |
| `unit` | string, optional | the base unit: `uV`, `fs`, `nA`, `udegC` (µ°C) or `count`, so the stream stays free of floats |
| `rung` | string | the lowest chosen fidelity rung over the in-scope instances with a port on a net the check reads (ADR-009 D1) |
| `msg` | string | human-readable detail |

```json
{"type":"assert","t_fs":10500000000000000,"name":"LED-DATA-V4","outcome":"pass","pass":true,"value":2971511,"limit":2670000,"unit":"uV","rung":"fast","msg":"VLED at P0.06 rise"}
```

### `reset` (version 2)
A machine reset of a CPU engine (ADR-009 D7): a brown-out holds the
machine in reset while `#bor` is high and releases it at `ceil_L` of its
fall. It sorts like a `log` record of its engine.

| key | type | meaning |
| --- | --- | --- |
| `t_fs` | int | the reset's sim time |
| `source` | string | the CPU engine's name |
| `cause` | string | `bor` |
| `phase` | string | `enter`, `exit`, or `stop` (the run ends at the reset, ADR-009 D7's fallback) |
| `ram` | string, optional | on `exit`: `kept` (RAM survives a brown-out deterministically) |

```json
{"type":"reset","t_fs":10000000000,"source":"renode","cause":"bor","phase":"exit","ram":"kept"}
```

## Companion outputs (not part of this stream)

- `nets.csv` (`bbcore::CsvWriter`): header `t_fs,<net>,...` (display names; a
  name that contains `,`, `"` or a line break is quoted as RFC 4180 says,
  `bbcore::csv_field`), followed by one row
  per sample instant `k * period`. Digital values are `0`/`1`, analog values are
  volts with exactly 6 decimals (printed through integer µV), and an undriven
  net is an empty field.
- `nets.vcd` (`bbcore::VcdWriter`): `$timescale 1 fs`, with no `$date`. The
  `$var` reference names of a project's analog nets are their SPICE names
  (`spec/board-bbm.md` section 4), MCU pins keep their kernel net names;
  when any reference differs from its display name, a `$comment` block
  after `$timescale` maps each such reference to its display name
  (`  <reference> = <display name>`, one per line, sorted). Phase 0 names
  need no mapping, so every Phase 0 file is unchanged. Digital
  nets are `wire 1`, one change per transition, stamped at `edge_time`. Analog
  nets are `real`, written only when the µV-rounded value changes, stamped at
  the boundary (`VcdWriter::new`) or, as `bb run` writes it, at the fixed
  sample instants of `--sample-rate` (`VcdWriter::sampled`, ADR-003 D3), so
  the file does not depend on the grant partition.
