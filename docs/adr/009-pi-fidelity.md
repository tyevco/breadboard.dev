# ADR-009: `pi` fidelity (power views, idd, rail-tracking pins, POFWARN and brown-out)

- **Status:** Accepted (2026-11-15; see "Approver decisions
  (2026-11-15)"). Proposed 2026-11-14, revised after design review round 1
  (see "Review notes"). Written before implementation, because it changes
  the package schema, the pin model and the event stream (AGENTS.md).
- **Amendments:** 1 (2026-11-15), the P0 report: replicas, option set
  `phase0`, the model difference, W2's tolerances; W1's feasibility row
  fails, so P0 stops for the approver's Q1 choice. 2 (2026-11-15), the
  approver's decision D-W1: W1 at a labelled stress corner (55 mA, RT9080
  worst) with its own tolerances, plus a gated 50 mA margin check; P0
  closed. 3 (2026-11-15), milestone S: this ADR's schema keys landed,
  accepted and inert (ADR-011 Amendment 1). 4 (2026-11-15), P1 as
  implemented: the `pi` packages, the TPS22917 restated (decision D-TPS),
  the `pi` lint, the dial, corners, the battery, and W1/W2 in static-load
  mode on the package views (`make pi-static`). 5 (2026-11-16), P2 as
  implemented: the rail-tracking, tristate glue, `oe` from `PIN_CNF.DIR`
  (the prototype: exact, within the 5 %), G9 on twenty Phase 1 decks, E1
  cold's LSW-3 without the pessimism, G8 (`make pi-glue`). 6 (2026-11-16),
  P3 as implemented: the nRF52840's `[power] idd` and `supply_load` view,
  the CPU state outputs (`bb_nrf52_state.cs`; Renode's WFI-entry hook comes
  a slice late, so the entry is timed by instruction count), `[[activity]]`
  and `[[load]] replace`, G6 (`make pi-idd`), P2's `pi` references
  re-recorded.
- **Part of:** the Phase 2 plan (`docs/phase2-plan.md`), milestones P0-P6
  and W-fw; worked examples W1 and W2. Its schema keys land in milestone S
  (the schema epoch).
- **Normative changes when implemented:** `spec/package.md`,
  `spec/pin-model.md`, `spec/event-stream.md` (joint version 2 with
  ADR-010), `spec/project.md` (bench keys, with ADR-010) and
  `spec/schema/part.schema.json`. `spec/time-grant.md` does not change.
- **Evidence:**
  - the `pi` research pass (`~/bbscratch-p2-pi`): E1's elaborated deck
    with a prototype RT9080 view, a battery, an nRF52840 idd load and a
    rail-tracking LED_PWR_EN, run through `NgspiceEngine` (`deck_run`);
  - the nRF52840 PS v1.1 and the RT9080 DS9080-09 (text in
    `~/bbscratch-p2-pi/dl/`);
  - the vendored MDK `nrf52840_bitfields.h`;
  - `examples/dial-me-spinner/suite/results.json`;
  - the review round's runs of the suite's own W1 and W2 decks
    (`~/bbscratch-p2-rev`; plan section 2).

## Summary

- `--fidelity pi` becomes a working rung.
- Power-path parts get averaged `pi` views written as B conductances. The
  RT9080 view is restated from its datasheet; the suite's file is a
  reference, not a source.
- MCU packages get a `supply_load` view driven by CPU state outputs.
- At `pi`, `digital_out` drives from its own rail, and goes high-Z (with a
  stated leakage) in reset and while its pin is an input.
- The nRF52840 gets a POWER model: POFWARN comes from an ngspice
  comparator on VDD at the codes the chip has, and a brown-out from VBOR
  holds the CPU in reset and emits a `reset` record.
- Package limits become assertions, and every assertion reports the rung
  of the models it read.
- `fast` results do not change by one byte.

## Context

**What exists.**

- Only `fast` runs: `cli/src/main.rs` refuses every other rung.
- Views may declare `pi`, but none does. Every first-party view, the
  generic R/C/L included, declares `fidelity = ["fast"]`. ADR-006 D3 says
  the TPS22917 `lsw` view carries `["fast", "pi"]`; the committed
  `part.toml` says `["fast"]`, so ADR-006 D3 is wrong on this point.
- Keys reserved for `bb-part/1` and refused today: top-level `power`, a
  view's `rails`, `[pins] ibis`, `[behavioral] cpu`/`svd`, `thermal`,
  `rtl`.
- `power{vmin, vmax, imax}` and `analog` are "not yet" in
  `spec/pin-model.md`.
- E1 ran with `voh` at the bench's 3.3 V while the cold rail was 2.93 V
  (pessimistic for LSW-3; ADR-008 Amendment 3).
- The engine already hooks `PIN_CNF` writes on the nRF52840 for the audit;
  each write becomes a `log` record at its own virtual time
  (`engines/cpu-renode/src/lib.rs`, `pin_cnf_writes.log`).
- Phase 1 deferred:
  - S4, the dead-battery OLED brown-out loop;
  - S5, sag, POFWARN and SHIP.

  Both "need a POWER/reset model driven by an analog rail" (Phase 1 plan
  §2).

**What the suite's references are built from** (read for the review):

- +3V3 in the W1/W2 decks is an ideal LDO, `B3V3 n_p3v3 0 V = min(3.3,
  V(n_vsys) − I3V3·LDO_RDO)`, with a static 50 mA drawn from VSYS. The
  suite's RT9080 file (`ldo_3v3__rt9080.lib`) is used by other tests and
  is itself built from about 14 V-type B sources.
- The suite's BQ25180 (`load_switches__bq25180.lib`) is an XSPICE `ilimit`
  block; BUVLO and the supplement gating are not modelled, and a cell
  below BUVLO is `vbat = 0`.
- The W1 reference ran at `reltol=1e-3` (fallback level 1). At
  `reltol=1e-4` it fails in the TI-derived TPS22917's V-type B branch. At
  `phase0` it needs Gear: 5.7 s, within 25 µV and 2.2 µs of the reference
  (measured).

**Measured (prototype, 120 ms, `phase0`, lattice 10 µs, 1 ms grants):**

| Case | Wall | Solver points | Result |
| --- | --- | --- | --- |
| nominal, motor averaged | 3.7-4.0 s | 28 252 | VLED min 3.336 V; +3V3 min 3.2976 V |
| nominal, motor PWM 25 kHz | 18.8 s | 128 633 | VLED min 3.335 V |
| cold, PWM | 17.9 s | 126 251 | +3V3 below 3.045 V for 3.2 ms, min 2.953 V |
| nominal + CPU wake at 1 kHz / 10 kHz | 4.6 s / 8.6 s | +9 % / 2.2× | |
| S4 at ILIM 100 mA, 60 ms | 0.96 s | 6 087 | VSYS parks at 1.567 V; +3V3 decays to 1.639 V (above VBOR,ON max) |

- The `nets.csv`-style outputs were byte-identical at 10 µs, 1 ms and
  10 ms grants. Solver-point minima and maxima were bit-identical across
  partitions.
- **Numerics findings:**
  - a three-argument `min()` segfaults libngspice 42 in the process;
  - a flat B-source region (no conductance above regulation) fails the
    operating point after 31 s;
  - a node whose only DC path is switched off (the first draft's `oe = 0`
    glue on a 10 pF net) gives "singular matrix", fails gmin and source
    stepping, and recovers only through ngspice's "Transient op" fallback
    (review measurement);
  - inline PWL playback is O(n²): 103 s for 100k points;
  - a 2 MHz switching converter would take about 1.5 h per simulated
    second;
  - PWM-resolved loads cost about 4.6×;
  - a 30 nH series inductor converged at +68 % cost.

**Hardware facts verified** (PS v1.1, MDK bitfields):

- POFCON.THRESHOLD V17..V28 (codes 4..15; 1.7-2.8 V), VPOFTOL ±5 %,
  VPOFHYST 40/50/60 mV. There is no 2.9 V code in normal-voltage mode;
- POFWARN also fires when VDD is already below the threshold at enable or
  at reconfiguration;
- POF blocks NVMC writes;
- VBOR,ON 1.57/1.60/1.63 V;
- a brown-out reset clears RESETREAS, the retained registers and GPIO;
  RAM is not reset but "may be corrupted";
- `PIN_CNF` resets to input with the input buffer disconnected (0x2).

**Current figures** (3 V, DC/DC, typ):

- ICPU0 3.3 mA;
- ION_RAMON_RTC 3.16 µA;
- ITX at +8 dBm 14.8 mA (IRADIO_TX0 16.4 mA);
- TX at 0 dBm 4.8 mA; RX 1M 4.6 mA;
- ISAADC,RUN 1.24 mA.

Renode's `NRF_CLOCK` has no POFCON, RESETREAS, SYSTEMOFF or GPREGRET
(checked on GitHub master).

## Decision

### D1. The fidelity dial

- `bb run --fidelity pi`, or the bench's `fidelity = "pi"` (ADR-010). The
  command line wins.
- View selection is ADR-006 D3's ladder, unchanged: the per-instance
  override, else `default_for` the rung, else walking **down**.
- `run.json` already records, per instance, the requested rung and the
  rung it was chosen at.
- **New honesty rule.** Every `assert` record (ADR-010) carries `rung`:
  the lowest chosen rung over the in-scope instances **with a port on a
  net the assertion reads**.
- An assertion declared `requires = "pi"` is `not_evaluable` when that
  rung is lower. It never passes silently on a `fast` model.
- **Existing views at `pi`.** A view whose behaviour does not depend on
  the rung (the generic R/C/L) or that is adequate at `pi` (TPS22917,
  WS2812C, after a check) declares `["fast", "pi"]` only after it is
  re-qualified at `pi`'s qualification set per ADR-006 D6. P1 does this
  for every view on W1's and W2's nets, and a gate checks that W1's
  assertions report `rung: pi`. There is no exemption by view kind:
  an exemption would be a rule nobody checks.
- **At `pi` only:**
  - the `digital_out` glue becomes rail-tracking (D5);
  - `supply_load` views enter the deck (D4);
  - POWER inputs are bound (D6, D7);
  - package limits become assertions (D3).
- At `fast` the deck and every output are byte-identical to Phase 1
  (gate G1).

### D2. `pi` power views: the rules

Each rule follows from a measurement:

1. **Pass elements are B conductances.**
   - The current is `I = f(V(nodes))`, with a sensed current computed from
     node voltages.
   - No branch-current unknown anywhere in a `pi` view: no 0 V ammeter, no
     CCVS, CCCS or V-type B/E source (ADR-006 Amendment 2). An internal
     signal (a limit, a soft-start ramp) is an I-type B source into a 1 Ω
     conductance, not a V-type source.
   - Lint refuses those in a view marked `pi`. There is no exemption for
     "signal nodes": lint cannot tell a signal node from a current loop,
     and the TI-derived TPS22917 failed in exactly such a branch.
