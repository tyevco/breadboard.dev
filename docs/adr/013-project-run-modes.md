# ADR-013: Project run modes: devices in `bb.toml`, single-MCU system runs, runs without a CPU target

- **Status:** Accepted (2026-11-15; see "Approver decisions
  (2026-11-15)"). Proposed 2026-11-14, revised after design review round 1
  (see "Review notes"). D1 implemented in R1 (Amendment 1).
- **Part of:** the Phase 2 plan, milestones R1-R3. W1, W2 and the caliper
  analog-only example use it. `bb-project/1` and `bb-board/1` land in
  milestone S, the schema epoch.
- **Amends:** ADR-007 (D5, Amendment 5 item 8) and ADR-006 Amendment 6
  (A6.5, route B).
- **Normative changes when implemented:**
  - `spec/project.md` (`bb-project/1`);
  - `spec/package.md` §12, a pointer only;
  - `spec/board-bbm.md` (`bb-board/1`: a `virtual` flag on instances);
  - `spec/schema/project.schema.json`.
- **Evidence:**
  - Approver decision 2 (2026-11-14): route B goes to Phase 2.
  - E3 runs on hand-written board files (`examples/dial-me-spinner/e3/*.cir`)
    because a project cannot carry a device.
  - ADR-007 Amendment 5 item 8: "What it would take to run the system".
  - `cli/src/elab.rs:309` requires exactly one MCU target.
  - caliper (ESP32-S3) cannot run on Renode, and its docs say no
    simulation exists.

## Summary

Three additions to `bb.toml`, all within one MCU:

- **Devices:** `[[device]]`, with pin-to-net bindings.
- **Systems:** `simulate` may list several boards, simulated in one
  analog deck.
- **No target:** `[run] cpu = "none"` runs ngspice, devices and the bench
  without a CPU.

Multi-MCU simulation stays in Phase 3.

## Decision

### D1. Devices in projects (route B)

```toml
[[device]]
ref = "main:M1"                      # an instance of the board (the KiCad symbol of the mechanism, or a virtual ref)
package = "dms/detent-mechanism"     # a package whose [behavioral] is engine = "device"
params = { ts = "50ms", s0 = "500" }
virtual = false                      # true: no schematic symbol; ref must not collide

[[device.bind]]
pin = "IN_A1"
net = "main:MOT_A_IN1"
model = "digital_in"
[[device.bind]]
pin = "HOME_N"
net = "main:HOME_N"
model = "digital_out"
```

- **Two cases:**
  - a device on a schematic symbol (resolution stage 1 already maps an
    instance to the package);
  - a **virtual** device: a physical thing with no symbol, such as the
    mechanism, the magnet or the motor's load.
- **The rules of `spec/package.md` §12 apply unchanged:**
  - routing through the kernel net, for a device driving a CPU input or
    the other way round;
  - `digital_out` glue and `digital_in` thresholds against the analog
    side;
  - `adc_in` refused;
  - the device's identity locked as `device:<id>@<major>`.
- **Names:**
  - `bb open` resolves `net` by the project's name rules (ADR-007 D7);
  - `board.bbm` records virtual devices as instances with `virtual: true`;
  - a device binding is part of the board's content, so it changes
    `board.bbm`.
- **The E3 board files** become a generated cross-check: R1's gate
  compares them with the project.
- **Long grants on the project route.** ADR-014's long grants need the
  MOT nets out of the analog engine. On a project that means leaving them
  out of `[analog] nets`, which takes R70 and any pin loads on them out of
  the deck (ADR-014 D4: a bound digital net outside the analog scope gets
  no glue). The dial-me-spinner project's E3 bench does that; a project
  that keeps them in scope runs correctly at one-cell grants.

### D2. Single-MCU system runs

- **`[project] simulate`** takes a list (`["main", "base"]`). A string
  keeps its Phase 1 meaning.
- **The deck contains every simulated board's in-scope instances:**
  - each board keeps its own ground node, with the simulated target's
    board ground as node `0` and every other board's ground as a named
    node;
  - every `[[connection]]` between simulated boards is instantiated
    through its harness view (ADR-007 Amendment 5);
  - a connection without a harness ties the mated nets directly.
- **Exactly one target** across the simulated boards.
- **Bench names are qualified** where ambiguous (`base:VBAT`). A bench
  port at a connector to an unsimulated board keeps its Phase 1 meaning.
- **The analog scope is per board:** `[analog] nets` takes qualified
  names.
- **A board-level convergence gate is required** for the bigger deck, as
  M6 had: the run at both engine orders within a stated wall budget.
- **Out (Phase 3):** a second target. A second MCU is refused with
  "multi-MCU simulation is Phase 3".

### D3. Runs without a CPU target

- `[run] cpu = "none"` in `bb.toml`, or `bb run --no-target`.
- **Engines:** ngspice, the bench engine (ADR-010) and any devices.
- **`run.json`** records `targets: []` and `cpu: "none"`. `bb flash`
  refuses such a project.
- **Unbound MCU pins** of a resolved MCU instance with no target are
  high-Z (glue rule 8). The bench may drive their nets as stand-ins,
  through playback or PWL. For caliper, the ESP32-S3's supply current is a
  bench `[[load]] profile`, and the report labels it as a bench stand-in.
- **The grant lattice still applies.** It gains an engine-neutral name,
  `--lattice` (and `[run] lattice` in `bb.toml`), with `--renode-quantum`
  kept as an alias, so a run with no Renode, and a Phase 3 QEMU backend,
  do not name an engine they do not use. The alias changes no output:
  `run.json` keeps the key it records today. The same partition rules
  hold, so equivalence gates mean the same thing.
- **Adoption.** A no-target run whose assertions are on its analog side
  counts as an external board in the plan's adoption exit; the report
  labels it "no CPU target".

## Exact `spec/` changes (described, not applied)

- **`spec/project.md` §1, `bb-project/1`:**
  - `[[device]]` and `[[device.bind]]`;
  - `simulate` as a list;
  - `[run] cpu`.

  `bb-project/0` files are read and migrated (`schema` line only).
- **`spec/project.md`:**
  - §3 (bench): qualified names across simulated boards;
  - §4 (deck): the per-board ground rule and harness instantiation;
  - §4 (deck): a bound digital net outside `[analog] nets` gets no glue
    (ADR-014 D4);
  - §5 (verbs): `bb run --no-target`, `--lattice`;
  - §6 (`run.json`): `cpu`, `simulated` (the list of boards).
- **`spec/package.md` §12:** "a device part exists only in a board file"
  becomes "in a board file, or in a project's `[[device]]`".
- **`spec/board-bbm.md` §3:** a `virtual` flag on an instance. The schema
  is strict (`spec/schema/board-bbm.schema.json` refuses unknown keys), so
  the flag makes `bb-board/1`, with a mechanical migration (ADR-006 D11).

## Determinism impact

- Devices: none new (ADR-006 Amendment 6).
- Systems: deck order is defined (sorted by board, then reference), so a
  system deck is a pure function of `board.bbm`, the bench and the
  packages.
- No-target runs: the same kernel, minus one engine.

## Gates

| # | Gate |
| --- | --- |
| G1 | R1: E3's `step` and `typ` on the project equal the board-file runs byte for byte after the documented net-name mapping, or (if names in the deck differ) their own one-cell references; `make pool-equiv` entry |
| G2 | R2: E1 on the system project with the base board's battery path: VLED within 10 mV of the suite's `led_rail_sequence` battery-network deck at the same corner (the reference E1 could only report); 120 ms in < 60 s at both engine orders; one-cell equivalence; determinism |
| G3 | R2: a second target is refused; a bench name ambiguous across boards is refused |
| G4 | R3: a no-target run of a synthetic power board with playback, 2 s, partition-independent at 10 µs, 1 ms and 10 ms, deterministic |
| G5 | Every committed project migrated to `bb-project/1` and `bb-board/1` gives its committed hashes |

## Rejected alternatives

- **Keeping devices in board files only.** Every imported project would
  need a hand-maintained board file, and E3 already shows the drift risk.
- **Running the base board's power path as a bench element** for W1 and
  W2. It duplicates models that belong to packages on the base board, and
  the suite's own reference is the system.
- **Multi-MCU now.** It needs a second CPU engine instance in one run, a
  time-grant analysis for two lookahead candidates, and pool work. That is
  Phase 3.

## Questions for the approver

Consolidated in the plan's section 9 as **Q5** (single-MCU system runs in
Phase 2).

