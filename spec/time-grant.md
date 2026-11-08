# Time-grant protocol

Status: normative. Rationale: `docs/adr/002-time-grant-protocol.md`
(Advance ordering: Amendment 2, Gauss-Seidel intra-grant delivery;
`t_safe` wording and sampled probes: Amendment 3).
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
  exact `edge_time` and at most one change per digital pin (invariant 5).
  It does *not* promise that outputs are constant inside the grant (a CPU
  can toggle a pin at any instruction). Engines registered after the
  producer apply the change at `edge_time` (see "Advance"); engines
  registered before it see it one grant late (Jacobi). It must satisfy
  `t_safe_i > now`. `Time::MAX` means idle. (ADR-002 Amendment 3.)
- **grant g:** the time every engine advances to in one iteration.
- **boundary:** the instant `g` at which pin signals are exchanged.
- **max_quantum:** configured upper bound on `g - now`, which must be greater than 0.
- **t_end:** end of the run.

## State machine

```
            +------+
            | Init |  bind engines and nets; initial Exchange at t = 0
            +------+
               |
               v
   +-------> Query ---- t_safe_i <= now -------------------------+
   |           |                                                   |
   |           v                                                   |
   |         Grant     g = min(t_safe_i, next_sample, now+max_quantum, t_end)
   |           |       emit {"type":"grant", t_fs: now, until_fs: g}  |
   |           v                                                   |
   |        Advance    per engine i, in registration order:  ------+-- engine error
   |           |         [apply_inputs if an input net changed      |   / causality
   |           |          earlier in this grant]; advance_to(g);    |
   |           |         drain_logs; read_outputs -> nets           |
   |           |       now := g                                    |
   |           v                                                   |
   |        Exchange   inputs (snapshot); samples; events; probes -+-- engine / I/O
   |           |                                                   |
   +-- now < t_end                                                 v
               |                                                +-------+
           now == t_end                                         | Error | (terminal)
               v                                                +-------+
            +------+
            | Done |  run(t_end') with t_end' > now resumes at Query
            +------+
```

| State | Entry action | Exit to | Condition |
| --- | --- | --- | --- |
| Init | none | Exchange (initial) | first `run()`, at least 1 engine |
| Query | `t_safe_i = next_safe_time(now)` for every *i* | Grant | every `t_safe_i > now` |
| Query | | Error | any `t_safe_i <= now` (`NoProgress`) |
| Grant | compute `g`, emit `grant` | Advance | always |
| Advance | for every *i* in order: pre-advance delivery, `advance_to(g)`, collect (see "Advance"); `now := g` | Exchange | every call returned Ok |
| Advance | | Error | any engine error or causality violation |
| Exchange | see "Exchange" below | Query | `now < t_end` |
| Exchange | | Done | `now == t_end` |
| Exchange | | Error | causality violation, engine error or I/O error |
| Done | none | Query | `run(t_end')` with `t_end' > now` |
| Error | none | none | every later call returns `Halted` |

## Invariants (MUST)

1. **No free-run:** an engine's internal time never exceeds the latest grant.
   `advance_to(t)` is called only with `now < t <= t_safe` (the engine's most
   recent promise), and the engine stops at exactly `t`.
2. **Grant = min:** `g` is exactly the minimum in the Grant row. When several
   bounds tie, `limited_by` names the first in this order: engines in
   registration order, then `sample`, then `max_quantum`, then `t_end`.
3. **Lockstep:** every engine is advanced to the same `g` in every iteration.
   Engines advance one after another in registration order (Gauss-Seidel),
   never concurrently.
4. **Progress:** `g > now`, which holds because every term is greater than `now`.
5. **Digital transitions:** at boundary `g`, after previous boundary `p`:
   - level changed: `p < edge_time <= g`, and `pin_edge` is emitted at `edge_time`;
   - level unchanged: `edge_time` is unchanged. A moved `edge_time` means
     more than one edge fell inside one grant, and is an error.
   - `edge_time > g` is always an error.