2. **A DC conductance at every operating point**, in every state of the
   view and of the glue (D5's high-Z state included).
   - Every branch of a `min`/`max`/limit expression keeps a slope in the
     region the operating point lands in; a limiter is written linear
     through it.
   - `bb pkg test` solves the operating point at every qualification
     corner and fails fast: an iteration cap, not ngspice's own
     give-up after 31 s (ADR-015 D4).
3. **Switching converters are averaged.** Efficiency comes from an
   η(Vin, Iout) table, from the datasheet figures (ADR-012's figure
   claims); power-save ripple is an optional parameter. Switching views
   are `si` (Phase 3).
4. **Expressions use ngspice's function table at its arity** (ADR-015 D3
   lint). In particular `min` and `max` take exactly two arguments.
5. **A series L with milliohms is a lint warning**, with its measured cost.

First views:

| Package | Model | Provenance and reference |
| --- | --- | --- |
| `richtek/rt9080` | LDO: `min(ILIM, GM·(VSET·ss − Vout), (Vin − Vout)/RDROP)`, soft start, Iq, foldback, EN discharge, written to rule 1 | **restated** from DS9080-09 (no vendor model exists). The suite's `ldo_3v3__rt9080.lib` is a measured reference only (P0 compares them on its cases), so no file is migrated and no licence grant is needed |
| `ti/bq25180` | ILIM, SYS_REG, RON_IN/RON_BAT, supplement mode, BUVLO 3.0 V for 60 µs, IBAT_OCP; ILIM and SYS_REG as instance parameters. A **`suite_compat` corner** (D8) turns off what the suite does not model (BUVLO timing, supplement gating) so static-load mode compares like with like; the datasheet corners are separate | datasheet-fitted |
| `ti/tps61040` | averaged constant-power input, soft-start surge, UVLO | datasheet-fitted. The suite's port of TI SLVMDA8 stays local-only (the TPS22917 rule) |
| `ti/tps63802` | averaged buck-boost, η table | datasheet-fitted (caliper) |

**The battery** is a bench element (ADR-010): an OCV table, R0 and RC
pairs, all written as B and C elements.

**Static-load tolerances are measured, not asserted.** P0 runs the suite
replica (the suite's own models through `deck_run`, local-only) and the
`pi` views in static-load mode, both at the qualified option set, and
fixes each W1/W2 tolerance from the measured difference before P4 starts.

### D3. The `power` pin model and package limits

- `power{vmin, vmax, imax}` becomes live.
- At `pi` (and at `fast` with `--limits`), every in-scope `power` pin gives
  an automatic `range` assertion (ADR-010 D5) named
  `limit:<ref>.<pin>:<key>`, carrying the package's provenance.
- Current is computable only where the view senses it from node voltages.
  Otherwise `imax` is `not_evaluable`.
- **New `pins.toml` key `limits`** (`bb-pins/1`, milestone S), a list of
  relational limits for absolute maximum ratings: `{ pin = "DIN", max =
  "VDD + 0.7V", source = "<ref>" }`.
- The grammar is `<pin or rail> ± <value>`: one supply-plus-offset term,
  no general expressions. This turns E1's LSW-3 into a package fact.
- At `fast`, limits stay off by default, so Phase 1's event streams do not
  change (G1).

### D4. idd: the `supply_load` view and CPU state outputs

- **New view kind `supply_load`** (`pi`).
  - Its ports are the package's supply pins (typed `pin` ports, as
    `pin_load` has) and state ports named `#<state>`.
  - The view draws `I = Σ s_k · I_k`, with each `s_k` a node in 0..1 and
    `I_k` from the table.
  - The current collapses linearly below the operating VDD minimum (a
    conductance at every operating point, D2).
  - The DC/DC makes the load closer to constant power. Scaling as P/VDD
    is a view parameter, off by default, until it is verified.
- **New top-level `[power]` table** (the reserved key accepted in
  `bb-part/1`): `idd = { <state> = { typ = "3.3mA", max = "…", ref =
  "PS v1.1 Table …" } }` and `vdd_min`.
- **CPU state outputs** are internal digital outputs of the CPU engine,
  following the `#smp` precedent (ADR-008 D7, glue rule 9). Each is an
  `EXTERNAL` 0/1 source on the view's state port, stamped like a GPIO
  edge.

  | State | Source in Renode 1.17.0 | Exactness |
  | --- | --- | --- |
  | `#run` | the WFI hook `TranslationCPU.AddHookAtWfiStateChange` (already used by `bb_lookahead.cs`) | exact virtual time |
  | `#hfxo` | writes to `NRF_CLOCK` TASKS_HFCLKSTART/STOP, through a hook in the overlay | exact |
  | `#nvmc` | Breadboard's NVMC model busy | exact; NVMC write timing is still a gap (ADR-008 Amendment 9) |
  | `#saadc` | Breadboard's SAADC model busy | exact |
  | `#radio_tx`, `#radio_rx` | **not from firmware.** Bench `[[activity]]` (ADR-010) drives them, and the report labels them "bench-scheduled radio" | stand-in |

- **Lookahead.** A sleeping CPU's `quiet_until` must cover its state
  outputs. They change only at a wake, which the existing promise already
  bounds. A test firmware proves that no state edge falls inside a
  promise (gate G6).
- **No coalescing.** Every transition is delivered (ADR-003 D3). The cost
  is measured: +9 % solver points at a 1 kHz wake, 2.2× at 10 kHz.
- **Static-load mode.** A bench `[[load]]` with `replace = "<ref>"`
  (ADR-010) removes the `supply_load` and puts the suite's DC load in its
  place, where the suite places it. The firmware still runs. Every
  cross-check against the suite uses it.
- **Unmodelled consumers.** The suite's 50 mA is a board budget: the
  flash, the sensors and the OLED logic have no `supply_load` yet. A
  co-simulation that wants the budget adds a bench "remainder" `[[load]]`
  up to it, and the report labels it. W1's dip is a cliff in this load
  (no crossing of 2.94 V at 40 mA, collapse at 60 mA, measured), which is
  why W1 gates in static-load mode and reports co-simulation.

### D5. Rail-tracking and tristate `digital_out` (at `pi`)

The glue changes from a fixed source plus `zout` to two linear B sources,
two control nodes and a fixed high-Z leakage. `s` is the ramped level in
0..1 (today's ramp, normalised), and `oe` is the output enable:

```
V_<pin>     <pin>_s  0 dc 0 external         ; s in 0..1, the ramp of today's rule 1
V_<pin>_oe  <pin>_oe 0 dc 0 external         ; 1 = driving, 0 = high-Z
B_<pin>_hi  <rail> <net> I = V(<pin>_oe)*V(<pin>_s)*(V(<rail>)-V(<net>))/{zout}
B_<pin>_lo  <net> <vss> I = V(<pin>_oe)*(1-V(<pin>_s))*(V(<net>)-V(<vss>))/{zout}
R_<pin>_hz  <net> <vss> {rhiz}                ; the high-Z state's DC path
```

- `<rail>` is the net of the package pin the binding names: a new pin
  model parameter `rail = "VDD"`, defaulting to the package's `[power]
  io_rail`. `voh` and `vol` are ignored at `pi` (a warning if set).
- VOH follows the rail, the output current comes from the right rail, and
  with `oe = 0` the pin is high-Z.
- **The high-Z leakage `rhiz`** gives every bound net a DC path in every
  state (D2 rule 2). Its default is 100 MΩ, a numerics floor, recorded in
  `run.json` and stated as such. A package may set it from its datasheet's
  input leakage, with a claim. At 3.3 V it draws 33 nA, below every
  current any Phase 2 check reads.
- **What drives `oe`:**
  - **primary: the pin's `PIN_CNF.DIR`**, from the engine's existing
    `PIN_CNF` write hook, delivered as a trace at each write's virtual
    time (exact, like a GPIO edge). After reset `DIR` is input, so
    `oe = 0` until firmware configures the pin;
  - **always 0 while the CPU is held in reset** (D7);
  - **fallback**, if the hook trace costs more than 5 % or is not exact:
    `oe = 1` except while held in reset (Phase 1's behaviour plus reset).
    Default 3 below decides after P2's prototype. Either source gives a
    DC path in every state, through `rhiz`.

  Renode's nRF GPIO keeps driving after a switch to input (Phase 1 R5).
  With the primary source, the glue models the chip instead.
- The prototype's Norton form ran: LED_PWR_EN max was 3.298 V while
  tracking +3V3.

### D6. POFWARN from the rail

- **New model `BbNrf52Power`**, in Breadboard's plugin assembly. It
  replaces or wraps Renode's `NRF_CLOCK` at the POWER/CLOCK base in the
  overlay (risk R4 of the plan; prototype first). It implements:
  - POFCON;
  - EVENTS_POFWARN, its INTEN and the shared POWER_CLOCK interrupt;
  - RESETREAS;
  - GPREGRET/GPREGRET2;
  - TASKS_CONSTLAT/LOWPWR as no-ops.

  This is a new platform major, `nrf52840-bb@4`.
- The model drives internal outputs `#pofen` and `#pofthr0..3` (the
  threshold code).
- **A threshold code that does not exist is refused** in normal-voltage
  mode: codes 0-3 and VDDH thresholds are out. The refusal is a `log` at
  error level, and the run fails, naming the register write.
- **The comparator** is a `pi` view in `nordic/nrf52840-qiaa`
  (`pof_comparator`):
  - it compares V(VDD) against the code's voltage, using `#pofthr*` as
    state nodes;
  - hysteresis and the tolerance corner (VPOFTOL −5 %, 0 or +5 %) are
    instance parameters, set by the bench's corner (D8).
- **Which corner a check gates on.** A firmware rule that must survive any
  part is checked at the corner where the comparator trips earliest,
  VPOFTOL +5 %. W1 gates there (2.94 V at V28), and reports nominal and
  −5 %. At nominal V28 the suite's own W1 deck never reaches the
  threshold (+3V3 min 2.8666 V), so a nominal-only gate would be vacuous.
- The comparator's output returns as the watched `digital_in` `#pofwarn`,
  thresholded by the producing analog engine as any `digital_in` is
  (glue rule 2). The model raises EVENTS_POFWARN on its rising edge. This
  is an interrupt source like EXTI, so it is **not** a sampled input.
- The "already below at enable" rule: `#pofen`'s rising edge, or a
  threshold change, with `#pofwarn` already high raises the event at the
  next lattice point. W1's firmware blanks POFWARN by disabling POFCON and
  re-enabling it, so this rule is what sheds the cue when a dip outlasts
  the blanking.
