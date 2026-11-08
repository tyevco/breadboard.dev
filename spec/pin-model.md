# Pin model (pin-boundary contract)

Status: normative. Implementation: `bbcore::pin`, `bbcore::signal`. Changing
this file requires an ADR. `digital_out` ramp start: ADR-003 D3 (plan step 3,
Amendment 3).

Every component pin has **exactly one** pin model, supplied by its package (in
Phase 0, hand-written in the board netlist). The kernel connects pins to nets
and routes signals between engines. Each engine converts between its own domain
and a `PinSignal`, using the pin model. IBIS buffer models map onto these
models directly in later phases.

## Signals crossing the boundary

```rust
enum PinSignal {
    Digital { level: Level /* Low | High */, edge_time: Time },
    Analog  { volts: f64 },
}
```

- `edge_time` is the absolute time of the most recent level change. For the
  rules it must follow, see `spec/time-grant.md` invariant 5.
- `volts` is relative to the board ground net. Text outputs round it to integer
  µV.
- A net has exactly one driver. The kernel passes the driver's `PinSignal`
  unchanged to every load on the net.

## Models

| Model | Parameters | Phase 0 use |
| --- | --- | --- |
| `digital_out` | `voh` V, `vol` V, `rise` Time, `fall` Time, `zout` Ω | **yes** (STM32 PA5) |
| `digital_in` | `vih` V, `vil` V, `cin` F | not yet |
| `analog` | none; bidirectional node, exchanged as volts | not yet (as a probe target only) |
| `power` | `vmin` V, `vmax` V, `imax` A | not yet |
| `passive` | none; exists only inside the analog engine (R, C, LED...) | **yes** (330 Ω, LED) |

### `digital_out{voh, vol, rise, fall, zout}`

A push-pull driver. When a digital net feeds an analog engine, the analog engine
models the pin as:

```
  V_<pin> <pin>_int 0 dc 0 external        ; value supplied per time point by the kernel side
  R_<pin>_zout <pin>_int <net> {zout}
```

The value of the `EXTERNAL` source at analog time `t` is
`DigitalOut::ramp_volts(from, to, t_start, t)`: a linear ramp from
`v(from)` to `v(to)` over `rise` (for a transition to High) or `fall` (for a
transition to Low). It starts at `t_start = max(edge_time, now)`: the edge's
`edge_time` when the edge is delivered before the analog engine advances
through it, which the plan order guarantees for every producer outside a
feedback loop (ADR-002 Amendment 2, ADR-003 D3), otherwise the boundary at
which it was delivered (ADR-002 D4.4). Every transition of a producer's trace
gets its own ramp. Before `t_start` the value is `v(from)`; after
`t_start + rise|fall` it is `v(to)`. The function is pure in its integer-time
arguments.

Phase 0 STM32F103 GPIO (`DigitalOut::STM32_GPIO_3V3`):
`voh = 3.3, vol = 0.0, rise = fall = 5 ns, zout = 25 Ω`.

### `digital_in{vih, vil, cin}`

When an analog net feeds a digital engine, the producing analog engine (or the
kernel glue) reports `Analog{volts}`. The consuming digital engine thresholds it
with hysteresis using `DigitalIn::threshold(previous, volts)`: from Low, the pin
reads High once `volts >= vih`; from High, it reads Low once `volts <= vil`;
otherwise it keeps the previous level. `cin` is added to the analog netlist as a
capacitor to ground on the net.

### `analog`

The pin reads or drives an analog value directly, for example an ADC input or
DAC output. ADC inputs are sampled at boundaries.

### `power{vmin, vmax, imax}`

A supply pin. Later phases check rail voltage and current against these limits
and emit `assert` records.

### `passive`

The pin belongs to a purely electrical component. It never crosses an engine
boundary; the analog engine handles it entirely.

## Glue-generation rules

1. A net driven by a `digital_out` and loaded by an analog engine gets the
   `EXTERNAL` source plus `zout` construct above, with one source per driver.
2. A net driven by an analog engine and loaded by a `digital_in` gets the `cin`
   capacitor, and its value crosses the boundary as `Analog`.
3. Glue element names are a pure function of the pin and net names
   (`V_<pin>`, `R_<pin>_zout`), so generated netlists are deterministic.
4. Phase 0 generates the glue by hand in the example netlist. The automatic
   generator arrives with the KiCad importer, which must not parse KiCad with
   regex.
