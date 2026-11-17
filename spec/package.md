# Component packages (`part.toml`, `pins.toml`, views, tests, `bb.lock`)

Status: normative. Decided by ADR-006 (accepted 2026-11-09, approver
decisions A-C; Amendment 1 for the KiCad value qualifiers; Amendment 3 for
modules, the view conflict report and `bb pkg test --report`; Amendment 6
for device parts in board files and device identities, approver decisions
2026-11-14). ADR-015 D6 and D7 (Phase 2 H4, Amendment 3): platform hashes
over the declared plugin sources, and the board-file package path. The
Phase 2 schema epoch (milestone S; ADR-009 to ADR-013, ADR-011 Amendment 1):
`bb-part/1`, `bb-pins/1` and `bb-lock/1`, the view-name rule and the relock.
Registries (milestone E1, ADR-011 Amendment 2): `registry+` sources, vendor
files, the licence gate (section 14 and `spec/registry.md`).
Implementation:
the `bbpkg` crate (`bbpkg/src`), `bb pkg`, `bb open` and `bb run` (`cli/src`).
JSON Schemas: `spec/schema/part.schema.json`, `pins.schema.json`,
`lock.schema.json`, `pkgtest.schema.json` (JSON Schema 2020-12; the TOML
files are converted to JSON as they are and validated, `make packages`).
**Changing this file requires an ADR.**

A package describes one part: what it is (`part.toml`), its pins and their
pin models (`pins.toml`, `spec/pin-model.md`), its analog views (SPICE
subcircuits with provenance), its behavioural layer (the CPU engine platform
it runs on), and the test vectors that qualify its views. Packages are local
directories, found on the search path, or archives fetched from a signed
registry (section 14, `spec/registry.md`); `bb.lock` pins their content by
tree hash, whatever the source.

## 1. Layout and search path

```
packages/                         first-party packages, in breadboard.dev
  generic/resistor/               package id "generic/resistor"
    part.toml
    pins.toml
    analog/resistor.lib           view files
    tests/                        test vectors (section 8)
  st/stm32f103c8/
```

- **Package id:** `<namespace>/<name>`, lower case `[a-z0-9._-]`. The
  namespace is the manufacturer, or `generic` for generic parts.
- **One directory is one package version.** The version is in `part.toml`,
  not in the path. A directory containing `part.toml` is a package; its
  subdirectories are not searched for further packages.