- Every EVENTS_POFWARN raise is a `log` record of the model, so a check
  can count it (W1's anti-vacuity assertion).
- POF blocks NVMC writes. The NVMC model refuses a write while `#pofwarn`
  is high, with a `log` record.

### D7. Brown-out and reset

- ngspice thresholds V(VDD) against VBOR (1.57/1.60/1.63 V by corner) into
  the internal digital input `#bor`.
- **While `#bor` is high**, the Renode engine holds the machine in reset:
  - no instructions;
  - every bound `digital_out` has `oe = 0` (D5; the leakage keeps the DC
    path);
  - `supply_load` draws the reset current.
- **On its fall**, the engine resets the machine at `ceil_L(t)` and
  releases it.
- **The reset's effects:**
  - RESETREAS = 0 (a brown-out leaves no reset reason);
  - retained registers are cleared;
  - `PIN_CNF` returns to its reset value, so `oe = 0` until firmware
    configures each pin again;
  - RAM is **kept** (deterministic, and the datasheet allows it). The
    `reset` record says `ram = "kept"`, so a firmware that relies on RAM
    surviving is visible.
- **New event record `reset`** (event stream version 2):
  `{"type":"reset","t_fs":…,"source":"<engine>","cause":"bor","phase":"enter"|"exit"|"stop"}`.
  It sorts like a `log` of its engine.
- **Unverified, so P5 starts with a one-week prototype:**
  - whether Renode 1.17.0 can reset a machine under External Control (a
    monitor `machine Reset` while paused) with the GPIO hooks and the
    plugin's state intact, both fresh and on a leased pool process;
  - whether a reset changes the External Control message count or the
    lookahead plugin's self-test;
  - whether the P1 views give a co-simulated crossing of VBOR. The
    suite's deck crosses it at any +3V3 load from 5 to 50 mA (measured),
    but Breadboard's provisional views held +3V3 at 1.639 V over 60 ms.
- **Fallback** if the reset cannot be done: `#bor` ends the run with a
  `reset` record and `cause = "bor"`, `phase = "stop"`. The plan's
  section 5 item 2 makes this the stated exit criterion in that case
  (first brown-out only), and the deviation is recorded.
- Below VDD minimum but above VBOR the CPU keeps running with no model of
  misbehaviour. The `power{vmin}` assertion (D3) reports it.

### D8. Named corners

- A view may declare `[[analog.corners]]`: a name (`typ`, `min`, `max`,
  `worst_<x>`, `suite_compat`) and parameter overrides, each with a `ref`.
- The bench selects `corner = "<name>"` globally, or per instance.
- `run.json` records the corner of every instance.
- Corners are deterministic. Seeded Monte Carlo is out of Phase 2.

### D9. PWM-resolved against averaged loads

A load driven by a PWM (the LED ring, the DRV8837 VM current) may have an
averaged view driven by a duty. That view is the default at `pi` only
after a gate shows its droop within 10 mV of the switched view on the
suite's aligned-PWM case (W3). Until then the switched view is the
default, and the cost row shows the 4.6×.

## Exact `spec/` changes (described, not applied)

All key changes land in milestone S (the schema epoch), together with the
other Phase 2 majors.

- **`spec/package.md`:**
  - `bb-part/1` accepts top-level `power` (D4) and a view's `rails`
    (informative supply ports for lint).
  - Section 3 gains view kind `supply_load`, typed `#state` ports and
    `[[analog.corners]]`.
  - Section 4 (`bb-pins/1`) gains `limits` and the pin model parameters
    `rail` and `rhiz`.
  - Section 11 (lint) gains the `pi` view rules of D2: the function table
    and arity, no branch-current unknown, the L warning, and an operating
    point at every corner.
  - `bb pkg migrate` rewrites `bb-part/0` to `/1` mechanically.
  - The ADR-006 D3 sentence about the TPS22917 `pi` view is corrected.
- **`spec/pin-model.md`:**
  - `digital_out` gains the `pi` glue form (D5), `rail`, `rhiz` and the
    `oe` sources.
  - `power` is defined (D3).
  - Internal pins `#<state>`, `#pofen`, `#pofthr<n>`, `#pofwarn` and `#bor`
    get glue rules 10-12.
  - The table's "Phase 0 use" column becomes "since".
- **`spec/event-stream.md`:** version 2 (joint with ADR-010). It adds the
  `reset` record, and `rung` on `assert`.
- **`spec/project.md`:** bench keys `fidelity`, `corner`, `[[load]]
  replace` and `[[activity]]` (specified in ADR-010).
- **`spec/package.md` section 9:** the platform table gains
  `nrf52840-bb@4`.

## Determinism impact

- **State outputs and `oe`** are edges from Renode's virtual time: integer
  fs, partition-independent under the lattice rule (ADR-003 D2). They are
  delivered as traces, never coalesced.
- **`#pofwarn` and `#bor`** come from the `digital_in` crossing machinery:
  interpolated and rounded up to fs, a pure function of the solver points.
- **Corners, limits and `rhiz`** are inputs, recorded in `run.json`.
- **The reset record** sorts like a `log` of the Renode engine at its
  `t_fs`.
- RAM kept across a brown-out avoids any pseudo-random corruption model.
- **`fast` is unchanged**: no new glue, no new records, unless `--limits`
  is given.

## Gates

| # | Gate |
| --- | --- |
| G1 | Every committed `fast` hash unchanged (`make test`, `make dms-rail`, `make dms-vsense`, `make dms-home`) |
| G2 | Every `pi` view: lint clean; operating point at every corner; lattice-qualified at the qualified option set through the engine (ADR-006 D6); vectors from datasheet tables. Every existing view on W1/W2's nets re-qualified at `pi` |
| G3 | P0: the suite replicas reproduce `results.json` (1 mV, 10 µs); the static-load tolerances fixed from the measured difference; W1's feasibility row (plan section 2), at the labelled stress corner, and the 50 mA margin check (Amendment 2) |
| G4 | W1 with the firmware in the loop, at the stress corner (Amendment 2): the rule passes with `pofwarn` fired; both negative controls fail exactly `cue-held`; every W1 assertion reports `rung: pi`. W2, or its fallback (plan section 5) |
| G5 | Partition independence: each worked example at 10 µs, 1 ms and 10 ms grants; one-cell reference equality except grant records; analog-first byte-identical; 10-run determinism |
| G6 | Lookahead: a sleeping nRF with state outputs keeps long grants (≤ 2× its grants without them); no `PromiseBroken`; a firmware that wakes every 1 ms gives the predicted grant count. idd: the total VDD current of a gate firmware with a known WFI duty equals a hand calculation of the duty-weighted average within 1 % (not a check of the view against its own table) |
| G7 | `BbNrf52Power` register and timing tests (every code, hysteresis, enable-while-below, refused codes, NVMC block); `make pool-equiv` with the `@4` configurations and a reset case |
| G8 | Rail-tracking: E1 cold at `pi`, LED_PWR_EN's high level under a stated load (the TPS22917 ON input plus a bench load drawing the PS's IOH condition) against the PS VOH-versus-IOH figure (VOH ≥ VDD − 0.4 V at that current), not against the unloaded rail, which holds by construction |
| G9 | High-Z: the DC operating point of every Phase 1 board's deck at `pi` solves with every `oe = 0`, without ngspice's "Transient op" fallback |

## Rejected alternatives

- **Pushing VDD to the POWER model as a sampled value.** The comparator is
  asynchronous, so the input would not be sampled (ADR-005 D3), and the
  push key would depend on the threshold.
- **Switching converter views at `pi`.** At about 1.5 h per simulated
  second, they are not usable.
- **Instruction-level energy (per-opcode idd).** No datasheet gives it,
  and nothing would gate it.
- **Changing the `fast` glue to rail-tracking.** It would rewrite every
  golden for no `fast` question.
- **A `logic` rung now.** No Phase 2 example needs it. Runs without
  analog already exist for boards with no analog scope.
- **XSPICE digital comparators for POF and BOR.** They would add a second
  digital domain inside ngspice. The `digital_in` machinery already gives
  exact crossings.
- **Seeded Monte Carlo** before named corners have a gate.
- **Migrating the suite's RT9080 file.** It is built from V-type B
  sources, which rule 1 refuses, and W1's reference does not use it. It
  stays a reference.
- **A lint exemption for V-type B sources on "signal nodes".** Lint cannot
  tell them from a current loop; the I-into-1 Ω form costs nothing.
- **Gating W1 at nominal V28.** POFWARN never fires there in the suite's
  own deck; the gate would pass vacuously.
- **Gating W1's co-simulation.** The dip is a cliff in an unmodelled load
  budget (D4); a gate on it would test the remainder load, not the model.

## Questions for the approver

Consolidated in the plan's section 9: **Q1** (W1 at V28 and the +5 %
corner, with the stress-corner fallback), **Q2** (W2 conditional) and
**Q12** (the POFWARN finding for the user).

**Defaults (apply unless the approver objects):**

1. **RAM on a brown-out:** kept, flagged `ram = "kept"`. A corruption
   model has no datasheet basis.
2. **Radio current:** bench-scheduled `[[activity]]`, labelled, until a
   faithful radio exists.
3. **`oe` source:** `PIN_CNF.DIR` through the existing hook if P2's
   prototype shows it exact and under 5 % cost; otherwise `oe = 1` except
   in reset.
4. **BQ25180 ILIM and SYS_REG as bench parameters** until a bus device
   exists, recorded as a deviation in W1/W2.
5. **STM32 idd tables:** deferred until ST's datasheet is obtained and
   checked (it could not be downloaded in research); nRF52840 only.
6. **`rhiz` default 100 MΩ** as a stated numerics floor.
7. **The dial-me-spinner model grant** is no longer needed for the RT9080
   (restated). If a later view migrates a suite file, the grant is asked
   then.

## Approver decisions (2026-11-15)

All twelve questions of the plan's section 9 were accepted with their
recommended answers (plan section 10). For this ADR:

