# ADR-014: Quiet grants with several producers, and promises that inputs cannot end

- **Status:** Accepted (2026-11-15) **behind gate G0** (see "Approver
  decisions (2026-11-15)"): if G0 misses its threshold, this ADR is
  withdrawn. Proposed 2026-11-14, revised after design review round 1
  (see "Review notes"). Kernel protocol: written before implementation
  (AGENTS.md). **A prototype must measure the gain** (gate G0) before
  D1-D3 are implemented beyond it, and before the rest of H5 is costed.
  **G0 passed** (2026-11-16: the protocol gain 32.3x against the 3x
  threshold), so the ADR stands; **implemented** in Phase 2 H5 with its
  normative changes applied (Amendment 1, which also records the
  refinements: the order of a grant without a producer, and the Renode
  engine's self-audit).
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

## Amendment 1: H5 as built (G0 passed, D1-D4, 2026-11-16)

Phase 2 milestone H5. G0 ran first and passed by an order of magnitude, so
this ADR stands (it is not withdrawn) and D1-D4 were built as decided, with
the refinements below. The normative changes are applied: `spec/time-grant.md`
("Quiet grants" steps 1-3, the promise-independent input, sequence
example 4), `spec/package.md` section 12 (output timers,
`promise_independent_inputs`, `*bb digital`) and section 9's device table
(`dms/mechanism@2`). `spec/project.md` section 4 already had D4's project rule
(Phase 2 R1, ADR-013 D1); it is unchanged.

### G0: the measurement

`make dms-home-g0` (`scripts/dms_home_g0.sh`, under the host measurement
lock; ADR-003 protocol: interleaved rounds, N = 3, medians of the simulation
time, every Renode fresh): E3 `typ` (`homing.elf`, 2 700 ms) three ways:

| Case | Grants | Simulation, median (the three runs) | Load (1 min) |
| --- | --- | --- | --- |
| base: the committed run (`bb` at `29e5006` on its own `board.cir`: MOT nets glued, `dms/mechanism@1`) | 270 000 | 248.43 s (330.58, 248.43, 233.41) | 8.5, 12.8, 4.9 |
| d4: the model change alone (D4's `board.cir` at one-cell grants, `--lookahead off`) | 270 000 | 222.89 s (334.62, 222.89, 215.67) | 5.4, 4.5, 3.8 |
| proto: D1-D4 (the same board at `bb`'s defaults) | 433 | 6.91 s (7.31, 6.91, 6.50) | 14.3, 4.9, 7.2 |

- **The protocol gain** (d4 / proto) is **32.3x** in simulation time, against
  the threshold of 3x: G0 passes (`dms-home-g0: PASS`). Against the
  committed run (base / proto) it is 36.0x.
- **The model change alone gains little** (base / d4 1.11x): ngspice's
  quiet deck was never the cost. About 0.8-0.9 ms per grant of Renode's
  fixed `RUN_FOR` cost is (D5's estimate was 0.4-0.5 ms), and only the long
  grants remove it.
- The host was not idle (load 3.8-14 from other tracks' gates). The ratio is
  far from the threshold, so the load does not change the verdict.
- proto equals d4 except grant records (`nets.csv`, `nets.vcd`, the
  non-grant records) in all three rounds, every case is deterministic
  across its three runs, and every base run equals the committed `typ`
  hashes (the script's own checks: `PASS: proto equals d4 except grant
  records, every case deterministic, base equals the committed typ
  hashes`).
- **An earlier run of the same measurement** (before the plan-order
  tie-break below, which does not touch E3's order; load 2-44) gave 32.4x
  (d4 369.32 s, proto 11.41 s, base 303.55 s; base / d4 0.82x). Its base
  check failed only because the script probed MOT_NSLEEP last instead of
  third, the committed order, so `nets.csv` and `nets.vcd` differed by
  column order while `events.jsonl` and the non-grant records matched; the
  script was fixed and the measurement repeated (the table above).

D5 guessed 10-20x; the measured gain is larger because the quiet grants are
`max_quantum` long while the PWM runs: 433 grants, of which 271 have
`Q = {devices}` (216 `max_quantum`, 54 `engine:devices`, 1 `engine:ngspice`),
160 are one-cell grants around M1's 54 samples (about three each: the
back-off skips two Queries after each),
and 2 are U10's own short promises (`Q = {U10}`, M1 stopping early).

### What was built

- **D1** (`bbcore/src/scheduler.rs`). The kernel keeps, beside the binding
  graph, the *ending* edges: `(j, i)` where `i` reads a net `j` drives through
  an input that can end its promise (`!input_is_sampled && input_ends_promise`).
  `quiet_order` counts producers over those edges only (`|P| <= 1`). With
  `P = {p}` the Kahn order prefers `p` among the ready engines, then
  registration index, and `p` must be first, as ADR-003 D5; with `P` empty
  the grant record is emitted at once with `G`'s bound and every engine runs
  `advance_to(G)`. The watch list of `p` uses the same ending inputs. One
  consequence on existing boards: on the ADC boards ngspice feeds the CPU
  only through sampled inputs, so their quiet grants now have `P` empty
  where ADR-003 made ngspice `p` with an empty watch; the grant records and
  outputs are the same (G4).
- **The order without a producer** (a refinement found by `make
  device-smoke`, below). With `P` empty, ties in the Kahn order are broken by
  the **static plan order**, not the registration index. An engine of `Q`
  may change an output exactly at `G` (its promise covers `(now, q)`, and
  `G = floor_L(q) = q` when `q` is a lattice point). Its consumers outside
  `Q` must then receive that change as the one-cell plan delivers it: before
  they advance when the plan puts them after it, at the boundary otherwise.
  A CPU never changes an output at a lattice point (ADR-003 D2's +1 fs rule),
  so ADR-003 never met this; a device does. On `tests/device-smoke`
  (`M1` samples at 7 ms + k 50 ms + 40 us, all lattice points; ngspice a pure
  consumer, so the plan is [U1, devices, ngspice]) ties by registration
  index advanced ngspice before M1, so M1's HOME_N rise at 57.04 ms reached
  ngspice at the boundary instead of before its advance. The analog engine
  treats the two deliveries differently (a ramp corner ahead of the solver,
  or a source change at its current time), and SENSE differed from the
  one-cell reference by 1-2 uV in 161 samples from 100.1 to 151.8 ms
  (identical in every partition in which M1's change was delivered the
  one-cell way: `--max-quantum` 30 us to 3 ms; different at 20 us, 5 ms and
  10 ms). With plan-order ties every partition equals the reference. The
  gate is `bbdev/tests/multi_producer.rs`
  `a_device_change_at_the_grant_end_reaches_a_later_consumer_before_it_advances`,
  which fails with registration-index ties (checked). With `P = {p}` the
  ADR-003 rule (registration index after `p`) is kept: no committed board
  has a device in `Q` beside a producer, and changing it would reorder
  ADR-003's grants. A future board that has one needs the same argument.
- **D2** (`bbcore::Engine::input_ends_promise`, default `true`; `bbdev`).
  `Ctx::schedule_output(at, id)` marks an output timer;
  `Device::promise_independent_inputs()` (default `false`) declares D2's
  property. For a declaring device the engine refuses `set_pin` in any
  other timer's handler (rule 1 already refuses it in `on_pin_change`), its
  `quiet_until` is its next output timer, and `input_ends_promise` is
  `false` for its inputs. A device that does not declare it keeps Phase 1's
  behaviour exactly.
- **`dms/mechanism@2`** (default 1 of "Questions"): `T_SAMPLE` an output
  timer, `T_WAKE` internal, inputs declared promise-independent. The package
  `dms/detent-mechanism` is 0.2.0 and names it; `@1` is no longer in the
  device table (nothing committed named it after the relock). Every lock
  that pins the package or the device was rewritten (`make locks`: e3,
  e3/home, e3/step, tests/device-smoke).
- **D3** (`query_quiet`). The useful promises are sorted by `floor_L(q)`
  descending (ties by registration index); for each distinct
  `G_k = floor_L(min(g', q_k))` the set `Q_k = {c : floor_L(q_c) >= G_k}` is
  tried; the first that passes D1 wins.
- **The Renode engine's own audit** (`engines/cpu-renode`). It audits its
  promise on the advance that starts where the promise was given. Under D3
  a shorter promise may be left out of `Q`, and the engine's inputs are then
  held by no one, so the engine now audits only when that advance stays
  within the promise (`t <= until`); a promise the kernel used always does
  (`G <= floor_L(q)`). Before D3 an advance past a useful promise never
  happened, so nothing else changes (G4).
- **D4** (`cli/src/board.rs`, `cli/src/main.rs`). `*bb digital <net>`: the
  net must be driven by a bound `digital_out`; every `digital_in` on it must
  be routed directly (a device reads it or drives it), an `adc_in` on it is
  refused, a SPICE element on it (outside `.subckt` definitions, a B
  expression's `V(<net>)` included) is refused, and `--probe net:<net>` is
  refused with the driver's pin named, as on the project route. The glue
  code already skipped `board.digital_only` (R1); a board file now fills
  it. E3's homing boards (`board.cir`, `board-ts75.cir`, `board-park.cir`,
  `scripts/dms_home_boards.py`) declare the five MOT nets and drop RPD_A1..B2,
  RPD_NS and R70; `board-step.cir` keeps them. The generator also writes
  `board-glued.cir`, the typical board as it was before H5 (its deck equals
  `main`'s `board.cir` line for line), kept as `make ngspice-stall`'s deck
  (H1's 77.43 ms repro and H2's 502.47 ms control are properties of that
  deck) and as the base of `schema-epoch`'s comparison of the homing boards.
- **Diagnostics.** `bb run` prints the quiet sets it used with their counts
  and early stops (`bb run: lookahead: quiet sets: {devices} 271 (0 with an
  early stop), ...`); stderr only, never an output file.

### D4's model change, measured

Re-recorded: `tests/golden/dms-home/{typ,worst,park}` (`nets.csv`,
`nets.vcd`). Reason: ADR-014 D4 (the five driver nets digital-only, their
pull-downs and R70 out of the deck). The measured difference against the
pre-H5 one-cell references (`dms/mechanism@2`'s, which equal `@1`'s, G2):

- every non-grant record is identical (244 252, 252 253 and 277 214
  records);
- every shared `nets.csv` column is identical, VSYS and HOME_N included: no
  sample moved by even 1 uV, against D4's expectation of uV-level moves;
- the MOT_NSLEEP column is gone (it is digital-only now; `U10.P1.15` is its
  level), which is the whole change of both files.

`step` (the bridges keep the MOT nets analog), `project-step` and
`project-typ` (already digital-only on the project route) keep their hashes.

### The gates

- **G0**: above.
- **G1**, `make multi-producer` (new, in `make test`; `scripts/multi_producer.sh`,
  `tests/multi-producer/board.cir`, `bbdev/tests/multi_producer.rs`): an
  nRF52840 running `homing.elf`, `dms/mechanism@2` at tS 5 ms on five
  digital-only nets, ngspice behind two sampled `adc_in` inputs, 100 ms,
  fresh Renode. bb's defaults (81 grants, 21 of them `Q = {devices}` with
  no early stop, 4 of U1's own) equal the one-cell reference (10 000) except
  grant records; analog-first is byte-identical; no promise broken. The
  scope control (MOT_NSLEEP's directive removed, so U1 -> ngspice -> U1)
  never takes `Q = {devices}` (9 864 grants: 1 807 short `Q = {U1}` grants
  while U1 sleeps) and its non-grant records equal the reference's. The
  kernel part runs the same graph with mock engines (`Q = {devices}`, no
  stop, order ngspice -> U1 -> devices, 1 ms and 10 ms and the other
  registration order equal to one cell), the negative controls below, the
  scope rule, and the plan-order tie-break (5 tests). 19.6 s in a pooled
  `make check` (the budget is 30 s).
- **G2**: every `dms-home` case (`step`, `typ`, `worst`, `park`,
  `project-step`, `project-typ`) at one-cell grants with `dms/mechanism@2`
  on the pre-D4 boards equals the committed `@1` hashes (`nets.csv`,
  `events.jsonl`, `nets.vcd`, the non-grant records): six PASS lines.
- **G3**, `make dms-home` (`scripts/dms_home.sh gate`; 11 min 34 s beside
  other gates, against 37-62 min before H5): every case's defaults equal
  its one-cell reference except grant records, a true equivalence now (`typ`
  433 grants against 270 000, `worst` 410 against 280 000, `park` 510
  against 320 000; `step` 3 694 against 4 500, U10's own promises); the
  references equal the re-recorded goldens; `check_dms_home.py` passes on
  each; no promise broken; `typ` at most 5 000 grants (433) and 21.4x less
  simulation than its one-cell reference in the same gate (26.23 s against
  562.17 s, beside the gate's other jobs; the protocol figure is G0's
  32.3x); analog-first byte-identical (`typ`, `step`); `--max-quantum 1ms`
  equal except grants (`typ` 2 859 grants, `step` 3 701); `typ`'s peak RSS
  28 596 kB (ADR-015 G2's bound 403 627 kB; the one-cell reference
  26 944 kB: with D4 ngspice no longer steps through the PWM edges); 10 runs of `typ` byte-identical; the three
  negative controls fail exactly their checks.
- **G4**: each existing quiet-grant board run with `main`'s `bb` and with
  H5's, fresh Renode: `examples/feedback` (10 ms and 1 ms), its polling
  firmware (10 ms and 1 ms), `examples/nrf-feedback` (10 ms and 1 ms),
  `examples/adc` (10 ms and 1 ms), `board-open.cir`, `examples/nrf-saadc`,
  E2 `s50` and E1 `ok-cold`: every grant record and every output identical
  (117, 562, 50 000, 50 000, 120, 565, 1 209, 1 209, 30, 209, 732 and 71
  grants). Two boards change by design: `tests/device-smoke` (37 -> 21
  grants: `Q = {devices}` and `{U1, devices}`; its outputs identical to
  `main`'s, and its four runs equal to each other byte for byte, `make
  device-smoke`) and E3 `step` (4 500 -> 3 694 grants: U10's own promise
  while it sleeps now forms `Q = {U10}` with M1 stopping early, which
  ADR-003's one-producer rule refused because ngspice also fed U10; its
  outputs are identical). The comparison was run twice: before the
  plan-order tie-break `device-smoke`'s outputs differed (the finding
  above), after it they are identical; every other line is the same in
  both runs. The gates themselves (`make feedback`, `make nrf-feedback`,
  `make adc`, `make dms-vsense-test` and the rest of `make test`) pass in
  `make check`, pooled and with `POOL=0`, and `make lookahead-equiv` and
  `make adc-equiv` pass; `make docs-plots-check` passes (no figure changed:
  the E3 plot reads `M1.HOME_N`, `U10.P1.15` and the records, which D4 left
  identical). `make ngspice-stall` runs on `board-glued.cir` and passes (under the
  lock): H1's stall at 77.43 ms in the cell (77.43, 77.44] with the snap
  off, H2's completion runs and the 502.47 ms control unchanged. One of its
  checks had assumed one-cell grants: the outputs written before the error
  equal a run that ends at the last completed grant, which was 77.43 ms.
  On the glued board U10's own promise now gives quiet grants after the
  step (`Q = {U10}`, M1 stopping early: ADR-014 D1), so the stall falls in
  the grant (67.94, 77.94] and the last completed grant ends at 67.94 ms.
  The script now reads that boundary from the stalled run's last grant
  record and compares with a run to it (byte for byte, as before).
- **G5**: `board::tests::digital_only_directive` (the directive, its
  refusals: an element on the net, through a continuation line and in a B
  expression; no driver; a duplicate; a key; a CPU `digital_in` through
  ngspice's threshold; an `adc_in`); and every board file without the
  directive builds the same deck with H5's `bb` as with `main`'s: spike,
  feedback, feedback-open, adc, adc-open, nrf-saadc, nrf-pwm, nrf-feedback,
  device-smoke and E3 step (`--elaborate-only`, comment lines aside), and
  `main`'s `board.cir` against `board-glued.cir`.

### The project route (R1), measured

`make dms-home-project` (`scripts/dms_home.sh project`, 9 min 17 s beside
other gates). `e3/home/bench.toml` leaves the five MOT nets out of `[analog] nets`
(R1), so they were already digital-only there, and `project-typ` takes the
same long grants as the board file: 433 grants at bb's defaults against
270 000 one-cell grants, 12.99 s of simulation against 514.67 s (39.6x,
on the pool, beside the gate's other jobs), the same quiet sets (`{devices}`
271, `{U10}` 2), equal to its one-cell reference except grant records, which
equals its committed hashes (unchanged by H5), and equal to the board
file's `typ` after the name mapping (`compare_project_run.py`). `project-step`
keeps the bridges in scope, so, like `step`, it takes only U10's own short
promises (3 694 grants against 4 500).

### Deviations

- **G1's first negative control.** "A device that sets a pin in
  `on_pin_change` while declaring independence" cannot reach the audit:
  rule 1 refuses `set_pin` in `on_pin_change` for every device, in any run.
  The gate keeps that refusal and adds the control that reaches the audit:
  a declaring device whose `on_pin_change` schedules an output timer 1 us
  after nSLEEP rises fails the lookahead run with `PromiseBroken`
  (`at` = the reaction, `promised` = its next sample) and passes the
  one-cell run.
- **G1's second negative control** says the scope board "falls back to
  one-cell grants". With D1, a sleeping CPU's own promise is usable on it
  (`Q = {U1}`, M1 stopping early: ngspice's sampled edge no longer counts as
  a producer), so it is not all one cell (9 864 of 10 000 grants). What the
  scope rule forbids, M1's promise lifting the cap while U1 is awake, the
  gate checks: no quiet grant with `Q = {devices}`.
- **D3 left the Renode engine's self-audit unspecified**; it is now limited
  to promises the kernel used (above).
- **The order of a grant without a producer** breaks ties by the static
  plan order (above). D1 named no tie-break for `|P| = 0`; ADR-003's
  registration index stays for `|P| = 1`.
- **Other gates that ran the E3 boards.** `make ngspice-stall` (H1, H2)
  keeps its pre-H5 deck as `board-glued.cir`, and its "outputs before the
  error" check now compares with a run to the last completed grant read
  from the stalled run (above). `make assert-gate` (B2) no longer probes
  MOT_NSLEEP on the homing boards (it is digital-only), and
  `schema-epoch` (S) compares the pre-epoch homing boards' decks with
  `board-glued.cir`'s.
- **D4's expected uV moves did not happen.** No probed analog sample
  (VSYS, HOME_N) moved, and the firmware's VSYS reading (its UART record,
  a non-grant record) is the same; the re-recording is the MOT_NSLEEP
  column alone.

### Merged into main (2026-11-16)

H5 was merged into main after P2 (ADR-009 Amendment 5) and R2 (ADR-013
Amendment 3). One interaction needed a change, none a re-recording:

- **`pi` glue on a digital-only net.** P2's `bb run --operating-point
  --fidelity pi` gives every target `digital_out` of a board file a
  stand-in rail. On a `*bb digital` net that would put `pi` glue on a net
  D4 gives no glue at all, so the board-file route now skips digital-only
  nets, as the project route (`elab.rs`) already did. The three E3 homing
  board files therefore have no `pi` output at G9 (ADR-009 Amendment 5's
  table; `scripts/pi_glue.sh` states 0, the count of the `e3/home`
  project), and their operating point solves as before; `board-step.cir`
  keeps its five.
- `tests/multi-producer/bb.lock` was relocked to `nordic/nrf52840-qiaa`
  0.2.0 (P2) with `make locks`; no other lock or `board.bbm` changed.
- The pool-equiv rows of both sides are kept (E3's homing cases compared
  on non-grant records, R2's `dms-e1-system` cases added).

On the merged tree: `make check` (pooled) green, `multi-producer` 25.3 s
and `device-smoke` 27.3 s in it; `make dms-home` PASS (typ 433, worst
410, park 510 grants against 270 000, 280 000 and 320 000 one-cell, every
reference equal to its committed hashes, typ 28.8x less simulation than
D4 alone beside the gate's jobs, 29 min 18 s) and `make dms-home-project`
PASS (`project-typ` 433 grants, 7 min 32 s).
