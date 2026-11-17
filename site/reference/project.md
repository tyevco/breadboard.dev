# Projects and benches

A **project** is a directory with a `bb.toml`. `bb open` turns it into
`board.bbm` and `bb.lock`; `bb run` runs it with a **bench**. This page
summarises [`spec/project.md`](../records/spec/project) and
[`spec/board-bbm.md`](../records/spec/board-bbm), which are normative.

In every one of these files, unknown keys are an error, and every number is a
string in the [value grammar](./packages#values) (`"3.3V"`, `"50mA"`,
`"44.167mV"`).

## `bb.toml` (`bb-project/1`)

```toml
schema = "bb-project/1"
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

[instances."main:U10"]
power_mode = "ldo"                   # the supply mode its idd view draws at pi (default: the part's first)

[targets."main:U10"]                 # written by bb flash
images = [{ file = "firmware/prebuilt/led_rail.elf", kind = "elf", hash = "sha256:…" }]

[[bind]]                             # an MCU pin across the engine boundary
pin = "main:U10.P1.04"
model = "digital_out"                # digital_out | digital_in | adc_in
set = "S0S1"                         # the pins.toml parameter set (default: default)
net = "LED_PWR_EN"                   # optional: checked against the netlist

[probes]
nets = ["VLED", "LED_DIN"]

[[device]]                           # a device part, here one with no schematic symbol
ref = "main:M1"
package = "dms/detent-mechanism"     # its [behavioral] engine is "device"
params = { s0 = "500", ts = "50ms" }
virtual = true

[[device.bind]]
pin = "HOME_N"
net = "HOME_N"
model = "digital_out"                # digital_in | digital_out
```

(Abridged from the spec's example. The spec also defines `[import] fields`,
per-instance `analog` view overrides and `[[connection]]` for two-board
systems, below.)

- **Bindings.** A `[[bind]]` takes its pin model and parameters from the
  target package's `pins.toml`; `set` picks a parameter set, and explicit
  keys override it. A connected MCU pin **without** a binding is unbound: no
  glue, no load (high-Z).
- **Module targets.** A binding may name a module pad as the board's symbol
  does (`main:U10.P1.04`); the module's pinmap leads to the chip pin, which
  gives the pin model and the CPU engine's pin.
- **Targets.** A list of typed images. Phase 1 implements exactly one image
  of `kind = "elf"` on one MCU of the simulated board.
