# Projects (`bb.toml`), benches, names and the CLI verbs

Status: normative. Decided by ADR-007 D5 and D7 (accepted 2026-11-09;
implemented in Phase 1 M4, ADR-007 Amendment 1; module targets, the image
hash of ADR-008 D5 and Norton bench supplies in M6, ADR-007 Amendment 3;
systems in M11, ADR-007 Amendment 5; devices and digital-only nets in
Phase 2 R1, ADR-013 D1 and Amendment 1, with ADR-014 D4's project rule;
single-MCU system runs in Phase 2 R2, ADR-013 D2 and Amendment 3).
Implementation:
`cli/src/project.rs`, `names.rs`, `import.rs`, `elab.rs`, `open.rs`,
`flash.rs`, `probe_cmd.rs`. JSON Schemas: `spec/schema/project.schema.json`
(`bb-project/1`) and `spec/schema/bench.schema.json` (`bb-bench/1` and `/0`); the TOML
files are converted to JSON as they are and validated (`make packages`).
**Changing this file requires an ADR.**

A project is a directory with `bb.toml`. `bb open` turns it into
`board.bbm` (`spec/board-bbm.md`), `bb.lock` (`spec/package.md`) and the
files of `.bb/`; `bb run` runs it. Unknown keys are an error. Every number
is a string in the value grammar of `spec/package.md` section 6 (ADR-006
D5).

## 1. `bb.toml` (`bb-project/1`)

`bb-project/1` adds `[[device]]` (ADR-013 D1), and with the Phase 2 schema
epoch (milestone S) `simulate` as a list (ADR-013 D2), `[run]` (ADR-013 D3)
and `[registries.<name>]` (ADR-011 D4). A `bb-project/0` file is the
previous major (ADR-006 D11): `bb` reads it with a migration hint, and its
migration is the schema line only (`bb pkg migrate --project`);
`[[device]]` and `[registries]` in it are errors.

- **`simulate`** is a board id, or a list of them (each a declared
  `[[board]]`, none twice). A list of one board means that board. A list
  of several is a **single-MCU system run** (ADR-013 D2, milestone R2):
  every listed board is in one analog deck (section 4). Its **primary
  board** is the target's, the board a `[targets]` key names by its
  qualifier when one names a listed board, else the first listed; its
  ground is node 0, and `bb run` refuses a run whose target is on another
  board. Exactly one target across the listed boards: a second is refused
  ("multi-MCU simulation is Phase 3"). A `[[device]]` of a system run is on
  the primary board.
- **`[run]`** (`cpu = "none"`, a run without a CPU target, and `lattice`,
  the engine-neutral name of the grant lattice) is milestone R3's, which is
  not in this build: it is refused, naming the milestone, so a project that
  names it never runs other than as written.
- **`[registries.<name>]`** (`url`, `https://` or `file:///`; `key`, the
  registry's minisign public key; `namespaces`, at least one, each routed
  to one registry only): a private registry (ADR-011 D4, milestone E1). An
  id of a routed namespace that no local source provides resolves on that
  registry only, never on another one or the public one; every other
  namespace resolves on the public registry (`spec/registry.md` section 6).
  Only `bb open` fetches.
- A binding (`[[bind]]`, `[[device.bind]]`) may also set the `bb-pins/1`
  `digital_out` parameters `rail` and `rhiz` (`spec/package.md` section 4),
  unused at `fast`.

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

[import]
fields = ["Value2"]                  # fields read besides MPN, Manufacturer, LCSC, bb:part

[instances."main:J11"]               # an override (resolution stage 1)
package = "none"                     # a package id[@version], or none with a reason
reason = "NFC coil pads: no RF model in Phase 1"
# analog = "lsw"                     # the view to use (ADR-006 D3 selection step 1)

[targets."main:U10"]                 # written by bb flash
images = [{ file = "firmware/prebuilt/led_rail.elf", kind = "elf", hash = "sha256:…" }]

[[bind]]                             # an MCU pin across the engine boundary
pin = "main:U10.P1.04"
model = "digital_out"                # digital_out | digital_in | adc_in
set = "S0S1"                         # the pins.toml parameter set (default: default)
net = "LED_PWR_EN"                   # optional: checked against the netlist
# voh = "3.3V" ...                   # explicit parameters override the set

[[bind]]                             # the nRF52840's SAADC (ADR-008 D7)
pin = "main:U10.P0.02"
model = "adc_in"
net = "VSYS_SENSE"
# load = "none"                      # the pin_load view (the set's default), or none: an ideal input
trace_samples = true                 # a boolean: every sample becomes a log record

[probes]
nets = ["VLED", "LED_DIN"]

[[device]]                           # a device part (ADR-013 D1; spec/package.md section 12)
ref = "main:M1"                      # an instance of the board, or (virtual) a new reference
package = "dms/detent-mechanism"     # a package whose [behavioral] engine is "device"
params = { s0 = "500", ts = "50ms" } # the device's parameters (value grammar)
virtual = true                       # no schematic symbol (default false)

[[device.bind]]                      # a device pin on a net
pin = "IN_A1"                        # the device package's pin name
net = "MOT_A_IN1"                    # by the name rules (section 2)
model = "digital_in"                 # digital_in | digital_out
# set = "default", vih = "1.5V" ...  # the package's set and explicit overrides, as [[bind]]

