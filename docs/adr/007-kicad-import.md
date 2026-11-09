# ADR-007: KiCad import, the elaborated board (`board.bbm`) and the project file

- **Status:** Accepted (2026-11-09), with the approver decisions below
  (revised 2026-11-09 after two design reviews, see Review notes). The
  reference revision is dial-me-spinner `6854d25` (approver decision C);
  Context below was measured at `dc96515` and re-checked at `6854d25`
  (Context 12). Written before implementation because it adds
  normative schemas to `spec/` and changes output naming rules in
  `spec/event-stream.md` and `spec/pin-model.md`.
- **Part of:** the Phase 1 plan (`docs/phase1-plan.md`), milestone M4, M6
  for D5, and M11 for systems.
- **Depends on:** ADR-006 (packages, views, `pins.toml`, `bb.lock`, the
  value grammar, schema versioning).
- **Normative changes when implemented:**
  - new `spec/board-bbm.md` and `spec/schema/board-bbm.schema.json`;
  - new `spec/canonical-json.md`;
  - new `spec/project.md` with `spec/schema/project.schema.json` and
    `bench.schema.json`;
  - amended `spec/pin-model.md` (glue rules 3 and 5, a net-naming rule);
  - amended `spec/event-stream.md` (net names in `nets.csv` and `nets.vcd`);
  - `spec/time-grant.md` does not change.