- **Supply modes.** At `pi` a part's `supply_load` view draws the currents
  of one of its `[power] modes`: the first (the part's reset state) unless
  `[instances."<ref>"] power_mode` names another. The nRF52840's modes are
  its regulator, `ldo` (the default) and `dcdc`; the mode must match the
  firmware's, and `bb run` fails a run whose firmware sets `POWER.DCDCEN`
  differently. `run.json` records the mode.
- **Devices** (`bb-project/1`). A `[[device]]` runs a device package (a
  mechanism, a switch) in the device engine beside the MCU, with a board
  file's device rules. A device on a schematic symbol is that instance; a
  `virtual` one (no symbol) is written into `board.bbm` as an instance with
  `virtual: true`. Its bindings route through the kernel: the MCU reads a
  device's output, and a device the MCU's, directly.
- **Digital-only nets.** A bound digital net outside the bench's `[analog]
  nets` gets no glue: its pins meet only through the kernel, so the analog
  engine neither drives nor reads it. `examples/dial-me-spinner/e3/home`
  runs E3's homing that way.

## Systems (`[[connection]]`)

A project with more than one `[[board]]` is a system (Phase 1 M11). The
example is `examples/dial-me-spinner/system`: the main board and the base
board, with a 12-wire harness from main J1 to base J3.

`simulate` names the board, or the boards, that are simulated: one board
id (`simulate = "main"`), or a list of them. A list of several is a
**single-MCU system run** (Phase 2 R2, ADR-013 D2; below):
`examples/dial-me-spinner/e1-system` (E1) and `examples/dial-me-spinner/w1`
(W1) run the main and base boards in one deck with
`simulate = ["main", "base"]`.

```toml
[project]
simulate = "main"                    # one board; or a list: ["main", "base"]

[[board]]
id = "main"
netlist = "../kicad/main/dial-me-spinner-main.net"
[[board]]
id = "base"
netlist = "../kicad/base/dial-me-spinner-base.net"

[[connection]]
a = "main:J1"
b = "base:J3"
model = "dial-me-spinner/link-harness:wires"   # optional: the harness view
# pins = [["1", "1"], ...]                     # optional: the mated pins
```

- **Names are qualified.** Instances, pins, test points and nets are
  `<board>:<name>` (`main:U1`, `base:TP1`, `main:VSYS`). The two
  dial-me-spinner boards share 15 references, so a bare `U1` is an error
  that lists `main:U1 (RT9080-33GJ5)` and `base:U1 (BQ25180YBGR)`. Each
  board keeps its own ground: the primary simulated board's ground is
  SPICE node `0`, and another board's ground is its net's SPICE name
  (`base_GND`).
- **The harness view** is `<package>[@<version>][:<view>]`. It is a
  `circuit` view of a `connector` package, and its ports are `A<pin>` (the
  end on `a`'s pin) and `B<pin>` (the end on `b`'s). It is locked in
  `bb.lock`.
- **The mated pins** are `pins` if given; else the harness's wires (`A<n>`
  with `B<n>`); else pin n to pin n, when both ends have the same pin
  numbers. `bb open` prints every pair with its two nets and warns when
  their leaf names differ. `board.bbm` records the complete list.
- **Only the boards of `simulate` are simulated.** A bench net on another
  board is an error. At a connector to a board that is not simulated, nets
  that no bench `[[supply]]` or `[[pwl]]` drives are left open, with a
  warning.

### System runs (`simulate` as a list)

A single-MCU system run (ADR-013 D2, Amendment 3; `spec/project.md`
sections 1 and 4) builds one analog deck from every listed board:

- **One MCU.** The **primary board** is the target's: the board a
  `[targets]` key names, else the first listed. Exactly one target across
  the listed boards; a second is refused (multi-MCU simulation is Phase 3).
  A `[[device]]` of a system run is on the primary board.
- **Grounds.** The primary board's ground is node `0`; every other board
  keeps its own ground node (`base_GND`). A bench element returns to its
  net's board's ground, and a B-source term reads its net against it.
- **Connections.** A `[[connection]]` between two simulated boards with a
  harness view is one instance line in the deck (`X_conn1_main_J1_base_J3`,
  its `A<pin>` ports on `a`'s nets and `B<pin>` on `b`'s). Without a
  harness, each mated pair's nets are tied into one node. In the
  dial-me-spinner system the harness's three GND wires join `base_GND` to
  node `0` through the wires' resistance.
- **Names.** "The simulated board" in the name rules is every listed board,
  so a leaf name on two of them is ambiguous: a bench, a binding or a probe
  qualifies it (`main:VSYS`, `base:VBAT`).
- **`run.json`** records `simulated` (the boards of `simulate`, as listed)
  only in a system run, and each harness in the deck in `analog` with
  `kind = "harness"`.

The examples: `examples/dial-me-spinner/e1-system` (E1 with the base
board's own power path: a bench `[[battery]]`, the BQ25180 and the harness
in place of E1's bench supplies; `make dms-e1-system-test`) and
`examples/dial-me-spinner/w1` (W1 at `pi`; `make dms-w1-test`).

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

The Phase 2 bench, `bb-bench/1` (ADR-010), adds CSV playback, current
profiles, timed and triggered actions, faults, and assertions: `[[assert]]`
tables of a closed set of kinds (`range`, `at`, `settle`, `edge_timing`,
`count`, `frequency`, `log`, `event`), in a bench or in an assertion file
given to `bb run --assert`. The kernel evaluates them at every accepted
solver point and on the edge and log records, and writes one `assert`
record per assertion; `bb assert <run>` evaluates the record-only kinds
again on a finished run. The normative text is section 3 of
`spec/project.md` in the repository.

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