- **Search path:** the directories of a project's `bb.toml` `[packages]
  paths` (relative to the project, in order; ADR-007), then breadboard.dev's
  first-party `packages/`. The first directory that provides an id wins, so a
  private package shadows a first-party one. Two packages with one id in one
  directory are an error. A hand-written `board.cir` has no `bb.toml`: its
  search path is the directories of its `*bb packages path=<dir>` lines
  (section 12; ADR-015 D7), then the first-party directory. `bb` finds that
  directory through the path of the checkout it was built from, or
  `$BB_FIRST_PARTY_PACKAGES`.
- **Then the registries** (ADR-011 D4, `spec/registry.md` section 6): an id
  that no local directory provides is resolved on a registry (`bb.toml`'s
  routed `[registries.<name>]`, else the public one) by `bb open`, which
  fetches it into the cache; `bb run` reads a locked registry package from
  the cache only. Local sources always come first, so an edited
  first-party package is used over its published copy, and `make test`
  never needs the network.

## 2. `part.toml` (`bb-part/1`)

TOML is the authored form; `spec/schema/part.schema.json` is canonical.
**Unknown keys are an error.** `bb-part/1` (the Phase 2 schema epoch) keeps
every `bb-part/0` key and its meaning; a `bb-part/0` file is the previous
major, read with the migration hint, and holds none of the `bb-part/1`
keys (refused there with "new in bb-part/1"). Keys still reserved are
refused with "reserved for a later milestone": top-level `layers`,
`thermal`, `rtl`; `[pins] ibis`; `[behavioral] cpu`. A reserved key
becomes accepted within the major when an ADR gives it a meaning
(`[behavioral] svd`: milestone E1, ADR-011 Amendment 2).

**The epoch's keys are accepted, checked and inert** until their
milestones use them: no `fast` deck, output or `run.json` depends on them.

| Key | Required | Meaning |
| --- | --- | --- |
| `schema` | yes | `"bb-part/1"` |
| `[part] id` | yes | the package id |
| `[part] version` | yes | `MAJOR.MINOR.PATCH` (section 10 says when to bump) |
| `[part] kind` | yes | `soc`, `module`, `power`, `analog_ic`, `logic`, `discrete`, `passive`, `connector`, `mechanical`, `testpoint` |
| `[part] manufacturer` | no | text |
| `[part] mpn` | no | exact manufacturer part numbers (resolution, ADR-007 D4) |
| `[part] mpn_patterns` | no | regular expressions matched against the **whole** normalised MPN (anchored implicitly; written without `^`/`$`) |
| `[part] lcsc` | no | LCSC part numbers (an extra lookup key) |
| `[part] datasheet` | no | `{ url, rev }` |
| `[part] license` | yes | an SPDX expression; `LicenseRef-local-only` forbids committing the package to a shared repository and is refused inside breadboard.dev's `packages/` |
| `[part] description` | no | text |
| `[footprints] kicad` | no | footprint library ids (informative; generic parts are selected by footprint library, ADR-007 D4) |
| `[footprints] pinmap` | no | a file mapping footprint pads to pin names, when they differ |
| `[pins] file` | yes | the `pins.toml` file |
| `[module] chip`, `pinmap` | for `kind = "module"` only | the chip package inside, and a module pad to chip pin map |
| `[behavioral]` | no | at most one behavioural layer (section 3) |
| `[behavioral] svd` | no | `bb-part/1` (ADR-011 D5, D7; Amendment 2): `{ url, sha256, license }`, the vendor SVD of the part, an `https://` url, its sha256 (64 lower-case hex digits) and its licence (one SPDX id, `LicenseRef-` included). It is a reference only, never part of the package: `bb open` fetches it at use and locks it as a `[[vendor_file]]` (section 9, `spec/registry.md` section 8) |
| `[[analog]]` | no | analog views (section 3) |
| `[provenance]` | no | the package-level default for views without their own block |
| `[tests] dir` | no | the test vector directory (section 8) |
| `[power]` | no | `bb-part/1` (ADR-009 D4, D5; used from milestones P2 and P3): `idd = { <state> = { typ, max, ref } }`, the supply current per CPU state (a state name is lower case `[a-z0-9_]`; `typ` and `max` currents, `max >= typ`; `ref` the source); `vdd_min` (a voltage, the operating minimum); `io_rail` (a `power_in`/`power_out` pin, the default `rail` of a `pi` `digital_out`) |
| `[[document]]` | no | `bb-part/1` (ADR-012 D3; used from milestone E2): `id` (lower case `[a-z0-9._-]`, unique), `sha256` (64 lower-case hex digits: the document's identity), and optional `title`, `revision`, `url` (informative) and `fetched_by` (`agent:<who>` or `user:<who>`). The PDF is never committed |
| `[claims] file` | no | `bb-part/1` (ADR-012 D4): the package's `claims.toml` (`bb-claims/0`, milestone E2; this build checks only that the file exists) |

**Provenance** (`[analog.provenance]` or `[provenance]`): `source`
(`datasheet`, `vendor_model`, `vendor_derived`, `measured`, `estimated`,
`agent_generated`; required), `base`, `refs`, `deviations`,
`not_modelled`, `origin` (`<repo>@<commit>:<path>` of a migrated file),
`generated_by`, `reviewed_by`, `upstream_license` (`bb-part/1`, ADR-011 D7:
the SPDX licence of the upstream model a `vendor_derived` view comes from;
publishing checks it, milestone E1), and `confidence` (required), an enum, not a
number: `unchecked`; `self_checked` (passes its own vectors);
`datasheet_checked` (the vectors are datasheet table values); `measured`
(the vectors are bench measurements). A view whose source is `estimated`
or `agent_generated` is flagged in `bb run`'s summary (ADR-007 D6, M4).
`source` says how the view was made, not where its numbers come from: a
view an agent wrote is `agent_generated` even when every number is a
datasheet value (`base` and `refs` name those, `generated_by` the agent),
so `bb run` flags it until a person reviews it (`reviewed_by`). A figure
taken from a secondary source (a document quoting the datasheet) names
that source in `refs`, and `datasheet_checked` needs the vectors checked
against the datasheet itself (ADR-006 Amendment 5).

## 3. Layers, views and the fidelity ladder

Phase 1 implements three layers.

- **`pins`** (always): each pin's role and pin models (section 4).
- **`behavioral`** (at most one):
  - `engine = "renode"` with `platform = "<name>@<major>"`: the part is the
    CPU engine's machine, on a platform identity that `bb` provides
    (section 9). Packages do not ship `.repl` or C# files in Phase 1.
  - `engine = "device"` with `device = "<id>@<major>"`: a pin-level device
    compiled into `bb` (ADR-006 D7).
  - Other engine ids are refused until an ADR defines them.
- **`analog`** (zero or more named views). A view:

  | Key | Meaning |
  | --- | --- |
  | `name` | unique in the package |
  | `kind` | `circuit` (default), `pin_load`, or (`bb-part/1`) `switch`, `supply_load` or `pof_comparator` |
  | `purpose` | text |
  | `fidelity` | the rungs it is valid for (`fast`, `pi`, `si`, `em`; at least one) |
  | `default_for` | the rungs it is the default for; a subset of `fidelity`; at most one view per package may name a rung |
  | `file` | the view file (SPICE), relative to the package (none for `switch`) |
  | `subckt` | the subcircuit the file defines (none for `switch`) |
  | `ports` | `circuit`: the subckt's ports in order, as pin names; `pin_load`: typed ports (section 4); `switch`: its two contacts, `{ role = "pin", pin = "<name>" }`; `supply_load`: `{ role = "pin", pin }` and `{ role = "state", state }`; `pof_comparator`: `{ role = "pin", pin }`, `{ role = "state", state = "pofthr0" }` .. `pofthr3` and `{ role = "observe", signal = "pofwarn" }` |
  | `params` | `{ name = "<value>" }` defaults, in the value grammar (section 6) |
  | `rails` | `bb-part/1` (ADR-009): the view's supply ports, informative, for lint: each a port of the view on a `power_in`/`power_out` pin |
  | `[[analog.corners]]` | `bb-part/1` (ADR-009 D8; a bench selects one from milestone P1): `name` (`typ`, `min`, `max`, `suite_compat` or `worst_<x>`, unique in the view), `params` (overrides of the view's own `params`, in the value grammar) and `ref` (required: where the corner's values come from) |
  | `[[analog.qualified]]` | `{ lattice, options }` pairs the view is qualified for, written by `bb pkg test --record` (section 8) |
  | `provenance` | section 2 |

**The `supply_load` view kind** (`bb-part/1`, ADR-009 D4; in the deck
since milestone P3, Amendment 6). The current a part draws from its supply
pins as a function of CPU state outputs: `I = Σ s_k · I_k`, each `s_k` the
node of the state port `#<state>` (0..1), `I_k` from `[power] idd`. Its
ports are the package's supply and ground pins (`{ role = "pin", pin }`,
at least one) and its state ports (`{ role = "state", state = "<name>" }`,
the node `#<name>`; each needs a `[power] idd.<name>` entry). It has a
`file` and a `subckt` like a `circuit` view. It enters the deck at `pi`
beside the instance's chosen view (`spec/pin-model.md` glue rule 12), so it
is never valid at `fast` and never a rung's default (`default_for` is
empty). Its instance parameters are `i_<state>` = `[power] idd.<state>.typ`
for each state port and `vdd_min` = `[power] vdd_min`, then the view's own
`params` (which may restate or override them by name), then a test
vector's; the subckt declares them. In a test vector a state port is keyed
by its state name.