- **Evidence:**
  - the KiCad research (`~/bbscratch-p1-kicad`; prototypes `sexp.py`,
    `netinfo.py`, `native_conn.py`, `resolve.py`, `direction.py`, `tokdiff.py`
    in the session scratchpad) and the critique's semantic netlist diff;
  - KiCad sources: `eeschema/sch_file_versions.h`,
    `pcbnew/pcb_io/kicad_sexpr/pcb_io_kicad_sexpr.h`, and
    `netlist_exporter_xml.cpp` on the 10.0 branch;
  - dev-docs.kicad.org (the file-format pages give only "the version is a
    YYYYMMDD date", with no change list);
  - `kicad-cli` 10.0.8 in WSL (`/usr/bin/kicad-cli`, package
    `kicad 10.0.8~ubuntu24.04.1`) and `kicad-cli.exe` 10.0.8 on Windows;
  - RFC 8785 §3.1 and Appendix B (number range);
  - today's `cli/src/board.rs`.

## Approver decisions (2026-11-09)

Recorded after the human approver reviewed the Phase 1 plan and ADR-006,
ADR-007 and ADR-008 (the same decisions A–C are in each of them).

- **A. Every recommended answer is accepted.** For this ADR's open
  questions:
  1. `board.bbm` and `run.json` as separate files, `board.bbm`
     content-only, source metadata in `.bb/import.json` (D6).
  2. Breadboard's integer-only canonical JSON instead of RFC 8785.
  3. Exact-MPN resolution with package-declared patterns (D4).
  4. Project overrides before the `bb:part` field (D4 stage 1).
  5. The output-naming changes in `spec/event-stream.md` (RFC 4180 header
     quoting; VCD SPICE names plus a mapping comment).
  6. KiCad 6–8 accepted with a warning.
  7. The optional PCB cross-check (D8) deferred unless M4 finishes early.

  Plan-level answers that bind this ADR too: Apache-2.0; the reference
  `.net`, `.kicad_sch` and `.kicad_pro` files are vendored under
  `examples/dial-me-spinner/` (with their sha256 in
  `examples/dial-me-spinner/SOURCE`); exit scope E1 on one board.
- **B. Publication:** nothing is published, pushed or given a remote
  during Phase 1's milestones; M7 tests from a local clone and the human
  decides publication afterwards (plan, Approver decisions B).
- **C. Reference revision:** the reference is dial-me-spinner branch
  `kicad10-port`, commit `6854d25` (KiCad 10.0.8). `dc96515` (KiCad 9.0.9)
  becomes the secondary import check: not a gate for E1, but gate M4c
  still requires its netlist to give the same `board.bbm` if the
  connectivity is truly identical, and records the differences if it is
  not. Gates M4b and M4c below are restated for `6854d25`, with counts
  re-derived there (Context 12).

## Context

### Inputs measured on the reference board (dial-me-spinner at `dc96515`; re-checked at `6854d25` in item 12)

1. **File versions.**
   - Every `.kicad_sch` is `(version 20250114)`, generator 9.0.
   - The `.kicad_pcb` is `20241229`.
   - The committed `.net` comes from Eeschema 9.0.9.
   - KiCad 10 writes schematic `20260306` and board `20260206`. A KiCad 10
     save upgrades only the sheets it rewrites: in the upgraded copies, the
     root, mcu and leds sheets are `20260306` and the others stay
     `20250114`.
   - KiCad's development branch is already at schematic `20261106`
     (KiCad 11).
2. **KiCad 9 and 10 netlists agree on connectivity.** The 9.0.9 export and
   `kicad-cli` 10.0.8 exports (Linux and Windows) give 201 parts, 142 nets
   and 657 pin nodes, with identical connections and all 142 net names
   equal. The 28 `libparts` are identical. The other differences:
   - **`pinfunction`:** KiCad 10 writes `name_number` (`P0.00_17`) for
     stacked-pin support (confirmed in `netlist_exporter_xml.cpp`). 419
     nodes differ for this reason. The `libparts` pin names carry no suffix.
   - **Net `class`:** KiCad 10 fills it from the project's net-class
     patterns, while the committed 9.0.9 file says `Default` everywhere. The
     cause was not isolated.
   - **Component tokens:** KiCad 10 adds a `units` block to all 201
     components and drops the `datasheet "~"` value (133 components; their
     `Datasheet` field becomes empty).
   - **The Windows build:** writes CRLF and lists some per-part pin entries
     in another order (8 lines).
   - **Run to run:** the only change is the `date` line.
3. **`kicad-cli` writes into the project.** Run on a project folder, it
   creates `<project>.kicad_prl` next to the `.kicad_pro`. The research run
   did this once in the dial-me-spinner repo; the file was removed and
   checksums restored. Without the `.kicad_pro`, net classes come out
   `Default`. Without `sym-lib-table`, only the netlist's `libraries`
   section changes. `kicad-cli` 10.0.8 `sch export netlist` also takes
   `--variant` ("When no --variant argument is provided, the default
   variant is output"), so a `.net` silently reflects one variant.
4. **Our own connectivity is possible but a moving target.** A 170-line
   native connectivity prototype matched `kicad-cli` exactly on this board
   after one fix: `PWR_FLAG` has a `power_out` pin and must not name a net.
   The board has none of the following, so the prototype is untested on
   them:
   - buses, sheet pins, hierarchical labels, mirrored symbols or junctions,
     and competing net names;
   - KiCad 10's local power symbols (20250227), "~ is no longer empty text"
     (20250318), stacked pins (20250901), variants (20250922, 20260306),
     flat hierarchy (20251012) and jumper pin groups.
5. **Cost.** A native `kicad-cli` export takes 1.46–2.74 s (median 1.85 s,
   load about 4). The Windows exe called from WSL takes 0.58–12.19 s. A
   Python parse of the 2.3 MB PCB takes about 1 s.
6. **The user's own toolchain** (`build.py`, `release.py`, `pcbgen.py`)
   already treats `kicad-cli sch export netlist` as the source of truth.
7. **Part fields on the main board** (201 parts):
   - MPN on 188, Manufacturer on 188, LCSC on 187;
   - no `bb:part` or `Sim.*` fields;
   - 3 DNP (C43, R11, U43); 14 excluded from the BOM.

   Resolution by the design doc's order:
   - 118 by footprint plus value (89 C, 28 R, 1 L; all values parse; every
     one also has an MPN);
   - 12 need no model (3 mounting holes, 9 test points);
   - 70 by MPN (21 distinct; none resolve against an empty registry);
   - 1 needs a prompt (J11, the NFC coil).

   The MPN strings are messy: `TC2030-IDC-NL (footprint only)`,
   `SC-20S 32.768kHz 20PPM 9pF`, `B5819W SL`, `BAV99,215`, and ordering
   codes that carry electrical meaning (`RT9080-33GJ5`: `-33` is the 3.3 V
   output). Symbols and MPNs disagree: the `LED:WS2812B-2020` symbol
   carries MPN WS2812C-2020-V6, and a 74LVC1G34 symbol carries SN74LV1T34.
8. **Net names.**
   - 109 of 142 names are not SPICE-safe: `/MCU, flash & debug/XL1`, `+3V3`.
     Local labels carry the sheet path (`/LED ring/LED_DIN_R`).
   - Several contain a comma, which today's `nets.csv` header cannot carry,
     and spaces, which a VCD reference name cannot carry.
   - There are no case-insensitive collisions after renaming, but ngspice
     and `bb` compare node names case-insensitively, so this needs a check.
   - 22 `unconnected-(U10-P0.29-Pad10)` nets exist.
9. **Direction is not in KiCad.**
   - Of the 42 connected U10 GPIO nets, KiCad pin types give a direction
     for only 17, and get I2C SCL wrong.
   - 21 have an R, C or L on them; 6 are buses that need device models;
     the rest go to test points or connectors.

   So direction and the pin model must come from the package and the
   project, never from KiCad.
10. **Two boards make one system, and they share reference designators.**
    The main and base boards are separate KiCad projects, joined by a
    12-way JST SH harness (main J1 to base J3, the same pinout as `nets.py`
    `LINK_PINOUT`). The suite keeps a separate ground per board and models
    the harness R and L. 15 refs exist on both boards with different
    parts: U1 is the RT9080 on main and the BQ25180 on base; U2 is the
    TPS22917 on main and a USBLC6 on base; J1, R1–R4, C1–C4 and TP1–TP4 too
    (TP1 is VSYS on main and VBUS on base). VSYS and GND exist on both.
11. **What exists today.** `cli/src/board.rs` builds `Board { parts, pins,
    spice }` from `*bb part`, `*bb pin` and a verbatim SPICE body. `bb run`
    allows exactly one part, the two STM32 platforms, and a pin direction
    fixed by its model.
12. **The reference revision `6854d25` (plan M0, 2026-11-09).** Measured
    on `git archive 6854d25` with an S-expression parser over the content
    D3 reads (refs, values, footprints, `libsource`, sheet path, the D3
    fields and flags; nets as sorted `(ref, pin number)` sets; `libparts`
    pins as number, name and type):
    - Both committed netlists come from `Eeschema 10.0.8-10.0.8~ubuntu24.04.1`.
      The `.kicad_pcb` files are `20260206` (generator 10.0). The
      `.kicad_sch` files still carry `(version 20250114)` and
      `(generator_version "9.0")`: the user's generators write that
      header; D2 accepts it.
    - **Main board:** 201 parts, 142 nets, 657 pin nodes, 28 `libparts`,
      and **no difference** from `dc96515`'s 9.0.9 netlist in any of that
      content. So M4b's counts are unchanged, and the secondary M4c check
      (`dc96515`'s netlist gives the same `board.bbm`) is expected to hold.
    - **Base board (M11):** 21 parts, 17 nets, 81 pin nodes, 12 `libparts`.
      One difference from `dc96515`: KiCad 10.0.8's library calls the
      USB-C shield pin `SH` (9.0.9: `S1`), so node `J1.S1` on `GND` is now
      `J1.SH`, and the `libpart`'s pin number changes with it (the
      dial-me-spinner commit message records the same rename). The two
      base netlists therefore give different `board.bbm` files by
      construction; M11 uses `6854d25`'s.

## Decision

### D1. Architecture: `kicad-cli` for connectivity, a native parser for its output (option c)

```
.kicad_pro + .kicad_sch* (+ sym-lib-table)
   │  copy to a temp dir (never run in the project)          ── or ── a committed .net (format "E")
   ▼
kicad-cli sch export netlist --format kicadsexpr  ──►  .net  ──►  importers/kicad: S-expression parse
                                                                       │  version gates (D2), canonical connectivity
                                                                       ▼
                       packages (ADR-006) + bb.toml ──►  resolver (D4)  ──►  board.bbm (D6, canonical JSON)
                                                                       │       + .bb/import.json, .bb/resolution.json
                                                                       ▼
                                   bench.toml + firmware + options ──►  bb run: elaborate (D5) → engines → run.json
```

- **A new crate, `importers/kicad`,** owns:
  - the S-expression reader (written for this; no regex, as AGENTS.md
    requires);
  - the `.net` model;
  - the version gates;
  - the `kicad-cli` runner.
- **A data-only module, `bbcore::board`,** holds the elaborated-board
  types, the canonical JSON writer and reader, and the integer units. The
  kernel owns the board model, as the design doc says; the importer and
  the CLI depend on it.
- **The `kicad-cli` runner:**
  - It finds `kicad-cli` through `BB_KICAD_CLI` or `PATH`. In WSL it may be
    the Windows exe, with paths through `wslpath -w`.
  - It copies the root `.kicad_sch` and every sub-sheet file reachable
    through `(sheet … (property "Sheetfile" …))`, plus the `.kicad_pro` and
    `sym-lib-table`, into a fresh temp dir.
  - It runs the export there, then deletes the dir.
  - A test asserts that the sha256 of every file in the source project is
    unchanged after `bb open` (Context 3).
- **`bb open` also accepts a `.net` directly.** The `.net` is the format
  `kicad-cli` emits anyway. This lets a stranger run the demo without KiCad
  (plan §5), and gate M4c proves that both routes give the same
  `board.bbm`.

**Why (c).** KiCad's own export defines connectivity "as the user sees it
in KiCad", which is the design doc's requirement. It already covers
everything the board does not exercise (Context 4) and every KiCad 10
change. It costs about 2 s per import, and `bb open` is not a hot path.
The native output parser is still needed, and is small. The prototype's
native connectivity (option b) stays as an optional cross-check for
machines without KiCad, gated against `kicad-cli` on a set of test boards.
It is not in Phase 1's path.

### D2. KiCad 9 and 10 support, and version gates

- **Netlist** (both routes: a committed `.net` and `kicad-cli`'s output):
  - requires `(export (version "E")`;
  - requires `(design (tool "Eeschema <major>.…"))` with major 9 or 10.
    Major 6–8 is accepted with a warning ("untested"); a newer major is
    refused with an error that names the file and the tool string. This
    closes the gap where a KiCad 11 `.net` passed on format "E" alone.
- **Schematics** (the root `.kicad_sch` and every sheet, read with the
  S-expression reader for their `version` only):
  - accept `20250114` (KiCad 9) through `20260306` (KiCad 10), in any mix
    across sheets (Context 1);
  - accept older KiCad 6–8 versions with a warning ("untested");
  - refuse anything newer than `20260306`, with an error that names the
    file and version.
  - Each newer KiCad needs its own ADR amendment, with the format token diff
    measured as in Context 2.
- **Pin names** come from `libparts`, keyed by `(lib, part)` and pin number.
  Nodes are keyed by `(ref, pin number)`. `pinfunction` is never used,
  because of the KiCad 10 `_N` suffix. (The user's own `spicelib.py` keys on
  `pinfunction` and will break on a KiCad 10 export. That is noted for the
  user, not changed here.)
- **Unread tokens are ignored by construction:** the importer reads only
  D3's list, so KiCad 10's `units` blocks, the dropped `datasheet "~"`, and
  any token KiCad adds later do not reach `board.bbm` (gate M4c covers it).
- **Refused in Phase 1, with a clear error:**
  - a pin number containing `[` (stacked pins);
  - a schematic that defines variants (the `variant` token in the root
    sheet);
  - a symbol with jumper pin groups.

  None of these appear on the reference board, and each needs a decision on
  meaning before it is supported. A `.net` carries no variant marker, so
  the `.net` route cannot detect variants; `.bb/import.json` records the
  tool string so a reviewer can.
- **Net classes are not imported in Phase 1.** Nothing uses them, and
  KiCad 9 and 10 disagree on them (Context 2).
- **Canonicalisation:** before anything is hashed or written, line endings,
  node order and the `date` line are dropped. The output is a function of
  content only.

### D3. What the importer reads from the netlist

For each component:

- `ref`, `value`, `footprint`, `libsource` (lib and part), `sheetpath`
  (names);
- the fields `MPN`, `Manufacturer`, `LCSC`, `bb:part` (exact names; the
  list is extensible in `bb.toml`);
- the flags `dnp`, `exclude_from_sim`, `exclude_from_bom`.

For each net: its name and its nodes as `(ref, pin number)`. For each
libpart: the pins (number to name, plus KiCad pin type, kept for lint
only).

For each component the importer also enforces three rules:

- **Module pin names.** The chip pin names, after the module's `pinmap`,
  must equal the symbol's pin names. For U10 the symbol already names pins
  `P0.06` and so on; any mismatch is an error.
- **`unconnected-(…)` nets.** They become probe-only nets with no binding.
- **The board ground.** It is the net named by `[[board]] ground` (default
  `GND`). It must exist, and it maps to SPICE node `0` for the simulated
  board.

### D4. Part resolution order

Resolution runs once per instance, in this order; the first stage that
yields a package wins.

1. **Project override:** `[instances."<qualified ref>"] package = "<id>"` in
   `bb.toml`, or `package = "none"` with `reason`. This comes first because
   Breadboard must never need edits to the user's KiCad files:
   dial-me-spinner itself may not be modified.
2. **`bb:part` field** on the symbol (`ti/tps22917` or
   `ti/tps22917@0.1.0`).
3. **MPN, exactly.**
   - **Normalisation, shown in the resolution table:** trim whitespace, and
     drop one trailing parenthesised comment (`TC2030-IDC-NL (footprint
     only)` → `TC2030-IDC-NL`). Nothing else is removed.
   - **Match:** the normalised field equals an entry of a package's `mpn`,
     or fully matches one of its `mpn_patterns` (anchored regexes the
     package author writes, for example `TPS22917DBV[RT]`). The package
     author, who knows which suffixes are electrically significant, decides
     what matches; the resolver never strips a suffix on its own.
     `RT9080-33GJ5` therefore never binds to an `RT9080` package unless that
     package lists it.
   - Two packages matching is an error, never a guess.
   - **The MPN beats the symbol:** a symbol/MPN mismatch (Context 7) is a
     lint warning, and the MPN decides.
4. **LCSC** against every package's `lcsc` (an addition to the design doc's
   order; 187/201 parts carry it).
5. **Generic passives** by footprint plus value (ADR-006 D5). This stage
   runs only when no MPN package matched; every main-board passive has an
   MPN, so a future MPN package overrides the generic one.
6. **No model needed:**
   - `MountingHole:*`;
   - `Fiducial:*`;
   - `TestPoint:*`, which become probe aliases (`main:TP1` → its net);
   - DNP and `exclude_from_sim`, kept with a reason.
7. **Unresolved:**
   - `bb open` lists them in its printed table and in
     `.bb/resolution.json` (machine-readable: per instance the stage
     reached, the keys tried, the normalised MPN, and candidate packages),
     so agents act on data, not on scraped text.
   - It writes a commented stub per unresolved instance to
     `.bb/unresolved.toml`, keyed by qualified ref and rewritten whole on
     each run (idempotent). `bb.toml` is never edited by `bb open`.
   - The instance is marked `unmodelled`. Its nets still exist.
   - A run fails only if a binding, a probe or the analog scope needs that
     instance.

Expected result on the main board, from the research's counts:

| Package registry state | Parts auto-resolved | Of which have a model |
| --- | --- | --- |
| Empty registry | 130 of 201 (118 passives, 12 no-model) | 118 |
| Phase 1 first-party packages | 181 of 201 (adds the 48 WS2812C LEDs, U2 and U21 TPS22917, and U10) | 169 |

J11 is an override (`package = "none"`). The other 19 MPN parts are listed
as `unmodelled`. That is the honest state of a board whose sensors have no
models yet.

### D5. Project file (`bb.toml`), names, bindings, bench subset and analog scope

```toml
schema = "bb-project/0"
[project]
name = "dial-me-spinner rev A"
simulate = "main"                                   # Phase 1: one board is simulated

[[board]]
id = "main"
netlist = "kicad/main/dial-me-spinner-main.net"     # or: kicad = "kicad/main/dial-me-spinner-main.kicad_pro"
ground = "GND"

[packages]
paths = ["packages"]

[instances."main:U10"]                              # qualified; "U10" also works while it is unique
package = "raytac/mdbt50q-u1mv2"                    # an override (resolution stage 1)
[instances."main:J11"]
package = "none"
reason = "NFC coil pads: no RF model in Phase 1"

[targets."main:U10"]                                # written by `bb flash`
images = [{ file = "firmware/prebuilt/led_rail.elf", kind = "elf", hash = "sha256:…" }]   # ADR-008 D5

[[bind]]                                            # MCU pins crossing an engine boundary
pin = "main:U10.P1.04"
model = "digital_out"
set = "S0S1"
net = "LED_PWR_EN"                                  # optional: checked against the netlist
[[bind]]
pin = "U10.P0.06"
model = "digital_out"
set = "H0H1"

[probes]
nets = ["VLED", "LED_DIN_R", "LED_DIN_MCU", "LED_PWR_EN"]   # leaf names, resolved by D7's rules
```

```toml
# bench/cold.toml; `bb run --bench bench/nominal.toml` picks another
schema = "bb-bench/0"
temp_c = "-10"                                      # ngspice .temp
[[supply]]
net = "VSYS"                                        # an ideal source plus an optional series R
volts = "3.0V"
rs = "0.569"                                        # RPACK(-10 °C) 0.50 + BAT-FET 0.055 + harness 0.0142
[[supply]]
net = "+3V3"                                        # a tracking supply: min(volts, V(track) - drop)
volts = "3.3V"
track = { net = "VSYS", drop = "44.2mV" }
[[load]]                                            # a DC current sink
net = "VSYS"
amps = "50mA"
[nominal]                                           # for supply-pin references (ADR-006 D4) on rails no bench source drives
[analog]
nets = ["VSYS", "+3V3", "VLED", "LED_PWR_EN", "LED_DIN_MCU", "LED_DIN_R", "LED_DIN", "GND", "…"]
exclude = ["U1"]                                    # U1 (RT9080) drives +3V3, which a bench supply drives here
[options]
set = "phase0"                                      # a named option set (ADR-006 D6)
```

- **Bindings.** Direction and pin model come from `[[bind]]`, checked
  against `pins.toml` (`models` lists what the pin allows). The parameter
  set key is `set` (one name, here and in ADR-006/008). A connected MCU pin
  without a binding is **unbound**: it gets no glue and loads nothing, like
  an unconfigured nRF pin (high-Z). Its net is probe-only on the MCU side,
  as with Phase 0's `digital_out` without `net=` (pin-model glue rule 4).
  ADR-008 D4 audits unbound pins that firmware configures as outputs.
- **Analog scope.** A part enters the ngspice deck if and only if:
  - all its connected pins lie on `analog.nets`;
  - it is not in `exclude`;
  - it has an analog view at the run's fidelity (ADR-006 D3, with the
    ladder).

  This is the suite's own `passives(nets=…)` rule, extended to ICs. A
  `[[supply]]` on a net that a part's `power_out` pin also drives is an
  error ("two drivers"), unless that part is excluded. Without
  `[analog] nets`, every net is in scope and `bb run` warns. Excluded and
  out-of-scope parts are listed in `run.json`.
- **Bench subset.** Phase 1's bench has: supplies (fixed, or tracking one
  other net with a drop and a ceiling, which is the suite's own
  `B3V3 = min(3.3, V(VSYS) − 44.2 mV)` idiom), DC loads, `[nominal]` rail
  voltages, `temp_c`, PWL sources on nets (`[[pwl]] net, points`), the
  analog scope and a named solver option set. Every number uses ADR-006
  D5's grammar. Phase 2 defines the full bench (CSV playback, timed
  actions, assertions) in its own ADR. That ADR decides YAML or TOML;
  Phase 1 picks TOML for consistency with `part.toml`.