[[connection]]                       # systems (M11): mated connectors
a = "main:J1"
b = "base:J3"
model = "dial-me-spinner/link-harness:wires"   # optional: the harness view
# pins = [["1", "1"], ...]           # optional: the mated pins (a, b)
```

- **Bindings.** The pin model and its parameters come from the target
  package's `pins.toml` (`models` lists what each pin allows); `set` picks
  a parameter set; explicit keys override it (a warning when they differ).
  A supply reference (`voh = "VDD"`) resolves to the bench `[[supply]]` on
  the net that supply pin is on, then to the bench `[nominal]` entry for
  that net; otherwise it is an error. A bound pin on an `unconnected-(…)`
  net is a `digital_out` that drives nothing (probe-only, glue rule 4).
  A connected MCU pin **without** a binding is unbound: no glue, no load
  (high-Z), its net probe-only on the MCU side.
- **The nRF52840's SAADC** (ADR-008 D7, Amendments 4 and 5): an `adc_in`
  takes `load` (the package's `pin_load` view, or `"none"`) as a parameter
  and `trace_samples` (a boolean) as a binding option, as a board file's
  `*bb pin` does; an `adc_in` on the chip's supply pin `VDD` is the SAADC's
  VDD input. On a module target the pad must list the model it is bound
  with (its `models`), except that VDD input.
- **A module target** (`spec/package.md` section 4; Phase 1 M6, ADR-007
  Amendment 3): a binding names the module pad as the board's symbol does
  (`main:U10.P1.04`); the pad's chip pin (the module's pinmap) gives the
  pin model, its parameter sets and the CPU engine's pin; a supply
  reference names a chip pin and resolves to the net of the module pads
  that map to it; the behavioural layer is the chip's.
- **The `PIN_CNF` audit** (nRF52840, ADR-008 D4): every bound pin, with the
  drive its `set` names (`spec/package.md` section 4), and every unbound
  GPIO of the target whose net is in the bench's analog scope.
- **Devices** (`[[device]]`, ADR-013 D1; Phase 1's route B). Every rule of
  `spec/package.md` section 12 applies as in a board file: the package's
  `[behavioral]` engine is `device` and names a device `bb` provides; its
  parameters are the device's keys in the value grammar (an unknown key, a
  wrong dimension or a value out of range is an error); a pin is a pin of
  the device package, `digital_in` on a device input and `digital_out` on
  an output (`adc_in` is refused); the routing through the kernel net
  (a device output read by the MCU or another device, the MCU's output
  read by a device, with no threshold and no `cin` glue) and the glue
  rules on the analog side are section 12's; one driver per net; the
  device identity is locked as `device:<id>@<major>` in `bb.lock` and
  listed in `board.bbm`'s `platforms`. The kernel net of a device pin is
  `<ref>.<pin>` without the board (`M1.HOME_N`), as the target MCU's is.
  - `ref` is `<board>:<ref>`, or a bare `<ref>` in a project with one
    board; the device must be on the simulated board (in a system run,
    the primary board).
  - **On a schematic symbol** (`virtual` false): the instance must exist
    and resolve to `package` (resolution stage 1, an `[instances]`
    override or its `bb:part` field), and each binding's net must be the
    net that pin is on.
  - **Virtual** (`virtual = true`): a physical thing with no symbol (a
    mechanism, a magnet, a motor's load). `ref` must not be an instance
    of the board. `bb open` writes it into `board.bbm` as an instance with
    `virtual: true` (`spec/board-bbm.md` section 3), its pins the device
    package's, each on the net its binding names, and adds those pins to
    the nets' `nodes`: a device binding is board content.
  - A device instance is never in the analog deck. `bb open` prints each
    device and reports a binding it cannot honour; `bb run` refuses it.
  - Device parameters are not board content: they are in `bb.toml` and
    `run.json` (`devices`, section 6), like an MCU binding's parameters.
- **Targets** are a list of typed images; Phase 1 implements exactly one
  image of `kind = "elf"` on one MCU of the simulated board (one across
  the boards of a system run). `bin`, `hex`,
  `uf2` and `flash` are reserved. An `elf` image's `hash` is its **image
  hash** (ADR-008 D5): `sha256:` and the SHA-256 of the 15 bytes
  `bb-elf-image/0\n`, then for each `PT_LOAD` program header with
  `p_filesz > 0`, sorted by `p_paddr` (then `p_offset`), `p_paddr` and
  `p_filesz` as u64 little-endian and the segment's `p_filesz` file bytes,
  then `e_entry` as u64 little-endian. Debug information and section
  headers are not part of it, so two builds that load the same bytes have
  one hash.
- **Systems** (more than one `[[board]]`; ADR-007 D5, Amendment 5).
  Instances, pins, test points and nets are qualified `<board>:<name>`
  (section 2), and each board keeps its own ground. A `[[connection]]`
  joins two instances on two different boards:
  - `model` (optional) is a harness view, `<package>[@<version>][:<view>]`
    (without a view, the package's `fast` default). The package must be of
    kind `connector` and the view a `circuit` view. Its ports are `A<pin>`
    for the end on `a`'s pin and `B<pin>` for the end on `b`'s, exactly
    the mated pins' ends. The package is locked in `bb.lock` like an
    instance's.
  - The mated pins are `pins`, each pin existing and mated at most once on
    each side. Without `pins` and with a harness, its wires (`A<n>` with
    `B<n>`; a harness whose two ends differ needs `pins`). Without either,
    pin n to pin n over every pin number, which both ends must have alike.
  - `bb open` prints each mated pair with its two nets. It warns about a
    mated pin on no net and about mated nets whose leaf names differ.
  - Only the boards `[project] simulate` lists are simulated. A bench name
    on another board is an error. The nets at a simulated connector whose
    other end is not simulated are bench ports: a `[[supply]]` or `[[pwl]]`
    drives one, and `bb run` warns about the rest as left open (not the
    simulated ground). A connection between two simulated boards is in the
    deck (section 4): through its harness view, or without one its mated
    nets tied into one node.

## 2. Names (ADR-007 D7)

Every `bb.toml` key, CLI argument, probe and bench entry that names an
instance, a pin or a net resolves by these rules; an ambiguous name is an
error that lists the candidates.

- **Instances:** `<board>:<ref>` always; a bare `<ref>` only if exactly one
  board has it (`U1 is ambiguous: main:U1 (RT9080-33GJ5), base:U1
  (BQ25180YBGR)`).
- **Pins:** `<instance>.<pin name>` (`main:U10.P0.06`, `U1.PA4`).
- **Nets**, the first unique match wins: the exact display name; the
  board-qualified name `<board>:<KiCad name>`; a unique leaf name (after
  the last `/`), within the simulated board first, then the project. In a
  system run (ADR-013 D2) "the simulated board" is every listed board: a
  leaf name on two of them is ambiguous, so a bench, a binding or a probe
  qualifies it (`main:VSYS`, `base:VBAT`).
- **Probe aliases** (test points) follow the instance rule: `main:TP1`, or
  `TP1` when only one board has it.
- **A probe** is a net or an alias, else a bound MCU pin
  (`U1.PA4`, whose digital level is the kernel net `U1.PA4`, as in Phase
  0).

## 3. Benches (`bb-bench/1`, `bb-bench/0`)

A bench is TOML (ADR-010 D1). `bb` reads `bb-bench/0` and `bb-bench/1` and
writes `/1`; every `/0` key keeps its meaning in `/1`, so `bb pkg migrate
--bench <file>` rewrites only the `schema` line and a migrated bench builds a
byte-identical deck. A `/0` file has none of the `/1` keys.

### 3.1 The `/0` keys

```toml
schema = "bb-bench/1"                # or "bb-bench/0"
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
set = "phase0"                       # a named option set (spec/package.md)
```

- **Analog scope.** An instance enters the ngspice deck if and only if its
  status is `ok`, it is not the target MCU, it is not in `exclude`, it has
  an analog view at the run's fidelity (ADR-006 D3, with the ladder), and
  every connected pin is on a net of `nets`. Without `[analog] nets` every
  net is in scope and `bb run` warns. Out-of-scope instances and the
  reason are listed in `run.json`.
- **Two drivers.** A `[[supply]]` on a net that an in-scope part's
  `power_out` pin drives is an error unless that part is excluded.
- **Digital-only nets** (ADR-014 D4, ADR-013 D1). A net that a
  `digital_out` or `digital_in` binding (an MCU's or a device's) is on,
  and that is not in `nets`, is digital-only: it gets no glue, and the
  analog engine has no input or output on it. Its pins are routed through
  the kernel net only (section 12 of `spec/package.md`), so every
  `digital_in` on it must read a digital output that a device drives or
  that a device reads; anything else is an error that names the net.
  Elements on it leave the deck by the scope rule above. A probe of such
  a net by name is an error that names its driver's pin (the column is the
  pin's). `run.json` lists the digital-only nets. Without `[analog] nets`
  every net is in scope and nothing changes.
- `bb run` without `--bench` uses an empty bench: no sources, every net in
  scope, option set `phase0`.
- **`[[pwl]]` is for short tables** (an inline PWL card is O(n²) in
  ngspice): `bb run` warns above 1 000 points and refuses above 10 000;
  `[[playback]]` is the route for long ones.

### 3.2 The `bb-bench/1` stimulus (ADR-010 D2-D4)

```toml
schema = "bb-bench/1"
seed = 1                             # declared random elements (bounce); default 1