**The `pof_comparator` view kind** (`bb-part/1`, ADR-009 D6; milestone
P4, Amendment 8). A CPU's power-fail comparator: its output rises where
the supply it compares falls to the threshold its CPU engine reports and
falls where it rises above the threshold plus the hysteresis. Its ports are
the package's supply and ground pins (`{ role = "pin", pin }`, at least one
supply), exactly the four state ports `#pofthr0..3` (`{ role = "state",
state = "pofthr<k>" }`, the bits of the threshold code, nodes in 0..1),
exactly one observe port, its output (`{ role = "observe", signal =
"pofwarn" }`, ground-referenced), and (milestone P5, ADR-009 D7, Amendment
9) at most one more observe port, the brownout reset detector's output
(`{ role = "observe", signal = "bor" }`, ground-referenced, in the same
form: 1 V where the supply falls to VBOR, 0 V at the release), which the
CPU engine's watched input `#bor` reads. It has a `file` and a `subckt`. It enters
the deck at `pi` beside the CPU target's views (`spec/pin-model.md` glue
rule 13), so it is never valid at `fast` and never a rung's default. The
output's levels are fixed by the glue's `digital_in` (`vih` 1 V, `vil`
0 V): the view's output reaches 1 V at the trip and 0 V at the release,
and the CPU engine's watched input `#pofwarn` reads it. Its corners (`typ`,
`min`, `max`: the threshold's tolerance) are the CPU target instance's
(`[corners]`, or the bench's `corner`). In a test vector a state port is
keyed by its state name and the outputs by `pofwarn` and `bor`.

**The `switch` view kind** (`bb-part/1`, ADR-010 D2). A contact that a
bench `[[action]] press` closes (`spec/project.md` section 3). `bb`
generates its element, so the view has no `file` or `subckt`:

- every switch instance is `X<ref> <contact 1> <contact 2> bb_ctl_<ref>
  bb_switch ron=<ron> roff=<roff>`, and the definition, emitted once with
  the other view definitions, is one B conductance with no branch-current
  unknown:

  ```spice
  .subckt bb_switch a b ctl ron=0.05 roff=1e8
  B1 a b I = (V(a) - V(b)) * (1 / {roff} + V(ctl) * (1 / {ron} - 1 / {roff}))
  .ends bb_switch
  ```

- `bb_ctl_<ref>` is the control node: 0 V open, 1 V closed. Its source
  `V_bb_ctl_<ref>` is `DC 0` (open) when no action presses the instance,
  else an `EXTERNAL` source the bench drives;
- `params`: `ron` and `roff` (resistances, defaults `50m` and `100Meg`, so
  both states keep a DC conductance; `0 < ron < roff`) and an optional
  `bounce_max` (a time: the longest bounce a press of it may ask for);
- a test vector maps the two contacts and `ctl` (the control node, driven
  by an `[[edge]]` of 0 V / 1 V) to nodes, and qualifies the view like any
  other (section 8).

**Selection, with the ladder** `fast < pi < si < em`. For a run at fidelity
`F` an instance uses: the view named by the project's per-instance override;
else the view whose `default_for` contains `F`; else, walking **down** the
ladder, the view whose `default_for` contains the first lower rung that has
one; else **no analog model** (the part is listed as unmodelled; never a
silent default). A view never moves up the ladder. `run.json` records the
requested rung, the view used and the rung it was chosen at (ADR-007 D6).

**View files.** A view file defines; it does not instantiate. At its top
level it may hold only comments, `.subckt` ... `.ends` blocks, `.model`,
`.func` and `.param` lines (with `+` continuations). It is self-contained:
no `.include` or `.lib`.

**One deck from several views.** Views are inlined into one SPICE deck.
Every top-level name a view defines (`.subckt`, `.model`, `.func`, `.param`
outside a subckt) is collected per (kind, name), names compared without
case (SPICE is case-insensitive):

- byte-identical definitions of one name are emitted once;
- different definitions of one name are an error naming both views.

Definitions are emitted in sorted (kind, name) order, so the deck does not
depend on the order of instances or views. Lint applies the same rule to a
package's own views.

**View names are unique by rule** (ADR-011 D11). Every top-level name a
view defines (`.subckt`, `.model`, `.func`) starts with the package prefix,
`<NS>_<NAME>_`: the package id upper-cased with `/`, `-` and `.` as `_`
(`ti/tps22917` gives `TI_TPS22917_`; compared without case, as SPICE reads
names). A view has no top-level `.param` (parameters go inside the
subcircuit or on its line). There is no automatic renaming. `bb pkg lint`
reports a violation as a warning, and with `--publish` (a package to be
published, milestone E1) as an error. The first-party packages were
renamed in the schema epoch (`bb_generic_resistor` became
`GENERIC_RESISTOR_IDEAL`, `NRF_SAADC_SE` `NORDIC_NRF52840_QIAA_SAADC_SE`,
`DRV8837_BRIDGE` and its `.model` `TI_DRV8837_BRIDGE` and
`TI_DRV8837_BRIDGE_BODY`, `TPS22917_LSW_FIT` `TI_TPS22917_LSW_FIT`,
`LSW_WS2812C` `WORLDSEMI_WS2812C_2020_LSW`), and `make packages` lints them
with `--publish`.

## 4. `pins.toml` (`bb-pins/1`)

```toml
schema = "bb-pins/1"

[[pin]]
number = "15"                 # footprint or chip pin number, as KiCad reports it
name = "PA1"                  # KiCad libpart pin names must match (ADR-007 D3)
role = "io"                   # power_in | power_out | ground | io | input | output | analog | nc | passive
supply = "VDD"                # optional: the supply pin its thresholds refer to
alt = ["ADC_IN1"]             # informative alternate functions
models = ["digital_out", "digital_in", "adc_in"]   # the pin models a binding may choose
adc = { channel = "IN1" }     # with adc_in, and only with it: the converter channel

[defaults.digital_out.default]   # named parameter sets: [defaults.<model>.<set>]
voh = "3.3V"
vol = "0V"
rise = "5ns"
fall = "5ns"
zout = "25"
# rail = "VDD"                  # bb-pins/1: the rail a pi output drives from (milestone P2)
# rhiz = "100Meg"               # bb-pins/1: its high-Z leakage (milestone P2)

[[limits]]                      # bb-pins/1 (ADR-009 D3): absolute maximum ratings
pin = "PA1"
max = "VDD + 0.3V"              # <value>, <pin> + <value> or <pin> - <value>
min = "-0.3V"
source = "DS p. 34, Table 7"
```

`bb-pins/1` (the Phase 2 schema epoch) keeps every `bb-pins/0` key and its
meaning, and adds two things, accepted, checked and inert until their
milestones use them (a `bb-pins/0` file, the previous major, has neither):

- **`limits`** (ADR-009 D3, Amendment 11): relational limits
  for absolute maximum ratings, each a `pin` of the package, `min` and/or
  `max`, and `source`. A bound is one supply-plus-offset term: a voltage
  (`3.6V`), or a pin of the package plus or minus a voltage (`VDD + 0.7V`,
  `VSS - 0.3V`; the spaces around the sign are part of the grammar). No
  general expressions. A pin has at most one `min` and one `max`. At `pi`,
  and at `fast` with `bb run --limits`, each bound is an automatic `range`
  assertion (`spec/project.md` section 3.4): `V(pin) - V(reference pin)`
  against the offset, or for a plain voltage `V(pin)` against the part's
  ground pin.
- **the `digital_out` parameters `rail` and `rhiz`** (ADR-009 D5; the `pi`
  glue of milestone P2), both optional: `rail`, a `power_in`/`power_out`
  pin of the package (the default is `[power] io_rail`), and `rhiz`, the
  high-Z leakage resistance (default 100 MΩ). At `fast` they are unused.
  A binding may set them too.

**The `power` pin model** (`power{vmin, vmax, imax}`, `bb-pins/1`, ADR-009
D3, Amendment 11) is a supply pin's ratings: only a `power_in` or
`power_out` pin takes it (`models = ["power"]`), and only it. Its
parameters come from `[defaults.power.<pin name>]` when the package has
that set, else from `[defaults.power.default]`; no binding chooses a set,
since a supply pin never crosses an engine boundary. Each parameter is
optional and a set gives at least one: `vmin` and `vmax` (voltages, against
the part's ground pin) and `imax` (a current). Like `limits`, they become
automatic `range` assertions at `pi` and with `bb run --limits`; `imax` is
`not_evaluable`, since no view senses a pin's current from node voltages
in this build. A `bb-pins/0` file has no `power` model.

```toml
[[pin]]
number = "4"
name = "VDD"
role = "power_in"
models = ["power"]

[defaults.power.default]
vmin = "3.7V"
vmax = "5.3V"
imax = "20mA"
```

- **Pins.** Every pin number of the footprint (or module pinmap) is listed
  exactly once. Pin names are unique, except that supply, ground and `nc`
  pins of one role may share a name (`VDD`, `VSS`). A role limits the
  models: `io` any but `power`; `input` `digital_in` or `adc_in`; `output`
  `digital_out`; `analog` `adc_in`; supply pins `power` (`bb-pins/1`);
  ground, `nc` and `passive` pins none. One binding is not a pin model of `pins.toml`: on a part whose
  platform is Breadboard's nRF52840, an `adc_in` binding on the supply pin
  `VDD` is the SAADC's VDD input (ADR-008 D7), with the binding's own
  parameters (`vref`).
- **Pin models and their parameters** (`spec/pin-model.md`):

  | Model | Parameters (all required unless noted) |
  | --- | --- |
  | `digital_out` | `voh`, `vol` (voltage or supply reference), `rise`, `fall` (time), `zout` (resistance); `bb-pins/1`: `rail` (a supply pin) and `rhiz` (resistance), both optional |
  | `digital_in` | `vih`, `vil` (voltage or supply reference), `cin` (capacitance) |
  | `adc_in` | `vref` (voltage or supply reference), `bits` (count); `model`, `load` (identifiers, ADR-008 D7; optional) |

- **Parameter values are strings**: a number in the value grammar
  (section 6), or for a voltage a supply pin name (`VDD`) or a decimal
  factor times one (`0.7*VDD`). There is no expression language.
- **Supply references resolve at elaboration**, in order: the bench supply
  on the net the supply pin connects to; the bench's `[nominal]` entry for
  that net; otherwise an error naming the pin and the net (ADR-007 D5). A
  hand-written `board.cir` has no bench, so a binding there needs numbers.
- **The parameter set of a binding** is the one its `set` key names, or
  `default`; neither existing is an error. Explicit parameters of a binding
  override the set's, and a value that differs from the set's is reported as
  a lint warning (ADR-007 D9).
- **nRF52840 drive sets** (ADR-008 D4, Amendment 3). On a package whose
  behavioural platform is Breadboard's nRF52840, a `digital_out` set's name
  begins with the `PIN_CNF` DRIVE it stands for (`S0S1`, `H0H1`, ... ; a
  suffix after `_` names a PS column, `H0H1_max`), and `default` stands for
  `S0S1`, DRIVE's reset value. `bb run`'s `PIN_CNF` audit takes a bound
  pin's expected drive from that name.
- **Pin-attached loads.** A view of `kind = "pin_load"` is a load the pin
  model attaches to its own pad (ADR-008 D7). Its ports are typed: `{ role =
  "pad" }` (exactly one: the bound pin's net), `{ role = "ctrl", signal }`
  (a digital control the CPU engine drives), `{ role = "observe", signal }`
  (a node the engine reads back), `{ role = "pin", pin }` (another package
  pin). Control and observe signals are internal to the binding: not pins.
- **Modules** (`kind = "module"`, ADR-006 Amendment 3). A module package
  lists its own pads in `pins.toml` (numbers and names as the board's
  symbol gives them, roles, the models a pad offers, `supply`) and maps
  them onto its chip package in the `[module] pinmap` file, a CSV with the
  header `pad,chip_pin` and one row per pad: the module pad number, then
  the chip package's pin name. Every pad except an `nc` one is mapped
  exactly once. The pads carry **no parameter sets** and the module has
  **no `[behavioral]` layer**: a binding on a module pad takes its pin
  model's parameter sets from the chip pin the pad maps to, a supply
  reference in such a set names a chip pin, which the pinmap leads back to
  a module pad and its net, and the behavioural layer is the chip's. The
  chip package is part of the resolved set wherever the module is (it is
  in `bb.lock`, and its platform too). A pad's models must be offered by
  its chip pin, a supply, ground or power pad maps to a chip pin of the
  same role, and a pad's `supply` maps to the chip pin's `supply`.

## 5. The `gnd` rule

ngspice treats a node named `gnd` as global ground, even inside a
subcircuit. No subckt port or internal node of a view may be named `gnd` (in
any case); lint refuses it (`spec/pin-model.md` glue rule 7). A migrated
view renames such a node (e.g. `GND` to `GNDP`) as a recorded deviation.

## 6. Values

One grammar for every numeric string in `part.toml`, `pins.toml`, test
vectors and the bench:

- a decimal number, optionally signed, optionally with an exponent (`3.3`,
  `1e-5`, `-10`);
- an optional SI prefix: `f p n u µ m k Meg G T` (`m` is milli, `Meg` is
  mega);
- **a bare `M` is an error** that suggests `Meg` or `m` (ngspice reads `M`
  as milli, EE text as mega);
- an optional unit, checked against the parameter's dimension: `V`, `s`,
  `Ω`/`ohm`, `F`, `H`, `A`, `°C`, `%` (a view's `params` declare no
  dimension, so any of these units, and `Hz`, `W`, is accepted there).

A prefix is taken only where what follows is empty or a unit (`5ns`, `3m`),
so `1F` is one farad, not one femto.

**Strings are never passed through to SPICE.** Each is parsed exactly into
an integer in its dimension's sub-unit (ADR-007 D6), and a value that is
not a whole number of sub-units is an error, never rounded:

| Dimension | Sub-unit |
| --- | --- |
| voltage | µV |
| time | fs |
| resistance | µΩ |
| capacitance | aF |
| inductance | fH |
| current | pA |
| temperature | m°C |
| percentage | ppm (`1%` and `1` both mean one percent) |
| count | 1 |

The engines take `f64` volts, ohms and farads: the integer is converted by
one division of exact integers, which gives the correctly rounded value of
the decimal, the same `f64` as parsing the decimal text. Elaboration emits
**SPICE numbers without a letter suffix**: plain digits, a decimal point and
an `e` exponent only (`2200000`, `4.7e-6`, `0.0015`).

**KiCad value fields** (the value of a generic passive) follow EE
convention instead, as the dial-me-spinner suite's `spicelib` reads them:

- the first token is the value: `M`, `Meg` and `MEG` are mega, `m` milli,
  `K` kilo; an RKM infix (`4k7`, `2R2`, `51R`; `R`/`r` marks the unit
  point); an optional unit (`Ω`, `ohm`, `F`, `H` by kind); an optional
  `/<rating>V` (`10u/25V`);
- then, each at most once (ADR-006 Amendment 1): a tolerance `<x>%`; a
  voltage rating `<v>V`; a power rating `<p>W` (`1/4W`, `0.1W`, `100mW`);
  for capacitors a dielectric code (`C0G`, `NP0`, `NPO`, `X5R`, `X6S`,
  `X7R`, `X7S`, `X7T`, `X8R`, `Y5V`, `Z5U`) or a technology (`Ta`, `Al`);
- anything else is a parse error, never a guess.

The result is the exact value in the kind's sub-unit, with the tolerance,
ratings, dielectric and technology kept as metadata for `pi` views. At
`fast` the nominal value is used.

## 7. Generic passives

`generic/resistor`, `generic/capacitor` and `generic/inductor`: two pins,
`1` and `2` (role `passive`), and one view, `ideal`, the SPICE primitive
`R`, `C` or `L` with the instance's value as the subckt parameter `value`
(in base units). They carry no MPN. They are selected by footprint plus
value (ADR-007 D4 stage 5): footprint library `Resistor_*`, `Capacitor_*`
or `Inductor_*`, or reference prefix `R`, `C` or `L` on a custom footprint.
A package for a specific MPN overrides them whenever one exists.

## 8. Named option sets, test vectors and lattice qualification

**Named option sets** (`bbpkg::optsets`):

| Name | `.options` card |
| --- | --- |
| `phase0` | `.options RELTOL=1e-5 VNTOL=1e-8 ABSTOL=1e-13` (every Phase 0 `board.cir`; the default for imported boards and for vectors) |
| `suite_fallback` | `.options RELTOL=1e-3 METHOD=GEAR RSHUNT=1e12` (the dial-me-spinner suite's last numeric fallback) |

Others are added here by name with their exact contents.

**A test vector** (`bb-pkgtest/0`, one TOML file in the package's tests
directory):

```toml
schema = "bb-pkgtest/0"
view = "ideal"                 # the view under test
circuit = "rc_step.cir"        # a SPICE body next to the vector: no .tran, .options or .control
stop = "6ms"                   # a multiple of the lattice
lattice = "10us"               # optional, default 10us
options = "phase0"             # optional, default phase0
sample = "10us"                # optional: the sampling period of crossing measurements (default the lattice)
params = { value = "1k" }      # optional: the instance's view parameters

[ports]                        # every view port -> a circuit node
1 = "in"
2 = "out"
# A pin_load view's typed ports (section 4) are named by role: pad = "...",
# a ctrl or observe port by its signal (smp = "...", hold = "..."), a pin
# port by its pin (VDD = "..."); ADR-008 Amendment 5.

[[edge]]                       # a control input, delivered as digital_out glue
net = "in"
initial = "low"                # optional, default low
model = { voh = "1V", vol = "0V", rise = "1ns", fall = "1ns", zout = "0" }
at = [["1ms", "high"]]         # increasing times (> 0), alternating levels

[[expect]]
meas = { node = "out", at = "2ms" }                   # V(node) at a time
value = "0.632120V"
tol = "0.1mV"                                         # absolute
source = "analytic: ..."                              # where the expected value comes from

[[expect]]
meas = { node = "out", when = "0.5V", rise = "1", after = "0s" }   # the time of the n-th crossing (rise or fall)
value = "1.693148ms"
tol = "2us"
source = "..."
```

**The runner** (`bb pkg test`) builds a deck from the view's definitions,
`X1` with the view's ports mapped to the nodes and its parameters (the
view's `params`, overridden by the vector's) as SPICE numbers, the circuit
and the option set's card, and runs it **through `NgspiceEngine` itself**
under the kernel's scheduler: lattice `L`, TMAX = `L`, the engine's lattice
clamp and breakpoint release, and each `[[edge]]` delivered as
`digital_out` glue at its time. A standalone `.tran` does not predict the
engine (ADR-006 Context 3). A voltage is measured at its instant
(interpolated between solver points) and rounded to µV; a crossing time is
linearly interpolated between the samples every `sample` and rounded to fs.
**Each vector runs at two grant partitions, one lattice cell and 1 ms, and
every sample and measurement must be identical.** `bb pkg test --report
<file>` also writes one JSON line per vector run (the package id, view,
vector file name, lattice, option set, partition, every measured value in
µV or fs, and the sha256 of every sample's `f64` bits), so two packages'
results compare byte for byte (ADR-006 gate G8).

Before the transient, the runner **lints the deck's expressions** (section
11) and **solves the vector's DC operating point** under the run's option
set (ADR-015 D4): an `.op` analysis with the option set's card, so its
iteration limits apply (Newton, gmin stepping, source stepping), without
ngspice's transient-op fallback (`optran`, which ngspice 42 makes its
default and which a large deck can spend tens of seconds in before it gives
up). A failure is reported at once, naming the nodes ngspice names. Every
pair a vector runs under (its own and every recorded qualification) gets
this check. **At every corner** (ADR-009 D2 rule 2, milestone P1): for a
view with `[[analog.corners]]`, `bb pkg test` also solves each vector's
operating point with each corner's `params` over the view's and the
vector's, under every option set the view is run under, and a failure
fails the view (a view qualified at a pair has its operating point solved
at every corner too). The engine's own `.save` (ADR-015 D2) is part of every deck:
a circuit or view with a `.save` card of its own is refused.

**Qualification.** A view is lattice-qualified for (`L`, option set) when
all its vectors pass under that pair. `bb pkg test` runs each view under
every vector's own pair and every recorded `[[analog.qualified]]` pair; a
recorded pair that fails is a failure. `--record` writes the passing pairs.
A content change invalidates records through the tree hash (section 9).

- **A looser option set is admissible only by evidence:** a view is
  qualified under a set other than `phase0` only if every vector also runs
  under `phase0` and the two results agree within each vector's tolerance.
  If `phase0` cannot run a vector, the view cannot be qualified under a
  looser set either.
- **`bb run` checks a run against qualification** (ADR-007, M4): the
  bench's options must be a named set, and every in-scope view qualified for
  (`L`, that set), unless `--allow-unqualified <view>`, which `run.json`
  records. The `pin_load` view of a loading `adc_in` binding is in the deck
  too and is checked the same way (ADR-008 D7, Amendment 5).
- **Board-level convergence is separate:** qualification in a test circuit
  says nothing about a whole board's deck (plan M6).

## 9. Tree hashes, platform identities and `bb.lock`

**Tree hash** of a package: `sha256:` + the hex sha256 of the sorted lines
`"<relative path>\0<hex sha256 of the file's bytes>\n"` over every regular
file of the package directory, excluding dotfiles (any path component
starting with `.`) and `tests/out/`. Paths use `/`. Bytes are hashed as
stored, so a CRLF checkout changes the hash (`.gitattributes` forces LF). A
symbolic link in a package is an error.

**Platform identity.** Every engine platform `bb` provides has an id
`<name>@<major>` and a content hash: the same list hash over the files that
decide its behaviour, by file name. For Renode that is the `.repl` the
platform loads and the plugin sources it declares (ADR-015 D6): the sources
of the models its `.repl` instantiates, and the shared ones every platform
declares (`bb_lookahead.cs`, the lookahead block that `bb run` maps on any
platform; there is no shared host-port base, each host port is defined in
its model's source). `bb` embeds the table (`cli/src/platforms.rs`; the
declarations are `bb_cpu_renode::PLATFORM_PLUGINS` and `SHARED_PLUGINS`, all
under `engines/cpu-renode/platforms/`):

| Platform | Loads | Declared plugin sources | Board-file name |
| --- | --- | --- | --- |
| `stm32f103-offline@1` | `stm32f103-offline.repl` | `bb_lookahead.cs` | `stm32f103` |
| `stm32f072-bb@1` | `stm32f072-bb.repl` | `bb_lookahead.cs`, `bb_stm32_timer.cs`, `bb_stm32f0_adc.cs` | `stm32f072` |
| `nrf52840-bb@4` | `nrf52840-bb.repl` (ADR-008 D1, D7-D9; ADR-009 D4, D6, D7) | `bb_lookahead.cs`, `bb_nrf52_saadc.cs`, `bb_nrf52_pwm.cs`, `bb_nrf52_state.cs`, `bb_nrf52_power.cs` | `nrf52840` |

A behaviour change bumps the major and changes the hash. The lock records
Renode's version (1.17.0) with each Renode platform. So a change to a
model's source changes the hash of the platforms that declare it and of no
other: an nRF model change re-locks no STM32 board.

- **The declaration is complete.** Each platform's declared sources compile
  alone, as a separate assembly, with Renode's own compiler (`make
  plugin-scope`). A model that used a type of an undeclared source fails
  there.
- **The residual.** At run time the plugin cache still compiles every
  plugin source into one assembly (ADR-005 D6), so a run on platform A
  loads code from sources that A's lock does not cover. A compile failure
  of that assembly fails every run, so it cannot pass silently. A model
  that A never instantiates cannot act on A's run: .NET runs a type's
  static initialiser only when the type is first used, and the assembly
  has no module initialiser (`make plugin-scope` checks its metadata).
  Process-wide static state is covered by the pool rule (`make
  pool-equiv`). **A's lock does not pin the sources A does not
  instantiate.**
- **The legacy hash** (ADR-015 Amendment 3). Before Phase 2 H4 the hash
  covered every plugin source. A `bb-lock/0` entry that holds exactly that
  hash of the current files pins strictly more than the scoped hash does,
  so `bb run` accepts it as it stands from a `bb-lock/0` file and carries it
  into `run.json`. `bb open` and `bb pkg migrate --lock` migrate a
  `bb-lock/0` file: such an entry is written with the scoped hash (a
  migration, not a change, so no `--update-lock` is needed). In a
  `bb-lock/1` file only the scoped hash matches. The schema epoch
  (milestone S) relocked every committed lock this way.

Every `bbdev` device (ADR-006 D7) has an identity the same way: an id
`<name>@<major>`, locked as `device:<id>`, and a content hash over the
sources that decide its behaviour, by file name: the device API and its
engine (`bbdev/src/lib.rs`, `bbdev/src/engine.rs`) and the device's own
files. `bb` embeds the table (`bbdev::DEVICES`):

| Device | Sources |
| --- | --- |
| `dms/mechanism@2` | `bbdev/src/lib.rs`, `bbdev/src/engine.rs`, `bbdev/src/dms_mechanism.rs` (ADR-008 Amendment 9; ADR-014 D2: its sample timer is an output timer and its inputs are promise-independent. It replaced `dms/mechanism@1` in Phase 2 H5, with `@1`'s results, ADR-014 G2) |

**`bb.lock`** (`bb-lock/1`, ADR-011 D5), next to the project's `bb.toml`,
or next to a hand-written `board.cir`:

```toml
schema = "bb-lock/1"

[[package]]
id = "st/stm32f103c8"
version = "0.1.0"
source = "path+../../packages/st/stm32f103c8"   # as found on the search path, relative to the lock (informative; a mismatch warns)
tree = "sha256:..."                             # the integrity root

# [[vendor_file]]                       # a file fetched at use, never redistributed ([behavioral] svd)
# url = "https://.../STM32F103.svd"
# sha256 = "<64 hex digits>"
# license = "LicenseRef-ST-EULA"
# used_by = "st/stm32f103c8"

[[platform]]
id = "renode:stm32f103-offline@1"
content = "sha256:..."
engine = "renode"                       # engine-neutral: a Phase 3 backend is another value,
engine_version = "1.17.0"               # not another lock major
```

- `source` is `path+<path>` for a local package, or `registry+<url>`
  (`https://` or `file:///`, one trailing `/`) for one a registry supplied
  (`spec/registry.md` section 6). A `path+` source's path is informative (the
  warning below); a change of the kind of source, or of registry, for a
  locked id is a mismatch (refused unless `--update-lock`, by `bb open` and
  by `bb run`). `bb run` takes a `registry+` package from the registry cache
  by its tree hash and never fetches.
- `[[vendor_file]]` entries (url, sha256, license, used_by; sorted by
  `used_by`, then `url`): one per `[behavioral] svd` of a resolved package
  (section 2). `bb open` fetches each into the cache and checks its sha256;
  a new one is added, a changed one is a mismatch.
- `engine` and `engine_version` come together. A device identity
  (`device:<id>@<major>`) has neither.
- **`bb-lock/0`**, the previous major, is read and migrated in memory:
  `path = "<p>"` is `source = "path+<p>"`, `renode = "<v>"` is `engine =
  "renode"`, `engine_version = "<v>"`. `bb run` reads it with the migration
  hint; `bb open` rewrites it as `bb-lock/1` (also when nothing else
  changed, so `bb open --check` fails on a `bb-lock/0` file), and so does
  `bb pkg migrate --lock <bb.lock>...`. The migration writes a legacy
  platform hash as the scoped one (above) and drops an entry for a Renode
  platform this `bb` does not provide (an old platform major that no
  package names any more, which has no scoped hash).

Entries are sorted by id. Written by `bb open`:

- an entry not yet in the lock is added;
- an entry whose version, tree or content differs from what is resolved now
  is **refused**, unless `bb open --update-lock`, which rewrites only the
  entries that changed and keeps every other entry byte for byte;
- `bb open --check` writes nothing and fails if the lock would change.

A module's chip package (section 4) is locked with the module, and the
chip's platform with it. `bb run` refuses a board whose packages or
platforms differ from its lock, or a board with packages and no lock, and
never writes the lock.

**A moved package warns** (ADR-015 D7). The tree hash decides acceptance,
and the `path+` source stays informative. But `bb open` and `bb run` print a
warning for every locked package that resolved somewhere other than its
recorded path (relative to the lock's directory; the same directory
spelled differently does not count). The warning says what is at the
recorded path:

- nothing with that id;
- an identical copy, not used;
- or a different package with that id, with its tree hash, that is **not
  used**: the trap of a modified package beside a copied board. It names
  the fix: `*bb packages path=` in a board file (section 12), or `bb.toml`'s
  `[packages] paths`.
`run.json` (ADR-007 D6) records both kinds of hash.

## 10. Schema versions and package versions

Applies to `bb-part`, `bb-pins`, `bb-pkgtest`, `bb-lock`, `bb-project`,
`bb-bench`, `bb-board` and `bb-test` (`spec/test.md`; ADR-010 D8,
milestone B3).

- A schema id is `<name>/<major>`. Within a major, changes are additive
  only, and only through reserved keys becoming accepted. Anything else (a
  rename, a removed key, a changed meaning) is a new major and needs an ADR.
- `bb` reads the current and the previous major of every schema and writes
  only the current one. Reading the previous major prints a migration hint;
  a newer major is refused with "upgrade bb"; an older one is refused.
- **Registry content** is immutable, so `bb` reads every `bb-part` major ever
  published to a registry, migrating in memory at read time (ADR-011 D12,
  `spec/registry.md` section 5); the archive and its tree hash never change.
  An unknown major there is refused with "update bb".
- `bb pkg migrate` rewrites a package (its `part.toml`, `pins.toml` and test
  vectors) in place to the current majors, mechanically. First-party
  packages are migrated in the same commit as the schema change. `bb pkg
  migrate --bench`, `--project`, `--board` and `--lock <file>...` rewrite
  bench files (ADR-010 D1), project files, `board.bbm` files (ADR-013; `bb
  open` rewrites those whole too) and lockfiles (ADR-011 D5, section 9).
- **Current majors** (the Phase 2 schema epoch, milestone S, ADR-009 to
  ADR-013): `bb-part/1` and `bb-pins/1` (sections 2 to 4), `bb-lock/1`
  (section 9), `bb-project/1`, `bb-bench/1` and `bb-board/1`
  (`spec/project.md`, `spec/board-bbm.md`). Every `/0` key of `bb-part`,
  `bb-pins`, `bb-bench`, `bb-project` and `bb-board` keeps its meaning,
  so their migrations rewrite the `schema` line (member) only; `bb-lock`'s
  renames two keys (section 9). `bb-pkgtest` stays at major 0, and
  `bb-test` (milestone B3) starts at major 0. Every committed file is at its
  current major.
- **Package semver:** a change to a view's results (a vector's measured
  value) is at least a minor bump; a changed port list, pin list or
  parameter-set name is a major bump; a provenance-only edit is a patch.

