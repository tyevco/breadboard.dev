# ADR-003: Co-simulation speed-up (lattice-aligned grants, edge traces, binding-graph order, CPU lookahead)

- **Status:** Accepted (2026-11-08). Parts A, B and C are approved; see
  "Approver decisions (2026-11-08)" below, which answer the open questions.
  The plan is implemented in order, step by step, each step behind its gates.
  `spec/` changes only when the matching step of the plan lands (AGENTS.md:
  spec changes ship with their ADR). Deviations found while implementing are
  recorded as amendments at the end of this ADR.
- **Amends:** ADR-002 (D2 `t_safe`, D3 grant/order, D4 exchange, D6
  determinism, Amendments 2 and 3). Normative text changes:
  `spec/time-grant.md`, `spec/pin-model.md`, `spec/event-stream.md`
  (schema version 0 → 1).
- **Parts.** Approval can be given per part:
  - **Part A** (plan steps 1–4): engine-local cost cuts, the lattice, edge
    traces, binding-graph order, the loop cap and a feedback example. Part A
    needs no Renode internals.
  - **Part B** (steps 5–8): the CPU `quiet_until` promise and the analog-first
    early stop, which make an idle MCU cheap even when the board has feedback.
  - **Part C** (step 9): ADC inputs.
- **Evidence:** the profiling and prototype work of 2026-11-08. It covers the
  spike, a stand-alone Renode harness and a stand-alone ngspice harness, plus
  three prototypes: lattice + `quiet_until` lookahead, dependency-aware
  scheduling, and lean lockstep with held sync. All of it lives in scratch
  clones outside the repo. The Renode facts below were re-checked against the
  Renode 1.17.0 sources (`~/bbscratch-src/renode`). The ngspice facts come from
  ADR-002 Amendment 1 (`sharedspice.h`, `dctran.c`).

## Approver decisions (2026-11-08)

The human approver accepted Parts A, B and C. The answers below resolve the
numbered "Open questions for the approver" at the end of this ADR and are
binding on the implementation.

1. **Defaults.** `--max-quantum 10ms` and `--renode-quantum 10us`. 10 µs edge
   and input resolution is the Phase 1 fidelity default.
2. **Sub-cell pulses.** Deliver every transition at `ceil_L(t)`, in order (the
   proposal). They are not collapsed into the final level.
3. **Renode internals.** Part B may rely on Renode 1.17.0 internals
   (reflection over `NVIC.pendingIRQs` and `TimeSourceBase.delayedActions`,
   the WFI hook), given the self-test fallback and the wake audit. Renode
   stays pinned to 1.17.0.
4. **ADC model.** Prefer an existing Renode ADC model on another STM32
   platform for the ADC demo; verify it in the 1.17.0 install first (for
   example `STM32_ADC_Common`, or the F0/G0/F4 platforms). Write a minimal
   in-repo STM32F1 ADC peripheral only if no existing model works.
5. **Schema bump.** event-stream v1 and regenerating the `events.jsonl`
   golden hash are approved.
6. **+1 fs convention.** Accepted: a genuine write exactly on a lattice point
   is reported 1 fs late.
7. **`--max-quantum`.** Keep the name; it becomes a pure grant cap, with no
   rename and no warning. The change of meaning is documented (CLI help,
   README, spec).
8. **ngspice handoff spin.** Ship it, default off.
9. **Plugin delivery.** Resolve during step 5 (prebuilt assembly vs. compile
   per run and cache). The default plugin address 0x5FFF0000 is acceptable.
10. **Scope.** All parts are approved. Implement them in plan order, each
    step behind its gates.

## Context

### The problem

`make spike` simulates 500 ms in 50 000 grants at the default 10 µs
`--max-quantum`. It takes 25–57 s of wall time depending on host load (README:
39.1 s, 0.73 ms per grant). The design doc's goal is that an idle MCU should
cost almost nothing. Phase 0 does not meet it.

### Where the time goes

10 µs grants, cross-check on, lightly loaded host. 25.4 s total: 2.24 s startup
plus 23.16 s simulation, which is 463 µs per grant.

| Component | Per grant | Share of sim time |
| --- | --- | --- |
| Renode `RUN_FOR` | 275 µs | 59 % |
| Renode `GET_TIME` cross-check | 69 µs | 15 % |
| Renode GPIO `GET_STATE` cross-check | 65 µs | 14 % |
| ngspice, total | 44 µs | 9.5 % |
| ↳ condvar handoffs, kernel→solver and solver→kernel | 17.4 + 15.5 µs | |
| ↳ solver work and vector reads | 10.5 + 2.4 µs | |
| Kernel | 8.8 µs | 1.9 % |

- **Most of the cost is fixed per call, not computation.** System time (13.6 s)
  exceeds user time (9.4 s). Emulation and SPICE solving are each under 3 %.
- **A `RUN_FOR` costs about the same whatever its length.** It takes 341 µs for
  10 µs and 350 µs for 1 ns. Each call creates and joins an OS thread for the
  CPU: 2014 threads for 2000 calls. Any request round trip costs about 65 µs.
- **Renode's internal sync points are cheap.** One 500 ms `RUN_FOR` with a
  10 µs global quantum takes 0.39–0.52 s, about 8–10 µs per sync point.
- **ngspice is cheap on its own.** `ngspice -b` on an equivalent netlist takes
  0.11 s for 50 536 points.
- **The cross-check costs 29 % and changes nothing.** `--no-verify` output is
  byte-identical.

### What decides correctness

These facts were measured and checked in source. They are why simply raising
`--max-quantum` is wrong.

1. **Renode's results depend on its sync-point sequence, not on where grants
   end.**
   - Sync points fall every global quantum, plus wherever timer limits stop a
     sleeping CPU. A `RUN_FOR` that ends between two of them shifts every later
     one.
   - Today `--max-quantum` *is* Renode's global quantum, so changing it changes
     firmware timing. 70 µs grants moved the first edges by 10 ns, and
     7/13/9.999 µs grants moved them by up to 23 ns.
   - Grants that are multiples of a fixed 10 µs Renode quantum reproduced the
     10 µs edge list exactly at 70 µs, 1 ms, 10 ms and mixed 10–2290 µs grants.
2. **GPIO stamps are sync-slice accurate, not instruction accurate.**
   - `GPIOPort.cs:97` stamps every GPIO event with
     `MasterTimeSource.ElapsedVirtualTime`, which is the last sync point.
   - A busy-loop firmware's edges landed on multiples of the quantum: 860 000 ns
     at a 10 µs quantum, 800 000 ns at 100 µs, 0 ns at 1 ms, and all 0 at 20 ms.
   - The spike only looks exact because a sleeping CPU syncs at each timer
     expiry. So Renode's quantum is a fidelity setting and must stay small even
     when grants grow.
3. **Renode paces idle firmware to the host clock unless told not to.**
   `BaseCPU.cs:812-819` sleeps on the host while the CPU is in WFI. It then
   sets the number of instructions to skip from the host's measured
   `intervalSlept`, unless `AdvanceImmediately` is set.
   - That is a host-timing dependence. It is invisible in short grants and real
     in long ones.
   - `emulation SetGlobalAdvanceImmediately true` exists
     (`MonitorTimeExtensions.cs:48`). Output stayed byte-identical with it, and
     a 500 ms `RUN_FOR` dropped from 563 ms to 74 ms.