[[load]]
net = "+3V3"
profile = "data/burst.csv"           # t,amps; instead of amps
interp = "hold"                      # linear (default) | hold

[[playback]]
net = "AIN"                          # an analog net: an EXTERNAL source
file = "data/ain.csv"                # t,volts (section 3.3)
rs = "100"                           # optional; without it an ideal source
interp = "linear"                    # linear (default) | hold
[[playback]]
net = "U1.PB0"                       # a bound digital_in pin: a digital stream (t,level)
file = "data/btn.csv"

[[action]]
press = "SW1"                        # an instance with a switch view (spec/package.md section 3)
at = "120ms"
for = "60ms"                         # optional: released (or set back) after it
bounce = { count = 3, max = "1ms" }  # a press only; max defaults to the view's bounce_max
[[action]]
set = "VSW"                          # a [[supply]]'s net (to: a voltage), or a bound
to = "2.5V"                          #   digital_in pin (to: "0" or "1")
after = { edge = "U1.PA5", level = 1, delay = "10ms" }   # a trigger instead of at

[[fault]]
kind = "open"                        # the instance pin leaves the net
net = "BTN"
pin = "C1.1"
at = "150ms"
for = "10ms"                         # optional: the fault ends after it
[[fault]]
kind = "short"                       # the net shorts to the board's ground
net = "AFILT"
at = "260ms"
```

- **Tables, handed over once.** Every change the bench knows before the
  run (a profile, an analog playback, a timed press or set, a fault) is a
  table of the analog engine's `EXTERNAL` sources, served by its callbacks
  with a cursor (O(1) per solver point), never a PWL card. No analog value
  crosses the kernel boundary per solver point.
  - `linear` interpolates between points; `hold` keeps each point's value
    until the next point, left-continuous (at its time the old value, after
    it the new one). Before the first point the first value holds, after the
    last the last.
  - Every point after `t = 0` is a corner the solver's steps are clamped to
    land on. The points of a `hold` table and of a table of at most 1 000
    points are also ngspice breakpoints (so such a playback gives the result
    of the same points as a `[[pwl]]`, ADR-010 G2); a longer linear table's
    points are not, since a breakpoint restarts the integration with short
    steps and the lattice clamp already puts a solver point on every
    lattice point (ADR-010 Amendment 1). A breakpoint is released at the
    accepted solver point that opens the lattice cell before the one
    holding it, a pure function of the solver points.
- **The bench engine** (`bench`, identity `bench@1`, ADR-010 D4) runs when
  the bench drives an MCU pin (a digital playback or a `set` on a bound
  `digital_in` pin) or has a triggered action. It is an ordinary engine:
  `t_safe = Time::MAX` (every change is in its trace), an exact
  `quiet_until` (its next scheduled change), an early stop on the lattice.
  It is registered first, in both engine orders.
  - Into an MCU pin it drives the pin's kernel net (`U1.PB0`) instead of
    the analog engine's threshold: the MCU applies each edge at `ceil_L`,
    like any edge, and the circuit net is not read. The pin starts low,
    unless its playback has a point at `t = 0`.
  - A triggered action (`after`) makes the bench a consumer of the MCU
    output pin it names: it fires once, at the first change of the pin to
    `level` after `t = 0`, at `t_edge + delay`. What it drives in the
    circuit (a switch control, a supply's set source) is then the bench
    engine's output, an analog input of the analog engine that changes at
    its own time by a 1 ns ramp (`Drive::Timed`). Such a bench engine sits
    between the MCU and the analog engine in a feedback loop, where the
    kernel finds no quiet grant (ADR-003 D5), so the run's grants are one
    lattice cell.
- **Actions.** `press` closes a switch (control 1) at `at` (or its
  trigger) and opens it after `for` (else it stays closed); a press on an
  instance without a switch view in the deck is refused, naming its views.
  `bounce = { count, max }`: `2 × count` seeded toggles inside `(0, max)`
  after the press and after the release (ending in the new state), from
  `seed`, the switch and the action; `max` at most the view's `bounce_max`
  and shorter than `for`. `set` drives the supply to `to` (the supply's
  voltage becomes the set source's node; an ideal fixed supply becomes the
  source) or the pin to the level, and back to the bench's own value after
  `for`. Timed actions on one target may not overlap.
- **Faults** are the switch element (`bb_switch` at the default `ron` and
  `roff`) switched by a table: `open` moves the instance pin to its own
  node `bb_fault<k>`, connected to the net while closed (before `at`);
  `short` connects the net to the ground from `at`. With `for`, the fault
  ends after it.
- **The rung and the corners** (ADR-009 D1, D8; milestone P1):
  - `fidelity = "fast" | "pi"`: the run's rung; `bb run --fidelity` wins,
    and without either the run is at `fast`. `si` and `em` are refused
    (Phase 3).
  - `corner = "<name>"`: every in-scope instance whose view declares a
    corner of that name (`spec/package.md` section 3) runs at it; the
    others run at their view's own parameters. A name no view in the deck
    declares is a warning.
  - `[corners]`, `"<board>:<ref>" = "<name>"` (or `<ref>` when one board
    has it): a per-instance corner, over `corner`. The instance's view must
    declare it (an error otherwise); one for an instance not in the deck is
    a warning.
  - A corner's `params` override the view's own, after the generic value;
    `run.json` records `corner` on each instance that runs at one.
- **`[[battery]]`** (ADR-010 D2; milestone P1): `net`, `ocv` (a CSV
  `soc,volts`, relative to the bench: a header `soc,volts`, then rows with
  `soc` strictly increasing in 0..1, a value in the value grammar or a plain
  number of volts; at least two rows), `soc` (0..1), `r0` (> 0), `rc`
  (optional `[["<R>", "<C>"], ...]`, each pair in series after R0) and
  `temp_c` (informative). The OCV is the table's at `soc`, linear between
  rows (µV, rounded half away from zero), and the state of charge holds for
  the whole run: ADR-010 D2 gives the cell no capacity. The battery drives
  its net as a `[[supply]]` does (two sources on one net, or a `power_out`
  pin on it, are refused).
- **`[[activity]]`** (ADR-009 D4, ADR-010 D2; milestone P3): a
  bench-scheduled internal state of an instance's `supply_load` view, for
  states no firmware reports (the radio):

  ```toml
  [[activity]]
  state = "main:U10#radio_tx"   # <instance>#<state>: a state port of its supply_load view
  at = "500us"                  # optional (default 0): the first burst's start
  every = "1ms"                 # optional: the period (> for); without it one burst
  for = "300us"                 # required (> 0): each burst's length
  ```

  The state is 1 during each burst, to the run's end (`--for`), and 0
  otherwise: a table of the analog engine (no bench engine), driving the
  state node of glue rule 12 (`spec/pin-model.md`); `run.json` labels the
  state bench-scheduled. Several `[[activity]]` on one state may not
  overlap. A state the CPU engine reports (`#run`, `#hfxo`, `#saadc` on the
  nRF52840 target) is refused. At `pi` an `[[activity]]` whose instance has
  no `supply_load` with that state in the deck is refused; at `fast` (no
  `supply_load` in the deck) it is ignored with a warning.
