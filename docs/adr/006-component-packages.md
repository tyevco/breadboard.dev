# ADR-006: Component packages v0 (part.toml, pins.toml, model views, local packages and lockfile)

- **Status:** Accepted (2026-11-09), with the approver decisions below
  (revised 2026-11-09 after two design reviews, see Review notes).
  Implemented for plan M3 (2026-11-09), with Amendment 1; S1 (Amendment 2);
  M5 and M5b (2026-11-09: the E1 packages, the datasheet-fitted TPS22917
  view and the local-only vendor-derived one, Amendment 3). Used by E1 in
  plan M6 (2026-11-09: module bindings, the nRF drive-set names, the
  board-level convergence gate; Amendment 4).
  Written before implementation because it defines the package schema
  (AGENTS.md: ADRs come before anything that touches the package schema).
- **Part of:** the Phase 1 plan (`docs/phase1-plan.md`), milestones S1, M3,
  M5 and M5b (required by approver decision A), and M10 for D7.
- **Normative changes when implemented:**
  - new `spec/package.md`;
  - new `spec/schema/part.schema.json`, `spec/schema/pins.schema.json`,
    `spec/schema/lock.schema.json` and `spec/schema/pkgtest.schema.json`;
  - an amended `spec/pin-model.md`;
  - `spec/time-grant.md` and `spec/event-stream.md` do not change.
- **Evidence:**
  - the design doc ("Component package spec", "Custom logic in packages",
    the fidelity ladder);
  - today's `cli/src/board.rs`, `examples/*/board.cir` and
    `engines/analog-ngspice/src/lib.rs`;
  - the dial-me-spinner SPICE suite at `dc96515` (the models are identical at
    `6854d25`, the reference since approver decision C): `hardware/sim/models/*.lib`
    (58 files), their header blocks, and `spicelib.py`'s value parsing and
    ground handling;
  - measurements in `~/bbscratch-p1-design`, `~/bbscratch-p1-crit`,
    `~/bbscratch-crit-p1` and `~/bbscratch-p1-rev`.

## Approver decisions (2026-11-09)

Recorded after the human approver reviewed the Phase 1 plan and ADR-006,
ADR-007 and ADR-008 (the same decisions A–C are in each of them).

- **A. Every recommended answer is accepted.** For this ADR's open
  questions:
  1. **Option sets in qualification (D6):** accepted: `phase0` is the
     default, a looser named set is admissible for a view only if its
     vectors agree with `phase0` within tolerance, and `bb run` refuses
     an unqualified bench option set unless `--allow-unqualified`.
  2. **`confidence`:** the enum (`unchecked`, `self_checked`,
     `datasheet_checked`, `measured`).
  3. **Licences and TI-derived views:** Apache-2.0 for first-party
     packages (and breadboard.dev). The TI-derived TPS22917 view stays
     **local-only** (`LicenseRef-local-only`, outside the repository,
     never committed), and a datasheet-fitted view is the committed
     default: **plan M5b is required.** The reference `.net`,
     `.kicad_sch` and `.kicad_pro` files and the migrated (non-TI-derived)
     suite models are vendored under `examples/dial-me-spinner/`.
  4. **Schema restructure versus the design doc:** accepted as a recorded
     deviation.
  5. **Device API in Rust only for Phase 1,** behavioural code pinned by
     hash: accepted.
  6. **Lattice qualification as a hard refusal** in `bb run`, with
     `--allow-unqualified` recorded in `run.json`: accepted.

  Plan-level answers that bind this ADR too: S1 (the numerics spike) with
  the datasheet-fitted view as fallback; integer-only canonical JSON.
- **B. Publication:** nothing is published, pushed or given a remote
  during Phase 1's milestones; M7 tests from a local clone and the human
  decides publication afterwards (plan, Approver decisions B).
- **C. Reference revision:** dial-me-spinner `6854d25` (branch
  `kicad10-port`, KiCad 10.0.8) replaces `dc96515` as the reference.
  `hardware/sim/models/*.lib` are byte-identical between the two commits
  (`git diff --stat dc96515 6854d25 -- hardware/sim` lists only
  `spicelib.py` and `tests/test_led_data.py`), so D9's migration and the
  M0 `results.json` both refer to `6854d25`.

## Context

### What exists today

A part is two `board.cir` directives:

- `*bb part U1 engine=renode platform=stm32f072`;
- one `*bb pin` line per bound pin, with the pin model's numbers written by
  hand.

The analog side is the raw SPICE body, including its `.options` line: the
ngspice engine adds only the title, the input glue, the `.tran` card and
`.end` (`NgspiceConfig::netlist`). Nothing records:

- where a number came from;
- which model of a part is used;
- whether two boards use the same model.

`spec/pin-model.md` already says that "every component pin has exactly one
pin model, supplied by its package (in Phase 0, hand-written in the board
netlist)". This ADR defines that package.

### What the reference board needs (evidence)

1. **Several analog views per part, and they disagree.** dial-me-spinner's
   suite has one `.lib` per (test file, part):
   - the BQ25180 has 8 variants and the nRF52840 has 8;
   - the TPS22917 has 4 (`load_switches__`, `oled_boost__`,
     `power_integrity__`, `sleep_current__`);
   - the WS2812C has 3: a DIN-only load, a per-pixel supply plus DIN, and a
     lumped ring with 2 kHz PWM.

   Their parameters conflict: nRF GPIO output resistance is 200 Ω in one
   file and 800 Ω in another, and BSS138 Vth is 1.2 V in one and 1.5 V in
   another. Each variant is right for its own test circuit and purpose. A
   package must hold several views keyed by purpose and fidelity, each with
   its own provenance. It must never hold one merged model.
2. **Provenance is already written down, in prose.** Every suite model
   header names:
   - the datasheet and its revision;
   - what is modelled, with the table or section for each number;
   - the assumptions;
   - what is NOT modelled.

   For example, `load_switches__tps22917.lib` says it is a translation of
   TI SLVMCE8, lists the replaced turn-on block and fixed vendor artifacts,
   and names the test that re-checks it against Table 7.6 on every run.
   This is exactly the design doc's `provenance` block, but unstructured.
