# ADR-010: Bench v1, assertions, `bb test` and JUnit

- **Status:** Accepted (2026-11-15; see "Approver decisions
  (2026-11-15)"). Proposed 2026-11-14, revised after design review round 1
  (see "Review notes"). Amendments: 1, B1 as implemented; 2, the schema
  epoch (milestone S); 3, B2 as implemented; 4, the P1 bench keys
  (`fidelity`, `corner`, `[corners]`, `[[battery]]`; numbered 3 on the
  P1 branch, renumbered at the merge after B2's Amendment 3); 5, B3 as
  implemented (`bb test`, `bb-test/0`, `test.json`, JUnit).
- **Part of:** the Phase 2 plan, milestones B1-B4. P1-P5 and E5 use it.
  Its schema keys (`bb-bench/1`, the `switch` view kind) land in milestone
  S, the schema epoch.
- **Normative changes when implemented:**
  - `spec/project.md` (`bb-bench/1`);
  - a new `spec/test.md` (`bb-test/0`, `test.json`, the JUnit subset);
  - `spec/event-stream.md` (version 2, joint with ADR-009);
  - `spec/schema/bench.schema.json` (updated for `/1`), and the new
    `test.schema.json` and `junit-subset.xsd`;
  - `spec/time-grant.md` does not change. The bench engine is an ordinary
    engine.
- **Evidence:**
  - the `pi`/bench research pass: PWL scaling, partition independence of
    solver-point extrema, and the mapping of Phase 1's check scripts onto
    assertion kinds;
  - `scripts/check_dms_rail.py`, `check_dms_vsense.py` and the E3 checks;
  - ADR-007 D5 ("Phase 2 defines the full bench in its own ADR; that ADR
    decides YAML or TOML").

## Summary

- Benches stay TOML (`bb-bench/1`). They gain timed actions, CSV playback
  through an `EXTERNAL` source, current profiles, a battery element,
  environment signals, faults, radio activity and per-probe rates.
- A built-in **bench engine** drives them.
- Checks become **assertions** of a closed set of kinds. The engines
  evaluate them deterministically and emit them as `assert` v2 records.
- `bb test` runs `bb-test/0` files of cases on the warm pool. It writes a
  canonical `test.json` and a `junit.xml`.

## Context

- Phase 1 checks are Python scripts over the outputs. There are about ten
  of them, and each re-implements windows, edge pairing and interpolation.
- Three of Phase 1's weak checks were found only by review:
  - FW-6.4 was missing;
  - the E3 step rate was unchecked;
  - the DRV5032 variant was unchecked.
- `bb-bench/0` has supplies (with `rs` and `track`), DC loads, `temp_c`,
  `[[pwl]]`, `[analog]` and `[options]`.
- **Measured:** inline PWL is O(n²). 1k, 10k and 100k points took 0.14 s,
  2.3 s and 103 s.
- **Measured:** solver-point minima and maxima are bit-identical at 10 µs,
  1 ms and 10 ms grants. Assertions evaluated at accepted solver points
  are therefore partition-independent.
- The vision doc names `bench.yaml`. The house rule is TOML authored,
  canonical JSON derived (ADR-006). YAML's implicit typing (`no`, `1e3`,
  `08`) is a hazard for values.

## Decision

### D1. Format: `bb-bench/1`, TOML

`bb-bench/1` is a new major. `bb` reads `/0` and `/1`, writes `/1`, and
`bb pkg migrate --bench` rewrites the `schema` line, since every `/0` key
keeps its meaning. A migrated bench builds a byte-identical deck (gate
G1).

### D2. New sections

```toml
schema = "bb-bench/1"
fidelity = "pi"                          # ADR-009 D1; the command line wins
corner = "typ"                           # ADR-009 D8
seed = 1                                 # only for declared random elements (bounce)

[[battery]]                              # B and C elements, no branch-current unknown
net = "VBAT"
ocv = "tables/cell_ocv.csv"              # soc,volts
soc = "0.05"
r0 = "0.25"
rc = [["0.05", "2F"]]
temp_c = "-10"

[[action]]                               # timed actions on a switch-like instance or a digital net
press = "SW1"                            # an instance with a `switch` view (closed while pressed)
at = "2s"
for = "50ms"
bounce = { count = 3, max = "1ms" }      # seeded, recorded in run.json
[[action]]
set = "USB_VBUS"                         # drive a bench source to a new value
to = "5V"
after = { edge = "U10.P1.04", level = 1, delay = "1ms" }   # relative trigger

[[playback]]                             # recorded or synthetic data
net = "NTC_IN"
file = "data/temp.csv"                   # t,volts  (canonical CSV, D3)
rs = "100"
interp = "linear"                        # linear | hold
[[playback]]
net = "U10.P0.11"                        # a digital stream into an MCU pin
file = "data/btn.csv"                    # t,level

[[load]]
net = "VSYS"
profile = "data/wifi_burst.csv"          # t,amps; a current profile
[[load]]
net = "+3V3"
amps = "50mA"
replace = "main:U10"                     # static-load mode: this load stands in for U10's supply_load (ADR-009 D4)

[[activity]]                             # bench-scheduled internal states (radio, ADR-009 D4)
state = "main:U10#radio_tx"
every = "1ms"
for = "300us"

[[env]]                                  # environment signals for devices and the TEMP model
signal = "temperature"
file = "data/temp_c.csv"

[[fault]]
kind = "open"                            # open | short (a B conductance switched by an EXTERNAL source)
net = "VLED"
pin = "main:C8.1"
at = "50ms"

[probes]
rates = { VLED = "1MHz" }                # analog nets only; refused if finer than TMAX = L
```

- **The `switch` view kind** (new in `bb-part/1`, `spec/package.md` §3),
  for `press` actions:
  - ports: the two contact pins, typed `pin` ports as `pin_load` has;
  - parameters: `ron` and `roff` in the value grammar (defaults 50 mΩ
    and 100 MΩ, so both states keep a DC conductance, ADR-009 D2 rule 2),
    and an optional `bounce_max`;
  - the element: one B conductance between the ports,
    `I = (V(a) − V(b))·(g_off + V(<ref>_ctl)·(g_on − g_off))`, with
    `<ref>_ctl` an `EXTERNAL` 0/1 source that the bench engine drives;
  - a `switch` instance that no action presses is open (`roff`).

  A `press` on an instance without a `switch` view is refused with the
  instance's view kinds named.
- **Supplies, loads, PWL and the scope** keep their `/0` form.
- **`[[pwl]]`** stays, for short tables only. `bb run` warns above 1 000
  points and refuses above 10 000; playback is the route for those.
- **A trigger** (`after`) makes the bench engine a reactive consumer of
  the named edge or `log` match. The action takes effect at
  `ceil_L(t_edge + delay)` when it drives an MCU input, and at
  `t_edge + delay` exactly when it drives the analog side.

### D3. Playback and captures

- **The runtime format is one canonical CSV:**
  - a header `t,<name>`;
  - `t` in seconds as a decimal string, converted to fs exactly (decimal
    arithmetic, round half up to whole fs);
  - values in the value grammar or as plain numbers.

  Reading it is deterministic and does not depend on float formatting.
- **Analog playback** is an `EXTERNAL` source. The engine interpolates in
  Rust with a cursor (O(1) per solver point), never with a PWL card.
- **Digital playback into an MCU** is a trace of the bench engine. It is
  applied at `ceil_L` like any edge.
- **`bb capture import <file> --out <csv>`** converts a capture into that
  CSV once:
  - sigrok `.sr` (a zip of `metadata`, `logic-1-N`, `analog-1-c-N`);
  - Saleae Logic 2 binary, version 0. Digital is the initial state plus
    f64 transition times; analog is f32 samples with a sample rate.

  Doubles convert to fs by one documented rounding. The importer writes
  the source file's sha256 into a comment line. A bench reads only CSV, so
  capture formats never enter the run.

### D4. The bench engine

- A built-in engine (`bench`), registered after the analog engine, in the
  style of `bbdev`'s `DeviceEngine`.
- It knows its whole stimulus in advance:
  - `t_safe` is its next scheduled change;
  - `quiet_until` is exact;
  - it is a lookahead candidate.
- With triggers it reads the nets it watches, as a consumer.
- Its outputs:
  - digital traces (actions, digital playback, `[[activity]]` states);
  - analog `EXTERNAL` values (playback, profiles, `set` actions), served
    by the analog engine's `EXTERNAL` callback from the bench's tables.

  So no analog value crosses the boundary per solver point. The bench
  hands the analog engine the table once, with its cursor rules.
- Its identity is a `bbdev`-style content hash, locked as `engine:bench@1`.

### D5. Assertions: a closed set of kinds

Kinds, in `[[assert]]` in a bench or a test case:

| Kind | Meaning | Example (Phase 1) |
| --- | --- | --- |
| `range` | a net, or the difference of two nets, within `min`/`max` over a window | LSW-3: `V(LED_DIN) − V(VLED) ≤ 0.7 V` |
| `at` | a net's value at each edge of another net (or at a time) within limits | LED-DATA-V4: VLED at P0.06's first rise ≥ 2.67 V |
| `settle` | inside a band within T after an event, and staying there to the window's end | LSW-2: VLED < 10 % within 30 ms of LED_PWR_EN falling |
| `edge_timing` | min/max gap between paired edges | FW-6.4: P0.06 rise ≥ 10 ms after LED_PWR_EN rise |
| `count`, `frequency` | edges in a window; a period within a tolerance | "blink at 1.00 Hz ± 2 %"; E3's step rate |
| `log` | a `log` record matching a literal or an anchored regex, present or absent | E2's UART code lines |
| `event` | a count of `reset` records, `PromiseBroken`, `warn` records | W2: ≥ 2 `reset` |

- **Windows:** `from`/`to` (times), `after = {edge, delay}`, `while = {net,
  level}`, and `until` the run's end.
- **Run end:** a `settle` whose deadline falls after the run's end is
  `not_evaluable`, never `pass`.
- **No expression language.** A check that does not fit a kind stays a
  script until a kind is added by an ADR amendment.

### D6. Evaluation, deterministic

- **Analog monitors run inside the producing analog engine** at every
  accepted solver point, with the `digital_in` crossing machinery
  (interpolated, rounded up to fs).
  - They are passive: no glue, no input, no grant boundary.
  - Extrema, crossings and band entries are therefore pure functions of
    the solver points, which the lattice makes partition-independent.
  - Values at an instant come from the engine's `sample_output(net, t)`.
- **Digital checks** read the `pin_edge` stream.
- **The verdict** is a kernel `assert` record at the verdict time. It
  sorts after the `net_sample` records at equal `t_fs` (the kernel's
  records come last). It is part of the non-grant equivalence comparison
  automatically.
- **Limitation, stated in the spec:** a glitch narrower than one solver
  step (at most TMAX = L) is missed, exactly as for `digital_in`.
- **Honesty rules:**
  - an assertion on a net forced by an ideal bench source is refused (a
    tautology);
  - a current is computable only from node voltages (Norton sources,
    B-source views). An assertion on a current through a 0 V source is
    refused;
  - each record carries `rung` (ADR-009 D1). A `requires` rung that is not
    met gives `not_evaluable`.

### D7. `assert` record v2 (event stream version 2)

Keys in order: `t_fs`, `name`, `outcome` (`pass`, `fail` or
`not_evaluable`), `pass` (present unless `not_evaluable`, for version 1
consumers), `value`, `limit`, `unit`, `rung`, `msg`.

- `value` and `limit` are **integers** in base units: µV, fs, nA, µ°C, or
  counts. `unit` names the base unit, so the stream stays free of floats.

```json
{"type":"assert","t_fs":10500000000000000,"name":"LED-DATA-V4","outcome":"pass","pass":true,"value":2971511,"limit":2670000,"unit":"uV","rung":"fast","msg":"VLED at P0.06 rise"}
```

### D8. `bb test` and `bb-test/0`

```toml
schema = "bb-test/0"
project = "."                         # a bb.toml project, or board = "<board.cir>"

[defaults]
for = "120ms"
fidelity = "fast"

[[case]]
name = "rail-ok-cold"
bench = "bench/cold.toml"
targets = { "main:U10" = "firmware/prebuilt/led_rail.elf" }
asserts = ["LED-DATA-V4", "LSW-3", "LSW-2", "FW-6.4"]   # from the bench, or [[case.assert]] inline
reference = true                      # also run one-cell; require identical non-grant records
golden = "tests/golden/dms-rail/ok-cold.sha256"          # optional

[[case]]
name = "rail-wrong-cold"
bench = "bench/cold.toml"
targets = { "main:U10" = "firmware/prebuilt/led_rail_wrong_order.elf" }
expect = { "LSW-3" = "fail" }         # a negative control; every other assertion expected pass
```

- **Cases run in parallel** on the warm pool (`BB_RENODE_POOL`, or a pool
  that `bb test` starts and stops itself). The pool rule holds: results
  must not depend on leasing.
- **Outputs:**
  - `<out>/<case>/` as `bb run` writes it;
  - `<out>/test.json` (canonical JSON, deterministic: no wall times, no
    paths; per case the outcomes, values, the `run.json` hash and the
    output hashes);
  - `<out>/junit.xml`.
- **Exit status** is 0 if and only if every outcome equals its
  expectation. An unexpected `pass` of a negative control is a failure.
- **`reference = true`** and **consistency.** When the case's partition
  equals the one-cell one (no grant longer than one cell), `test.json`
  says `reference: "consistency"`, not `"equivalence"`.

### D9. JUnit

- No formal JUnit standard exists. The target is the Jenkins/Ant subset
  that GitHub and GitLab reporters read:
  - one `testsuite` per case, one `testcase` per assertion;
  - `failure` with message `value`, `limit`, `t`;
  - `skipped` for `not_evaluable`;
  - `properties`: rung, fidelity, the deck, bbm, firmware and run hashes,
    and the provenance flags (`agent_generated`, unreviewed views).
- **Wall times appear only here.** `junit.xml` is excluded from
  determinism gates; `test.json` is the deterministic record.
- The subset is committed as `spec/schema/junit-subset.xsd`. A CI job
  feeds a real reporter action. Which one, and whether its parser accepts
  the subset, is checked in B3's prototype (not verified in research).

### D10. Ported checks

The dms-* checks map onto the kinds of D5 (the research mapped E1's four
checks one to one). Each is ported when its board's `bb test` file is
written, stays as a cross-check for one milestone, and is then deleted:

- `check_dms_rail.py`;
- `check_dms_vsense.py`;
- the E3 checks.

**Not ported:** `check_feedback.py` and `check_adc.py`. They predict each
edge analytically (the RC crossing `TAU·ln((VOH − v)/(VOH − VIH))`; every
conversion's edges from the code and the model's 1063 ns / 4750 ns
timing), which no kind expresses without an expression language. By D5
they stay scripts. Adding kinds for one-off predictive checks is the scope
creep D5 exists to prevent. Their boards may still get a `bb test` file
for the checks that do fit (edge counts, `log` lines).

## Exact `spec/` changes (described, not applied)

- **`spec/project.md` section 3:** rewritten for `bb-bench/1` (D1-D4),
  with `[[assert]]` (D5) and the honesty rules (D6).
- **New `spec/test.md`:**
  - `bb-test/0`;
  - `bb test`'s semantics and exit status;
  - `test.json`;
  - the JUnit mapping.
- **`spec/event-stream.md`:** version 2. `assert` gets D7's keys and
  order; `reset` (ADR-009); the ordering rule for `assert` records.
- **`spec/package.md` section 10:** `bb-bench` and `bb-test` join the
  versioned schemas; `bb pkg migrate --bench`.
- **`spec/package.md` section 3:** the `switch` view kind (D2), accepted in
  `bb-part/1` with the schema epoch.
- **`spec/pin-model.md`:** the monitor is not glue. A sentence that
  analog monitors add no element and no boundary.

## Determinism impact

- **Every new input is data hashed into `run.json`:** CSV files, the seed
  for bounce, captures through their imported CSV.
- **Exact fs conversion** of decimal times.
- **Monitors at solver points** are partition-independent (measured).
- **Verdict records** sort deterministically.
- **`junit.xml`** carries wall times and is not a deterministic output.

## Gates

| # | Gate |
| --- | --- |
| G1 | Every Phase 1 bench migrated to `/1` builds a byte-identical `deck.cir`, and every committed hash holds |
| G2 | Playback: 1 s at 100 ksps adds ≤ 2 s of wall time; equal to the same data as a short `[[pwl]]` where both apply (≤ 1 000 points, byte-identical); partition independence at 10 µs, 1 ms and 10 ms |
| G3 | Assertions reproduce the Phase 1 scripts on the committed runs: same verdicts, same integer values (the scripts' floats rounded the documented way), every Phase 1 negative control fails exactly as before |
| G4 | Verdicts unchanged across partitions and engine orders; 10-run determinism of `test.json` |
| G5 | `junit.xml` validates against the subset and renders in the chosen reporter in CI |
| G6 | Honesty refusals: an assertion on an ideal-source net, and a current through a 0 V source, are refused with the documented messages |
| G7 | `bb capture import` round trip exact to fs on synthetic captures of both formats |
| G8 | `switch`: a pressed switch on a synthetic board gives the analytic RC settling of `ron` into a capacitor; an unpressed one draws `V/roff`; a `press` on a non-switch instance is refused |
| G9 | Every negative control in a `bb test` file has an assertion that the controlled event occurred (an `event` or `count` kind), so it cannot pass vacuously (the plan's W1 pattern) |

## Rejected alternatives

- **YAML `bench.yaml`.** It would mean a second parser and implicit typing;
  the house rule is TOML authored.
- **An expression language for assertions.** It is hard to keep
  deterministic and honest (what does `max(I(x))` mean without a sensed
  current?), and every expression would need its own semantics.
- **Evaluating assertions afterwards from `nets.csv`.** That misses
  everything between samples, and it depends on `--sample-rate`.
- **Captures read directly at run time.** That would put two binary
  parsers on the hot path, plus their float semantics.
- **The bench inside the analog engine.** Digital actions into the MCU
  then need a second producer path; an engine is the existing mechanism.

## Questions for the approver

Consolidated in the plan's section 9 as **Q6** (TOML `bb-bench/1`, a
closed set of kinds, only the dms-* checks ported).

**Defaults (apply unless the approver objects):**

1. **The ported dms-* scripts** stay one milestone as cross-checks, then
   are deleted.
2. **Capture formats:** sigrok `.sr` and Saleae binary v0 through `bb
   capture import` only. Scope CSVs are already canonical CSV after a
   header rename.
3. **Limits at `fast`** off by default (so Phase 1 event streams do not
   change) and on at `pi` (ADR-009 D3).

## Approver decisions (2026-11-15)

All twelve questions of the plan's section 9 were accepted with their
recommended answers (plan section 10). For this ADR:

- **Q6, accepted.** TOML `bb-bench/1` (not `bench.yaml`); a closed set of
  assertion kinds, extended only by amendment; only the dms-* checks are
  ported, and `check_feedback.py` and `check_adc.py` stay scripts.
- **Q3, accepted.** `bb-bench/1`, the `switch` view kind and event stream
  version 2 land in milestone S.
- **Build scope.** B1-B3 are in this build. B4 (`bb capture import`)
  belongs to the adoption exit and is not started.

**Defaults.** No objection was made to the defaults listed under
"Questions for the approver", so they apply as written.

## Review notes (design review round 1, 2026-11-14)

**Adopted:**

- **Exit item 3 and D10 over-promised** (major): `check_feedback.py` and
  `check_adc.py` make analytic predictions the closed kinds cannot
  express. D10 now ports only the dms-* checks, and the plan's exit says
  so.
- **The `switch` view kind did not exist** (minor): defined in D2 and
  added to `bb-part/1` in the schema epoch.
- **No vacuous negative controls** (from the W1 finding): G9.

**Not adopted:** none.

## Amendment 1: milestone B1 as implemented (2026-11-15)

Milestone B1 (plan section 4: `bb-bench/1`, the bench engine, timed
actions, CSV playback, current profiles, faults, the `switch` view kind)
is implemented in `bbbench`, `engines/analog-ngspice`, `bbpkg` and `cli`,
and gated by `make bench-test` (in `make test`), `make bench-gate` and
`make bench-perf` on `examples/bench`. `spec/project.md` section 3 and
`spec/package.md` sections 3 and 10 state the result. Where the
implementation decides something D1-D4 leave open, or departs from their
wording, it is recorded here.

**A1.1 The schema epoch.** Milestone S has not landed. B1 carries its own
part of the epoch, as the plan allows ("tracks develop against the epoch's
draft schemas"): the registry's current majors are `bb-part/1` (only the
`switch` view kind is new; the other `bb-part/1` keys of ADR-009 and
ADR-012 stay reserved) and `bb-bench/1`. Both migrate from `/0` by the
schema line. `/0` packages now lint with the migration hint (a warning);
S migrates them, with the one `make locks`. The Phase 1 benches are
migrated in their own commit. No `/1` file reaches `main` before S.

**A1.2 Tables, and which points are breakpoints.** D3's `EXTERNAL` source
with a cursor is a `TableSource` of the analog engine (V and I sources;
`GetISRCData` is now registered for current profiles). The solver's steps
are clamped to every point. Whether a point is also an ngspice breakpoint
is a cost question that D3 does not settle, and G2 and the plan's B1 row
pull in different directions:

- G2 asks a playback to equal the same data as a short `[[pwl]]`, byte for
  byte. A PWL card sets a breakpoint at every point, so the table must too.
  It does: `bench/playback-small.toml` (601 points) and
  `bench/playback-pwl.toml` give identical `nets.csv`, `nets.vcd` and
  non-grant records.
- The B1 row asks for 1 s at 100 ksps in at most 2 s of added wall time. A
  breakpoint restarts the integration at order 1 with a step of a tenth of
  the gap, so a breakpoint on every lattice point makes about four solver
  points per cell instead of one. Measured on `examples/bench` (interleaved,
  N = 3, medians; the host was shared with other tracks, load 14): 3.22 s
  added with breakpoints, 0.36 s without. With the rule below, `make
  bench-perf` on the idle host (N = 5, load 0.7): base 3.08 s, playback
  3.07 s total, so the playback adds nothing measurable (−0.01 s).

The rule: the points of a `hold` table (a step, which needs the
breakpoint) and of a table of at most 1 000 points (the `[[pwl]]` warning
limit, where G2 applies and the cost is negligible) are breakpoints; a
longer linear table's points are corners only. On `bench/playback-small`
the difference the breakpoints make is 4 µV on AFILT, the RC behind the
source. A breakpoint is released at the accepted point that opens the
lattice cell before the one holding it, so it is in ngspice's table
before the step decision at the cell's start; that is a function of the
solver points only (every lattice point has one).

**A1.3 `hold` is left-continuous**, as an input ramp is: at a point's time
the old value, after it the new one. The step then falls in the solver's
first, short step after the breakpoint. (Right-continuous, the solver
point at the breakpoint itself was solved with the new value, which spreads
the step over the preceding step.)

**A1.4 The bench engine's registration (D4).** D4 says "registered after
the analog engine". It is registered **first**, in both engine orders.
Inside a feedback loop the kernel orders a component's members
lattice-bound engines first, then by registration (ADR-003 D3), so the
bench then advances after the CPU (whose edges trigger it) and before the
analog engine (which its triggered sources drive): a triggered change
reaches the analog engine before it advances through the change's time, in
either engine order. Its records sort first at equal times in both orders,
so `--engine-order analog-first` stays byte-identical (gated).

**A1.5 A triggered source** is the bench engine's output, an input of the
analog engine (`Drive::Timed`) that changes at its own time by a 1 ns
ramp. An MCU edge is at a lattice point plus 1 fs, closer to the lattice
point than the solver resolves a corner (1 ps); a bare step there would be
integrated over whatever step the solver takes next (a press reached PB0
480 ns late in a trial); with the ramp's end as a corner and a breakpoint
it reaches PB0 7.6 ns after the press, as the timed press does (7.3 ns).
The cost: a bench engine between the CPU and the analog engine in a loop
leaves the kernel no quiet grant (ADR-003 D5 needs exactly one producer
feeding the quiet CPU, first in its order), so such a run's grants are one
lattice cell (`bench/trigger.toml`: 7 000 grants for 70 ms). Its partition
gates are consistency checks. A bench engine that only drives the CPU
(`bench/digital-set.toml`) is that one producer, and lookahead serves it
(31 grants for 100 ms).

**A1.6 Triggers are edges only.** D2's trigger on "the named edge or `log`
match": a `log` trigger is refused in this build; the bench engine reads
pins, and `log` matching arrives with B2's `log` assertion kind. A trigger
fires once, at the first change of the pin to `level` after `t = 0`; the
initial level is not an edge. Its action is at `t_edge + delay` on the
analog side; on an MCU pin the MCU applies it at `ceil_L(t_edge + delay)`,
as D2 states. A trigger that reached the bench only at a boundary after
its action's time is a protocol error (it cannot happen with A1.4's
order).

**A1.7 Details D2 leaves open**, decided here:

- `set` targets a `[[supply]]`'s net (`to` a voltage: the supply's voltage
  becomes the set source's node; an ideal fixed supply becomes the source
  itself) or a bound `digital_in` pin (`to` `0` or `1`). With `for`, it
  returns to the bench's own value (the supply's `volts`, the pin's
  initial level). A pin the bench drives starts low, unless its playback
  has a point at `t = 0`. One pin takes one stimulus (a playback or sets).
