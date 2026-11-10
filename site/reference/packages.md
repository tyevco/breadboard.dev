# Component packages

A **package** describes one part: what it is (`part.toml`), its pins and
their pin models (`pins.toml`), its analog views (SPICE subcircuits with
provenance), its behavioural layer (the CPU engine platform it runs on), and
the test vectors that qualify its views. This page summarises
[`spec/package.md`](../records/spec/package), which is normative, with JSON
Schemas in `spec/schema/`.

Packages are local directories; `bb.lock` pins their content. There is no
registry, no signing and no `bb pkg publish` in Phase 1.

## Layout and search path

```text
packages/                         first-party packages, in breadboard.dev
  generic/resistor/               package id "generic/resistor"
    part.toml
    pins.toml
    analog/resistor.lib           view files
    tests/                        test vectors
  st/stm32f103c8/
```

- **Package id:** `<namespace>/<name>`, lower case. The namespace is the
  manufacturer, or `generic`.
- **One directory is one package version.** The version is in `part.toml`.
- **Search path:** a project's `[packages] paths`, in order, then the
  first-party `packages/`. The first directory that provides an id wins, so a
  private package shadows a first-party one.

The first-party packages today: `generic/resistor`, `generic/capacitor`,
`generic/inductor`, `st/stm32f103c8`, `st/stm32f072rb`, `nordic/nrf52840-qiaa`,
`raytac/mdbt50q-u1mv2` (a module), `ti/tps22917` and `worldsemi/ws2812c-2020`.

## `part.toml` (`bb-part/0`)

| Key | Required | Meaning |
| --- | --- | --- |
| `schema` | yes | `"bb-part/0"` |
| `[part] id`, `version` | yes | The package id and `MAJOR.MINOR.PATCH`. |
| `[part] kind` | yes | `soc`, `module`, `power`, `analog_ic`, `logic`, `discrete`, `passive`, `connector`, `mechanical`, `testpoint` |
| `[part] mpn`, `mpn_patterns`, `lcsc` | no | Keys for resolving a board's parts to this package. |
| `[part] license` | yes | An SPDX expression. `LicenseRef-local-only` forbids committing the package and is refused inside `packages/`. |
| `[part] manufacturer`, `datasheet`, `description` | no | Informative. |
| `[footprints]` | no | KiCad footprint ids, and a pad-to-pin map when they differ. |
| `[pins] file` | yes | The `pins.toml` file. |
| `[module] chip`, `pinmap` | modules only | The chip package inside, and a pad-to-chip-pin map. |
| `[behavioral]` | no | At most one behavioural layer. |
| `[[analog]]` | no | Analog views. |
| `[provenance]` | no | The default provenance for views without their own. |
| `[tests] dir` | no | The test-vector directory. |

Unknown keys are an error. Some keys are reserved for `bb-part/1` and refused
with a message saying so.

### Layers

- **`pins`** (always): each pin's role and allowed pin models.
- **`behavioral`** (at most one): `engine = "renode"` with
  `platform = "<name>@<major>"` (the part is the CPU engine's machine, on a
  platform identity that `bb` provides), or `engine = "device"` with a
  pin-level device compiled into `bb`.
- **`analog`** (zero or more views): each has a `name`, `kind` (`circuit` or
  `pin_load`), the `fidelity` rungs it is valid for, the rungs it is the
  default for, a SPICE `file` and `subckt`, `ports`, default `params`, the
  qualifications written by `bb pkg test --record`, and a `provenance` block
  (see [Fidelity and honesty](../concepts/fidelity)).

A view file defines and never instantiates: only comments, `.subckt` blocks,
`.model`, `.func` and `.param` lines, with no `.include` or `.lib`. Views are
inlined into one deck; byte-identical definitions of a name are emitted once,
and different definitions of one name are an error.

## `pins.toml` (`bb-pins/0`)