**Defaults (apply unless the approver objects):**

1. **Virtual devices** (no schematic symbol) in `bb.toml`, flagged
   `virtual` in `board.bbm`.
2. **No-target runs as a user-facing mode**, for caliper and for any
   board whose MCU Renode cannot run.
3. **`--lattice`** as the engine-neutral name, `--renode-quantum` an
   alias.

## Approver decisions (2026-11-15)

All twelve questions of the plan's section 9 were accepted with their
recommended answers (plan section 10). For this ADR:

- **Q5, accepted.** Single-MCU system runs are in Phase 2 (D2), because W1
  needs the base board's power path. If R2 slips, the bench fallback is
  recorded.
- **Q3, accepted.** `bb-project/1` and `bb-board/1` land in milestone S.
- **Build scope.** R1 and R2 are in this build. R3 (runs without a CPU
  target, D3) belongs to the adoption exit and is not started; D3 stands
  as the design for when it does.

**Defaults.** No objection was made to the defaults listed under
"Questions for the approver", so they apply as written.

## Review notes (design review round 1, 2026-11-14)

**Adopted:**

- **Project-route long grants** (major, against ADR-014): D1 says how a
  project leaves the MOT nets out of the analog engine, and that a project
  which keeps them runs correctly at one-cell grants.
- **Engine-neutral names** (minor): `--lattice`.
- **No-target runs and the adoption exit** (minor): they count, labelled.

