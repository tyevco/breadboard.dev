# CLI reference (`bb`)

`bb` is the single binary of Breadboard (`cli/`). `make setup` builds it and
installs it into `~/.local/bin`. The usage text on this page is the output of
`bb --help` and of each subcommand's `--help`, from the commit this site was
built from; where the spec says more, it is noted.

```text
bb run       run a co-simulation (a board file or a project)
bb test      run the cases of a bb-test/0 file; write test.json and junit.xml
bb assert    evaluate assertions again on a finished run
bb open      import a project or resolve a board file's packages; write board.bbm and bb.lock
bb flash     record a firmware image and its hash in bb.toml
bb probe     list probe-able nets, or add probes to bb.toml
bb pool      a warm Renode pool
bb pkg       lint, test, migrate, pack and inspect component packages
bb registry  the tools of a package registry repository's CI
```

`bb -h`, `bb --help` and `bb help` print the usage (`bb run --help` and
`bb assert --help` too). An unknown subcommand prints it to stderr and exits
2. `make docs-sync` (part of `make test`) checks that this page has a
section for every subcommand of `bb --help` and names every `bb run`
option.

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
       bb run [--project <dir>] --for <time> [--bench <bench.toml>] [--fidelity fast|pi]
              [--probe net:<name|alias|pin> ...] --out <dir> [--allow-unqualified <pkg:view>]
              [--max-quantum 10ms] [--renode-quantum 10us] [--sample-rate 100kHz] [--seed 0]
              [--renode <path>] [--verify edges] [--no-verify] [--engine-order mcu-first]
              [--lookahead on|off] [--adc-push change|every]
              [--renode-pool env|off] [--uart-log on|off] [--gdb <port> [--gdb-wait on|off]]
              [--ngspice-watchdog 200000,10000] [--ngspice-lattice-snap on|off]
              [--assert <file.toml> ...] [--image <instance>=<file.elf> ...]
              [--pi-oe pin-cnf|fixed] [--operating-point] [--limits]
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
| `--fidelity fast\|pi` | The analog fidelity rung of a project's views (ADR-009 D1): `fast` (the default) or `pi`, the power-integrity rung; wins over the bench's `fidelity`. At `pi` every MCU `digital_out` drives from its rail (the package's `[power] io_rail`, or the binding's `rail`) through `zout`, and is high-Z (`rhiz`, default 100 MΩ) while its output enable is 0 (ADR-009 D5); `supply_load` views, the power-fail comparator and brown-out come into the deck, and the package limits become assertions (below). |
| `--pi-oe pin-cnf\|fixed` | What drives a `pi` output's enable: `pin-cnf` (the default) reads `PIN_CNF.DIR` on the nRF52840, so a pin is high-Z until the firmware makes it an output; `fixed` (and every other platform): always enabled. Changes results; recorded in `run.json` `pi_glue`. |
| `--limits` | A project at `fast`: the package limits become assertions too, as at `pi` (ADR-009 D3): each `pins.toml` `[[limits]]` bound and each supply pin's `power{vmin, vmax}` is a `range` assertion `limit:<board>:<ref>.<pin>:<key>` over the run (`imax`: `not_evaluable`). Reads only; recorded in `run.json` (`limits`, `limits_flag`). |
| `--assert <file.toml>` | An assertion file: a `bb-bench/1` file of `[[assert]]` tables only (ADR-010 D5, Amendment 3), added to the bench's. Each verdict is an `assert` record in `events.jsonl` at its window's close; a failing assertion does not fail the run. Repeatable; a board file takes it too. |
| `--image <instance>=<file.elf>` | A project: run this image on `<instance>` in place of `bb.toml`'s `[targets]` image (what `bb test`'s `targets` use). Its image hash is recorded in `run.json`; `bb.toml` is not written. Repeatable. |
| `--operating-point` | Build the deck and glue, solve its DC operating point (no transient-op fallback, ADR-015 D4), report and stop without starting the CPU. At `pi` every `digital_out` is high-Z, an MCU's state after reset (ADR-009 G9). |
| `--ngspice-watchdog <points per cell>,<points without progress>` | ngspice's progress watchdog (default `200000,10000`, ADR-015 D1): a lattice cell that takes more accepted solver points, or more consecutive points that do not advance time by a whole fs, fails the run with `NoProgress:ngspice` instead of hanging. Counts, not wall time, so the failure is deterministic. Recorded in `run.json`. |
| `--ngspice-lattice-snap on\|off` | `on` (the default): a solver step that would land just short of the grant or the next lattice point lands on it (ADR-015 Amendment 4). `off` is the clamp before Phase 2 H2, kept only to reproduce the SAADC stall; it changes results and is recorded in `run.json`. |
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
| `--renode-pool env\|off` | `env` (default): lease a pre-warmed Renode from the pool in `$BB_RENODE_POOL` when one is idle, else start fresh. `off`: always fresh. Never changes the outputs. |
| `--uart-log on\|off` | `on` (default): on the nRF52840, every line the firmware writes to UARTE0/1 becomes an `info` log record at the time of its last byte. |
| `--gdb <port>` | Debug the firmware with GDB: a GDB remote server on `127.0.0.1:<port>` (`0`: a free port, printed). Forces lookahead off and starts Renode fresh. A breakpoint, a step, a write or a peripheral read can change the results, and the first one marks the run; attaching, halting, reading and detaching do not. See [Debugging with GDB](../guide/debugging). |
| `--gdb-wait on\|off` | `on` (default): the CPU waits at reset until a debugger attaches and continues. `off`: the run starts at once; attach at any time. |