- Timed actions on one target may not overlap (refused).
- `bounce = { count, max }`: `2 × count` distinct toggles in `(0, max)`
  after the press and after the release (SplitMix64 from `seed`, the
  switch and the action), ending in the new state; `max` defaults to the
  view's `bounce_max` and may not exceed it or `for`. The offsets are in
  `run.json`.
- A fault is the `bb_switch` element at the default `ron` and `roff`:
  `open` moves the named instance pin to a node of its own, connected
  through the closed element before `at` (so 50 mΩ of series resistance
  before the fault); `short` is to the board's ground. Both take an
  optional `for` (not in D2) after which the fault ends.
- A `[[load]]` with a `profile` takes `interp` (not in D2), default
  `linear`.
- The switch element is the subcircuit `bb_switch a b ctl` (a control port
  beside the two contacts); its control node is `bb_ctl_<ref>` (D2 calls
  it `<ref>_ctl`).
- The bench engine's identity (`bench@1`, the content hash of `bbbench`'s
  sources) is recorded in `run.json` (`bench.engine`). Locking it as
  `engine:bench@1` changes `bb.lock`, whose schema is ADR-011's (S); it
  waits for S.
- The `bb-bench/1` keys of D2 that later milestones implement (`fidelity`,
  `corner`, `[[battery]]`, `[[activity]]`, `[[env]]`, `[probes]`, a
  `[[load]]` `replace`) are refused with the milestone named, not accepted
  inert: a bench that names one would not run as written. S may make them
  inert instead; this ADR prefers the refusal.

