# Time-grant protocol

Status: normative. Rationale: `docs/adr/002-time-grant-protocol.md`
(Advance ordering: Amendment 2, Gauss-Seidel intra-grant delivery;
`t_safe` wording and sampled probes: Amendment 3). The grant lattice,
`edge_time` at the producer's resolution and the never-early input rule are
`docs/adr/003-cosim-speedup.md` D2 (plan step 2, Amendment 2); the end-of-run
hook (`end_of_run`) is ADR-003 Amendment 1; traces, the plan order, the loop
cap, the record order and sampled VCDs are ADR-003 D3/D4 (plan step 3,
Amendment 3); `digital_in` thresholding, which produces the analog → digital
traces of example 3, is ADR-003 D3 (plan step 4, Amendment 4); CPU
lookahead (`quiet_until`), the per-grant plan, the early stop
(`advance_until`), the promise audit and the query back-off are ADR-003 D5
(kernel side: plan step 6, Amendment 6); lookahead on by default and
the Part B half of example 3 are ADR-003 plan step 8 (Amendment 9); ADC
pushes (`Analog` trace entries at lattice points), their delivery at their
own time and the code-change watch are ADR-003 D6 (plan step 9, Amendment
10).
Implementation: `bbcore::Scheduler`. Changing this file requires an ADR (see
`AGENTS.md`).

This is the piece agents most often get subtly wrong. Read all of it before you
touch an engine adapter.

## Definitions

- **Time:** `u64` femtoseconds (`bbcore::Time`). All comparisons and arithmetic
  below are integer.
- **now:** the kernel's current time, which is always the last boundary.
  Every engine is at exactly `now` between iterations.
- **t_safe_i:** engine *i*'s promise that, advanced to any `g <= t_safe_i`,
  it reports every output change in `(now, g]` at the boundary `g`, with its
  exact `edge_time`: as an ordered **trace** (any number of changes per pin)
  or, for an engine without one (legacy), at most one change per digital pin
  (invariant 5). It does *not* promise that outputs are constant inside the
  grant (a CPU can toggle a pin at any instruction). Consumers after the
  producer in the plan order apply each change at its `edge_time` (see
  "Advance"). It must satisfy `t_safe_i > now`. `Time::MAX` means "I report
  everything", not "I am idle" (ADR-002 Amendment 3, ADR-003 D4). An engine
  with a complete trace (Renode) returns `Time::MAX`.
- **trace** (ADR-003 D3): `read_output_trace()`, every output change of the
  last grant `(p, g]` as `Transition { t, pin, signal }`, sorted by `t`. Per
  pin, times are strictly increasing, a digital entry's `edge_time` equals
  its `t`, levels alternate, and the last entry equals the pin's
  `read_outputs()` value. A digital pin absent from the trace did not
  change; an analog pin absent from it is reported by its boundary value.
  An `adc_in` output (ADR-003 D6, `spec/pin-model.md`) is an analog pin that
  *is* traced: each push is an `Analog` entry at a lattice point, and a pin
  absent from the trace did not change.
  Consumers receive the changes on their inputs through
  `apply_input_trace(base, trace)`; its default applies `apply_inputs` once
  per entry, in order, with the snapshot as of that entry. An `Analog`
  value carries no time of its own, so an engine that must apply an analog
  entry at its time (a lattice-bound ADC consumer) implements
  `apply_input_trace` and takes the time from the entry's `t`.
- **binding graph, plan order** (ADR-003 D3): an edge j→i when an output of
  engine *j* drives a net that engine *i* reads. At the first `run` the
  kernel computes its strongly connected components and orders them with
  Kahn's algorithm, ties broken by the lowest registration index; inside a
  component, lattice-bound engines come first, then registration index.
  That is the **plan order**. The graph is **cyclic** if a component has more
  than one engine or an engine reads a net it drives.
- **grant g:** the time every engine advances to in one iteration.
- **lattice L** (ADR-003 D2): the step on which every grant boundary must
  fall. An engine declares it through `grant_lattice()` (Renode: its global
  quantum, `--renode-quantum`, default 10 µs); engines that declare none are
  not lattice-bound. All declared lattices must be equal. Without one, `L`
  is 1 fs and every rounding below is the identity. `L` is run
  configuration, like the seed: it sets the CPU's firmware-timing, edge-stamp
  and input resolution, and changing it changes results. `floor_L(t)` and
  `ceil_L(t)` round to multiples of `L`.
