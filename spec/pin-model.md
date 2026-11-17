# Pin model (pin-boundary contract)

Status: normative. Implementation: `bbcore::pin`, `bbcore::signal`. Changing
this file requires an ADR. `digital_out` ramp start: ADR-003 D3 (plan step 3,
Amendment 3). `digital_in` thresholding by the producing analog engine and its
glue: ADR-003 D3 (plan step 4, Amendment 4). `adc_in`: ADR-003 D6 (plan step
9, Amendments 5 and 10), and ADR-005 D3/D5 (the code12 push key, raw
microvolt delivery, timed inputs and the sampled-input rule). Pin models
from packages, named parameter sets and glue rule 7: ADR-006 (D4, Phase 1
M3). Net naming in glue rule 3, glue rule 5 and rule 8 (unbound pins):
ADR-007 (D5, D6; Phase 1 M4, Amendment 1). `adc_in` on the nRF52840's
SAADC, the raw-microvolt push key, the loading `adc_in` (`load`, the `#smp`
toggle) and glue rule 9: ADR-008 D7 (Phase 1 M8, Amendment 4). The `pi`
form of `digital_out` and glue rule 11: ADR-009 D5 (Phase 2 P2, Amendment
5). CPU state outputs and glue rule 12: ADR-009 D4 (Phase 2 P3, Amendment 6).

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
| `digital_out` | `voh` V, `vol` V, `rise` Time, `fall` Time, `zout` Ω; optional `rail` (a supply pin) and `rhiz` Ω (`bb-pins/1`, ADR-009 D5: the `pi` glue of milestone P2; unused at `fast`) | **yes** (STM32 PA5) |
| `digital_in` | `vih` V, `vil` V, `cin` F | **yes** (STM32 PB0 in `examples/feedback`) |
| `adc_in` | `vref` V, `bits`; `model`, `load` (ADR-008 D7) | **yes** (STM32F072 PA1, ADC channel 1, in `examples/adc`; nRF52840 AIN0 in `examples/nrf-saadc`) |
| `analog` | none; bidirectional node, exchanged as volts | not yet (as a probe target only) |
| `power` | `vmin` V, `vmax` V, `imax` A | not yet: the name is reserved in `bb-pins/1` (refused as a model until milestone P1 defines it, ADR-009 D3) |
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

#### At `pi`: rail-tracking and tristate (ADR-009 D5, milestone P2)

At the `pi` rung (`bb run --fidelity pi`, or the bench's `fidelity`) an
MCU `digital_out` on a circuit net drives from its own rail and can be
high-Z. `s` is the level ramp above normalised to 0..1 (0 V / 1 V over
`rise`/`fall`), and `oe` the output enable, ramped the same way:

```
  V_bb_<id>     __pi_s_<id>  0 dc 0 external       ; s in 0..1
  V_bb_<id>_oe  __pi_oe_<id> 0 dc 0 external       ; oe: 1 = driving, 0 = high-Z
  B_bb_<id>_hi  <rail> <net> I = V(__pi_oe_<id>)*V(__pi_s_<id>)*(V(<rail>)-V(<net>))/{zout}
  B_bb_<id>_lo  <net> <vss>  I = V(__pi_oe_<id>)*(1-V(__pi_s_<id>))*(V(<net>)-V(<vss>))/{zout}
  R_bb_<id>_hz  <net> <vss>  {rhiz}
```

- `<id>` is `<part>_<pin>` with every character other than an ASCII
  letter or digit replaced by `_`. With `<vss>` the ground node `0`, the
  low side's term is `V(<net>)`.
- **`rail`** is the net of the package pin the binding's `rail` names, by
  default the package's `[power] io_rail`; a pin with neither is refused
  at `pi`. The rail's net must be in the analog scope. `<vss>` is the net
  of the package's ground pins (the board's ground when they are on none).
  `voh` and `vol` are not used (a binding that sets them gets a warning).
- **`rhiz`** (default 100 MΩ, a numerics floor: 33 nA at 3.3 V) gives the
  net a DC path in every state, so the operating point solves with every
  output high-Z.
- **`oe`** is the pin's `PIN_CNF.DIR` on the nRF52840: the CPU engine's
  output `<pin>#oe`, each `PIN_CNF`/`DIR`/`DIRSET`/`DIRCLR` write that
  changes the bit at the write's virtual time by the lattice rule of GPIO
  edges (the hook and a GPIO event read the same clock), low after reset
  (`PIN_CNF = 0x2`), so a pin is high-Z until firmware makes it an output.
  On every other platform, or with `bb run --pi-oe fixed`, `oe = 1` (a
  `dc 1` source). While a CPU is held in reset `oe` is 0 (milestone P5).