**A1.8 Gates as run.** G1: the five Phase 1 benches elaborate to
byte-identical decks as `/0` and `/1` (`bb run --elaborate-only`), and every
case that runs them gives its committed hashes on the migrated files:
`dms-rail-test` (E1's four cases), `dms-vsense-test` (s50, c5 and their
twins) and, run once outside `make test`, E2's sag, rec, s1 and s1-none
(`tests/golden/dms-vsense`). `make pool-equiv` with three `examples/bench`
entries passes (28 configurations, one Renode process). G2: the playback cost and the short-table identity (A1.2).
The plan's digital-playback gate: a digital playback into U1.PB0 with the
edge times that an ideal `[[pwl]]` on its net gives at PB0's thresholds
(`data/btn.csv`, checked against the `[[pwl]]` run) gives the same eight
`pin_edge` records to the fs, the same PA6 response, and every change in
`(ceil_L(t), ceil_L(t) + L]`. G8: a 1 kΩ switch charges 1 µF within 9 µV
of the analytic RC at 20.1-30 ms; open, it divides by `roff` within 5 µV;
a press on a resistor is refused naming its views. Partition independence:
every case at `--max-quantum` 10 µs, 1 ms and 10 ms equals its default run
in `nets.csv`, `nets.vcd` and the non-grant records, and the committed
hashes are those of the one-cell references.