- **edge_time:** the producer's reported time of a change, at the producer's
  resolution. For Renode that is the sync point that opens the slice in which
  the write executed (at most `L` before the true instruction time), with the
  lattice +1 fs rule of ADR-003 D2: a stamp `s` that is a multiple of `L` is
  reported as `s + 1 fs`, any other stamp (a timer-driven sync) exactly; a
  change stamped exactly at the grant end belongs to the next grant. "Applied
  at its true time" below means "applied at the producer's `edge_time`".
- **boundary:** the instant `g` at which pin signals are exchanged.
- **max_quantum:** a pure cap on `g - now`, greater than 0 (`bb` default
  10 ms). It bounds the wall time of one advance, buffered traces and output
  latency; with the lattice and traces it never changes results.
- **loop_quantum** (ADR-003 D4): the cap on `g - now` while the binding graph
  is cyclic. Default `L` (one-cell lockstep); a cyclic graph without a
  lattice needs an explicit value. With lookahead a quiet grant lifts it
  (see "Quiet grants").
- **quiet_until** (ADR-003 D5): `quiet_until(now)`, a promise separate from
  and stronger than `t_safe`: if the engine's inputs stay constant, its
  outputs stay constant on `(now, quiet_until)`. `now` (the default) is no
  promise; a promise is *useful* when `floor_L(quiet_until) > now`. Asked
  only with lookahead on, in Query, before any input of the grant is
  delivered; a pure read.
- **lookahead:** a scheduler setting, on by default (`SchedulerConfig`,
  and `bb` on every board that loads Renode's lookahead plugin, i.e. every
  board with a feedback loop). Off, every grant of a cyclic graph is
  capped at `loop_quantum`: the one-cell reference. On or off gives the
  same outputs; only `grant` records differ.
- **lookahead candidates:** the engines in a feedback loop (a component of
  more than one engine, or reading a net they drive) for which a quiet grant
  with `Q = {c}` passes the structural checks below. Only candidates are
  asked for `quiet_until`.
- **early stop:** `advance_until(t, watch)` on an engine with
  `can_stop_early()`: it advances towards `t` and, at the first change of a
  pin in `watch` at `t_x`, stops at `r = ceil_L(t_x)` (at most `t`) and
  returns `r`; with no such change it returns `t`. A watched `adc_in` pin
  changes only at lattice points (its pushes), so it stops at the push
  itself: the code-change watch of ADR-003 D6.
- **t_end:** end of the run.

## State machine

```
            +------+
            | Init |  bind engines and nets; build the plan; initial Exchange at t = 0
            +------+
               |
               v
   +-------> Query ---- t_safe_i <= now -------------------------+
   |           |                                                   |
   |           v       [lookahead, cyclic: quiet_until(now) of the candidates]
   |         Grant     g = floor_L(min(t_safe_i, next_sample,      |
   |           |             now+loop_quantum [cyclic], now+max_quantum, t_end))
   |           |       or a quiet grant G (see "Quiet grants")      |
   |           |       (floor_L(..) <= now: Lattice error)            |
   |           |       emit {"type":"grant", t_fs: now, until_fs: g}  |
   |           |       (a quiet grant: once the early stop r is known) |
   |           v                                                   |
   |        Advance    per engine i, in plan order:  --------------+-- engine error
   |           |         [apply_input_trace if an input net changed  |   / causality
   |           |          earlier in this grant]; advance_to(g);    |   / PromiseBroken
   |           |         (quiet grant: the grant's order; the first
   |           |          runs advance_until(G, watch) -> r; g := r)
   |           |         drain_logs; read_output_trace; read_outputs -> nets
   |           |       now := g                                    |
   |           v                                                   |
   |        Exchange   undelivered changes; inputs (snapshot);    -+-- engine / I/O
   |           |       samples; events; probes                     |
   +-- now < t_end                                                 v
               |                                                +-------+
           now == t_end                                         | Error | (terminal)
               v                                                +-------+
            +------+
            | Done |  end_of_run() per engine; run(t_end') with t_end' > now resumes at Query
            +------+
```