4. **ngspice's step sequence follows `TMAX`, which today is
   `min(max_quantum, sample period)`.** At `--max-quantum 1ms --sample-rate
   1kHz` (the README's "cheap" setting), `TMAX` becomes 1 ms, and `check_spike`
   now fails: LED_A edge spacing came out at 48.93 ms.
5. **One edge per pin per grant.** 50 ms grants fail with "more than one edge
   inside grant".
6. **Latent bug.** `RenodeEngine::apply_inner` calls `gpio_set_state` as soon as
   it is called, at the grant start `p`, and ignores `edge_time`. That violates
   `spec/time-grant.md` Advance step 1, "must apply the transition at
   `edge_time`". It would apply an analog→MCU input up to a full grant early. No
   Phase 0 board has MCU inputs, so it has never fired.
7. **The External Control API has no lookahead primitive.**
   - The command set is CHECK_VERSION, RUN_FOR, GET_TIME, GET_MACHINE, ADC,
     GPIO, SYSTEM_BUS, TIME_ELAPSED_CALLBACK, SPI and CAN_BUS (`ICommand.cs`).
     There is no run-until-event command.
   - TIME_ELAPSED_CALLBACK sends a blocking `SendRequest` at every sync point
     and has no unregister path, only `Dispose`.
   - `protocol.rs` classifies messages by id bit 15. Renode's own callback
     events carry 0x8001, so they would be misrouted.
8. **The Phase 0 platform has no ADC.** Renode 1.17.0's
   `platforms/cpus/stm32f103.repl`, which the repo's platform file includes,
   contains no ADC peripheral (0 matches). GPIO→EXTI routing does exist, through
   `afio` (`[0-15] -> exti@[0-15]`).

## Decision

The kernel stays a **global, conservative lockstep**:

- every engine is at the same `now` between iterations;
- no engine runs past the grant;
- there is no rollback.

Four changes make grants long without changing any output.

### D1. Mandatory engine-local cost cuts (Part A)

1. **AdvanceImmediately is mandatory.** The Renode engine always issues
   `emulation SetGlobalAdvanceImmediately true` and has no option to turn it
   off. This is a determinism fix, not tuning (Context fact 3).
2. **The cross-check becomes `Verify::{Every, Edges, Final}`, default `Edges`.**
   - `Edges` runs `GET_TIME` and `GET_STATE` only on grants that carried an edge
     or an input, and at the end of the run.
   - `Every` is today's behaviour and runs in one CI job.
   - Checks are pure reads, so outputs do not change.
3. **`protocol.rs` classifies messages by `CommandType`, not by id bit 15.**
   - A unit test replays a byte stream that contains an event with id 0x8001.
   - `sysbus_read_u32s` and `sysbus_write_u32` are added for Part B.
4. **ngspice handoff spin (optional).** The engine spins briefly on an atomic
   sequence number before falling back to the condvar. It is bounded by
   `NgspiceConfig.handoff_spin`, which defaults to 0. It only matters while
   grants are short, in loop lockstep. Spinning changes only when a thread
   wakes, never what it computes.

### D2. The lattice: Renode's quantum becomes run configuration (Part A)

- **The lattice `L`.** `Engine::grant_lattice()` returns the step on which an
  engine's grant boundaries must fall. For Renode that is its global quantum,
  `--renode-quantum`, default 10 µs, which is no longer tied to `--max-quantum`.
  `L` is run configuration, like the seed: changing it changes results.
  - It is the firmware-timing and edge-stamp resolution.
  - It costs about 8–22 µs of wall time per sync point, so a 1 µs lattice costs
    roughly 5–11 s per 500 ms.
- **Validation and rounding.**
  - All declared lattices must be equal; Phase 1 allows only one CPU engine.
  - `max_quantum`, `loop_quantum`, `t_end` and an aligned `event_sampling`
    period must be multiples of `L`. Otherwise the run fails with a `Config`
    error.
  - The kernel computes `g` as today, then rounds it down: `g := floor_L(g)`.
    Rounding down is always safe, because the result is still at or below
    every `t_safe`.
  - If `floor_L(g) <= now`, the run fails with
    `SchedError::Lattice { engine, now, t_safe }`. The remedy is a smaller `L`
    or a fix to that engine's `t_safe`. The kernel never clamps silently.
- **What an edge time means.**
  - `edge_time` is the producer's best-known time of the change. For Renode it
    is the sync point that opens the slice in which the write executed. The
    true instruction time lies in `[edge_time, next sync point)`.
  - The spec restates "edges applied at true time" as "applied at the
    producer's reported `edge_time`". The producer's resolution is part of its
    model (Renode: one slice, at most `L`).
- **The lattice +1 fs rule** (replaces the Renode relabelling from ADR-002
  Amendment 3). Let `s` be Renode's ns stamp converted to fs.
  1. If `s` is a multiple of `L`, the write happened *after* the re-sync at
     `s`, and the engine reports `s + 1 fs`. This applies whether or not `s` is
     also a grant boundary.
  2. Otherwise `s` is a limit-driven sync point (a timer expiry that stopped a
     sleeping CPU, such as the spike's 499.99987 ms edge), and the engine
     reports `s` exactly.
  3. An event stamped exactly at the grant end `g`, which is a lattice point,
     belongs to the next grant by rule 1. The engine carries it over and
     reports it at `g + 1 fs` in the next grant. `read_outputs` at `g` reports
     the old level, and `Verify` accounts for carried events.
  4. A write that genuinely happens exactly on a lattice point is reported
     1 fs late. That is a documented convention.

  The rule depends only on `s` and `L`, never on where grants end. That removes
  the 70 µs anomaly, where the first five edges came out at 50 ms instead of
  50 ms + 1 fs. The spike's current edge list (k·50 ms + 1 fs, then
  499.99987 ms) is reproduced unchanged.
- **Renode applies inputs at `ceil_L(t)`, never early.** This fixes the latent
  bug (Context fact 6).
  - Any input transition at time `t` takes effect at `ceil_L(t)`, the first
    sync point at or after `t`, which is the earliest Renode can observe it.
  - When `apply_input_trace` delivers transitions with `t > now` before an
    advance, the engine splits its own `advance_to(g)` into `RUN_FOR` segments
    that end at each distinct `ceil_L(t)`. It sends `SET_STATE` (or ADC
    `SetValue`) while paused there. This splitting is internal to the engine:
    every segment ends on the lattice, at or before `g`, so it is not a
    free-run.
  - **Several transitions on one pin within one cell** (a sub-cell pulse) are
    *all* sent, in order, at `ceil_L(t)`. They are not collapsed into the final
    level.
    - `STM32_EXTICore.CanSetInterruptValue` latches pending on the configured
      edge, so an edge-triggered EXTI still sees the pulse.
    - Polling firmware sees only the final level, which matches what a real
      synchronizer sampling at `ceil_L(t)` would see.
    - This needs approval: see Open question 2.
- **ngspice steps on the lattice, whatever the partition.** Context fact 4
  shows why.
  1. **`TMAX`:** `TMAX = L`, or a board-declared value that divides `L`. It no
     longer derives from `max_quantum` or the sample rate.
  2. **Step clamp:** `clamp()` limits every step to the next lattice point as
     well as the grant end and ramp corners. The solver then has an accepted
     point at every lattice point.
  3. **Breakpoint release:** a ramp's corners are passed to `ngSpice_SetBkpt`
     only from the sync callback, at the lattice point `kL` that opens the cell
     `(kL, (k+1)L]` containing the ramp's `edge_time`. This is exactly when a
     one-cell run sets them, while parked at the cell start. It avoids
     ngspice's own breakpoint handling (`CKTsaveDelta`) seeing them earlier in
     a long grant.

  Ramp values are a pure function of `t`, and a ramp is never evaluated past
  its start before it exists, so the solver gets the same calls with the same
  arguments in a one-cell run and a long-grant run. Partition independence then
  holds by construction rather than by observation. It is gated by a raw
  solver-point dump.

### D3. Edge traces and binding-graph order (Part A)

- **Traces.**
  - An engine may report every output change in `(p, g]` as an ordered trace
    through `read_output_trace`, with any number of transitions per pin.
  - Consumers receive the transitions on their inputs through
    `apply_input_trace` before they advance.
  - The default `apply_input_trace` calls `apply_inputs` once per transition,
    in order, so existing engines keep working. ngspice already queues a ramp
    per edge.
  - Engines that do not implement `read_output_trace` keep the one-change-per-
    pin rule.
- **Analog → digital crosses as a digital trace.**
  - When a net driven by an analog engine is loaded by a `digital_in` pin, the
    *producing* engine applies `DigitalIn::threshold` (hysteresis: from Low,
    High once `v >= vih`; from High, Low once `v <= vil`) at each accepted
    solver point.
  - At a level change, the crossing time is linearly interpolated between the
    two straddling points and rounded up to whole fs. It is emitted as a
    `Digital` transition. This is a pure function of the solver points, which
    D2 makes partition independent.
  - The cost: a glitch that starts and ends between two solver points, which
    are at most `TMAX = L` apart, is missed.
- **Order comes from the bindings, not from registration.**
  1. At the first `run()`, the kernel builds the binding graph: an edge j→i
     when an output of j drives a net that i reads.
  2. It computes strongly connected components (SCCs) and orders them with
     Kahn's algorithm, breaking ties by the lowest registration index.
  3. Inside an SCC, lattice-bound engines (`grant_lattice().is_some()`) come
     first, then registration index.

  **Why lattice-bound first loses nothing.** A lattice-bound consumer applies
  an input at `ceil_L(t)`. In a one-cell grant `(p, p+L]`, that is `p+L = g`
  for any `t` in the cell. So it makes no difference whether the consumer runs
  before or after the producer within the cell. Putting it first gives every
  analog consumer its edges at their true time.

  The "register producers before consumers" rule in AGENTS.md is removed.
  Swapping registration order must give byte-identical output, and step 4 has a
  gate for it.
- **Event tie order is fixed by registration index.** Boundary records are
  stably sorted by `(t_fs, registration index, pin)`, never by plan order. In
  Part B the plan order can change from grant to grant, and records must not
  depend on it.

### D4. Grant rule (Part A) and the loop cap

`t_safe` for an engine that reports a complete trace is `Time::MAX`. That means
"I report everything", not "I am idle". In Part A, Renode returns it. ngspice
already returns `MAX`.

```
g = min( t_safe_i,                         // as today
         next aligned sample,              // only aligned probes; bb uses sampled probes
         now + max_quantum,                // bb default 10 ms (was 10 us)
         now + loop_quantum   if the binding graph has a cycle (non-trivial SCC or self-loop),
         t_end )
g = floor_L(g);  if g <= now -> SchedError::Lattice
```

- **`max_quantum`.** Default 10 ms in `bb`. It bounds the wall time of a single
  `RUN_FOR` against the 120 s I/O timeout and Ctrl-C, bounds memory for
  buffered traces, and bounds output latency. On an error, every record of the
  grants already completed is flushed before the error returns.
- **`loop_quantum`.** It defaults to `L`, so a board with feedback runs today's
  one-cell lockstep in the order D3 gives, which is correct.
  - The cap comes from the graph, not from registration order. An acyclic
    analog→MCU board (ngspice first) gets long grants, and Renode applies each
    input at `ceil_L(t)`.
  - Part A gives a feedback board only the D1 savings.
- **`limited_by`.** It gains `loop_quantum`. In Part B it also gains
  `stop:<engine>`; a grant shaped by a `quiet_until` reports `engine:<name>`.
  Tie-break order: engines by registration index, then `sample`, then
  `loop_quantum`, then `max_quantum`, then `t_end`.

**Spike under Part A:** one cycle-free edge U1→ngspice, `L = 10 µs`,
`max_quantum = 10 ms`, so 50 grants. In each grant, Renode makes one 10 ms
`RUN_FOR` and reports its trace, and ngspice advances with every ramp already
loaded.

### D5. CPU lookahead for feedback boards (Part B)

Part B lifts the loop cap for a grant, but only while a CPU proves that it is
quiet.

- **`quiet_until(now) -> Time` (`now` = no promise).** It is a stronger, separate
  promise than `t_safe`: if the engine's inputs stay constant, its outputs stay
  constant on `(now, quiet_until)`.
- **Renode implementation.** A read-only C# peripheral, `bb_lookahead`, mapped
  at a configurable unused address (default 0x5FFF0000). One `SYSTEM_BUS` read
  of 8 dwords per Query returns the values Renode's own WFI fast-forward uses
  (`BaseCPU.cs:771/809/933`):
  - in-WFI, tracked by `TranslationCPU.AddHookAtWfiStateChange`;
  - NVIC pending, the private `pendingIRQs` set read by reflection, never the
    side-effecting `FindPendingInterrupt()`;
  - queued synced-state actions, `TimeSourceBase.delayedActions` on the local
    and master time sources;
  - `IsHalted`;
  - `BaseClockSource.NearestLimitIn`, which covers SysTick, every
    `LimitTimer` including `STM32_Timer` compare channels, and
    `ScheduleAction`;
  - the clock-source time.

  The promise is `clock + NearestLimitIn` when all of these hold: the CPU is in
  WFI, nothing is pending or queued, it is not halted, a limit exists and the
  clock equals Renode's time. Otherwise there is no promise.
  - **Arming** goes through a monitor call in the generated script. Firmware
    writes to the block are ignored and logged.
  - **Self-test at startup.** It requires exactly one machine, one
    `TranslationCPU` and an NVIC, both reflected fields present, and no
    UART/socket host backends. If any check fails, the engine logs it and
    returns `now`, so the board falls back to the Part A cap.
  - **Hook use.** The WFI hook changes a tlib setting
    (`TlibSetCpuWfiStateChangeHookPresent`), so the hook is armed in *every* run
    of a board that loads the plugin, including `--lookahead off` reference
    runs. Only cyclic boards load the plugin.
  - **Startup cost.** The plugin is precompiled and cached (Open question 9).
    Gate: startup within 0.3 s of a run without the plugin.
- **Per-grant plan when lookahead is on.**
  1. Let `G = floor_L(min(t_safe_i, now + max_quantum, t_end, quiet_until_c))`
     for the quiet candidates `c`. The quiet set is
     `Q = {c : floor_L(quiet_until_c) >= G}`.
  2. Remove the out-edges of every engine in `Q`. The long grant `(now, G]` is
     allowed only if all four of these hold:
     1. the remaining graph is acyclic;
     2. every producer of a net read by an engine in `Q` has
        `can_stop_early() == true`;
     3. there is exactly one such producer;
     4. that producer is first in the topological order of the remaining graph
        (ties by registration index).

     Otherwise `g = now + loop_quantum` with the D3 order, which is Part A
     behaviour.
  3. **Early stop.** The first engine runs `advance_until(G, watch)`, where
     `watch` is the set of its output pins that feed `Q`. It returns `r`, the
     first lattice point at or after the first transition on a watched pin, or
     `G` if there is none. Every later engine then advances to `r`. When
     `r < G` the grant record says `limited_by: "stop:<engine>"`.

     Renode has no run-until-event primitive, so `RenodeEngine::can_stop_early`
     is `false`. MCU→MCU feedback therefore always falls back to one-cell
     grants.
  4. **Promise audit.**
     - The plugin records a WFI-exit counter and the timestamp of the first
       exit since the last Query.
     - After a quiet grant reaching `r`, the engine must have had no WFI exit
       at a time `< r` and no output transition in `(now, r]`. Otherwise the
       run fails with `SchedError::PromiseBroken { engine, promised, at }`.
     - The audit catches wakes that produce no edge: a wake that reads an input
       or arms a timer and goes back to sleep.
  5. **Query back-off.** A Query costs 0.3–0.7 ms. After `k` consecutive Queries
     with no promise, the next `min(2^k, 64)` one-cell grants skip the Query.
     A promise resets `k` to 0. The back-off depends only on simulated history.
     It changes only grant records, because lookahead never changes outputs
     (see Determinism).
- **ngspice `advance_until`** (`can_stop_early = true`). It runs with the
  lattice clamp and the thresholded outputs from D3. When a watched pin's
  thresholded level changes at a solver point, the engine computes `t_x`, keeps
  stepping to `L* = ceil_L(t_x)`, and parks there.
  - D2 puts a solver point on every lattice point, so `L*` is always a real
    park point.
  - Its inputs from `Q` are constant by the promise, so it computes exactly
    what a one-cell run computes.

### D6. ADC (Part C)

- **New pin model.** `adc_in{vref, bits}` is added to `spec/pin-model.md`. It
  is a *push* model: the producing analog engine emits a transition, carrying
  `Analog{volts}` at the lattice point, only at lattice points where
  `code = floor(v / vref · 2^bits)` changes.
- **Delivery.** Renode applies each transition with External Control ADC
  `SetValue(channel, value)` (`ADC.cs`) at that lattice point, splitting its
  `RUN_FOR` as in D2. A conversion then sees the value from the latest lattice
  point, at most `L` old. That is exactly what one-cell lockstep gives.
- **Lookahead.** In Part B, ADC-bound pins are watched like digital inputs, with
  a code-change watch. A code change can affect a quiet CPU through the analog
  watchdog interrupt, so it is an input change.
- **Rejected:** the pull model, in which a bus-read callback advances ngspice
  mid-grant (see Rejected alternatives).
- **Blocker:** Part C needs an ADC model for the platform (Context fact 8; Open
  question 4).

## Exact changes (described, not applied)

### `bbcore` (`engine.rs`, `signal.rs`, `scheduler.rs`)

```rust
// signal.rs (new)
pub struct Transition { pub t: Time, pub pin: String, pub signal: PinSignal }

// engine.rs: Engine gains (all defaulted; existing engines and mocks compile unchanged)
fn grant_lattice(&self) -> Option<Time> { None }                        // Part A
fn read_output_trace(&mut self) -> Result<Option<Vec<Transition>>, EngineError> { Ok(None) }
    // Some(trace): every change in (p, now], ordered by t, any count per pin;
    // the last entry per pin equals read_outputs(). None: legacy one-change rule.
fn apply_input_trace(&mut self, trace: &[Transition]) -> Result<(), EngineError> {
    // default: for each entry in order, apply_inputs(snapshot as of that entry)
}
fn quiet_until(&mut self, now: Time) -> Time { now }                    // Part B
fn can_stop_early(&self) -> bool { false }
fn advance_until(&mut self, t: Time, watch: &[String]) -> Result<Time, EngineError> {
    self.advance_to(t).map(|()| t)   // only called when can_stop_early() is true
}
// EngineError gains PromiseBroken { promised: Time, at: Time }.

// scheduler.rs
pub struct SchedulerConfig {
    pub max_quantum: Time,            // bb default 10 ms
    pub loop_quantum: Option<Time>,   // None = the lattice
    pub lookahead: bool,              // Part B; default false until step 8
    pub log_grants: bool,
    pub event_sampling: Option<EventSampling>,
}
// SchedError gains Lattice { engine, now, t_safe } and PromiseBroken { engine, promised, at }.
// Limiter gains LoopQuantum and Stop(usize).
// New: Plan { order: Vec<usize>, cyclic: bool } built once from bindings (Kahn, SCCs);
//      per-grant plan when lookahead is on (D5).
// probe.rs: VcdWriter::sampled(period). Analog reals are written at sample
//      instants through the sampler and digital wires at every trace entry, so
//      the VCD does not depend on the partition. The sampler reads digital
//      nets from the trace.
```

### Engines and CLI

- **`engines/cpu-renode`**
  - `RenodeConfig { quantum /* = L */, verify: Verify, lookahead: LookaheadCfg }`.
    `quantum` no longer comes from `--max-quantum`.
  - AdvanceImmediately is always on.
  - The lattice +1 fs rule, with carry-over of events stamped at `g`.
  - `apply_input_trace` with the `ceil_L` split, delivering every transition.
  - `read_output_trace`, and `t_safe = MAX`.
  - `protocol.rs` classifies messages by type and gains sysbus reads and
    writes.
  - `platforms/bb_lookahead.cs` (Part B).
- **`engines/analog-ngspice`**
  - `TMAX = L`, the lattice clamp in `clamp()`, and breakpoint release from the
    callback.
  - Thresholded (`digital_in`) and `adc_in` output bindings producing traces.
  - `advance_until` with park at `ceil_L(t_x)`, and `handoff_spin`.
- **`cli`**
  - New flags: `--renode-quantum 10us`, `--lookahead off|on` and
    `--verify every|edges|final`.
  - `--max-quantum` changes meaning ("cap on one grant") and its default to
    10 ms.
  - `--no-verify` becomes an alias for `--verify final`.
  - The board parser accepts `model=digital_in vih= vil= cin=` (and
    `model=adc_in` in Part C).

### `spec/time-grant.md` (diff-like)

- **Header:** cite ADR-003.
- **Definitions:**
  - `+ lattice L`: every boundary is a multiple of `L`.
  - `+ binding graph, plan order` (D3).
  - `+ trace`.
  - `+ loop_quantum`.
  - `+ quiet_until` (D5), defined separately from `t_safe`.
  - `~ t_safe`: "reports every output change in `(now, g]` at `g`, as an
    ordered trace (any number per pin) or, for legacy engines, one change per
    digital pin".
  - `~ edge_time`: "the producer's reported time of the change, at the
    producer's resolution (Renode: the sync slice, at most `L`)".
- **Grant row:** `g = floor_L(min(t_safe_i, next_sample, now+max_quantum,
  now+loop_quantum [cyclic graph, no quiet cut], t_end))`, plus the D5 quiet
  rule.
- **Invariant 1 (no free-run):**
  - `advance_to(t)` only with `now < t <= t_safe` and `t` on the lattice.
  - `advance_until` returns `r` with `now < r <= t`, where `r` is on the lattice
    or equals `t`. Every later engine is then advanced to `r`.
  - Advancing a producer before a quiet consumer is not a free-run.
- **Invariant 2:** add the `loop_quantum` term and the D4 tie-break order;
  rounding to the lattice follows the min.
- **Invariant 3 (lockstep):** every engine reaches the same boundary each
  iteration, in plan order, never concurrently.
- **Invariant 5:** per pin, trace times are strictly increasing within
  `(p, g]`, levels alternate, and the last entry equals the boundary value.
  Legacy engines keep the old rule.
- **New invariant 7:** every boundary and `t_end` is a multiple of `L`.
- **New invariant 8:** a broken quiet promise, meaning an output transition or
  a CPU wake before the grant's reached boundary, is a `PromiseBroken` error.
- **Advance:**
  - "registration order" → "plan order";
  - pre-advance delivery passes the trace;
  - a lattice-bound consumer applies inputs at `ceil_L(t)`, never early;
  - add the early-stop step.
- **Exchange step 4:** sort by `(t_fs, registration index, pin)`.
- **Sampled probes:** the VCD is sampled too, and digital samples come from the
  trace.
- **Examples:**
  - Example 1: separate the Renode quantum from the grant.
  - Example 2: replace it with lattice-aligned 10 ms grants.
  - New example 3: the feedback sequence below, Part A and Part B.
- **"What engines MUST NOT do":**
  - Renode advances only by `RunFor` to lattice points and runs with
    AdvanceImmediately;
  - inputs are never applied before `ceil_L(t)`;
  - remove "register producers first".

### Other specs and docs

- **`spec/pin-model.md`**
  - `digital_in`: thresholding is done by the producing analog engine over its
    solver points (D3).
  - `digital_out`: the ramp starts at `edge_time` (Amendment 2 already does this
    in code; the text still says "boundary").
  - New `adc_in` (Part C).
- **`spec/event-stream.md` → schema version 1**
  - `limited_by` gains `loop_quantum` and `stop:<engine>`.
  - `pin_edge` may repeat a net within one grant.
  - The tie order is defined by registration index.
- **AGENTS.md and README:** cost table; drop the "register producers first" bad
  idea; add "a `RUN_FOR` that ends off the lattice" as a bad idea; drop the
  cheap-config caveat.

## Determinism argument

1. **Renode.**
   - Its results are a function of its sync-point sequence and of when inputs
     arrive (ADR-002 D6.5).
   - Sync points are the lattice plus limit-driven syncs. Both are functions of
     emulation state, not of grant length, *provided* every `RUN_FOR` ends on
     the lattice (D2). So a run with any lattice-aligned partition passes
     through the same sync points as a one-cell run.
   - Inputs take effect only at lattice points, `ceil_L(t)`, as they
     effectively do in one-cell lockstep.
   - AdvanceImmediately removes the only host-time input (`BaseCPU.cs:812-819`).
   - `GET_TIME`, `GET_STATE` and the plugin reads are pure. The plugin reads
     private sets and never calls `FindPendingInterrupt()`.
2. **ngspice.** `TMAX = L`, the lattice clamp, breakpoint release at the cell
   start and pure ramp evaluation give an identical sequence of
   `GetSyncData`/`GetVSRCData` calls and arguments for every partition (D2). So
   the solver points are identical by construction.
3. **Kernel.**
   - Every decision is `u64` arithmetic.
   - The plan comes from bindings in `BTreeMap`s and Kahn's algorithm with
     index ties.
   - Traces are `Vec`s in emulation order.
   - Records are sorted by `(t_fs, registration index, pin)`.
   - The query back-off is a function of simulated history.
4. **Lookahead changes only the partition.**
   - By points 1 and 2, outputs do not depend on the partition.
   - The audit (D5.4) proves each quiet grant met its promise. A quiet CPU
     computes nothing during the grant, so letting ngspice go first sees
     exactly the constant inputs a one-cell run would have.
   - The early stop parks at the same `ceil_L(t_x)` at which a one-cell run
     delivers the input.
5. **Sampling.** CSV and VCD are sampled probes, so neither changes the
   partition and neither depends on it.

This gives one strong, mechanical gate, the **partition-equivalence gate**:
for any `max_quantum` that is a multiple of `L`, and for lookahead on or off, a
run must match the one-cell reference (`max_quantum = L`) byte-for-byte in
`nets.csv`, `nets.vcd` and every `events.jsonl` record except `grant`.

Determinism evidence from the prototypes:

- **Byte-identical to `out/spike` in every lattice-aligned run that was
  hashed:**
  - `nets.csv` 09fed4ac…, at 10 µs, 70 µs, 100 µs, 1 ms and 10 ms grants;
  - `nets.vcd` da42656b… (with the sampled VCD);
  - all `pin_edge` records.
- **Repeat runs:** 4 lookahead runs gave the same `events.jsonl` hash, and
  12 held-mode runs at load averages 3.4–9.7 were identical.

## Feedback-path correctness: worked example (`examples/feedback`)

**Board.**

- U1 PA5 (`digital_out`, 3.3 V, 5 ns, 25 Ω) drives R2 1 kΩ into node SENSE.
  C1 1 µF goes from SENSE to GND.
- SENSE drives U1.PB0, which is `digital_in{vih 2.0, vil 0.8, cin 5 pF}`.
- U1 PA6 drives R3 330 Ω into LED_B, through D2 to GND.
- Firmware: SysTick toggles PA5 every 50 ms. EXTI0 (PB0 through AFIO, both
  edges) runs an ISR that copies PB0 to PA6. The main loop is `wfi`.
- `L = 10 µs`. The graph is U1→ngspice (PA5, PA6) and ngspice→U1 (PB0), one
  SCC, so lockstep order is `[U1, ngspice]` (D3).

**Analytic crossings.** τ = (25 + 1000) Ω · 1 µF = 1.025 ms; `cin` adds 5 ppm.

| Edge | Crossing | Delay after the PA5 edge | Lands in cell | PB0 applied at |
| --- | --- | --- | --- | --- |
| PA5 rises at 50 ms + 1 fs | 2.0 V | τ·ln(3.3/1.3) = 0.954847 ms, so t_x ≈ 50.954847 ms | (50.95, 50.96] ms | 50.96 ms |
| PA5 falls at 100 ms + 1 fs | 0.8 V | τ·ln(3.3/0.8) = 1.452493 ms, so t_x ≈ 101.452493 ms | (101.45, 101.46] ms | 101.46 ms |

Between the thresholds, hysteresis holds the level. A slow ramp through
0.8–2.0 V yields exactly one transition per direction.

**Part A (loop cap, one-cell lockstep).**

```
Grant (50.95, 50.96] limited_by=loop_quantum
  U1  advance_to(50.96)   RUN_FOR(10us) to a lattice point; no edge
  ng  advance_to(50.96)   points every <=10us; SENSE crosses vih between two points
                          -> trace U1.PB0: High @ t_x = 50.954847.. ms (interpolated, fs)
  Exchange  pin_edge{t_fs: t_x, net: U1.PB0}; U1.apply_input_trace(PB0 High @ t_x)
            -> ceil_L(t_x) = 50.96 ms = now -> GPIO SET_STATE PB0=1 while paused at 50.96
Grant (50.96, 50.97]
  U1  RUN_FOR: EXTI0 pending at sync 50.96 -> ISR writes PA6; stamp 50.96 ms (lattice)
      -> reported 50.96 ms + 1 fs (lattice +1 fs rule)
  ng  pre-advance: PA6 High @ 50.96 ms + 1 fs -> ramp at its true time; advance
```

The MCU sees the crossing at `ceil_L(t_x)`: never early, and at most one cell
late. That is the granularity at which Renode observes any external input.

**Part B (lookahead).**

```
Query @50.97   U1 WFI, nothing pending, NearestLimitIn -> quiet_until ~ 99.99999 ms
               G = floor_L(min(MAX, 50.97+10ms)) = 60.97 ms; Q = {U1}; cut U1->ng;
               ng->U1 acyclic, ng first, can_stop_early
Grant (50.97, 60.97]   ng: advance_until(60.97, watch {U1.PB0}); no change -> r = 60.97
                       U1: RUN_FOR(10 ms); audit: no WFI exit, no edge -> ok
...            (two more 10 ms grants, then the grant to 99.99 ms)
Grant (99.99, 100.00]  quiet_until < 100.00 -> loop cap; SysTick ISR runs at sync 100 ms
Grant (100.00, 100.01] PA5 edge reported 100 ms + 1 fs; ng ramps at true time
Query @100.02  quiet again; G = 110.02 ms
Grant (100.02, 110.02] ng: SENSE falls through vil at 101.452493 ms -> parks at
                       r = 101.46 ms; limited_by=stop:ngspice; U1: RUN_FOR to 101.46
                       Exchange: SET_STATE PB0=0 at 101.46 (same lattice point as Part A)
Query @101.46  plugin: EXTI pending -> no promise -> one cell, U1 first:
               ISR -> PA6 Low at 101.46 ms + 1 fs, delivered to ng before it advances
Query @101.47  WFI -> long grants resume
```

This gives about 10 grants per 50 ms period, against 5 000 one-cell grants. A
script-driven Renode prototype of the same flow (crossings injected at
123.456789, 123.5, 200, 350.000001, 350.004 and 350.009999 ms) produced
93 grants and 0.75 s, against 50 002 grants and 32.68 s. The PA5 edges were
identical, the promise was withdrawn after every `SET_STATE` (status
0x12/0x13), and there were 0 violations. The ngspice half (`advance_until`) is
not built yet, so it is gated in step 7.

**ADC on the same board (Part C).**

- SENSE also feeds ADC channel 1 as `adc_in{vref 3.3, bits 12}`. TIM3 TRGO
  starts a conversion every 1 ms.
- **Part A.** ngspice emits a transition at each lattice point where the 12-bit
  code changes. Renode applies `SetValue` there. A conversion at a TIM3 update
  sees `v` of the latest lattice point, at most 10 µs old, which is identical
  to one-cell lockstep.
- **Part B.** The ADC pin is watched by code change. While SENSE is charging
  (about 5τ), the code changes nearly every cell, so grants stay one cell: no
  gain while the analog side moves. On plateaus, TIM3's 1 ms limit bounds the
  quiet window (TIM3 is a `LimitTimer` on the clock source). That is honest and
  sound.

## Expected speed-up and supporting measurements

Host load varied from about 1 to 9 during profiling, so the targets are ratios
and grant counts. **Measurement protocol for every gate:**

- interleaved A/B runs, N ≥ 5, medians;
- user+sys CPU time and the grant count, which is insensitive to load;
- startup reported separately from simulation;
- the load average recorded.

| Case | Supporting measurement (sim time, same build and host) | Target |
| --- | --- | --- |
| Spike, Part A (`L` = 10 µs, 10 ms grants, AdvanceImmediately, verify off) | profile: 0.95–1.00 s vs 31.24 s for 10 µs grants; lean lockstep: 0.83/1.08 s vs 43.71/40.51 s; dependency prototype at 10 ms: 1.10 s | exactly **50 grants**; sim ≤ 2.0 s median; **≥ 15×** the 10 µs baseline sim time (about 25–50× measured); total wall about 3–8 s, dominated by 2–6 s of startup, vs 25–57 s today |
| Feedback board, Part A (one-cell lockstep) | profile: 18.12 s with `--no-verify` vs 23.16 s | about 1.3–2× from D1 only; correct but slow |
| Feedback board, Part B, idle (WFI) firmware | Renode harness: 93 vs 50 002 grants, 0.75 vs 32.68 s; spike with lookahead: 38 grants, 0.80–1.99 s vs 36.75 s | **≤ 200 grants per 500 ms**; ≥ 10× vs Part A |
| Feedback board, busy or polling firmware | lookahead gains nothing; a Query costs 0.3–0.7 ms | ≤ 10 % slower than `--lookahead off` (back-off) |

## Rejected alternatives

| Alternative | Why rejected |
| --- | --- |
| **Raise `--max-quantum` only** (today's "cheap config") | It coarsens Renode's quantum, so firmware timing and GPIO stamps degrade to 1 ms. It also coarsens ngspice `TMAX`, and `check_spike` fails at 1 kHz. |
| **Per-engine epochs with `now_i`** (the dependency proposal's full form) | It breaks "every engine is at `now`" and adds a per-engine grant schema, a planner and sub-windows. On the spike it gains nothing over lattice + traces (0.85 vs 0.95–1.0 s). Its traces, graph order, never-early rule and sampled VCD are adopted. |
| **Held-sync Renode drive** (park by not acknowledging TIME_ELAPSED_CALLBACK) | 2.3–3.4× faster in one-cell lockstep, but the callback can never be unregistered (`Dispose` only), so every later sync point costs 150–200 µs. Long grants in held mode took 7.5–10 s. It is incompatible with D4/D5 and rests on undocumented nested-request behaviour. Kept on file as the only route to a Renode `advance_until` (MCU→MCU feedback). |
| **Static cap from registration order** ("Jacobi binding") | Order-dependent. An acyclic ngspice-first board would get long grants with inputs applied at `p`, up to a grant early. Replaced by the graph and `ceil_L`. |
| **TIME_ELAPSED_CALLBACK to deliver inputs each sync point** | 170–190 µs per callback, so slower than one-cell `RUN_FOR`. |
| **Peeking at SysTick registers over SYSTEM_BUS** | Specific to one peripheral; misses TIMx, EXTI, USART and DMA. 180 µs per read. |
| **ADC pull via `ExternalControlBusPeripheral` and nested requests** | Untested. From source, GPIO and bus events originate on the CPU thread while the default thread is busy in `RUN_FOR`, so nested requests would queue. It would also stamp `t_r` at the sync point, not the instruction. Would need its own ADR and experiment. |
| **Lateness-reporting relaxation** (`feedback_late`) | Not conservative. |
| **Optimistic execution with rollback** | Already rejected in ADR-002; neither engine snapshots cheaply. |
| **Busy-polling the Renode socket** | About 7 % gain, at the cost of a core. |
| **A smaller default lattice for edge accuracy** | 8–22 µs of wall time per sync point. It stays a per-run knob. |

## How each review concern is resolved

| Concern | Resolution |
| --- | --- |
| `apply_inner` applies inputs at `p` | D2 `ceil_L` split; gate in step 2 |
| GPIO stamps are sync-slice accurate | D2 redefines `edge_time` at the producer's resolution; `L` is a documented fidelity knob; busy-loop gate |
| +1 fs rule ambiguity and the 70 µs anomaly | D2 rules 1–4 (lattice points only, exact non-lattice syncs, carry-over at `g`) |
| ngspice partition independence only observed | D2 `TMAX = L`, lattice clamp, breakpoint release; raw solver-point gate |
| Broken promise detected only through edges | D5.4 wake audit |
| Private Renode internals; the WFI hook | Renode pinned at 1.17.0; self-test with fallback; hook armed in reference runs too; gate firmware set (step 5) |
| No gain for busy firmware; Query cost | Part A gives busy *acyclic* boards the full gain; D5.5 back-off |
| Held-mode callback cannot be removed | Rejected or deferred |
| Unbounded `RUN_FOR` wall time | `max_quantum` 10 ms; deterministic flush on error |
| Untested pull ADC and nested requests | Rejected; push at lattice points (D6) |
| Sub-cell pulses collapsed | Every transition delivered at `ceil_L` (D2); Open question 2 |
| Measurement rigor | Measurement protocol above |
| Startup cost | Plugin precompiled and cached, loaded only for cyclic boards; startup budget gate |
| README cheap config fails | `TMAX` decoupled (D2); `check_spike` gate at 1 ms / 1 kHz |
| Schema change | event-stream v1 (D4) |
| No-AdvanceImmediately hash agreement was luck | AdvanceImmediately mandatory (D1) |
| Analog half of lookahead unbuilt | Part B gated on the step 7–8 equivalence gates |
| Plugin gaps (multi-core, WFE/SEV, host terminals, timer outputs, DMA) | Self-test refusal conditions; gate firmware; audit |
| No Renode `advance_until` | `can_stop_early = false` for Renode; MCU→MCU stays one-cell |
| `quiet_until` vs `t_safe` | Separate definition; invariant 1 amended |
| Id bit 15 misclassification | D1.3 with a replay test |
| Firmware-visible plugin | Configurable address; arming by monitor call; firmware writes ignored and logged |
| Order inside an SCC | Lattice-bound first, with the proof in D3 |
| Event order under a dynamic plan | Sorted by registration index (D3) |

## Implementation plan (ordered, each step gated)

Every step keeps `make check` green: `fmt-check`, `clippy` and `test`,
including `determinism` and `golden`.

1. **Engine-local cost cuts (D1).**
   - Work: AdvanceImmediately, `Verify`, type-based message classification,
     sysbus helpers, optional ngspice spin.
   - Gates:
     - `out/spike` hashes unchanged: `nets.csv` 09fed4ac…, `events.jsonl`
       83bd875e…, `nets.vcd` da42656b…;
     - id 0x8001 replay test;
     - an injected time or GPIO mismatch is caught by `Edges` and `Final`;
     - a `--verify every` CI job;
     - ngspice suite 20× bit-identical with the spin on.
2. **Lattice (D2).**
   - Work: `grant_lattice`, rounding, `Config`/`Lattice` errors,
     `--renode-quantum`, +1 fs rule with carry-over, `ceil_L` input split,
     ngspice `TMAX = L`, lattice clamp and breakpoint release. `--max-quantum`
     stays 10 µs here, so hashes are unchanged.
   - Gates:
     - spike hashes unchanged;
     - `blinky.rs`: 10, 70, 100 µs and 1 ms partitions give an identical
       `pin_edge` list, the 70 µs anomaly is gone, and `nets.csv` is identical;
     - busy-loop firmware: stamps land on lattice points + 1 fs, and a
       100 µs lattice gives coarser stamps (documents fidelity);
     - copy firmware (PB0→PA6 busy loop) with an input `edge_time` mid-grant:
       the firmware sees it at `ceil_L(t)`, never earlier;
     - ngspice raw solver-point dump identical for one-cell vs 1 ms grants, on
       a netlist with a non-10 µs natural step and an edge in mid-cell;
     - RC analytic test unchanged;
     - `bb run --max-quantum 1ms --sample-rate 1kHz` passes `check_spike`;
     - scheduler mock tests for rounding and both errors.
3. **Traces, graph order, sampled VCD, loop cap (D3, D4); default
   `--max-quantum 10ms`.**
   - Gates:
     - mocks: 3 edges per pin per grant; Kahn and SCC order (DAG, cycle,
       self-loop, ties); a consumer registered before its producer now gets
       true-time edges; a cycle caps at `loop_quantum`; record tie order is
       independent of plan order;
     - new `make partition`: the spike at `max_quantum` ∈ {10 µs, 100 µs,
       10 ms, 50 ms, 500 ms} gives `nets.csv` and `nets.vcd` equal to the
       baseline hashes and identical non-grant records;
     - `make determinism`;
     - performance: exactly 50 grants, sim ≤ 2 s median, ≥ 15× the baseline
       (interleaved, N = 5).
4. **`digital_in` thresholding and `examples/feedback`** (cyclic, Part A).
   This makes feedback correctness testable before any optimization.
   - Gates:
     - PB0 transitions land in the analytic cells (50.95–50.96 ms and
       101.45–101.46 ms, then every period);
     - PA6 edges are in `(ceil_L(t_x), ceil_L(t_x) + L]` and never before
       `ceil_L(t_x)`;
     - swapping registration order gives byte-identical output;
     - 10-run determinism;
     - golden snapshot `tests/golden/feedback/`.
   - Plus an **acyclic variant** (`examples/feedback-open`): PA6 is probed but
     not loaded by the circuit, so the graph is ngspice→U1 and grants are
     long. Gate: identical to its one-cell reference (partition gate).
5. **Renode lookahead plugin (D5, Renode side).**
   - Work: `quiet_until`, wake audit, self-test, precompiled plugin,
     `--lookahead off|on`.
   - Gates:
     - the feedback example with the plugin armed and `off` is byte-identical
       to step 4 without the plugin (the hook and the reads are inert);
     - gate firmware, each either equivalent to one-cell or failing with
       `PromiseBroken`/fallback: TIM1 PWM on PA8, DMA→`GPIOx_BSRR`, UART TX,
       SLEEPONEXIT, WFE/SEV;
     - an injected early wake with no edge raises `PromiseBroken`;
     - the self-test fallback changes only grant records;
     - startup within 0.3 s of a run without the plugin.
6. **Kernel per-grant plan and early stop (D5).**
   - Mock gates:
     - a quiet producer with its consumer advanced first;
     - an early stop shrinks `g` for later engines;
     - a stopping engine that is not first falls back to one cell;
     - a remaining cycle falls back to one cell;
     - two producers feeding `Q` fall back;
     - a broken promise and a broken wake each raise `PromiseBroken`;
     - the back-off depends only on history;
     - identical plans across two runs.
7. **ngspice `advance_until`.**
   - Gates:
     - the RC crossing parks at exactly the analytic `ceil_L(t_x)`;
     - solver points are identical to the one-cell run;
     - two runs are bit-identical;
     - a hysteresis test with a slow ramp through 0.8–2.0 V gives exactly one
       transition per direction.
8. **Lookahead on for cyclic boards.**
   - Gates:
     - **equivalence:** `examples/feedback` with `--lookahead on` and
       `max_quantum` ∈ {1 ms, 10 ms} vs the one-cell `--lookahead off`
       reference, byte-identical in `nets.csv`, `nets.vcd` and all non-grant
       records;
     - 0 promise violations;
     - ≤ 200 grants per 500 ms;
     - ≥ 10× sim time vs step 4;
     - a polling-firmware variant is equivalent and ≤ 10 % slower than off.
9. **ADC (D6)**, after Open question 4 is decided.
   - Work: `adc_in`, code-change traces, `SetValue` at lattice points, a
     code-change watch.
   - Gates:
     - equivalence with a reference that pushes the value at every lattice
       point;
     - timer-triggered ADC firmware equivalent in Part A and Part B;
     - an analog-watchdog wake inside a quiet window is caught by the watch,
       not the audit.
10. **Docs.** README cost table (measured with the protocol), AGENTS.md rules,
    `make golden-update` for the new `events.jsonl` hash, which changes only in
    `grant` records.

## Open questions for the approver

1. **Defaults.** `--max-quantum 10ms` (vs 50–500 ms; the dependency prototype
   ran 500 ms at about the same cost) and `--renode-quantum 10us`. Is 10 µs
   edge and input resolution acceptable as the Phase 1 fidelity default?
2. **Sub-cell pulses.** Deliver every transition at `ceil_L(t)`, so an
   edge-triggered EXTI latches a 3 µs pulse (proposed). Or deliver only the
   final level and require a smaller `L` for short pulses?
3. **Part B dependence on Renode internals.** It relies on reflection over
   `NVIC.pendingIRQs` and `TimeSourceBase.delayedActions`, and on the WFI hook,
   with Renode pinned at 1.17.0. Acceptable with the self-test and audit? Or
   should a public "nearest event / quiet" API be upstreamed to Renode first?
4. **ADC model.** Renode 1.17.0's STM32F103 platform has no ADC. Should we
   write an `STM32F1_ADC` peripheral (C#, in-repo platform), move the ADC demo
   to a platform that has one (for example the F0/G0 families via
   `STM32_ADC_Common`), or defer Part C?
5. **Schema bump.** event-stream v1 (new `limited_by` values, repeated
   `pin_edge` per grant). The `events.jsonl` golden hash changes in step 3,
   though only in `grant` records. OK to regenerate?
6. **+1 fs convention.** A genuine write exactly on a lattice point is reported
   1 fs late. Accept, or carry a separate "slice start" flag?
7. **Behaviour change.** `--max-quantum` becomes a pure grant cap, and the old
   coupling to Renode's quantum disappears. Should `--max-quantum` keep
   working with a warning, or be renamed?
8. **ngspice handoff spin.** Ship it, off by default, or drop it? It helps only
   one-cell lockstep (feedback boards under Part A) and burns cores under
   `make determinism -j`.
9. **Plugin delivery.** Can Renode 1.17's monitor load a prebuilt assembly, or
   must the `.cs` be compiled per run and cached? This is unverified, and step 5
   resolves it before the startup gate. Is the default plugin address
   0x5FFF0000 acceptable?
10. **Scope.** Approve Part A now and Part B after step 4 shows the feedback
    example's one-cell cost? The proposal recommends this.

## Amendments

### Amendment 1: step 1 (D1) as implemented (2026-11-08)

**Status:** Accepted with step 1. `spec/time-grant.md` (state machine, "What
engines MUST NOT do") is updated in the same change.

- **End-of-run hook.** D1.2 says `Edges` and `Final` check "at the end of the
  run", but the Engine trait had no call at the end of a run: the engine
  never learns `t_end`. `bbcore::Engine` gains a defaulted
  `end_of_run(&mut self) -> Result<(), EngineError>`. The scheduler calls it
  for every engine, in registration order, each time `run` reaches its
  `t_end` (entry to Done); an error is `SchedError::Engine { phase: Done }`.
  It is for pure self-checks only and must not change anything that affects
  results, because a later `run` may resume. The Renode engine runs its
  `GET_TIME` + `GET_STATE` check there for `Edges` and `Final`. Existing
  engines and mocks compile unchanged.
- **What `Edges` counts as "an edge or an input".** Any GPIO event received
  during the grant (including a rewrite of the same level), or any
  `SET_STATE` sent since the previous advance.
- **Fault injection.** `RenodeEngine::inject_fault` (`#[doc(hidden)]`) arms a
  `RUN_FOR` that is 1 ns too long or drops one GPIO event, so the
  verify gate tests a real desynchronisation. Measured: `Every` catches the
  time fault at the faulty grant (19.9 ms boundary), `Edges` at the grant of
  the next edge (50 ms), `Final` at the end (70 ms); the lost 50 ms edge is
  caught by `Every` at its grant and by `Edges`/`Final` at the end of the run
  (no event reached the engine, so `Edges` has no reason to check earlier).
- **`--verify every` CI job.** `make spike-verify-every` re-runs the spike
  with `--verify every` and `cmp`s all three outputs against `out/spike`;
  the CI job `verify-every` runs it.
- **ngspice handoff spin gate.** "Suite 20× bit-identical with the spin on"
  is implemented as: the test helpers read `BB_NGSPICE_HANDOFF_SPIN_US`, the
  whole `bb-analog-ngspice` suite is run 20 times with it set, and the new
  test `handoff_spin_is_bit_identical` compares spin off against spin on
  within each run and prints a digest of the values, which must be the same
  in all 20 runs.
- **Port reservation (robustness, no protocol change).** Starting many Renode
  engines concurrently in one process (the verify gate starts six) exposed a
  race in `free_port`: the OS can hand the same ephemeral port to two
  `bind(0)` probes, and one engine then connected to the other's server. The
  engine now never reuses a port within a process.

### Amendment 2: step 2 (D2) as implemented (2026-11-08)

**Status:** Accepted with step 2. `spec/time-grant.md` (header,
Definitions: lattice `L` and `edge_time`; state machine and table: Init
lattice checks, Grant `floor_L`, `Lattice` error; invariants 1, 2, 4 and new
7; Advance step 1 `ceil_L`; sampled-probe note; example 1; "What engines MUST
NOT do") is updated in the same change. The other spec edits listed under
"Exact changes" belong to steps 3 and later.

- **Renode `t_safe` in step 2.** D4 (step 3) makes Renode's `t_safe`
  `Time::MAX`, which needs traces. Until then the engine keeps the
  one-change-per-pin rule with `t_safe = now + max_grant`, a new
  `RenodeConfig::max_grant` (a multiple of the quantum) that `bb run` sets to
  `--max-quantum`. This keeps the spike's grant records (`limited_by:
  engine:U1`) and with them the `events.jsonl` hash unchanged, as step 2
  requires, while `quantum` (the lattice) is now separate from the grant.
- **Lattice checks.** Besides `max_quantum`, `t_end` and the
  `event_sampling` period, every sample instant an *aligned* probe asks for
  (`CsvWriter::new`) must be on the lattice (`Config` error); otherwise
  `floor_L` could silently skip it. `SchedError::Lattice` names the engine
  with the smallest `t_safe` (the only term that can round down to `now`).
  `bb run` also rejects `--max-quantum` or `--for` off `--renode-quantum`.
- **Renode advance.** `advance_to(t)` refuses `t` off the lattice. Queued
  inputs split the advance into `RUN_FOR` segments at each distinct
  `ceil_L(t)`; `SET_STATE` is sent there in delivery order. An input
  delivered at a boundary (`edge_time <= now`) is applied at once. Events
  stamped exactly at the grant end are carried over (rule 3) and `Verify`
  expects the carried level.
- **ngspice.** `NgspiceConfig::lattice` (bb: `--renode-quantum`) with
  `t_max` required to divide it (bb: `t_max = L`). The lattice clamp treats a
  solver time within the grant tolerance of `kL` as on it, so a point a few
  ulps short of `kL` is not followed by a micro-step. A ramp's breakpoints
  are released at `max(now, ceil_L(t0) - L)`, the lattice point opening the
  cell that holds the ramp start `t0`.
- **Gate: `bb run --max-quantum 1ms --sample-rate 1kHz` passes
  `check_spike` is infeasible as written, and Context fact 4 misattributed
  the symptom.** With D2 in place (TMAX = 10 µs, partition-independent
  solver points) that run still fails `check_spike` with exactly the spacing
  the ADR quotes, 48.928731 ms. The cause is `check_spike` itself: it finds
  LED_A transitions by interpolating 1.0 V crossings between consecutive CSV
  samples, so at 1 kHz each crossing is only known to within the 1 ms sample
  period, against a 0.1 ms spacing tolerance. TMAX was not the cause of that
  number. The closest correct gate, `make lattice-check`, checks what D2
  actually promises: (1) the spike in 1 ms grants passes `check_spike` (100
  kHz CSV) and its `nets.csv` is byte-identical to the 10 µs spike
  (09fed4ac...), with identical non-grant records; (2) the 1 ms-grant 1 kHz
  CSV is row-for-row identical to the 100 kHz spike CSV at the same instants
  (501 rows, 0 differences), so the analog solution no longer depends on the
  sample rate or the partition. `check_spike` is unchanged.
- **Measured gate results.**
  - Spike hashes unchanged at the defaults: `nets.csv` 09fed4ac...,
    `events.jsonl` 83bd875e..., `nets.vcd` da42656b....
  - `blinky.rs`: 10, 70 and 100 µs and 1 ms grants on the 10 µs lattice give
    the golden edge list (first five at k·50 ms + 1 fs, including at 70 µs),
    identical non-grant records and identical sampled CSVs.
  - Busy-loop firmware (`examples/spike/firmware/busyloop.c`): every edge is
    a lattice point + 1 fs (1.500, 3.010, 4.510 ms ... + 1 fs at 10 µs; the
    same with 50 µs grants); a 100 µs lattice gives 1.5, 3.0, 4.5 ms ... + 1 fs.
  - Copy firmware (`copy.c`, PB0 -> PA6), driver registered before Renode,
    inputs at 123.456789 µs, 1.2 ms and 2 ms + 1 fs: PA6 follows at 130 µs,
    1.2 ms and 2.01 ms, each + 1 fs, in 1 ms grants and in 10 µs grants alike.
  - ngspice raw solver-point dump (RC with τ = 2.2 µs, three mid-cell
    edges): 424 points, bit-identical for 10 µs, 70 µs and 1 ms grants; as a
    negative control, without the lattice the same runs give 424, 389 and
    401 points.
  - RC analytic tests unchanged (same 317 solver points and value digest).
- **Test firmware.** `examples/spike/firmware/Makefile` now builds
  `busyloop.elf` and `copy.elf` next to `blinky.elf`; `blinky.elf` is
  byte-identical to before.
- **Process note.** The step 1 commit accidentally included early draft
  lattice text for `spec/time-grant.md`; the next commit restored the step 1
  version, and the text lands with this step.

### Amendment 3: step 3 (D3, D4) as implemented (2026-11-08)

**Status:** Accepted with step 3. `spec/time-grant.md` (header; Definitions:
`t_safe`, trace, binding graph and plan order, `max_quantum` as a pure cap,
`loop_quantum`; state machine and table; invariants 2, 3, 4, 5, 7; Advance;
Exchange steps 2 and 4; sampled probes and the sampled VCD; examples 1–3;
"What engines MUST NOT do"), `spec/event-stream.md` (schema version 1) and
`spec/pin-model.md` (`digital_out` ramp start) are updated in the same change.
The `digital_in` text of `spec/pin-model.md` belongs to step 4.

- **What landed.** `bbcore::Transition`; `Engine::read_output_trace` and
  `Engine::apply_input_trace` (defaulted); `SchedulerConfig::loop_quantum`;
  `Scheduler::plan()` returning `Plan { order, cyclic }`; `Limiter::LoopQuantum`;
  records sorted by registration index; `VcdWriter::sampled` and
  `NetSampler::digital_changes`. The Renode engine reports a trace and
  returns `t_safe = Time::MAX`; `RenodeConfig::max_grant` (the interim rule
  of Amendment 2) is removed. `bb run` defaults to `--max-quantum 10ms` and
  writes a sampled VCD at `--sample-rate`. Part B items (`quiet_until`,
  `advance_until`, `Limiter::Stop`, `SchedulerConfig::lookahead`) are not
  added yet.
- **`apply_input_trace(base, trace)`.** The ADR sketch has
  `apply_input_trace(&mut self, trace)` with a default of "`apply_inputs`
  (snapshot as of that entry)". `apply_inputs` takes the full snapshot of the
  engine's bound inputs, which the trace alone cannot rebuild, so the method
  also takes `base`, the full input snapshot before the first entry; the
  default applies `base` updated by every entry up to and including the
  current one. Renode and ngspice use the default: both already queue one
  change per call (Renode at `ceil_L(t)`, every transition in order; ngspice
  one ramp per edge at its `edge_time`).
- **Boundary delivery inside a loop.** D3 says consumers receive the
  transitions on their inputs "before they advance". A consumer before its
  producer in the plan order (only inside a cycle, or a self-loop) cannot.
  Exchange step 2 therefore first calls `apply_input_trace` with the changes
  of this grant from engines at or after the consumer in the plan, then the
  snapshot. That delivers every transition of a sub-cell pulse to a
  lattice-bound consumer in a loop too, at `ceil_L(t) = g` (approver decision
  2), instead of only the final level.
- **Trace scope.** A digital pin that a traced engine leaves out of its trace
  must be unchanged (otherwise `Causality`). An analog output left out of the
  trace is reported by its boundary value, as before: ngspice's analog
  outputs (LED_A) change at every solver point and stay out of traces;
  sampled probes read them through `sample_output`. The sampler reads a
  digital net from the window's changes; an analog net of an engine without
  `sample_output` keeps the old rule (its value at `g`).
- **Two changes of one pin in one Renode slice.** Renode stamps both with the
  same sync point, so the lattice rule gives them the same time and a trace
  cannot order them. The engine reports a protocol error (lower
  `--renode-quantum`). The error depends only on the stamps, so it is the same
  in every partition, as the one-change rule of Phase 0 was in one-cell runs.
- **Record order.** The key is `(t_fs, registration index, class, index)`:
  per engine its logs (emission order) before its edges (output-binding
  order), and the kernel's `net_sample`s after every engine. This keeps the
  Phase 0 order for every existing stream and never uses the plan order.
- **A cycle without a lattice.** `loop_quantum = None` means "the lattice";
  with no lattice-bound engine a cyclic graph has no cell, so it is a
  `Config` error unless `loop_quantum` is set. `loop_quantum` must be a
  multiple of `L`.
- **Flush on error (D4).** The scheduler flushes the event sink when a run
  fails, and `bb run` writes all three outputs before it reports a
  simulation error.
- **Grant labels.** In the spike every grant but the last is `max_quantum`.
  The last says `engine:ngspice`: ngspice's `t_safe` is its transient end,
  500 ms, which ties `t_end`, and engines win ties (invariant 2).
- **Test changes required by D3.** The Phase 0 tests that expected a consumer
  registered before its producer to get edges one grant late (the bbcore
  Gauss-Seidel mock test and the ngspice `mid_grant_edge_is_applied_at_its_true_time`
  "Jacobi" half) now assert the opposite: true-time delivery whatever the
  registration order (ngspice: bit-identical values for both orders).
- **Renode startup robustness (no protocol change).** The first full test
  run, with about 20 Renode instances starting at once, failed one start with
  "Connection reset by peer" during the External Control handshake and an
  empty Renode log. A likely cause, not verified, is a TCP self-connect: while
  the reserved port has no listener yet, the client's ephemeral source port
  can be that same port. The engine now drops a connection whose local and
  peer addresses are equal and retries. The failure did not recur in later
  runs.
- **Gates.**
  - Mocks (`bbcore/tests/scheduler.rs`, `bbcore/src/scheduler.rs` unit tests):
    three edges per pin in one grant reach a consumer registered before the
    producer as three ordered deliveries at their true times, with four
    `pin_edge` records and a sampled CSV and VCD read from the trace
    (`traces_carry_three_edges_per_pin_per_grant`); bad traces are
    `Causality` errors; Kahn and SCC order for a DAG, a backwards chain, ties,
    a cycle (lattice-bound first), a producer feeding a cycle and a
    self-loop; a cycle and a self-loop cap grants at `loop_quantum` (default
    the lattice, or an explicit 50 µs), an acyclic graph does not, and a
    lattice-free cycle or an off-lattice `loop_quantum` is a `Config` error;
    two engines with logs and edges at the same instants give byte-identical
    streams under the plan orders [b, a] and [a, b]
    (`record_tie_order_does_not_depend_on_plan_order`); a consumer
    registered before its producer gets the edge before it advances
    (`consumers_get_edges_before_they_advance_whatever_the_registration_order`).
  - Renode (`engines/cpu-renode/tests/blinky.rs`): 10, 70 and 100 µs and 1,
    10, 50 and 500 ms grants on the 10 µs lattice give the golden edge list,
    identical non-grant records and an identical sampled CSV; the 500 ms run
    is one grant whose trace holds all ten edges. The copy firmware follows
    inputs at `ceil_L(t) + 1 fs` with the driver registered after Renode, and
    with all three inputs inside one 3 ms grant.
  - **`make partition`** (`scripts/partition.sh`, CI job `partition`): the
    spike at `max_quantum` 10 µs, 100 µs, 10 ms, 50 ms and 500 ms (50 000,
    5 000, 50, 10 and 1 grants) gives `nets.csv` 09fed4ac…, `nets.vcd`
    da42656b… and non-grant records 55e9991c…, the step 2 baseline. The hashes
    are committed in `tests/golden/spike/partition.sha256`, which
    `make golden-update` regenerates. The gate is not part of `make test`
    (five runs, about a minute in parallel); CI runs it as its own job.
  - `make determinism` (10 runs of the new default) is part of `make check`.
    The earlier gates still pass against the 10 ms default: `make
    lattice-check` (1 ms grants, 1 kHz resample) and `make
    spike-verify-every`.
  - The `events.jsonl` hash of the spike changes only in its `grant` records
    (approver decision 5): 95775a08… at the 10 ms default.
  - **Performance** (`make perf`, `scripts/perf.sh`, the measurement
    protocol: interleaved A/B, N = 5, medians, startup reported separately,
    user+sys CPU and load average recorded; both sides `--verify edges`):
    A = `--max-quantum 10us` (50 000 grants), B = the 10 ms default, exactly
    50 grants in all five runs. Median simulation time A 45.89 s, B 1.42 s,
    a **32.3×** speed-up (gates: B ≤ 2.0 s, ≥ 15×). Median startup A 6.67 s,
    B 4.92 s; median total A 53.95 s, B 9.20 s; median user+sys CPU A
    49.92 s, B 11.25 s. Load average 3.0–11.8 (other work shared the host,
    including an orphaned Renode from the profiling harness at 100 % of one
    core). Two of the five B runs took 3.13 s and 4.13 s of simulation, so
    the 2 s bound holds for the median, as the protocol states it, not for
    every run under that load. Per-run numbers: `out/perf/results.tsv`.