The full help text (`bb --help`) explains each option in more detail.

### Outputs

| File | Contents |
| --- | --- |
| `events.jsonl` | One `grant` record per grant, `pin_edge` records with exact edge times, `log` records, and (event stream version 2, Phase 2) `assert` records with each assertion's verdict and `reset` records of a brown-out's hold and release. See [Outputs](./outputs). |
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
usage: bb open [<dir>|<bb.toml>|<.kicad_pro>|<.net>] [--check] [--update-lock] [--offline]
       bb open <board.cir> [--check] [--update-lock] [--offline]
```

`spec/project.md` section 5 describes the project behaviour below.

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
| `--update-lock` | Accept packages, sources, vendor files or platforms that changed (rewrites only those entries; without it they are refused). |
| `--offline` | Fetch nothing: registry packages and vendor files come from the cache only (`bb run` never fetches). |

A package id that no local directory provides is resolved on a registry
(the public one, or a project's routed `[registries]`; ADR-011 D4), which
`bb open` fetches into the cache.

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
usage: bb pkg lint [--publish] [<dir>...]     lint packages (default: first-party packages/);
                                              --publish: the view-name rule is an error (ADR-011 D11)
       bb pkg test [<dir>...] [--record] [--report <file>]
                                              run test vectors through NgspiceEngine (ADR-006 D6)
       bb pkg migrate [<dir>...]              rewrite packages to the current schema majors
       bb pkg migrate --bench <file>...       rewrite bench files to the current bb-bench major
       bb pkg migrate --project <bb.toml>...  rewrite project files to the current bb-project major
       bb pkg migrate --board <board.bbm>...  rewrite board.bbm files to the current bb-board major
       bb pkg migrate --lock <bb.lock>...     rewrite lockfiles to the current bb-lock major
       bb pkg tree <package dir>              print a package's tree hash
       bb pkg pack <package dir> [-o <file>]  write its deterministic archive (ADR-011 D6)
       bb pkg unpack <archive> <dir>          extract an archive (strictly) and print its tree hash
       bb pkg value [--kicad R|C|L] [--dim voltage|time|resistance|capacitance|inductance|current|temperature|count|percent] <text>...
       bb pkg init --from-svd|--from-ibis ... refused: Phase 3 (ADR-011 Amendment 5)
```

- `bb pkg lint --publish` makes the view-name rule of ADR-011 D11 an error
  instead of a warning, as a registry's CI requires.
- `bb pkg test --record` writes the lattice qualifications of the views that
  pass; `--report <file>` writes one JSON line per vector run.
- `bb pkg migrate` rewrites a previous schema major (Phase 2's schema epoch:
  `bb-part/0` to `/1`, and so on) mechanically; with `--bench`,
  `--project`, `--board` or `--lock` it rewrites those files instead.
