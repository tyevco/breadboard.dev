# ADR-009: `pi` fidelity (power views, idd, rail-tracking pins, POFWARN and brown-out)

- **Status:** Accepted (2026-11-15; see "Approver decisions
  (2026-11-15)"). Proposed 2026-11-14, revised after design review round 1
  (see "Review notes"). Written before implementation, because it changes
  the package schema, the pin model and the event stream (AGENTS.md).
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
| G3 | P0: the suite replicas reproduce `results.json` (1 mV, 10 µs); the static-load tolerances fixed from the measured difference; W1's feasibility row (plan section 2) |
| G4 | W1 with the firmware in the loop: the rule passes with `pofwarn` fired; both negative controls fail exactly `cue-held`; every W1 assertion reports `rung: pi`. W2, or its fallback (plan section 5) |
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
