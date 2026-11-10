# Pin model (pin-boundary contract)

Status: normative. Implementation: `bbcore::pin`, `bbcore::signal`. Changing
this file requires an ADR. `digital_out` ramp start: ADR-003 D3 (plan step 3,
Amendment 3). `digital_in` thresholding by the producing analog engine and its
glue: ADR-003 D3 (plan step 4, Amendment 4). `adc_in`: ADR-003 D6 (plan step
9, Amendments 5 and 10), and ADR-005 D3/D5 (the code12 push key, raw
microvolt delivery, timed inputs and the sampled-input rule). Pin models
from packages, named parameter sets and glue rule 7: ADR-006 (D4, Phase 1
M3). Net naming in glue rule 3, glue rule 5 and rule 8 (unbound pins):
ADR-007 (D5, D6; Phase 1 M4, Amendment 1).

Every component pin has **exactly one** pin model, supplied by its package
from the package's `pins.toml` (ADR-006 D4, `spec/package.md`); `board.cir`
directives remain a way to state them by hand. A pin model may attach a
`pin_load` view to its own pad; the view's control and observe signals are
internal to the binding, not pins. The kernel connects pins to nets
and routes signals between engines. Each engine converts between its own domain
and a `PinSignal`, using the pin model. IBIS buffer models map onto these
models directly in later phases.

A pin model's parameters may be a named set (`digital_out.H0H1`), chosen per
binding by `set`; without `set` the set named `default` is used, and a
binding's explicit parameters override the set's (ADR-006 D4, ADR-007 D9).

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
  point and not after the later one. A solver point's time in fs is exactly
  `kL` when the point is on a lattice point (within the grant tolerance of
  it, the lattice clamp's own test), whether or not a grant ends there, and
  its time rounded to fs otherwise (ADR-003 Amendment 12). It is reported
  as a `Digital` transition of the producer's trace (`spec/time-grant.md`),
  so any number
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
- It emits a transition at `kL` only where the **push key** changes: the
  finest value the consumer's converter can see (ADR-005 D5).
  - Breadboard's STM32F072 ADC model (the default, `bb run --adc-model bb`):
    the key is
    `code12 = min(4095, round_half_up(uV * 4095 / Vref_uV))` with
    `Vref_uV = round(vref * 1e6)` (RM0091 13.9: `FULL_SCALE = 4095`;
    saturating, as silicon does; ADR-005 approver decision 2), and the
    delivered value is the **raw clamped `uV`** of the lattice point. The
    model converts exactly that mapping and derives every lower resolution
    and the analog watchdog from `code12` with low bits cleared, so the rule
    is exact whatever resolution or alignment the firmware selects. `bits`
    must be 12 and `vref` the model's reference (3.3 V; `bb` checks).
  - Renode 1.17.0's stock `STM32_ADC_Common` (`bb run --adc-model stock`,
    ADR-003 Amendments 5 and 10): the key is whole millivolts,
    `floor(uV / 1000)`, delivered as exactly that many millivolts, because
    the stock model converts
    `code = round_half_even(floor(uV / 1000) / (vref * 1000) * (2^bits - 1))`
    and nothing finer; `bits` documents the code mapping only.

  A reference mode pushes `uV` at every lattice point; the converter sees
  the same values (`bb run --adc-push every`).
- The value at t = 0 is the value delivered at the initial solver point.
- The pin's boundary value is the last delivered value; a pin without a
  transition in the grant did not change.
- The consumer applies a push at its lattice point `t`, never earlier.
  Breadboard's model: the push is a **timed input** (written to the model's
  host port before Renode runs past it); a conversion whose sample instant
  is `s` (the end of its sampling phase, a clock limit) converts the latest
  push with `t < s`, strictly earlier, so a sample exactly on a lattice point
  sees the previous cell's value. The stock model: External Control `ADC
  SetValue` on the channel while paused at `t`, splitting the advance there.
  Either way a conversion sees the value of the latest lattice point, at most
  `L` old, in every partition.
- On Breadboard's model the pin is a **sampled input**
  (`spec/time-grant.md`, ADR-005 D3): the model reads it only at sample
  instants, which the CPU's lookahead promise never passes, so the kernel
  does not watch it and a quiet grant does not stop at a push. The analog
  watchdog flag and interrupt appear at the end of a conversion only: an
  out-of-window push is reported at the next end of conversion of that
  channel (RM0091 13.8). On the stock model a watched `adc_in` pin
  (`advance_until`, CPU lookahead) stops the producer at its first push: a
  change of the converted value is an input change of the CPU (the
  code-change watch, ADR-003 D6).

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
   deterministic. Pin and net names are mapped to SPICE names by the rule
   of `spec/board-bbm.md` section 4 (net naming): the glue of a project's
   binding connects to its net's SPICE name (`/LED ring/LED_DIN_R` →
   `LED_ring_LED_DIN_R`, the simulated board's ground → `0`).
4. A `digital_out` that is not connected to the circuit (a board pin without
   `net=`, e.g. `examples/feedback-open` U1.PA6) gets no glue and loads
   nothing; its digital net can still be probed.
5. Glue is generated from the bindings of the elaborated board (ADR-007):
   a project's `[[bind]]` entries (`spec/project.md`), or a board file's
   `*bb pin` directives; the analog engine writes it into the netlist, with
   the parameters of the part's package where the part comes from one
   (ADR-006). Schematics are read through KiCad's own netlist export and an
   S-expression parser, never with regex.
6. A net driven by an analog engine and loaded by an `adc_in` gets no glue;
   its value crosses the boundary as the pushed `Analog` trace above
   (ADR-003 D6).
7. An analog view's subckt ports and internal nodes never include `gnd`
   (in any case): ngspice treats a node named `gnd` as global ground even
   inside a subcircuit (ADR-006 Context 4; `bb pkg lint` refuses it).
8. An unbound connected MCU pin (a project pin with no `[[bind]]`) gets no
   glue and loads nothing (high-Z), like an unconfigured nRF pin; its net
   is probe-only on the MCU side (ADR-007 D5).