3. **Lattice qualification is a real failure mode, and solver options are
   part of it** (measured; this ADR's main new requirement). Breadboard's
   ngspice engine requires TMAX to divide the 10 µs grant lattice and clamps
   every solver step to the next lattice point (`NgspiceConfig::lattice`).
   The suite runs its decks at ngspice's default TMAX (20 µs), and its
   `run()` retries a stalled deck with looser options; its LED rail
   sequence deck starts on the last fallback (reltol 1e-3, Gear,
   `rshunt` 1e12) because "the TPS22917 logic edges stall the default step
   control".

   **Standalone ngspice 42** (the suite's E1 sequence deck at the cold
   corner, TMAX 10 µs, 120 s cap, 7 runs in parallel at load 11,
   `~/bbscratch-p1-rev`):

   | Options | Result |
   | --- | --- |
   | Phase 0 (`RELTOL=1e-5 VNTOL=1e-8 ABSTOL=1e-13`) | killed at 120 s, stuck at the LED_PWR_EN fall (15.6 ms) |
   | Phase 0 + `rshunt=1e12`; + Gear + `rshunt`; reltol 1e-4 + Gear + `rshunt` | killed at 120 s |
   | Phase 0 + Gear | 112.8 s |
   | reltol 1e-4 | 85.4 s |
   | suite fallback (reltol 1e-3, Gear, `rshunt` 1e12) | 4.4 s |

   Every run that finished gives the same VLED to 1 µV (2.971511 V at
   10.5 ms, 0.104326 V at 50.6 ms). The QOD deck at reltol 1e-5 also fails:
   "Timestep too small" at 0.1 ms with TMAX 20 µs, a hang at 0.1 ms with
   TMAX 10 µs (critique measurement).

   **Through `NgspiceEngine`** (lattice 10 µs, TMAX 10 µs, the two control
   edges delivered as 100 ns digital glue, a scratch harness on the real
   crate, grants of 10 µs, 50 µs, 0.1 ms, 0.5 ms and 1 ms, 120–240 s caps):

   | Options | Result (the same at every grant size) |
   | --- | --- |
   | Phase 0 | reaches 15.6 ms in about 1.5–3.3 s (VLED 2.971511 V at 10.5 ms), then stalls at the LED_PWR_EN fall |
   | reltol 1e-4; suite fallback | pass the fall, then stall at 27.92 ms, during QOD (VLED 0.914 V) |
   | Phase 0 + Gear | stops ("ngspice stopped") at the LED_PWR_EN rise |

   So a standalone `.tran` run at the right TMAX does **not** predict what
   the engine does (the suite fallback finishes standalone in 4.4 s and
   stalls in the engine), an option set can decide whether a deck runs at
   all, and no option set measured gets E1 through the engine. The stall
   points do not move with the grant size, as ADR-003's partition
   invariant predicts. The TPS22917 turn-off and QOD path needs a
   view-local fix before E1 can run (plan S1).
4. **The ground rule.** ngspice treats a node named `gnd` as global ground,
   even inside a subcircuit. The suite renames most such ports, and its
   `power_integrity__ws2812c.lib` warns about it in its header. One file
   does not: `load_switches__tps22917.lib` declares
   `.subckt LSW_TPS22917 CT GND ON QOD VIN VOUT` (line 55). A package that
   names a port `gnd` would silently short a second board's ground to
   node 0.
5. **Passives dominate.** On the main board, 118 of 201 parts are generic
   R/C/L resolved by footprint plus value. They need value parsing with
   tolerances and voltage ratings: `4k7`, `10u/25V`, `499k 0.5%`, `1M`,
   `2.2M`. The suite's `spicelib` already does this, and reads `M` as mega.
   ngspice reads `M` as **milli** (`R1 a 0 1M` at 1 V gives 1000 A; `1Meg`
   gives 1 µA; measured with ngspice 42).
6. **Behavioural stand-ins.** Bus devices (TMAG5273, LIS2DW12, BQ25180,
   W25Q128) cannot be served outside Renode in Phase 1:
   - Renode 1.17.0's nRF52840 TWI, SPI, I2S and UART ignore their
     pin-select registers and drive no pins, so a kernel-side
     `on_i2c_write` has nothing to listen to on a net.
   - **SPI has a transaction-level route:** Renode 1.17.0's External Control
     can attach an `ExternalControlSPIPeripheral` (`OnTransmit`,
     `OnFinishTransmission` callbacks) to the nRF SPI2 container
     (`NRF52840_SPI` is a `NullRegistrationPointPeripheralContainer<ISPIPeripheral>`).
     Its callbacks are synchronous inside `RunFor`, so a device served that
     way needs a time-grant analysis before it is deterministic.
   - **I2C has no route:** External Control has no I2C command.

   Pin-level stand-ins can sit outside Renode: a Hall switch fed by a
   position model, or a WS2812 bitstream decoder. Phase 1 needs at most one,
   in stretch example E3.
7. **The cost of a device in another process** (measured, for the
   Python-versus-Rust decision). A JSON-lines round trip to a Python 3.12
   child over pipes took **85.5 µs** (median of 3×20 000, load average
   about 8). Python start-up was 20 ms. Renode's one-cell grant costs
   310–450 µs. So a Python device in a feedback loop adds about 20–25 % per
   one-cell grant. That is acceptable, but not free.
8. **Behaviour that decides results is not all data.** The nRF52840's
   behaviour lives in `engines/cpu-renode/platforms` (the `.repl` overlay and
   C# models such as ADR-008's `BbNrf52Saadc`), and a Phase 1 device lives
   in `bb` (D7). ADR-005's plugin cache already keys the compiled assembly
   by a hash of every plugin source.

## Decision

### D1. Package directory layout

```
packages/                         # first-party packages, in breadboard.dev
  generic/resistor/               # id "generic/resistor"
    part.toml
    pins.toml
    analog/resistor.lib
    tests/                        # bb pkg test vectors (D6)
  nordic/nrf52840-qiaa/
    part.toml
    pins.toml
    analog/gpio.lib  analog/saadc_se.lib
    tests/
    PROVENANCE.md                 # optional long-form notes; the structured fields are in part.toml
  raytac/mdbt50q-u1mv2/           # a module: pinmap onto nordic/nrf52840-qiaa
  ti/tps22917/
  worldsemi/ws2812c-2020/
```

- **Package id:** `<namespace>/<name>`, lower case, `[a-z0-9._-]`. The
  namespace is the manufacturer, or `generic` for generic parts. The chip
  package id is `nordic/nrf52840-qiaa` everywhere (plan, ADRs, examples).
- **One directory is one package version.** The version is in `part.toml`,
  not in the path. There is no registry, so a project selects packages by
  path and the lockfile pins the content.
- **Search path:**
  1. directories listed in the project's `bb.toml` (`[packages] paths = [...]`,
     relative to the project), in order;
  2. then breadboard.dev's `packages/` (first-party).

  The first directory that provides an id wins, so a private package shadows
  a first-party one. Two packages with the same id in one directory are an
  error.

### D2. `part.toml` v0

TOML is the authored form. The JSON Schema in `spec/schema/part.schema.json`
is canonical, as the design doc proposes: tools convert TOML to JSON and
validate. Unknown keys are an error in v0, because the schema must stay
honest while it is young. **Reserved keys** (D11) are recognised and
refused with "reserved for bb-part/1", so Phase 2 adds them without a
breaking rename.

```toml
schema = "bb-part/0"

[part]
id = "ti/tps22917"
version = "0.1.0"                       # semver; D11 says when to bump
kind = "analog_ic"                      # soc | module | power | analog_ic | logic | discrete | passive | connector | mechanical | testpoint
manufacturer = "Texas Instruments"
mpn = ["TPS22917DBVR", "TPS22917DBVT"]  # exact manufacturer part numbers (resolution, ADR-007 D4)
mpn_patterns = []                       # optional anchored regexes over the whole MPN, e.g. "TPS22917DBV[RT]"
lcsc = ["C2681320"]                     # optional extra lookup key
datasheet = { url = "https://www.ti.com/lit/ds/symlink/tps22917.pdf", rev = "SLVSDW8B" }
license = "Apache-2.0"                  # SPDX; "LicenseRef-local-only" forbids committing to a shared repo (D9)
description = "1-5.5 V, 2 A load switch, adjustable rise time, quick output discharge"

[footprints]
kicad = ["Package_TO_SOT_SMD:SOT-23-6"] # informative; resolution does not depend on it except for generic parts
pinmap = "pinmap.csv"                   # optional: footprint pad -> pin name, when the pad numbers are not the pin numbers

[pins]
file = "pins.toml"
# ibis = "..."                          # reserved (bb-part/1)

[module]                                # only for kind = "module"
chip = "nordic/nrf52840-qiaa"           # the chip package the module contains
pinmap = "module-pinmap.csv"            # module pad -> chip pin name

[behavioral]                            # at most one; absent for passive or analog-only parts
engine = "renode"                       # an open, versioned engine id: "renode" | "device" in Phase 1 (D3)
platform = "nrf52840-bb@1"              # platform name @ major version, known to engines/cpu-renode (D8, ADR-008 D1)
# cpu, svd                              # reserved (bb-part/1)

[[analog]]                              # zero or more analog views (D3)
name = "lsw"
kind = "circuit"                        # circuit (default) | pin_load (D4, ADR-008 D7)
purpose = "rail switching, turn-on ramp, quick discharge, reverse blocking"
fidelity = ["fast", "pi"]               # rungs this view is valid for (D3)
default_for = ["fast", "pi"]
file = "analog/tps22917.lib"
subckt = "LSW_TPS22917"
ports = ["CT", "GND", "ON", "QOD", "VIN", "VOUT"]   # subckt port order, as pin names from pins.toml
params = { ron_add = "0", isd_out = "0" }           # D5 grammar; parsed, then emitted as SPICE numbers
[[analog.qualified]]                                # written by `bb pkg test --record` (D6)
lattice = "10us"
options = "phase0"                                  # a named option set (D6)
[analog.provenance]
source = "vendor_derived"                           # datasheet | vendor_model | vendor_derived | measured | estimated | agent_generated
base = "TI SLVMCE8 TPS22917_TRANS.LIB Final 1.00 (14 Dec 2017)"
refs = ["SLVSDW8B Table 7.5", "SLVSDW8B Table 7.6"]
deviations = ["turn-on block replaced, recalibrated to Table 7.6", "IIN,RCB leak gated by the RCB latch", "QOD path as one B-source", "port GND renamed GNDP (D9)"]
not_modelled = ["temperature (vendor note 2)", "RON max-vs-temperature except via ron_add"]
origin = "dial-me-spinner@6854d25:hardware/sim/models/load_switches__tps22917.lib"
generated_by = ""                                   # agent or tool, when generated
reviewed_by = []                                    # people who reviewed it
confidence = "self_checked"                         # unchecked | self_checked | datasheet_checked | measured

[provenance]                                        # package-level default for views without their own block
source = "datasheet"
confidence = "datasheet_checked"

[tests]
dir = "tests"

# Reserved (bb-part/1): [layers.*], [analog].rails, [power].idd, [thermal], [rtl]
```

- **`confidence` is an enum, not a number.** The design doc's 0..1 number
  is a float in an otherwise integer-only canonical form (ADR-007 D6), and
  an author-stated number is easy to inflate. The enum says what was
  checked: `unchecked`; `self_checked` (passes its own package vectors);
  `datasheet_checked` (vectors are datasheet table values);
  `measured` (vectors are bench measurements). Approver question 2.
- **`kind`** follows the design doc where it has a name (`soc`, `power`)
  and adds `module`, `mechanical` and `testpoint`, which the reference board
  needs.

### D3. Model layers, views and the fidelity ladder (Phase 1)

The design doc defines five layers. Phase 1 implements three:

- **`pins`** (always present, D4). It holds each pin's role and pin model.
  This is what `spec/pin-model.md` calls "supplied by its package".
- **`behavioral`** (at most one):
  - `engine = "renode"`: the part *is* the CPU engine's machine. The
    package names a platform identity that `engines/cpu-renode` provides
    (D8). It does not ship `.repl` or C# files in Phase 1: Breadboard's
    Renode plugins are compiled together into one cached assembly
    (ADR-005 D6), and that assembly stays in `engines/cpu-renode/platforms`.
  - `engine = "device"`: a pin-level device from D7.
  - Other engine ids (`renode-peripheral`, `qemu`, …) are refused until an
    ADR defines them. The field is an open identifier, not a closed enum,
    so Phase 3's QEMU target adds a value, not a schema change.
- **`analog`** (zero or more named views). Each view is a SPICE subcircuit
  file plus its kind, port map, parameters, fidelity rungs, qualification
  records and provenance.

**Selection, with the ladder.** The ladder is ordered
`fast < pi < si < em`. For a run at fidelity `F`, an instance uses:

1. the view named in the project's per-instance override
   (`[instances."main:U2"] analog = "lsw"`, ADR-007);
2. otherwise the view whose `default_for` contains `F`;
3. otherwise, walking **down** the ladder from `F`, the view whose
   `default_for` contains the first lower rung that has one;
4. otherwise **no analog model**: the part is left out of the analog engine
   and listed as unmodelled in `board.bbm` and `run.json` (honesty rule:
   never a silent default).

`run.json` records, per instance, the requested rung, the view used and the
rung it was chosen at, so a `pi` run that fell back to a `fast` view says
so. A view never moves **up** the ladder.

- **Lint:** at most one view per package may name a given rung in
  `default_for`, and `default_for` ⊆ `fidelity`.
- **Phase 1:** only `fast` runs. E1's views are tagged with every rung
  their authors validated them for (the TPS22917 `lsw` view models power
  sequencing and QOD, so it carries `["fast", "pi"]`). Phase 2 therefore
  adds `pi` views without re-tagging, and without changing tree hashes,
  locks or goldens.

Layers `thermal` and `rtl` are not in v0. Their keys are reserved (D11).

**Elaboration of views into one deck.** Views are inlined into one SPICE
deck (ADR-007 D7). Every top-level name a view defines (`.subckt`,
`.model`, `.func`, `.param` outside a subckt) is collected per resolved
set:

- byte-identical definitions of one name (two instances of one view, or
  two views sharing a helper `.model`) are emitted once;
- different definitions of one name are an error naming both views.

Prefix renaming (package id plus view) needs a SPICE tokenizer and is
deferred to the registry ADR, where third-party views make collisions
likely. Lint runs the same check over each package's own views.

### D4. `pins.toml`

```toml
schema = "bb-pins/0"

[[pin]]
number = "A12"                # footprint or chip pin number, as KiCad reports it (string); aQFN73 ball, checked against the PS pin table in M5
name = "P0.02"                # logical name; KiCad libpart pin names must match (ADR-007 D3)
role = "io"                   # power_in | power_out | ground | io | input | output | analog | nc | passive
supply = "VDD"                # pin name of the supply that its thresholds refer to (optional)
alt = ["AIN0"]                # informative alternate functions
models = ["digital_out", "digital_in", "adc_in"]   # the pin models this pin may be bound with
adc = { channel = "AIN0" }    # only for analog-capable pins (ADR-008 D2)

[defaults.digital_out.S0S1]   # named parameter sets, chosen per binding by `set` (ADR-007 D5)
voh = "VDD"                   # a number with unit, or the name of a supply pin: resolved at elaboration
vol = "0V"
rise = "25ns"                 # placeholders in this example; real numbers come from the PS in M5
fall = "25ns"
zout = "800"
[defaults.digital_out.H0H1]
voh = "VDD"
vol = "0V"
rise = "8ns"
fall = "8ns"
zout = "200"
[defaults.digital_in.default]
vih = "0.7*VDD"
vil = "0.3*VDD"
cin = "3p"
[defaults.adc_in.default]     # ADR-008 D7
model = "nrf_saadc"
load = "saadc_se"             # name of a pin_load view, or "none"
```

- **Parameter values are strings** in the one grammar of D5. Each is
  either a number with an optional unit, a supply pin name, or the product
  of a decimal factor and a supply pin name (`0.7*VDD`). There is no
  general expression language.
- **Supply references resolve in this order** at elaboration:
  1. the bench supply on the net that pin is connected to (its `volts`);
  2. the bench's `[nominal]` table entry for that net (ADR-007 D5), for a
     rail that an in-scope part generates (for example +3V3 from the
     RT9080);
  3. otherwise an error naming the pin and the net.

  Elaboration then converts the value to the integer units of `board.bbm`
  (ADR-007 D6).
- **The parameter set used by a binding** is named by the binding's `set`
  key (ADR-007 D5; one key name everywhere), or is the set named
  `default`. If neither exists, elaboration fails with an error.
- **Pin-attached loads.** A view of `kind = "pin_load"` is a load the pin
  model attaches to its own pad (ADR-008 D7). Its `ports` are typed:

  ```toml
  [[analog]]
  name = "saadc_se"
  kind = "pin_load"
  ports = [{ role = "pad" }, { role = "ctrl", signal = "smp" }, { role = "observe", signal = "hold" }, { role = "pin", pin = "VDD" }]
  ```

  `pad` is the bound pin's net, `ctrl` a digital control driven by the CPU
  engine, `observe` a node the engine reads back, `pin` another package pin.
  Those control and observe signals are internal to the binding: they are
  not pins and need no `pins.toml` entry.
- **Lint:**
  - every pin of the footprint (or module pinmap) is listed exactly once;
  - every `ports` entry of a `circuit` view names a pin, and every `pin`
    port of a `pin_load` view names a pin;
  - no subckt port or internal node is named `gnd` (Context 4);
  - parameter sets are complete for their model;
  - `vil < vih` after resolution at the bench nominal;
  - every numeric string parses under D5 (a bare `M` is an error);
  - the D3 top-level-name check.

### D5. Value grammar, and generic passive packages

**One grammar** is specified in `spec/package.md` for every numeric string
in `part.toml`, `pins.toml`, test vectors and the bench:

- a decimal number, optionally with an exponent (`3.3`, `1e-5`);
- an optional SI prefix: `f p n u µ m k Meg G T` (`m` milli, `Meg` mega);
- **a bare `M` is a lint error** that suggests `Meg` or `m`, because
  ngspice reads it as milli and EE text reads it as mega (Context 5);
- an optional unit (`V A s F H Ω ohm Hz %`), checked against the
  parameter's dimension;
- for capacitors, resistors and inductors from KiCad (below), the
  conventional forms too.

Strings are **never passed through to SPICE**. Elaboration parses each to
an integer in the D6 units of ADR-007 and emits a plain SPICE number
(`2.2e6`), so no consumer ever sees an ambiguous suffix.

**Generic passive packages:**

- **The packages:** `generic/resistor`, `generic/capacitor`,
  `generic/inductor`. Each has two pins (`1`, `2`, role `passive`) and one
  view, `ideal`, which is the primitive `R`, `C` or `L` with `{value}`. They
  carry no MPN.
- **They are selected by footprint plus value** (resolution stage 5,
  ADR-007 D4): footprint library `Resistor_*`, `Capacitor_*` or `Inductor_*`,
  or reference prefix `R`, `C` or `L` when the footprint is a custom one.
- **KiCad value fields** follow EE convention, as the suite's `spicelib`
  does:
  - `M` is **mega** here (`1M`, `2.2M` on R2, R20, R59, R1), as are `Meg`
    and `MEG`; `m` is milli; `K` is accepted as `k`;
  - "RKM" infix (`4k7`, `2R2`);
  - an optional unit letter (`Ω`, `ohm`, `F`, `H`), ignored;
  - an optional `/<voltage>V` rating;
  - an optional `<x>%` tolerance;
  - anything else is a parse error, never a guess.

  The result is the exact value in integer sub-units plus the tolerance and
  rating as metadata. M3 tests the parser against all 29 distinct values of
  the main board.
- **Tolerance and rating** are kept for later Monte Carlo and DC-bias views
  (`pi`). At `fast` the nominal value is used.
- **A specific MPN package overrides the generic one** whenever it exists:
  for example, an MLCC package with a DC-bias table as a `pi` view. Every
  main-board passive has an MPN (KiCad research §3), so this happens as
  soon as such packages exist.

### D6. Package tests and lattice qualification (through the engine, per option set)

`tests/` holds one or more vectors:

```toml
# tests/qod_discharge.toml
schema = "bb-pkgtest/0"
view = "lsw"
circuit = "qod_discharge.cir"          # a SPICE body that instantiates the view as X1 (ports by name); no .tran, no .options
stop = "50ms"
[[edge]]                               # control inputs, delivered as digital_out glue, not as PWL
net = "on"
model = { voh = "1V", vol = "0V", rise = "100ns", fall = "100ns", zout = "1" }
at = [["0.1ms", "high"], ["5ms", "low"]]
[[expect]]
meas = { node = "vout", at = "4.95ms" }   # or { node, when = "<v>", fall|rise = n, after = "<t>" }
value = "2.984V"
tol = "10mV"                           # absolute
source = "examples/dial-me-spinner/suite/results.json: test_u2_qod_discharge[bat4.2] (6854d25)"
```

- **Named option sets.** `spec/package.md` defines `phase0`
  (`RELTOL=1e-5 VNTOL=1e-8 ABSTOL=1e-13`, the options of every Phase 0
  `board.cir`), the default for imported boards. Others are added by name
  with their exact contents (for example `suite_fallback`: reltol 1e-3,
  Gear, `rshunt` 1e12).
- **`bb pkg test` runs every vector through `NgspiceEngine` itself**, not a
  standalone `.tran`: lattice `L`, TMAX = `L`, the engine's step clamp and
  breakpoint release, the named option set, and the `[[edge]]` inputs
  delivered as `digital_out` glue at their times. Context 3 shows that a
  standalone run does not predict the engine. The vector runs at two grant
  partitions (one-cell and 1 ms) and requires identical results.
- **A view is lattice-qualified for (`L`, option set)** when all its
  vectors pass under that pair. `bb pkg test --record` writes the
  `[[analog.qualified]]` entries; a content change invalidates them through
  the tree hash (D8).
- **A looser option set is admissible only by evidence.** A view may be
  qualified under an option set other than `phase0` only if every vector
  also runs under `phase0` and the two results agree within the vector's
  tolerance. "Loosen until it runs" is therefore not a qualification. If
  `phase0` cannot run a vector at all, that view cannot be qualified under
  a looser set either, and the view needs a fix (S1) or a replacement.
- **`bb run` checks the run against qualification.** The bench's
  `[options]` must be a named option set, and every in-scope view must be
  qualified for (`L`, that set). Otherwise `bb run` refuses, unless the
  user passes `--allow-unqualified <view>`, which `run.json` records. The
  option set is recorded in `run.json`.
- **Board-level convergence is separate.** A view qualified in its own test
  circuit says nothing about a 243-element deck. Each example board gets a
  convergence gate at its bench's option set (plan M6).
- **Gate (S1/M5):** every view used by E1 is qualified at (10 µs,
  `phase0`). The TPS22917 view fails this today (Context 3). The fix belongs
  in the view (the suite already replaced two numerically fragile vendor
  blocks), and is recorded under `deviations`. The fallback is a
  datasheet-fitted view (D9).

### D7. Behavioural stand-ins: the Phase 1 device API is Rust, in-process

**Decision.** Phase 1's pin-level device API is a Rust trait in a new crate,
`bbdev`. Devices are compiled into `bb`, and a package names one with
`[behavioral] engine = "device"` and `device = "<id>@<major>"`. Bus-attached
devices are out of Phase 1 (Context 6). The Python API moves to Phase 2,
under its own ADR, with the same call shape.

```rust
pub trait Device {
    /// Pins, each with direction (In: digital_in from the kernel; Out: digital_out to the kernel).
    fn pins(&self) -> &[DevicePin];
    /// A digital input changed at `t` (applied at `t`, never earlier).
    fn on_pin_change(&mut self, ctx: &mut Ctx, pin: usize, level: Level, t: Time);
    /// A timer the device scheduled fired at `t`.
    fn on_timer(&mut self, ctx: &mut Ctx, id: u32, t: Time);
}
// Ctx: now(), schedule(at: Time, id: u32), cancel(id), set_pin(pin, Level) (stamped at now()),
//      rng() seeded from the run seed and the instance ref. No wall clock and no I/O.
```

`DeviceEngine` wraps any number of devices as one engine. It follows
`spec/time-grant.md`:

- **`t_safe`** is the earliest scheduled timer, which is exact. Outputs
  change only at timer or input instants, both of which the engine knows.
- **`quiet_until`** is the same: a device has no hidden time source.
- **`input_is_sampled`** is `false`: a device may react to any edge.
- Every output change is reported in its trace with its exact time.

**Why Rust, and why not Python, now:**

- **Determinism review.** A device in the kernel's language uses `Time`
  (integer fs) and `Level` directly. It gets the same equivalence gates as
  every engine, and no new protocol enters `spec/`. A Python device would
  need a wire protocol for advance, trace, `t_safe` and promises. That is a
  `spec/time-grant.md` change with its own ADR and gates.
- **Phase 1 needs at most one or two devices,** in E3 only. E1 and E2 need
  none.
- **Cost was not the decider.** The measured 85.5 µs per Python round trip
  is about 20–25 % of a one-cell Renode grant (Context 7). It is noted here
  for the Phase 2 ADR.
- **The design doc asks for "5 example sensor packages" and a Python API**
  in Phase 1. Both would be bus sensors. I2C devices have no route at all
  in Renode 1.17.0. SPI devices could be served through External Control,
  but its synchronous mid-`RunFor` callbacks need a time-grant analysis
  (who answers MISO, at what virtual time, under a promise) that is an ADR
  of its own. That makes them Phase 2 work. This is a recorded deviation
  (plan §7 Q7).
- **Recorded consequence:** in Phase 1 an agent cannot add a behavioural
  part without changing `bb` itself. Behavioural code becomes package
  content with the WASM/Python ADR (Phase 2).

### D8. Local packages, platforms and the lockfile (`bb.lock`)

- **Written by** `bb open`, next to `bb.toml`. It is TOML, entries are
  sorted by id, and it is validated by `spec/schema/lock.schema.json`:

  ```toml
  schema = "bb-lock/0"
  [[package]]
  id = "ti/tps22917"
  version = "0.1.0"
  path = "../../packages/ti/tps22917"      # as found on the search path, relative to the project
  tree = "sha256:4b1c…"                   # tree hash, below

  [[platform]]                             # engine-side behaviour that decides results
  id = "renode:nrf52840-bb@1"
  content = "sha256:9e07…"                 # hash of the .repl overlay and every plugin source it loads
  renode = "1.17.0"
  [[platform]]
  id = "device:dms/mechanism@1"            # only if E3
  content = "sha256:…"                     # hash of the device's source files, as built into bb
  ```

- **Tree hash:** sha256 over the sorted list of `"<relative path>\0<sha256 of
  content>\n"` for every regular file in the package directory. The list
  excludes dotfiles and `tests/out/`. Paths use `/` and content bytes are
  hashed as stored, so a CRLF checkout changes the hash. `.gitattributes`
  already forces LF.
- **Platform identity.** Every Renode platform and every `bbdev` device
  has an id with a major version (`nrf52840-bb@1`) and a content hash. For
  Renode it is the same hash ADR-005's plugin cache key uses (every source
  in `PLUGIN_SOURCES`) plus the `.repl` files the platform loads; for a
  device, its source files. `bb` embeds the table at build time. A package
  names a platform by id and major; a behaviour change bumps the major and
  the hash.