- **`[[load]]` `replace`** (static-load mode, ADR-009 D4; milestone P3):
  `replace = "<instance>"` removes that instance's `supply_load` view from
  the deck; the load (`amps` or `profile`) stands in for it, on its own
  `net`. At `pi` an instance without a `supply_load` in the deck is
  refused; at `fast` the load is added as written, with a warning.
  `run.json`'s `supply_load` records the instance `replaced_by` the load.
- **Refused, not ignored:** the `bb-bench/1` keys of ADR-010 D2 that later
  milestones implement (`[[env]]`, `[probes]`), and an `after` on a `log`
  record.

### 3.3 The bench CSV (ADR-010 D3)

One canonical format for a playback, a profile or a digital stream: a header
`t,<name>`, then `<t>,<value>` rows. `t` is seconds as a plain decimal string
(`0.00001`, `1e-5`; no prefix), converted to whole fs exactly (decimal
arithmetic, rounded half up); times strictly increase after that. A value is
in the value grammar or a plain number (volts, amps), or `0`/`1` for a
digital stream. Lines starting with `#` and empty lines are ignored. Each
value becomes an `f64` once, from its exact decimal. A bench reads only this
CSV; capture formats never enter a run (`bb capture import`, ADR-010 D3).

### 3.4 Assertions (`[[assert]]`, ADR-010 D5-D7, Amendment 3)

