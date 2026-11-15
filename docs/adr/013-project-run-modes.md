# ADR-013: Project run modes: devices in `bb.toml`, single-MCU system runs, runs without a CPU target

- **Status:** Accepted (2026-11-15; see "Approver decisions
  (2026-11-15)"). Proposed 2026-11-14, revised after design review round 1
  (see "Review notes").
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
