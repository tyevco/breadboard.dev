# ADR-006: Component packages v0 (part.toml, pins.toml, model views, local packages and lockfile)

- **Status:** Accepted (2026-11-09), with the approver decisions below
  (revised 2026-11-09 after two design reviews, see Review notes).
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
