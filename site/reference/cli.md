# CLI reference (`bb`)

`bb` is the single binary of Breadboard (`cli/`). `make setup` builds it and
installs it into `~/.local/bin`. The usage text on this page is the output of
`bb --help` and of each subcommand's `--help`, from the commit this site was
built from; where the spec says more, it is noted.

```text
bb run    run a co-simulation (a board file or a project)
bb open   import a project or resolve a board file's packages; write board.bbm and bb.lock
bb flash  record a firmware image and its hash in bb.toml
bb probe  list probe-able nets, or add probes to bb.toml
bb pool   a warm Renode pool
bb pkg    lint, test and inspect component packages
```

`bb -h`, `bb --help` and `bb help` print the usage. An unknown subcommand
prints it to stderr and exits 2.

## Projects and their files

| Verb | Writes into the project |
| --- | --- |
| `bb open` | `board.bbm`, `bb.lock`, `.bb/`; `bb.toml` only when it creates a minimal one |
| `bb flash`, `bb probe` | `bb.toml`, edited in place (other lines and comments kept) |
| `bb run` | nothing: it is read-only on the project and writes only under `--out` |

## `bb run`

```text
usage: bb run --netlist <board.cir> --firmware <elf> --target <ref> --for <time>
              --probe net:<NET> [--probe net:<NET> ...] --out <dir>
       bb run [--project <dir>] --for <time> [--bench <bench.toml>] [--fidelity fast]
              [--probe net:<name|alias|pin> ...] --out <dir> [--allow-unqualified <pkg:view>]
              [--max-quantum 10ms] [--renode-quantum 10us] [--sample-rate 100kHz] [--seed 0]
              [--renode <path>] [--verify edges] [--no-verify] [--engine-order mcu-first]
              [--lookahead on|off] [--adc-push change|every] [--adc-model bb|stock]
              [--renode-pool env|off]
```

There are two front ends:

- **A board file** (`--netlist`): a SPICE body plus `*bb` directives, as in
  `examples/spike/board.cir`. You name the firmware and the target MCU on the
  command line. Parts given by `package=` need a current `bb.lock` next to it.
- **A project** (`--project`, default: the current directory): a `bb.toml`.
  `bb run` reads `board.bbm` (which must be current), the firmware from
  `[targets]`, the `[[bind]]` pins and `[probes]`, and also writes `deck.cir`
  and `run.json`. A stale `board.bbm` or `bb.lock` is refused with "run
  `bb open`", and the firmware's image hash must match `[targets]`.

### Options

| Option | Meaning |
| --- | --- |
| `--netlist <file>` | Board file (SPICE body plus `*bb` directives). |
| `--project <dir>` | A `bb.toml` project (the default when `--netlist` is not given: the current directory). Never written. |
| `--bench <file>` | Bench file: supplies, loads, temperature, analog scope, option set. Default: no sources, every net in scope, option set `phase0`. |
| `--fidelity <rung>` | The analog fidelity rung (Phase 1: `fast`). |
| `--elaborate-only` | A project: elaborate (checks, deck, probes), write `<out>/deck.cir` and `run.json`, then stop without starting any engine. |
| `--allow-unqualified <pkg:view>` | Run a view that is not lattice-qualified for (lattice, option set); recorded in `run.json`. Repeatable. |
| `--firmware <elf>` | ELF loaded into the target MCU (board-file front end). |
| `--target <ref>` | Part reference the firmware runs on, e.g. `U1` (board-file front end). |
| `--for <time>` | Simulated duration, e.g. `500ms`. |
| `--probe net:<name>` | A net to record: a circuit net (analog volts) or an MCU pin net `<ref>.<pin>` (digital level). Repeatable; the order is the CSV column order. |
| `--out <dir>` | Output directory. |
| `--max-quantum <t>` | Cap on the length of one grant (default `10ms`), a multiple of the Renode quantum. A pure cap: never changes results. |
| `--renode-quantum <t>` | Renode's global quantum, the grant lattice (default `10us`). Run configuration: changing it changes results. |
| `--sample-rate <f>` | `nets.csv` and `nets.vcd` sample rate (default `100kHz`). Samples never change the grant partition. |
| `--seed <n>` | Renode emulation seed (default `0`). |
| `--renode <path>` | Path to the `renode` launcher (default: `$BB_RENODE`, `PATH`, `~/.local/bin`). |
| `--verify every\|edges\|final` | When to cross-check Renode's virtual time and pin states against the engine (default `edges`). Pure reads: outputs are identical in every mode. |
| `--no-verify` | Alias for `--verify final`. |
| `--engine-order mcu-first\|analog-first` | Registration order (default `mcu-first`). The advance order comes from the bindings, so outputs are identical either way. A test knob. |
| `--lookahead on\|off` | CPU lookahead for boards with a feedback loop. On by default whenever the lookahead plugin is loaded. Changes only the grant records; `off` is the one-cell reference. |
| `--lookahead-plugin auto\|always\|never` | Load Renode's lookahead plugin (default `auto`: boards with a feedback loop). The plugin is inert; a test knob. |
| `--adc-push change\|every` | When an `adc_in` pin's value reaches the MCU's ADC: at the lattice points where it changes (default), or at every lattice point (the reference of the equivalence gate). A test knob. |
| `--adc-model bb\|stock` | The STM32F072's ADC: Breadboard's model (default) or Renode's stock `STM32F0_ADC`. Changes results: each has its own references. |
| `--renode-pool env\|off` | `env` (default): lease a pre-warmed Renode from the pool in `$BB_RENODE_POOL` when one is idle, else start fresh. `off`: always fresh. Never changes the outputs. |