## Amendment 2 (2026-11-15): the schema epoch (milestone S)

Milestone S landed `bb-bench/1`, the `switch` view kind and event stream
version 2. ADR-011 Amendment 1 is the record.

- **`bb-bench/1` and `switch` are B1's**, unchanged (A1.1). Every Phase 1
  bench was already `/1`. The keys that later milestones implement stay
  **refused, naming the milestone** (A1.7's preference): `fidelity`,
  `corner`, `[[battery]]`, `[[activity]]`, `[[env]]`, `[probes]` and a
  `[[load]] replace`. `spec/schema/bench.schema.json` accepts their shape.
- **`assert` v2 (D7)** is `bbcore::event::Event::Assert`. Its keys are, in
  order: `t_fs`, `name`, `outcome` (`pass`, `fail` or `not_evaluable`),
  `pass` (absent when not evaluable), `value`, `limit`, `unit` (`uV`,
  `fs`, `nA`, `udegC` or `count`), `rung` and `msg`. Optional keys are
  omitted. B2 emits it.
- **`engine:bench@1` is not locked yet** (A1.7 deferred it to S). Locking
  it in every project lock would relock all of them on every `bbbench`
  edit, which is the churn H4 removed for plugins. Which projects lock it,
  and when, is decided in track B. `run.json` keeps recording the identity.

## Amendment 3: milestone B2 as implemented (2026-11-15)

Milestone B2 (plan section 4: assertions and `assert` v2, the closed
kinds, analog monitors at solver points, windows) is implemented in
`bbcore::assert` (the kinds, the windows, the evaluator and the replay),
`bbcore::Engine::solver_points` (implemented by `NgspiceEngine`), the
scheduler (`Scheduler::set_assertions`) and `cli` (`asserts.rs`: the
`[[assert]]` tables, `bb run --assert`, `bb assert`). It is gated by `make
assert-test` (in `make test`), the replay step of `dms-vsense-test`, and
`make assert-gate`. `spec/project.md` section 3.4, `spec/event-stream.md`,
`spec/pin-model.md` glue rule 10 and `spec/schema/bench.schema.json` state
the result. Where the implementation decides something D5-D7 leave open, or
departs from their wording, it is recorded here.

**A3.1 Where the monitors run.** D6 says the analog monitors run "inside
the producing analog engine at every accepted solver point". They run in
the kernel's evaluator, over the accepted solver points that the producing
engine hands over for each exchange window (`Engine::solver_points`: a pure
read, like `sample_output`; the window `[prev, now]`, the point at `prev`
first). It is the same function of the same points. A crossing is
interpolated in the solver's own seconds and rounded up to whole fs, kept
strictly after the earlier point (the `digital_in` rule, ADR-003 D3). A
value at an instant is `bbcore::assert::value_at`, bit for bit the
interpolation `NgspiceEngine::sample` uses. One evaluator then serves every
analog engine and `bb assert`'s replay (A3.5), and no engine gains state.
Nothing in `spec/time-grant.md` changes: no element, no input and no grant
boundary is added. An analog net that an assertion reads and nothing probes
becomes an output of the analog engine, bound to its kernel net and absent
from every output file (`spec/pin-model.md` rule 10). Gated: with the
assertions on, every committed one-cell hash of E1, E2 and E3 (nets.csv,
nets.vcd, the non-grant records without the `assert` records) holds.

**A3.2 One record per assertion, at its window's close.** D5 and D7 leave
open how the instances of one check (several edges in a window) become
records. Each assertion gives exactly one `assert` record, at the close of
its window: its `to`, its `until` event, or the run's end. The record
reports the worst instance (the one past a limit, else the one nearest a
limit) and counts the instances in `msg`. B3's `test.json` and JUnit map
one `testcase` to one assertion (D9). At equal `t_fs` the kernel's `assert`
records come after its `net_sample` records, in the run's assertion order
(the bench's, then each `--assert` file's).

Two details keep this partition independent. Both were found by the gate
(`bb assert` on E2's s50-none disagreed with the run on one record's
`msg`).

- A `log` record can sit at its exchange window's start. A UART line that
  ends on a lattice point is reported at the next exchange, with its own
  time. The evaluator reads every record at its own time; it reads a
  window's records in `[prev, now]`, not `(prev, now]`.
- A window that closes exactly at a boundary is decided at the next
  exchange, which may still bring a log at that instant. Its record keeps
  the close's time, so it sorts as it would where that instant falls
  inside a window.

A unit test runs both at three partitions.

**A3.3 Windows.** D5's windows are built as follows.

- `from` and `to` are times.
- `after = { edge | log, delay }` opens the window at the first such event
  at or after `from`, plus the delay.
- `until = { edge | log }` closes it at the first such event at or after
  the opening. D5's "until the run's end" is the default.
- `while = { net, level }` keeps only the parts where a digital net is at a
  level.

The window is the closed interval between the opening and the close. A
window that never opens fails. `edge_timing` takes a third kind of event,
an analog crossing (`{ cross, ref, below | above }`). E2's shed timing
needs it: LED_PWR_EN must fall within one burst period after V(VSYS_SENSE)
crosses the code boundary.

**A3.4 Kind details that D5 leaves open.**

- `settle`. Each change of the event net to its level opens an instance.
  - The instance passes if the quantity is inside the band by `within` and
    stays inside until the net changes again (or the window closes).
  - The band is in volts, or `"10%"` of the value at the event (LSW-2:
    10 % of VLED at the LED_PWR_EN fall).
  - D5's run-end rule applies to an instance that is still open at the
    run's end and whose deadline is after it: that instance is
    `not_evaluable`. The other instances are decided.
  - The assertion fails if any instance failed. It passes if at least one
    instance passed and none failed (as `check_dms_rail.py`'s LSW-2 does
    for E1's second fall, 4.6 ms before the run's end). Otherwise it is
    `not_evaluable`.
- `edge_timing` pairs each `end` event in one of two ways:
  - `last` (the default): with the last `start` event at or before it.
    FW-6.4 needs this.
  - `first`: with the first `start` event since the `end` net last
    changed. E3's latency is measured from the first `field above BOP`
    record, and E2's shed from the first crossing.
- `log`.
  - A pattern is a literal, anchored with `^` and `$` or neither. Every
    other regular-expression metacharacter is refused, so a later full
    regex cannot change what a pattern written today means.
  - `field = "<name>"` reads the number after `<name>=` or `<name> ` (a
    whole word) in each matching record: an integer, or a decimal with a
    unit, converted exactly. That is how a code or a device parameter is
    read without an expression language.
- `event` counts `reset` records and `warn` and `error` log records. D5
  lists `PromiseBroken` as an event, but a broken promise fails the run, so
  no completed run has one to count. It is not a countable event.
- A kind with no instance in its window fails. Negative controls therefore
  cannot pass vacuously (plan section 7).
- `rung` is the run's rung (`fast` until P1), and `requires` above it gives
  `not_evaluable`. `nA` and `udegC` are not used yet.

**A3.5 Where assertions live: `--assert` and `bb assert`.** D5 puts
assertions in a bench or a test case. B2 implements `[[assert]]` in a
bench. It also adds two things D5 does not name.

- **Assertion files.** An assertion file is a `bb-bench/1` file of
  `[[assert]]` tables only. `bb run --assert` adds it to a run, for a
  project and for a board file alike. E3's homing runs are board files,
  which take no bench.
  - The ported checks are assertion files. So the Phase 1 benches and every
    committed hash stay unchanged, and the cases whose codes differ (E2's
    eight) need no copies of their benches.
  - Gated: a bench's own `[[assert]]` and the same tables in an `--assert`
    file give a byte-identical `events.jsonl`.
  - B3 decides where its test cases take their assertions from.
- **`bb assert <run>`.** It evaluates assertions again on a finished run's
  `events.jsonl`.
  - Every kind that reads only records gives the record the run gave,
    since it is a pure function of the records. Gated on every gate run.
  - A kind that reads solver points is `not_evaluable` there.
  - It is how Phase 1's FW-6.4 negative controls carry over: they move
    `pin_edge` records of a finished run. It is also how `dms-vsense-test`
    checks E2's codes on its own runs without a second run.

**A3.6 G3, "the same integer values", as gated.** D6 and G3 conflict where
the scripts read samples rather than solver points: E1's every 10 µs, E2's
every 100 µs, each rounded to whole µV in `nets.csv`. An extremum between
two samples, or a crossing between them, is not on the scripts' data, and
evaluating from `nets.csv` is the rejected alternative. Measured on the
committed runs:

- **LSW-3:** the assertion is 6 µV above the script on wrong-cold (the
  maximum is at the P0.06 fall, 45.26175 ms, 1.75 µs after the script's
  sample) and 9 µV above on wrong-nominal. It equals the script on the ok
  cases.
- **LSW-2:** the settle time is within 15 ns of the script's on every case.
- **E2's shed:** the crossing is 174.4 µs before the script's first sample
  below. The script compares µV-rounded samples (`round(v) + e < B`), which
  is 0.5 µV further down a ramp of 3.5 µV/ms.

The gate, `scripts/check_asserts.py`, is the closest correct one:

- **Verdicts** equal everywhere.
- **Exact values** where both read the same data:
  - every edge time and gap (FW-6.4's, exact in fs);
  - every level, count and log field (E2's 36 reading codes and its shed
    and restore codes; E3's tS, tACTIVE and latencies, exact in fs);
  - the value at an instant (LED-DATA-V4's VLED at each P0.06 rise, in µV).
- **An extremum** within [−1 µV, its change over one script sample period
  at the script's extremum + 1 µV].
- **A settle time** equal at the scripts' printed resolution (1 µs).
- **The shed crossing**, moved by 0.5 µV divided by the ramp's slope (from
  the script's own samples 1 ms either side), within one script sample
  period of the script's crossing. Measured: +31.5 µs, against 100 µs.

**A3.7 How the scripts' checks became assertions.**

- **E1** is `examples/dial-me-spinner/asserts/e1.toml`: LED-DATA-V4 (`at`),
  LSW-3 (`range` of `V(LED_DIN) - V(VLED)`), LSW-2 (`settle`) and FW-6.4
  as three assertions: LED_PWR_EN high at each P0.06 rise, P0.06 at least
  10 ms after the LED_PWR_EN rise, and LED_PWR_EN off at least 30 ms. The
  script's FW-6.4 passes exactly when all three do.
- **E2's codes** are `asserts/e2/<case>.toml`, written by
  `scripts/dms_asserts.py` from `check_dms_vsense.py`'s results. Each
  reading's code equals the code the script's model predicts (loaded
  cases) or the unloaded pad's code (the `-none` twins), and the number of
  readings is fixed.
  - The model stays in the script, since D5 has no expression language;
    the assertion carries its result for this bench as exact integers.
  - `assert-gate` regenerates the files from its own runs and requires
    them byte for byte.
  - The shed (`sag`) and the restore (`rec`) are thresholds: codes ≥ 1283
    before the shed and ≤ 1282 at it, the LEDs' state in each reading, the
    restoring reading's VBAT_est ≥ 3.58 V, LED_PWR_EN's edge counts, and
    the off time ≥ 30 ms.
  - The shed timing's threshold is the script's code boundary less its
    modelled mean sampling error, rounded up to whole µV.
- **E3's HOME_N latency check** is three assertions in
  `e3/asserts/<corner>.toml`: the device's tS and tACTIVE (`log` fields of
  its start record), and the latency from the first `field above BOP`
  record to HOME_N's fall after H1, at most tS + tACTIVE. The worst-phase
  bound tS + tACTIVE ≤ the limit then follows from the constants. Park
  adds H1's release latency. The script's check passes exactly when all
  three pass.

**A3.8 Negative controls (plan section 7).** Every Phase 1 negative control
of the ported checks fails as before:

- E1's wrong-order firmware fails exactly LSW-3, at both benches, in-run.
- The FW-6.4 controls (ok-cold's `pin_edge` records moved): `bb assert`
  fails exactly FW-6.4-on (on-wait) and FW-6.4-off (off-wait), as
  `check_dms_rail.py` fails exactly FW-6.4 on the same files. The analog
  assertions are `not_evaluable` in the replay; the script read them from
  `nets.csv`.
- E3's neg-fb200 fails exactly HOME_N-tS: its seeded phase gives the FA
  run's HOME_N instant.
- E3's neg-fb286 fails HOME_N-tS and HOME_N-latency (108.302 ms, the
  script's number).
- E3's neg-fast fails none of the three. The script fails it on its step
  rate, park-stop and H3 checks, which are not ported.

**A3.9 Partition independence.**

- A synthetic run in `bbcore`'s unit tests gives identical records at three
  partitions, including one of one-cell windows.
- E1 is acyclic, with 10 ms grants. Each case at `--max-quantum` 10 µs (its
  one-cell reference, a true equivalence) and 1 ms gives the same `assert`
  records and every non-grant record. ok-cold with `--engine-order
  analog-first` is byte-identical, and 10 runs are byte-identical.
- E2's s50 and sag as their every-lattice-point references (`--lookahead
  off --adc-push every`: other partitions) give the same records.
- E3's partitions coincide (ADR-008 Amendment 12), so it adds a consistency
  check only and is not part of the partition gate.

**A3.10 Honesty (D6, G6).** Both refusals are implemented, each with its
message, and gated:

- a check on a net that an ideal bench source forces;
- a branch current (`i(...)`, `#branch`). No kind reads a current yet.

**A3.11 Not changed.** `engine:bench@1` is still not locked (Amendment 2).
B2 adds no engine and no bench stimulus.

## Amendment 4 (2026-11-15): the P1 bench keys

Milestone P1 implements four `bb-bench/1` keys that D2 named for it (ADR-009
Amendment 4 is the record; `spec/project.md` section 3.2 states them):

- `fidelity` (`fast` or `pi`; `--fidelity` wins) and `corner`, as D2 shows.
- **`[corners]`**, `"<board>:<ref>" = "<name>"`: per-instance corners,
  which ADR-009 D8 asks for ("globally, or per instance") without a
  syntax. A table beside `corner` keeps `corner` a string, as S's schema
  has it.
- **`[[battery]]`** has D2's keys exactly (`net`, `ocv`, `soc`, `r0`,
  `rc`, `temp_c`). D2 gives the cell no capacity, so the state of charge
  holds for the run. It is written as a current `OCV/R0` beside a real
  `R0`, then the RC pairs (`I`, `R` and `C` elements), not D2's comment
  "B and C elements": a node held only by a B source fails ngspice's
  source stepping (ADR-009 Amendment 4).

`[[activity]]`, `[[env]]`, `[probes]` and a `[[load]] replace` stay
refused, naming their milestones.

## Amendment 5: milestone B3 as implemented (2026-11-16)

Milestone B3 (plan section 4: `bb-test/0`, `bb test`, `test.json`, JUnit) is
implemented in `cli` (`test_cmd.rs`: the file, the runs, the comparisons,
`test.json` and `junit.xml`; `bb run --image`), `bbpkg::schema` (`bb-test`
at major 0) and the normative `spec/test.md`, `spec/schema/test.schema.json`
(the file), `spec/schema/test-result.schema.json` (`test.json`) and
`spec/schema/junit-subset.xsd`; `spec/project.md` section 5 (`bb run
--image`, `bb test`) and `spec/package.md` section 10 (`bb-test` joins the
versioned schemas) state the rest. The three `dms-*-test` targets of `make
test` are `bb test` files (`examples/dial-me-spinner/tests/e1.toml`,
`e2.toml`, `e3.toml`), gated in `make test` and by `make bb-test-gate`.
Where the implementation decides what D8 and D9 leave open, or departs from
their wording, it is recorded here.

**A5.1 The case keys.** D8's example shows `bench`, `targets`, `asserts`,
`[[case.assert]]`, `reference`, `golden` and `expect`, and `[defaults]` with
`for` and `fidelity`. The dms-* targets need more of `bb run`, so a case (or
`[defaults]`) takes every run option those targets use, each with its `bb
run` meaning: `probes`, `sample_rate`, `max_quantum`, `renode_quantum`,
`engine_order`, `lookahead`, `adc_push`, `seed`, `allow_unqualified`, and a
case's own `project` or `board` (E2's variants and E3's project route are
other projects than the file's). Two keys are new:

- **`assert_files`**: B2 (Amendment 3, A3.5) left to B3 where a case takes
  its assertions from. From the bench's `[[assert]]`, from assertion files
  (each `bb run --assert`; the ported checks are assertion files, so the
  Phase 1 benches and every committed hash stay unchanged), and from inline
  `[[case.assert]]` tables, which `bb test` writes as an assertion file for
  the run. `asserts` then lists the names the case reports; without it,
  every `assert` record of the run is reported.
- **`same_as = "<case>"`**: `nets.csv`, `nets.vcd` and `events.jsonl`
  byte-identical to another case. Every dms-* target checks an engine order
  that way (ADR-003 D3), which neither a golden (no grant records) nor
  `reference` (another partition) expresses.

**A5.2 `targets` and `bb run --image`.** D8's `targets = { "main:U10" =
"firmware/prebuilt/led_rail.elf" }` runs a project with another image than
its `[targets]`, and `bb run` is read-only on a project (ADR-007 D7) and
took the image from `[targets]` only. `bb run --image <instance>=<elf>` (a
project only) puts the ELF in place of that instance's `[targets]` image in
memory: its image hash is computed as `bb flash` would write it, `elab`
checks the target as for a flashed image, and `run.json` records it exactly
as a flashed one (`kind`, `hash`), so a case and a `bb flash`ed copy of the
project give the same `run.json`. `bb.toml` is not written. The Phase 1
scripts ran each firmware on a scratch copy of the project with `bb flash`;
the bb test files run the repository's project itself.

**A5.3 E2's variant projects.** Three of `dms-vsense-test`'s four cases run
the project with one edit, which `scripts/dms_vsense.sh` made in scratch
copies: `load = "none"` on the VSYS_SENSE binding, C5 at 10 nF in the
netlist, and both. A `bb test` case names a project, so they are committed
projects, `examples/dial-me-spinner/e2/none`, `e2/c5` and `e2/c5-none`,
written by `scripts/dms_e2_projects.py` from the project's own `bb.toml`
(every binding and probe kept, the netlist path changed, no `[targets]`)
and, for C5, from the vendored netlist with exactly that one edit;
`--check` runs in `dms-vsense-test`, and their `board.bbm` and `bb.lock`
are `bb open`'s (`PKG_BOARDS`, so `make packages` and `make locks` cover
them). `e2/none`'s `board.bbm` is the project's byte for byte (`load` is a
binding parameter, not board content). `make dms-vsense` keeps its scratch
copies.

**A5.4 `golden` and `reference`.**

- A golden is a `sha256sum` file of a run without assertions (every
  committed one). So `non-grant-records` is compared with the run's records
  without the grant and the `assert` records: the assertions are passive
  (A3.1), and the committed one-cell hashes hold for a case with
  assertions. `nets.csv` and `nets.vcd` are compared as they are.
  `events.jsonl`'s line is not compared (its grant records depend on the
  partition) and `test.json` lists it as `not_checked`, as the scripts'
  `nongrant` checks did.
- `reference = true` is the one-cell run `--max-quantum <L> --lookahead
  off`. Its records without the grant records (the `assert` records
  included: B2 gated that they do not depend on the partition) must equal
  the case's. "Consistency" (D8) is decided from the case's own grant
  records: no grant longer than one lattice cell. The E2 references are
  `--lookahead off --adc-push every`; their hashes are E2's goldens, so the
  E2 file compares goldens and runs no reference.

**A5.5 `test.json` (`bb-test-result/0`).** D8's content: per case the
outcomes and values (each reported assertion's record: `t_fs`, `value`,
`limit`, `unit`, `rung`, `msg`, with its expectation), the `run.json` hash
and the output hashes, plus `firmware`, the run's status and the result of
each comparison; at the top the file's name and sha256 and the failed
cases. No wall time, path or host; an error message (which can carry a
path) goes to `junit.xml` only.

**A5.6 JUnit (D9).**

- The reporter is **dorny/test-reporter v3.2.0** (commit
  `d1b5dff021aa968b288b02285a8bf47471c5928a`), `reporter: java-junit`.
  The prototype check D9 asks for runs the action's own parser
  (`JavaJunitParser`) and report renderer (`getReport`), from its source at
  that commit, in a `node:24` container (`scripts/junit_reporter_check.sh`,
  in `make bb-test-gate`): on every `junit.xml` of the gate, the parser's
  suites, testcases and counts equal what `bb test` wrote, and the report
  renders naming every suite. `.github/workflows/ci.yml`'s `test` job runs
  the action at the same commit on `out/dms-*-test/junit.xml` after `make
  test` (with `checks: write`; skipped on a pull request from a fork, whose
  token cannot create a check run). That step runs on GitHub only after a
  push, which this build does not make (no publishing): it is unrun until
  the human's next push, and the local run of the action's parser is the
  check.
- **Departures from D9's wording.** Every case has a `run` testcase (an
  `error` when the run failed), because the parser drops a `testsuite`
  without a `testcase`. `skipped` is for a `not_evaluable` outcome that was
  expected; one that was not expected is a `failure` (type
  `not_evaluable`), since it fails the exit status (D8). The `properties`
  are D9's: `rung`, `fidelity`, `deck`, `board_bbm`, `firmware`,
  `run_json`, and the provenance flags as `agent_generated` and
  `unreviewed_views` (the views `bb run` flags until a person reviews them,
  `spec/package.md` section 2; `run.json` carries no `reviewed_by` yet).
- The subset is `spec/schema/junit-subset.xsd`; `scripts/check_bb_test.py`
  validates every `junit.xml` with `xmllint` (`libxml2-utils`, added to
  `make setup` and `make doctor`) and requires it to say what `test.json`
  says.

**A5.7 G9 is a file rule.** A case with an expected `fail` must report a
`count` or `event` assertion expected to pass; otherwise `bb test` refuses
the file before running anything, naming G9. E1's wrong-order controls
report `EN-driven` and `DIN-driven` (`asserts/e1-run.toml`: the firmware
switched LED_PWR_EN on and drove LED_DIN_MCU).

**A5.8 The dms-* targets as `bb test` files, with the same outcomes.** Each
target runs its file through `bb test` (on the pool `make` provides, or
every Renode fresh with `POOL=0`), validates `test.json` and `junit.xml`,
then runs on the same outputs what is not an assertion kind (D5, D10):

| Target | In the `bb test` file | Still a script, on `bb test`'s outputs |
| --- | --- | --- |
| `dms-rail-test` (`tests/e1.toml`) | the four cases at bb's defaults; E1's four checks (`asserts/e1.toml`); the PIN_CNF audit (`PIN_CNF-audit`, `no-warn`) and the firmware's drive (`asserts/e1-run.toml`); wrong fails exactly LSW-3 (`expect`); the committed one-cell hashes (`golden`); ok-cold at analog-first byte-identical (`same_as`) | `check_dms_rail.py` (the D10 cross-check) and `check_asserts.py e1`; the VLED snapshots (implied by the golden's `nets.csv`, kept as a tolerance check); the convergence wall time (not deterministic); the two FW-6.4 negative controls (moved `pin_edge` records of a finished run) |
| `dms-vsense-test` (`tests/e2.toml`) | s50, s50-none, c5, c5-none on the project and its variants; E2's codes in-run (`asserts/e2/<case>.toml`, record-only); the committed hashes (`golden`) | `dms_e2_projects.py --check`; `check_dms_vsense.py` (the analog side against `load = "none"`, the suite's charge-sharing arithmetic, C5 / 10) and `check_asserts.py e2`; `run.json`'s load records |
| `dms-home-test` (`tests/e3.toml`) | step on `board-step.cir` and on the project `e3/step`; the step test's UART phases (`e3/asserts/step.toml`: each line present, the SETTLE time and microstep as `log` fields, no line saying fail); the committed one-cell hashes (`golden`); step at analog-first byte-identical (`same_as`) | the boards are the generator's output and the projects current; `check_dms_home.py --step` (the phases' order, every phase's step rate from the PWM edges, the step against the suite's microstep figures: no kind); `compare_project_run.py` (the documented name mapping) |

Every check the Phase 1 targets made is still made, on the same runs, and
gives the same verdict: `make dms-rail-test`, `dms-vsense-test` and
`dms-home-test` pass, and every committed hash holds with the assertions on.
The engine-order run of `dms-rail-test`, run after the four cases before,
is now a case beside them.

**A5.9 Gates (`make bb-test-gate`, `scripts/bb_test_gate.sh`).** bb test's
semantics and negative controls on the Phase 0 boards (`tests/bb-test`:
equivalence and consistency, an expected `not_evaluable`; an unexpected
pass, a golden and a `same_as` mismatch and a refused run each failing for
its own reason, exactly), the G9 refusal, ten runs of each dms-* test file
with `test.json` byte-identical (G4; the tenth with every Renode fresh, so
`test.json` does not depend on leasing), `check_bb_test.py` on every run,
and the JUnit reporter on every `junit.xml` (G5). Measured:
`make bb-test-gate` passes in 860 s pooled, beside other tracks (load
6-18): the reference kinds as stated, the four negative controls failing
exactly, G9 refused, `test.json` byte-identical over ten runs of each file
(E1 `c433561e…`, E2 `3be4e89f…`, E3 `7416aa13…`, the tenth fresh), and the
reporter's parser reading all five `junit.xml` files (14, 11, 60, 42 and 30
testcases; 4 failed and 1 skipped where `bb test` wrote them).

**A5.10 Budget (+0 s, "replaces scripts").** `make bb-test-cost` (measurement lock;
`scripts/bb_test_cost.sh`), the three targets against their Phase 1
scripted runs on one 5-slot pool, interleaved, N = 3, medians: dms-rail-test
25.5 s scripted against 11.2 s as a `bb test` file (its five runs now in
parallel), dms-vsense-test 46.7 s against 48.5 s, dms-home-test 53.9 s
against 48.5 s; the sum 122.6 s against 108.2 s, **-14.4 s**. The host was
not idle (other tracks' gates; load 12-19 over the rounds), so the numbers are
to be re-measured idle with `make baseline` (H6); the sign holds in every
round. In a pooled `make check` (1045 s, load 9-15) the steps took 4.6 s,
30.3 s and 36.0 s.

**A5.11 Not changed.** No engine, no kernel record and no output of `bb
run` changes; the Phase 1 scripts' scratch-copy runs stay in `make
dms-rail`, `make dms-vsense` and `make dms-home` (and as `scripts/dms_<x>.sh
test-scripted`). The ported scripts stay one milestone as the cross-check
(D10), so B3 deletes none of them.