- **When the lockfile is enforced:** `bb open` and `bb run` refuse a
  package whose tree hash, or a platform whose content hash, differs from
  the lock, unless the user passes `bb open --update-lock`. `bb run` never
  rewrites the lock (ADR-007 D7). This is the design doc's "reproducible
  years later" property without a registry, now covering the behavioural
  code too. `run.json` records both kinds of hash.
- **Recorded deviation:** platform and device code is pinned by hash but
  is not package content until the WASM/Python ADR (D7).
- **Signing is deferred** to the registry (Phase 2).

### D9. How `hardware/sim/models/*.lib` become packages

The suite's files are named `<test file>__<part>.lib`. Each becomes **one
view** of the part's package:

| Package | Views (purpose) |
| --- | --- |
| `ti/tps22917` | `lsw` (load_switches), `oled` (oled_boost), `pi` (power_integrity), `sleep` (sleep_current) |
| `worldsemi/ws2812c-2020` | `din` (led_data DIN load), `pixel` (load_switches: supply + DIN per pixel), `ring` (power_integrity, 48 lumped) |
| `nordic/nrf52840-qiaa` | `gpio_led`, `gpio_lsw`, `gpio_motor`, `gpio_bus` (the GPIO files), `saadc_se` (`pin_load`), `usb`, `load`, `sleep` |
| … one row per part: 22 parts, counting the stepper and the link harness | |