## 11. Lint (`bb pkg lint`)

`bb pkg lint --publish` (and `bb registry publish`, which runs it) checks a
package for publication: the view-name rule below is an error, and **the
licence gate** (ADR-011 D7) applies: every licence id of `part.license`
OSI-approved, or CC0-1.0, CC-BY-4.0 or CC-BY-SA-4.0 for a package without
view code; no `LicenseRef-*`; every view with a provenance `source`; a
`vendor_derived` view with a redistributable `upstream_license`. A vendor
file (`[behavioral] svd`) may carry any licence: it is not in the package.

Errors:

- the schemas (section 2, 4): unknown or reserved keys, enums, formats;
- the licence is an SPDX expression; `LicenseRef-local-only` is refused in
  breadboard.dev's `packages/`;
- `[behavioral]` names a platform or device this `bb` provides;
- every pin number once; pin names unique (section 4); models allowed by
  the role; `adc = { channel }` exactly with `adc_in`; `supply` names a
  supply pin;
- every parameter set complete for its model; every value parses (section
  6, so a bare `M` is an error); supply references name supply pins of the
  part; `vil < vih`;
- views: unique names; `default_for` a subset of `fidelity`; one default per
  rung; `params` parse; qualification records name a lattice and a named
  option set; a provenance block (the view's or the package's); `circuit`
  ports name pins, once each, and match the subckt's port count; a
  `pin_load` view has exactly one pad port;