| State | Entry action | Exit to | Condition |
| --- | --- | --- | --- |
| Init | check the lattice: declared lattices equal; `max_quantum`, `loop_quantum`, `t_end` and the `event_sampling` period multiples of `L` (`Config` error otherwise); build the plan order from the bindings; a cyclic graph without a lattice or `loop_quantum` is a `Config` error | Exchange (initial) | first `run()`, at least 1 engine |
| Query | `t_safe_i = next_safe_time(now)` for every *i*; with lookahead on, a cyclic graph and the loop cap binding, `quiet_until(now)` for every candidate not backed off (see "Quiet grants") | Grant | every `t_safe_i > now` |
| Query | | Error | any `t_safe_i <= now` (`NoProgress`) |
| Grant | compute `g` (or the quiet grant `G`), emit `grant` (a quiet grant: after the early stop) | Advance | `g > now` |
| Grant | | Error | `floor_L(min(..)) <= now` (`Lattice`) |
| Advance | for every *i* in plan order (a quiet grant: its own order, the first engine with `advance_until`): pre-advance delivery, `advance_to(g)`, collect (see "Advance"); `now := g` | Exchange | every call returned Ok |
| Advance | | Error | any engine error, causality violation or broken promise (`PromiseBroken`) |
| Exchange | see "Exchange" below | Query | `now < t_end` |
| Exchange | | Done | `now == t_end` |
| Exchange | | Error | causality violation, engine error or I/O error |
| Done | `end_of_run()` for every engine, in registration order (ADR-003 Amendment 1) | Error | any `end_of_run` error |
| Done | | Query | `run(t_end')` with `t_end' > now` |
| Error | the records of every completed grant are flushed to the sink's writer | none | every later call returns `Halted` |

## Invariants (MUST)

1. **No free-run:** an engine's internal time never exceeds the latest grant.
   `advance_to(t)` is called only with `now < t <= t_safe` (the engine's most
   recent promise) and `t` on the lattice, and the engine stops at exactly
   `t`. An engine may split its own advance into steps that each end on the
   lattice at or before `t` (Renode, to apply inputs at `ceil_L`); that is
   not a free-run. `advance_until(t, watch)` returns `r` with
   `now < r <= t`, `r` on the lattice or equal to `t`, and the engine stops
   at exactly `r`; every later engine of the grant is then advanced to `r`.
   Advancing a producer before a quiet consumer is not a free-run.
2. **Grant = min:** `g` is exactly the minimum in the Grant row, rounded
   down to the lattice after the min. The `loop_quantum` term is present only
   while the binding graph is cyclic and the grant is not a quiet grant. When
   several bounds tie, `limited_by` names the first in this order: engines in
   registration order (a `t_safe` or a `quiet_until` term), then `sample`,
   then `loop_quantum`, then `max_quantum`, then `t_end`. Rounding never
   changes the label. A quiet grant whose first engine stopped early
   (`r < G`) is labelled `stop:<engine>`.
3. **Lockstep:** every engine is advanced to the same `g` in every iteration.
   Engines advance one after another in plan order (ADR-003 D3), or in the
   quiet grant's order (D5), never concurrently.
4. **Progress:** `g > now`. Every term is greater than `now`, and `now`,
   `max_quantum`, `loop_quantum`, `t_end` and aligned sample instants are on
   the lattice, so only an engine with `t_safe < now + L` can make
   `floor_L(g) <= now`. That is `SchedError::Lattice { engine, now, t_safe }`,
   never a silent clamp; the remedy is a smaller `L` or a fix to that
   engine's `t_safe`.