The rules:

1. **Copy the file as it is at `6854d25`,** with only **recorded mechanical
   deviations**: a port or node rename (for example `GND` → `GNDP` in
   `LSW_TPS22917`, which the `gnd` lint requires) and the S1 numerics fix.
   Each is listed under `deviations`, and a vector run with the original
   and the changed file must give identical results (for a rename) or
   results within tolerance (for a numerics fix). Record `origin`, map the
   header onto the `provenance` fields (`source`, `base`, `refs`,
   `deviations`, `not_modelled`), and keep the full header text as the
   file's comment.
2. **Conflicting parameters stay per view.** Lint lists them in a
   `conflicts` report (for example the GPIO `rout` of 200 Ω against 800 Ω),
   and M5 resolves the ones E1 uses against the nRF52840 PS before setting
   `default_for`.
3. **Migrate only what a scenario uses:** E1 in M5, E2 in M8, E3 in M10.
   The rest stay in dial-me-spinner until needed.
4. **Licensing:**
   - **Datasheet-fitted views** are the dial-me-spinner author's own work.
     They are committed under the licence the approver chooses (plan Q2).
   - **Vendor-derived views** (TPS22917 from SLVMCE8, TPS61040 from
     SLVMDA8) are committed only if TI's terms allow redistribution of a
     derived model (plan Q3). Otherwise the package gets
     `license = "LicenseRef-local-only"` and lives in a local package path
     outside the repo. A datasheet-fitted view becomes the committed
     default (plan M5b), gated against the vendor-derived one where it
     exists. **Decided (approver decision A):** the TI-derived views stay
     local-only, so M5b is required.
   - `bb pkg lint` refuses `LicenseRef-local-only` inside breadboard.dev's
     `packages/`.
5. **Each migrated view gets the D6 vectors** from the suite test that uses
   it. The expected values come from `examples/dial-me-spinner/suite/results.json`,
   the suite re-run at `6854d25` (the reference since approver decision C)
   that plan M0 commits, with the ngspice version and the numeric fallback
   level each deck used. `6854d25` itself commits no results
   (`hardware/sim/results/` is git-ignored). `source` names the test and
   the corner.

### D10. Packages for today's examples

`st/stm32f103c8` (platform `stm32f103-offline@1`) and `st/stm32f072rb`
(platform `stm32f072-bb@1`) carry the pin models that the `board.cir` files
state today:

- `digital_out` 3.3 V, 5 ns, 25 Ω;
- `digital_in` 2.0/0.8 V, 5 pF;
- `adc_in` 3.3 V, 12 bits.

These are set `default` for the examples. They are not claimed to be
datasheet values: `source = "estimated"` and `confidence = "unchecked"`,
until someone fits them. The gate (M3) is that the three Phase 0 examples
give their committed hashes through packages.

### D11. Schema versioning and migration

Applies to `bb-part`, `bb-pins`, `bb-pkgtest`, `bb-lock`, `bb-project`,
`bb-bench` and `bb-board` (ADR-007).

- **A schema id is `<name>/<major>`.** Within a major, changes are additive
  only, and only through keys that were reserved: a reserved key becomes
  accepted. Anything else (a rename, a removed key, a changed meaning) is a
  new major and needs an ADR.
- **`bb` reads the current and the previous major** of every schema, and
  writes only the current one. Reading an older major prints the
  migration hint. A **newer** major is refused with "upgrade bb".
- **`bb pkg migrate`** rewrites a package in place to the current major and
  shows the diff. Migrations are mechanical; first-party packages are
  migrated in the same commit as the schema change.
