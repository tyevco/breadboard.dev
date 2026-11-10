# Projects and benches

A **project** is a directory with a `bb.toml`. `bb open` turns it into
`board.bbm` and `bb.lock`; `bb run` runs it with a **bench**. This page
summarises [`spec/project.md`](../records/spec/project) and
[`spec/board-bbm.md`](../records/spec/board-bbm), which are normative.

In every one of these files, unknown keys are an error, and every number is a
string in the [value grammar](./packages#values) (`"3.3V"`, `"50mA"`,
`"44.167mV"`).

## `bb.toml` (`bb-project/0`)

```toml
schema = "bb-project/0"
[project]
name = "dial-me-spinner rev A"
simulate = "main"                    # required with more than one [[board]]

[[board]]
id = "main"                          # [a-z0-9_-]+
netlist = "kicad/main/x.net"         # exactly one of netlist = (a KiCad .net) ...
# kicad = "kicad/main/x.kicad_pro"   # ... or kicad = (exported with kicad-cli)
ground = "GND"                       # default GND

[packages]
paths = ["packages"]                 # searched before the first-party packages

[instances."main:J11"]               # an override (resolution stage 1)
package = "none"                     # a package id[@version], or none with a reason
reason = "NFC coil pads: no RF model in Phase 1"

[targets."main:U10"]                 # written by bb flash
images = [{ file = "firmware/prebuilt/led_rail.elf", kind = "elf", hash = "sha256:…" }]

[[bind]]                             # an MCU pin across the engine boundary
pin = "main:U10.P1.04"
model = "digital_out"                # digital_out | digital_in | adc_in
set = "S0S1"                         # the pins.toml parameter set (default: default)
net = "LED_PWR_EN"                   # optional: checked against the netlist

[probes]
nets = ["VLED", "LED_DIN"]
```

(Abridged from the spec's example. The spec also defines `[import] fields`,
per-instance `analog` view overrides and `[[connection]]` for two-board
systems; system import is [roadmap](../guide/roadmap) milestone M11.)

- **Bindings.** A `[[bind]]` takes its pin model and parameters from the
  target package's `pins.toml`; `set` picks a parameter set, and explicit
  keys override it. A connected MCU pin **without** a binding is unbound: no
  glue, no load (high-Z).
- **Module targets.** A binding may name a module pad as the board's symbol
  does (`main:U10.P1.04`); the module's pinmap leads to the chip pin, which
  gives the pin model and the CPU engine's pin.
- **Targets.** A list of typed images. Phase 1 implements exactly one image
  of `kind = "elf"` on one MCU of the simulated board.

## Names

Every key, CLI argument, probe and bench entry that names an instance, a pin
or a net resolves by these rules; an ambiguous name is an error that lists the
candidates.

- **Instances:** `<board>:<ref>` always; a bare `<ref>` only if exactly one
  board has it.
- **Pins:** `<instance>.<pin name>` (`main:U10.P0.06`, `U1.PA4`).
- **Nets**, the first unique match wins: the exact display name; the
  board-qualified name `<board>:<KiCad name>`; a unique leaf name (after the
  last `/`). In the demo, `net:LED_DIN` resolves to `/LED ring/LED_DIN` as
  the unique leaf name.
- **Probe aliases** (test points): `main:TP1`, or `TP1` when only one board
  has it.
- **A probe** is a net or an alias, else a bound MCU pin.

## Benches (`bb-bench/0`)

```toml
schema = "bb-bench/0"
temp_c = "-10"                       # ngspice .temp
[[supply]]
net = "VSYS"
volts = "3.0V"
rs = "0.569"                         # optional series resistance
[[supply]]
net = "+3V3"
volts = "3.3V"
track = { net = "VSYS", drop = "44.2mV" }   # min(volts, V(net) - drop)
[[load]]
net = "VSYS"
amps = "50mA"                        # a DC sink to ground
[nominal]
"+3V3" = "3.3V"                      # supply references on rails no bench source drives
[[pwl]]
net = "X"
points = [["0", "0V"], ["1ms", "3.3V"]]     # optional rs
[analog]
nets = ["VSYS", "+3V3", "VLED", "GND"]      # the analog scope
exclude = ["U1"]
[options]
set = "phase0"                       # a named option set
```

- **Analog scope.** An instance enters the ngspice deck if and only if its
  status is `ok`, it is not the target MCU, it is not in `exclude`, it has an
  analog view at the run's fidelity, and every connected pin is on a net of
  `nets`. Without `[analog] nets` every net is in scope and `bb run` warns.
  Out-of-scope instances and the reason are listed in `run.json`.
- **Supplies.** A `[[supply]]` without `rs` is a voltage source; with `rs` it
  is written as its Norton equivalent, one conductance with no
  branch-current unknown (ADR-007 Amendment 3).
- **No bench.** `bb run` without `--bench` uses an empty bench: no sources,
  every net in scope, option set `phase0`.
- **Option set.** It must be one every in-scope view is lattice-qualified for,
  unless `--allow-unqualified` names the view.

A fuller bench (CSV playback, timed actions, assertions) is a Phase 2 item,
to be defined in its own ADR.

## The elaborated board (`board.bbm`)

`board.bbm` is the imported board in **canonical JSON**: instances, their
resolved packages and nets. It is a function of the content only: source
paths, source hashes, tool strings and dates go to `.bb/import.json`, which is
not committed. So the committed netlist, a fresh KiCad 10 export and the
schematics give the same `board.bbm`. Example projects commit `board.bbm` and
`bb.lock`.

## The elaborated deck (`deck.cir`)

`bb run` builds the analog deck from `board.bbm`, the bench and the locked
packages, and writes it to `<out>/deck.cir` for review: a header comment;
every view definition once, in sorted order; one instance line per in-scope
instance; the bench's sources, loads and `.temp`; and the option set's
`.options` card. Every number is a plain SPICE number (`4.7e-7`), never a
letter suffix, and the deck contains no file paths.