- **Systems (M11, after the exit).** With more than one `[[board]]`:
  - instances and nets are qualified `<board>:<name>`;
  - `[[connection]] a = "main:J1"  b = "base:J3"  model = "<view>"` joins
    mated pins (pin n to pin n unless `pins = [[a, b], …]`), through the
    harness view if given;
  - each board keeps its own ground net.

  Only `simulate` is ever simulated in Phase 1. Off-board nets at its
  connectors are bench ports: a `[[supply]]`, a `[[pwl]]`, or left open
  with a warning. The schema keys exist from v0 so that M11 adds no schema
  change; M4 validates them on a synthetic fixture.
- **Firmware targets** are a list of typed images
  (`{file, kind, offset?, hash}`); Phase 1 implements `kind = "elf"`.
  `bin`, `hex`, `uf2` and `flash` (a merged flash image with offsets, as
  Espressif's QEMU uses) are reserved kinds, each with its own image-hash
  rule when implemented.

### D6. `board.bbm`: the elaborated board (canonical JSON, content only)

`board.bbm` is the output of `bb open`. It is a pure function of:

- the canonical netlist content of each board (not where it came from:
  the `.net` and the `.kicad_sch` route give the same content);
- `bb.toml`'s board ids and grounds, `[[connection]]`, `[instances]` and
  `[packages]` tables (not whether a board is given as `kicad=` or
  `netlist=`);
- the locked package and platform identities.

It is what CI diffs when the schematic changes.

**Out of `board.bbm`, into `.bb/import.json`:** source paths, their sha256,
schematic versions, the tool string, raw-netlist hashes, host and dates.
They change between routes, KiCad 9 and 10, and Linux and Windows, while
the board does not (Context 1, 2). `.bb/import.json` is not canonical and
not diffed; `run.json` copies it.

**Canonical JSON (`spec/canonical-json.md`).** Breadboard's own canonical
form, not RFC 8785: JCS requires numbers to be IEEE 754 doubles, and the
integer sub-units below exceed 2^53 (2^53 fs is about 9.007 s, so a run
longer than 9 s would already be out of range).

- UTF-8; object keys sorted by UTF-16 code unit (as JCS); no
  insignificant whitespace; one `\n` at the end; strings escaped as JCS
  does.
- **Numbers are integers only,** written as arbitrary-precision decimal
  literals: optional `-`, no leading zeros, no fraction, no exponent.
  Readers must parse them as i128 or big integers; a value outside i128 is
  an error. No floats anywhere, in `board.bbm` or `run.json` (ADR-006
  `confidence` is an enum).
- **Quantities are integers in fixed sub-units:**

  | Quantity | Unit |
  | --- | --- |
  | Voltage | µV |
  | Time | fs |
  | Resistance | µΩ |
  | Capacitance | aF |
  | Inductance | fH |
  | Current | pA |
  | Temperature | m°C |

  Original text is kept alongside (`value_text`).
- **Arrays are sorted by their key:**
  - nets by display name;
  - instances by `board` then `ref`;
  - nodes by `ref` then pin number;
  - pins by number.

  All comparisons are bytewise on UTF-8, which is deterministic though not
  "natural" (`R10` < `R2`).

```json
{"schema":"bb-board/1",
 "boards":[{"id":"main","ground":"GND"}],
 "nets":[{"name":"/LED ring/LED_DIN_R","board":"main","spice":"LED_ring_LED_DIN_R","nodes":[{"ref":"R40","pin":"2"}]}],
 "instances":[{"board":"main","ref":"U2","value_text":"TPS22917DBVR","footprint":"Package_TO_SOT_SMD:SOT-23-6",
               "fields":{"LCSC":"C2681320","MPN":"TPS22917DBVR"},
               "package":{"id":"ti/tps22917","version":"0.1.0","tree":"sha256:…"},
               "resolution":{"stage":"mpn","key":"TPS22917DBVR"},
               "pins":[{"number":"1","name":"VIN","net":"VSYS"}]},
              {"board":"main","ref":"R40","value":{"ohm_u":220000000},"value_text":"220",
               "package":{"id":"generic/resistor","version":"0.1.0","tree":"sha256:…"},
               "resolution":{"stage":"generic","key":"Resistor_SMD"}},
              {"board":"main","ref":"J11","status":"excluded","reason":"NFC coil pads: no RF model in Phase 1"}],
 "platforms":[{"id":"renode:nrf52840-bb@1","content":"sha256:…"}],
 "probes":[{"alias":"main:TP1","net":"VSYS"}]}
```

- **Firmware, bench and run options are excluded too.** They go into
  `out/<run>/run.json`, written by `bb run`:
  - `board.bbm`'s sha256, and `.bb/import.json`'s content;
  - the bench file content and its named option set;
  - each target's image hashes (ADR-008 D5);
  - per instance: the requested fidelity, the view used, the rung it was
    chosen at, its provenance, confidence and qualification;
  - excluded and unmodelled parts;
  - package tree hashes and platform content hashes (ADR-006 D8);
  - engine and tool versions;
  - all `bb run` options, including any `--allow-unqualified`.

  `run.json` uses the same canonical JSON and contains no absolute paths
  and no wall-clock times.

  This splits the design doc's single "fully describes the run" file in
  two. The board changes with the schematic; the run changes with every
  invocation. Diffs of either stay meaningful (plan §7 Q7).

**Net names:**

- **Display name:** the KiCad name. In a multi-board project it is
  qualified with `<board>:`.
- **SPICE name:**
  - every character other than `[A-Za-z0-9_]` becomes `_`, and runs of `_`
    and a leading `_` from the sheet path are collapsed;
  - in a system, the result is prefixed with `<board>_`;
  - a leading digit gets an `n` prefix;
  - a name that equals `gnd` case-insensitively gets `_n` appended;
  - the simulated board's ground is `0`;
  - names that collide case-insensitively get `_2`, `_3`, … in display-name
    order.

  Glue element names (`V_<pin>`, `R_<pin>_zout`, `C_bbin_<pin>`) keep their
  rule 3 form over pin names: `U10.P0.06` → `V_U10_P0_06`. Phase 0 names
  are unchanged by these rules (`LED_A`, `PA5`, `ADC_IN`).

### D7. Names, and the CLI: `bb open`, `bb flash`, `bb run`, `bb probe`

**Name resolution** (`spec/project.md`), used by every `bb.toml` key, CLI
argument, probe and bench entry that names an instance, a pin or a net:

- **Instances:** `<board>:<ref>` always works. A bare `<ref>` is accepted
  only if exactly one board has it; otherwise it is an error that lists the
  candidates (`U1 is ambiguous: main:U1 (RT9080-33GJ5), base:U1
  (BQ25180YBGR)`). Pins are `<instance>.<pin name>` (`main:U10.P0.06`); the
  instance part follows the same rule.
- **Nets,** tried in order, the first unique match wins:
  1. the exact display name (`/LED ring/LED_DIN_R`);
  2. the board-qualified name (`main:/LED ring/LED_DIN_R`);
  3. a unique **leaf** name, the part after the last `/` (`LED_DIN_R`),
     within the simulated board first, then the project.

  An ambiguous name at any step is an error that lists the candidates.
  `[[supply]] net = "VSYS"` in a system resolves within the simulated
  board, so it is unambiguous there.
- **Probe aliases** from test points are qualified (`main:TP1`) and follow
  the instance rule.

**The commands:**

- **`bb open [<dir>|<.kicad_pro>|<.net>|<board.cir>]`**
  - Reads `bb.toml` (and creates a minimal one if absent; the only time it
    writes `bb.toml`).
  - Imports each board (D1–D3), resolves (D4), and writes `board.bbm`,
    `bb.lock` (ADR-006 D8), `.bb/import.json`, `.bb/resolution.json` and
    `.bb/unresolved.toml`.
  - Prints a resolution table (stage counts and unresolved parts).
  - Exits non-zero if a `[[bind]]`, probe or analog-scope part is
    unresolved. `--check` writes nothing and fails if `board.bbm` would
    change (for CI).
- **`bb flash --target <instance> <file> [--kind elf]`**
  - Verifies that the instance's package has a behavioural layer whose
    engine accepts that image kind (Phase 1: `renode`, `elf`).
  - Computes the image hash.
  - Writes `[targets."<qualified ref>"]` into `bb.toml` with a
    format-preserving TOML editor (comments are kept).
  - Breadboard simulates, so "flash" means "associate". No device is
    touched.
- **`bb run --for <t> [--bench f] [--fidelity fast] [--probe net:<name|alias>] --out <dir>`**
  - **Read-only on the project.** It checks that `board.bbm` and `bb.lock`
    are current (`bb open --check` logic); if a source, package or
    platform changed, it fails with "run `bb open`" and changes nothing.
  - Checks the bench option set against each in-scope view's
    qualification (ADR-006 D6).
  - Elaborates: bindings to `PinSpec`s and pin models; analog scope to a
    SPICE deck, with each view's subckt text inlined in sorted order, shared
    top-level definitions once (ADR-006 D3), and no file paths.
  - Runs exactly as today, with every Phase 0 option, and writes the same
    outputs plus `run.json`.
  - `--netlist <board.cir>` remains as the Phase 0 path (D9).
- **`bb probe [--list] [net:<name> …]`**
  - Adds probes to `bb.toml` `[probes]`, or lists the probe-able nets and
    aliases (test points, connector pins) with their qualified and leaf
    names.

### D8. PCB

Phase 1 does not need geometry: nothing at `fast` consumes it.

- **Optional (M4 stretch):** a native `.kicad_pcb` reader that checks the
  pad-to-net binding of every footprint against `board.bbm`. It must handle
  both KiCad 9's `(net 4 "GND")` with a net table and KiCad 10's
  `(net "GND")` without one, plus the new via tokens. That catches a
  schematic and board that disagree.
- **Geometry, stackup and extraction** wait for `si` (Phase 3), under their
  own ADR.

### D9. Relation to today's `board.cir` (migration)

| `board.cir` today | Elaborated board |
| --- | --- |
| `*bb part U1 engine=renode platform=stm32f072` | An instance whose package (`st/stm32f072rb`) has `behavioral.platform = "stm32f072-bb@1"`. `bb open board.cir` maps each `platform=` to its package |
| `*bb pin U1.PA5 net=… model=digital_out voh=… …` | A `[[bind]]`. Explicit numbers override the package's parameter set. Lint warns when they differ from it |
| `*bb pin` without `net=` | Unbound, probe-only (glue rule 4) |
| SPICE body lines, `.model`, `.options` | `spice_body` (verbatim text) in `board.bbm`, for hand-written boards only. An imported board has none: its deck is generated from instances and views, and its options come from the bench's named set |
| Node `0` | The ground net |

- **One elaborated-board type, two front ends.** Both `board.cir` and KiCad
  produce the same `bbcore::board` types, and `bb run` consumes only those.
- **Hashes stay.** A `board.cir` body passes through verbatim, so the
  Phase 0 examples keep their committed hashes (ADR-006 gate G2).
- **No deprecation in Phase 1.** `board.cir` stays the quickest way to
  write a test board.
- **The first import gate (M4a).** `examples/adc` is drawn in KiCad (a new
  `examples/adc/kicad/`), imported, and run. Its `nets.csv`, `nets.vcd` and
  non-grant records must equal those of `bb open board.cir`.
  - The target is byte identity with the committed hand-written hashes.
  - The generated deck may order elements differently from the hand-written
    body, and ngspice's sparse ordering could move the last digits. If
    that happens, the gate requires byte identity between the two generated
    routes, a waveform diff within the golden tolerance (1 mV / 1 sample)
    against the committed run, and an identical `pin_edge` list. The cause
    is recorded in an amendment.

## Exact `spec/` changes (described, not applied)

1. **New `spec/board-bbm.md`** (normative): D6 (schema, units, ordering,
   net naming, what is excluded), D3's mapping, D4's resolution stages as
   the `resolution.stage` enum, with a JSON Schema in
   `spec/schema/board-bbm.schema.json`.
2. **New `spec/canonical-json.md`:** D6's canonical form, shared by
   `board.bbm` and `run.json`.
3. **New `spec/project.md`:** `bb.toml` and `bench.toml` (D5), name
   resolution (D7), `.bb/resolution.json`, and how `bb flash` edits
   `bb.toml`, with JSON Schemas.
4. **`spec/pin-model.md`:**
   - Glue rule 3 adds: "pin and net names are mapped to SPICE names by the
     rule of `spec/board-bbm.md` (net naming)".
   - Glue rule 5 is rewritten: "Glue is generated from the bindings of the
     elaborated board (ADR-007); schematics are read through KiCad's own
     netlist export and an S-expression parser, never with regex."
   - A new rule: "an unbound connected MCU pin gets no glue (high-Z)".
5. **`spec/event-stream.md`:**
   - `net` in `pin_edge` and `net_sample` is the **display name**.
   - The `nets.csv` header fields follow RFC 4180 quoting when a name
     contains `,`, `"` or a line break.
   - In `nets.vcd`, `$var` reference names are the SPICE names, and a
     `$comment` block maps each to its display name, sorted.
   - Phase 0 names need none of this, so every committed golden is
     unchanged (gate: `make test` with no golden update).
   - The schema version stays 1: no record gains or loses a field.

## Determinism impact

- **`board.bbm` is canonical** (integer-only canonical JSON, sorted
  arrays) and content-only. It does not depend on:
  - the route (`.net` or `.kicad_sch`);
  - the KiCad version (9 or 10), or a partial KiCad 10 re-save;
  - the host (Linux or Windows `kicad-cli`; CRLF and node order are
    canonicalised);
  - the date line or the temp-dir path.

  Gates M4c and M4d check this.
- **Elaboration is a pure function** of `board.bbm`, the bench, the locked
  packages and platforms, and the options. Generated SPICE contains no
  paths, and instances and inlined subckts are emitted in sorted order.
- **`bb run` writes nothing into the project,** so a run cannot change the
  inputs of the next one.
- **The protocol and engines are unaffected.** Naming changes are display
  only, and the Phase 0 outputs are byte-identical.

## Gates (M4)

- **M4a:** the KiCad-drawn `examples/adc` against `board.cir` (D9).
- **M4b:** the dial-me-spinner main board gives 201 parts, 142 nets and
  657 nodes (re-derived at `6854d25`, Context 12), from the netlist
  vendored at `examples/dial-me-spinner/kicad/main/` (a copy of
  `git archive 6854d25`, its sha256 in `examples/dial-me-spinner/SOURCE`). A synthetic two-board
  fixture in which both boards have `U1`, `TP1` and a `VSYS` net: a bare
  `U1` in `[instances]`, `[[bind]]` and `--probe` is an error listing both
  candidates; `a:U1` resolves; a leaf net name resolves within the
  simulated board. (The dial-me-spinner base board and the mated system
  are M11.)
- **M4c:** `6854d25`'s committed KiCad 10.0.8 `.net`, a fresh KiCad
  10.0.8 Linux export, a KiCad 10.0.8 Windows export, and the `kicad=`
  route on the schematics (as committed, and after a partial KiCad 10
  re-save) give a byte-identical `board.bbm`. **Secondary, not a gate for
  E1 (approver decision C):** `dc96515`'s KiCad 9.0.9 `.net` (vendored
  under `examples/dial-me-spinner/kicad/kicad9-dc96515/`) must give the
  same `board.bbm`, since Context 12 found its D3 content identical on
  the main board; a difference is either an importer bug (fixed) or a
  real connectivity difference (recorded in an amendment). This also
  shows that KiCad 10's `units` and `datasheet` changes are ignored.
