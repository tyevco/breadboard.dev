# ADR-002: Time-grant protocol (conservative lockstep with lookahead)

- **Status:** Accepted. Approved via the Phase 0 plan on 2026-11-07.
  Sub-decision D5 (ngspice pause mechanism) was resolved by Amendment 1 at
  the end of this ADR: `GetSyncData` lock-step. Amendment 2 changes the
  Advance phase to Gauss-Seidel order (D3, D4.4): an edge from an earlier
  engine reaches later engines inside the same grant, at its true time.
  Amendment 3 (Phase 0 verification) makes sampling independent of the
  grant partition, fixes ngspice grant detection past ~2.3 s, and corrects
  the `t_safe` wording (D2).
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
time, more than any Phase 0–3 run needs. (Engines must not assume an f64 of
seconds round-trips to the same fs: above ~2.3 s it does not; see Amendment 3.) 1 fs resolves every edge we model (the
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
to any `t <= t_safe` without the kernel missing an interaction. (Corrected by
Amendment 3: outputs *may* change inside the grant; every change is reported
at the boundary with its exact `edge_time`, at most one per digital pin, and
only consumers registered after the producer apply it at that time.) The rules:

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
  (Amendment 3: this changes the partition and with it Renode's results;
  `bb run` now uses `CsvWriter::sampled`, which does not align grants.)
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
4. *(Superseded for engines registered after the producer by Amendment 2;
   still the rule for engines registered before it.)*
   Inputs applied at `g` take effect for the interval `(g, next grant]`. A
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

### D5. ngspice pause mechanism (resolved: candidate C, see Amendment 1)

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
   or wall-clock time. (Amendment 3: sampled probes interpolate inside the
   window instead; still a pure function of the solver points.)
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
  would need its own ADR. *(Adopted later by Amendment 2.)*
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

### Amendment 1: D5 resolution (2026-11-07)

**Status:** Accepted. Implemented in `engines/analog-ngspice` (`bb-analog-ngspice`).

**Decision: candidate C, `GetSyncData` lock-step**, running on ngspice's own
background thread (`bg_run`). `bg_halt` is used only for teardown, never to
pause at a grant.

**Evidence.** It was checked against the ngspice 42 sources (the
`ngspice-42.tar.gz` release, which is the version installed as `42+ds-3build1`)
and against the installed `/usr/include/ngspice/sharedspice.h`:

- `src/spicelib/analysis/dctran.c` calls `sharedsync(..., loc = 0)` once per
  accepted time point. That happens after `CKTaccept`, after `CKTdump` has
  appended the point to the output vectors, and after ngspice has done its own
  breakpoint/delta forcing for the next step, but before the next step is
  computed. `sharedspice.c:sharedsync` hands the caller `&ckt->CKTdelta`
  through `GetSyncData` and then only caps it at the final time. A callback
  can therefore cut the next step so it lands exactly on `g`, and when the
  accepted point *is* `g`, it can block on a condvar while the full solver
  state at `g` sits intact and already dumped.
- `loc = 1` is the accept/reject site. On a rejected step (`redostep = 1`),
  `CKTtime` is first rolled back to the last accepted point. The adapter only
  clamps there and always returns 0, so it never forces a redo.
- Candidate A (`bg_halt` to pause): `_thread_stop` sets `ft_intrpt` and polls
  every 10 ms. The run stops at whichever accepted point `IFpauseTest` sees
  first, so how far it gets depends on wall-clock timing. That is
  nondeterministic, so it was rejected for pausing. It is kept for teardown,
  where overshoot does not matter.
- Candidate B (`stop when` + `resume`): a foreground `run` blocks the calling
  thread, and `stop` conditions are checked after the step that crosses them,
  so precision depends on the step sequence. It also re-enters the front end
  on every grant. Rejected.
- `ngSpice_SetBkpt` adds to `CKTbreaks` (or to a holding list before the
  transient starts). It does not affect the step that was already chosen.
  This is why the step is clamped inside the callback, and why breakpoints are
  set from the solver thread only, while it is between steps.

**Mechanism** (`engines/analog-ngspice/src/lib.rs`):

1. `new()` loads the netlist (`ngSpice_Circ`). The engine appends the input
   glue, `.tran TMAX (t_stop + TMAX) 0 TMAX` with every time written as exact
   integer fs (`<n>e-15`), and `.end`. It then issues `bg_run`. The solver
   parks in the callback at the first accepted point, t = 0, which is the DC
   operating point.
2. `advance_to(g)` publishes `g`, which bumps a grant sequence number, then
   waits until the solver parks at `g`. A point is "at `g`" when
   `round(time * 1e15) >= g_fs`. Landing past `g` is reported as an error;
   clamping makes that impossible in practice.
3. While parked, the solver thread copies the new samples of `time` and of
   every watched node vector (`ngSpice_CurPlot` + `ngGet_Vec_Info("<plot>.<node>")`).
   The kernel thread then reads them from the shared state. ngspice is never
   called from two threads at once.
4. Inputs (`apply_inputs` at `g`, ADR D4.4) add a linear ramp to that
   source's piecewise-linear waveform, starting at `g`. `GetVSRCData`
   evaluates the waveform at the solver's trial time. Ramp corners (`g + rise`)
   become clamp points and `ngSpice_SetBkpt` breakpoints, so the solver
   resolves the edge.
5. Teardown (`Drop`): set `abort` (callbacks stop blocking), `bg_halt`,
   `remcirc`, `destroy all`. ngspice is a process singleton: one engine at a
   time, enforced at runtime, and tests serialize on a mutex.

**Why it is deterministic.** The solver thread only moves while the kernel
thread is blocked waiting for it, and every value it reads (source voltages,
the step limit) is a pure function of the netlist, `TMAX` and the integer-time
call sequence. Thread scheduling can change *when* things happen, but never
*what* is computed. ngspice is advanced once per grant, never per instruction.

**`t_safe`.** `min(t_stop, now + lookahead, next kernel-visible breakpoint)`.
`lookahead` defaults to none, which per D2 means no internal events. The
analysis runs one `TMAX` past `t_stop`, because ngspice ends the analysis on
its final point without calling the sync callback, so a grant can end exactly
at `t_stop`.

**Sampling.** Grants are aligned to sample times (D3), so the CSV value at a
boundary is the solver point at exactly `g`. `NgspiceEngine::sample(node, t)`
also linearly interpolates the solver points retained since the previous
boundary. That is a pure function of those points, so it is deterministic.

**Test results** (WSL Ubuntu 24.04, ngspice 42, `cargo test -p bb-analog-ngspice`):

- RC 1 kΩ / 1 µF with a 0→1 V external step at t = 0 (1 ns rise), paused at
  0.5, 1 and 3 ms: relative error against `1 - e^(-t/RC)` of 7.0e-6, 5.1e-6
  and 1.3e-6 (the limit is 1 %). Interpolated samples at 1.25, 2 and 2.75 ms
  are within 7.5e-7.
- Two identical runs give bit-identical boundary and interpolated values, and
  the same solver-point count (317).
- Under `bbcore::Scheduler` with a mock 1 ms toggler and a 100 µs `CsvWriter`,
  two runs give byte-identical CSV and `events.jsonl`, and v(2 ms) is within
  1 % of 3.3·(1 − e⁻¹).
- The full suite was run 20 times in a row and passed every time.

**Known limits.** A grant shorter than ngspice's `delmin` (≈1e-11·TMAX) has
not been tested. Output vectors grow for the whole run, because ngspice keeps
every point. A boundary that carries an input edge is not made an ngspice
breakpoint retroactively, because the step from `g` had already been chosen.
Only the clamp to the ramp corner applies, and ngspice's truncation-error
control handles the slope change.

### Amendment 2: Gauss-Seidel Advance, intra-grant edge delivery (2026-11-07)

**Status:** Accepted. It was required by the Phase 0 integration task ("an
edge occurring inside a grant is applied at its true time: the CPU engine
advances first for each grant, then analog advances to the same g with the
new edges known"). It is implemented in `bbcore/src/scheduler.rs`, and
`spec/time-grant.md` ("Advance") is updated in the same change.

**Problem.** Under D3/D4.4 every engine advanced to `g` before any outputs
were read. An MCU edge at `te` in `(p, g]` therefore reached ngspice only at
`g`, and ngspice started the ramp there. The skew `g - te` is deterministic
and below one quantum, but it is not zero, and the analog waveform was
shifted by a grant-dependent amount.

**Decision.** The Advance phase is ordered Gauss-Seidel style. For each engine
*i*, in registration order:

1. If an earlier engine changed a net bound to an input of *i* during this
   grant, the kernel calls `apply_inputs(snapshot)` on *i* **before** it
   advances. Digital signals in that snapshot may carry `edge_time` in
   `(p, g]`, and the consumer applies the transition at `edge_time`.
2. `advance_to(g)`.
3. The kernel collects at `g`: it drains logs, reads outputs, routes them to
   nets, runs the invariant-5 checks and marks the changed nets.

Exchange then runs as before, minus the output read: the full
`apply_inputs` snapshot to every engine, samples, events, probes. Engines
registered *before* a producer still get its edges at the boundary (Jacobi,
D4.4 unchanged for them), which covers feedback paths such as analog → MCU
input.

**Trait contract changes (no signature change).**

- `apply_inputs` can be called before `advance_to` within a grant, with
  `edge_time > now`. An engine that can apply an input at a sub-grant time
  (ngspice: a PWL ramp starting at `edge_time`) must do so. An engine that
  cannot do so applies it at `now`, which is no worse than before.
- `apply_inputs` must be idempotent for a snapshot that was already applied,
  because a later engine sees the same edge before its advance and again at
  the boundary. Both shipped engines already are: ngspice ignores an
  unchanged level, and Renode only writes a changed input level.
- The order of engine calls within a grant changes (advance, read and the
  next engine's apply are interleaved). It is still a pure function of the
  registration order and the net values, so D6 holds.
- Registration order now matters for fidelity, not just for tie-break
  labels. Producers must be registered before consumers. `bb run` registers
  the CPU engine first and ngspice second.

**Why it is safe.** No engine runs past `g`, and no engine runs before its
inputs for `(p, g]` are known from every engine earlier in the order. A
producer's outputs in `(p, g]` are final once it has reached `g`, so
delivering them earlier cannot be undone. The `t_safe` promise of each
engine is unchanged.

**Changes made.**

- `Scheduler::run`: the Advance loop does pre-advance delivery, then
  `advance_to`, then collect. `exchange()` no longer reads outputs.
- `NgspiceEngine::apply_inputs`: a digital edge is pushed at
  `max(edge_time, now)` instead of `now`.
- Tests: `bbcore/tests/scheduler.rs::gauss_seidel_delivers_edges_before_later_engines_advance`
  checks call order, the edge_time and that no extra calls are made.
  `routing_edges_and_outputs` now expects the follower to answer at the same
  boundary. `engines/analog-ngspice/tests/scheduler.rs::mid_grant_edge_is_applied_at_its_true_time`
  uses an edge at 0.3 ms inside a 1 ms grant into an RC with τ = 1 ms. With
  the driver registered first, v(1 ms) = 1.661274 V = 3.3·(1 − e^−0.7)
  (true time). With the driver registered after ngspice, v(1 ms) = 0 and
  v(2 ms) = 2.086005 V = 3.3·(1 − e^−1) (Jacobi, starts at the boundary).
- Phase 0 spike (`make spike`): every PA5 edge reaches ngspice in its own
  grant, and LED_A has switched by the first 10 µs sample after the edge.

### Amendment 3: Phase 0 verification fixes (2026-11-07)

**Status:** Accepted. Made by the Phase 0 fix task after independent
verification. `spec/time-grant.md` ("Definitions", "Exchange", "Sampled
probes") is updated in the same change.

**Finding A: Renode results depend on the `RUN_FOR` partition.** Measured
with Renode 1.17.0 and the blinky firmware at a 1 ms quantum: with the CSV at
1 kHz, every 50 ms instant was a grant boundary and the first five PA5 edges
came out at `k·50 ms + 1 fs` (SysTick ISR deferred to the next `RUN_FOR`,
then relabelled by the engine). At 1024 Hz the same edges came out mid-grant,
up to 50 ns earlier (`199 999 950 000 000` fs instead of
`200 000 000 000 001`). Even mid-grant edges moved by a few ns. The run was
deterministic for a fixed configuration, but D3's sample alignment made the
firmware-visible timing a function of an *output* setting, and the old claims
("an error of at most 1 ns"; "edge timing does not depend on the quantum")
were wrong.

Decision:

1. **Sampling never shapes the partition by default.** `Probe` gains
   `on_window(prev, now, nets, sampler)` and `Engine` gains
   `sample_output(pin, t)` (default `None`), both with defaults, so existing
   engines and probes are unchanged. `CsvWriter::sampled` does not register
   sample times; each row due in `[prev, now]` is read through the
   scheduler's sampler: the driving engine's `sample_output` (ngspice:
   linear interpolation between the solver points of that window, a pure
   function of them), otherwise the boundary value, with a digital net read
   at its previous level before `edge_time`. `bb run` uses it, so the grant
   partition is a function of the quantum, `t_end` and engine lookahead only.
   The aligned `CsvWriter::new` and `event_sampling` remain (D3 for them is
   unchanged) for callers that want boundary-exact samples and accept the
   partition dependence.
2. **Edge times are exact per partition, not partition independent.** An
   edge carries the time at which the emulated firmware changed the pin in
   *this* partition. Another quantum can move it by tens of ns (70 µs vs
   10 µs grants: 10 ns for the first five edges, measured). That is
   recorded as a Phase 0 limitation; a fixed quantum is part of the run
   configuration, like the seed. The engine docs, README and tests state it.
3. Tests: `engines/cpu-renode/tests/blinky.rs` asserts the exact golden edge
   list for the spike partition (10 µs, 500 ms), runs a 70 µs partition in
   which every expiry falls mid-grant, and checks that 1 kHz and 1024 Hz CSV
   rates give identical edges, grants and event streams.

**Finding B: f64 grant detection in ngspice broke after ~2.3 s.** Amendment 1
parked the solver when `round(time · 1e15) >= target_fs`. Above about 2.3 s an
f64 ulp exceeds 0.5 fs, so a solver landing one ulp above `target_s` rounded
to `target_fs + 1` and was reported as an overshoot; any run longer than
~2.29 s failed. D1's "converts at its own boundary" is kept, but the
boundary is now recognised in the f64 domain: the solver is at the grant when
`|time − target_s| <= max(0.5 fs, 4 ulp(target_s))`, and the parked point is
recorded as exactly `g`. Interior points are rounded to fs and clamped into
`[p, g]`. Test: `long_run_past_ten_seconds_in_1ms_grants` (12 s, 1 ms grants,
edges mid-grant).

**Finding C: the `t_safe` wording over-promised.** D2 said outputs "will not
change in `(now, t_safe)`". Renode's `t_safe = now + quantum` makes no such
promise: firmware can toggle at any instruction. The actual guarantee is:
advanced to any `g <= t_safe`, the engine reports every output change in
`(now, g]` at `g` with its exact `edge_time`, at most one per digital pin.
Consumers registered after the producer apply it at `edge_time`
(Amendment 2); consumers registered before it get it at the boundary
(Jacobi). The scheduler does not reject a consumer-before-producer order,
because the Jacobi fallback is legal and needed for feedback paths; the
wording in `bbcore::Engine` and the spec now says what is guaranteed.

**Smaller fixes.** `make test` sets `BB_REQUIRE_RENODE=1`, so the Renode
tests fail instead of skipping without Renode. A Renode engine that fails
after startup keeps its workdir and attaches the `renode.log` tail to the
error. `make setup` no longer changes the global rustup default
(`rust-toolchain.toml` pins the toolchain).