5. **Digital transitions:** at boundary `g`, after previous boundary `p`:
   - **traced pin** (the engine's trace has entries for it): entry times are
     strictly increasing within `(p, g]`, each digital entry's `edge_time`
     equals its time, levels alternate starting from the level at `p`, and
     the last entry equals the boundary value. A `pin_edge` is emitted for
     every entry. A digital pin of a traced engine that changed without a
     trace entry is an error.
   - **legacy pin** (engine without a trace):
     - level changed: `p < edge_time <= g`, and `pin_edge` is emitted at
       `edge_time`;
     - level unchanged: `edge_time` is unchanged. A moved `edge_time` means
       more than one edge fell inside one grant, and is an error.
   - `edge_time > g` is always an error.
6. **Determinism:** the order of every call and every emitted record is a pure
   function of the inputs. See ADR-002 D6 and ADR-003 "Determinism argument".
7. **Lattice:** every boundary, `max_quantum`, `loop_quantum`, `t_end` and
   every aligned sample instant is a multiple of `L` (ADR-003 D2).
8. **Quiet promises** (ADR-003 D5.4): in a quiet grant `(now, r]`, an engine
   of `Q` that promised `q = quiet_until(now)` reports no output change at a
   time `< q` (the kernel checks its changes in `(now, r]`; since
   `r <= floor_L(q)`, only a change exactly at `r = q` is allowed), and an
   engine that detects its own early wake (a CPU leaving its sleep before
   `r` without an output change) reports `EngineError::PromiseBroken` from
   its advance. Either is `SchedError::PromiseBroken { engine, promised, at
   }`. The early stop `r` must be `min(ceil_L(t_x), G)` for the first
   change `t_x` of a watched net in `(now, r]`, or `G` if there is none
   (otherwise an engine error).

## Advance (grant `g`, previous boundary `p`)

For each engine *i* in plan order:

1. **Pre-advance delivery.** If an engine earlier in the plan order changed
   a net bound to an input of engine *i* during this grant (in its step 3),
   call `apply_input_trace(base, trace)` on engine *i* now, before it
   advances. `trace` holds every such change, in time order, keyed by *i*'s
   pin names; `base` is *i*'s full input snapshot at `p`. A digital change
   carries an `edge_time` with `p < edge_time <= g`. The consumer is still
   at `p` and **must apply each transition at `edge_time`**, its true time,
   or, if it can only observe inputs at its lattice points (lattice-bound,
   Renode), at `ceil_L(edge_time)`, the first lattice point at or after it,
   and **never earlier**. Several transitions of one pin inside one lattice
   cell are all applied, in order, at `ceil_L` (ADR-003 approver decision 2).
   An ADC push (an `Analog` entry of an `adc_in` pin) is at a lattice point
   `t` and is applied at `t` (Renode: `ADC SetValue` while paused there),
   never earlier.
   An engine whose inputs did not change in this grant gets no pre-advance
   call.
2. `advance_to(g)`.
3. **Collect** at `g`: `drain_logs()` (a record with `t > g` is an error; a
   record with `t < p` is clamped to `p`), then `read_output_trace()` and
   `read_outputs()`. For each output binding `(engine, pin) -> net` whose pin
   is present, check invariant 5, record its changes (trace entries, or the
   one legacy change) for the rest of this grant and set `net := signal`. An
   absent pin holds its previous value.

Then `now := g`. The initial exchange at t = 0 runs step 3 for every engine,
with no advance, no trace and no pre-advance delivery.

Because the plan puts every producer before its consumers when the graph is
acyclic, every change reaches every consumer inside the same grant, at its
exact time, whatever the registration order. Only inside a feedback loop
does a consumer come before a producer; it gets that producer's changes at
the boundary (Exchange step 2), which `loop_quantum` (one cell by default)
makes harmless: a lattice-bound consumer would apply them at `ceil_L(t) = g`
in any order (ADR-003 D3).

## Quiet grants (ADR-003 D5; `lookahead` on, the default)

Only while the binding graph is cyclic and the loop cap binds, i.e.
`floor_L(g') > now + loop_quantum` for `g'` the Grant-row minimum without
the `loop_quantum` term:

1. **Query.** Every lookahead candidate *c* that is not backed off is asked
   `q_c = quiet_until(now)`. Let `G = floor_L(min(g', q_c))` over the useful
   promises (`floor_L(q_c) > now`) and `Q = {c : floor_L(q_c) >= G}`. With
   no useful promise, the grant is the one-cell grant.
2. **Structure.** With the out-edges of every engine in `Q` removed, the
   remaining graph must be acyclic; exactly one engine `p` outside `Q` may
   drive a net read by an engine in `Q`; `p.can_stop_early()` must hold; and
   `p` must be first in the remaining graph's Kahn order (ties by
   registration index). Otherwise the grant is the one-cell grant of the
   static plan (`loop_quantum`, plan order).
3. **Early stop.** The grant's order is that Kahn order. Engine `p` runs
   `advance_until(G, watch)`, with `watch` its output pins whose nets an
   engine in `Q` reads, and returns `r`. The `grant` record `(now, r]` is
   emitted then, labelled `stop:<p>` when `r < G` and otherwise by `G`'s
   bound. Every later engine gets its pre-advance delivery as usual and
   `advance_to(r)`. Engines of `Q` thus receive a change of `p` at `t_x`
   before they advance and, lattice-bound, apply it at `ceil_L(t_x) = r`:
   exactly where a one-cell run applies it.