- **Package semver:** any change to a view's results (a vector's measured
  value changes) is at least a minor bump; a changed port list, pin list or
  parameter-set name is a major bump; provenance-only edits are a patch.
  `bb pkg lint` compares against the locked version where it can.
- **Reserved now for bb-part/1:** `[pins] ibis`; `[behavioral] cpu`, `svd`;
  per-view `rails`; `[power] idd` (the design doc's `power_profile.csv`);
  `[layers.*]`; `[thermal]`; `[rtl]`.

## Exact `spec/` changes (described, not applied)

1. **New `spec/package.md`** (normative):
   - layout and search path (D1);
   - `part.toml` v0 (D2), layers, the ladder and view selection (D3),
     `pins.toml` and `pin_load` views (D4);
   - the value grammar (D5);
   - named option sets, test vectors and lattice qualification (D6);
   - lockfile, tree hash and platform identities (D8);
   - versioning (D11);
   - the `gnd` rule and the top-level-name rule;
   - "changing this file requires an ADR".
2. **New `spec/schema/part.schema.json`, `pins.schema.json`,
   `lock.schema.json` and `pkgtest.schema.json`** (JSON Schema 2020-12),
   with three complete example packages, as the design doc asks: a
   resistor, `ti/tps22917` and `nordic/nrf52840-qiaa`.
3. **`spec/pin-model.md`:**
   - The intro changes "(in Phase 0, hand-written in the board netlist)" to
     "from the package's `pins.toml` (ADR-006 D4); `board.cir` directives
     remain a way to state them by hand".
   - A new paragraph: "A pin model's parameters may be a named set
     (`digital_out.H0H1`), chosen per binding by `set`."
   - Glue rule 5: "the automatic generator from schematics" points to
     ADR-007.
   - A new glue rule 7: "an analog view's subckt and internal node names
     never include `gnd` (ngspice global ground)".
   - "Exactly one pin model per pin" gains: "a pin model may attach a
     `pin_load` view to its own pad; the view's control and observe signals
     are internal to the binding, not pins".

   No pin model's semantics change here. ADR-008 changes `adc_in`.

## Determinism impact

- **Packages and platforms are inputs.** Their content is pinned by the
  tree and platform hashes in `bb.lock` and recorded in `run.json`. The
  same project, lock and firmware give the same elaborated netlist.
- **Elaboration is deterministic:**
  - view selection is a pure function of fidelity, overrides,
    `default_for` and the ladder;
  - parameter resolution uses integer units and no floats (ADR-007 D6);
  - subckt instances are emitted in sorted instance-reference order, and
    shared top-level definitions once, in sorted name order.
- **Qualification runs through the engine** at two partitions, so a view
  whose solver points depend on the partition is caught before a board
  uses it.
- **A device engine (D7)** advances only on grants and uses `Time`
  throughout. Its RNG is seeded from the run seed and the instance
  reference. It is gated like every engine: equivalence against the
  one-cell reference, and `--engine-order` independence.
- **The kernel, the time-grant protocol and the event stream are
  unchanged.**

## Gates

- **G1 (M3):** schema validation of every first-party package in CI.
  `bb pkg lint` is clean.
- **G2 (M3):** `examples/spike`, `feedback` and `adc` through
  `st/stm32f103c8` and `st/stm32f072rb` reproduce `tests/golden/*/outputs.sha256`
  byte for byte.
- **G3 (M3):** the value parser on all main-board values (29 distinct)
  equals the suite's `spicelib` parse, including `1M` and `2.2M` as mega
  in KiCad values; a bare `M` in a package file is a lint error; emitted
  SPICE numbers never carry a letter suffix.
- **G4 (S1/M5):** every E1 view is lattice-qualified at (10 µs, `phase0`)
  through `NgspiceEngine`, at both partitions, and its vectors reproduce
  M0's `results.json` within tolerance.
- **G5 (M3):** a lockfile mismatch (package or platform) is refused, and
  `--update-lock` rewrites only the changed entry.
- **G6 (M10, only if E3):** `DeviceEngine` mock tests in `bbcore` style.
  The E3 board is equivalent to its one-cell reference.
- **G7 (M3):** two views defining one top-level name differently are
  refused; identical definitions are emitted once.
- **G8 (M5):** the `GND` → `GNDP` rename of `LSW_TPS22917` gives
  byte-identical vector results.
- **G9 (M3):** `bb` reads a bb-part/0 package after a synthetic
  bb-part/1 bump through `bb pkg migrate`, and refuses a newer major.

## Rejected alternatives

- **One merged analog model per part.** The suite shows why not: its
  variants are tuned per question and disagree (Context 1). Merging would
  hide the disagreement.
- **JSON as the authored format.** The design doc proposes TOML authored
  and JSON canonical, and agents write either equally well. TOML allows
  comments, which the provenance prose needs.
- **Shipping Renode `.repl` and C# inside MCU packages now.** ADR-005 D6
  compiles all of Breadboard's plugins into one cached assembly, for
  start-up time (one `ScanFile`). Per-package C# would mean one compile per
  package set, and plugin cache keys that depend on the project. Revisit
  with the registry. Pinning by platform hash (D8) gives the
  reproducibility without moving the code.
- **Qualifying with a standalone `.tran` run.** Measured not to predict the
  engine (Context 3).
- **A global looser solver option set.** It hides view defects and is not
  shown result-neutral per view; D6 admits a looser set only by evidence.
- **Passing parameter strings through to SPICE.** `M` means milli to
  ngspice and mega to the board's values (Context 5).
- **A Python device API in Phase 1.** See D7. It adds a protocol to
  `spec/time-grant.md` for at most one or two Phase 1 devices.
- **WASM devices.** They are sandboxed and deterministic, so they are the
  likely answer for third-party device code in a registry. They are not
  needed while device code is first-party. Phase 2 candidate.
- **Registry and signing now.** Not needed for one project on one machine.
  The tree hash gives reproducibility.

## Open questions for the approver (consolidated, with recommendations)

1. **Option sets in qualification (D6):** `phase0` is the default; a looser
   named set is admissible for a view only if its vectors agree with
   `phase0` within tolerance; `bb run` refuses a bench whose option set is
   not qualified for every in-scope view, unless `--allow-unqualified`.
   **Recommended: accept.**
2. **`confidence`:** an enum (`unchecked`, `self_checked`,
   `datasheet_checked`, `measured`) instead of the design doc's 0..1 number.
   **Recommended: the enum** (it is checkable, and keeps floats out of
   canonical JSON).
3. **Licences and TI-derived views (plan Q2, Q3):** Apache-2.0 for
   first-party packages; keep the TI-derived TPS22917 view local and commit
   a datasheet-fitted default. **Recommended: yes to both.**
4. **Schema restructure versus the design doc** (D2, D3, D11: top-level
   `[behavioral]`, `[[analog]]` views, typed `pin_load` views, reserved
   `ibis`/`idd`/`rails`/`cpu`/`svd`, kind `soc`/`power` kept): accept as a
   recorded deviation (plan §7 Q7)? **Recommended: accept.**
5. **Device API in Rust only for Phase 1** (D7), with Python deferred to
   Phase 2, and behavioural code pinned by hash rather than shipped in
   packages (D8): accept? **Recommended: accept.**
6. **Lattice qualification as a hard refusal** in `bb run`, with
   `--allow-unqualified` recorded in `run.json`, rather than a warning.
   **Recommended: hard refusal.**

## Review notes (2026-11-09)

Two critiques of the first draft. Adopted: qualification through the
engine and per option set (Context 3, D6), the restated R1 evidence, the
ladder fallback (D3), the single value grammar (D5), typed `pin_load` views
(D4), platform identities in the lock (D8), the schema versioning policy and
reserved keys (D11), the recorded `GND` port rename (D9), the
`results.json` source (D9 rule 5), the External Control SPI correction
(Context 6, D7), the `confidence` enum, one package id and the `set` key.
Not adopted as proposed:

- **"Rename every top-level name of a view with a deterministic prefix."**
  Deferred. Phase 1 refuses differing definitions of one name and dedupes
  identical ones (D3), which is cheap and safe. Prefixing needs a SPICE
  tokenizer that rewrites references inside each view; it belongs with the
  registry, where third-party views make it necessary.
- **"An explicit per-view solver option recorded in `run.json`" as the
  fallback when no view-local fix exists.** Not adopted in that form:
  ngspice options are global to a deck, so a per-view option is really a
  per-bench option set, which D6 already names, qualifies by evidence and
  records in `run.json`. Context 3 also shows that no option set measured
  gets E1 through the engine, so the fallback that can work is a different
  view (plan M5b), not different options.
- **"Edges in qualification as the package's rise/fall."** Adopted in
  form (`digital_out` glue), but each vector states its own edge model,
  because one package's view is driven by another package's pin.

## Amendment 1 (2026-11-09): M3 as implemented

Plan M3 implemented D1-D6, D8, D10 and D11 (`spec/package.md`,
`spec/schema/*.schema.json`, the `bbpkg` crate, `bb pkg`, `bb open` for
board files, `packages/`). Where the implementation had to settle something
this ADR left open, or differs from its text, the decision is recorded here.
None of it changes the kernel, the time-grant protocol or the event stream.

1. **KiCad value qualifiers (D5).** D5 lists `/<voltage>V` and `<x>%` as the
   only additions to a KiCad value. Five of the main board's 29 distinct
   (kind, value) pairs carry other text: `12p C0G`, `33p C0G`, `180p C0G`
   (a dielectric), `51R 1/4W` (a power rating), `4.7u/25V Ta` (tantalum).
   The suite's `spicelib.parse_value` reads them by ignoring everything
   after the first token. Under D5 as written they are parse errors, so G3
   ("equals the suite's parse on all 29") could not pass. The grammar now
   accepts exactly these qualifiers, each at most once, and keeps them as
   metadata: a tolerance `<x>%`, a voltage rating (`/<v>V` or `<v>V`), a
   power rating `<p>W` (`1/4W`, `0.1W`, `100mW`), and for capacitors a
   dielectric code (`C0G NP0 NPO X5R X6S X7R X7S X7T X8R Y5V Z5U`) or a
   technology (`Ta`, `Al`). Anything else is still an error, never a guess.
   G3 then passes on all 29 (`make dms-values`, and `bbpkg/tests/values.rs`
   on the committed fixture).
2. **Test vectors (D6).** The D6 example names the view's ports nowhere
   ("instantiates the view as X1 (ports by name)"). A vector now maps every
   view port to a circuit node in a `[ports]` table, and the runner writes
   `X1` itself, in the subckt's port order, with the view's `params`
   overridden by the vector's `params` as SPICE numbers. Further keys:
   `lattice` (default 10 µs), `options` (default `phase0`), `sample` (the
   crossing-measurement sampling period, default the lattice) and an edge's
   `initial` level. A crossing is `{ node, when, rise|fall = "<n>", after }`
   (the crossing number as a string, like every other value). Each vector
   runs under the kernel's real `Scheduler` with a scripted edge engine
   (every edge in its trace at its exact time) at one lattice cell and at
   1 ms, and every sample (as `f64` bits) and measurement must be identical.
3. **`bb.lock` for a hand-written board (D8, ADR-007 D9).** M3 implements
   `bb open <board.cir>` only, and it writes only `bb.lock`, next to the
   board file (`board.bbm`, `bb.toml` and KiCad are M4). A board file with
   a `package=` part needs a current lock: `bb run` refuses a missing lock
   or any mismatch, and never writes it. A board file without packages (the
   Phase 0 form) needs none. The lock's `path` is informative: identity is
   the version and the tree hash, so moving the checkout does not change a
   lock.
4. **Platform content hash (D8).** The content hash of a Renode platform is
   the tree-hash list form over the platform's `.repl` and **every** file in
   `PLUGIN_SOURCES`, by file name (`cli/src/platforms.rs`). Since all plugin
   sources are compiled into one assembly (ADR-005 D6), a change to any of
   them changes every Renode platform's hash, including the STM32 ones when
   an nRF model is added. That is accepted: the assembly is shared code
   that decides results. `make locks` rewrites the examples' locks after
   such a change; AGENTS.md says to commit them with it.
5. **Generic passive views (D5).** The `ideal` view of `generic/resistor`,
   `generic/capacitor` and `generic/inductor` is a subckt
   `bb_generic_<kind>` with ports `1`, `2` and a parameter `value` in base
   units (`R1 1 2 {value}`). Its provenance is honest about its origin:
   `source = "agent_generated"`, `confidence = "self_checked"`. Each is
   qualified at (10 µs, `phase0`) by analytic RC or RL step vectors
   (`value` within 0.1 mV at 1 ms and 4 ms after the step, and the 0.5 V
   crossing within 2 µs; the resistor also by a divider within 1 µV).
6. **The STM32 packages (D10).** `st/stm32f103c8` (48 pins) and
   `st/stm32f072rb` (64 pins) list every pin, checked against KiCad
   10.0.8's `MCU_ST_STM32F1:STM32F103C8Tx` and `MCU_ST_STM32F0:STM32F072RBTx`
   (`scripts/check_pins_kicad.py`: 0 mismatches). Their only parameter set,
   `default`, holds the Phase 0 numbers (`source = "estimated"`,
   `confidence = "unchecked"`). The F103 offers no `adc_in` (no ADC model on
   that platform); the F072's ADC pins carry `adc = { channel = "IN<n>" }`,
   which `bb run` now uses instead of its built-in pin-to-channel table.
7. **The `adc_in` parameter set (D4).** `vref` and `bits` are required (the
   Phase 0 model); `model` and `load` are accepted (ADR-008 D7) but `bb run`
   refuses a set that uses them until M8 implements the loading model.
8. **No new crates.** The workspace builds from its vendored registry
   without downloads (setup-time downloads are toolchains only). `bbpkg`
   therefore carries its own TOML 1.0 reader (no dates, which no schema
   uses) and SHA-256 (checked against the FIPS vectors). Schema validation
   in CI uses Python's `tomllib` and Ubuntu's `python3-jsonschema` (4.10.3),
   which `make setup` installs.
9. **G9 (schema bump).** Every schema is at major 0, so there is no real
   previous major. The gate is a synthetic `bb-part/1` registry in the unit
   tests (`bbpkg/src/schema.rs`, `bbpkg/tests/lint.rs`): a `bb-part/0`
   package is read with the migration hint, `migrate` rewrites its schema
   line (format-preserving), and `bb-part/2` is refused with "upgrade bb".
   `bb pkg migrate` runs the same registry code; with today's registry it
   rewrites nothing.
10. **What lint cannot check yet.** "Every pin of the footprint is listed
    once" is checked as "every pin number once": `bb` reads no footprint
    or symbol library in M3 (`scripts/check_pins_kicad.py` does, outside
    `make test`). `mpn_patterns` are compiled by `validate_packages.py`
    (Python's `re`); the Rust resolver that matches them is M4. The
    conflicts report of D9 rule 2 needs migrated views (M5).
11. **Setup (plan M3, R7).** `make setup` builds `bb` and installs it into
    `~/.local/bin`; `make setup SETUP=demo` skips the ARM toolchain, uv and
    `python3-jsonschema`; as root it runs apt without sudo. In a clean
    `ubuntu:24.04` container (podman) from `git archive 590c455`, `apt
    install make` plus `make setup SETUP=demo` took 63.6 s, and 66.0 s in a
    second run (plan, Review notes: M3).

## Amendment 2 (2026-11-09): S1, the TPS22917 view's numerics fix

Plan S1 is done **without the fallback**. A view-local fix of the suite's
TPS22917 view (`load_switches__tps22917.lib` at `6854d25`) lets the E1 deck
run 120 ms through `NgspiceEngine` at `phase0` on both benches. The fixed
view is TI-derived, so it stays **local-only** (approver decision A3). Nothing
TI-derived is in the repository: `make dms-s1` (`scripts/dms_s1.sh`) exports
the reference with `git archive`, writes the fixed view to
`~/.cache/bb-dms-s1/local-packages/ti/tps22917/` (a local package path
outside the repository, D9 rule 4) and runs the gates there.

1. **Cause (measured).** Context 3's stalls are not step-size (LTE)
   limits. A step trace inside the engine (a scratch copy with a
   `GetSyncData` log) shows ngspice accepting a step, doubling it twice,
   failing Newton on the next one and cutting it by 8, over and over. It
   cycles at 5e-14 to 2e-13 s during the LED_PWR_EN fall (t = 15.60005 ms,
   inside the 100 ns glue ramp) and at 2e-16 to 1.7e-15 s at the second
   P0.06 rise of a 120 ms run. Phase 0's `ABSTOL = 1e-13` is the Newton
   tolerance of every **branch-current unknown**. In this view, TI's
   `RVAR U4` (a B voltage source R·I, a 0 V ammeter and a current-copy CCCS)
   and TI's current sense `H1` (a 0 V ammeter and a CCVS) put branch
   unknowns in series with the pass path. There, 0.1 mΩ (`S10`) to about
   50 mΩ (`RVAR`) separate nodes at about 3 V. Converging such a branch
   current to 1e-13 A + 1e-5·|i| (with the ring's ~50 µA quiescent current)
   needs those node voltages to about 1e-14 V, which is the resolution of a
   double at 3 V. At the stall the pass-path nodes alternate by 2.4 µV
   between accepted points.

   Supporting measurements (`~/bbscratch-s1`):
   - With ABSTOL 1e-12 instead of 1e-13, the fall passes.
   - Replacing every XSPICE `aswitch` of the view by an equivalent B-source
     does not help.
   - Rewriting the logic blocks' Thevenin outputs as Norton sources does
     not help on its own.
   - The suite's view did not stop for good at the fall. With a 150 s cap
     it crawled through the fall, reached 51 ms (just past the second
     turn-on) after 112 s, and then made no further progress before the
     cap. Context 3's 120 s cap ended inside the crawl.
2. **The fix** is the only change to the view, and M5 lists it under the
   view's `deviations` (D9 rule 1):
   - `RVAR U4` becomes one B conductance from VIN to the next pass-path
     node: I = V/R, where R = V(`E4`) (TI's resistance table plus `ron_add`),
     floored at 1 µΩ. The floor only matters below VIN ≈ 5 mV, where the
     table gives less than 1 µΩ.
   - `H1` loses its 0 V ammeter. Its two nodes are merged (a 0 V source is
     a short), and its 1 Ω transresistance output `I_REVERSE` is computed
     from the same branch current (VOUT side to VIN through `RVAR`).

   The I-V is the same and nothing else changes. Input sha256 `b487750f…`
   (the file at `6854d25`, identical at `dc96515`). Output sha256
   `061285e8…` (the input plus a local-only header and the two
   replacements). The replacement lines are breadboard.dev's own text, in
   `scripts/dms_s1.py`.
3. **What `make dms-s1` measures** (18 gates, all pass; `out/dms-s1/summary.json`).

   **The deck.** The suite's own `led_rail_sequence` deck, built by its
   functions at `6854d25`, with the power path replaced by each plan §2
   bench: supply, series R, 50 mA, tracking +3V3. It runs two on/off
   cycles of firmware.md §6.4 to 120 ms:
   - LED_PWR_EN high at 0.5 ms;
   - P0.06 high from 10.5 to 15.5 ms;
   - LED_PWR_EN low at 15.6 ms;
   - the same again from 50.6 ms.

   **How it runs.** The control edges are 100 ns digital glue through
   `engines/analog-ngspice/examples/deck_run.rs`: lattice 10 µs, TMAX
   10 µs, grants of 1 ms, and every edge delivered through
   `Engine::apply_input_trace`.

   | Gate (each bench) | nominal | cold |
   | --- | --- | --- |
   | 120 ms at `phase0` (solver points) | 19 689 | 19 967 |
   | Grants of 10 µs, 1 ms and 10 ms: samples byte-identical | yes | yes |
   | VLED at the check instants (each P0.06 rise, each QOD end) vs the suite's `suite_fallback` run (its view, `tran 20u`), ≤ 1 mV | 0.0003 mV | 0.0004 mV |
   | LSW-2 time (VLED < 10 % after LED_PWR_EN falls) vs the same run, ≤ 10 µs | 0.026 µs (23.8659 ms) | 0.025 µs (24.0641 ms) |
   | VLED at every 10 µs vs the converged `suite_fallback` run (its view, TMAX 1 µs), ≤ 1 mV | 0.52 mV | 0.59 mV |
   | The fix changes no result: converged runs (TMAX 1 µs) of the fixed view at `phase0` and of the suite's view at `suite_fallback`, ≤ 0.1 mV | 0.068 mV | 0.038 mV |

   The suite's two E1 corners (bat3.0 −10 °C, bat4.2 25 °C, its own
   battery network) also run 120 ms. VLED at 10.5 ms and 50.6 ms is
   2.971511 / 0.1043265 V and 4.184020 / 0.1428753 V. M0's `results.json`
   has 2.971511 / 0.1043262 V and 4.18402 / 0.1428747 V.

   **The view's own suite vectors (reported here; M5 qualifies them).**
   U2 QOD at bat3.0, bat4.2 and usb5.0, and Table 7.6 at 3.6 V and 5 V. The
   QOD deck at reltol 1e-5 also failed before (Context 3). They now run at
   `phase0` through the engine and reproduce `results.json`'s t_off10, tR
   and tON within 4 ns.
4. **How the plan's S1 gate is applied** ("VLED within 1 mV of the suite's
   fallback-option result"), stated so it is not a silent change. The
   suite's `suite_fallback` run is not a 1 mV reference over the whole
   waveform:
   - On the turn-on ramp's knee it moves by up to 2.4 mV when the PWL edge
     times change by about one ulp (the same deck with times written in
     seconds or in `e-15` units).
   - Its TMAX-20 µs and TMAX-10 µs runs are 1.3–5.5 mV and 0.5–1.8 mV from
     the converged solution there (both benches; summary.json
     `max_dVLED_mV`). The engine's distance to the TMAX-10 µs run is
     reported: 0.52 mV nominal, 1.19 mV cold.

   So the 1 mV is gated two ways:
   - **At the instants the suite measures**, against the suite's run as the
     suite runs it.
   - **Over the whole waveform**, against the converged run of the suite's
     own view at the suite's fallback option set.

   Both are at least as strict as the plan's wording intends. Neither one
   loosens a tolerance.
5. **Consequences.**
   - D6 "Gate (S1/M5)": the S1 half holds for E1. M5 migrates the fixed
     view as `ti/tps22917`'s `lsw` view (local-only), with the deviation in
     item 2. Its D9 rule 1 comparison ("results within tolerance" for a
     numerics fix) is item 3's view-neutrality row. `bb pkg test` (D6,
     Amendment 1 item 2) runs the qualification itself.
   - The R1 fallback (a datasheet-fitted view *because* no fix works) is
     not needed. M5b stays required by approver decision A3 (licensing),
     with its gate unchanged. Its comparison with the vendor-derived view
     (10 mV / 1 ms on the E1 checks) can now run both views through the
     engine at `phase0`, locally.
   - The suite's other TPS22917 files (`oled_boost__`, `power_integrity__`,
     `sleep_current__`) carry the same `RVAR`/`H1` blocks, so they need the
     same fix when a scenario migrates them (M8 and later).
   - **A rule for view authors** (AGENTS.md, Known bad ideas): no
     branch-current unknown in series with a path of a few milliohms or
     less in a view qualified at `phase0`. That covers 0 V ammeters, CCVS
     and CCCS sense sources, and V-type B or E sources. Use a B conductance
     instead, and compute a sensed current from node voltages.
6. **Cost.** Plan §8 has the measured table (`make dms-s1-cost`).

## Amendment 3 (2026-11-09): M5 and M5b, the E1 packages

Plan M5 and M5b (required by approver decision A3) are implemented. Four
first-party packages and one LOCAL-ONLY package exist; every E1 view is
lattice-qualified at (10 µs, `phase0`) through `NgspiceEngine` at both
partitions, and its vectors reproduce M0's `results.json` within the stated
tolerance (G4). Where the implementation settled something this ADR left
open, or differs from its text, it is recorded here. The kernel, the
time-grant protocol and the event stream are unchanged; `spec/package.md`
gains the module rules (section 4), `bb pkg test --report` (section 8), the
conflict warning (section 11) and the `nrf52840-bb@1` platform row.

1. **The packages.**
   - `ti/tps22917` 0.1.0 (Apache-2.0, kind `power`): one view, `lsw`,
     **datasheet-fitted** (M5b; `analog/lsw_fit.lib`, subckt
     `TPS22917_LSW_FIT`). It is breadboard.dev's own behavioural model from
     SLVSDW8B Tables 7.3, 7.5 and 7.6 and contains no text of TI's SLVMCE8
     or of the suite's view: an ON comparator with hysteresis (0.68 V /
     0.63 V, inside the guaranteed window) and a 2 µs enable RC; a CT sink
     of VIN/tONk, so that the CT-to-VIN capacitance (external CT plus a
     32 pF internal one) sweeps in exactly tON; a linear VOUT target through
     10 % at tON - tR and 90 % at tON; a forward-only pass element that
     follows the target (20 S) and is RON (Table 7.5 typ, linear in VIN)
     once the target passes VIN; 150 Ohm QOD while disabled; a 750 kOhm ON
     pull-down. Every element is a B current source or a plain R or C, so
     the view follows Amendment 2's rule for `phase0`. It keeps the suite
     view's port order, parameters (`ron_add`, `isd_out`) and view name, so
     a project that puts the local package on its search path swaps views
     with no other change. `fidelity = ["fast"]`: it is not validated for
     `pi`. Measured in the datasheet test circuit through the engine: tON
     3.818 / 4.020 ms and tR 1.613 / 1.815 ms at 3.6 / 5 V (Table 7.6: 3.8 /
     4.0 and 1.6 / 1.8 ms), RON 90.0 / 80.0 mOhm, QOD 150 Ohm.
   - `worldsemi/ws2812c-2020` 0.1.0: one view, `pixel`, the suite's
     `load_switches__ws2812c.lib` **copied unchanged** (D9 rule 1; no port is
     named `gnd`, so there is no deviation; sha256 `2df4a820...`, origin
     `dial-me-spinner@6854d25:hardware/sim/models/load_switches__ws2812c.lib`).
     The suite's `din` and `ring` files stay in dial-me-spinner (D9 rule 3:
     E1 uses only the pixel). Pins as KiCad's `LED:WS2812B-2020` names them
     (`DOUT`, `VSS`, `DIN`, `VDD`), the names a netlist carries.
   - `nordic/nrf52840-qiaa` 0.1.0 (kind `soc`): `[behavioral]` Renode
     `nrf52840-bb@1`; 74 pins (73 aQFN73 balls and the exposed pad) as KiCad
     10.0.8's `MCU_Nordic:nRF52840` numbers them, checked with
     `check_pins_kicad.py --composite` (0 mismatches, 18 composite names).
     **No analog view** (item 5a).
   - `raytac/mdbt50q-u1mv2` 0.1.0 (kind `module`): 61 pads as the
     dial-me-spinner project symbol `MDBT50Q-U1MV2_byport` gives them (0
     mismatches), mapped onto `nordic/nrf52840-qiaa` by `module-pinmap.csv`
     (item 5c).