- **Q1, accepted.** W1 is gated at V28 and the VPOFTOL +5 % corner
  (2.94 V), with the firmware in the loop in static-load mode (the
  suite's 50 mA budget), and the negative controls derived from the
  measured window (blanking off; blanking that ends inside the window).
  The co-simulation is reported. The stated fallback is accepted: if P0's
  feasibility row fails on the `pi` views, the static load is raised to
  the smallest whole mA at which the deck crosses 2.94 V for 0.5 ms,
  labelled a stress corner. **Invoking it is the approver's choice** on
  P0's report, before P4 starts; an implementer stops and reports.
- **Q2, accepted.** W2 counts fully only if P5's prototype passes;
  otherwise the run-ending fallback of the plan's section 5 item 2.
- **Q12, accepted.** firmware.md's 2.9 V finding goes to the user as a
  dial-me-spinner issue. D6 refuses threshold codes that do not exist.
- **Q3, accepted.** This ADR's schema keys land in milestone S, before any
  `/1` file reaches `main`.
- **Build scope.** P0-P5 and W-fw are in this build; P6 (W3) is not.

**Defaults.** No objection was made to the defaults listed under
"Questions for the approver", so they apply as written.

## Review notes (design review round 1, 2026-11-14)

**Adopted:**

- **W1 cannot discriminate at nominal V28** (blocker): D6 states which
  corner a firmware rule gates on and why; G3 adds the feasibility row
  and G4 the anti-vacuity check; the measured window is in the plan.
- **High-Z glue without a DC path** (both critiques): `rhiz` in D5, rule 2
  extended to every glue state, G9. The `oe` source is now consistent
  (the existing `PIN_CNF` hook, or "1 except in reset"), with no
  dependency on a hook the first draft had deferred.
- **The suite's models are not the views'**: the suite replica, the
  restated RT9080, the BQ25180 `suite_compat` corner, and tolerances from
  P0 (D2).
- **Numerics at `phase0`**: P0 measures; the review already found that the
  suite's W1 deck needs Gear at `phase0` (Context).
- **The rung of every assertion would be `fast`**: D1's re-qualification
  of existing views and G4's `rung: pi` check.
- **Tautological gates**: G6's idd check and G8 replaced by independent
  ones.
- **ADR-012's checker cannot verify figure-derived values**: rule 3 now
  names ADR-012's figure claims; without them a view's confidence tops
  out at `self_checked` plus `reviewed_by` (plan R2).

**Not adopted:** a lint exemption for V-type B sources on internal signal
nodes (one critique offered it as an option). Restating the RT9080 avoids
the need, and the exemption could not be checked.

## Amendment 1 (2026-11-15): P0 report (the numerics and feasibility spike); stopped for the approver

**Status: W1's feasibility row fails** at the suite's 50 mA budget
(the approver's answer, D-W1, is Amendment 2). Under
Q1 (approver decisions) invoking the stress-corner fallback is the
approver's choice, so P0 stops here with the measurements below. Every
other P0 item is done. `make pi-spike` (`scripts/pi_spike.sh`,
`scripts/pi_spike.py`) runs 82 configurations through `NgspiceEngine`
(`deck_run`, lattice 10 µs) in about 100 s on 8 jobs. The decks are built
by the suite's own functions from a `git archive` export of `6854d25` and
stay in `$BB_SCRATCH/bb-pi-spike`: they hold the TI-derived TPS22917 or the
suite's netlist extraction, so they are local-only.

**The first `pi` views** are in `examples/dial-me-spinner/pi-spike/views`.
They are this repository's own text, from DS9080-09, SLUSE99C and SLVS413L;
no suite model is migrated:

- `RT9080_LDO_PI`: regulation transconductance, dropout resistance,
  fold-back limit, EN, discharge;
- `BQ25180_PP_PI`: ILIM, SYS_REG through RON_IN, supplement, the BUVLO
  gate; `sup_gate = 0` is the `suite_compat` corner;
- `TPS61040_BOOST_PI`: an averaged PFM cycle, soft start by cycle count,
  UVLO.

The spike's lint of rules 1 and 4 passes on all three, and its negative
control fires 5 findings. P1 turns them into packages. Not yet modelled:
charging, VINDPM, BATOCP, the RT9080 soft start (no datasheet figure).

**1. Replicas (pass).** The suite's decks through the engine at `phase0`
with the S1-fixed TPS22917 (ADR-006 Amendment 2; the local-only package of
`make dms-local-packages`) against `results.json`:

| Case | VSYS min | +3V3 min | Other values |
| --- | --- | --- | --- |
| W1 `[ILIM80min-SYSREG4.4-2pct]` | 2.910883 V (+0.077 mV) | +0.077 mV | last below 3.045 V 5.8285 ms (+4.4 µs); +3V3 after blanking, OLED_VIN exact |
| W2 rule `[ILIM100-SYSREG4.5]` | 3.266628 V (+0.040 mV) | +0.040 mV | exact |
| W2 old rule (5 ms) | 1.461121 V (−0.246 mV) | −0.246 mV | — |

The suite's unfixed view, on the suite's own fallback ladder, comes within
0.14 mV. W1 and W2 old ran at reltol 1e-4, W2 rule at 1e-3. **Finding:**
through the engine the unfixed view fails at `phase0` and at `phase0` with
Gear (`Timestep too small` on `b.xu2.x_u1_u2_u4.beout#branch` at 200.05 µs,
or no end in 240 s). That is ADR-006 Context 3: the review's standalone
Gear run does not predict the engine. The engine's failure text misses it,
because `failure_lines` matches only lower-case `timestep too small` (for
H1).

**2. Option set: `phase0`, named and qualified.** Every static-load deck
converges at plain `phase0` with trapezoidal integration, so no new option
set is needed:

- grants of 10 µs, 1 ms and 10 ms give byte-identical samples;
- against TMAX 1 µs the gated quantities differ by at most 0.62 mV and
  0.56 µs;
- the whole VSYS and +3V3 waveforms differ by at most 1.08 mV, at U2's
  turn-on knee (the committed TPS22917 view's limiter corner). The
  waveform bound is therefore 2 mV (`tolerances.toml`), a deviation from
  S1's 1 mV.

`phase0` with Gear and `suite_fallback` differ from `phase0` by at most
0.41 mV on W1 and W2 rule. On W2 old, `suite_l1` (reltol 1e-3) moves +3V3
min by +17.8 mV and `suite_fallback` by −8.9 mV, so W2's collapse needs
`phase0`.

**3. Model difference.** The static-load deck uses the spike's views, the
committed `ti/tps22917` and `worldsemi/ws2812c-2020` views, the board's
+3V3 capacitors (37.6 µF nominal, from the netlist), the 50 mA budget as a
bench load on +3V3, and LED_PWR_EN as `digital_out` glue at 3.3 V
(rail-tracking is P2). Each value is the static-load deck minus the
replica:

| Case | VSYS min | +3V3 min | Windows / crossings |
| --- | --- | --- | --- |
| W1 | +87.8 mV | +97.3 mV | 2.94 V never crossed (replica 3.295-4.447 ms); 3.045 V window ends 1.08 ms earlier |
| W2 rule | −4.5 mV | +6.9 mV | none crossed (as the replica) |
| W2 old | +8.4 mV | +17.7 mV | VBOR,ON 1.57/1.60/1.63 V crossed +207/+199/+192 µs later |

Attribution on W1, each variant against the replica:

- without the +3V3 capacitors: −7.5 mV, window −117/−152 µs. The BQ25180
  and RT9080 views reproduce the suite's ideal LDO and ilimit block, and
  the rest is the TPS22917 view. The suite's deck with only the fitted
  TPS22917 swapped in gives −6.9 mV;
- with the +3V3 capacitors halved (a DC-bias stand-in): +48.7 mV;
- the fitted TPS22917 replaced by the S1-fixed one: +96.0 mV.

So the W1 difference is the +3V3 rail's capacitance. The suite's ideal
+3V3 has none. In dropout those capacitors carry part of the 50 mA, which
in effect adds 46 % to VSYS's 82 µF.

**4. Tolerances.** The rule: 1.5 × |measured difference|, rounded up in the
1-2-5 series, and at least 1 mV / 10 µs.

- W2 rule: VSYS min 10 mV, +3V3 min 20 mV.
- W2 old: VSYS min 20 mV, +3V3 min 50 mV, VBOR crossings 500 µs.
- W1: **pending**. Its difference depends on the static-load
  configuration, which is the approver's choice. By the rule, the 50 mA
  deck would get 200 mV and 2 ms, which would not discriminate.

The W2 values hold for this deck definition and are measured again if
the answer changes the deck.

**5. W1's feasibility row fails.** At 50 mA, +3V3 min is 2.9640 V (RT9080
worst corner: dropout 0.53 V at 600 mA, load regulation 1 %), and 2.9768 V
at the typical corner. Both are above 2.94 V, so there is no window. The
same row on the variants:

- without the +3V3 capacitors: passes (3.178-4.295 ms, 1.12 ms);
- with them halved: fails (0.38 ms);
- with the S1-fixed TPS22917: fails.

Load sweep against the plan's table: 30 mA, no crossing (as the suite);
60 mA, collapse (as the suite); 50 mA, no crossing where the suite crosses
and recovers. The verdict at 50 mA differs.

**6. Data for Q1's stress corner** (static-load deck, +3V3 below 2.94 V
for at least 0.5 ms, recovered by 8 ms):

| Static load | 53 | 54 | 55 | 56 | 57 | 58 | 59 | 60 mA |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Below 2.94 V, RT9080 worst (ms) | 0.15 | 0.44 | **0.83** (3.255-4.089) | 1.41 | 2.37 | 4.29 (to 7.51) | 10.16 (to 13.36) | no recovery |
| Below 2.94 V, RT9080 typ (ms) | 0 | 0 | 0.27 | **0.70** (3.247-3.950) | 1.41 | 2.83 | 7.14 (to 10.35) | OP failed |

- The smallest whole mA is **55 mA** at the worst corner and 56 mA at the
  typical one.
- At 55 mA the replica crosses 2.94 V from 3.234 to 6.947 ms, near its
  cliff. The difference there is +102.5 mV in VSYS min and −2.86 ms at the
  window's end. A stress corner therefore needs its own tolerances.
- The OP failure at 60 mA, typical corner, is "singular matrix at
  `xu2.lsf_vinf`" after 31 s. That node of the committed TPS22917 view
  has only a 1 µS path. It is an R1 instance for P1 and H1's fail-fast.

**Needs approval (Q1).** Raising the static load is the stated fallback.
The measurements also show that the outcome turns on the +3V3
capacitance, which the suite does not model. The approver decides.
P1-P4's W1 work waits for that decision. W2's tolerances are fixed.

**Findings for P1:**

- the committed TPS22917 view uses V-type B sources (rule 1), so it must
  be restated before it can declare `pi`;
- the B5819W rectifier has no package (the spike borrows the suite's
  model, in the scratch decks only);
- the RT9080's corner for W1 is to be stated in P1 (worst: the suite's
  dropout).

## Amendment 2 (2026-11-15): W1 at a labelled stress corner (approver decision D-W1); P0 closed

**Decision (approver, round 2b, 2026-11-15, "D-W1").** On Amendment 1's
report the approver invoked Q1's fallback, with a margin check added:

- **W1 is gated at a labelled stress corner:** 55 mA static +3V3 load at
  the RT9080 worst corner. That is the smallest whole mA that gives at
  least 0.5 ms below 2.94 V: 0.83 ms, from 3.255 to 4.089 ms after
  LED_PWR_EN rises.
- Every firmware-in-the-loop check of W1 runs there: the rule firmware,
  both negative controls, the 5 ms report, and the nominal and −5 %
  corners.
- The static-load checks at the stress corner have **their own
  tolerances**. They are fixed from P0's measured model difference at
  55 mA. The replica is near its cliff at that load (below).
- **A gated margin check at the suite's 50 mA:**
  - no POFWARN at the +5 % corner;
  - +3V3 min reported with its margin to 2.94 V;
  - the same reported at the nominal and the halved +3V3 capacitance
    (the DC-bias-derated stand-in).
- **Every W1 report and `run.json` labels the stress corner.**
- The 50 mA feasibility row of the plan (section 2 W1, section 4 P0,
  section 5 item 1) is replaced by the stress-corner row and the margin
  check.

D-W1 changes only W1's load. The W2 decks are unchanged, so W2's
tolerances (Amendment 1, item 4) stand.

**What `make pi-spike` now gates** (37 gates, all pass):