4. **Audit** (invariant 8) after each engine of `Q` is collected.
5. **Back-off.** Per candidate: after `k` consecutive Queries without a
   useful promise, the next `min(2^k, 64)` loop-capped grants (those where
   the Query would be asked) skip it; a useful promise resets `k` to 0. The
   back-off is a function of the simulated history only, and lookahead
   changes only the partition (`grant` records), never outputs.

## Exchange (at boundary `g`, previous boundary `p`)

1. Logs and outputs have already been collected for each engine during
   Advance (step 3).
2. For each engine in registration order: if a net bound to one of its
   inputs was changed during this grant by an engine at or after it in the
   grant's order (the plan order, or a quiet grant's order; only possible in
   a feedback loop), `apply_input_trace` with
   those changes, which then take effect at once, in order. Then
   `apply_inputs(snapshot)`, where `snapshot[pin] = net value` for every
   input binding whose net has a value. An engine must treat a snapshot or a
   transition it has already applied as a no-op.
3. If event sampling is configured: for every sample instant `s <= g` not yet
   emitted, emit `net_sample` for each configured net.
4. Emit the collected `log`, `pin_edge` and `net_sample` records, sorted by
   `(t_fs, registration index of the producing engine, pin)`: at equal
   times, an engine's logs come before its edges, edges are in output-binding
   order, and the kernel's `net_sample`s come last. Never by plan order
   (ADR-003 D3).
5. Call `Probe::on_window(p, g, nets, sampler)` for each probe, such as
   `CsvWriter` and `VcdWriter` (default: `on_boundary(p, g, nets)`). See
   "Sampled probes".

## Sampled probes (ADR-002 Amendment 3, ADR-003 D3)

`next_sample` in the Grant row comes only from probes that ask for aligned
boundaries (`CsvWriter::new`, `event_sampling`). Every extra boundary changes
the grant partition, and Renode's results depend on the partition (when a
timer ISR runs relative to a `RUN_FOR` end). Outputs that must not perturb the
simulation therefore use sampled probes (`CsvWriter::sampled` and
`VcdWriter::sampled`, used by `bb run`): they never constrain grants and read
each sample instant `s` in `[p, g]` through the sampler:

1. the driving engine's `sample_output(pin, s)` if it returns a value
   (ngspice: linear interpolation between the solver points of the window);
2. otherwise, for a digital net that changed in the window, the value of its
   last change at or before `s` (from the trace), or its value at `p`;
3. otherwise the net's value at `g`.

A sampled VCD writes analog values at the fixed sample instants (only when
the µV-rounded value changes) and every digital change of the window, from
the trace, at its `edge_time`. The sampler is a pure function of engine
state and net values, so sampled outputs are deterministic, and since the
sample instants and the traces do not depend on the partition, neither do
`nets.csv` and `nets.vcd`. With every `RUN_FOR` ending on the lattice (D2),
Renode's edge times do not depend on the partition, only on `L`.

Inputs applied at the boundary `g` govern the consumer for `(g, next g]`.
Changes delivered before an advance already took effect at their
`edge_time` (a lattice-bound consumer: `ceil_L(edge_time)`).

## Sequence example 1: MCU toggles PA5, LED net follows

Renode quantum (the lattice `L`, `--renode-quantum`) 10 µs. The Renode quantum
is not the grant: Renode reports a trace and returns `t_safe = Time::MAX`,
and here `max_quantum` = 10 µs, so grants are one lattice cell (the one-cell
reference). CSV sampling at 1 ms, t_end = 60 ms. A timer ISR sets PA5 high
at 49.995 ms; that is a timer-driven sync, not a lattice point, so Renode
stamps it exactly (lattice rule 2). ngspice is idle (`Time::MAX`). The plan
order is [renode, ngspice] because renode drives ngspice's input, whatever
the registration order.