- a module (section 4): the pinmap file parses, every pad except `nc` is
  mapped once and only pads are mapped, no `[defaults]`, no
  `[behavioral]`; with the chip package found (in the module's own package
  root, else first-party) each chip pin exists, roles, models and supplies
  agree;
- view files: top-level content (section 3), the subckt exists, the `gnd`
  rule (section 5), and the package's own views define no top-level name
  differently;
- **expressions** (ADR-015 D3; `bbpkg::bexpr`): every function call in a
  B source's `V=`/`I=` expression, an E or G source's `value=`, `vol=` or
  `cur=` expression and a `.func` body is checked against ngspice 42's own
  tables, taken from its sources: the B-expression parser's
  (`src/spicelib/parser/inpptree.c`: one argument, except `pow`, `pwr`,
  `min` and `max` with exactly two, `ternary_fcn` with three, `pwl` with an
  input and `x, y` pairs; `v()` one or two nodes, `i()` one source) and,
  inside `{...}`, numparam's (`src/frontend/numparam/xpressn.c`). An
  unknown function (not in the table and not a `.func`), a wrong arity and a
  `.func` called with the wrong number of arguments are errors. A
  three-argument `min()` segfaults libngspice 42; in numparam it is
  silently `min(a, c)`. `bb run` and `bb pkg test` run the same check on
  the whole deck before ngspice reads it;
