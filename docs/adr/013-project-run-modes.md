# ADR-013: Project run modes: devices in `bb.toml`, single-MCU system runs, runs without a CPU target

- **Status:** Accepted (2026-11-15; see "Approver decisions
  (2026-11-15)"). Proposed 2026-11-14, revised after design review round 1
  (see "Review notes"). D1 implemented in R1 (Amendment 1); the majors
  landed in milestone S (Amendment 2); D2 implemented in R2 (Amendment 3).
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
     `dms-home-project` (both `all`: one partition). It passed with them:
     27 configurations, 2 rounds, every pooled run leased and byte-identical
     to its fresh run and to the committed hashes (104 min under load).
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
   on an idle host. The full gate took 9 min 0 s under the same load;
   its committed run passed pooled (714 s) and with `POOL=0` (1 014 s),
   under a 1-minute load of 10-70. `make check` passed pooled (1 725 s)
   and with `POOL=0` (1 696 s; `dms-home-test` 73.8 s) under that load.
10. **Cost-table row.** `make cost-table-dms-home-project` (also in `make
    cost-table`): `project-step`, the one-cell reference against bb's
    defaults, fresh and on a 1-slot pool, with the ADR-003 protocol. On E3
    the two are one partition, so the row's ratio is 1 by construction. It
    was not measured: the host was never idle while other tracks ran, and
    the protocol requires an idle host.

## Amendment 2 (2026-11-15): the schema epoch (milestone S)

Milestone S landed `bb-project/1` and `bb-board/1`. R1's drafts (Amendment
1, item 1) are S's majors with the same keys. ADR-011 Amendment 1 is the
record. S adds these `bb-project/1` keys (`spec/project.md` section 1):

- **`[project] simulate`** may be a list (D2). A list of one board means
  that board; a list of several is refused, naming R2, until R2
  implements system runs.
- **`[run]`** (`cpu = "none"`, `lattice`; D3) is refused, naming R3, which
  is not in this build.
- **`[registries.<name>]`** (ADR-011 D4) is read, checked and inert.

