# Event stream (`events.jsonl`)

Status: normative, schema version 1 (ADR-003 D3/D4, plan step 3; version 0
was Phase 0). Implementation: `bbcore::event`. Changing a record's fields,
their order, or their meaning requires an ADR.

Version 1 changes, all from `docs/adr/003-cosim-speedup.md`: `limited_by`
gains `loop_quantum` (and `stop:<engine>`, reserved for Part B); `pin_edge`
may repeat a net within one grant; ties in the boundary records are ordered
by registration index, never by the advance (plan) order. No record gains or
loses a field.

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
  output-binding order), and the kernel's `net_sample` records (in configured
  net order) last. The order never depends on the order in which engines
  advance.
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
| `limited_by` | string | `engine:<name>`, `sample`, `loop_quantum` (the binding graph has a feedback loop), `max_quantum` or `t_end`; `stop:<engine>` is reserved for ADR-003 Part B (an early stop) |

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

### `assert`
Result of a check. Benches and tests emit it; the scheduler does not.

| key | type | meaning |
| --- | --- | --- |
| `t_fs` | int | sim time the check refers to |
| `name` | string | stable check identifier |
| `pass` | bool | outcome |
| `msg` | string | human-readable detail |

```json
{"type":"assert","t_fs":500000000000000,"name":"led_blink_period","pass":true,"msg":"50.00 ms"}
```

## Companion outputs (not part of this stream)

- `nets.csv` (`bbcore::CsvWriter`): header `t_fs,<net>,...`, followed by one row
  per sample instant `k * period`. Digital values are `0`/`1`, analog values are
  volts with exactly 6 decimals (printed through integer µV), and an undriven
  net is an empty field.
- `nets.vcd` (`bbcore::VcdWriter`): `$timescale 1 fs`, with no `$date`. Digital
  nets are `wire 1`, one change per transition, stamped at `edge_time`. Analog
  nets are `real`, written only when the µV-rounded value changes, stamped at
  the boundary (`VcdWriter::new`) or, as `bb run` writes it, at the fixed
  sample instants of `--sample-rate` (`VcdWriter::sampled`, ADR-003 D3), so
  the file does not depend on the grant partition.