- **a view valid at `pi`** (ADR-009 D2 rule 1, milestone P1;
  `bbpkg::pirules`): no branch-current unknown in its file. A V source (a
  0 V ammeter too), an E, F or H source, a V-type B source, a W switch, a
  transmission line (T, O, U) and an XSPICE code model (A) are errors; an
  internal signal is an I-type B source into a 1 Ω conductance. R, C, G,
  D, transistor primitives and I-type B sources are allowed. There is no
  exemption for "signal nodes";
- test vectors parse and name existing views and their ports;
- the schema epoch's keys (`bb-part/1`, `bb-pins/1`): `[power] io_rail` and
  a `digital_out` set's `rail` name `power_in`/`power_out` pins; an idd
  `max` is not below its `typ`; a `supply_load` view is not valid at
  `fast`, is no rung's default, has a supply pin port, its pin ports are
  supply or ground pins and each state port has a `[power] idd` entry; only
  a `supply_load` view has state ports; `rails` name the view's ports on
  supply pins; a corner's `params` override the view's own and parse, its
  name is unique and its `ref` is not empty; `[claims] file` exists; an
  `upstream_license` is an SPDX expression; a limit's pin, and its bound's
  pin, are pins of the package;
- with `--publish`, the view-name rule (section 3; ADR-011 D11).