2. **The LOCAL-ONLY vendor-derived `ti/tps22917`** (D9 rule 4, approver
   decision A3). `make dms-local-packages` (`scripts/dms_packages.sh local`)
   exports the reference with `git archive`, applies Amendment 2's S1 fix and
   the `GND` -> `GNDP` port rename (D9 rule 1) to the suite's view, and
   writes a package with the same id, view name, ports and parameters,
   `license = "LicenseRef-local-only"`, into the git-ignored
   `local-packages/` (`DMS_LOCAL_PACKAGES`; the script refuses a path that
   git tracks or does not ignore). Its view's sha256 is `f754ea77...`. It is
   linted (clean, outside the first-party `packages/`) and qualified there
   by `bb pkg test --record`; its nine vectors (the committed package's
   circuits, item 3) reproduce `results.json` within 2 us (Table 7.6
   crossings; measured within 0.5 ns), 50 uV (RON), 10 uV (QOD resistance)
   and 10 us (QOD discharge; measured within 10 ns). **G8:** the same package
   with the port left as `GND` gives byte-identical `bb pkg test --report`
   lines (every measured value and a sha256 of every sample; 18 vector
   runs). `scripts/check_local_only.sh` (in `make packages`, so in `make
   test`) fails if a tracked `part.toml` is `LicenseRef-local-only`, if git
   tracks anything under `local-packages/` or stops ignoring it, or if a
   tracked model file has the bytes of the suite's TPS22917 view, its
   S1-fixed form or the local view.
