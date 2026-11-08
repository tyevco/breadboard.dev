# ADR-002: Time-grant protocol (conservative lockstep with lookahead)

- **Status:** Accepted. Approved via the Phase 0 plan on 2026-11-07.
  Sub-decision D5 (ngspice pause mechanism) is **open**. The ngspice engine
  agent closes it in an "Amendment" section at the end of this ADR.
- **Normative spec:** `spec/time-grant.md`. If this ADR and the spec disagree,
  the spec wins, and the disagreement is a bug to fix through a follow-up ADR.
- **Implementation:** `bbcore/src/{time,engine,scheduler}.rs`.

## Context

The project's top risk is that "co-sim sync is slow or unstable" (design doc,
"Co-simulation scheme"). The hard part is coupling a variable-step analog solver
(ngspice) with an instruction-quantum CPU emulator (Renode) so that:

- neither engine ever simulates past a point where the other could have
  influenced it (**no engine free-runs**);
- runs are **deterministic**: same inputs and seed give byte-identical
  `events.jsonl` and `nets.csv`;
- an idle MCU costs almost nothing, and analog is never re-stepped per
  instruction.

The scheme follows SystemC-TLM's quantum keeper and HLA conservative time
management: the kernel owns time, engines promise lookahead, and the kernel
grants the minimum.

## Decision

### D1. Time is an integer number of femtoseconds

`bbcore::Time(u64)` counts femtoseconds. The range is about 18 446 s of simulated
time, more than any Phase 0–3 run needs. 1 fs resolves every edge we model (the
fastest is a 5 ns ramp) with six orders of magnitude to spare.

Floats never take part in a scheduling decision: lookahead, grant, cap,
comparison and sample alignment are all integer operations. An engine whose
native clock is a float, such as ngspice's `double` seconds, converts at its own
boundary. The kernel's view of time stays integer.

### D2. Engine trait

```rust
pub trait Engine {
    fn name(&self) -> &str;
    fn next_safe_time(&mut self, now: Time) -> Time;          // t_safe > now; Time::MAX = idle
    fn advance_to(&mut self, t: Time) -> Result<(), EngineError>; // exactly to t
    fn read_outputs(&mut self) -> Result<PinSignals, EngineError>;
    fn apply_inputs(&mut self, inputs: &PinSignals) -> Result<(), EngineError>;
    fn drain_logs(&mut self) -> Vec<LogRecord> { Vec::new() }
}
```

`t_safe` is a promise. The engine's outputs will not change in `(now, t_safe)`
except as the engine will report at the boundary, and the engine can be advanced
to any `t <= t_safe` without the kernel missing an interaction. The rules:

- **CPU engine (Renode):** `t_safe = now + quantum`, with a default quantum of
  10 µs that can be configured. When the adapter can learn the next peripheral
  timer expiry or GPIO event from the External Control API, it may promise more,
  but never past an event it cannot rule out. Renode is advanced **only** with
  `RunFor(g - now)`, so its virtual time can never get ahead of the grant.
- **Analog engine (ngspice):** `t_safe = Time::MAX` when the circuit has no
  internal events the kernel must observe; otherwise its next breakpoint.
  ngspice is advanced one grant at a time, **never per CPU instruction**.
- **Idle engines** return `Time::MAX`. The grant is then bounded by the other
  engines, by `max_quantum` and by `t_end`.

### D3. Scheduler loop

```
Init:     initial exchange at t = 0
loop while now < t_end:
  Query:    t_safe_i = engine_i.next_safe_time(now)   (error if t_safe_i <= now)
  Grant:    g = min( min_i t_safe_i,
                     next probe / sample time > now,
                     now + max_quantum,
                     t_end )
  Advance:  engine_i.advance_to(g) for every i, in registration order
  Exchange: read outputs -> route by net -> apply inputs -> emit events -> probes
  now = g
Done
```

- `max_quantum` must be greater than 0 and can be configured. It bounds how
  stale an idle engine's view of the world can get, and how coarse an
  idle-system run is.
- **Sample alignment:** fixed-rate outputs (`CsvWriter`, `net_sample` events)
  register their next sample time, and grants are capped at it. Every sample is
  therefore taken at an exchange boundary, not interpolated from solver points.
- **Tie-break order for `limited_by`:** engines in registration order, then
  `sample`, then `max_quantum`, then `t_end`. It only affects the label on the
  grant record, never `g` itself.
- Any error moves the scheduler to `Error`. It then refuses further work.

### D4. Boundary exchange semantics

At boundary `g`:

1. For every engine in registration order: drain logs, then `read_outputs()`.
   Each bound output pin's `PinSignal` becomes the value of its net. A net has
   exactly one driver, which is checked at bind time. A pin the engine omits
   keeps its previous value.
2. **Digital transition contract.** When a digital net's level changes at
   boundary `g`, the reported `edge_time` must satisfy `prev < edge_time <= g`.
   While the level is unchanged, `edge_time` must not move. A moved `edge_time`
   with an unchanged level means two or more edges fell inside one grant, which
   means the engine's `t_safe` was too optimistic. Each of these cases is a
   `Causality` error, never silently accepted. The kernel emits a `pin_edge`
   record at `edge_time`.
3. For every engine: `apply_inputs()` receives the **full snapshot** of its bound,
   driven input pins, keyed by the engine's own pin names.