- A board file names no rail: only `bb run --operating-point --fidelity
  pi` builds its glue, with a stand-in rail `V_bb_<id>_rail
  __pi_rail_<id> 0 dc <voh>` (gate G9). `--operating-point` solves the DC
  operating point with every `oe` at 0.
- A device's `digital_out` keeps the form above at every rung.

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
  - Breadboard's STM32F072 ADC model (`BbStm32F0Adc`, ADR-005): the key is
    `code12 = min(4095, round_half_up(uV * 4095 / Vref_uV))` with
    `Vref_uV = round(vref * 1e6)` (RM0091 13.9: `FULL_SCALE = 4095`;
    saturating, as silicon does; ADR-005 approver decision 2), and the
    delivered value is the **raw clamped `uV`** of the lattice point. The
    model converts exactly that mapping and derives every lower resolution
    and the analog watchdog from `code12` with low bits cleared, so the rule
    is exact whatever resolution or alignment the firmware selects. `bits`
    must be 12 and `vref` the model's reference (3.3 V; `bb` checks).
  - Breadboard's nRF52840 SAADC (`model = "nrf_saadc"`, ADR-008 D7): the
    key is the **raw clamped `uV`** itself, delivered as is, on every
    change, because gain, reference and resolution are run-time choices of
    the firmware and no coarser key exists. `vref` is the clamp of the
    pushed value (ADR-008's `vmax`; for this model the VDD of the bound
    supply), and `bits` documents only. Because the pin is a sampled input,
    pushes cost batch size, not grants.

  Renode 1.17.0's stock ADC (`bb run --adc-model stock`, a whole-millivolt
  key applied with External Control `ADC SetValue`) is no longer a consumer:
  it was removed in Phase 2 H6 (ADR-015 D9), and an `adc_in` pin needs one
  of Breadboard's models.

  A reference mode pushes `uV` at every lattice point; the converter sees
  the same values (`bb run --adc-push every`).
- The value at t = 0 is the value delivered at the initial solver point.
- The pin's boundary value is the last delivered value; a pin without a
  transition in the grant did not change.
- The consumer applies a push at its lattice point `t`, never earlier.
  Breadboard's models: the push is a **timed input** (written to the model's
  host port before Renode runs past it); a conversion whose sample instant
  is `s` (the end of its sampling phase, a clock limit) converts the latest
  push with `t < s`, strictly earlier, so a sample exactly on a lattice point
  sees the previous cell's value. A conversion sees the value of the latest
  lattice point, at most `L` old, in every partition.
- On Breadboard's models (`BbStm32F0Adc`, ADR-005; `BbNrf52Saadc`,
  ADR-008 D7) the pin is a **sampled input**
  (`spec/time-grant.md`, ADR-005 D3): the model reads it only at sample
  instants, which the CPU's lookahead promise never passes, so the kernel
  does not watch it and a quiet grant does not stop at a push. The analog
  watchdog flag and interrupt appear at the end of a conversion only: an
  out-of-window push is reported at the next end of conversion of that
  channel (RM0091 13.8). (The code-change watch of ADR-003 D6, which
  stopped the producer at a watched `adc_in` pin's first push, served only
  the stock model, removed by ADR-015 D9.)

Without `load` (or with `load = "none"`) there is no glue: the input is
ideal (no load). The ADC channel follows from the pin and the platform, or
from the package's `adc = { channel }` (STM32F0x2: `PA0`..`PA7` are
channels 0..7, `PB0`..`PB1` 8..9, `PC0`..`PC5` 10..15; nRF52840: `AIN0`..
`AIN3` are `P0.02`..`P0.05`, `AIN4`..`AIN7` `P0.28`..`P0.31`, and an
`adc_in` on the supply pin `VDD` is the SAADC's VDD input, read by
`PSELP = VDD` and by the `VDD1_4` reference); only platforms with an ADC
model accept it.

**Loading (ADR-008 D7).** An `adc_in` with `load = "<view>"` names a
`pin_load` view of its package (`spec/package.md` section 4) that loads the
pad (the nRF52840's `saadc_se`: pad capacitance and a sample capacitor). The
CPU engine has one internal digital output per loaded pin, `<pin>#smp`,
which **toggles at every acquisition start** of that input, in either
direction: one edge per acquisition, stamped like a GPIO edge (the
acquisition's exact time, or 1 fs after it when it falls on a lattice
point). The view detects each edge itself and does the rest: it pre-charges
its sample capacitor to `VPRE` (by default `VDD`) for a short internal time
with the sample switch open, then lets it settle towards the pad, so the pad
gives up one charge transfer per acquisition however late the edge was
delivered (a toggle cannot be lost; two toggles could only cancel inside
one lattice cell, which the rule below excludes). The value pushed to the
model is the view's **observe** node (V(hold), the settled sample
capacitor), not the pad. A loaded input whose acquisition time is shorter
than `2 L` is refused (the model's error word fails the run; the message
names `--renode-quantum` and `load = "none"`); there is no automatic
fallback to an unloaded input. The pin is still a sampled input: `#smp`
changes only at the model's own clock limits or at CPU writes.

