# Time-grant protocol

Status: normative. Rationale: `docs/adr/002-time-grant-protocol.md`
(Advance ordering: Amendment 2, Gauss-Seidel intra-grant delivery;
`t_safe` wording and sampled probes: Amendment 3). The grant lattice,
`edge_time` at the producer's resolution and the never-early input rule are
`docs/adr/003-cosim-speedup.md` D2 (plan step 2, Amendment 2); the end-of-run
hook (`end_of_run`) is ADR-003 Amendment 1; traces, the plan order, the loop
cap, the record order and sampled VCDs are ADR-003 D3/D4 (plan step 3,
Amendment 3); `digital_in` thresholding, which produces the analog → digital
traces of example 3, is ADR-003 D3 (plan step 4, Amendment 4).
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
  Consumers receive the changes on their inputs through
  `apply_input_trace(base, trace)`; its default applies `apply_inputs` once
  per entry, in order, with the snapshot as of that entry.
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
  lattice needs an explicit value.
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
   |           v                                                   |
   |         Grant     g = floor_L(min(t_safe_i, next_sample,      |
   |           |             now+loop_quantum [cyclic], now+max_quantum, t_end))
   |           |       (floor_L(..) <= now: Lattice error)            |
   |           |       emit {"type":"grant", t_fs: now, until_fs: g}  |
   |           v                                                   |
   |        Advance    per engine i, in plan order:  --------------+-- engine error
   |           |         [apply_input_trace if an input net changed  |   / causality
   |           |          earlier in this grant]; advance_to(g);    |
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
| Query | `t_safe_i = next_safe_time(now)` for every *i* | Grant | every `t_safe_i > now` |
| Query | | Error | any `t_safe_i <= now` (`NoProgress`) |
| Grant | compute `g`, emit `grant` | Advance | `g > now` |
| Grant | | Error | `floor_L(min(..)) <= now` (`Lattice`) |
| Advance | for every *i* in plan order: pre-advance delivery, `advance_to(g)`, collect (see "Advance"); `now := g` | Exchange | every call returned Ok |
| Advance | | Error | any engine error or causality violation |
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
   not a free-run.
2. **Grant = min:** `g` is exactly the minimum in the Grant row, rounded
   down to the lattice after the min. The `loop_quantum` term is present only
   while the binding graph is cyclic. When several bounds tie, `limited_by`
   names the first in this order: engines in registration order, then
   `sample`, then `loop_quantum`, then `max_quantum`, then `t_end`. Rounding
   never changes the label.
3. **Lockstep:** every engine is advanced to the same `g` in every iteration.
   Engines advance one after another in plan order (ADR-003 D3), never
   concurrently.
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

## Exchange (at boundary `g`, previous boundary `p`)

1. Logs and outputs have already been collected for each engine during
   Advance (step 3).
2. For each engine in registration order: if a net bound to one of its
   inputs was changed during this grant by an engine at or after it in the
   plan order (only possible in a feedback loop), `apply_input_trace` with
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

## Sequence example 3: a feedback loop (one-cell lockstep)

U1 PA5 drives an RC; the RC node drives U1.PB0 back (`digital_in`, ADR-003
"Feedback-path correctness"). The binding graph has the cycle U1 → ngspice →
U1, so the plan order is [U1, ngspice] (lattice-bound first) and every grant
is capped at `loop_quantum` = `L` = 10 µs, whatever `max_quantum` is. This is
`examples/feedback` (`make feedback`); the times below are its measured
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
  `ceil_L(edge_time)`), apply a pre-advance input later than that, drop an
  intermediate transition of a trace, or treat a repeated identical snapshot
  as a new edge.
- Report a digital `edge_time` outside `(p, g]`, hide a change from the trace
  (a traced engine), or hide a second edge inside a grant (a legacy engine).
- Step ngspice per CPU instruction.
- Depend on wall-clock time, thread timing or unseeded randomness.
- Change anything that affects results in `end_of_run`: it is for pure
  self-checks (e.g. the Renode cross-check), and a later `run` may resume.