4. Inputs applied at `g` take effect for the interval `(g, next grant]`. A
   consuming engine has already simulated up to `g`, so it **starts the
   transition at `g`** (the boundary where the edge was delivered). It uses
   `edge_time` only for reporting. The CPU→analog edge skew is therefore
   `g - edge_time`, which is less than the producer's grant length. It is zero
   when the producer's lookahead lands the grant exactly on its edge, and at
   most one Renode quantum (10 µs by default) otherwise. The skew is
   deterministic, because it depends only on integer grant arithmetic.
5. The kernel routes `PinSignal`s unchanged. Domain conversion happens in the
   consuming engine using the pin model of the pin involved
   (`spec/pin-model.md`). For example, the ngspice adapter turns a
   `digital_out{voh, vol, rise, fall, zout}` edge into a PWL ramp on an
   `EXTERNAL` voltage source plus a series `zout`.

### D5. ngspice pause mechanism (OPEN sub-decision)

ngspice has to integrate a transient analysis up to exactly `g`, stop there with
its state intact, report node voltages, accept new `EXTERNAL` source values, and
continue. These facts are verified against the installed
`/usr/include/ngspice/sharedspice.h` (ngspice 42):

- `ngSpice_Init(...)`, `ngSpice_Init_Sync(GetVSRCData*, GetISRCData*, GetSyncData*, int*, void*)`
- `GetVSRCData(double* value, double time, char* node, int id, void* user)` asks
  for the value of an `EXTERNAL` source
- `GetSyncData(double time, double* delta, double olddelta, int redostep, int id, int loc, void* user)`
- `ngSpice_SetBkpt(double time)`, `ngSpice_running()`, `ngSpice_Command(char*)`,
  plus a `BGThreadRunning` callback

The candidates:

| # | Mechanism | Notes |
| --- | --- | --- |
| A | Background thread: `ngSpice_Command("bg_run")`, `ngSpice_SetBkpt(g)` at each grant; halt when the solver reaches `g` (`bg_halt` from the data/sync callback), read the vectors, then `bg_resume` | The usual shared-library pattern. Needs care to halt *exactly* at the breakpoint and not one step later. |
| B | Foreground: `stop when time >= g` (or `stop after`), then `resume` | Single-threaded, so it is simpler to keep deterministic. Breakpoint precision depends on `stop when` semantics. |
| C | `GetSyncData` lock-step: the callback clamps `*delta` so that steps land on `g`, then blocks the ngspice thread on a condvar until the next grant | Most precise and needs no halt race, but it relies on more niche API behaviour. |

The `bg_*` and `stop`/`resume` command semantics come from the ngspice manual,
and the ngspice engine agent **must check them against the manual** (the
shared-library chapter) before choosing. That agent records the choice,
evidence and test results in an "Amendment" section of this ADR.

Constraints on any choice:

- ngspice is never stepped per CPU instruction.
- The transient never runs past `g`. If the mechanism overshoots internally, the
  overshoot must not be observable: no output and no state leaks past `g`.
- The adapter converts `g` to seconds only at the FFI call.
- The `TMAX` and `.options` used are fixed by the netlist, so the step sequence
  is a pure function of the inputs.

### D6. How determinism is guaranteed

1. **Integer time:** every grant is computed with `u64` arithmetic (D1).
2. **Fixed order:** engines are queried, advanced and exchanged in registration
   order. Nets, pins and signals live in `BTreeMap`s. The events of one
   exchange are emitted after a stable sort by `t_fs`, so ties keep
   engine/pin order.
3. **Fixed serialization:** event records are serde struct variants. Key order
   is declaration order, and there are no floats: voltages are integer µV and
   times are integer fs. The CSV prints volts through integer µV. The VCD
   header has no `$date`.
4. **Sample alignment:** waveform samples sit at sim-time multiples of a fixed
   period, which are exchange boundaries. They do not depend on solver points
   or wall-clock time.
5. **Engines are deterministic:** each adapter must produce identical results
   for identical call sequences. Renode is run headless, and ngspice runs with
   a fixed netlist and options. Any engine-side randomness must take the run
   seed.
6. **Checked:** a unit test runs the mock-engine scenario twice and asserts
   byte-identical events, CSV and VCD. `make determinism` (Phase 0
   integration) does the same over 10 real runs.

## Alternatives considered

- **Optimistic (Time Warp) with rollback:** ngspice and Renode cannot cheaply
  snapshot and roll back. Rejected.
- **Fixed-step lockstep (no lookahead):** simple, but an idle MCU would cost as
  much as a busy one, against the design doc's goal. It survives only as the
  `max_quantum` cap.
- **Gauss-Seidel ordering** (advance the CPU first, then deliver its edges to
  analog inside the same grant): this removes the CPU→analog skew in D4.4. It is
  deferred because it changes the phase order and the trait contract, so it
  would need its own ADR.
- **Float seconds for time:** rejected because of rounding nondeterminism and
  unstable comparisons.

## Consequences

- Correctness rests on honest `t_safe` promises. Optimistic promises are caught
  as `Causality` errors for digital outputs; analog outputs cannot be checked
  that way.
- The CPU→analog edge skew is bounded and deterministic but not zero
  (D4.4). The Phase 0 gate (edges spaced 50 ms ± 0.1 ms) tolerates it.
- Wall-clock cost scales with the number of grants. Each engine's lookahead
  quality is the lever for performance.

## Amendments

_None yet. The ngspice engine agent appends "Amendment 1: D5 resolution" here._