The full help text (`bb --help`) explains each option in more detail.

### Outputs

| File | Contents |
| --- | --- |
| `events.jsonl` | One `grant` record per grant, `pin_edge` records with exact edge times, and `log` records. See [Outputs](./outputs). |
| `nets.csv` | `t_fs` and one column per probe, sampled at `--sample-rate`. |
| `nets.vcd` | The probed analog nets as `real`, sampled; MCU pin nets as wires with every edge. Timescale 1 fs. |
| `deck.cir` | A project: the elaborated SPICE deck. |
| `run.json` | A project: every analog view with its provenance, confidence and qualification, the option set, the locked hashes. |

`bb run` prints the wall-clock time to the terminal, never into the outputs.

### Examples

```sh
# A project (examples/dial-me-spinner), the cold bench
bb run --for 120ms --bench bench/cold.toml --probe net:VLED --probe net:LED_DIN --out out/rail

# A Phase 0 board file
bb run --netlist examples/spike/board.cir --firmware examples/spike/firmware/build/blinky.elf \
    --target U1 --for 500ms --probe net:LED_A --probe net:PA5 --out out/spike
```

## `bb open`

```text
usage: bb open [<dir>|<.kicad_pro>|<.net>|<board.cir>] [--check] [--update-lock]
```

(This line is from `bb --help`. `bb open --help` prints an older text that
covers only board files and says KiCad projects are "Phase 1 M4"; M4 is done,
and `spec/project.md` section 5 describes the project behaviour below.)

- **On a project** (a directory or `bb.toml`): imports every board, resolves
  each part to a package, prints the stage and status counts and the
  unresolved instances, and writes `board.bbm`, `bb.lock` and the files of
  `.bb/`. A file whose content does not change is not rewritten. It exits
  non-zero when a `[[bind]]`, a `[targets]` key or a `[probes]` name does not
  resolve.
- **Given a `.kicad_pro` or `.net`** in a directory without `bb.toml`, it
  first writes a minimal one. `kicad-cli` (for a `kicad =` board) always runs
  on a temporary copy of the project, never in it.
- **On a board file** (`board.cir`): resolves its `package=` parts and writes
  `bb.lock` next to it: each package's version and tree hash, and each engine
  platform's content hash.

| Option | Meaning |
| --- | --- |
| `--check` | Write nothing; fail if `board.bbm` or `bb.lock` is missing or would change. |
| `--update-lock` | Accept packages or platforms whose hash changed (rewrites only those entries; without it they are refused). |

## `bb flash`

```text
usage: bb flash --target <instance> <file.elf> [--kind elf] [--project <dir>]
       bb flash --hash-only <file.elf>
```

