# ADR-010: Bench v1, assertions, `bb test` and JUnit

- **Status:** Accepted (2026-11-15; see "Approver decisions
  (2026-11-15)"). Proposed 2026-11-14, revised after design review round 1
  (see "Review notes").
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