```
kernel                renode (cpu)                ngspice (analog)
  | Init: plan [renode, ngspice]; exchange @0          |
  |-- read_outputs ------> PA5=Low,e=0                 |
  |-- read_outputs ---------------------------------> LED_A=0.0V
  |-- apply_inputs ---------------------------------> {PA5: Low,e=0}
  | Query @0                                           |
  |-- next_safe_time(0) -> MAX                         |
  |-- next_safe_time(0) ----------------------------> MAX
  | Grant g=10us (max_quantum)      emit grant 0->10us
  |-- advance_to(10us) -> RunFor(10us)                 |
  |-- read_output_trace -> []; read_outputs -> PA5 unchanged
  |-- advance_to(10us) -----------------------------> transient to 10us, pause
  | Exchange @10us ... (repeats every 10 us)           |
  ...
  | Grant g=50.000ms (max_quantum)                     |
  |-- advance_to(50ms) -> firmware writes ODR at 49.995ms
  |-- read_output_trace -> [PA5 High @49.995ms]; read_outputs -> PA5=High,e=49.995ms
  |   PA5: Low -> High, 49.995ms in (49.99ms, 50ms]  OK
  |   pin_edge{t_fs: 49.995ms, net: PA5, level: 1} collected
  |-- apply_input_trace (pre-advance) --------------> [PA5 High @49.995ms]
  |-- advance_to(50ms) -----------------------------> V_PA5 ramps 0->3.3V over
  |                                                   [49.995ms, 49.995ms+5ns]
  | Exchange @50ms                                     |
  |-- apply_inputs (same snapshot: no-op) ----------> {PA5: High,e=49.995ms}
  |   emit pin_edge; CsvWriter row @50ms: PA5=1, LED_A ~ 1.9V (same grant)
```

The CPU→analog edge skew is zero, because ngspice starts the ramp at the
edge's Renode timestamp. Registering ngspice first changes nothing: the plan
order still advances renode first.

## Sequence example 2: lattice-aligned 10 ms grants

The same board with `bb`'s defaults: `L` = 10 µs, `max_quantum` = 10 ms,
t_end = 500 ms. The binding graph is renode → ngspice (acyclic), so there is
no loop cap. Renode returns `Time::MAX` and ngspice its transient end, so
every grant is capped by `max_quantum`: 50 grants.

```
Query @0        renode t_safe = MAX, ngspice = 500ms
Grant           g = floor_L(min(MAX, 500ms, 0+10ms, 500ms)) = 10ms   limited_by=max_quantum
Advance         renode: RunFor(10ms), trace []; ngspice: transient to 10ms
...
Grant (40, 50]  renode: SysTick fires at sync 50ms (a lattice point and the
                grant end): the event is carried over (lattice rule 3)
Grant (50, 60]  renode: trace [PA5 High @50ms+1fs]; ngspice gets it before it
                advances and ramps at 50ms+1fs
...
Grant (290,300] renode: trace [PA5 Low @299.99993ms] (timer-driven sync, exact)
...
Grant (490,500] limited_by=engine:ngspice (its t_safe 500ms ties t_end; engines win)
Done @500ms     50 grants; nets.csv, nets.vcd and every pin_edge identical to the
                one-cell run of example 1 (the partition-equivalence gate)
```

A grant may hold any number of edges per pin: with `max_quantum` = 500 ms
the whole run is one grant whose trace holds all ten PA5 edges, with the same
outputs.

## Sequence example 3: a feedback loop (one-cell lockstep, then quiet grants)

U1 PA5 drives an RC; the RC node drives U1.PB0 back (`digital_in`, ADR-003
"Feedback-path correctness"). The binding graph has the cycle U1 → ngspice →
U1, so the plan order is [U1, ngspice] (lattice-bound first) and, with
lookahead off, every grant is capped at `loop_quantum` = `L` = 10 µs,
whatever `max_quantum` is. This is `examples/feedback` (`make feedback-ref`,
the one-cell reference); the times below are its measured
`pin_edge` records. ngspice thresholds SENSE with the `digital_in` hysteresis
at every accepted solver point and reports each level change in its trace,
at the crossing interpolated between the two straddling points
(`spec/pin-model.md`).

