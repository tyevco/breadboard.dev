# ADR-016: The waveform viewer and IDE integration

- **Status:** Accepted (2026-11-15; see "Approver decisions
  (2026-11-15)"). Proposed 2026-11-14, revised after design review round 1
  (see "Review notes").
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