3. **The vectors (D6, D9 rule 5).** `scripts/dms_packages.py` builds every
   circuit with the suite's own deck builders at `6854d25`
   (`test_load_switches.py`: `test_model_turn_on_matches_table_7_6`,
   `u2_turn_off`, `phantom_op`), cuts the part under test out (the runner
   writes it as `X1`) and turns its PWL control sources into `[[edge]]`
   inputs. The RON and QOD-resistance circuits are switches A and B of
   `test_model_ron_qod_and_on_threshold`, written out. The circuits hold
   only the suite's own, non-TI models (the BQ25180 power path, the nRF52840
   GPIO pad, the WS2812C pixel, the board's passives), which approver
   decision A2 lets the repository vendor. `make dms-vectors-check`
   regenerates them and requires the committed files byte for byte (and
   item 5a's sets). Expected values are `results.json`'s, with the source
   named in every `[[expect]]`. Settled here:
   - **A crossing level is fixed in the vector.** The suite measures the QOD
     discharge time to 10 % of `vled0`, its own run's VLED at 0.99 t_off;
     the generator takes `vled0` from the suite's own run of the same deck
     (2.984022 / 4.18402 / 4.485772 V) and writes the levels as numbers.
   - **The committed view states two tolerances.** Each TPS22917 vector has
     datasheet expectations (Table 7.6 crossings within 40 us, RON within
     0.1 mV, QOD within 10 uV) and `results.json` expectations whose
     tolerance is the fitted view's stated distance to the vendor-derived
     values: 0.2 ms on the Table 7.6 crossings (the vendor-derived view runs
     3.7 % slower than Table 7.6 typ), 3 mV on RON (77 against 90 mOhm),
     0.5 mV on QOD (145 against 150 Ohm), and 1 ms on the discharge time (the
     M5b tolerance). Measured: Table 7.6 crossings 0.12-0.16 ms from
     `results.json`, the discharge crossings within 0.56 ms.
   - **The WS2812C vectors stand U2 in as its off state.** The suite's
     phantom deck (U2 off, P0.06 driven high into D101's DIN clamp) needs
     U2, and no TI-derived text may be committed, so U2 is its off-state
     QOD resistance, 145.001 Ohm (the vendor-derived view's QOD,
     `results.json` `test_model_ron_qod_and_on_threshold`, at VIN >= 3.5 V;
     hence only the bat4.2 and usb5.0 corners), plus CT held at VIN (without
     it the CT node has no DC path and the operating point fails). The
     phantom VLED and DIN - VDD reproduce `results.json` within 5 uV.
4. **Plan M5b's gate** (`make dms-m5b`, 20 gates, `out/dms-m5b/summary.json`).
   The E1 deck of Amendment 2 (the suite's `led_rail_sequence` with each
   plan section 2 bench, two firmware.md 6.4 cycles, 120 ms) and a
   wrong-order variant (P0.06 high from 0.5 ms with VLED off, through each
   turn-on and 10 ms into each QOD discharge) run through `NgspiceEngine` at
   `phase0` (lattice 10 us, 1 ms grants, edges as digital glue) with each
   view; the vendor-derived view comes from the local package:

   | Check (limit) | nominal | cold |
   | --- | --- | --- |
   | 120 ms at `phase0`, both views, both orders | yes | yes |
   | LED-DATA-V4: VLED at each P0.06 rise (10 mV) | 0.001 mV (3.684020 V) | 0.001 mV (2.971511 V) |
   | LSW-3: max(V(LED_DIN) - V(VLED)) (10 mV) | 0.0000 mV | 0.0000 mV |
   | LSW-2: VLED < 10 % after LED_PWR_EN falls (1 ms) | 0.555 ms (23.866 / 24.420 ms) | 0.356 ms (24.064 / 24.420 ms) |
   | The E1 verdicts identical | yes | yes |
   | Wrong order: maximum excursion (10 mV); LSW-3 fails with both | 0.000 mV (0.8176 V, 9.97 mA) | 0.000 mV (0.8517 V, 8.34 mA) |
   | Wrong order: settled DIN - VLED at 10.49 ms (10 mV) | 0.416 mV | 0.227 mV |

   The LSW-2 difference is the QOD resistance (Table 7.5's 150 Ohm typ
   against the vendor-derived view's 145-147 Ohm, through R4 = 68 Ohm). The
   wrong-order VLED before turn-on differs by 4-8 mV for the same reason
   (0.965 against 0.973 V nominal); the excursion by under 0.5 mV.
5. **Deviations and settlements.**
   - a. **The nRF52840 GPIO "views" are pin parameter sets.** D9's table
     lists `gpio_led`, `gpio_lsw` and others as analog views. Those suite
     files are the pad's output DRIVER. In a co-simulation the pin model is
     the driver, and a circuit view of the same pad would put a second
     driver on the net (one driver per net, AGENTS.md). So the package has
     no analog view, and the suite's numbers became named `digital_out` sets
     in `pins.toml`: `S0S1` (and `default`), `S0S1_min`, `S0S1_max`,
     `S0S1_bound`, `H0H1`, `H0H1_min`, `H0H1_max`, with zout = 0.4 V / I at
     the PS columns. D9 rule 2's "200 Ohm against 800 Ohm" conflict is two
     PS figures for two purposes (a current at a 0.4 V drop; the VOH
     guarantee at 0.5 mA), so both are kept, by name, never averaged. `make
     dms-vectors-check` requires the sets to equal the suite's `ROUT`
     values. The pad's ESD clamps and 3 pF are not modelled at the pin (a
     `pin_load` view could carry them; `bb run` refuses `load` until M8).
     Their use as E1's drive is gated in M6 (the PWL replay within 5 mV).
   - b. **The nRF52840's composite KiCad names.** A GPIO is named by its
     port name (`P0.02`, the name ADR-008 D2 binds); KiCad's `AIN0/P0.02`
     keeps `AIN0` in `alt`. ADR-007 D3's exact pin-name match would refuse a
     board that places the bare chip. No reference board does (U10 is the
     module), so matching composite names on import is deferred.
   - c. **Modules** (`spec/package.md` section 4, new). The pinmap is a CSV
     `pad,chip_pin`. The pads carry no parameter sets and the module no
     `[behavioral]`: both come from the chip. Resolution adds a module's
     chip package to the resolved set, so `bb.lock` and `board.bbm` pin it
     and its platform (`renode:nrf52840-bb@1` on the reference board).
     Binding a module pad in `bb run` (through the pinmap to the chip pin's
     sets) is plan M6's work. Raytac's own pad table was not checked
     offline; the board's symbol and footprint decide the pads.
   - d. **The conflict report** (D9 rule 2, Amendment 1 item 10): lint warns
     when one parameter has different defaults in two views of a package.
     None of the M5 packages has two views.
   - e. **`bb pkg test --report <file>`**, for G8 and for comparing two
     packages' results byte for byte.
   - f. The committed TPS22917's kind is `power` (the format example in
     `spec/examples` says `analog_ic`).
6. **Resolution of the reference board.** With these packages, `bb open`
   resolves U2 and U21 (`TPS22917DBVR`), D101-D148 (`WS2812C-2020-V6`) and
   U10 (`MDBT50Q-U1MV2`) by exact MPN: stages generic 118, mpn 51, no_model
   12, override 1, unresolved 19 (70 at M4). ADR-007 Amendment 2 restates
   gate M4f; `examples/dial-me-spinner/board.bbm` and `bb.lock` are
   regenerated (`make locks`).
7. **Cost.** `bb pkg test` of the four new packages (11 vectors, two
   partitions each) takes about 6 s, and `make packages` about 22 s in all
   (load 6-10 from other agents). The QOD vectors dominate (5 000 one-cell
   grants each). There is no new board or run mode, so no equivalence gate
   or cost-table row of its own; M6 adds both for E1.

## Amendment 4 (2026-11-09): plan M6, the packages in E1

No package changes in M6; what E1 exercised:

1. **Module pads bound in `bb run`** (Amendment 3 item 5c, `spec/package.md`
   section 4, ADR-007 Amendment 3 item 1). A binding on a pad of
   `raytac/mdbt50q-u1mv2` takes its pin model and parameter set from the
   `nordic/nrf52840-qiaa` pin the pinmap maps it to, and a `voh = "VDD"`
   supply reference resolves through the pinmap to the module's VDD pad
   and its net. The module's own `pins.toml` `models` lists are not
   consulted; the chip pin's are (they agree for every GPIO pad).
2. **nRF52840 drive-set names** (`spec/package.md` section 4). The names
   of Amendment 3 item 5a's sets already begin with the `PIN_CNF` DRIVE
   they stand for (`S0S1`, `H0H1_max`, ...). M6 makes that the rule
   `bb run`'s `PIN_CNF` audit reads (ADR-008 Amendment 3), with `default`
   standing for S0S1. No set was renamed.
3. **Item 5a's gate.** "Their use as E1's drive is gated in M6 (the PWL
   replay within 5 mV)": E1 binds LED_PWR_EN with `S0S1` (200 Ohm, 13 ns)
   and P0.06 with `H0H1_max` (28.6 Ohm, 5 ns). The replay into the suite's
   deck, whose pads are the suite's own `LSW_NRF_GPIO` (hd_typ on
   LED_PWR_EN, hd_max on P0.06, ESD clamps, 3 pF, VDD from +3V3), agrees
   with the co-simulation on VLED within 0.223 mV (nominal) and 0.489 mV
   (cold) with the vendor-derived TPS22917 view (ADR-007 Amendment 3 item
   7). PASS.
4. **Board-level convergence** (D6, "each example board gets a convergence
   gate at its bench's option set"). The 132-instance E1 deck runs 120 ms
   at `phase0`, at both engine orders, in 3.0-6.2 s. Getting there took one
   change outside the packages: the bench's tracking +3V3 source in Norton
   form (ADR-007 Amendment 3 item 4). Every view was already qualified at
   (10 us, `phase0`), and `bb run` checked that.