```
Grant (50.95, 50.96] limited_by=loop_quantum
  U1  advance_to(50.96)   RunFor(10us); no edge
  ng  advance_to(50.96)   SENSE crosses vih between two solver points
                          -> trace U1.PB0: High @ t_x = 50.954859.. ms
  Exchange  U1 comes before ngspice: apply_input_trace([PB0 High @ t_x]) at the
            boundary -> ceil_L(t_x) = 50.96 ms = now -> SET_STATE PB0=1 while paused
Grant (50.96, 50.97]
  U1  RunFor: the EXTI0 ISR writes PA6 in the slice after 50.96 ms
      -> trace [PA6 High @ 50.96 ms + 1 fs] (lattice rule 1)
  ng  pre-advance: PA6 High @ 50.96 ms + 1 fs -> ramp at its true time
```

The MCU sees the crossing at `ceil_L(t_x)`: never early, and at most one cell
late, exactly as in any one-cell run.

With lookahead on (the default, `make feedback`), U1 is the one lookahead
candidate (ngspice's producer U1 cannot stop early, so ngspice is not
one). While the firmware sleeps in `wfi` with nothing pending, U1 promises
to stay quiet until its next SysTick, and ngspice, the only producer
feeding it, runs first with an early stop on U1.PB0. These are the
measured `grant` records of the default run (500 ms in 117 grants instead
of 50 000; every other record, `nets.csv` and `nets.vcd` are those of the
one-cell run byte for byte):

```
Query @50.02      U1 in WFI, nothing pending -> quiet_until just below 100 ms
                  (its next SysTick limit; floor_L gives 99.99 ms)
                  G = floor_L(min(..., 50.02 + 10 ms)) = 60.02 ms; Q = {U1}
Grant (50.02, 50.96] limited_by=stop:ngspice
  ng  advance_until(60.02, watch {U1.PB0}): SENSE crosses vih at
      t_x = 50.954859.. ms -> stops at r = ceil_L(t_x) = 50.96 ms
  U1  pre-advance: PB0 High @ t_x; advance_to(50.96) -> SET_STATE PB0=1 at
      50.96 ms (the lattice point a one-cell run uses); audit: no output, no wake
Query @50.96      EXTI0 pending -> no promise -> one cell in plan order [U1, ng]
Grant (50.96, 50.97] limited_by=loop_quantum
  U1  the EXTI0 ISR: trace [PA6 High @ 50.96 ms + 1 fs], as in the one-cell run
...               (the back-off skips the next two Queries)
Grant (50.99, 60.99]  limited_by=max_quantum      quiet; ng stops nowhere
...               (three more)
Grant (90.99, 99.99]  limited_by=engine:U1        the promise, floored to L
Grant (99.99, 100.00] limited_by=loop_quantum     SysTick fires at sync 100 ms
Grant (100.00, 100.01]                            PA5 Low @ 100 ms + 1 fs
...
Grant (100.02, 101.46] limited_by=stop:ngspice    SENSE falls through vil at
                                                  101.452500.. ms
```

Opening the loop (`examples/feedback-open`: SENSE driven by a circuit source,
PA6 not loaded) leaves only ngspice → U1. The plan is [ngspice, U1] and grants
are `max_quantum` long: ngspice reports `PB0 High @ t_x` in the trace of the
grant holding it, U1 receives it before it advances and splits its `RunFor`
at `ceil_L(t_x)`, so the outputs equal the one-cell run's byte for byte.

## What engines MUST NOT do

- Advance past `g`, or advance on their own between calls (no free-run). For
  Renode this means only `RunFor` calls that each end on a lattice point at
  or before `g`, never `start`, and always with
  `emulation SetGlobalAdvanceImmediately true` (ADR-003 D1.1, D2).
- Return `t_safe <= now`.
- Apply an input before its `edge_time` (a lattice-bound engine: before
  `ceil_L(edge_time)`; an ADC push: before its `t`), apply a pre-advance
  input later than that, drop an intermediate transition of a trace, or
  treat a repeated identical snapshot as a new edge.
- Report a digital `edge_time` outside `(p, g]`, hide a change from the trace
  (a traced engine), or hide a second edge inside a grant (a legacy engine).
- Step ngspice per CPU instruction.
- Depend on wall-clock time, thread timing or unseeded randomness.
- Change anything that affects results in `end_of_run`: it is for pure
  self-checks (e.g. the Renode cross-check), and a later `run` may resume.
- Change state in `quiet_until` (it may be skipped by the back-off), promise
  quiet outputs it cannot guarantee, or stop `advance_until` anywhere but the
  first lattice point at or after the first watched change (or `t`).