### `analog`

The pin reads or drives an analog value directly, for example a DAC output.
ADC inputs use `adc_in`.

### `power{vmin, vmax, imax}`

A supply pin. Later phases check rail voltage and current against these limits
and emit `assert` records (the kinds of `spec/project.md` section 3.4).

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
6. A net driven by an analog engine and loaded by an `adc_in` without
   `load` gets no glue; its value crosses the boundary as the pushed
   `Analog` trace above (ADR-003 D6).
7. An analog view's subckt ports and internal nodes never include `gnd`
   (in any case): ngspice treats a node named `gnd` as global ground even
   inside a subcircuit (ADR-006 Context 4; `bb pkg lint` refuses it).
8. An unbound connected MCU pin (a project pin with no `[[bind]]`) gets no
   glue and loads nothing (high-Z), like an unconfigured nRF pin; its net
   is probe-only on the MCU side (ADR-007 D5).
9. An `adc_in` with `load` (ADR-008 D7) gets its `pin_load` view's subckt
   on the pad, instantiated `X_bbload_<part>_<pin>` with the view's ports
   in order: the pad on the pin's net, the `ctrl` port on a generated glue
   net `__smp_<part>_<pin>` driven by one `EXTERNAL` source
   (`V_bb_<part>_<pin>_smp`, rule 1 with 0 V / 1 V, 1 ns edges and no
   `zout`) from the CPU engine's `<pin>#smp` trace, the `observe` port on
   `__hold_<part>_<pin>`, and a `pin` port on the net of that package pin's
   binding (the nRF52840's `VDD` input). The analog engine pushes the
   `observe` node.
10. An analog monitor is not glue (ADR-010 D6, milestone B2). An assertion
    on an analog net (`spec/project.md` section 3.4) adds no element, no
    input and no grant boundary: the kernel reads the analog engine's
    accepted solver points of the net, which an assertion makes an output of
    the engine (bound to its kernel net, in no output file) when no probe
    does. A run's outputs other than its `assert` records are those of the
    same run without the assertions.
11. At `pi` an MCU `digital_out` on a circuit net gets the rail-tracking,
    tristate glue of the `digital_out` section (ADR-009 D5) in place of
    rule 1, after the board body and the loads of rule 9. Its `oe` source
    is driven by the CPU engine's `<pin>#oe` trace (kernel net
    `<part>.<pin>#oe`) on the nRF52840, else it is `dc 1`. At `fast` no
    such glue exists and no `#oe` output is bound.
12. At `pi` an in-scope instance whose package has a `supply_load` view
    (`spec/package.md` section 3) gets that view beside its other views,
    instantiated `X_bb_idd_<ref>` (`<board>_<ref>` on a system board), its
    pin ports on the nets of the package's pins of those names (a module
    target: through its pinmap), its parameters `i_<state>` and `vdd_min`
    from `[power]`, then the view's own. Each state port `#<state>` is a
    node `bb_st_<ref>_<state>` driven by one source
    `V_bb_st_<ref>_<state>`:
    - the CPU engine's **state output** `#<state>` (kernel net
      `<ref>.#<state>`), rule 1 with 0 V / 1 V, 1 ns edges and no `zout`,
      for the target MCU's states the platform reports. On the nRF52840
      (`bbState`, ADR-009 D4): `#run`, 1 while the CPU executes and 0
      from the instant after a WFI/WFE instruction to the wake (1 after
      reset); `#hfxo`, set by `TASKS_HFCLKSTART` and cleared by
      `TASKS_HFCLKSTOP` at the write's virtual time; `#nvmc`, the NVMC
      model's busy (an erase); `#saadc`, the SAADC model's active (a
      buffer started or a scan in progress). Each change is stamped like a
      GPIO edge (an instant on a lattice point is reported 1 fs after it)
      and delivered, never coalesced. A state the view has no port for is
      not bound;
    - a bench `[[activity]]` (`spec/project.md` section 3.2): a table of
      the analog engine, labelled bench-scheduled in `run.json`;
    - otherwise `DC 0`.
    A bench `[[load]]` with `replace` naming the instance removes the view
    (static-load mode). At `fast` no `supply_load` view and no state output
    is bound. A sleeping CPU's `quiet_until` covers its state outputs: they
    change only at a wake, at a CPU write or at a model's own clock limit,
    which the promise already bounds.
