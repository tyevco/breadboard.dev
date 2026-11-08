# Pin model (pin-boundary contract)

Status: normative. Implementation: `bbcore::pin`, `bbcore::signal`. Changing
this file requires an ADR. `digital_out` ramp start: ADR-003 D3 (plan step 3,
Amendment 3). `digital_in` thresholding by the producing analog engine and its
glue: ADR-003 D3 (plan step 4, Amendment 4). `adc_in`: ADR-003 D6 (plan step
9, Amendments 5 and 10).

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
| `digital_in` | `vih` V, `vil` V, `cin` F | **yes** (STM32 PB0 in `examples/feedback`) |
| `adc_in` | `vref` V, `bits` | **yes** (STM32F072 PA1, ADC channel 1, in `examples/adc`) |
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

When an analog net feeds a digital engine, the **producing** analog engine
thresholds it (ADR-003 D3) and the value crosses the boundary as `Digital`:

- At every accepted solver point it applies
  `DigitalIn::threshold(previous, volts)` with hysteresis: from Low, the pin
  reads High once `volts >= vih`; from High, it reads Low once
  `volts <= vil`; otherwise it keeps the previous level. `vil < vih`.
- At a level change, the crossing time is linearly interpolated between the
  two straddling solver points, through `vih` (rising) or `vil` (falling),
  and rounded up to whole femtoseconds, but kept strictly after the earlier
  point and not after the later one. It is reported as a `Digital`
  transition of the producer's trace (`spec/time-grant.md`), so any number
  of changes per grant reach the consumer, each at its time. This is a pure
  function of the solver points, which the grant lattice makes partition
  independent (ADR-003 D2).
- The level at t = 0 is `threshold(Low, v(0))`.
- A glitch that starts and ends between two solver points (at most
  `TMAX = L` apart) is missed. A slow ramp, even with peak-to-peak ripple
  smaller than `vih - vil`, gives exactly one transition per direction.

`cin` is added to the analog netlist as a capacitor to ground on the net.
A lattice-bound consumer (Renode) applies each transition at `ceil_L(t)`.

### `adc_in{vref, bits}`

An MCU ADC input reading an analog net. It is a **push** model (ADR-003 D6):
the *producing* analog engine reads the net and the value crosses the
boundary as `Analog` transitions of its trace, only at lattice points:

- At every lattice point `kL` the producer reads the net at the solver point
  there (the grant lattice puts one on every lattice point, ADR-003 D2) and
  delivers `uV = clamp(round(volts * 1e6), 0, round(vref * 1e6))`, whole
  microvolts clamped to `[0, vref]` (real converters saturate; Renode's
  model does not).
- It emits a transition at `kL` only where the push key `floor(uV / 1000)`
  (whole millivolts) changes, delivering exactly that many millivolts. That
  is all Renode 1.17.0's `STM32_ADC_Common` converts:
  `code = round_half_even(floor(uV / 1000) / (vref * 1000) * (2^bits - 1))`
  (ADR-003 Amendment 5). So the rule is exact whatever resolution or
  alignment the firmware selects, and `bits` documents the code mapping
  only. A reference mode pushes `uV` at every lattice point; the converter
  sees the same values (`bb run --adc-push every`).
- The value at t = 0 is the value delivered at the initial solver point.
- The pin's boundary value is the last delivered value; a pin without a
  transition in the grant did not change.
- The consumer applies a push at its lattice point `t`, never earlier
  (Renode: External Control `ADC SetValue` on the channel while paused at
  `t`, splitting its advance there). A conversion therefore sees the value
  of the latest lattice point, at most `L` old, in every partition.
- A watched `adc_in` pin (`advance_until`, CPU lookahead) stops the
  producer at its first push: a change of the converted value is an input
  change of the CPU (the code-change watch).

No glue: the input is ideal (no load). The ADC channel follows from the pin
and the platform (STM32F0x2: `PA0`..`PA7` are channels 0..7, `PB0`..`PB1`
8..9, `PC0`..`PC5` 10..15); only platforms with an ADC model accept it.

### `analog`

The pin reads or drives an analog value directly, for example a DAC output.
ADC inputs use `adc_in`.

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
   capacitor (`C_bbin_<pin> <net> 0 {cin}`, omitted when `cin = 0`), and its
   value crosses the boundary as the thresholded `Digital` trace above.
3. Glue element names are a pure function of the pin and net names
   (`V_<pin>`, `R_<pin>_zout`, `C_bbin_<pin>` with every character other
   than an ASCII letter or digit replaced by `_`), so generated netlists are
   deterministic.
4. A `digital_out` that is not connected to the circuit (a board pin without
   `net=`, e.g. `examples/feedback-open` U1.PA6) gets no glue and loads
   nothing; its digital net can still be probed.
5. In Phase 0, `bb run` generates the glue from the board file's `*bb pin`
   directives (the analog engine writes it into the netlist). The automatic
   generator from schematics arrives with the KiCad importer, which must not
   parse KiCad with regex.
6. A net driven by an analog engine and loaded by an `adc_in` gets no glue;
   its value crosses the boundary as the pushed `Analog` trace above
   (ADR-003 D6).