A `bb-bench/1` bench may hold `[[assert]]` tables. So may an **assertion
file**: a `bb-bench/1` file of `schema` and `[[assert]]` tables and nothing
else, which `bb run --assert <file>` (repeatable) adds after the bench's,
for a project and for a board file alike. The kernel evaluates every
assertion of a run; each gives exactly one `assert` record
(`spec/event-stream.md`) at its **verdict time**, its window's close. A
failing assertion does not fail the run.

```toml
[[assert]]
name = "LSW-3"                       # required: letters, digits, _ . : - ; unique in the run
kind = "range"                       # the closed set below
msg = "LED_DIN above VLED"           # optional: the start of the record's msg
requires = "pi"                      # optional: a rung; below it, not_evaluable
# the window (every kind; all optional):
from = "10ms"                        # opens at from (default 0) ...
after = { edge = "U10.P1.04", level = 1, delay = "1ms" }   # ... or at the first such event at or after from, plus delay
until = { log = "^uart0: done", source = "U10" }           # closes at the first such event at or after the opening ...
to = "100ms"                         # ... or at to, or at the run's end, whichever is first
while = { net = "U10.P1.04", level = 1 }                   # only while a digital net is at a level
```

An **event** is `{ edge = "<net>", level = 0|1 }` (a change of a digital
net to the level) or `{ log = "<pattern>", source = "<engine>" }` (a `log`
record; `source` optional) and, in `edge_timing` only, `{ cross =
"<net>", ref = "<net>", below = "<V>" }` or `above` (a crossing of an
analog quantity, `ref` optional). A **pattern** is a literal, anchored with
`^` at the start of the message and `$` at its end, or neither (a
substring); every other regular-expression metacharacter (`. * + ? [ ] ( )
{ } | \`) is refused, so a later full regular expression cannot change what
a pattern means.

| `kind` | keys | value (unit) |
| --- | --- | --- |
| `range` | `net`, `ref` (optional: `V(net) - V(ref)`), `min`/`max` (volts) | the extremum that bounds it (`uV`) |
| `at` | `net`, `ref`; `at = "<t>"` or `at = { edge, level }` (each such edge in the window); `min`/`max` in volts for an analog net, integers for a digital net's level | the worst instance (`uV`, or `count`) |
| `settle` | `net`, `ref`, `event = { edge, level }`, `within = "<t>"`, a band `min`/`max` (volts, or `"10%"`: of the value at the event) | the worst settle time (`fs`) |
| `edge_timing` | `start`, `end` (events), `pair = "last"` (default) or `"first"`, `min`/`max` (times) | the worst gap (`fs`) |
| `count` | `net`, `level` (optional), `min`/`max` (integers) | the number of changes (`count`) |
| `frequency` | `net`, `level` (default 1), `period` with `tol` (`"2%"`), or `min`/`max` (times) | the worst period between changes to `level` (`fs`) |
| `log` | `match` (a pattern), `source`; `present = true` or `false`, or `min`/`max` counts, or `field = "<name>"` with `min`/`max` | the number of matching records (`count`), or the worst field value (integer limits: `count`; otherwise `fs` or `uV` by the limits' unit) |
| `event` | `event = "reset"`, `"warn"` or `"error"`; `min`/`max` | the number of `reset` records, or of `log` records of that level (`count`) |

- **What a check reads.** Analog values are the producing engine's
  accepted solver points (`Engine::solver_points`, in the kernel's
  evaluator): an extremum is taken over the linear interpolant of the
  window's points (the points inside it and its two interpolated ends); a
  crossing is interpolated between the two straddling points and rounded up
  to whole fs, as for `digital_in`; a value at an instant is the
  interpolation `sample_output` uses. The monitors are passive: no element,
  no input, no grant boundary. A glitch narrower than one solver step (at
  most TMAX = L) is missed, as it is by `digital_in`. Digital checks read
  the `pin_edge` stream; `log` and `event` checks the `log` and `reset`
  records. Values and limits are integers in base units: a voltage is
  rounded half away from zero to whole µV before it is compared.
- **Instances.** `at`, `settle`, `edge_timing`, `frequency` and a `log`
  `field` check every instance in the window and report the worst (one past
  a limit, else the one nearest a limit). A kind with no instance in its
  window fails, as does a window that never opens.
- **`settle`.** After each change of `event`'s net to its level, the
  quantity must be inside the band by `within` after it, and stay inside
  until the net changes again (or the window closes). An instance still
  open at the run's end whose deadline is after the run's end is
  `not_evaluable`. The assertion fails if an instance failed, passes if at
  least one passed and none failed, and is otherwise `not_evaluable`; the
  record's `msg` lists every instance.
- **`edge_timing` pairs** each `end` event with the last `start` event at or
  before it (`last`), or with the first `start` event since the `end` net
  last changed (`first`; for a `log` or crossing `end`: since the previous
  `end` event). Both lie in the window. An `end` without a `start` is
  unpaired, counted in `msg`.
- **`log` fields.** `field = "code"` reads the number after `code=` or
  `code ` (a whole word) in each matching record: an integer, or a decimal
  with a unit (`50 ms`, `40 us`, `3.58 V`), converted exactly.
- **Names.** In a project an analog net is a net or an alias of the deck
  (section 2), a digital net a bound MCU or device pin (`U10.P1.04`,
  `main:U10.P1.04`) or its kernel net; in a board file they are the board
  file's names (an MCU or device pin's kernel net `<ref>.<pin>` is digital,
  any other name a circuit node). An analog net no probe names is read all
  the same: the analog engine reports it to the kernel, in no output file.
- **Honesty (ADR-010 D6).** A check on a net that an ideal bench source
  forces (a `[[supply]]`, `[[pwl]]` or analog `[[playback]]` without `rs`)
  is refused: a tautology. A branch current (`i(...)`, `#branch`) is
  refused: a current is computable only from node voltages. The record's
  `rung` is the run's (`fast` in this build); `requires` above it gives
  `not_evaluable`.
- **`bb assert <run> --assert <file> ...`** evaluates assertions again on a
  finished run's `events.jsonl` (the run's end is its last grant's end):
  every kind that reads only records gives the record the run gave, a pure
  function of the records (a net's initial level is inferred from its first
  edge); a kind that reads solver points is `not_evaluable` there. Its nets
  are the kernel's, as written.
- `run.json` records the assertion files as `assert_files`, `[{ "file",
  "text" }]`, when a run has any.

## 4. The elaborated deck

`bb run` builds the analog deck from `board.bbm`, the bench and the locked
packages; it contains no file paths:

1. a comment line naming the board, the fidelity and the option set;
2. every view definition once, in sorted (kind, name) order
   (`bbpkg::spice::assemble`; two different definitions of one name are an
   error, ADR-006 D3);
3. one instance line per in-scope instance, sorted by board then
   reference: `X<ref> <port nodes in the view's port order> <subckt>
   <param>=<value> …` (in a system `X<board>_<ref>`), each port's node the
   SPICE name of the net on the package pin of that name. The view's
   `params` come first; a generic passive's `value` is its parsed KiCad
   value. Every number is a plain SPICE number (`4.7e-7`), never a letter
   suffix;
4. the bench's sources, loads and PWL sources, then `.temp`. A
   `[[supply]]` without `rs` is a voltage source (`V_bench_<net> <net> 0
   DC <volts>`, or for a tracking supply `B_bench_<net> <net> 0 V =
   min(<volts>, V(<track>) - <drop>)`); with `rs` it is its Norton
   equivalent, one B conductance with no branch-current unknown
   (`B_bench_<net> 0 <net> I = (<volts or min(...)> - V(<net>)) / <rs>`;
   ADR-007 Amendment 3). `bb-bench/1` (section 3.2): a supply a `set`
   drives takes `V(bb_set_<net>)` for `<volts>`, with `V_bb_set_<net>
   bb_set_<net> 0 dc 0 external` (an ideal fixed one is `V_bb_set_<net>
   <net> 0 dc 0 external` itself); a profile load is `I_bench_load<k>_<net>
   <net> 0 dc 0 external`; then the playback sources (`V_play_<net> <net> 0
   dc 0 external`, or behind `rs` on `bb_play_<net>` with `R_play_<net>`),
   the switch control sources (`V_bb_ctl_<ref>`) and the faults
   (`X_bb_fault<k> <net> <node> bb_fault<k>_ctl bb_switch ...` with
   `V_bb_fault<k>_ctl`). A `[[battery]]` (milestone P1) is its Norton
   form, `I_bench_bat<k> 0 <n0> DC {<ocv> / <r0>}` and `R_bench_bat<k>_0
   <n0> 0 <r0>`, then for each RC pair `j` (from 1) `R_bench_bat<k>_<j>`
   and `C_bench_bat<k>_<j>` in parallel from `bb_bat<k>_<j-1>` to the next
   node; `<n0>` is `bb_bat<k>_0`, or the net itself without RC pairs. No
   branch-current unknown, and a plain resistor on every node (ADR-009
   Amendment 4);
5. the option set's `.options` card.

The pin-model glue (`spec/pin-model.md`) is added by the analog engine, as
for a board file. `bb run` writes the deck to `<out>/deck.cir` for review;
its sha256 is in `run.json`.

**A single-MCU system run** (ADR-013 D2, Amendment 3; section 1) builds
one deck from every listed board; a one-board run's deck is unchanged:

- The comment line names the boards in `simulate`'s order and the primary
  board (`boards main, base (ground main)`).
- **Each board keeps its own ground.** The primary board's ground net is
  node `0`; another board's ground is its net's SPICE name (`base_GND`,
  `spec/board-bbm.md` section 4). Every instance of a simulated board is a
  candidate for the deck, by the same scope rule, in `board.bbm`'s order
  (board, then reference).
- **A bench element returns to its net's board's ground**: in item 4, the
  `0` of a supply, battery, load, PWL or playback source, and a `short`
  fault's other end, is that ground's node. A B-source term reads the net
  against it, `V(<net>,<ground>)` (`V(<net>)` on node 0, so no one-board
  deck changes). A bench control node (`bb_set_`, `bb_ctl_`,
  `bb_fault<k>_ctl`) stays on node 0. A supply on another board's
  ground net is refused as on the primary ground.
- **A `[[connection]]` between two simulated boards with a harness view**
  is one instance line after the instances, in `board.bbm`'s connection
  order: `X_conn<k>_<a>_<b>` (`k` from 1, `a` and `b` the ends, made
  SPICE-safe; `X_conn1_main_J1_base_J3`), its ports in the view's port
  order, `A<pin>` on the net of `a`'s pin and `B<pin>` on `b`'s, then the
  view's `params`. The view must be valid at the run's rung or a lower
  one; `run.json` records the rung it runs at, and the view is checked
  for its lattice qualification like an instance's. Per mated pair: both
  nets in the analog scope, both ends on their nets; both outside it, a
  dead wire, both ends on node `0` (it carries no current; `bb run` lists
  such pairs in a warning); one in the scope and its mate not, an error
  (list both or neither); a pin on no net, its end on its own node
  `bb_conn<k>_<a>_<b>_<port>`.
- **A connection between two simulated boards without a harness** ties
  each mated pair's nets: they become one node, `0` if the group holds
  the primary ground, else the bytewise first SPICE name among the
  group's nets on the primary board, else among all of them.
- A connection with one end simulated keeps section 1's bench ports.

## 5. The verbs

- **`bb open [<dir>|<bb.toml>|<.kicad_pro>|<.net>|<board.cir>] [--check]
  [--update-lock]`.** On a project: imports every board, resolves, prints
  the stage and status counts and the unresolved instances, and writes
  `board.bbm`, `bb.lock` (ADR-006 D8: a changed package or platform is
  refused without `--update-lock`), `.bb/import.json`,
  `.bb/resolution.json`, `.bb/unresolved.toml` and, for a `kicad =` board,
  `.bb/<board>.net` (the exported netlist). A file whose content does not
  change is not rewritten. It exits non-zero when a `[[bind]]`, a
  `[targets]` key or a `[probes]` name does not resolve to a resolved
  instance or a net. `--check` writes nothing and fails if `board.bbm` or
  `bb.lock` would change. Given a `.kicad_pro` or `.net` in a directory
  without `bb.toml`, it first writes a minimal one (`[[board]] id =
  "main"`): the only time `bb open` writes `bb.toml`. `kicad-cli` runs on a
  temporary copy of the project, never in it (`$BB_KICAD_CLI`, else
  `PATH`; the Windows build from WSL through `wslpath` and the Windows
  temporary directory, `$BB_KICAD_WINTMP`).
- **`bb flash --target <instance> <file> [--kind elf] [--project <dir>]`.**
  Checks that the instance's package (a module's chip package) has a
  behavioural layer whose engine takes the image kind (Phase 1: `renode`,
  `elf`), computes the image hash (section 1), and writes
  `[targets."<board>:<ref>"]` with a format-preserving edit (every other
  line and comment kept). No device is touched. `bb flash --hash-only
  <file>` prints the image hash (hex) and touches nothing.