Records the image as the firmware of `<instance>` (`U10`, or `main:U10`) in
`bb.toml` `[targets]`, with its **image hash**: a SHA-256 over the ELF's
`PT_LOAD` segments and the entry point, not over the file, so two builds that
load the same bytes have one hash. It reads `board.bbm` (run `bb open` first).
No device is touched. `--hash-only` prints the image hash and touches nothing.
Phase 1 implements `elf` images only.

## `bb probe`

```text
usage: bb probe [--list] [net:<name> ...] [--project <dir>]
```

| Option | Meaning |
| --- | --- |
| `--list` | The probe-able nets (display name, SPICE name, leaf name) and the test-point aliases, from `board.bbm`. |
| `net:<name>` | Add a probe to `bb.toml` `[probes] nets`: a net name, a leaf name, an alias or a bound MCU pin. |

Names resolve by the rules in [Projects](./project#names).

## `bb pool`

```text
usage: bb pool run [--slots N] [--max-runs K] [--rss-limit-mb M] [--renode <path>]
                   [--dir <dir>] [--no-wait] -- <command> [args...]
       bb pool start [--slots N] [--max-runs K] [--rss-limit-mb M] [--renode <path>]
                     [--dir <dir>] [--idle-exit 1800s]
       bb pool stop [--socket <path>]
       bb pool status [--socket <path>]
```

A warm Renode pool (ADR-004 Part B): pre-warmed Renode processes that
`bb run` (and the Renode engine tests) lease instead of starting Renode, when
`BB_RENODE_POOL` names the pool's socket. A run that finds no idle process
starts Renode fresh. Outputs are identical either way.

| Subcommand / option | Meaning |
| --- | --- |
| `run` | Run `<command>` with a pool, then stop it; exits with the command's status. |
| `start` | Start a pool in the background; prints the export line. It stops after `--idle-exit` without a lease (default `1800s`, or `never`). |
| `stop` | Stop the pool at `--socket` (default `$BB_RENODE_POOL`). |
| `status` | Print the pool's slots and counters. |
| `--slots` | Renode processes (default 1). |
| `--max-runs` | Leases a process serves before it is replaced (default 100). |
| `--rss-limit-mb` | Replace a process whose RSS exceeds this after a lease (default 2048). |
| `--renode` | The `renode` launcher (default: `$BB_RENODE`, `PATH`, `~/.local/bin`). |
| `--dir` | Private (0700) directory for the socket (default `$XDG_RUNTIME_DIR/bb`, else `$BB_CACHE_DIR/pool`, else `~/.cache/bb/pool`). |
| `--no-wait` | Run `<command>` without waiting for the pre-warm. |

```sh
bb pool run -- make spike      # one command on a warm pool
```

## `bb pkg`

```text
usage: bb pkg lint [<dir>...]                 lint packages (default: first-party packages/)
       bb pkg test [<dir>...] [--record] [--report <file>]
                                              run test vectors through NgspiceEngine (ADR-006 D6)
       bb pkg migrate [<dir>...]              rewrite packages to the current schema majors
       bb pkg tree <package dir>              print a package's tree hash
       bb pkg value [--kicad R|C|L] [--dim voltage|time|resistance|capacitance|inductance|current|temperature|count|percent] <text>...
```

- `bb pkg test --record` writes the lattice qualifications of the views that
  pass; `--report <file>` writes one JSON line per vector run.
- `bb pkg value` parses values in the package value grammar, or with
  `--kicad` as a KiCad value field of a generic R, C or L.

See [Component packages](./packages).

## Environment variables

These are the variables the help texts and `AGENTS.md` name:

| Variable | Used for |
| --- | --- |
| `BB_RENODE` | The `renode` launcher. |
| `BB_RENODE_POOL` | The socket of a warm Renode pool (set by `bb pool run` / `bb pool start`). Only this variable names a pool; nothing discovers one. |
| `BB_CACHE_DIR`, `XDG_CACHE_HOME` | Override the cache directory (`~/.cache/bb`). |
| `BB_FIRST_PARTY_PACKAGES` | The first-party package directory, when `bb` cannot find it from the checkout it was built from. |
| `BB_KICAD_CLI`, `BB_KICAD_WINTMP` | The `kicad-cli` to run, and the Windows temporary directory when using the Windows build from WSL. |
| `BB_RENODE_JIT_PROFILE=off` | Disable the cached .NET JIT startup profile. |