- `tolerances.toml` has a new `[w1]` table: the stress load, the margin
  load, the RT9080 corner, the threshold, the 0.5 ms / 2 ms / 10 ms
  limits and the decision. Every W1 line of the output carries the label
  "W1 stress corner: 55 mA static +3V3 load (suite budget 50 mA), RT9080
  worst". The summary carries it as `w1_corner`.
- **The stress load is re-derived on every run.** The smallest whole mA
  from 50 to 60 mA, at the RT9080 worst corner, with at least 0.5 ms below
  2.94 V must equal `[w1] stress_load_mA`. A missing run below that load
  fails the gate.
- **The option set is qualified at the stress corner too.** Grants of
  10 µs, 1 ms and 10 ms give byte-identical samples. Against TMAX 1 µs
  the gated quantities differ by at most 0.022 mV and 0.14 µs, and the
  waveforms by at most 0.97 mV (bound 2 mV).
- **Each tolerance table is checked against the rule:** a table must
  equal the stated rule applied to the measured difference. This now
  holds for W2 as well as for W1's stress corner.

**1. The stress corner** (static-load deck, RT9080 worst, the board's
+3V3 capacitors at their netlist values):

| Quantity | Static-load (`pi`) | Replica at 55 mA | Difference | Tolerance (the rule) |
| --- | --- | --- | --- | --- |
| VSYS min | 2.9589 V | 2.8564 V | +102.5 mV | 200 mV |
| +3V3 min | 2.9189 V | 2.8078 V | +111.1 mV | 200 mV |
| 2.94 V window start (after LED_PWR_EN) | 3.2552 ms | 3.2341 ms | +21.1 µs | 50 µs |
| 2.94 V window end | 4.0893 ms | 6.9472 ms | −2858 µs | 5000 µs |

- **Feasibility (gated, passes).** +3V3 is below 2.94 V for 0.834 ms and
  recovers at 4.089 ms. The limit is 8 ms: 2 ms before the 10 ms blanking
  ends. +3V3 min after the blanking is 3.114 V (flash VCC min 2.7 V).
- **The replica is near its cliff at 55 mA.** It collapses at 60 mA, and
  at 55 mA its window is 3.71 ms long, 4.4 times the static-load deck's.
  The tolerances therefore bound the model difference without
  discriminating the window's end: 5 ms against a 10 ms blanking. At this
  corner W1 decides on the firmware checks (the cue held, `pofwarn`
  fired, the two negative controls), not on the window's end against the
  suite.
- **Data for P4 and W-fw (not a gate here).** The window's midpoint is
  3.672 ms. The window-midpoint negative control (the whole ms nearest it,
  plan section 2) is therefore 4 ms, only 89 µs before the window ends.
  P4 derives the control from its own measured window. If that window
  ends before 4 ms, or within one lattice cell after it, the control
  stops testing the re-enable rule. P4 then reports it and stops. It does
  not move the control by its own choice.
- The 5 ms blanking ends after the window, so the 5 ms report is
  expected to pass.
- At the RT9080 typical corner the smallest whole mA is 56 mA (0.70 ms).
  This is reported only, because W1 gates the worst corner.

**2. The margin check at 50 mA** (gated: RT9080 worst, nominal
capacitance):

| +3V3 capacitance (corner) | +3V3 min | Margin to 2.94 V | Below 2.94 V |
| --- | --- | --- | --- |
| **nominal, 37.6 µF (worst): gated** | 2.9640 V | **+24.0 mV** | never: no POFWARN (pass) |
| halved, DC-bias stand-in (worst) | 2.9188 V | −21.2 mV | 0.381 ms (3.253-3.634 ms, inside the blanking) |
| nominal (RT9080 typ) | 2.9768 V | +36.8 mV | never |
| none, as the suite's ideal LDO (worst) | 2.8592 V | −80.8 mV | 1.116 ms (inside the blanking) |
| nominal, S1-fixed TPS22917 (worst) | 2.9661 V | +26.1 mV | never |

The margin is 24 mV at the nominal capacitance. Halving the capacitance,
which stands in for DC-bias derating, removes it: +3V3 then crosses
2.94 V for 0.38 ms. That dip is still inside the 10 ms blanking, so the
rule firmware would hold the cue. So whether the real board raises
POFWARN at the suite's budget depends on capacitance that the suite does
not model. That is a finding for the user, alongside Q12's.

**3. The load sweep** (report; P1 gates it), as verdicts at 2.94 V:

| Load | Static-load (`pi`) | Replica |
| --- | --- | --- |
| 30 mA | no crossing | no crossing |
| 40 mA | no crossing | no crossing |
| 50 mA (margin) | no crossing | cross and recover |
| 55 mA (stress) | cross and recover | cross and recover |
| 60 mA | collapse | collapse |

The plan's sweep row demanded the suite's verdict at 50 mA. D-W1 makes
50 mA a no-POFWARN margin check, so the row now gates 30 mA (no
crossing), 50 mA (the margin check's verdict), 55 mA (cross and recover)
and 60 mA (collapse). The 50 mA row is the one case where the verdict
differs from the suite's, and it is recorded.

**Labels from P1 on.** W1's stress-corner bench names the corner in its
file name (`bench/w1-stress.toml`). It holds the 55 mA `[[load]]`, which
`run.json` records with the bench. Every W1 report also names the corner:
the `bb test` report and JUnit suite names, and P1's summaries. A
dedicated `run.json` key would be a schema change and is not made here.

**P0 is closed.** Its remaining finding goes to P1: the committed
`ti/tps22917` view is restated to D2's rules (approver decision D-TPS).
The singular matrix at `xu2.lsf_vinf` at 60 mA, typical corner, is the
same finding (Amendment 1, item 6). `make pi-spike` runs 85
configurations in 84 s on 8 jobs (74 s of runs, host load 0.3-3.9).

## Amendment 3 (2026-11-15): the schema keys landed (milestone S)

Milestone S, the schema epoch, landed this ADR's keys. ADR-011 Amendment
1 is the record; `spec/package.md`, `spec/pin-model.md` and
`spec/event-stream.md` state them. All of them are accepted, checked by
lint and inert. No `fast` deck, output or `run.json` depends on them, and
`--fidelity pi` stays refused until P1.

- **`bb-part/1`:** `[power]` (`idd.<state> = { typ, max, ref }`, `vdd_min`,
  `io_rail`; D4, D5), a view's `rails`, the `supply_load` view kind (D4),
  and `[[analog.corners]]` (D8; names `typ`, `min`, `max`, `suite_compat`,
  `worst_<x>`, each with a `ref`).
  - A `supply_load` view has `{ role = "pin" }` supply or ground ports and
    `{ role = "state", state }` ports. Each state port needs a `[power]
    idd` entry.
  - Lint keeps a `supply_load` view off `fast` and off every rung's
    `default_for`. D1 adds it to the deck beside the chosen view, so the
    ladder must never choose it.
- **`bb-pins/1`:** `limits` (D3). A bound is `<value>`, `<pin> + <value>`
  or `<pin> - <value>`. The `digital_out` parameters `rail` and `rhiz`
  (D5) are optional, and unused at `fast`.
  - The pin model name `power` is **reserved**, not defined. D3 names
    `power{vmin, vmax, imax}` but not where its values come from, so P1
    defines it within `bb-pins/1`.
- **Event stream version 2:** the `reset` record (D7: `t_fs`, `source`,
  `cause`, `phase`, and `ram` on `exit`) and `assert`'s `rung` (D1).
  `bbcore::event` has both types. Nothing emits them before P5 and B2.
- **The ADR-006 D3 sentence** about the TPS22917 `pi` view is not
  corrected here. That sentence is corrected when P1 restates the view
  (decision D-TPS).

## Amendment 4 (2026-11-15): P1 as implemented (the `pi` views, the dial, corners, the battery, static-load W1/W2)

Milestone P1 (plan section 4). `make pi-static` is its gate (28 gates,
all pass); `make pi-static-test` (in `make test`) runs the same seven
cases without the partition runs.

**The packages** (all `pi`-only unless stated; each view qualified at (10
µs, `phase0`) and lint clean, `--publish` names):

| Package | View | Source | Corners |
| --- | --- | --- | --- |
| `richtek/rt9080` (U1) | `ldo`: P0's view; IQ split half ground current, half divider leak, so the no-load input current is IQ | DS9080-09 | `typ`, `worst_dropout` (0.53 V at 600 mA, 1 % load regulation: D-W1's "RT9080 worst") |
| `ti/bq25180` (base U1) | `pp`: P0's view | SLUSE99C | `typ`, `suite_compat` (`sup_gate = 0`) |
| `ti/tps61040` (U20) | `boost`: P0's averaged PFM view, its output made the view's own state | SLVS413L | — |
| `cj/b5819w` (D21, `fast` and `pi`) | `diode`: a SPICE diode fitted to the CJ datasheet's typical curves | CJ rev D 2015 | — |

- Each has vectors from its datasheet's tables (5, 6, 3 and 2). The
  packages list their MPNs, so `bb open` now resolves main U1, U20, D21
  and base U1. At `fast` the ladder finds no view for the `pi`-only ones,
  so no `fast` deck changes (their `run.json` lists them out of scope).
  `make locks` relocked every example. Two tests assumed those parts
  unresolved: `cli/tests/system.rs` now expects main U1 resolved, and
  `cli/tests/kicad_import.rs` runs its synthetic two-board fixture (whose
  RT9080-33GJ5 and BQ25180YBGR have made-up pins) without first-party
  packages.
- **The TPS61040's output.** An averaged boost cannot deliver its energy
  through the board's rectifier (at DC the inductor holds SW at VIN), and a
  view's ports are the package's pins, none of which is the output. P0
  used a modelling port on D21's cathode. The package keeps the output as
  the view's own state: `cout`, `rload` and `vout` are instance parameters
  that the deck takes from the board (14.8 µF, 551.3 kΩ, 12.9972 V); the
  board's +13V_OLED net is not driven by it. Recorded as a deviation.
- **`pi` declarations** (D1): `generic/{resistor,capacitor,inductor}` and
  `worldsemi/ws2812c-2020` are now valid and the default at `fast` and
  `pi`. `pi` runs at `phase0` (Amendment 1), so their qualification
  records stand; `bb pkg test` re-ran their vectors.

**The TPS22917 restated (decision D-TPS).** The `lsw` view now has no
V-type B source: every internal signal is an I-type source into 1 Ω.
SLVSDW8B was opened for it, and the view is valid and the default at
`fast` and `pi` (so ADR-006 D3's sentence is true again). Three findings:

- **Source stepping.** With the old 1 µS VINF node fixed (1 S into 1 mF),
  the W1 decks still failed their operating point at 50 and 55 mA (gmin
  stepping, then "singular matrix: check node xmain_u2.lsf_vinf" and
  `lsf_en`, `base_vbat` in source stepping). The singular nodes are the
  ones whose only DC path is a B source. A plain resistor on each fixed
  it: 1 GΩ keepers from `lsf_en` to `lsf_ont` and from `lsf_vinf` to VIN
  (beside the sources that drive them, so they change no DC value), and
  the bench cell as a current OCV/R0 with a real R0. D2 rule 2 is
  therefore read as "a plain resistor on every node", not only "a
  conductance". Every division by an internal node is also guarded by a
  clamp that is inactive at any solution.
- **The rise time below 3.6 V.** Table 7.6's tR row (0.6 / 0.95 / 1.2
  µs/pF at 1 / 1.2 / 1.8 V) moved E1's cold VLED ramp (VSYS 3.0 V) by
  about 40 µs, 64 mV at one sample, beyond its recorded 1 mV / 1 sample
  snapshot. The view keeps Phase 1's 0.8 VIN / SRON there (0.727 / 0.96 /
  1.309 µs/pF; a linear ramp cannot meet tR and SRON together). RON now
  follows Table 7.5's five typ columns.