- **M4d:** two `bb open` runs are byte-identical, including
  `.bb/resolution.json` and `.bb/unresolved.toml`. The source tree hashes
  are unchanged after `bb open` (no `.kicad_prl`), and `bb run` changes no
  file outside `--out`.
- **M4e:** a schematic stamped `20261106` is refused. A `.net` whose tool
  is `Eeschema 11.…` is refused. A netlist with `[1-4]` pins is refused.
- **M4f:** the value parser and the exact-MPN/pattern resolver on the main
  board give the expected resolution counts, with J11 as the only prompt;
  `RT9080-33GJ5` does not bind to a package that lists only `RT9080`.
- **M4g:** canonical JSON round-trips a time of 2^63 fs and a negative
  value exactly, and a reader rejects a float.

## Rejected alternatives

- **(a) `kicad-cli` and nothing else, i.e. reading its SPICE export.** That
  output loses fields, DNP flags and pin names, and bakes in KiCad's
  simulator conventions.
- **(b) Our own connectivity from `.kicad_sch`.** It matched on this board,
  but the board does not exercise half of the connectivity rules, nor any
  KiCad 10 change (Context 4). It would re-implement KiCad's netlister and
  chase every release. It is kept as an optional cross-check.
- **KiCad's Python API (`pcbnew`, IPC API).** It needs a running KiCad or
  its Python, is version-coupled, and adds nothing for connectivity that
  the CLI lacks.
