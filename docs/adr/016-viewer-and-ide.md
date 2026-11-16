# ADR-016: The waveform viewer and IDE integration

- **Status:** Accepted (2026-11-15; see "Approver decisions
  (2026-11-15)"). Proposed 2026-11-14, revised after design review round 1
  (see "Review notes"). D1 and D2 implemented, milestone E4 (2026-11-15,
  Amendment 1): G2's VS Code step is automated in a headless VS Code
  instead of checked by hand, and G1 runs in the CI job that already runs
  the demo. D3 is not built (default 1).
- **Part of:** the Phase 2 plan, milestone E4.
- **Normative changes when implemented:** none to `spec/`. The outputs
  stay as `spec/event-stream.md` defines them. This ADR records licence
  and scope decisions about tools around them.
- **Evidence:** the ecosystem research pass (`~/bbscratch-p2-eco`):
  - `wellen` 0.25.7 (Surfer's waveform library, BSD-3) loading bb's VCDs;
  - a 40-line JS parser;
  - libsigrok's VCD input source;
  - cortex-debug's attach path against `bb run --gdb`;
  - the licences and versions of Surfer, Vaporview, GTKWave, PulseView and
    `vcd-stream`.

## Summary

- **Ship documentation and templates first, code second.**
  - A docs page shows how to open `nets.vcd` in Surfer (web or VS Code),
    Vaporview or GTKWave.
  - Committed VS Code `tasks.json` and `launch.json` templates attach
    cortex-debug to `bb run --gdb`.
- **An optional, small Apache-2.0 viewer** on the docs site overlays
  Breadboard's own records on the waveforms. No generic viewer can show
  those records.
- **No sigrok/PulseView bridge:** its VCD importer cannot handle a 1 fs
  timescale.

## Context

**bb's VCD in existing tools:**

- **Surfer's backend `wellen` reads bb's VCD correctly** (measured):
  - `out/demo/rail-wrong/nets.vcd`: 231 KiB, with `real` variables;
  - `out/dms-home/park/nets.vcd`: 4.3 MiB, 277k changes;
  - both open with the timescale as fs and `real` variables as Real, in
    26 ms and 44 ms;
  - a 40-line JS parser takes 12.5 ms and 100-138 ms (node 24.11).
- **libsigrok's VCD input** sets samplerate = 1/timescale. bb's `1 fs`
  is 1e15 Hz, so E3's 2.975 s would be about 3e15 samples.

**Tools and licences:**

| Tool | Licence | Version | Notes |
| --- | --- | --- | --- |
| Surfer | EUPL-1.2 | v0.7.0, 2026-04-27 | analog drawing since 0.5.0; a web build ("some features are missing"); a VS Code extension; an iframe `postMessage` API (`LoadUrl`); WCP |
| Vaporview | GPL-3.0 | VS Code extension v1.6.1 | |
| GTKWave | GPL-2.0 | 3.3.116 in Ubuntu 24.04 | the README already points to it |

**cortex-debug against `bb run --gdb`** (measured with gdb-multiarch, on
dial-me-spinner):

- `target extended-remote` works.
- cortex-debug's default `monitor halt` is refused ("bb: monitor commands
  are refused under bb run --gdb"), so `overrideAttachCommands` is
  needed.
- An SVD peripheral view would read `0x40000000`-`0x5FFFFFFF`, which marks
  the run as a debug run.

## Decision

### D1. The docs page (no code)

- A `site/guide/viewing.md` page:
  - Surfer: web, or the VS Code extension;
  - Vaporview;
  - GTKWave;
  - which file to open, how real nets show, and the `$comment` name map.
- It is verified by a CI step in the docs job that loads, with `wellen`:
  - the VCDs that job's `make demo` writes (`out/demo/*/nets.vcd`: the
    repository commits no VCD, so the gate generates them);
  - one small committed fixture, `tests/fixtures/vcd/mixed.vcd` (a few
    KB: digital and `real` variables, the fs timescale, the `$comment`
    name map), so the check has an input even when the demo changes.
- The loader is a tiny binary in its own crate, `tools/vcd-check`,
  **outside the Cargo workspace**: `wellen` 0.25.7 brings 11 normal
  dependencies (rayon, memmap2, lz4_flex, fst-reader and others), which
  `cargo build --workspace` and `bb` never see.
- The page states that PulseView cannot read bb's VCD.

### D2. VS Code templates

Templates in `docs/templates/vscode/`, which the user copies into a
project's `.vscode/` by hand (the docs page gives the command). `bb open`
does not write them: `spec/project.md` lists exactly what `bb open`
writes, and templates are not worth a spec change.

- **`tasks.json`:** a background task running `bb run --gdb 3333 …`, with
  a problemMatcher whose `endsPattern` is `GDB: listening on`.
- **`launch.json`:** cortex-debug with:
  - `servertype: "external"`, `gdbTarget: "localhost:3333"`;
  - `request: "attach"`, with no `load`;
  - `gdbPath: "gdb-multiarch"`;
  - `overrideAttachCommands: []`;
  - **no `svdFile`**.

  A comment explains each setting.
- **Gate:** the templates attach in VS Code on the user's host, and a
  session that only attaches, reads and detaches leaves the outputs
  byte-identical (ADR-008 Amendment 7's property).
- The VS Code step is a manual check recorded in review notes, because CI
  has no GUI. The gdb-multiarch half is automated, as in `make gdb-test`.

### D3. The optional site viewer

- About 500 lines of Apache-2.0 JavaScript on the VitePress site.
- It opens a local `nets.vcd` and `events.jsonl` with the File API (no
  upload).
- It draws analog and digital lanes, and overlays:
  - `pin_edge`;
  - `log` and `warn` records (the debug mark);
  - `assert` verdicts with their windows (ADR-010);
  - `reset` records (ADR-009);
  - grant boundaries, as an option.
- It is optional for the exit. It is built only if E5's external users
  ask for it, or the preprint needs its figures interactive.

### D4. Not built

- **A sigrok bridge.** The reverse direction, captures into benches, is
  ADR-010 D3.
- **A VS Code extension with a `bb` debug type.** It would be about 200
  lines: run with `--gdb 0`, parse the port, hand over to cortex-debug,
  and open the VCD. Built only on demand.
- **Hosting Surfer's WASM on the docs site.** That redistributes
  EUPL-1.2 code with its source obligations. Linking to
  app.surfer-project.org avoids it.

## Determinism impact

None. Viewers read outputs. The templates do not change a run's options
beyond `--gdb`, which already carries its debug semantics.

## Gates

| # | Gate |
| --- | --- |
| G1 | The docs CI job loads `make demo`'s VCDs and the committed fixture with `tools/vcd-check` (`wellen`, outside the workspace): every variable present, `real` variables read as Real, the timescale 1 fs |
| G2 | The templates attach (manual, recorded) and the attach-read-detach session is byte-identical (automated through gdb-multiarch) |
| G3 | (if D3 is built) the viewer renders the E1 demo's VCD and records, checked by a headless browser test |

## Rejected alternatives

- **A sigrok/PulseView bridge** (D4).
- **An optional VCD export at a coarser timescale for PulseView.** Lossy,
  and a second format to keep honest.
- **Embedding Surfer** by hosting its WASM (D4).

## Questions for the approver

None in the plan's consolidated list.

**Defaults (apply unless the approver objects):**

1. **Docs and templates first**, the viewer only on demand.
2. **`wellen`** (BSD-3) in a separate crate outside the workspace, for
   the VCD load check only.
3. **No `bb open --vscode`**: templates are copied by hand.

## Approver decisions (2026-11-15)

The plan's consolidated list has no question for this ADR; all twelve of
those were accepted with their recommended answers (plan section 10).

- **Build scope.** E4 is in this build (docs page and VS Code templates
  first; the site viewer only on demand, default 1).

**Defaults.** No objection was made to the defaults listed under
"Questions for the approver", so they apply as written.

## Review notes (design review round 1, 2026-11-14)

**Adopted:**

- **G1 checked nothing** (minor): the repository commits no VCD. The docs
  job now generates them with `make demo`, plus one committed fixture.
- **`bb open --vscode` would change `spec/project.md`** (minor): dropped;
  templates are copied by hand, so this ADR still changes no `spec/` file.

**Not adopted:** "wellen is not light" as a reason to avoid it. Its
dependencies are real, but they stay out of the workspace and out of `bb`;
a hand-written VCD reader would check the docs page against Breadboard's
own understanding of VCD, not against Surfer's.

## Amendments

### Amendment 1: D1 and D2 as implemented (E4, 2026-11-15)

No `spec/` file, no output and no protocol changes. `bb` is unchanged.
The only change to `make test` is `vscode-templates`, a static check of
under 1 s.

**D1, the docs page.** `site/guide/viewing.md` ("Viewers and VS Code" in
the guide's sidebar) covers:

- which file to open, and the 1 fs timescale;
- the one `board` scope, in which dotted pin names stay one variable;
- real variables written only at sample instants (draw them as steps);
- the `$comment` name map;
- Surfer (web, desktop, VS Code extension), Vaporview and GTKWave, with
  the licences and versions of the Context table;
- why PulseView cannot read the file.

The page links to app.surfer-project.org and hosts no Surfer code (D4).
`site/guide/debugging.md`, `site/guide/testing.md` and
`site/reference/outputs.md` link to it.

**G1 as implemented.** `tools/vcd-check` is a crate with an empty
`[workspace]` of its own and its own `Cargo.lock`. It uses
`wellen = "=0.25.7"`, is built `--locked` into
`$(CARGO_TARGET_DIR)/vcd-check`, and never enters `cargo build
--workspace` or `bb`. For each VCD it requires:

- wellen's timescale `Timescale { 1, FemtoSeconds }`;
- every `$var` of the header as a wellen variable of the same leaf name
  in `board`, with nothing extra: `real 64` as `VarType::Real` with the
  Real encoding, `wire 1` as `VarType::Wire` with `BitVector(1)`;
- a value at time 0 for every variable, and as many changes as the file
  has value-change lines for its identifier;
- the time table ending at the file's last `#` time;
- every `$comment` entry naming a declared variable;
- with the run's `nets.csv`: its columns are exactly the real variables,
  through the name map, and at every row each value wellen holds equals
  the CSV's value as a number. This is more than D1 listed. It is what
  "real variables read as Real" means for a viewer: the value drawn is
  the value bb wrote.

`make vcd-check` (`scripts/vcd_check.sh`) runs `make demo` and loads its
two VCDs (`out/demo/rail`, `out/demo/rail-wrong`; fewer than two found
fails). It then loads the committed fixture `tests/fixtures/vcd/mixed.vcd`
(1.3 KB) with `mixed.csv` (9.7 KB). These are a real E1 run of 32 ms at
5 kHz: five real nets, three wires with two edges and the name map. Two
runs gave identical bytes, and `tests/fixtures/vcd/README.md` gives the
command.

Six negative controls, each on an edited copy of the fixture, must fail
with their own message, and each copy must differ from the fixture:

| Control | Edit | Expected failure |
| --- | --- | --- |
| timescale | the timescale set to 1 ps | `timescale is not 1 fs` |
| csv-value | one CSV value moved by 1 µV | `CSV values differ` |
| drop-change | one VLED change line dropped | `CSV values differ` |
| comment | a map entry naming an undeclared variable | `not declared` |
| real-as-wire | a real declared as a 1-bit wire | wellen 0.25.7 panics (`Failed to parse four state value`); the tool catches the panic and fails with `wellen could not load it` |
| wire-as-reg | a wire declared as `reg` | `bb writes only` |

`make vcd-check-fixture` runs the fixture and the controls without the
demo.

- **Deviation: G1 runs in the CI job `dms-rail`, not in the docs
  workflow.** The docs workflow builds and deploys the Pages site, runs
  only on a push to `main` and has no Renode, so it cannot run `make
  demo`. `dms-rail` already runs the demo's E1 runs for
  `docs-plots-check`, the other check of the docs site's content. The step
  is the same check, in a job that can produce its input.
- **Measured** (2026-11-15, this worktree): `make vcd-check` 16 s
  including the demo. The tool loads and checks E1's 120 ms VCD (190 KB,
  against 12 001 CSV rows) in 0.02 s.

**D2, the templates.** `docs/templates/vscode/tasks.json` and
`launch.json` (JSON with comments; every setting is explained in place):

- **The task** `bb: run --gdb`: `bb run --gdb 3333 --bench
  bench/cold.toml --for 120ms --out out/debug` from the workspace folder.
  It is the debugging guide's E1 run; the user edits the bench, the
  duration and the output directory. `isBackground` is true.
- **The problem matcher.** `background` has `activeOnStart: true`, a
  `beginsPattern` that matches none of bb's lines, and `endsPattern:
  "GDB: listening on"`.
- **The configuration** `bb: attach (cortex-debug)`, exactly as D2
  lists:
  - `type: cortex-debug`, `request: attach`, `servertype: external`;
  - `gdbTarget: localhost:3333`, `gdbPath: gdb-multiarch`;
  - `executable`: the project's ELF;
  - `overrideAttachCommands: []`;
  - no `svdFile` and no load;
  - `preLaunchTask: "bb: run --gdb"`.

Facts verified against the installed code, not guessed:

- **cortex-debug 1.12.1** (`dist/debugadapter.js`):
  - its `External` server controller's `initCommands` is `target-select
    extended-remote <gdbTarget>`;
  - its `attachCommands` is `interpreter-exec console "monitor halt"`,
    which `overrideAttachCommands: []` replaces;
  - `disconnectRequest` deletes the breakpoints (`break-delete`) and then
    detaches with `target-detach`. It sends `kill` only for
    `terminateDebuggee` or `suspendDebuggee`, which bb treats as a detach
    anyway.
- **VS Code 1.145.3** (`workbench.desktop.main.js`):
  - the background matcher key is `activeOnStart`;
  - `processLineInternal` is `tryBegin(line) || tryFinish(line)`, so a
    line that matches `beginsPattern` is never tested against
    `endsPattern`.

  The first draft had `beginsPattern: "^bb run: "`. bb's first line is
  the listening line, so the task never became ready and the attach never
  started (measured: `startDebugging` timed out at 240 s with bb waiting
  at reset). `make vscode-templates` now requires that `beginsPattern`
  does not match the listening line.

**G2 as implemented: the VS Code step is automated, not manual.** D2 had
"the templates attach in VS Code on the user's host" as a manual check
recorded in review notes, because CI has no GUI. `make vscode-test`
(`scripts/vscode_test.sh`) runs it in a real VS Code instead.

The setup:

- **Pinned downloads.** The test fetches once into
  `~/.cache/bb/vscode-test`, each file checked against a sha256 in the
  script:
  - VS Code 1.145.3 (linux-x64, from update.code.visualstudio.com);
  - cortex-debug 1.12.1 with its declared `extensionDependencies`
    `mcu-debug.debug-tracker-vscode` 0.0.15, `memory-view` 0.0.29,
    `rtos-views` 0.0.18 and `peripheral-viewer` 1.6.5, from Open VSX.

  The VSIXs are unpacked into a private extensions directory. Nothing
  goes through `code --install-extension` or the marketplace.
- **The project.** The templates are copied byte for byte into the
  `.vscode/` of a copy of the dial-me-spinner project with `led_rail.elf`
  recorded (`dms_rail.sh prepare`, as `make gdb-test` does).
- **The VS Code run:**
  - under Xvfb, in a network namespace of its own with only loopback
    (`unshare -rn`, then back to the user's own uid);
  - telemetry, updates and experiments off, workspace trust disabled;
  - `scripts/vscode_test/runner.js` as `--extensionTestsPath`. It
    starts the configuration by name with `vscode.debug.startDebugging`,
    as the Run and Debug view does, and logs every DAP message.
- **What a run goes through.** VS Code runs the preLaunchTask through
  the templates' problem matcher. cortex-debug starts gdb-multiarch and
  attaches.
- **The shim.** A `bb` shim on `PATH` records the argv and tees bb's
  output; the templates call `bb` as written.

The two cases:

- **`attach`.** After the stop that the attach reports, the test reads
  the threads, the stack, every scope's variables (98 values, the core
  registers, `pc=0x9c4 <z_arm_reset>`) and the word at `$sp`. Then it
  stops the session, and the run finishes. Every one of these held:
  - VS Code exit 0, task exit 0;
  - bb's argv equal to the template's `args`;
  - one DAP `attach` request with the template's settings;
  - no `monitor` refusal;
  - bb's summary `debugger sessions 1, no stop inside a slice and no
    write`, and no debug-run mark;
  - all four hashes (`nets.csv`, `nets.vcd`, non-grant records,
    `events.jsonl`) equal to `tests/golden/dms-rail/ok-cold/outputs.sha256`,
    the run without GDB;
  - nothing added to `.vscode/`.
- **`break`, the negative control** (the comparison and the GDB path
  acted). After the attach, a `FunctionBreakpoint` at
  `gpio_nrfx_port_set_bits_raw`, then continue. Every one of these held:
  - the stop came at `gpio_nrfx_port_set_bits_raw@0x000036b6`;
  - the breakpoint was removed and the session stopped;
  - exactly one `debug run: GDB` warn record, and `run.json` `debug: true`;
  - `nets.csv`, `nets.vcd` and the non-grant records all differ from the
    golden.
- **Measured** (2026-11-15): both cases passed in two consecutive full
  runs. The cases took 19-61 s of VS Code time each, with the host
  shared with other tracks, and `make vscode-test` took 116 s with the
  downloads cached.

Deviations and limits:

- **Not in `make test` or CI.** The test downloads about 380 MB once, and
  needs `xvfb` and unprivileged user namespaces. GitHub's ubuntu-24.04
  runners restrict those through AppArmor, which was not tried, since CI
  only runs on a push. `make test` keeps the network-free rule of E1. The
  automated gdb-multiarch half of G2 remains `make gdb-test` (in `make
  test`) and `make gdb` (CI).
- **The host.** VS Code ran on Linux (WSL2 Ubuntu 24.04, on the user's
  machine). VS Code for Windows with its WSL remote was not exercised;
  the docs page says so and recommends the WSL remote. A human check
  there stays optional. It is not a gate, because the automated run is
  a stronger check than the manual one D2 asked for.
- **`make vscode-templates`** (in `make test`, about 0.1 s) checks the
  templates statically against D2, and checks the `endsPattern` against
  the format string in `cli/src/main.rs`. E4's `make test` budget is "—";
  this sub-second check follows H0's `measure-lock-test` precedent and
  is recorded in AGENTS.md's milestone table.

**D3 is not built** (default 1: only if E5's users ask for it, or the
preprint needs interactive figures). G3 does not apply.