- **E1 within its recorded tolerances.** VLED is within 0.096 mV (cold)
  and 0.001 mV (nominal) of the committed snapshots at every 100 µs. The
  four checks give the same verdicts and the same printed values (VLED
  2.971510 / 3.684020 V at the P0.06 rises, LSW-2 24.420 ms; the wrong
  order fails exactly LSW-3). The bytes changed (different equations), so
  `tests/golden/dms-rail/*/outputs.sha256` are re-recorded from the
  one-cell references, and `vled.csv` is kept as Phase 1 recorded it. The
  docs site's `e1-rail` plots are regenerated. E2's s1, s50 and c5 cases
  and their `load = "none"` twins keep their hashes (U2 does not switch in
  them). `sag` and `rec`, where U2 sheds and restores the LEDs, changed
  bytes and nothing else `make dms-vsense` checks: the same codes (1296,
  1293, 1289, 1286, 1282; 1296, 1269, 1314), the shed at 4000.480 ms
  (154.4 ms after the crossing) and the restore at 2000.480 ms as ADR-008
  recorded, the defaults equal to their references, determinism. Their
  `tests/golden/dms-vsense/{sag,rec}/outputs.sha256` are re-recorded; the
  E2 plots are unchanged.
  `make dms-m5b` (local) passes 20/20 against the vendor-derived view
  with Phase 1's margins (LSW-2 0.555 / 0.356 ms, the rest 0.001 mV). That
  local-only package (`make dms-local-packages`) now declares `fast` only:
  its V-type B sources, CCCS and 0 V source are what rule 1 refuses at
  `pi`, and the new lint said so.

**Lint (D2).** `bb pkg lint` applies rules 1 and 5 to every view valid at
`pi` (`bbpkg::pirules`; `spec/package.md` section 11): V, E, F, H, V-type
B, W, T/O/U and A elements are errors, L and K warnings (the generic
inductor warns). Rule 4 is ADR-015 D3's lint. Rule 2: `bb pkg test`
solves every vector's operating point at every corner, under every option
set the view runs under (22 corner operating points on the four new
packages).

**The dial, corners and the battery** (`spec/project.md` section 3.2).
`bb run --fidelity fast|pi` and the bench's `fidelity` (the command line
wins); `si`/`em` are refused, and so is `pi` on a board file. `corner`
applies where a view declares it; `[corners]` sets one per instance (the
syntax D8 left open), must exist in the view, and `run.json` records
`corner` on the instance. `[[battery]]` is ADR-010 D2's shape (no
capacity: the state of charge holds for the run), written as `I` and `R`
elements. `cli/tests/fidelity.rs` elaborates the main board at `pi` and
`fast` with them.

**W1 and W2 in static-load mode** (`scripts/pi_static.py`,
`examples/dial-me-spinner/pi/static_load.toml`). The decks are assembled
from the package views (the ladder at `pi`, the corners) and the system
import's `board.bbm`, as `bb run` would, plus the bench of static-load
mode (5 V VBUS, the dead cell as the suite models it, the suite's lumped
harness, the +3V3 budget as a load in place of the CPU's `supply_load`).
They run through `deck_run` at `phase0`. The replica's values are P0's
recorded measurements, so the gate needs no dial-me-spinner checkout.

| Row (W1 rows labelled "W1 stress corner: 55 mA static +3V3 load (suite budget 50 mA), RT9080 worst_dropout") | Result |
| --- | --- |
| Stress corner against the replica at 55 mA (200 mV, 200 mV, 50 µs, 5 ms) | VSYS min +102.4 mV, +3V3 min +111.1 mV, window start +21.9 µs, end −2860 µs: pass |
| Feasibility at 2.94 V | below for 0.831 ms (3.256-4.087 ms), recovered by 8 ms; +3V3 3.114 V after the blanking: pass |
| Margin check, 50 mA, nominal +3V3 capacitance | +3V3 min 2.9642 V, margin +24.2 mV, no POFWARN: pass; halved capacitance (report): 2.9190 V, −21.0 mV, 3.254-3.633 ms |
| Load sweep 30 / 50 / 55 / 60 mA | no crossing / no crossing / cross and recover / collapse: pass (50 mA differs from the suite's verdict, as recorded in Amendment 2) |
| W2 rule (10, 20 mV) | VSYS min −4.3 mV, +3V3 min +7.2 mV: pass |
| W2 old rule (20, 50 mV, 500 µs) | +12.3 mV, +21.7 mV; VBOR crossings +92.5 / +85.9 / +79.4 µs: pass |
| Every W1 check's rung (D1) | `pi` (29 instances on +3V3 and VSYS, every one at `pi`) |
| Partitions (`make pi-static`) | 10 µs, 1 ms and 10 ms grants and a second 1 ms run byte-identical (a consistency check: deck_run's partitions) |

The package decks reproduce P0's static-load decks closely (stress corner
VSYS min 2.9588 against 2.9589 V, window 0.831 against 0.834 ms). W2
old takes 24 s (75 284 solver points, against P0's 51 602): the
TPS61040's own output state regulates through a band.

**Deviations, recorded.**

- The `power` pin model stays reserved: its only consumer is D3's limit
  assertion, which needs B2's assertion engine. `[power]` and
  `supply_load` stay as S left them (their consumers are P2 and P3).
  P1 uses `rails` (every new view declares them; lint checks them).
- `bench/w1-stress.toml` is not written: P1 has no project route (R2).
  P1's labels are the case spec's and the summary's (Amendment 2 allows
  "P1's summaries").
- ILIM and SYS_REG are base U1's instance parameters (default 4).
- `scripts/kicad_import.sh`'s M4f counts were stale since Phase 1 M10
  (`ti/drv8837`); with P1's packages they are 56 by MPN and 14
  unresolved (`make kicad-import` passes with `KICAD_WINDOWS=0`).

## Amendment 5 (2026-11-16): P2 as implemented (rail-tracking, tristate `digital_out`; the `oe` prototype; G8, G9)

Milestone P2 (plan section 4). `make pi-glue` is its gate (all pass);
`make pi-glue-test` (in `make test`) is its fast part, and `make
pi-oe-cost` (a measurement, under the host lock) is the prototype's cost
row. `spec/pin-model.md` (the `pi` form of `digital_out`, glue rule 11) and
`spec/project.md` (`--pi-oe`, `--operating-point`, `run.json` `pi_glue`)
state what follows; both are D5's normative changes.

**The glue** is D5's, with names: `V_bb_<id>` (the level `s`, 0..1),
`V_bb_<id>_oe`, `B_bb_<id>_hi`, `B_bb_<id>_lo` and `R_bb_<id>_hz`, `<id>`
being `<part>_<pin>` (`cli/src/main.rs` `pi_glue_deck`). `s` and `oe` ramp
0 V / 1 V over the output's own `rise` and `fall` (D5 does not say how
`oe` ramps; a step would put a discontinuity in two B sources). A pin with
`zout = 0` is refused at `pi`. At `fast` nothing changes: no glue, no
`#oe` output, no `run.json` key (G1).

- **`rail`** is the binding's `rail`, else the set's, else the package's
  `[power] io_rail`; a pin with none is refused at `pi`. The rail's net
  must be in the analog scope. `nordic/nrf52840-qiaa` (0.2.0) and
  `st/stm32f072rb` (0.2.0) now declare `[power] io_rail = "VDD"`; `make
  locks` relocked every example that pins them, and `examples/adc/kicad`'s
  `board.bbm` records the new version. `st/stm32f103c8` does not: only
  board files use it, and a board file has no rail (below), and the
  registry gate publishes it at 0.1.0.
- **`<vss>`** is the net of the package's ground pins (a module: through
  its pinmap), else the board's ground; ground pins on two nets are
  refused. **`rhiz`** defaults to 100 MΩ (default 6); a set or a binding
  may set it.
- **`run.json`** records, only at `pi`, `pi_glue`: per output its kernel
  net, `rail`, `rail_net`, `rhiz`, `vss_net` and `oe`.
- `voh` and `vol` are unused at `pi`; a binding that sets them gets a
  warning (the package's `voh = "VDD"` is not warned about).
- **Devices** (ADR-013) keep the `fast` form at every rung: D5 is about
  MCU pins, and a device package has no rail.

**The `oe` prototype (default 3).** On the nRF52840, `oe` is the CPU
engine's output `<pin>#oe` (`PinDirection::OutputEnable`,
`PinSpec::nrf_oe`): the engine reads the existing `PIN_CNF` write hook
(ADR-008 D4) after every advance, tracks `DIR` per pin from `DIR`,
`DIRSET`, `DIRCLR` and `PIN_CNF[n]` writes, and reports each change at the
write's virtual time by the GPIO lattice rule (rule 1, 2; a write stamped
at the grant end is carried to the next grant, rule 3). It is low after
reset (`PIN_CNF = 0x2`). The kernel binds it to the glue's `oe` source
like a loading `adc_in`'s `#smp`; its edges are `pin_edge` records of the
kernel net `<part>.<pin>#oe`. A carried `#oe` change ends a lookahead
promise, and the audit treats it like a carried GPIO event.

- **Exact** (`engines/cpu-renode/tests/nrf52840.rs`; both in `make
  pi-glue` and in `cargo-test`):
  - `nrf_oe_same_clock_as_gpio` (new gate firmware `nrf_oe.c`: `DIRSET`
    and `OUTSET` in adjacent stores, then `OUTCLR` and a `PIN_CNF` input
    write): every `#oe` edge has the time of its ISR's GPIO edge (5 of 5
    in the gate's 60 ms; 9 of 9 over 100 ms while prototyping), so a
    direction write and a GPIO write of one slice get one time, and
    program order is never inverted. In Renode 1.17.0 the hook's
    `machine.LocalTimeSource` is the `MasterTimeSource` itself on a
    single-machine emulation (checked in the monitor), the clock GPIO
    events are stamped with.
  - `nrf_oe_trace_exact` (`nrf_pincnf.c`): the boot writes at 1 fs, the
    input pins with no edge, P0.17's ISR writes a few instructions after
    the ISR's P0.13 toggle, in program order and within one cell (here the
    next slice: 10.010125 ms against 10.010000000001 ms, and 20.020000000001
    against 20.019625 ms); the GPIO edges unchanged by the `#oe` pins; the
    audit's mid-run record at the `#oe` rise when both run; identical at
    one-cell grants, 10 ms grants and lookahead on.