Warnings: the view-name rule without `--publish`, a previous-major file (the migration hint), a parameter set for a
model no pin takes, a view without test vectors, a module whose chip
package was not found, an inductor (L) or coupling (K) in a view valid at
`pi` (ADR-009 D2 rule 5: ngspice gives it a branch current, and a series
L of milliohms cost +68 % in the prototype), and a **conflict**: one parameter that several views
of the package carry with different defaults (ADR-006 D9 rule 2; reported,
never averaged, because each view is right for its own purpose).

`scripts/validate_packages.py` validates the TOML of every package and lock
against the JSON Schemas, and checks that `mpn_patterns` compile.
`scripts/check_pins_kicad.py` compares a `pins.toml` with a KiCad symbol
library entry (`--composite`: a KiCad chip symbol's multi-function name such
as `AIN0/P0.02` matches the pin named by one component, `P0.02`, with the
others in `alt`).

## 12. Parts from packages in a hand-written `board.cir`

```
*bb part U1 package=st/stm32f103c8
*bb pin U1.PA5 net=PA5 model=digital_out
*bb pin U1.PB0 net=SENSE model=digital_in set=default
*bb pin U1.PA6 net=PA6 model=digital_out zout=50       # overrides the set (lint warning)
```

- `package=<id>[@<version>]` takes the engine and platform from the
  package's `[behavioral]` layer, which must be `renode` on a platform `bb`
  provides, or `device` on a device `bb` provides (ADR-006 D7 and
  Amendment 6). No `engine=` or `platform=` alongside it. A board has
  exactly one `renode` part (its CPU) and any number of `device` parts.