```toml
schema = "bb-pins/0"

[[pin]]
number = "15"                 # footprint or chip pin number, as KiCad reports it
name = "PA1"                  # KiCad libpart pin names must match
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

Every pin number is listed exactly once. A parameter value is a number in the
value grammar, or for a voltage a supply pin name (`VDD`) or a factor of one
(`0.7*VDD`); there is no expression language. Supply references resolve at
elaboration against the bench. See [Pin models](./pin-models) for the
parameters of each model.

On Breadboard's nRF52840, a `digital_out` set's name begins with the
`PIN_CNF` drive it stands for (`S0S1`, `H0H1`, ...), and `bb run`'s `PIN_CNF`
audit checks the firmware's drive against it.

## Values

One grammar for every numeric string in `part.toml`, `pins.toml`, test vectors
and benches:

- a decimal number, optionally signed, optionally with an exponent;
- an optional SI prefix: `f p n u µ m k Meg G T` (`m` is milli, `Meg` is mega);
- **a bare `M` is an error** that suggests `Meg` or `m`;
- an optional unit, checked against the parameter's dimension.

Each value is parsed exactly into an integer in its dimension's sub-unit (µV,
fs, µΩ, aF, fH, pA, m°C, ppm), and a value that is not a whole number of
sub-units is an error, never rounded. `bb pkg value` shows how a string
parses.

KiCad value fields of generic passives follow EE convention instead (`4k7`,
`10u/25V`, `M` as mega), with tolerance, ratings and dielectric kept as
metadata. Anything else is a parse error, never a guess.

## Generic passives

`generic/resistor`, `generic/capacitor` and `generic/inductor` have two
passive pins and one view, `ideal`. They are selected by footprint plus value
(footprint library `Resistor_*`, `Capacitor_*`, `Inductor_*`, or reference
prefix `R`, `C`, `L` on a custom footprint). A package for a specific MPN
overrides them.

## Option sets, test vectors and qualification

Named solver option sets include `phase0`
(`.options RELTOL=1e-5 VNTOL=1e-8 ABSTOL=1e-13`, the default for imported
boards and for vectors) and `suite_fallback`; the spec lists them all. A
view's test vectors run through the real ngspice engine (`bb pkg test`), at
two grant partitions that must agree. `bb pkg test --record` records each
(lattice, option set) pair a view passes as a qualification.

## `bb.lock`

`bb open` writes `bb.lock` next to a board file or in a project: each
package's version and tree hash, and each engine platform's content hash
(the platform's `.repl` and Breadboard's Renode plugin sources). `bb run`
refuses a mismatch and never writes the lock. After an intended change to a
package or platform, run `bb open --update-lock` (`make locks` for every
example) and review the diff.

## Parts from packages in a board file

A hand-written `board.cir` can take a part from a package instead of writing
every pin parameter out:

```text
*bb part U1 package=st/stm32f103c8
*bb pin U1.PA5 net=PA5 model=digital_out
*bb pin U1.PB0 net=SENSE model=digital_in set=default
*bb pin U1.PA6 net=PA6 model=digital_out zout=50       # overrides the set (lint warning)
```

The engine and platform come from the package's `[behavioral]` layer. Its
search path is the first-party directory alone, and it needs a current
`bb.lock` next to it (`bb open <board.cir>`). The Phase 0 form, with
`engine=renode platform=...` and every parameter written out, still works and
needs no lock.

## Engine platforms

The Renode platforms `bb` provides (`cli/src/platforms.rs`):

| Platform | Loads | Board-file name |
| --- | --- | --- |
| `stm32f103-offline@1` | `engines/cpu-renode/platforms/stm32f103-offline.repl` | `stm32f103` |
| `stm32f072-bb@1` | `engines/cpu-renode/platforms/stm32f072-bb.repl` | `stm32f072` |
| `nrf52840-bb@1` | `engines/cpu-renode/platforms/nrf52840-bb.repl` | `nrf52840` |

A behaviour change bumps the major and changes the content hash. The lock
records Renode's version (1.17.0) with each Renode platform.