- **Cost:** `make pi-oe-cost` (E1 cold at `pi`, 120 ms, fresh Renode;
  A = `--pi-oe fixed`, B = `PIN_CNF.DIR`; interleaved, medians of the
  simulation time; the gate is B / A ≤ 1.05). The first run (N = 5, host
  load 13-55 from other tracks' gates) gave **1.261**, with one B run of
  31 s. The engine's `EXTERNAL` callbacks, which ngspice makes for every
  source at every Newton load, then allocated two strings per call, and B
  has two more such sources (the `oe` inputs). They now look the name up
  on the stack (`bb_analog_ngspice::source_key`; the same keys, so no
  result changes: every committed hash holds, `make check` pooled and
  `POOL=0`). After it: N = 5 at load 13-16, 0.875; N = 9 at load 7-12,
  **1.000** (3.15 s against 3.15 s; CPU 7.58 against 7.93 s). The first
  figure was not measured again on the old callbacks, so how much of it
  was the allocations and how much the load is not known. `make
  pi-oe-cost` now takes N = 9 (`PI_OE_COST_N`). The host was never idle.
- **Decision (default 3 applied):** `PIN_CNF.DIR` is the `oe` source on
  the nRF52840 (exact, and within the 5 %). `bb run --pi-oe fixed` keeps
  the fallback (`oe = 1`) for comparison: on E1 cold it moves no sample by
  more than 0.009 mV (VLED at 2.23 ms; LED_PWR_EN and LED_DIN_MCU are 0 V
  through R3 and the LED chain either way before the firmware drives
  them). "Always 0 while held in reset" waits for P5's reset.
- **STM32 (deviation):** no direction hook exists for the STM32 GPIO
  (`MODER`), so every STM32 output uses the fallback, `oe = 1`, recorded
  as `oe = "fixed"`. No Phase 2 check reads an STM32 output at `pi`.

**G9: the operating point with every `oe = 0`** (`bb run
--operating-point`: the run's deck and glue, ADR-015 D4's `.op` with the
transient-op fallback off, every input at its initial value and every `oe`
at 0; no CPU starts). Twenty decks, each with its expected number of `pi`
outputs, all solve:

| Deck | `pi` outputs |
| --- | --- |
| `spike`, `nrf-saadc` | 1 |
| `feedback`, `adc`, `nrf-feedback`, `adc-kicad`, dial-me-spinner at `nominal`, `cold`, `vsense`, `sag`, `sag-recover` | 2 |
| `nrf-pwm` | 4 |
| E3 `board.cir`, `board-step.cir`, `board-ts75.cir`, `board-park.cir`, the `e3/step` project | 5 |
| `feedback-open`, `adc-open`, the `e3/home` project (its MOT nets are digital-only) | 0: no MCU output on a circuit net |

- A board file names no rail, and P1 refuses `pi` on one. `--operating-
  point` alone accepts `--fidelity pi` on a board file and gives its MCU
  outputs a stand-in rail at their `voh`; with `oe = 0` a rail carries no
  current, so this is the board's `pi` operating point.
- The projects run at their benches made `pi` in scratch copies
  (`fidelity = "pi"`, and `main:U1` excluded: at `pi` the RT9080 has a view
  and would drive +3V3 beside the bench's supply, which stands in for it).
  `adc-kicad` has no bench, and at `pi` its outputs' rail +3V3 must be
  driven: its G9 bench is a 3.3 V supply on +3V3.
- **Negative control** (`cli` unit test `pi_glue_text_and_high_z_
  operating_point`): examples/feedback's RC with `oe = 0` solves with
  `rhiz` and fails without it (no DC path to SENSE); it solves with
  `oe = 1`. The test also fixes the glue's text.

**E1 cold at `pi`** (`bench/cold-pi.toml`: `bench/cold.toml` with
`fidelity = "pi"` and `main:U1` excluded, the same reason). Every part
but U1 has the same view at `fast` and `pi` (P1), so the deck differs
only in the glue. P1.04 and P0.06 are high-Z until the boot code makes
them outputs (`#oe` at 0.12 and 0.13 ms), then drive from +3V3 (2.927 V at
this corner): LED_PWR_EN 5.8 mV below it (R3, 100 kΩ, through 200 Ω),
LED_DIN_MCU 0.8 mV below it.

| | `fast` (`voh` 3.3 V) | `pi` (rail-tracking) |
| --- | --- | --- |
| LSW-3, correct order | 0.320308 V at 30.2 ms | **0.000000 V** (at t = 0; while P0.06 is high at most −51.4 mV); the suite: −5.5e-10 V |
| LSW-3, wrong order | FAIL (ADR-008 Amendment 3: 0.793 V) | **FAIL, 0.777445 V** at 45.26 ms (DIN clamp 3.14 mA): exactly LSW-3, as at `fast` |
| LED-DATA-V4, LSW-2, FW-6.4 (correct) | pass: 2.971510 V, 24.420 ms | pass: 2.971510 V, 24.420 ms |

So the documented pessimism is removed and the verdicts stand.

**G8** (`bench/cold-pi-ioh.toml`: the same bench, plus 0.5 mA drawn from
LED_PWR_EN from 5 to 30 ms, `data/led_pwr_en_ioh.csv`). 0.5 mA is the
condition of the PS v1.1's VOH,SD row (section 6.9.3.1: VOH ≥ VDD − 0.4 V
at 0.5 mA, VDD ≥ 1.7 V); the pin also feeds R3 and U2's ON input, so it
sources 0.5282 mA. The assertion `PS-VOH-SD` (range of V(LED_PWR_EN) −
V(+3V3) from 6 to 29 ms, min −0.4 V) passes: VOH 2.821693 V at VDD
2.927342 V, −0.105649 V, a margin of 0.294 V. The rail at the same
instant is the reference, not 3.3 V and not the unloaded rail.

**Partition independence** (`make pi-glue`), for the correct order, the
wrong order and the G8 bench: the default run (lookahead on, 10 ms cap:
71, 79 and 29 grants) and 1 ms grants equal the one-cell reference
(`--max-quantum 10us --lookahead off`: 12 000, 12 000 and 3 200 grants)
in `nets.csv`, `nets.vcd` and every non-grant record; the references
equal `tests/golden/pi-glue/<case>/outputs.sha256` (new), grant records
included; `--engine-order analog-first` and a second run are
byte-identical. No promise was broken.

**Deviations, recorded.**

- STM32 outputs use the fallback `oe = 1` (no direction hook).
- Device outputs keep the `fast` glue at `pi`.
- A board file's `pi` glue exists only for `--operating-point`, with a
  stand-in rail.
- E1's `pi` benches exclude `main:U1`; the bench's +3V3 supply is the
  suite's ideal LDO, as at `fast`.
- `oe` ramps over the output's `rise`/`fall`.
- `scripts/bench_gate.sh` (B1's G1) now names the five Phase 1 benches it
  migrates between `bb-bench/0` and `/1`; the new `cold-pi*.toml` use
  `/1`-only keys (`fidelity`, `exclude`, a profile load, `[[assert]]`).
- **Budget:** `pi-glue-test` 5.9 s pooled and 11.9 s `POOL=0`
  (`make check` runs of 1 009 s and 1 076 s beside other tracks, load
  7-15; earlier runs 8.0 s and 9.7 s), within the +15 s. The two
  `nrf_oe_*` tests in `cargo-test` take 17-21 s of their own, in parallel
  with the `nrf52840` binary's other tests (the binary took 44-53 s in
  these runs). The `cli` unit test takes under 0.1 s. `make pi-glue`: 2.0
  min (load 8-15).

## Amendment 6 (2026-11-16): P3 as implemented (supply_load, `[power] idd`, CPU state outputs, `[[activity]]`; G6)

Milestone P3 (plan section 4). `make pi-idd` is its gate (all pass);
`make pi-idd-test` (in `make test`) is its fast part, and the Renode-only
test `nrf_state_outputs_exact` (`engines/cpu-renode/tests/nrf52840.rs`)
runs in `cargo-test` and in `make pi-idd`. `spec/pin-model.md` (glue rule
12), `spec/package.md` (the `supply_load` instance parameters) and
`spec/project.md` (`[[activity]]`, `[[load]] replace`, `run.json`
`supply_load`) state what follows; `spec/schema/bench.schema.json` gives
`[[activity]]` its shape.

**The package** (`nordic/nrf52840-qiaa` 0.3.0). `[power] idd`, every `typ`
from the PS v1.1 section 5.2 scenario currents at 3 V, 25 °C (the PS gives
typical values only, so no `max`), regulator DC/DC (the plan's stated
assumption, risk R2):

| State | `typ` | Source |
| --- | --- | --- |
| `run` | 3.1 mA | ICPU4: CoreMark from flash, HFINT, DC/DC |
| `hfxo` | 0.2 mA | ICPU0 − ICPU4 (3.3 − 3.1 mA), the same CPU on HFXO; section 5.2.1.9's ITIMER2 − ITIMER0 gives 228 µA for the same difference |
| `saadc` | 1.24 mA | ISAADC,RUN: 16 ksps, TACQ 20 µs |
| `radio_tx` | 6.4 mA | IRADIO_TX1: 0 dBm, 1 Mbps BLE |
| `radio_rx` | 6.26 mA | IRADIO_RX0: 1 Mbps BLE |

`vdd_min` is 1.7 V (section 5.3.8.1). The `supply_load` view `idd`
(`analog/idd.lib`, `pi` only, `datasheet_checked`) is one I-type B source
from VDD to VSS: `(ibase + Σ V(state)·i_state) · min(1, V/vdd_min)`, with
`ibase` 2.35 µA (ION_RAMON_EVENT, always drawn) and the optional constant-
power scaling `pscale` (off; D4: unverified). Below `vdd_min` the current
falls linearly to 0 at 0 V (D2 rule 2). Three vectors from hand
calculations (`idd_states`: each state adds its current through 20 Ω;
`idd_collapse` at 1.0 V; `idd_pscale` at 2.0 V), all within 1 µV, qualified
at (10 µs, `phase0`). `bb pkg test` now runs `supply_load` vectors (S had
specified their port keys), and `--record` no longer duplicates a
`[[analog.qualified]]` entry whose header carries its own comment (a
pre-existing bug the second view of this package exposed).

- **Instance parameters** (`bbpkg::package::supply_load_params`, used by
  `bb pkg test` and by elaboration): `i_<state>` = the state's `typ` and
  `vdd_min`, then the view's `params`, which may restate or override them
  by name (S's fixture restates `i_run` and has a `worst_hot` corner).
- **`#nvmc`** has no `idd` entry: the PS gives no flash write or erase
  current. The CPU engine reports `#nvmc` (below) and the view has no port
  for it, so it draws nothing (a deviation; a measured table can add it).

**The deck at `pi`** (glue rule 12). Every in-scope instance whose package
has a `supply_load` view valid at the rung gets it beside its other views,
`X_bb_idd_<ref>`, its pin ports on the nets of the package's `VDD`/`VSS`
pins (a module target through its pinmap), its state nodes
`bb_st_<ref>_<state>` driven by `V_bb_st_<ref>_<state>`: the CPU engine's
state output for the nRF52840 target's `run`, `hfxo` and `saadc` (0 V /
1 V, 1 ns edges; `#run` starts at 1), a bench `[[activity]]` table, or
`DC 0`. `run.json` gets `supply_load` (only at `pi`): per instance its
package, view, provenance, qualification, `idd` and each state's driver
(`"cpu"`, `"bench [[activity]] (bench-scheduled)"`, `"0"`). At `fast`
nothing changes: no view, no state output, no `run.json` key.

- **`[[activity]]`** (ADR-010 D2's shape plus an optional `at`, the first
  burst's start, default 0): a table of the analog engine, 1 for `for`
  every `every` (or once) up to the run's end, so no bench engine and no
  grant boundary from the kernel; overlapping bursts and a state the CPU
  engine reports are refused; at `fast` it is ignored with a warning.
- **`[[load]] replace`** (static-load mode): removes the named instance's
  `supply_load`; the load stands in for it. At `pi` naming an instance
  without one is refused; at `fast` the load is added with a warning.
  `run.json` records `replaced_by`.

**The CPU state outputs** (`bb_nrf52_state.cs`, `Miscellaneous.
BbNrf52State`). The engine maps the block (`bbState`, 0x5FFE0000) after the
platform and the firmware only on a run that binds a state output and arms
it with their mask, so `nrf52840-bb@3`'s `.repl` is unchanged; the source
joins the platform's plugin list (its content hash changed: `make locks`
relocked every nRF52840 lock, and `examples/nrf-idd` joined `PKG_BOARDS`).
Its ring (2 048 entries of `t` in fs, the state and the level) and its
notification line follow the PWM host port's (ADR-008 Amendment 9).
`BbNrf52Saadc.ActiveChange` and `BbNrf52Nvmc.BusyChange` are null unless
`#saadc`/`#nvmc` is bound, and only then do the SAADC's START and STOP
sync the CPU's time; so no run without state outputs changes.

- **`#run` and Renode's WFI hook (a finding).** Renode 1.17.0 calls the
  WFI-entry hook only when the CPU thread next runs, at the start of the
  next slice, with every clock already there: the hook of a WFI executed
  at 1.007266 ms read 1.010 ms on the clock source and the time source,
  with or without `cpu.SyncTime()`, and a WFI in a RUN_FOR's last slice is
  hooked in the next RUN_FOR (the kernel rejected the first runs with a
  causality violation). While the CPU runs, its virtual time is its
  executed instructions at `PerformanceInMips`, and a WFI ends a
  translation block, so the block keeps a reference (the clock time of the
  last wake, its executed instruction count) and stamps a WFI entry at the
  reference plus the instructions since it (15.625 ns each at 64 MIPS,
  the count including the WFI): the instant after the WFI instruction,
  exact to the fs. It records the entry at the first of the entry hook,
  the wake's hook and the engine's SYNC read after every advance (a running
  CPU's instructions always fill the time since the reference, so a
  shortfall is a sleep). The wake is the clock time of the wake hook,
  exact. With `#run` bound the engine reads SYNC after every advance (one
  External Control read).
- **`#hfxo`**: `TASKS_HFCLKSTART` raises it and `TASKS_HFCLKSTOP` lowers
  it, through a write hook on the stock `NRF_CLOCK`, at the write's time as
  Renode's peripherals see it (after `cpu.SyncTime()`, as the SAADC and
  NVMC models time a write: the executing block's start on Renode's 125 ns
  clock grid). The crystal's start-up is not modelled (D4: the level
  follows the task).