`bb pkg migrate --project` and `--board` rewrite the schema line or member.
The board-file form of `bb run --elaborate-only` (it writes the analog
engine's netlist) is S's deck gate, recorded in ADR-011 Amendment 1.

## Amendment 3 (2026-11-16): R2 as implemented (D2)

Milestone R2 implements D2 on track `r` (`p2/r`). `spec/project.md`
sections 1, 2, 4 and 6 state the rules; this is the record. A one-board
run is unchanged: every one-board deck and output is byte for byte what it
was (`make test`'s committed hashes; `make dms-system`'s E1-on-the-system
elaboration).

1. **`simulate` as a list** (S's key, Amendment 2, now honoured). Each
   entry a declared board, none twice. The **primary board** is the
   target's: the board a `[targets]` key names by its qualifier when one
   names a listed board, else the first listed. Its ground is node 0;
   `bb run` refuses a target on another board. One target across the
   listed boards (G3: a second is refused, "multi-MCU simulation is Phase
   3"). A `[[device]]` stays on the primary board, because a device's
   kernel nets are `<ref>.<pin>` without the board (a deviation from
   nothing D2 says, recorded).
2. **Names.** In a system run "the simulated board" of ADR-007 D7's leaf
   rule is every listed board, so a leaf name on two of them is ambiguous
   and must be qualified, in a bench, a binding or a probe (G3: `VSYS` is
   refused, listing `base:VSYS` and `main:VSYS`; so is a bare `U1` in
   `[corners]`).
3. **The deck.** Every listed board's instances, in `board.bbm`'s order;
   each board its own ground (`base:GND` is `base_GND`). Two rules D2 did
   not spell out:
   - **A bench element returns to its net's board's ground** (supplies,
     batteries, loads, PWL and playback sources, a `short` fault), its
     B-source terms read `V(net,ground)`; on node 0 the one-board form, so
     no deck moves. A battery at J2 then drives its current through the
     base board's ground and the harness's ground wires, as on the board.
   - **The harness** is one line after the instances,
     `X_conn<k>_<a>_<b>`. A mated pair with both nets in the analog scope
     is wired; with both outside, a **dead wire** on node 0 at both ends
     (listed in a warning); one in and one out is refused (list both or
     neither: the wire would join them). Without the dead-wire rule the
     dial-me-spinner harness could not enter a deck: USB_DM and USB_DP have
     nothing else in it, so their wires would float. Its view must be valid
     at the run's rung or below; `dial-me-spinner/link-harness:wires` (plain
     resistors) now declares `fast` and `pi` (ADR-009 D1, D2; its
     qualification is unchanged, its tree hash moved, `system/bb.lock`
     relocked, `board.bbm` unchanged).
   - A connection without a harness ties the mated nets into one node (`0`
     with the primary ground, else the primary board's net's name).
   - `run.json`: `simulated` (only in a system run) and the harness in
     `analog` (`ref` `main:J1=base:J3`, `kind = "harness"`).
4. **The project: `examples/dial-me-spinner/e1-system`.** E1's target,
   bindings and probes on both boards (`simulate = ["main", "base"]`), and
   two benches, `bench/cold.toml` and `bench/nominal.toml`, that are E1's
   with the VSYS supply replaced by the base board's power path, as the
   suite's `vsys_network` has it: a `[[battery]]` at `base:VBAT` (the
   corner's 3.0 / 3.7 V behind RPACK 0.50 / 0.25 Ω), base U1
   (`ti/bq25180:pp`, RON_BAT 55 mΩ as the suite's) at `suite_compat`, the
   base board's C2, C3 and C4, and the harness. Choices, each measured:
   - **`suite_compat` for base U1**: the suite's battery-only block has no
     BUVLO gate. At `typ` a 3.0 V cell (below BUVLO's 3.15 V rising
     threshold) would never close the gate, by the view's equations, so the
     cold corner would have no VSYS; that corner is below the firmware's
     3.465 V floor anyway (not run).
   - **+3V3 stays the suite's ideal LDO** (E1's tracking supply, the 50 mA
     budget on VSYS) and main U1 is excluded, because the reference deck has
     exactly that. With the RT9080 (`richtek/rt9080:ldo`, `worst_dropout`)
     in the deck instead, the cold corner ran (VLED within 6.72 mV of the
     reference) but the **nominal corner's operating point failed**: gmin
     stepping fails with the RT9080 regulating (VIN 3.68 V), then source
     stepping scales away the B-source paths of the `saadc_se` hold node
     ("singular matrix: check node __hold_u10_p0_02"). Standalone, without
     the pin load, source stepping converges; P1's decks give the RT9080 a
     `.nodeset`, which a bench cannot. **Open finding for P4**: W1 with the
     firmware in the loop needs the RT9080 in a `bb run`, so it needs an
     operating-point guide or a restated view; not R2's to decide.
   - **No USB**: VBUS is a `[[supply]]` of 0 V behind 1 kΩ (a Norton B
     conductance). A `[[pwl]]` with `rs` there puts a voltage source, a
     branch-current unknown, on `base_GND`, which the harness's milliohm
     GND wires tie to node 0: AGENTS.md's known bad idea. Measured: the
     nominal run stalled (`NoProgress:ngspice` at 70.26 ms, the watchdog at
     200 001 points in one cell, after 541 s); with the Norton it completes
     in 6 s.
5. **Gate G2, as measured** (`make dms-e1-system`, `scripts/dms_e1_system.sh`;
   numbers from the committed run, item 7):

   | Check | ok-cold | ok-nominal |
   | --- | --- | --- |
   | **Cross-check: VLED against the suite's `led_rail_sequence` battery-network deck** (the run's `pin_edge` records replayed into it, the fitted TPS22917 view in both; every 10 µs sample; limit 10 mV) | **3.164 mV** (4.07 ms) | **3.219 mV** (4.15 ms) |
   | Negative control: the cell's R0 doubled (1.0 Ω), must fail | 49.202 mV: fails | — |
   | Reported: the suite's deck with E1's bench supply vs its battery network (what E1's bench leaves out) | 2.472 mV (74.22 ms) | 1.515 mV (4.18 ms) |
   | Reported: the system run vs E1's bench run (`bb run`, VLED) | 5.616 mV (74.21 ms) | 4.701 mV (4.17 ms) |
   | E1's four checks (`check_dms_rail.py --board main`) | pass (VLED 2.969854 V at the P0.06 rises; LSW-2 24.420 ms) | pass (3.682364 V) |
   | Equivalence: default (71 grants, 14 quiet) = one-cell reference (`--max-quantum 10us`, 12 000 grants) except grant records | identical | identical |
   | Analog-first byte-identical, grant records included | yes | yes |
   | Board-level convergence, 120 ms (limit 60 s), both engine orders | 2.9-10.8 s wall; one-cell reference 15.7 / 33.3 s (pooled / fresh) | 2.8-11.9 s; 14.1 / 33.1 s |
   | Determinism, 10 runs | identical | — |
   | Wrong-order firmware | fails exactly LSW-3 | — |

   The system run is further from the suite than E1's bench (3.2 mV against
   1.6-2.5 mV): the harness model carries its contacts (28.4 mΩ on VSYS and
   18.9 mΩ on GND, against the suite's lumped 14.2 mΩ), so the inrush sags
   more. **No vacuous pass:** `scripts/check_system_run.py` requires, on
   every run, `simulated`, base U1 at `suite_compat` and the base
   capacitors in the deck, no bench VSYS supply, the harness between main
   and base nodes with its ground wires on 0 and `base_GND`, the base
   elements on `base_GND`, and the load carried (base:VSYS above main:VSYS
   by 1.42-3.95 mV, base:VBAT below the OCV, whenever VLED is above 2.5 V).
   The cross-check needs `DMS_REPO` and the suite venv, so it is local, as
   `make dms-crosscheck` is; CI runs `dms-e1-system-test`.
6. **Gate G3** (`cli/tests/system_run.rs`, in `cargo-test`): the system
   deck line for line (grounds, harness, bench returns, `run.json`), a
   second target refused, ambiguous bench names refused (a net in `[[load]]`
   and `[analog] nets`, an instance in `[corners]`), a harness wire half in
   the scope refused, and a connection without a harness tying its nets.
   `names::tests` has the leaf rule.
7. **Runs and budget** (`make test` +30 s): `dms-e1-system-test` took
   15.8 s alone on its own pool (with the pool's start; its three runs go
   in parallel), 3.4 s in a pooled `make check` (1 234 s in all) and
   38.7 s in a `POOL=0` one (1 485 s), both green, both beside another
   track's `make check` (1-minute load 7-46, so to be re-measured idle).
   The full gate took 93 s pooled and 169 s with `POOL=0` (load 9-20).
8. **`make pool-equiv`** gains `dms-e1-system` and `dms-e1-system-ref`
   (ok-cold, default and one-cell). It passed with them: 32
   configurations, 2 rounds, every pooled run leased and byte-identical to
   its fresh run and to the committed hashes (57 min, load 5-17).
   `make lock-scope` passes with the project in `PKG_BOARDS`.
   **Cost-table row** (`make cost-table-dms-e1-system`, also in `make
   cost-table`; ok-cold, the one-cell reference against bb's defaults, the
   ADR-003 protocol, N = 5, interleaved, medians, under the measurement
   lock): fresh, 12 000 against 71 grants, simulation 20.71 / 4.89 s
   (4.2x), total 23.86 / 9.25 s; on a 1-slot pool, 17.23 / 3.31 s (5.2x),
   total 17.43 / 3.51 s, startup 0.17 / 0.16 s. Other tracks raised the
   1-minute load from 4.9 to 37 during the fresh round (9-14 on the pool),
   so the row is to be re-measured on an idle host.
9. **Deviations, recorded.**
   - +3V3 is not the RT9080 (item 4): the cross-check's reference has the
     suite's ideal LDO, and the RT9080 view does not reach an operating
     point in a `bb run` at the nominal corner (open finding for P4).
   - The harness keeps its JST contacts (47.3 mΩ round trip against the
     suite's 14.2 mΩ); the difference is inside the 10 mV and reported.
   - The cross-check is local (`DMS_REPO`, the suite venv), as Phase 1's
     `dms-crosscheck` is; CI and `make test` run `dms-e1-system-test`.
   - Devices on the primary board only (item 1).
   - `scripts/check_dms_rail.py --board main` and `dms_suite_deck.py
     --battery` / `compare --run-col` read a system run's qualified columns;
     their Phase 1 behaviour is unchanged.
10. **At the merge into main (after P2), `tests/golden/dms-e1-system`
    re-recorded.** R2 branched at `29e5006` (H2's lattice snap already in,
    so D-SNAP is not a difference); main then gained B3 (result-neutral)
    and P2 (ADR-009 D5, Amendment 5: at `pi` a `digital_out` drives from its
    rail through `zout`, tristate by `oe` = `PIN_CNF.DIR`;
    `nordic/nrf52840-qiaa` 0.2.0, `io_rail = "VDD"`; `e1-system/bb.lock`
    relocked to it). The e1-system benches are `fidelity = "pi"`, so both
    MCU outputs now carry the glue, on `main:+3V3`. Attributed per sample
    against p2/r's own outputs (`c31d95b`, its bb, which reproduces its
    committed hashes), 12 001 samples per case, 120 ms:
    - **`oe`** (`bb run --pi-oe fixed` against the default): two new
      `pin_edge` records, `U10.P1.04#oe` and `U10.P0.06#oe` rise at
      0.120 ms and 0.130 ms (the firmware's `PIN_CNF` writes; high-Z
      before); the 8 GPIO `pin_edge` records are unchanged. With
      `--pi-oe fixed` the non-grant records equal R2's byte for byte. Its
      effect on the probes: at most 8 µV (cold) and 15 µV (nominal), around
      the two `oe` instants and the inrush.
    - **The rail** (`--pi-oe fixed` against p2/r): cold, VOH is now +3V3
      (VSYS - 44.167 mV, 2.925 V) instead of the set's 3.3 V, so while P0.06
      is high LED_DIN_MCU is 374.203 mV lower and LED_DIN, LED_DIN_R
      373.382 mV lower (1 007 samples); every other probe within 9 µV.
      Nominal, +3V3 is 3.3 V: the DIN chain within 2 µV, every other probe
      within 1 µV.
    - Combined (merged default against p2/r): cold VLED 7 µV, main:VSYS
      9 µV, +3V3 10 µV, base:VSYS 9 µV, base:VBAT 8 µV, the DIN chain
      374.203 mV (rail); nominal VLED 7 µV, main:VSYS 15 µV, base:VSYS
      14 µV, base:VBAT 12 µV, +3V3 1 µV, the DIN chain 2 µV.
    E1's checks move as P2's E1-at-`pi` record says: ok-cold LSW-3 0.000 V
    (was 0.322 V; LED_DIN now never rises above VLED), wrong-cold still
    fails exactly LSW-3 (0.777 V). `make dms-e1-system` on the merged tree:
    gate PASS; cross-check 3.163 mV (cold) and 3.217 mV (nominal), the
    negative control 49.201 mV; default = one-cell reference except grants
    (71 against 12 000); analog-first and 10-run determinism identical.
    The new hashes are the one-cell references (`make
    dms-e1-system-update`), which the default runs equal.
