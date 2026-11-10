# Pin models

Engines interact only through **pins bound to nets** (the pin-boundary
contract). Every component pin has exactly one pin model, supplied by its
package's `pins.toml`, and every net has exactly one driver. Each engine
converts between its own domain and a `PinSignal` using the pin model. This
page summarises [`spec/pin-model.md`](../records/spec/pin-model), which is
normative.

## What crosses the boundary

```rust
enum PinSignal {
    Digital { level: Level /* Low | High */, edge_time: Time },
    Analog  { volts: f64 },
}
```

- `edge_time` is the absolute time of the most recent level change.
- `volts` is relative to the board ground net; text outputs round it to
  integer µV.
- The kernel passes the driver's `PinSignal` unchanged to every load on the
  net.

## Models

| Model | Parameters | Used today |
| --- | --- | --- |
| `digital_out` | `voh` V, `vol` V, `rise` time, `fall` time, `zout` Ω | yes (an MCU output into the circuit) |
| `digital_in` | `vih` V, `vil` V, `cin` F | yes (the circuit read by an MCU input) |
| `adc_in` | `vref` V, `bits` | yes (an MCU ADC channel) |
| `analog` | none; a bidirectional node, exchanged as volts | not yet (as a probe target only) |
| `power` | `vmin` V, `vmax` V, `imax` A | not yet |
| `passive` | none; exists only inside the analog engine | yes (R, C, LED, ...) |

### `digital_out`

A push-pull driver. In the analog engine the pin becomes an `EXTERNAL`
voltage source behind `zout`:

```text
  V_<pin> <pin>_int 0 dc 0 external        ; value supplied per time point by the kernel side
  R_<pin>_zout <pin>_int <net> {zout}
```

The source ramps linearly from the old level to the new one over `rise` or
`fall`, starting at the edge's `edge_time` when the edge is delivered before
the analog engine advances through it (which the plan order guarantees
outside a feedback loop). Every transition of a producer's trace gets its own
ramp. Phase 0's STM32F103 GPIO uses `voh = 3.3`, `vol = 0`, `rise = fall =
5 ns`, `zout = 25 Ω`.

### `digital_in`

When an analog net feeds a digital engine, the **producing** analog engine
thresholds it with hysteresis at every solver point: from Low the pin reads
High once `volts >= vih`; from High it reads Low once `volts <= vil`. The
crossing time is interpolated between the two solver points and reported as a
`Digital` transition in the producer's trace. `cin` is added to the netlist
as a capacitor to ground. A lattice-bound consumer (Renode) applies each
transition at the next lattice point. A glitch that starts and ends between
two solver points (at most one lattice cell apart) is missed.

### `adc_in`

An MCU ADC input reading an analog net. It is a **push** model: the producing
analog engine reads the net and sends the value to the MCU's ADC at lattice
points (where the code changes, by default; `--adc-push every` pushes at every
lattice point). On Breadboard's STM32F072 ADC model it is a **sampled input**:
the pushes are timed inputs and a conversion reads the latest push strictly
before its sample instant. See
[Co-simulation](../concepts/cosim#sampled-inputs).

## Parameter sets

A pin model's parameters may come from a named set in the package's
`pins.toml` (`[defaults.digital_out.H0H1]`), chosen per binding by `set`;
without `set` the set named `default` is used, and a binding's explicit
parameters override the set's. On the nRF52840 the `digital_out` set names
stand for the `PIN_CNF` drive (`S0S1`, `H0H1`, ...) and are audited at run
time.

## Unbound pins

A connected MCU pin without a binding is unbound: no glue and no load
(high-Z), and its net is probe-only on the MCU side.