- `bb pkg pack` and `bb pkg unpack` write and read the deterministic archive
  a registry serves, keyed by the package's tree hash.
- `bb pkg value` parses values in the package value grammar, or with
  `--kicad` as a KiCad value field of a generic R, C or L.
- `bb pkg init` (a package scaffolded from an SVD or an IBIS file) is
  Phase 3: it is refused with that message. Write a package's `part.toml`
  and `pins.toml` by hand.

See [Component packages](./packages).

## `bb test`

```text
usage: bb test <file.toml> --out <dir> [--jobs N] [--pool auto|env|off]
```

Runs the cases of a `bb-test/0` file (ADR-010 D8, `spec/test.md`), each as
a `bb run` into `<out>/<case>/` (its log `<out>/<case>.log`; a case with
`reference = true` also `<out>/<case>-ref/`, its one-cell reference), then
writes `<out>/test.json` (canonical JSON: no wall times and no paths, only
the outcomes, values, and the `run.json` and output hashes) and
`<out>/junit.xml` (one testsuite per case, one testcase per assertion and
per comparison; wall times only here). It exits 0 if and only if every
outcome equals its expectation (an unexpected pass of a negative control is
a failure) and every golden, reference and `same_as` comparison holds.

| Option | Meaning |
| --- | --- |
| `--out` | The output directory (its `<case>` subdirectories are replaced). |
| `--jobs` | Cases run at a time (default: the number of runs, at most 4). |
| `--pool auto\|env\|off` | `auto` (default): the warm Renode pool in `$BB_RENODE_POOL`, else a pool with `--jobs` slots for this command; `env`: `$BB_RENODE_POOL` if set, else every Renode fresh; `off`: every Renode fresh. The outputs are the same either way. |

```sh
cd examples/dial-me-spinner
bb test tests/w1.toml --out out/w1     # W1 at pi: 6 cases, the two controls expected to fail cue-held
```

An assertion that a case does not expect to pass (a negative control, or a
`not_evaluable` package `imax`) is named in the case's `expect` table.

## `bb assert`

```text
usage: bb assert <run-dir> --assert <file.toml> [--assert ...] [--bench <bench.toml>] [--out <file>]
```

Evaluates assertions again on a finished run (ADR-010 Amendment 3): the
`[[assert]]` tables of the `--assert` files and of `--bench`'s bench, read
against the run's `events.jsonl`. It prints the `assert` records, or writes
them to `--out`. Only the record-only kinds can be evaluated this way (a
digital edge, a count, a `log`, an `event`); a kind that reads analog solver
points is `not_evaluable` here, because a finished run keeps only its
samples. A net is named as in the run (a board file's pins as
`<ref>.<pin>`).

## `bb registry`

```text
usage: bb registry init <dir> --key <minisign public key> [--dl <url template>]
       bb registry publish <dir> --pubtime <YYYY-MM-DD> <package dir>...
       bb registry yank <dir> <id>@<version> [--undo]
       bb registry remove <dir> <id>@<version> --reason <text>
       bb registry snapshot <dir>
       bb registry check <dir> [--base <dir>] [--author <login>] [--approver <login>]...
       bb registry verify <url> --key <minisign public key>
```

The tools of a registry repository's CI (ADR-011 D8, `spec/registry.md`).
`bb registry publish` appends index lines and archives (versions are
immutable); `bb registry snapshot` bumps the serial and writes
`snapshot.json`, which the registry's CI then signs (`minisign -S -s
<secret key> -m <dir>/snapshot.json`); `bb registry check` runs the pull
request checks; `bb registry verify` is a client's full check of a published
registry (the signature, every index file and archive). `bb registry init`
starts a registry repository, `bb registry yank` marks a version not to be
chosen (`--undo` reverses it) and `bb registry remove` takes one out with a
recorded reason. No public registry is published by this build; the
tools are tested against local fixtures (`make registry-test`).

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
| `BB_GDB_TRACE` | With `bb run --gdb`: a file to which every GDB packet (debugger, proxy, Renode) is written. A diagnostic, never an output. |
