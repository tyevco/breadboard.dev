# Component packages (`part.toml`, `pins.toml`, views, tests, `bb.lock`)

Status: normative. Decided by ADR-006 (accepted 2026-11-09, approver
decisions A-C; Amendment 1 for the KiCad value qualifiers; Amendment 3 for
modules, the view conflict report and `bb pkg test --report`). Implementation:
the `bbpkg` crate (`bbpkg/src`), `bb pkg`, `bb open` and `bb run` (`cli/src`).
JSON Schemas: `spec/schema/part.schema.json`, `pins.schema.json`,
`lock.schema.json`, `pkgtest.schema.json` (JSON Schema 2020-12; the TOML
files are converted to JSON as they are and validated, `make packages`).
**Changing this file requires an ADR.**

A package describes one part: what it is (`part.toml`), its pins and their
pin models (`pins.toml`, `spec/pin-model.md`), its analog views (SPICE
subcircuits with provenance), its behavioural layer (the CPU engine platform
it runs on), and the test vectors that qualify its views. Packages are local
directories; `bb.lock` pins their content. There is no registry, no signing
and no `bb pkg publish` in Phase 1 (ADR-006 D8).

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
  directory are an error. A hand-written `board.cir` has no `bb.toml`, so its
  search path is the first-party directory alone. `bb` finds that directory
  through the path of the checkout it was built from, or
  `$BB_FIRST_PARTY_PACKAGES`.

## 2. `part.toml` (`bb-part/0`)

TOML is the authored form; `spec/schema/part.schema.json` is canonical.
**Unknown keys are an error.** Keys reserved for `bb-part/1` are refused
with "reserved for bb-part/1": top-level `layers`, `thermal`, `rtl`,
`power`; `[pins] ibis`; `[behavioral] cpu`, `svd`; a view's `rails`.

| Key | Required | Meaning |
| --- | --- | --- |
| `schema` | yes | `"bb-part/0"` |
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
| `[[analog]]` | no | analog views (section 3) |
| `[provenance]` | no | the package-level default for views without their own block |
| `[tests] dir` | no | the test vector directory (section 8) |

**Provenance** (`[analog.provenance]` or `[provenance]`): `source`
(`datasheet`, `vendor_model`, `vendor_derived`, `measured`, `estimated`,
`agent_generated`; required), `base`, `refs`, `deviations`,
`not_modelled`, `origin` (`<repo>@<commit>:<path>` of a migrated file),
`generated_by`, `reviewed_by`, and `confidence` (required), an enum, not a
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
  | `kind` | `circuit` (default) or `pin_load` |
  | `purpose` | text |
  | `fidelity` | the rungs it is valid for (`fast`, `pi`, `si`, `em`; at least one) |
  | `default_for` | the rungs it is the default for; a subset of `fidelity`; at most one view per package may name a rung |
  | `file` | the view file (SPICE), relative to the package |
  | `subckt` | the subcircuit the file defines |
  | `ports` | `circuit`: the subckt's ports in order, as pin names; `pin_load`: typed ports (section 4) |
  | `params` | `{ name = "<value>" }` defaults, in the value grammar (section 6) |
  | `[[analog.qualified]]` | `{ lattice, options }` pairs the view is qualified for, written by `bb pkg test --record` (section 8) |
  | `provenance` | section 2 |

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

## 4. `pins.toml` (`bb-pins/0`)

```toml
schema = "bb-pins/0"

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
```

- **Pins.** Every pin number of the footprint (or module pinmap) is listed
  exactly once. Pin names are unique, except that supply, ground and `nc`
  pins of one role may share a name (`VDD`, `VSS`). A role limits the
  models: `io` any; `input` `digital_in` or `adc_in`; `output`
  `digital_out`; `analog` `adc_in`; supply, ground, `nc` and `passive` pins
  none.
- **Pin models and their parameters** (`spec/pin-model.md`):

  | Model | Parameters (all required unless noted) |
  | --- | --- |
  | `digital_out` | `voh`, `vol` (voltage or supply reference), `rise`, `fall` (time), `zout` (resistance) |
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
  records.
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
platform loads and every source of Breadboard's Renode plugin assembly
(`PLUGIN_SOURCES`, the inputs of ADR-005's plugin cache key). `bb` embeds
the table (`cli/src/platforms.rs`):

| Platform | Loads | Board-file name |
| --- | --- | --- |
| `stm32f103-offline@1` | `engines/cpu-renode/platforms/stm32f103-offline.repl` | `stm32f103` |
| `stm32f072-bb@1` | `engines/cpu-renode/platforms/stm32f072-bb.repl` | `stm32f072` |
| `nrf52840-bb@1` | `engines/cpu-renode/platforms/nrf52840-bb.repl` (ADR-008 D1) | `nrf52840` |

A behaviour change bumps the major and changes the hash. The lock records
Renode's version (1.17.0) with each Renode platform.

**`bb.lock`** (`bb-lock/0`), next to the project's `bb.toml`, or next to a
hand-written `board.cir`:

```toml
schema = "bb-lock/0"

[[package]]
id = "st/stm32f103c8"
version = "0.1.0"
path = "../../packages/st/stm32f103c8"   # as found on the search path, relative to the project (informative)
tree = "sha256:..."

[[platform]]
id = "renode:stm32f103-offline@1"
content = "sha256:..."
renode = "1.17.0"
```

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
`run.json` (ADR-007 D6) records both kinds of hash.

## 10. Schema versions and package versions

Applies to `bb-part`, `bb-pins`, `bb-pkgtest`, `bb-lock`, `bb-project`,
`bb-bench` and `bb-board`.

- A schema id is `<name>/<major>`. Within a major, changes are additive
  only, and only through reserved keys becoming accepted. Anything else (a
  rename, a removed key, a changed meaning) is a new major and needs an ADR.
- `bb` reads the current and the previous major of every schema and writes
  only the current one. Reading the previous major prints a migration hint;
  a newer major is refused with "upgrade bb"; an older one is refused.
- `bb pkg migrate` rewrites a package (its `part.toml`, `pins.toml` and test
  vectors) in place to the current majors, mechanically. First-party
  packages are migrated in the same commit as the schema change.
- **Package semver:** a change to a view's results (a vector's measured
  value) is at least a minor bump; a changed port list, pin list or
  parameter-set name is a major bump; a provenance-only edit is a patch.

## 11. Lint (`bb pkg lint`)

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
- test vectors parse and name existing views and their ports.

Warnings: a previous-major file (the migration hint), a parameter set for a
model no pin takes, a view without test vectors, a module whose chip
package was not found, and a **conflict**: one parameter that several views
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
  provides. No `engine=` or `platform=` alongside it.
- A pin must be in the package's `pins.toml` and allow the model. Its
  parameters are the package's set (`set=`, default `default`); explicit
  keys override them in the value grammar of section 6. An `adc_in` pin's
  channel is the package's `adc = { channel = "IN<n>" }`.
- The board needs a current `bb.lock` next to it (`bb open <board.cir>`).
- The Phase 0 form (`engine=renode platform=...` with every parameter
  written out) is unchanged and needs no lock.

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