- **`#saadc`**: the SAADC model active, a buffer started or a scan in
  progress, which covers the PS's ISAADC,RUN scenario (continuous
  sampling) as well as a single burst. **`#nvmc`**: the NVMC model busy (an
  erase, READY low); word writes still have no timing (ADR-008 Amendment
  9).
- **Lookahead.** The promise is unchanged: `#run` rises only at a wake,
  which the audit already bounds, `#hfxo` changes only at a CPU write, and
  `#saadc`/`#nvmc` at their models' own clock limits. A state change
  stamped past a grant end is carried like a PWM change, ends a promise,
  and the audit treats it like one.

**Gates as run** (`make pi-idd`, `examples/nrf-idd`: U1 supplied from the
bench's VSUP, 3.0 V behind 10 mΩ, through R1 10 Ω, with C1 4.7 µF on VDD,
and examples/nrf-feedback's loop; a 1 kΩ resistor stands in for its red
LED so every part has a first-party package):

| Gate | Result |
| --- | --- |
| Renode-only, `#run` at the WFI instants, exact to fs (`nrf_state.elf`: with PRIMASK set a pending RTC compare or SAADC END wakes WFI and no handler runs, so the code between two WFIs is one fixed inline asm block) | 119 cycles in 120 ms: every rise at an END wake equals the SAADC's END, `a + TACQ + tCONV` = the `#smp` edge + 22 µs, exactly; every fall is exactly 9 instructions after an RTC wake and 7 after an END wake (140.625 and 109.375 ns), as the asm text counts them; the first WFI at 1.109375 µs (71 instructions from reset). `#nvmc` high for exactly 85 ms (tERASEPAGE); `#hfxo` and `#saadc` in program order; one-cell grants, 10 ms grants, plugin armed and lookahead on identical; the GPIO and `#smp` edges identical with and without the state outputs bound |
| The total VDD current against a hand calculation (`nrf_duty.elf`: RTC0 every 32 ticks, 976.5625 µs; awake for a 2 × 7812-instruction delay loop, 244.125 µs, duty 0.249984; HFXO on from boot) | measured (V(VSUP) − V(VDD)) / R1 over 47 RTC periods: 0.977976 mA; hand 2.35 µA + 0.249984 × 3.1 mA + 0.2 mA = 0.977300 mA: **+0.069 %** (limit 1 %); the `#run` edges show a duty of 0.250192 (the ISR and loop overhead) |
| The same with a bench-scheduled radio (`[[activity]]` `#radio_tx` 300 µs every 1 ms from 0.5 ms) | measured 2.896962 mA; hand + 0.300664 (the schedule's share of the window) × 6.4 mA = 2.901552 mA: **−0.158 %** |
| Negative control | the duty firmware at `fast` (no `supply_load`): 0 mA, the check fails as required |
| Static-load mode (`bench/pi-static.toml`: `[[load]] amps = "0.9773mA" replace = "main:U1"`, the hand-calculated average) | `run.json` lists U1's `idd` `replaced_by` the load, no state output is bound, and the current through R1 is the load to 0.0000 % |
| Lookahead, the nRF feedback board with state outputs (`nrf_feedback.elf`, 500 ms) | **120 grants** at `pi` with `#run`, `#hfxo`, `#saadc` (39 `#run` edges), against 120 for examples/nrf-feedback's board and 120 for the project at `fast` (limit 2×); no promise broken |
| G6's 1 ms waking firmware (`nrf_duty.elf`, a wake every 976.6 µs, 50 ms) | 1 774 grants at `pi` with the state outputs, exactly the count without them (`fast`): per wake 34 one-cell grants (the CPU is awake in a feedback loop, then the query back-off) and one promise grant to the next compare. The prediction is the count without state outputs; it holds exactly |
| Equivalence against one-cell (`duty`, `radio`, `feedback`) | the default runs and 1 ms grants equal the one-cell references (`--max-quantum 10us --lookahead off`: 5 000, 5 000, 50 000 grants) in `nets.csv`, `nets.vcd` and every non-grant record; the references equal `tests/golden/pi-idd/<case>/outputs.sha256` (new), grant records included; `--engine-order analog-first` and a second run byte-identical |
| `make pool-equiv` | the three `pi-idd` cases and the duty reference added: PASS, 34 configurations, 68 pooled runs by one Renode process, every one leased and byte-identical to its fresh run and to the committed hashes (23.4 min) |

**Cost** (`make pi-idd-cost`, under the measurement lock, host load
1.0-1.2: `nrf_duty.elf` 50 ms at `pi`, A = static-load mode, the same deck
with the bench load in the view's place and no state output, B = the
`supply_load` with `#run`, `#hfxo`, `#saadc`; N = 5 interleaved, medians):
1 774 grants either way; simulation 0.74 s against 0.99 s, **B / A =
1.34** (CPU 2.85 s against 3.09 s). A wake every 976.6 µs gives about 100
`#run` edges per 50 ms, each a 1 ns ramp the solver lands on, and one SYNC
read per grant; D4's estimate was +9 % solver points at a 1 kHz wake. The
row is a report (risk R5: a cost row per example), not a gate.

**The duty firmware's delay** is counted in instructions, not by polling
RTC0 `COUNTER`: a running CPU sees `COUNTER` advance at its sync points (a
poll for 8 ticks overshot by 9.4 µs, almost one cell), which would have
put the designed duty 4 % off.

**P2's `pi` references re-recorded** (`tests/golden/pi-glue`): at `pi`,
dial-me-spinner's U10 (the module's nRF52840) now draws its `supply_load`
from +3V3. With U10's `supply_load` replaced by a 0 A load (static-load
mode) the three cases reproduce P2's committed hashes byte for byte, so the
difference is the load alone: at most 0.032 mV per sample (+3V3,
LED_PWR_EN, LED_DIN_MCU, LED_DIN, LED_DIN_R), 0.007 mV on VSYS, 0.003 mV on
VLED; every GPIO `pin_edge` unchanged. The checks print the same values to
the µV but one: LSW-3 with the wrong order 0.777443 V (P2: 0.777445 V);
G8's VOH 2.821693 V at VDD 2.927342 V, unchanged. The G9 decks at `pi` (now
with U10's `supply_load` on the dial-me-spinner decks) all solve.

**Deviations, recorded.**

- `#nvmc` draws nothing (no PS figure); `#radio_tx`/`#radio_rx` are
  bench-scheduled only (default 2).
- A bench `corner` does not reach a `supply_load` view (its corners are
  not applied in P3; a corner only it declares is warned of as unused).
- STM32 targets have no state outputs and no `idd` tables (default 5).
- `[[activity]]` takes an `at` that ADR-010 D2 does not show.
- `examples/nrf-idd` has a resistor where examples/nrf-feedback has its LED.
- **Budget:** `pi-idd-test` 4.3 s in a pooled `make check` (490 s in
  all) and 7.2 s with `POOL=0` (582 s), seven runs in parallel, beside other
  tracks (load 6-7); `nrf_state_outputs_exact` about 17 s of its own, in
  parallel with the `nrf52840` binary's other tests (the binary took 31.4 s
  and 52.9 s in those runs); within the +30 s. `make pi-idd` 69 s,
  `make pi-glue` 60 s (P2's gate with the re-recorded references),
  `make plugin-scope` 4.2 s and `make lock-scope` 63 s (`examples/nrf-idd`
  among the 11 nRF52840 locks that change under the saadc edit).