6. **Determinism:** the order of every call and every emitted record is a pure
   function of the inputs. See ADR-002 D6.

## Advance (grant `g`, previous boundary `p`)

For each engine *i* in registration order:

1. **Pre-advance delivery.** If an engine earlier in the order changed a net
   bound to an input of engine *i* during this grant (in its step 3), call
   `apply_inputs(snapshot)` on engine *i* now, before it advances. The
   snapshot is the same full snapshot as in Exchange step 2. A digital signal
   in it may carry an `edge_time` with `p < edge_time <= g`. The consumer is
   still at `p` and **must apply the transition at `edge_time`**, its true
   time. An engine whose inputs did not change in this grant gets no
   pre-advance call.
2. `advance_to(g)`.
3. **Collect** at `g`: `drain_logs()` (a record with `t > g` is an error; a
   record with `t < p` is clamped to `p`), then `read_outputs()`. For each
   output binding `(engine, pin) -> net` whose pin is present, check
   invariant 5 and set `net := signal`. An absent pin holds its previous
   value. A net whose value differs from before is marked changed for the
   rest of this grant.

Then `now := g`. The initial exchange at t = 0 runs step 3 for every engine,
with no advance and no pre-advance delivery.

As a result, an edge produced by engine *j* reaches every engine registered
after *j* inside the same grant, at its exact time. Engines registered before
*j* only get it at the boundary (Exchange step 2), one grant late (Jacobi),
and they start the transition at `g`. So register producers before consumers.
In Phase 0 the CPU engine (Renode) is registered before the analog engine
(ngspice).

## Exchange (at boundary `g`, previous boundary `p`)

1. Logs and outputs have already been collected for each engine during
   Advance (step 3).
2. For each engine in registration order: `apply_inputs(snapshot)`, where
   `snapshot[pin] = net value` for every input binding whose net has a value.
   An engine must treat a snapshot it has already applied as a no-op: an
   engine later in the order sees the same edge in its pre-advance delivery
   and again here.
3. If event sampling is configured: for every sample instant `s <= g` not yet
   emitted, emit `net_sample` for each configured net.
4. Emit the collected `log`, `pin_edge` and `net_sample` records, stably sorted
   by `t_fs`.
5. Call `Probe::on_window(p, g, nets, sampler)` for each probe, such as
   `CsvWriter` and `VcdWriter` (default: `on_boundary(p, g, nets)`). See
   "Sampled probes".

## Sampled probes (ADR-002 Amendment 3)

`next_sample` in the Grant row comes only from probes that ask for aligned
boundaries (`CsvWriter::new`, `event_sampling`). Every extra boundary changes
the grant partition, and Renode's results depend on the partition (when a
timer ISR runs relative to a `RUN_FOR` end). Outputs that must not perturb the
simulation therefore use sampled probes (`CsvWriter::sampled`, used by
`bb run`): they never constrain grants and read each sample instant
`s` in `[p, g]` through the sampler:

1. the driving engine's `sample_output(pin, s)` if it returns a value
   (ngspice: linear interpolation between the solver points of the window);
2. otherwise the net's value at `g`, except that a digital net whose
   `edge_time > s` reads its value at `p`.

The sampler is a pure function of engine state and net values, so sampled
outputs are deterministic. Edge times are exact for the partition the run
uses; a different quantum may change them by tens of ns.

Inputs applied at the boundary `g` govern the consumer for `(g, next g]`. For
an edge the consumer has not seen yet (one from an engine registered after
it), the transition starts at `g`, because the consumer has already simulated
up to `g`. Edges from earlier engines were delivered before the consumer
advanced, and they already took effect at their `edge_time`.

## Sequence example 1: MCU toggles PA5, LED net follows