- A pin must be in the package's `pins.toml` and allow the model. Its
  parameters are the package's set (`set=`, default `default`); explicit
  keys override them in the value grammar of section 6. An `adc_in` pin's
  channel is the package's `adc = { channel = "IN<n>" }`.
- The board needs a current `bb.lock` next to it (`bb open <board.cir>`).
- The Phase 0 form (`engine=renode platform=...` with every parameter
  written out) is unchanged and needs no lock.

**The package path** (ADR-015 D7):

```
*bb packages path=../../packages      # searched before the first-party directory
```

- Each `*bb packages path=<dir>` line adds one directory to the board
  file's search path, in file order, before the first-party directory, as
  a `bb.toml`'s `[packages] paths` do (section 1). The line has no name and
  exactly the key `path`.
- `<dir>` is relative to the board file's directory. It has no whitespace
  (the grammar has no quoting). An absolute path is an error, and so is a
  directory that does not exist: a skipped entry would hide the package it
  was meant to provide.
- The lock records each package's `path` as resolved through it, so a board
  and its packages copied together keep resolving to the copy.

**Device parts** (ADR-006 D7, Amendment 6):

```
*bb part M1 package=dms/detent-mechanism ts=75ms s0=500
*bb pin M1.NSLEEP net=MOT_NSLEEP model=digital_in
*bb pin M1.HOME_N net=HOME_N model=digital_out
```

- The other keys of a device part's line are the device's parameters, in
  the value grammar of section 6. Each device states its keys, their
  dimensions and their defaults; they are part of its source, so of its
  content hash (section 9). An unknown key or a value of the wrong
  dimension is an error.
- A device pin is `digital_in` or `digital_out`; `adc_in` is refused.
- **Routing.** A device `digital_out` drives its kernel net `<ref>.<pin>`.
  The analog engine gets it as `digital_out` glue on `net=`
  (`spec/pin-model.md` glue rule 1), and every `digital_in` on that net
  (the CPU's or another device's) reads the kernel net itself: the kernel
  passes the driver's signal unchanged (`spec/pin-model.md`, "A net has
  exactly one driver"), with no threshold and no `cin` glue. In the same
  way a device `digital_in` on a net that the CPU's or another device's
  `digital_out` drives reads that kernel net. On any other net, the analog
  engine thresholds the node with the pin's `digital_in` parameters (glue
  rule 2).
- A device's id and content hash are locked as `device:<id>@<major>`
  (section 9).
- **Output timers and promise-independent inputs** (ADR-014 D2). A
  device schedules timers with `ctx.schedule(at, id)` or
  `ctx.schedule_output(at, id)`; the latter is an *output timer*. A device
  may declare `promise_independent_inputs` (a fixed property of the
  device, stated in its source) only if `on_pin_change` never calls
  `set_pin` (rule 1, every device) and never schedules an output timer
  earlier than a pending one, and only its output timers' handlers call
  `set_pin`: then its outputs change only at its output timers. For such
  a device the engine refuses `set_pin` in any other timer's handler, its
  quiet promise (`quiet_until`) is its next **output** timer, and its
  inputs cannot end that promise (`input_ends_promise` is `false`,
  `spec/time-grant.md`), so the CPU driving them is not a producer of its
  quiet grant. The kernel's promise audit (`PromiseBroken`) catches a
  device that breaks the declaration. A device that does not declare it
  keeps the Phase 1 rule: every timer may set a pin, and its promise is
  its next timer of any kind. A change of a device's promise is a change
  of behaviour: the major bumps (section 9), even when, as for
  `dms/mechanism@2`, every result stays the same.
- A device part exists in a board file, or in a project's `[[device]]`
  (`spec/project.md` section 1; ADR-013 D1, Phase 2 R1). The rules above
  apply to both.

**Digital-only nets** (ADR-014 D4):

```
*bb digital MOT_A_IN1
```

- `*bb digital <net>` declares the net a bound `digital_out` drives
  (`net=`, matched without regard to case) **digital-only**. The line has
  no keys, and a net is declared once.
- A digital-only net gets no glue: the driver's pin is routed through its
  kernel net only, the analog engine has no input or output on it, and
  every `digital_in` on it reads that kernel net directly. So every reader
  must be a device, or read a device's output (the direct route above); a
  CPU `digital_in` on a CPU output's net, or an `adc_in` on the net, is an
  error.
- A SPICE element on a digital-only net (a line outside `.subckt`
  definitions that names the node, a B expression's `V(<net>)` included)
  is an error, so the directive cannot silently drop a part.
- `--probe net:<net>` on a digital-only net is refused: its level is the
  driver's kernel net (`<ref>.<pin>`), as on the project route
  (`spec/project.md` section 4).
- Without the directive nothing changes: no existing board's deck or
  results move. Declaring one changes the board's deck, so its results:
  a model change (ADR-014 D4: E3's homing boards, Phase 2 H5).

## 13. Examples

- A complete package: `packages/generic/resistor/` (view, pins, two test
  vectors, a recorded qualification).
- MCU packages: `packages/st/stm32f103c8/`, `packages/st/stm32f072rb/`,
  `packages/nordic/nrf52840-qiaa/` (named GPIO parameter sets per PIN_CNF
  drive and PS column), and the module `packages/raytac/mdbt50q-u1mv2/`
  (section 4).
- Analog views with suite-derived vectors (Phase 1 M5): `packages/ti/
  tps22917/` (a datasheet-fitted view written by an agent, so
  `agent_generated`; the vendor-derived one is local-only, ADR-006 D9 rule
  4) and `packages/worldsemi/ws2812c-2020/` (a migrated suite model, listed
  with its sha256 in `examples/dial-me-spinner/SOURCE`).
- Format examples of a load switch and a chip with an ADC pin load:
  `spec/examples/packages/ti/tps22917/` and
  `spec/examples/packages/nordic/nrf52840-qiaa/`. They illustrate the
  format and are schema-validated; they are **not** the Phase 1 packages
  (plan M5 and M5b write those from the datasheets and the suite), and
  their view files are not included.

## 14. Registries

Registries are specified in `spec/registry.md` (ADR-011): the static layout,
the canonical index lines, the signed snapshot and its serial, the
deterministic archive (`bb pkg pack`, `bb pkg unpack`), yanking and removal,
routing by namespace, vendor files, the cache, and `bb registry`, the
registry's CI tools. A registry package is the same package as a local one
when its tree hash is the same: decks and outputs depend on tree hashes
only, and `run.json` records each package's source (`path`, or its
`registry+` source) as an input.
