# Projects (`bb.toml`), benches, names and the CLI verbs

Status: normative. Decided by ADR-007 D5 and D7 (accepted 2026-11-09;
implemented in Phase 1 M4, ADR-007 Amendment 1). Implementation:
`cli/src/project.rs`, `names.rs`, `import.rs`, `elab.rs`, `open.rs`,
`flash.rs`, `probe_cmd.rs`. JSON Schemas: `spec/schema/project.schema.json`
(`bb-project/0`) and `spec/schema/bench.schema.json` (`bb-bench/0`); the TOML
files are converted to JSON as they are and validated (`make packages`).
**Changing this file requires an ADR.**

A project is a directory with `bb.toml`. `bb open` turns it into
`board.bbm` (`spec/board-bbm.md`), `bb.lock` (`spec/package.md`) and the
files of `.bb/`; `bb run` runs it. Unknown keys are an error. Every number
is a string in the value grammar of `spec/package.md` section 6 (ADR-006
D5).

## 1. `bb.toml` (`bb-project/0`)

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

[probes]
nets = ["VLED", "LED_DIN"]

[[connection]]                       # systems (M11): mated connectors
a = "main:J1"
b = "base:J3"
# model = "<view>"; pins = [["1", "1"], ...]
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
- **Targets** are a list of typed images; Phase 1 implements exactly one
  image of `kind = "elf"` on one MCU of the simulated board. `bin`, `hex`,
  `uf2` and `flash` are reserved.

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
  the last `/`), within the simulated board first, then the project.
- **Probe aliases** (test points) follow the instance rule: `main:TP1`, or
  `TP1` when only one board has it.
- **A probe** is a net or an alias, else a bound MCU pin
  (`U1.PA4`, whose digital level is the kernel net `U1.PA4`, as in Phase
  0).

## 3. Benches (`bb-bench/0`)

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
- `bb run` without `--bench` uses an empty bench: no sources, every net in
  scope, option set `phase0`.
- Phase 2 defines the full bench (CSV playback, timed actions,
  assertions) in its own ADR.

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
4. the bench's sources, loads and PWL sources, then `.temp`;
5. the option set's `.options` card.

The pin-model glue (`spec/pin-model.md`) is added by the analog engine, as
for a board file. `bb run` writes the deck to `<out>/deck.cir` for review;
its sha256 is in `run.json`.

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
  Checks that the instance's package has a behavioural layer whose engine
  takes the image kind (Phase 1: `renode`, `elf`), computes the image's
  sha256, and writes `[targets."<board>:<ref>"]` with a format-preserving
  edit (every other line and comment kept). No device is touched.
- **`bb run [--project <dir>] --for <t> [--bench <file>] [--fidelity fast]
  [--probe net:<name>]… --out <dir> [--allow-unqualified <pkg:view>]…`**
  (with every Phase 0 option except `--netlist`, `--firmware` and
  `--target`). **Read-only on the project:** `board.bbm` must equal what
  the import gives now (for a `kicad =` board, from `.bb/<board>.net`
  after checking every source's sha256 against `.bb/import.json`) and
  `bb.lock` must match, else it fails with "run `bb open`"; the image hash
  must match `[targets]`. The bench's option set must be one every
  in-scope view is lattice-qualified for at the run's lattice (ADR-006
  D6), unless `--allow-unqualified` names the view. Writes the Phase 0
  outputs, `deck.cir` and `run.json`.
- **`bb probe [--list] [net:<name>]… [--project <dir>]`.** Lists every net
  (display name, board, SPICE name, leaf name) and alias, or adds probes
  to `[probes] nets` after resolving them.

## 6. `run.json` (`bb-run/0`)

Canonical JSON, no absolute paths, no wall-clock times: `board_bbm` (the
sha256 of `board.bbm`); `import` (`.bb/import.json`'s `boards`, without
the host); `bench` (file name, text, sha256, option set and its card);
`targets` (instance and image kinds and hashes); `analog` (per in-scope
instance: package, version, view, requested fidelity, the rung it was
chosen at, provenance `source` and `confidence`, its qualifications and
whether one covers the run); `out_of_scope` and `not_simulated` (with
reasons); `packages` and `platforms` (the locked hashes); `deck_sha256`;
`options` (every run option that can change results, the probes); and
`versions`. `bb run` flags a view whose provenance is `estimated` or
`agent_generated` in its summary.