- **One combined `board.bbm` with firmware and bench** (the design doc's
  wording). Board diffs would then change on every run option (D6).
- **Source paths and hashes inside `board.bbm`.** They differ between the
  two routes, so the M4c identity could not hold.
- **RFC 8785 (JCS).** Its numbers are doubles; the integer sub-units exceed
  2^53 (D6).
- **A token-prefix MPN heuristic.** It silently drops electrically
  significant ordering codes (Context 7).
- **Direction from KiCad pin types.** Measured wrong or missing for 25 of
  42 GPIO nets (Context 9).
- **Net classes for ground detection.** KiCad 9 and 10 disagree on them
  (Context 2). An explicit `ground` is unambiguous.

## Open questions for the approver (consolidated, with recommendations)

1. **`board.bbm` and `run.json` as separate files,** with `board.bbm`
   content-only and source metadata in `.bb/import.json` (D6), rather than
   the design doc's single file: accept? **Recommended: accept.**
2. **Breadboard canonical JSON** (integer-only, arbitrary precision) instead
   of RFC 8785: accept? **Recommended: accept** (JCS cannot carry the
   units).
3. **Exact-MPN resolution** with package-declared patterns, instead of any
   resolver heuristic (D4): accept? **Recommended: accept;** it makes the
   first import resolve fewer parts until packages declare patterns.