**Not adopted:** none.

## Amendment 1 (2026-11-15): R1 as implemented (D1)

Milestone R1 implements D1 on track `r` (`p2/r`). D2 (R2) and D3 (R3, out
of this build) are not touched. The kernel, the time-grant protocol and
every committed output of a Phase 1 board are unchanged.

1. **The schema majors (the draft of milestone S).** `[[device]]` needs
   `bb-project/1` and the `virtual` flag needs `bb-board/1`, and S, which
   lands every Phase 2 major in one commit, has not merged. Following plan
   section 4 ("tracks develop against the epoch's draft schemas"), this
   branch carries those two majors as D1 states them:
   - `bbpkg::schema::Registry` is at `bb-project/1` and `bb-board/1`;
     their `/0` files are the previous major (ADR-006 D11), read with a
     migration hint, migrated by the schema line (member) only.
   - `spec/schema/project.schema.json` adds `device`;
     `spec/schema/board-bbm.schema.json` adds an instance's `virtual`.
   - The committed `bb.toml` files (`examples/adc/kicad`,
     `examples/dial-me-spinner`, `…/system`, `tests/fixtures/kicad/
     two-board`) are migrated by their schema line, and their `board.bbm`
     files by `bb open`: the schema member is the only byte that changes,
     so no deck and no output hash moves (gate G5 for these two majors).
   - **For the merger:** this branch writes `/1` files, so S must merge
     first (plan section 4). When it has, the majors here reduce to S's
     (the same keys); only D1's use of them is R1's.
2. **`[[device]]`** (`spec/project.md` section 1). Keys `ref`, `package`,
   `params` (a table of value-grammar strings), `virtual` and
   `[[device.bind]]` (`pin`, `net`, `model`, `set`, pin-model parameters).
   Every rule of `spec/package.md` section 12 is applied by the board-file
   code (`cli/src/board.rs` `device_params`, `device_pin`, `build_pin`), so
   the two routes cannot drift: unknown or out-of-range parameters,
   `adc_in`, a model against the pin's direction, a package that is not a
   device, and a second driver on a net are refused. A device on the
   simulated board only. Its kernel nets are `<ref>.<pin>` without the
   board, as the target MCU's (`M1.HOME_N`), so a board file and a project
   name them alike.
3. **`board.bbm`.** A virtual device is an instance with `virtual: true`,
   empty `value_text`, `footprint`, `lib`, `part` and `sheet`, the device
   package, `resolution = {stage: "override", key: <package as written>}`
   (the `[[device]]` entry is a project override; no new stage), status
   `ok`, and the device package's pins on the nets their bindings name. The
   bound pins join those nets' `nodes`. Device parameters are not board
   content (they are run configuration, in `run.json` `devices`), like an
   MCU binding's. The device identity `device:<id>@<major>` is in
   `platforms` and `bb.lock`, as for a board file.
4. **ADR-014 D4's project rule is implemented here.** D1 depends on it
   (this ADR's own `spec/` list names it), and it needs no protocol
   change: a bound `digital_in`/`digital_out` net outside `[analog] nets`
   gets no glue, every input on it must read a device route (else an
   error), a probe of it by name is refused with its driver's pin, and
   `run.json` lists it as `digital_only`. Without `[analog] nets` nothing
   changes, and every committed project keeps its bound nets in scope, so
   no committed deck moves (`make test` unchanged). It is the project half
   of D4 only: H5's board-file directive `*bb digital`, the model change of
   E3's board files and D1-D3 of ADR-014 are H5's. If G0 withdraws
   ADR-014, D4's default 2 decides this rule's fate too.
5. **E3 on the project route** (`examples/dial-me-spinner/e3/home`, the
   `typ` case; `e3/step`, the `step` case). Two projects, because the
   cases differ in firmware, in M1's `s0` and in J40's view, and a bench
   cannot carry device parameters. Each imports the vendored main-board
   netlist; `M1` is a virtual `[[device]]` on the five driver commands and
   HOME_N; U42 (the DRV5032FA, inside M1) is excluded by an override; J40
   resolves to a project-local package, `dial-me-spinner/detent-stepper`
   (`e3/packages`; the motor's two phases, 20 Ω + 4.3 mH, view `coils`,
   and `step_emf` with the board file's back-EMF window, qualified at
   (10 µs, `phase0`) by its own vectors). The benches stand in for the
   base board's power path at DC: 3.7 V behind 0.370667 Ω (the suite's
   cell, BAT-FET and link resistances summed), 150 mA, and +3V3 behind
   50 mΩ. `e3/home/bench.toml` leaves the MOT nets out of `[analog] nets`
   (so U40, U41, R70 and J40 leave the deck and the five MOT nets are
   digital-only); `e3/step/bench.toml` keeps them and the bridges' outputs.