- **`bb run [--project <dir>] --for <t> [--bench <file>] [--fidelity
  fast|pi] [--probe net:<name>]… --out <dir> [--allow-unqualified
  <pkg:view>]…`** (`--fidelity` over the bench's `fidelity`, default
  `fast`; a board file runs at `fast` only, except for
  `--operating-point` below)
  (with every Phase 0 option except `--netlist`, `--firmware` and
  `--target`). At `pi` every MCU `digital_out` gets the rail-tracking,
  tristate glue of `spec/pin-model.md` (ADR-009 D5); `--pi-oe pin-cnf`
  (the default) drives its `oe` from `PIN_CNF.DIR` on the nRF52840,
  `--pi-oe fixed` holds it at 1. `--operating-point` (a project or a
  board file) builds the deck and the glue, solves the DC operating point
  with every input at its initial value and, at `pi`, every `oe` at 0,
  without the transient-op fallback (ADR-015 D4), prints the result and
  stops: no CPU engine starts and nothing is written (ADR-009 gate G9). **Read-only on the project:** `board.bbm` must equal what
  the import gives now (for a `kicad =` board, from `.bb/<board>.net`
  after checking every source's sha256 against `.bb/import.json`) and
  `bb.lock` must match, else it fails with "run `bb open`"; the image hash
  must match `[targets]`. The bench's option set must be one every
  in-scope view is lattice-qualified for at the run's lattice (ADR-006
  D6), unless `--allow-unqualified` names the view. Writes the Phase 0
  outputs, `deck.cir` and `run.json`. `--elaborate-only` stops before any
  engine starts: a project writes `deck.cir` and `run.json`; a board file
  (`--netlist`, milestone S) writes `deck.cir` as the analog engine's
  netlist (the board's SPICE, its package views and the glue). `bb run`
  reads a `bb-lock/0` lock with the migration hint (`spec/package.md`
  section 9). `--assert <file>` (repeatable, a board file too) adds an
  assertion file's checks (section 3.4); the summary prints every verdict.
  `--image <instance>=<file.elf>` (a project; repeatable, one per instance;
  ADR-010 D8, Amendment 5) runs that ELF on the instance in place of
  `[targets]`' image: its image hash is computed as `bb flash` would write
  it and recorded in `run.json`'s `targets` as a flashed image's; `bb.toml`
  is neither read for it nor written. It is how a `bb test` case's
  `targets` reach the run.
- **`bb test <file> --out <dir> [--jobs N] [--pool auto|env|off]`**
  (ADR-010 D8-D9, milestone B3): runs a `bb-test/0` file's cases and writes
  `test.json` and `junit.xml` (`spec/test.md`).
- **`bb assert <run dir> --assert <file>… [--bench <file>] [--out <file>]`.**
  Evaluates assertions again on a finished run's `events.jsonl` and prints
  (or writes) their `assert` records (section 3.4). Reads nothing else and
  writes nothing but `--out`.
- **`bb probe [--list] [net:<name>]… [--project <dir>]`.** Lists every net
  (display name, board, SPICE name, leaf name) and alias, or adds probes
  to `[probes] nets` after resolving them.

## 6. `run.json` (`bb-run/0`)

Canonical JSON, no absolute paths, no wall-clock times: `board_bbm` (the
sha256 of `board.bbm`); `import` (`.bb/import.json`'s `boards`, without
the host); `bench` (file name, text, sha256, option set and its card;
with `bb-bench/1` and only when used, `data`: each CSV file as named and
its sha256; `seed` and `bounce`: each press's generated toggle offsets;
`engine`: the bench engine's `id` and `content` hash, ADR-010 D4);
`targets` (instance and image kinds and hashes); `analog` (per in-scope
instance: package, version, view, requested fidelity, the rung it was
chosen at, `corner` when it runs at one (ADR-009 D8), provenance `source`
and `confidence`, its qualifications and whether one covers the run; and per loading `adc_in`, its `pin_load` view
under the bound pin, `kind = "pin_load"`; an explicit `load = "none"` on
an `adc_in` binding is recorded the same way, with `view = "none"` and only
`ref`, `package`, `version` and `kind`; and in a system run per harness
in the deck, `ref` `<a>=<b>` with `kind = "harness"` and the instance
keys); `out_of_scope` and `not_simulated` (with
reasons); `devices`, only when the project has a `[[device]]` (per device
`ref`, `package`, `version`, `device` (its id), `virtual` and `params` as
written; ADR-013 D1); `digital_only`, only when a net is (its display
names, sorted; ADR-014 D4); `pi_glue`, only at `pi` (ADR-009 D5,
milestone P2): per MCU `digital_out` on a circuit net, in binding order,
`pin` (its kernel net), `rail` (the package pin) and `rail_net`, `rhiz`
(Ω, a decimal string), `vss_net` (when the ground pins are on a net) and
`oe` (`"pin_cnf_dir"` or `"fixed"`); `simulated`, only in a system run (the
boards of `simulate`, as listed; ADR-013 D2); `supply_load`, only at `pi` (ADR-009 D4,
milestone P3): per instance whose `supply_load` view is in the deck (glue
rule 12), `ref`, `package`, `version`, `view`, provenance `source` and
`confidence`, `qualified_for_run`, `idd` (each state's `typ` as written)
and `states` (each state port's driver: `"cpu"`, `"bench [[activity]]
(bench-scheduled)"` or `"0"`); an instance whose view a `[[load]] replace`
removed is listed with `ref`, `package`, `view` and `replaced_by` only;
`assert_files`, only when `bb run --assert`
names any (each file's name and text, in order; ADR-010 Amendment 3);
`packages` (each one's id, version, `source`: `path` for a local package,
else its `registry+<url>` source, ADR-011; and tree) and `platforms` (the
locked hashes); `deck_sha256`;
`options` (every run option that can change results, the probes (since
Phase 2 H6 without `adc_model`: `--adc-model` was removed with Renode's
stock ADC, ADR-015 D9), and
`ngspice_watchdog`: `max_points_per_cell` and
`max_points_without_progress`, the progress watchdog's limits, which decide
whether a run completes or fails with `NoProgress`, ADR-015 D1;
`ngspice_lattice_snap`: `"off"`, only for a run with
`bb run --ngspice-lattice-snap off`, ngspice's step clamp before the
lattice snap, kept to reproduce the SAADC stall (ADR-015 Amendment 4), and
absent otherwise; with `--gdb` also `gdb` and `gdb_wait`); `debug`, `true` for a run with a
debugger attached (`bb run --gdb`, ADR-008 D10 and Amendment 7) and
absent otherwise: a debug run is never compared against a reference; and
`versions`. `bb run` flags a view whose provenance is `estimated` or
`agent_generated` in its summary.