Renode quantum 10 µs, `max_quantum` = 1 ms, CSV sampling at 1 ms, t_end = 60 ms.
Firmware sets PA5 high at 49.995 ms. ngspice is idle (`Time::MAX`). Renode is
registered first and ngspice second.

```
kernel                renode (cpu)                ngspice (analog)
  | Init: exchange @0                                  |
  |-- read_outputs ------> PA5=Low,e=0                 |
  |-- read_outputs ---------------------------------> LED_A=0.0V
  |-- apply_inputs ---------------------------------> {PA5: Low,e=0}
  | Query @0                                           |
  |-- next_safe_time(0) -> 10us                        |
  |-- next_safe_time(0) ----------------------------> MAX
  | Grant g=10us (engine:renode)     emit grant 0->10us
  |-- advance_to(10us) -> RunFor(10us)                 |
  |-- read_outputs ------> PA5 unchanged               |
  |-- advance_to(10us) -----------------------------> transient to 10us, pause
  | Exchange @10us ... (repeats every 10 us)           |
  ...
  | Grant g=50.000ms (engine:renode)                   |
  |-- advance_to(50ms) -> firmware writes ODR at 49.995ms
  |-- read_outputs ------> PA5=High,e=49.995ms         |
  |   PA5: Low -> High, 49.995ms in (49.99ms, 50ms]  OK; PA5 marked changed
  |   pin_edge{t_fs: 49.995ms, net: PA5, level: 1} collected
  |-- apply_inputs (pre-advance) -------------------> {PA5: High,e=49.995ms}
  |-- advance_to(50ms) -----------------------------> V_PA5 ramps 0->3.3V over
  |                                                   [49.995ms, 49.995ms+5ns]
  | Exchange @50ms                                     |
  |-- apply_inputs (same snapshot: no-op) ----------> {PA5: High,e=49.995ms}
  |   emit pin_edge; CsvWriter row @50ms: PA5=1, LED_A ~ 1.9V (same grant)
```

The CPU→analog edge skew is zero, because ngspice starts the ramp at the
edge's Renode timestamp. If the registration order were reversed, ngspice
would see the edge only at the 50 ms boundary and start the ramp there. That
is a 5 µs skew, below one quantum and still deterministic.

## Sequence example 2: idle MCU gets large grants

The firmware is in `WFI` and its next SysTick is 50 ms away. The Renode adapter
knows this from timer lookahead. `max_quantum` = 20 ms, no probes, ngspice idle,
t_end = 100 ms.

```
Query @0        renode t_safe = 50ms, ngspice = MAX
Grant           g = min(50ms, MAX, 0+20ms, 100ms) = 20ms   limited_by=max_quantum
Advance         RunFor(20ms) ; ngspice transient to 20ms
Exchange @20ms  nothing changed; no records except the grant
Query @20ms     renode t_safe = 50ms
Grant           g = min(50ms, 40ms) = 40ms                 limited_by=max_quantum
Query @40ms     renode t_safe = 50ms
Grant           g = 50ms                                   limited_by=engine:renode
Exchange @50ms  SysTick ISR ran at 50ms: PA5 edge e=50ms (zero skew)
Query @50ms     renode t_safe = 100ms
Grant           g = 70ms, 90ms (max_quantum), then 100ms (engine:renode = t_end; engine wins the tie)
Done @100ms     6 grants for 100 ms of idle-ish sim
```

Without lookahead, a 10 µs quantum would need 10 000 grants for the same run.
Either way the result is identical; only the cost differs.

## What engines MUST NOT do

- Advance past `g`, or advance on their own between calls (no free-run). For
  Renode this means only `RunFor(g - now)`, never `start`.
- Return `t_safe <= now`.
- Apply a pre-advance input at any time other than its `edge_time`, or
  treat a repeated identical snapshot as a new edge.
- Report a digital `edge_time` outside `(p, g]`, or hide a second edge inside a grant.
- Step ngspice per CPU instruction.
- Depend on wall-clock time, thread timing or unseeded randomness.