6. **Gate G1, as measured** (`make dms-home-project`,
   `scripts/dms_home.sh project`, `scripts/compare_project_run.py`):
   - **The documented name mapping is the identity on every kernel net**
     (`U10.P0.26`, `M1.HOME_N`), and a project's analog column (a display
     name, `/Detent actuator & home sensor/MOT_A_OUT1`) maps to the board
     file's node by its leaf name.
   - **Byte for byte after the mapping:** both cases' `events.jsonl` equal
     the board files' committed ones, grant records included
     (`tests/golden/dms-home/{step,typ}`: non-grant records `828e5ed1…` and
     `3975cb11…`), and so does every digital column of `nets.csv`.
     `project-typ` matches although its deck has no MOT pull-downs and no
     glue on the MOT nets: none of the homing's records depends on them.
   - **Not byte for byte: the analog columns**, because the decks differ
     (the project's is elaborated from the KiCad netlist: element and node
     names, the schematic's capacitor values without the board file's
     derating and ESR, the bridge view's default rDS(on) 0.28 Ω against
     the board file's 0.33 Ω, and no base-board capacitors). Measured
     largest differences: `step` VSYS 26.3 mV and MOT_A_OUT1/MOT_B_OUT1
     270 mV (on the PWM edges); HOME_N and MOT_NSLEEP identical; `typ`
     VSYS and HOME_N identical. So, as the plan's row allows, `nets.csv`
     and `nets.vcd` have **their own one-cell references and goldens**
     (`tests/golden/dms-home/project-step`, `project-typ`).
   - **A consistency check, not an equivalence:** on E3 bb's defaults and
     the one-cell reference (`--lookahead off`) build the same partition
     (4 500 and 270 000 one-cell grants, no quiet grant), as for the board
     files (ADR-008 Amendment 12). `check_dms_home.py --corner typ` passes
     on `project-typ` (H0-H5, every phase's step rate, the latency
     44.302 ms). Analog-first is byte-identical on `project-step`.
   - **Negative control:** `project-step` at `--seed 2` moves M1's seeded
     sample phase; `compare_project_run.py` must fail it on M1's first
     record (it does), so the comparison is not vacuous.
   - **`make pool-equiv`** gains `dms-home-project-step` and
     `dms-home-project` (both `all`: one partition).
7. **Deviation: the step case's suite checks stay on the board file.**
   `check_dms_home.py --step` reads COILA_L and COILB_L (nodes inside J40's
   view, which a project probe cannot name: `spec/project.md` section 2)
   and NB_VBAT and NB_VSYS (the base board's, which needs R2). Its
   event-derived checks (the phases, the duty line, the PWM schedule, the
   step's start against the EMF window) read records that are byte-
   identical to `step`'s, so their verdicts carry over; its analog checks
   (the current vector, the thrust, VSYS max, BAT and the link current)
   remain gated on `board-step.cir` by `make dms-home-test`. R2's system
   project can carry them.
8. **ADR-014's long grants on the project route (reported).** The home
   bench leaves the MOT nets out of `[analog] nets`, so `project-typ` has
   them digital-only (`run.json`: MOT_A_IN1, MOT_A_IN2, MOT_B_IN1,
   MOT_B_IN2, MOT_NSLEEP) and no U10 → ngspice edge on them. It still takes
   270 000 one-cell grants with 0 quiet grants: two producers (M1 and
   ngspice) feed U10, which today's quiet-grant rule refuses. Long grants
   need H5's D1-D3; with them, the project route has D4's graph.
9. **Budget** (`make test` +15 s). `dms-home-test` runs `project-step` in
   the same parallel batch as `step` and `step-order` (three jobs on the
   5-slot pool). Measured interleaved, N = 3, under the measurement lock
   but beside other tracks' builds and gates (1-minute load 16-48, so not
   an idle host): `dms-home-test` 62.4 s median before, 67.2 s after
   (+4.8 s). The baseline host's idle figure (18.3 s) is to be re-measured
   on an idle host. The full gate took 9 min 0 s under the same load.
