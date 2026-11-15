# ADR-014: Quiet grants with several producers, and promises that inputs cannot end

- **Status:** Accepted (2026-11-15) **behind gate G0** (see "Approver
  decisions (2026-11-15)"): if G0 misses its threshold, this ADR is
  withdrawn. Proposed 2026-11-14, revised after design review round 1
  (see "Review notes"). Kernel protocol: written before implementation
  (AGENTS.md). **A prototype must measure the gain** (gate G0) before
  D1-D3 are implemented beyond it, and before the rest of H5 is costed.
- **Part of:** the Phase 2 plan, milestone H5.
- **Normative changes when implemented:**
  - `spec/time-grant.md` ("Quiet grants", steps 1-2; the Engine contract);
  - `spec/package.md` §12 (device output timers) and the board-file
    directive `*bb digital` (D4);
  - `spec/project.md` §4 (a bound digital net outside the analog scope
    gets no glue, D4);
  - `bbcore::Engine` and `bbdev`'s device API.
- **Evidence:**
  - ADR-008 Amendment 11 Finding 3 and Amendment 12 item 4: every E3 run
    takes one-cell grants, 270 000 for the 2.7 s homing;
  - the retrospective's profile of E3 `typ`:
    - 215.95 s wall (README idle figure 163.9-165.1 s, about 0.61 ms per
      grant);
    - Renode 144.7 s of CPU, `bb` 75.6 s;
    - the engines alternate, so there is no overlap;
  - `bbcore/src/scheduler.rs`: `build_plan` makes an edge for every
    output/input pair on a net (lines 736-741), and the quiet-grant
    structure check requires the remaining graph to be acyclic with the
    producer first (lines 1385-1407);
  - `cli/src/main.rs` (about line 1080): every bound MCU `digital_out`
    becomes an ngspice `InputBinding` with glue, unconditionally;
  - `examples/dial-me-spinner/e3/board.cir`, and `out/dms-home/typ/nets.csv`,
    where MOT_NSLEEP and HOME_N are analog columns (MOT_NSLEEP 3.222656 V
    while driven: 3.3 V through `zout` 800 Ω into RPD_NS 50k ∥ R70 100k);
  - `bbdev/src/dms_mechanism.rs`.

## Summary

Four changes let E3, and a board like it with a device beside the CPU,
take long grants:

1. **Count only inputs that can end a promise.** The structure check of a
   quiet grant counts only the producers that feed an engine in Q through
   an input able to end its promise.
2. **Promises that inputs cannot end.** An engine may declare that a
   change on an input cannot make an output change before its current
   promise. For a device, that means its outputs change only at its own
   output timers. The existing wake audit enforces it.
3. **Q is chosen to maximise the grant**, among the valid sets.
4. **Digital-only nets.** A net declared digital-only gets no analog glue,
   so the CPU's edges on it do not feed ngspice. On E3's homing boards the
   MOT nets become digital-only, which removes their pull-downs from the
   deck. That is a **model change**, recorded and re-hashed, and G0
   measures it separately from the protocol gain.

**Scope.** The gain exists only where, after Q's out-edges are removed,
the remaining graph is acyclic. A CPU that is awake and has edges both
into ngspice (glue, `#smp`, `pi` state outputs) and out of it (`adc_in`,
`digital_in`) is a cycle no ordering resolves without a new promise
(below). Such boards stay at one-cell grants while the CPU is awake.

## Context

**E3's binding graph** (`e3/board.cir`), complete:

```
U10 (Renode) ──MOT_A/B_IN1/2, MOT_NSLEEP (digital)──▶ M1 (device)
M1 ──HOME_N (digital_in, not sampled)──▶ U10
U10 ──MOT_*, MOT_NSLEEP (glue)──▶ ngspice      (RPD_A1..B2, RPD_NS, R70 on those nets, lines 132-137)
M1 ──HOME_N (glue)──▶ ngspice                  (HOME_N is an analog net)
ngspice ──VSYS_SENSE, P3V3 (adc_in, sampled)──▶ U10
```

- `plan_order` puts the lattice-bound U10 first in the component, so in a
  one-cell grant M1 and ngspice consume U10's edges at their exact times.
- Today's structure check requires exactly one producer into Q, counts
  every edge (`producers.len() != 1`), and requires the remaining graph
  to be acyclic with the producer first.
- **With Q = {U10}** (the CPU asleep): M1 and ngspice are both producers,
  so the check fails.
- **With the motor running**, U10's promise ends at its next PWM-related
  wake, at most about 40 µs away. Even a working Q = {U10} would give
  4-cell grants.
- **With Q = {M1}** (its next sample up to 50 ms away): removing M1's
  out-edges leaves U10 → ngspice (the MOT glue) and ngspice → U10
  (`adc_in`), a cycle. **Removing the pull-downs alone does not break
  it**: the glue is created from the binding, whether or not the net has
  another element (`cli/src/main.rs`).
- **The mechanism** (`dms/mechanism@1`):
  - HOME_N changes only in `on_timer(T_SAMPLE)`, every tS = 50 ms (plus
    tACTIVE);
  - `on_pin_change` sets no pin; it schedules only `T_WAKE`, an internal
    timer;
  - its `quiet_until` is its next timer: exact, but conservative, because
    `T_WAKE` counts.

**ngspice and the shared timestep.** Even though no ngspice output that
U10 reads depends on the MOT nets, ngspice's solver points are global. A
MOT edge applied at a different time moves every node's solver points,
and so the µV-rounded samples. A per-pin dependency analysis of the deck
therefore cannot make ngspice's consumption of CPU edges order-free. The
same argument says that **taking the MOT glue out of the deck changes
VSYS, P3V3 and VSYS_SENSE at the µV level**, not only the MOT columns. D4
accepts that and re-records.

**Where Phase 2 adds edges back.** Each of these gives U10 an edge into
ngspice, so with ngspice → U10 the remaining graph is cyclic again:

- the project route (R1) carries R70 and any DRV8837 pin loads on the MOT
  nets, unless its `[analog] nets` leaves them out;
- the loading `saadc_se` view (H2) brings `#smp` from U10 into ngspice
  (glue rule 9);
- `pi` (ADR-009 D4) brings CPU state outputs (`#run`, `#hfxo`) into the
  `supply_load` view.

## Decision

### D1. Which producers need an early stop

In "Quiet grants" step 2, the producer set becomes:

> P = the engines outside Q that drive a net which an engine in Q reads
> through an input that **can end its promise**: not sampled (ADR-005 D3),
> and not declared promise-independent (D2).

- `|P| ≤ 1` is required.
- **With `|P| = 1`:** step 3 is unchanged. `p` runs `advance_until(G,
  watch)` and must be first in the remaining graph's Kahn order. The Kahn
  order prefers `p` among ready engines, then registration index.
- **With `|P| = 0`:** no engine stops early. Every engine runs
  `advance_to(G)` in the remaining graph's Kahn order, and the `grant`
  record is labelled by `G`'s bound.
- **The other producers** (feeding Q only through sampled or
  promise-independent inputs) run in Kahn order like any engine. They must
  not precede `p`. If one must, because `p` reads it, the grant falls back
  to the one-cell grant.
- **The remaining graph** (Q's out-edges removed) must still be acyclic.
  This is unchanged, and it is what limits the scope.

### D2. Promise-independent inputs

- **New `Engine` method** `input_ends_promise(pin) -> bool`, default
  `true`.
- **An engine returns `false` for an input** only when a change on that
  input can never make an output change before the promise its
  `quiet_until` last returned.
- **For `bbdev` devices,** the device API gains output timers:
  - `ctx.schedule_output(at, id)` marks a timer whose handler may call
    `set_pin`;
  - `quiet_until` becomes the next **output** timer;
  - a device may declare `promise_independent_inputs = true` only if
    `on_pin_change` never calls `set_pin` and never schedules an output
    timer earlier than the current promise.
- **`dms/mechanism@1` qualifies:**
  - `T_SAMPLE` becomes an output timer;
  - `T_WAKE` stays internal;
  - the change is a device major bump, `dms/mechanism@2`, because its
    promise changes. Its results must not change (G2).
- **Enforcement.** The existing audit (invariant 8, `PromiseBroken`)
  applies: an engine in Q that changes an output before its promise fails
  the run. A device that breaks its declaration is caught the same way.
  A negative-control device in the gate proves it.
- **Renode** keeps the default (`true`) for GPIO inputs. Its sampled
  inputs are already exempt.

### D3. Choosing Q

- Let the useful promises be `q_c` (`floor_L(q_c) > now`), sorted by
  `floor_L(q_c)` descending, ties by registration index.
- For each distinct value `G_k = floor_L(min(g', q_k))` in that order, try
  `Q_k = {c : floor_L(q_c) ≥ G_k}` against the structure rules (D1). The
  first valid `Q_k` wins.
- If none is valid, the grant is the one-cell grant.
- With today's engine counts (≤ 4) that is at most four checks per
  loop-capped grant.
- **Determinism:** this is a pure function of the promises and the
  binding graph. Today's rule is the special case where the minimum's set
  is valid.

### D4. Digital-only nets, and E3's homing boards (a model change)

- **The rule.** A bound digital net that is outside the analog scope gets
  no glue: its MCU and device pins are routed through the kernel net only
  (ADR-006 Amendment 6's direct routing, which already exists for device
  ↔ MCU), and the analog engine has no input or output on it.
  - In a project, "outside the analog scope" means not in `[analog]
    nets` (`spec/project.md` §4). Elements on such a net leave the deck
    by the existing scope rule.
  - In a board file, a new directive `*bb digital <net>` declares it. A
    board-file element on a digital-only net is an error, so the
    directive cannot silently drop a part.
  - Without a declaration nothing changes: no existing board's deck or
    hash moves.
- **E3's homing boards** (`scripts/dms_home_boards.py`) declare
  MOT_A_IN1, MOT_A_IN2, MOT_B_IN1, MOT_B_IN2 and MOT_NSLEEP digital-only,
  and drop RPD_A1..B2, RPD_NS and R70.
  - This is a **model change**. Those elements load only U10's own
    outputs, and no E3 check reads their analog value, but they are real
    parts of the board.
  - **Expected outputs:** MOT_NSLEEP and the MOT_* columns become digital
    columns (VCD `wire`, not `real`); VSYS, P3V3 and VSYS_SENSE move at
    the µV level, because ngspice's solver points no longer follow the
    MOT edges. `nets.csv`, `nets.vcd` and the non-grant records of every
    homing case are re-recorded, with this reason in the commit, and the
    E3 checks must give the same verdicts.
  - `board-step.cir`, which carries the bridges' power stage, keeps them.
    `make dms-home-test` (in `make test`) runs it, so H5 does not shorten
    `make test`.
- **The graph after the change**, with Q = {M1}:

  ```
  ngspice → U10 → M1          (M1 → U10 and M1 → ngspice are Q's out-edges)
  ```

  M1's inputs are promise-independent (D2), so P = ∅ and every engine
  runs `advance_to(G)` in Kahn order:
  - G = min(max_quantum, M1's next output timer);
  - ngspice runs to G and pushes;
  - U10 runs to G, with HOME_N constant by M1's promise;
  - M1 consumes U10's whole trace exactly, as in the one-cell plan where
    U10 precedes it.
- **The project route (R1).** E3 on the project reaches long grants only
  if its `[analog] nets` leaves the MOT nets out. R1's gate reports
  whether it does; if the project keeps them in scope, the project run
  stays at one-cell grants, by design.

### D5. Expected gain (unmeasured; the reason for G0)

- **Grants:** from 270 000 to about 2.7 s / 10 ms = 270 grants at
  `--max-quantum 10ms`, plus the samples' stops (about 54).
- **Renode's fixed per-`RUN_FOR` cost** (about 0.4-0.5 ms × 270 000, so
  about 110-135 s of the 164 s) mostly disappears.
- **What remains:**
  - Renode executing the PWM ISRs and the firmware;
  - ngspice's quiet deck (no longer driven by MOT edges);
  - M1 consuming about 244 000 edges from U10.
- **A guess:** 10-20×. **G0 measures it before acceptance**, separating:
  - the model change alone (digital-only nets at one-cell grants), which
    also makes ngspice cheaper;
  - the protocol gain (D1-D3) on top of it.
- **One threshold.** If the protocol gain over the model change is under
  **3×** in simulation time, this ADR is withdrawn, the model change is
  kept or reverted on its own merits, and `dms-home` stays in the nightly
  tier. G3 uses the same 3×.

## Exact `spec/` changes (described, not applied)

- **`spec/time-grant.md`, "Quiet grants":**
  - step 1 gains D3's choice of Q;
  - step 2's producer rule becomes D1's;
  - step 3 gains the `|P| = 0` case;
  - the Engine contract gains `input_ends_promise`;
  - Sequence example 4 (device + CPU + analog producer) is added, with
    the scope statement.
- **`spec/package.md` §12:** device output timers and
  `promise_independent_inputs`. The device table gains `dms/mechanism@2`.
  The board-file directive `*bb digital <net>` (D4).
- **`spec/project.md` §4:** a bound digital net outside `[analog] nets`
  gets no glue (D4).
- **`spec/event-stream.md`:** no change. `limited_by` keeps its values.

## Determinism impact

- Q's choice and the plan are pure functions of the promises (themselves
  pure reads) and the bindings.
- With D1-D3 alone, only `grant` records change.
- D4 changes the deck of the boards that declare digital-only nets, and
  so their results. That is a model change, re-recorded once with its
  reason, not a partition effect.
- The equivalence argument is ADR-003 D5's:
  - each engine's inputs over (now, G] are either constant by a promise,
    sampled (read at instants the promise covers), or delivered as
    complete traces before it advances;
  - the lattice makes every engine's results independent of where grants
    end.

## Gates

| # | Gate |
| --- | --- |
| G0 | **Prototype, before acceptance** (one week): E3 `typ` three ways, the committed run, D4 alone at one-cell grants, and D1-D4 on a branch: grants, wall and simulation time (ADR-003 protocol, N = 3, idle host). D1-D4 equal to D4 alone except grant records. The protocol gain (D1-D4 against D4 alone) ≥ 3×, else withdrawn |
| G1 | `make multi-producer` (new, in `make test`): a synthetic board with a CPU, a device with promise-independent inputs, and an ngspice sampled input. Long grants equal to the one-cell reference except grant records. Negative control: a device that sets a pin in `on_pin_change` while declaring independence fails with `PromiseBroken`. A second negative control: the same board with one CPU → ngspice edge added falls back to one-cell grants (the scope rule) |
| G2 | `dms/mechanism@2` gives `dms/mechanism@1`'s committed results on every `dms-home` case at one-cell grants |
| G3 | `make dms-home` (nightly tier): every case equal to its one-cell reference except grant records (a true equivalence now: the partitions differ); E3 `typ` ≤ 5 000 grants and ≥ 3× faster in simulation than D4 alone (ADR-003 protocol); no promise broken; analog-first byte-identical. The full gate's wall time recorded |
| G4 | Every existing quiet-grant gate unchanged: `make lookahead-equiv`, `make adc-equiv`, `make feedback`, `make nrf-feedback`, `make dms-vsense-test`; the same grant counts (the rule is a superset; any change is explained) |
| G5 | `*bb digital`: a board-file element on a digital-only net is refused; every board without the directive builds a byte-identical deck |

## Rejected alternatives

- **Pin-level output dependencies for the analog engine** (an
  FMI-ModelStructure-like declaration, which ngspice's deck connectivity
  could provide). ngspice's solver points are global, so the arrival time
  of an edge on an unrelated node still changes every node's samples.
- **Running the CPU first and the device "afterwards" without D2.** The
  CPU would be `p` and would stop at every PWM edge the device watches,
  which is no better than one cell.
- **Coalescing the PWM edges the device consumes.** Forbidden (ADR-003 D3:
  consumers apply every transition).
- **An averaged DRV8837 path in place of the device's inputs.** It changes
  the model, not the protocol, and it is still a one-cell loop.
- **Dropping glue automatically on every net with no other analog
  element.** It would change Phase 1 decks silently. D4 requires a
  declaration.
- **Not counting a sampled-only back edge (ngspice → U10 through
  `adc_in`) for acyclicity.** No ordering makes U10 → ngspice → U10 safe
  without a new promise: ngspice must run after U10 to consume its edges,
  and U10 must have the ADC value before its next sample. That needs a
  **sample horizon**, a CPU promise of when it next samples. Breadboard's
  SAADC starts a sample when firmware writes a task, which a running CPU
  can do at any instruction, so the horizon is not knowable today. It is
  recorded as a Phase 3 candidate.

## Questions for the approver

Consolidated in the plan's section 9 as **Q4** (pull this into Phase 2
behind G0, with the 3× threshold and D4's model change).

**Defaults (apply unless the approver objects):**

1. **The device bump:** `dms/mechanism@2` for the output-timer split,
   instead of changing `@1` in place (ADR-006 D8: a behaviour change bumps
   the major, and the promise is behaviour).
2. **D4 kept even if the ADR is withdrawn**, only if its own one-cell
   speed-up is worth a recorded model change; otherwise reverted.

## Approver decisions (2026-11-15)

All twelve questions of the plan's section 9 were accepted with their
recommended answers (plan section 10). For this ADR:

- **Q4, accepted.** This ADR is pulled into Phase 2 **behind G0**, with
  one threshold: the protocol gain over the model change alone is at
  least 3× in E3 `typ`'s simulation time. D4's model change is accepted:
  E3's homing boards' MOT nets become digital-only, a recorded model
  change that re-records their hashes. The scope excludes awake CPUs with
  edges both ways.
- **Q2, accepted.** E3's runtime is not an exit item. The exit item is
  "ADR-014 accepted with G3 met, or withdrawn by G0 with `dms-home` in the
  nightly tier". The outcome rule is fixed: if G0 misses 3×, this ADR is
  withdrawn (status updated with the measurement) and D4 follows default 2.
- **Build scope.** H5 is in this build, G0 first.

**Defaults.** No objection was made to the defaults listed under
"Questions for the approver", so they apply as written.

## Review notes (design review round 1, 2026-11-14)

**Adopted:**

- **D4's premise was wrong** (both critiques): removing the pull-downs
  does not remove the glue, so the graph stayed cyclic. D4 now adds a
  declared digital-only rule, states the model change, and expects the
  hashes to change (the first draft's "unchanged" contradicted both the
  committed analog MOT_NSLEEP column and this ADR's own timestep
  argument). The graph in Context now includes M1 → ngspice.
- **G0 separates the model change from the protocol gain**, runs before
  H5 is costed, and uses the same 3× threshold as G3.
- **Scope**: the Summary and Context say which Phase 2 configurations
  bring a CPU → analog edge back (R1, H2, `pi`), and G1 has a negative
  control for it. The plan removes E3's runtime from the exit.
- **D5's remaining cost** names M1, not ngspice, as the consumer of the
  MOT edges.

**Not adopted:** extending D1 so a sampled-only back edge does not count
for acyclicity (one critique's alternative). See "Rejected alternatives":
it needs a sample horizon that a running CPU cannot give.