4. **Project overrides before the `bb:part` field** (D4 stage 1): accept?
   **Recommended: accept** (Breadboard must work without edits to the
   user's KiCad files).
5. **Output-naming changes** in `spec/event-stream.md` (RFC 4180 header
   quoting; VCD SPICE names plus a mapping comment), or require probes to
   use SPICE-safe aliases instead? **Recommended: the naming changes.**
6. **Older KiCad (6–8)** accepted with a warning, or refused until tested?
   **Recommended: warn.**
7. **The optional PCB cross-check** (D8): in Phase 1, or deferred entirely?
   **Recommended: defer** unless M4 finishes early.

## Review notes (2026-11-09)

Two critiques of the first draft. Adopted: content-only `board.bbm` (D6),
qualified names and the name-resolution rules (D7), integer-only canonical
JSON instead of JCS (D6), exact-MPN resolution (D4), the `.net` tool-major
gate (D2), the KiCad 10 `units`/`datasheet` differences (Context 2), a
read-only `bb run` with stubs outside `bb.toml` (D4, D7), typed firmware
images (D5), `.bb/resolution.json`, the corrected resolution count, and
systems moved to M11. Not adopted as proposed:

- **"Support `--variant` instead of refusing schematics with variants."**
  Deferred. A variant changes which parts exist; recording it in
  `board.bbm` needs a decision on how probes, bindings and goldens name
  variant-specific parts. The reference board has no variants, so Phase 1
  refuses them on the schematic route and records the tool string on the
  `.net` route.
- **"Prefix-rename every view's top-level names."** See ADR-006 Review
  notes: Phase 1 refuses differing duplicates instead.
