//! Conservative lockstep scheduler (the time-grant protocol).
//!
//! Normative description: `spec/time-grant.md`; rationale:
//! `docs/adr/002-time-grant-protocol.md` and `docs/adr/003-cosim-speedup.md`.
//!
//! ```text
//! Init -> Query -> Grant -> Advance -> Exchange -> Query -> ... -> Done
//!                                                      \-> Error (any failure)
//! ```
//!
//! Each iteration: `g = floor_L(min(t_safe_i, next probe/sample time,
//! now + loop_quantum (feedback only), now + max_quantum, t_end))`; every
//! engine advances to exactly `g`; outputs are read, routed by net name and
//! applied as inputs. Only integer [`Time`] takes part in the decision, and
//! every collection is ordered, so runs are deterministic.
//!
//! Advance order is the **plan order** (ADR-003 D3), built once from the
//! bindings: producers before consumers (strongly connected components in
//! Kahn order, ties by the lowest registration index; inside a component,
//! lattice-bound engines first). Each engine's outputs (an ordered trace,
//! [`Engine::read_output_trace`], or one change per pin) are read right after
//! its advance, and every engine later in the order receives the changes on
//! its inputs *before* its own `advance_to(g)` ([`Engine::apply_input_trace`]),
//! so it applies them at their true time. A binding graph with a cycle caps
//! every grant at `loop_quantum` (default: the lattice, one cell).
//!
//! With `lookahead` on (ADR-003 D5) the loop cap is lifted for a grant while
//! CPU engines promise to stay quiet ([`Engine::quiet_until`]): the kernel
//! cuts the out-edges of the quiet set `Q`, and if the rest of the graph is
//! acyclic with exactly one producer feeding `Q` that can stop early and is
//! first in its topological order, that producer runs
//! [`Engine::advance_until`] on the pins feeding `Q` and every later engine
//! advances to where it stopped (a per-grant plan). Otherwise the grant is
//! the one-cell grant of the static plan.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};

use crate::engine::{Engine, EngineError};
use crate::event::{Event, EventSink};
use crate::probe::{NetSampler, Probe};
use crate::signal::{volts_to_microvolts, NetValues, PinSignal, PinSignals, Transition};
use crate::time::Time;

/// Index of an engine registered with [`Scheduler::add_engine`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EngineId(usize);

impl EngineId {
    /// The registration index (0 for the first engine registered).
    pub fn index(self) -> usize {
        self.0
    }
}

/// Direction of a pin binding, seen from the engine.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Direction {
    /// The engine drives the net.
    Output,
    /// The engine reads the net.
    Input,
}

/// Periodic `net_sample` records in the event stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventSampling {
    pub period: Time,
    pub nets: Vec<String>,
}

/// Scheduler configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchedulerConfig {
    /// Upper bound on any single grant (a pure cap: it bounds the wall time
    /// of one advance, buffered traces and output latency). Must be > 0.
    pub max_quantum: Time,
    /// Upper bound on a grant while the binding graph has a cycle (ADR-003
    /// D4). `None`: the grant lattice, i.e. one-cell lockstep. A cyclic
    /// graph without a lattice needs an explicit value.
    pub loop_quantum: Option<Time>,
    /// CPU lookahead for cyclic graphs (ADR-003 D5): ask the engines in a
    /// feedback loop for [`Engine::quiet_until`] and lift the loop cap while
    /// a promise and the per-grant plan allow it. Lookahead changes only the
    /// partition (and so only `grant` records), never outputs. Default
    /// `true` (ADR-003 plan step 8); it matters only for a cyclic graph with
    /// a candidate that promises (an engine without `quiet_until` never
    /// does), and `false` gives the one-cell reference.
    pub lookahead: bool,
    /// Emit a `grant` record per grant.
    pub log_grants: bool,
    /// Emit `net_sample` records at a fixed rate (grants are aligned to it).
    pub event_sampling: Option<EventSampling>,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        SchedulerConfig {
            max_quantum: Time::from_ms(1),
            loop_quantum: None,
            lookahead: true,
            log_grants: true,
            event_sampling: None,
        }
    }
}

/// The plan of one grant (ADR-003 D5), recorded when
/// [`Scheduler::record_plans`] is on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrantPlan {
    /// The grant `(from, until]` as executed (`until` is the early stop).
    pub from: Time,
    pub until: Time,
    /// Registration indices in this grant's advance order.
    pub order: Vec<usize>,
    /// The quiet set `Q` (registration order); empty for a static grant.
    pub quiet: Vec<usize>,
    /// The engine that ran `advance_until` (a quiet grant only).
    pub stop: Option<usize>,
    /// Engines asked for `quiet_until` in this grant's Query.
    pub queried: Vec<usize>,
}

/// Query back-off of one lookahead candidate (ADR-003 D5.5).
#[derive(Copy, Clone, Debug, Default)]
struct Backoff {
    /// Consecutive Queries without a promise.
    misses: u32,
    /// Remaining loop-capped grants that skip the Query.
    skip: u32,
}

/// The longest skip after Queries without a promise (ADR-003 D5.5).
const MAX_QUERY_SKIP: u32 = 64;

/// A quiet grant chosen in Query (ADR-003 D5).
struct QuietGrant {
    /// `G`, on the lattice.
    g: Time,
    limiter: Limiter,
    /// Advance order of the remaining graph; `order[0]` stops early.
    order: Vec<usize>,
    /// `Q` with each promise, in registration order.
    promises: Vec<(usize, Time)>,
    /// Output pins of `order[0]` that feed `Q`.
    watch: Vec<String>,
    /// Nets of those pins.
    watch_nets: Vec<String>,
}

/// The advance order built from the bindings at the first `run` (ADR-003
/// D3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// Registration indices in advance order.
    pub order: Vec<usize>,
    /// The binding graph has a cycle: a strongly connected component of more
    /// than one engine, or an engine reading a net it drives.
    pub cyclic: bool,
}

/// Protocol state (see `spec/time-grant.md`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Constructed; engines and bindings may be added.
    Init,
    /// Collecting `t_safe` from every engine.
    Query,
    /// Grant computed and published.
    Grant,
    /// Engines advancing to the grant.
    Advance,
    /// Reading outputs, routing nets, applying inputs, notifying probes.
    Exchange,
    /// `now == t_end`. A later `run` with a larger `t_end` resumes at Query.
    Done,
    /// A failure occurred; the scheduler refuses further work.
    Error,
}

/// Scheduler errors.
#[derive(Debug, thiserror::Error)]
pub enum SchedError {
    #[error("invalid configuration: {0}")]
    Config(String),
    #[error("engine name {0:?} is empty or already registered")]
    DuplicateEngine(String),
    #[error("net {net:?} already driven by {existing}; cannot also be driven by {new}")]
    MultipleDrivers {
        net: String,
        existing: String,
        new: String,
    },
    #[error("bindings cannot change after the run has started")]
    AlreadyStarted,
    #[error("engine {engine}: t_safe {t_safe} is not after now {now}")]
    NoProgress {
        engine: String,
        now: Time,
        t_safe: Time,
    },
    #[error("engine {engine} failed during {phase:?}: {source}")]
    Engine {
        engine: String,
        phase: Phase,
        #[source]
        source: EngineError,
    },
    #[error(
        "engine {engine}: t_safe {t_safe} rounds down to the grant lattice at or before \
         now {now}; use a smaller lattice or fix the engine's t_safe (ADR-003 D2)"
    )]
    Lattice {
        engine: String,
        now: Time,
        t_safe: Time,
    },
    #[error("causality violation on net {net}: {detail}")]
    Causality { net: String, detail: String },
    /// A quiet promise was broken (ADR-003 D5.4, invariant 8): an output
    /// change, or a wake the engine reported, at `at`, before the boundary
    /// of a grant that relied on `quiet_until = promised`.
    #[error("engine {engine}: quiet promise until {promised} broken at {at} (ADR-003 D5.4)")]
    PromiseBroken {
        engine: String,
        promised: Time,
        at: Time,
    },
    #[error("output I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("scheduler halted after an earlier error")]
    Halted,
}

/// Summary of a completed `run`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RunSummary {
    /// Sim time reached (== `t_end`).
    pub now: Time,
    /// Grants issued since construction.
    pub grants: u64,
}

struct Binding {
    engine: usize,
    pin: String,
    net: String,
}

/// Changes of each net inside the current grant, in time order: `(t,
/// signal)`. Digital entries carry `edge_time == t`; an analog change of an
/// engine without a trace is recorded at the boundary.
type NetChanges = BTreeMap<String, Vec<(Time, PinSignal)>>;

/// A boundary record with its sort key `(t_fs, origin, class, index)`:
/// origin is the registration index of the engine that produced it
/// (`usize::MAX` for the kernel's `net_sample`s), class 0 for logs and 1 for
/// pin edges, index the log order or the output-binding index. Never the
/// plan order (ADR-003 D3).
type Record = ((Time, usize, u8, usize), Event);

/// The kernel's conservative lockstep scheduler.
pub struct Scheduler {
    config: SchedulerConfig,
    engines: Vec<Box<dyn Engine>>,
    outputs: Vec<Binding>,
    inputs: Vec<Binding>,
    drivers: BTreeMap<String, String>,
    nets: NetValues,
    /// Net values at the previous boundary (for sampling inside a window).
    prev_nets: NetValues,
    now: Time,
    phase: Phase,
    grants: u64,
    next_event_sample: Time,
    /// The grant lattice (ADR-003 D2), fixed at the first `run`.
    lattice: Option<Time>,
    /// The advance order (ADR-003 D3), fixed at the first `run`.
    plan: Option<Plan>,
    /// Registration index of the engine driving each net.
    driver: BTreeMap<String, usize>,
    /// The binding graph `(j, i)`: an output of j drives a net i reads.
    edges: BTreeSet<(usize, usize)>,
    /// The cap on a grant of a cyclic graph, fixed at the first `run`.
    loop_quantum: Option<Time>,
    /// Lookahead candidates (ADR-003 D5): engines in a feedback loop for
    /// which a quiet grant with `Q = {c}` is structurally possible.
    candidate: Vec<bool>,
    /// Query back-off per engine (ADR-003 D5.5).
    backoff: Vec<Backoff>,
    /// Per-grant plans, when recording is on.
    plan_log: Option<Vec<GrantPlan>>,
    /// Changes of each net in the last grant (for sampled probes).
    changes: NetChanges,
}

/// What bounded a grant, in tie-break order.
#[derive(Copy, Clone, Debug)]
enum Limiter {
    Engine(usize),
    /// The early stop of this engine (ADR-003 D5.3), `stop:<engine>`.
    Stop(usize),
    Sample,
    LoopQuantum,
    MaxQuantum,
    TEnd,
}

impl Scheduler {
    pub fn new(config: SchedulerConfig) -> Result<Self, SchedError> {
        if config.max_quantum.is_zero() {
            return Err(SchedError::Config("max_quantum must be > 0".into()));
        }
        if config.loop_quantum.is_some_and(Time::is_zero) {
            return Err(SchedError::Config("loop_quantum must be > 0".into()));
        }
        if let Some(s) = &config.event_sampling {
            if s.period.is_zero() {
                return Err(SchedError::Config("event sample period must be > 0".into()));
            }
        }
        Ok(Scheduler {
            config,
            engines: Vec::new(),
            outputs: Vec::new(),
            inputs: Vec::new(),
            drivers: BTreeMap::new(),
            nets: NetValues::new(),
            prev_nets: NetValues::new(),
            now: Time::ZERO,
            phase: Phase::Init,
            grants: 0,
            next_event_sample: Time::ZERO,
            lattice: None,
            plan: None,
            driver: BTreeMap::new(),
            edges: BTreeSet::new(),
            loop_quantum: None,
            candidate: Vec::new(),
            backoff: Vec::new(),
            plan_log: None,
            changes: NetChanges::new(),
        })
    }

    /// Record the plan of every grant from now on ([`Scheduler::plan_log`]),
    /// for tests and diagnostics (ADR-003 D5). Off by default.
    pub fn record_plans(&mut self, on: bool) {
        match (on, self.plan_log.is_some()) {
            (true, false) => self.plan_log = Some(Vec::new()),
            (false, true) => self.plan_log = None,
            _ => {}
        }
    }

    /// The recorded per-grant plans (empty unless [`Scheduler::record_plans`]
    /// is on).
    pub fn plan_log(&self) -> &[GrantPlan] {
        self.plan_log.as_deref().unwrap_or(&[])
    }

    /// Register an engine. The registration index orders the Query, the
    /// boundary exchange, ties in the event stream and ties in the plan
    /// order; the advance order itself comes from the bindings (ADR-003 D3).
    pub fn add_engine(&mut self, engine: Box<dyn Engine>) -> Result<EngineId, SchedError> {
        if self.phase != Phase::Init {
            return Err(SchedError::AlreadyStarted);
        }
        let name = engine.name().to_string();
        if name.is_empty() || self.engines.iter().any(|e| e.name() == name) {
            return Err(SchedError::DuplicateEngine(name));
        }
        self.engines.push(engine);
        Ok(EngineId(self.engines.len() - 1))
    }

    /// Bind `engine.pin` to `net`. A net has at most one driver.
    pub fn bind(
        &mut self,
        engine: EngineId,
        pin: &str,
        net: &str,
        dir: Direction,
    ) -> Result<(), SchedError> {
        if self.phase != Phase::Init {
            return Err(SchedError::AlreadyStarted);
        }
        let e = self
            .engines
            .get(engine.0)
            .ok_or_else(|| SchedError::Config(format!("unknown engine id {}", engine.0)))?;
        let b = Binding {
            engine: engine.0,
            pin: pin.to_string(),
            net: net.to_string(),
        };
        match dir {
            Direction::Output => {
                let who = format!("{}.{pin}", e.name());
                if let Some(existing) = self.drivers.get(net) {
                    return Err(SchedError::MultipleDrivers {
                        net: net.to_string(),
                        existing: existing.clone(),
                        new: who,
                    });
                }
                self.drivers.insert(net.to_string(), who);
                self.outputs.push(b);
            }
            Direction::Input => self.inputs.push(b),
        }
        Ok(())
    }

    /// Current sim time (the last boundary).
    pub fn now(&self) -> Time {
        self.now
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Current value of a net.
    pub fn net(&self, net: &str) -> Option<&PinSignal> {
        self.nets.get(net)
    }

    /// The advance order (ADR-003 D3), known after the first `run`.
    pub fn plan(&self) -> Option<&Plan> {
        self.plan.as_ref()
    }

    /// Run until `now == t_end`. May be called again with a larger `t_end`.
    pub fn run<W: Write>(
        &mut self,
        t_end: Time,
        sink: &mut EventSink<W>,
        probes: &mut [&mut dyn Probe],
    ) -> Result<RunSummary, SchedError> {
        if self.phase == Phase::Error {
            return Err(SchedError::Halted);
        }
        match self.run_inner(t_end, sink, probes) {
            Ok(()) => {
                self.phase = Phase::Done;
                Ok(RunSummary {
                    now: self.now,
                    grants: self.grants,
                })
            }
            Err(e) => {
                self.phase = Phase::Error;
                // Every record of the grants already completed reaches the
                // sink's writer before the error returns (ADR-003 D4).
                let _ = sink.flush();
                Err(e)
            }
        }
    }

    fn run_inner<W: Write>(
        &mut self,
        t_end: Time,
        sink: &mut EventSink<W>,
        probes: &mut [&mut dyn Probe],
    ) -> Result<(), SchedError> {
        if self.phase == Phase::Init {
            if self.engines.is_empty() {
                return Err(SchedError::Config("no engines registered".into()));
            }
            self.lattice = self.check_lattice()?;
            self.build_plan()?;
        }
        if let Some(l) = self.lattice {
            if !t_end.as_fs().is_multiple_of(l.as_fs()) {
                return Err(SchedError::Config(format!(
                    "t_end {t_end} is not a multiple of the grant lattice {l}"
                )));
            }
        }
        let order = self
            .plan
            .as_ref()
            .expect("plan built at Init")
            .order
            .clone();
        let static_pos = positions(&order);
        let loop_quantum = self.loop_quantum;
        if self.phase == Phase::Init {
            self.phase = Phase::Exchange;
            let mut records = Vec::new();
            let mut changes = NetChanges::new();
            for i in 0..self.engines.len() {
                self.collect(i, Time::ZERO, Time::ZERO, true, &mut records, &mut changes)?;
            }
            self.changes = NetChanges::new();
            self.exchange(Time::ZERO, records, sink, probes, &static_pos)?;
        }
        while self.now < t_end {
            // Query
            self.phase = Phase::Query;
            let now = self.now;
            self.prev_nets.clone_from(&self.nets);
            let mut g = t_end;
            let mut limiter = Limiter::TEnd;
            // The engine with the smallest t_safe (first registered on a
            // tie): the only term that can round down to `now`.
            let mut min_safe: Option<(usize, Time)> = None;
            for (i, e) in self.engines.iter_mut().enumerate() {
                let t_safe = e.next_safe_time(now);
                if t_safe <= now {
                    return Err(SchedError::NoProgress {
                        engine: e.name().to_string(),
                        now,
                        t_safe,
                    });
                }
                if min_safe.is_none_or(|(_, t)| t_safe < t) {
                    min_safe = Some((i, t_safe));
                }
                consider(t_safe, Limiter::Engine(i), &mut g, &mut limiter);
            }
            let sample = probes
                .iter()
                .filter_map(|p| p.next_sample_time(now))
                .chain(
                    self.config
                        .event_sampling
                        .as_ref()
                        .map(|_| self.next_event_sample),
                )
                .filter(|t| *t > now)
                .min();
            if let Some(t) = sample {
                if let Some(l) = self.lattice {
                    if !t.as_fs().is_multiple_of(l.as_fs()) {
                        return Err(SchedError::Config(format!(
                            "aligned sample instant {t} is not a multiple of the grant lattice {l}"
                        )));
                    }
                }
                consider(t, Limiter::Sample, &mut g, &mut limiter);
            }
            consider(
                now.saturating_add(self.config.max_quantum),
                Limiter::MaxQuantum,
                &mut g,
                &mut limiter,
            );
            // ADR-003 D4: a feedback loop runs in one-cell lockstep (or
            // `loop_quantum` grants) in plan order, unless a quiet promise
            // lifts the cap for this grant (D5). Asking is useful only when
            // the cap binds.
            let mut quiet = None;
            let mut queried = Vec::new();
            if let Some(q) = loop_quantum {
                let capped = now.saturating_add(q);
                if self.config.lookahead && self.floor_l(g) > capped {
                    quiet = self.query_quiet(now, g, limiter, &mut queried);
                }
                if quiet.is_none() {
                    consider(capped, Limiter::LoopQuantum, &mut g, &mut limiter);
                }
            }

            // Grant
            self.phase = Phase::Grant;
            if let Some(qg) = &quiet {
                g = qg.g;
                limiter = qg.limiter;
            }
            // ADR-003 D2: round down to the lattice after the min. Every
            // term except an engine's t_safe is on the lattice, so only an
            // engine promising less than one cell can leave no progress.
            if let Some(l) = self.lattice {
                g = Time::from_fs(g.as_fs() / l.as_fs() * l.as_fs());
                if g <= now {
                    let (i, t_safe) = min_safe.expect("at least one engine");
                    return Err(SchedError::Lattice {
                        engine: self.engines[i].name().to_string(),
                        now,
                        t_safe,
                    });
                }
            }
            self.grants += 1;
            // A quiet grant's record waits for the early stop (below); no
            // other record of this grant is emitted before Exchange.
            if quiet.is_none() {
                self.emit_grant(sink, now, g, limiter)?;
            }

            // Advance, in plan order (ADR-003 D3), or in this grant's
            // order with an early stop (D5).
            self.phase = Phase::Advance;
            let grant_order = quiet.as_ref().map_or(&order, |qg| &qg.order).clone();
            let gpos = positions(&grant_order);
            let mut records: Vec<Record> = Vec::new();
            let mut changes = NetChanges::new();
            for (pos, &i) in grant_order.iter().enumerate() {
                // Pre-advance delivery: every change on this engine's inputs
                // made by an engine earlier in the order reaches it before it
                // advances, so it can act at the true time.
                let (base, trace) = self.input_trace(i, &changes, |d| gpos[d] < pos);
                if !trace.is_empty() {
                    let e = &mut self.engines[i];
                    e.apply_input_trace(&base, &trace)
                        .map_err(|source| engine_error(e.name(), Phase::Advance, source))?;
                }
                let e = &mut self.engines[i];
                match &quiet {
                    Some(qg) if pos == 0 => {
                        let r = e
                            .advance_until(g, &qg.watch)
                            .map_err(|source| engine_error(e.name(), Phase::Advance, source))?;
                        let on_lattice = self
                            .lattice
                            .is_none_or(|l| r.as_fs().is_multiple_of(l.as_fs()));
                        if r <= now || r > g || !(on_lattice || r == g) {
                            return Err(engine_error(
                                e.name(),
                                Phase::Advance,
                                EngineError::Protocol(format!(
                                    "advance_until({g}) from {now} returned {r}: not in \
                                     ({now}, {g}] on the lattice"
                                )),
                            ));
                        }
                        let lim = if r < g { Limiter::Stop(i) } else { qg.limiter };
                        g = r;
                        self.emit_grant(sink, now, g, lim)?;
                    }
                    _ => e
                        .advance_to(g)
                        .map_err(|source| engine_error(e.name(), Phase::Advance, source))?,
                }
                self.collect(i, now, g, false, &mut records, &mut changes)?;
                if let Some(qg) = &quiet {
                    if pos == 0 {
                        self.check_stop(i, qg, now, g, &changes)?;
                    }
                    if let Some(&(_, promised)) = qg.promises.iter().find(|(c, _)| *c == i) {
                        self.audit_quiet(i, promised, &changes)?;
                    }
                }
            }
            self.now = g;
            if let Some(log) = &mut self.plan_log {
                log.push(GrantPlan {
                    from: now,
                    until: g,
                    order: grant_order,
                    quiet: quiet
                        .as_ref()
                        .map(|qg| qg.promises.iter().map(|(c, _)| *c).collect())
                        .unwrap_or_default(),
                    stop: quiet.as_ref().map(|qg| qg.order[0]),
                    queried,
                });
            }

            // Exchange
            self.phase = Phase::Exchange;
            self.changes = changes;
            self.exchange(now, records, sink, probes, &gpos)?;
        }
        // Done: end-of-run self-checks (ADR-003 Amendment 1).
        for e in self.engines.iter_mut() {
            e.end_of_run().map_err(|source| SchedError::Engine {
                engine: e.name().to_string(),
                phase: Phase::Done,
                source,
            })?;
        }
        Ok(())
    }

    /// The grant lattice declared by the engines (ADR-003 D2): all declared
    /// lattices must be equal and positive, and `max_quantum`,
    /// `loop_quantum` and the event sampling period must be multiples of it.
    fn check_lattice(&self) -> Result<Option<Time>, SchedError> {
        let mut lattice: Option<(Time, &str)> = None;
        for e in &self.engines {
            let Some(l) = e.grant_lattice() else { continue };
            if l.is_zero() {
                return Err(SchedError::Config(format!(
                    "engine {}: grant lattice must be > 0",
                    e.name()
                )));
            }
            match lattice {
                Some((l0, n0)) if l0 != l => {
                    return Err(SchedError::Config(format!(
                        "engines {n0} and {} declare different grant lattices ({l0} vs {l})",
                        e.name()
                    )))
                }
                Some(_) => {}
                None => lattice = Some((l, e.name())),
            }
        }
        let Some((l, _)) = lattice else {
            return Ok(None);
        };
        let off = |what: &str, t: Time| {
            SchedError::Config(format!(
                "{what} {t} is not a multiple of the grant lattice {l}"
            ))
        };
        if !self.config.max_quantum.as_fs().is_multiple_of(l.as_fs()) {
            return Err(off("max_quantum", self.config.max_quantum));
        }
        if let Some(q) = self.config.loop_quantum {
            if !q.as_fs().is_multiple_of(l.as_fs()) {
                return Err(off("loop_quantum", q));
            }
        }
        if let Some(s) = &self.config.event_sampling {
            if !s.period.as_fs().is_multiple_of(l.as_fs()) {
                return Err(off("event sampling period", s.period));
            }
        }
        Ok(Some(l))
    }

    /// The grant lattice in effect (ADR-003 D2), known after the first `run`.
    pub fn lattice(&self) -> Option<Time> {
        self.lattice
    }

    /// Build the plan from the binding graph (ADR-003 D3) and fix the loop
    /// cap (D4).
    fn build_plan(&mut self) -> Result<(), SchedError> {
        let n = self.engines.len();
        let mut edges = BTreeSet::new();
        for o in &self.outputs {
            for i in self.inputs.iter().filter(|i| i.net == o.net) {
                edges.insert((o.engine, i.engine));
            }
        }
        let bound: Vec<bool> = self
            .engines
            .iter()
            .map(|e| e.grant_lattice().is_some())
            .collect();
        let plan = plan_order(n, &edges, &bound);
        self.driver = self
            .outputs
            .iter()
            .map(|o| (o.net.clone(), o.engine))
            .collect();
        // ADR-003 D5: an engine in a feedback loop is a lookahead candidate
        // if a quiet grant with Q = {it} is structurally possible (the check
        // is repeated per grant for the actual Q).
        let can_stop: Vec<bool> = self.engines.iter().map(|e| e.can_stop_early()).collect();
        let cyc = cyclic_members(n, &edges);
        self.candidate = (0..n)
            .map(|c| {
                let mut q = vec![false; n];
                q[c] = true;
                cyc[c] && quiet_order(n, &edges, &q, &can_stop).is_some()
            })
            .collect();
        self.backoff = vec![Backoff::default(); n];
        self.edges = edges;
        self.loop_quantum = if plan.cyclic {
            match self.config.loop_quantum.or(self.lattice) {
                Some(q) => Some(q),
                None => {
                    return Err(SchedError::Config(
                        "the binding graph has a cycle but no engine declares a grant \
                         lattice: set loop_quantum (ADR-003 D4)"
                            .into(),
                    ))
                }
            }
        } else {
            None
        };
        self.plan = Some(plan);
        Ok(())
    }

    /// Current input snapshot of engine `i`, keyed by its pin names.
    fn input_snapshot(&self, i: usize) -> PinSignals {
        let mut ins = PinSignals::new();
        for b in self.inputs.iter().filter(|b| b.engine == i) {
            if let Some(sig) = self.nets.get(&b.net) {
                ins.insert(b.pin.clone(), *sig);
            }
        }
        ins
    }

    /// `floor_L(t)` (the identity without a lattice).
    fn floor_l(&self, t: Time) -> Time {
        match self.lattice {
            Some(l) => Time::from_fs(t.as_fs() / l.as_fs() * l.as_fs()),
            None => t,
        }
    }

    /// Emit the `grant` record of `(now, g]`.
    fn emit_grant<W: Write>(
        &self,
        sink: &mut EventSink<W>,
        now: Time,
        g: Time,
        limiter: Limiter,
    ) -> Result<(), SchedError> {
        if !self.config.log_grants {
            return Ok(());
        }
        let limited_by = match limiter {
            Limiter::Engine(i) => format!("engine:{}", self.engines[i].name()),
            Limiter::Stop(i) => format!("stop:{}", self.engines[i].name()),
            Limiter::Sample => "sample".to_string(),
            Limiter::LoopQuantum => "loop_quantum".to_string(),
            Limiter::MaxQuantum => "max_quantum".to_string(),
            Limiter::TEnd => "t_end".to_string(),
        };
        sink.emit(&Event::Grant {
            t_fs: now,
            until_fs: g,
            limited_by,
        })?;
        Ok(())
    }

    /// ADR-003 D5 steps 1, 2 and 5: ask the lookahead candidates (unless
    /// backed off) for `quiet_until`, and build the quiet grant if the
    /// promises and the remaining graph allow one. `g` and `limiter` are the
    /// grant without the loop cap (not yet rounded). Every decision is a
    /// function of the simulated history.
    fn query_quiet(
        &mut self,
        now: Time,
        g: Time,
        limiter: Limiter,
        queried: &mut Vec<usize>,
    ) -> Option<QuietGrant> {
        let n = self.engines.len();
        let mut promises = Vec::new();
        for c in (0..n).filter(|&c| self.candidate[c]) {
            let b = &mut self.backoff[c];
            if b.skip > 0 {
                b.skip -= 1;
                continue;
            }
            queried.push(c);
            let q = self.engines[c].quiet_until(now);
            let useful = match self.lattice {
                Some(l) => q.as_fs() / l.as_fs() * l.as_fs() > now.as_fs(),
                None => q > now,
            };
            let b = &mut self.backoff[c];
            if useful {
                b.misses = 0;
                promises.push((c, q));
            } else {
                b.misses = b.misses.saturating_add(1);
                b.skip = 1u32
                    .checked_shl(b.misses)
                    .unwrap_or(u32::MAX)
                    .min(MAX_QUERY_SKIP);
            }
        }
        if promises.is_empty() {
            return None;
        }
        let (mut big_g, mut lim) = (g, limiter);
        for &(c, q) in &promises {
            consider(q, Limiter::Engine(c), &mut big_g, &mut lim);
        }
        let big_g = self.floor_l(big_g);
        // Q = {c : floor_L(quiet_until_c) >= G}.
        promises.retain(|&(_, q)| self.floor_l(q) >= big_g);
        let mut in_q = vec![false; n];
        for &(c, _) in &promises {
            in_q[c] = true;
        }
        let can_stop: Vec<bool> = self.engines.iter().map(|e| e.can_stop_early()).collect();
        let (order, p) = quiet_order(n, &self.edges, &in_q, &can_stop)?;
        // The producer's output pins whose nets an engine in Q reads through
        // an input that is not sampled (ADR-005 D3): a sampled input reads
        // its value only at sample instants at or after the promise, so a
        // change of it cannot end the quiet window.
        let mut watch = Vec::new();
        let mut watch_nets = Vec::new();
        for o in self.outputs.iter().filter(|o| o.engine == p) {
            let feeds_q = self.inputs.iter().any(|i| {
                i.net == o.net && in_q[i.engine] && !self.engines[i.engine].input_is_sampled(&i.pin)
            });
            if feeds_q && !watch.contains(&o.pin) {
                watch.push(o.pin.clone());
                watch_nets.push(o.net.clone());
            }
        }
        Some(QuietGrant {
            g: big_g,
            limiter: lim,
            order,
            promises,
            watch,
            watch_nets,
        })
    }

    /// ADR-003 D5.3: the early stop `r` of the first engine must be the first
    /// lattice point at or after its first change on a watched net, or `G`
    /// if there is none in `(now, r]`.
    fn check_stop(
        &self,
        i: usize,
        qg: &QuietGrant,
        now: Time,
        r: Time,
        changes: &NetChanges,
    ) -> Result<(), SchedError> {
        let first = qg
            .watch_nets
            .iter()
            .filter_map(|n| changes.get(n).and_then(|c| c.first()).map(|(t, _)| *t))
            .min();
        let expected = match first {
            Some(t) => {
                let c = match self.lattice {
                    Some(l) => Time::from_fs(t.as_fs().div_ceil(l.as_fs()) * l.as_fs()),
                    None => t,
                };
                c.min(qg.g)
            }
            None => qg.g,
        };
        if r != expected {
            return Err(engine_error(
                self.engines[i].name(),
                Phase::Advance,
                EngineError::Protocol(format!(
                    "advance_until stopped at {r}, but its first watched change in \
                     ({now}, {r}] is {first:?}: expected a stop at {expected} (ADR-003 D5.3)"
                )),
            ));
        }
        Ok(())
    }

    /// ADR-003 D5.4: a quiet engine reports no output change in the grant
    /// `(now, r]` before its promised time. The promise covers the open
    /// interval `(now, promised)`, and `r <= floor_L(promised)`, so the only
    /// change it allows is one exactly at `r = promised` (a lattice-bound
    /// CPU reports a write at a lattice point 1 fs later anyway). That change
    /// reaches the engines advanced before it at the boundary, where it takes
    /// effect from `r` on, as in a one-cell run.
    fn audit_quiet(
        &self,
        i: usize,
        promised: Time,
        changes: &NetChanges,
    ) -> Result<(), SchedError> {
        let first = self
            .outputs
            .iter()
            .filter(|o| o.engine == i)
            .filter_map(|o| changes.get(&o.net).and_then(|c| c.first()).map(|(t, _)| *t))
            .filter(|&t| t < promised)
            .min();
        match first {
            Some(at) => Err(SchedError::PromiseBroken {
                engine: self.engines[i].name().to_string(),
                promised,
                at,
            }),
            None => Ok(()),
        }
    }

    /// The changes of this grant on engine `i`'s input nets whose driver
    /// (registration index) satisfies `pick`, as a trace in time order (ties
    /// in input-binding order), plus the snapshot before them: picked nets at
    /// the previous boundary, the others at their current value.
    fn input_trace(
        &self,
        i: usize,
        changes: &NetChanges,
        pick: impl Fn(usize) -> bool,
    ) -> (PinSignals, Vec<Transition>) {
        let mut base = PinSignals::new();
        let mut trace = Vec::new();
        for b in self.inputs.iter().filter(|b| b.engine == i) {
            let picked = self.driver.get(&b.net).is_some_and(|&d| pick(d));
            let ch = changes.get(&b.net).filter(|_| picked);
            let before = if ch.is_some() {
                self.prev_nets.get(&b.net)
            } else {
                self.nets.get(&b.net)
            };
            if let Some(sig) = before {
                base.insert(b.pin.clone(), *sig);
            }
            for (t, sig) in ch.into_iter().flatten() {
                trace.push(Transition {
                    t: *t,
                    pin: b.pin.clone(),
                    signal: *sig,
                });
            }
        }
        trace.sort_by_key(|tr| tr.t);
        (base, trace)
    }

    /// Drain logs and read outputs of engine `i`, which has just reached the
    /// boundary `at` (previous boundary `prev`). Validates the trace (or the
    /// single transition per digital pin), updates the driven nets and
    /// records every change in `changes`.
    fn collect(
        &mut self,
        i: usize,
        prev: Time,
        at: Time,
        initial: bool,
        records: &mut Vec<Record>,
        changes: &mut NetChanges,
    ) -> Result<(), SchedError> {
        let e = &mut self.engines[i];

        let name = e.name().to_string();
        let engine_err = |source: EngineError| SchedError::Engine {
            engine: name.clone(),
            phase: Phase::Exchange,
            source,
        };
        for (k, rec) in e.drain_logs().into_iter().enumerate() {
            if rec.t > at {
                return Err(engine_err(EngineError::Protocol(format!(
                    "log record at {} is after boundary {at}",
                    rec.t
                ))));
            }
            let t = rec.t.max(prev);
            records.push((
                (t, i, 0, k),
                Event::Log {
                    t_fs: t,
                    source: name.clone(),
                    level: rec.level,
                    msg: rec.msg,
                },
            ));
        }
        let trace = if initial {
            None
        } else {
            e.read_output_trace().map_err(engine_err)?
        };
        let outs = e.read_outputs().map_err(engine_err)?;
        for (bi, b) in self
            .outputs
            .iter()
            .enumerate()
            .filter(|(_, b)| b.engine == i)
        {
            let driver = format!("{name}.{}", b.pin);
            let entries: Vec<&Transition> = trace
                .iter()
                .flatten()
                .filter(|tr| tr.pin == b.pin)
                .collect();
            let Some(sig) = outs.get(&b.pin) else {
                if !entries.is_empty() {
                    return Err(engine_err(EngineError::Protocol(format!(
                        "{driver}: trace reports changes but read_outputs omits the pin"
                    ))));
                }
                continue; // omitted: hold previous value
            };
            let old = self.nets.get(&b.net).copied();
            if trace.is_some()
                && entries.is_empty()
                && sig.is_digital()
                && old.as_ref() != Some(sig)
            {
                return Err(SchedError::Causality {
                    net: b.net.clone(),
                    detail: format!("{driver} changed to {sig:?} without a trace entry"),
                });
            }
            if entries.is_empty() {
                // One change per pin (an engine without a trace, a digital
                // pin the trace says is unchanged, or an analog pin reported
                // by its boundary value).
                if let Some(ev) =
                    check_transition(&b.net, &driver, old.as_ref(), sig, prev, at, initial)?
                {
                    records.push(((ev.time(), i, 1, bi), ev));
                }
                if old.as_ref() != Some(sig) {
                    let t = match sig {
                        PinSignal::Digital { edge_time, .. } => *edge_time,
                        PinSignal::Analog { .. } => at,
                    };
                    changes.entry(b.net.clone()).or_default().push((t, *sig));
                }
            } else {
                for (t, s) in check_trace(&b.net, &driver, old.as_ref(), sig, &entries, prev, at)? {
                    if let PinSignal::Digital { level, .. } = s {
                        records.push((
                            (t, i, 1, bi),
                            Event::PinEdge {
                                t_fs: t,
                                net: b.net.clone(),
                                driver: driver.clone(),
                                level: level.as_bit(),
                            },
                        ));
                    }
                    changes.entry(b.net.clone()).or_default().push((t, s));
                }
            }
            self.nets.insert(b.net.clone(), *sig);
        }
        Ok(())
    }

    /// Boundary exchange at `self.now`; `prev` is the previous boundary.
    /// `records` holds the log and edge records collected during Advance.
    fn exchange<W: Write>(
        &mut self,
        prev: Time,
        mut records: Vec<Record>,
        sink: &mut EventSink<W>,
        probes: &mut [&mut dyn Probe],
        pos: &[usize],
    ) -> Result<(), SchedError> {
        let now = self.now;

        // 1. Logs and outputs were collected per engine right after its
        //    advance (see `collect`).

        // 2. Route inputs to every engine: first the changes it has not seen
        //    yet (from an engine at or after it in this grant's order, which
        //    only a feedback loop has), every one in order, then the full
        //    snapshot.
        for (i, &here) in pos.iter().enumerate() {
            let (base, trace) = self.input_trace(i, &self.changes, |d| pos[d] >= here);
            let ins = self.input_snapshot(i);
            let e = &mut self.engines[i];
            let r = if trace.is_empty() {
                Ok(())
            } else {
                e.apply_input_trace(&base, &trace)
            };
            r.and_then(|()| e.apply_inputs(&ins))
                .map_err(|source| SchedError::Engine {
                    engine: e.name().to_string(),
                    phase: Phase::Exchange,
                    source,
                })?;
        }

        // 3. Periodic net samples.
        if let Some(s) = &self.config.event_sampling {
            let mut k = 0;
            while self.next_event_sample <= now {
                for net in &s.nets {
                    let (uv, level) = match self.nets.get(net) {
                        Some(PinSignal::Analog { volts }) => {
                            (Some(volts_to_microvolts(*volts)), None)
                        }
                        Some(PinSignal::Digital { level, .. }) => (None, Some(level.as_bit())),
                        None => (None, None),
                    };
                    records.push((
                        (self.next_event_sample, usize::MAX, 0, k),
                        Event::NetSample {
                            t_fs: self.next_event_sample,
                            net: net.clone(),
                            uv,
                            level,
                        },
                    ));
                    k += 1;
                }
                match self.next_event_sample.checked_add(s.period) {
                    Some(t) => self.next_event_sample = t,
                    None => {
                        self.next_event_sample = Time::MAX;
                        break;
                    }
                }
            }
        }

        // 4. Emit sorted by (t_fs, registration index, pin): never by plan
        //    order (ADR-003 D3).
        records.sort_by_key(|r| r.0);
        for (_, ev) in &records {
            sink.emit(ev)?;
        }

        // 5. Probes.
        let sampler = WindowSampler {
            engines: &self.engines,
            outputs: &self.outputs,
            nets: &self.nets,
            prev_nets: &self.prev_nets,
            changes: &self.changes,
        };
        for p in probes.iter_mut() {
            p.on_window(prev, now, &self.nets, &sampler)?;
        }
        Ok(())
    }
}

/// The plan order (ADR-003 D3) of `n` engines for the binding edges `(j,
/// i)` ("an output of j drives a net i reads"): strongly connected
/// components in Kahn order, ties by the lowest registration index; inside a
/// component, lattice-bound engines (`bound[i]`) first, then registration
/// index. A pure function of its arguments.
fn plan_order(n: usize, edges: &BTreeSet<(usize, usize)>, bound: &[bool]) -> Plan {
    let mut succ = vec![Vec::new(); n];
    let mut self_loop = false;
    for &(j, i) in edges {
        if i == j {
            self_loop = true;
        } else {
            succ[j].push(i);
        }
    }
    // reach[a][b]: b is reachable from a (engine counts are small).
    let mut reach = vec![vec![false; n]; n];
    for (a, row) in reach.iter_mut().enumerate() {
        let mut stack = vec![a];
        row[a] = true;
        while let Some(v) = stack.pop() {
            for &w in &succ[v] {
                if !row[w] {
                    row[w] = true;
                    stack.push(w);
                }
            }
        }
    }
    // Components, numbered by their lowest member.
    let mut comp = vec![usize::MAX; n];
    let mut members: Vec<Vec<usize>> = Vec::new();
    for a in 0..n {
        if comp[a] != usize::MAX {
            continue;
        }
        let id = members.len();
        let m: Vec<usize> = (0..n).filter(|&b| reach[a][b] && reach[b][a]).collect();
        for &b in &m {
            comp[b] = id;
        }
        members.push(m);
    }
    let c = members.len();
    let mut csucc = vec![BTreeSet::new(); c];
    let mut indeg = vec![0usize; c];
    for &(j, i) in edges {
        let (cj, ci) = (comp[j], comp[i]);
        if cj != ci && csucc[cj].insert(ci) {
            indeg[ci] += 1;
        }
    }
    // Kahn; a component's id is its lowest registration index, so the
    // ready set ordered by id breaks ties by the lowest index.
    let mut ready: BTreeSet<usize> = (0..c).filter(|&k| indeg[k] == 0).collect();
    let mut order = Vec::with_capacity(n);
    while let Some(k) = ready.pop_first() {
        let mut m = members[k].clone();
        m.sort_by_key(|&e| (!bound[e], e));
        order.extend(m);
        for &s in &csucc[k] {
            indeg[s] -= 1;
            if indeg[s] == 0 {
                ready.insert(s);
            }
        }
    }
    let cyclic = self_loop || members.iter().any(|m| m.len() > 1);
    Plan { order, cyclic }
}

/// Net values inside the last grant window `[prev, now]`: the driving
/// engine's [`Engine::sample_output`] if it has one; otherwise, for a
/// digital net, its level from the window's changes (the trace); otherwise
/// the boundary value.
struct WindowSampler<'a> {
    engines: &'a [Box<dyn Engine>],
    outputs: &'a [Binding],
    nets: &'a NetValues,
    prev_nets: &'a NetValues,
    changes: &'a NetChanges,
}

impl WindowSampler<'_> {
    fn digital(&self, net: &str) -> Option<&[(Time, PinSignal)]> {
        self.changes
            .get(net)
            .filter(|ch| ch.iter().all(|(_, s)| s.is_digital()))
            .map(Vec::as_slice)
    }
}

impl NetSampler for WindowSampler<'_> {
    fn sample(&self, net: &str, t: Time) -> Option<PinSignal> {
        if let Some(b) = self.outputs.iter().find(|b| b.net == net) {
            if let Some(v) = self.engines[b.engine].sample_output(&b.pin, t) {
                return Some(v);
            }
        }
        if let Some(ch) = self.digital(net) {
            return match ch.iter().rev().find(|(ti, _)| *ti <= t) {
                Some((_, s)) => Some(*s),
                None => self.prev_nets.get(net).copied(),
            };
        }
        self.nets.get(net).copied()
    }

    fn digital_changes(&self, net: &str) -> Vec<PinSignal> {
        self.digital(net)
            .map(|ch| ch.iter().map(|(_, s)| *s).collect())
            .unwrap_or_default()
    }
}

/// The engines in a feedback loop: in a strongly connected component of
/// more than one engine, or reading a net they drive.
fn cyclic_members(n: usize, edges: &BTreeSet<(usize, usize)>) -> Vec<bool> {
    let mut succ = vec![Vec::new(); n];
    for &(j, i) in edges {
        succ[j].push(i);
    }
    // a is in a loop iff a is reachable from one of its successors.
    (0..n)
        .map(|a| {
            let mut seen = vec![false; n];
            let mut stack: Vec<usize> = succ[a].clone();
            while let Some(v) = stack.pop() {
                if v == a {
                    return true;
                }
                if !seen[v] {
                    seen[v] = true;
                    stack.extend(&succ[v]);
                }
            }
            false
        })
        .collect()
}

/// ADR-003 D5.2: the advance order of a quiet grant with quiet set `q`, and
/// the engine that stops early, or `None` if the grant must fall back to one
/// cell. With the out-edges of every engine in `q` removed, the remaining
/// graph must be acyclic; exactly one engine outside `q` may drive a net
/// read by an engine in `q`; it must be able to stop early; and it must be
/// first in the remaining graph's Kahn order (ties by registration index).
/// A pure function of its arguments.
fn quiet_order(
    n: usize,
    edges: &BTreeSet<(usize, usize)>,
    q: &[bool],
    can_stop: &[bool],
) -> Option<(Vec<usize>, usize)> {
    let producers: BTreeSet<usize> = edges
        .iter()
        .filter(|&&(j, i)| q[i] && !q[j])
        .map(|&(j, _)| j)
        .collect();
    if producers.len() != 1 {
        return None;
    }
    let p = *producers.first()?;
    if !can_stop[p] {
        return None;
    }
    let mut succ = vec![Vec::new(); n];
    let mut indeg = vec![0usize; n];
    for &(j, i) in edges.iter().filter(|&&(j, _)| !q[j]) {
        if i == j {
            return None; // a self-loop remains
        }
        succ[j].push(i);
        indeg[i] += 1;
    }
    let mut ready: BTreeSet<usize> = (0..n).filter(|&k| indeg[k] == 0).collect();
    let mut order = Vec::with_capacity(n);
    while let Some(k) = ready.pop_first() {
        order.push(k);
        for &s in &succ[k] {
            indeg[s] -= 1;
            if indeg[s] == 0 {
                ready.insert(s);
            }
        }
    }
    if order.len() < n || order[0] != p {
        return None; // a cycle remains, or the producer is not first
    }
    Some((order, p))
}

/// `pos[i]`: the position of engine `i` in `order`.
fn positions(order: &[usize]) -> Vec<usize> {
    let mut pos = vec![0; order.len()];
    for (p, &i) in order.iter().enumerate() {
        pos[i] = p;
    }
    pos
}

/// An engine error during `phase`; a broken quiet promise the engine
/// reported (ADR-003 D5.4) becomes [`SchedError::PromiseBroken`].
fn engine_error(engine: &str, phase: Phase, source: EngineError) -> SchedError {
    match source {
        EngineError::PromiseBroken { promised, at } => SchedError::PromiseBroken {
            engine: engine.to_string(),
            promised,
            at,
        },
        source => SchedError::Engine {
            engine: engine.to_string(),
            phase,
            source,
        },
    }
}

/// Lower `g` to `t` if `t` is earlier (ties go to the lower-ranked limiter).
fn consider(t: Time, l: Limiter, g: &mut Time, lim: &mut Limiter) {
    if t < *g || (t == *g && rank(&l) < rank(lim)) {
        *g = t;
        *lim = l;
    }
}

fn rank(l: &Limiter) -> (u8, usize) {
    match l {
        Limiter::Engine(i) | Limiter::Stop(i) => (0, *i),
        Limiter::Sample => (1, 0),
        Limiter::LoopQuantum => (2, 0),
        Limiter::MaxQuantum => (3, 0),
        Limiter::TEnd => (4, 0),
    }
}

/// Validate one pin's trace entries for the grant `(prev, now]` against the
/// net's previous value `old` and the pin's boundary value `sig` (invariant
/// 5, ADR-003 D3): times strictly increasing inside the grant, a digital
/// entry's `edge_time` equal to its time, levels alternating starting from
/// the previous level, and the last entry equal to the boundary value.
/// Returns the changes as `(t, signal)`.
fn check_trace(
    net: &str,
    driver: &str,
    old: Option<&PinSignal>,
    sig: &PinSignal,
    entries: &[&Transition],
    prev: Time,
    now: Time,
) -> Result<Vec<(Time, PinSignal)>, SchedError> {
    let causality = |detail: String| SchedError::Causality {
        net: net.to_string(),
        detail,
    };
    let mut last_t = prev;
    let mut last = old.copied();
    let mut out = Vec::with_capacity(entries.len());
    for tr in entries {
        if tr.t <= last_t || tr.t > now {
            return Err(causality(format!(
                "{driver} trace entry at {} is not after {last_t} inside the grant ({prev}, {now}]",
                tr.t
            )));
        }
        match (last, tr.signal) {
            (
                Some(PinSignal::Digital { level: pl, .. }),
                PinSignal::Digital { level, edge_time },
            ) => {
                if edge_time != tr.t {
                    return Err(causality(format!(
                        "{driver} trace entry at {} carries edge_time {edge_time}",
                        tr.t
                    )));
                }
                if level == pl {
                    return Err(causality(format!(
                        "{driver} trace entry at {} repeats level {level:?}",
                        tr.t
                    )));
                }
            }
            (None | Some(PinSignal::Analog { .. }), PinSignal::Analog { .. }) => {}
            (p, s) => {
                return Err(causality(format!(
                    "{driver} trace changes signal kind ({p:?} -> {s:?}) at {}",
                    tr.t
                )))
            }
        }
        last_t = tr.t;
        last = Some(tr.signal);
        out.push((tr.t, tr.signal));
    }
    if last.as_ref() != Some(sig) {
        return Err(causality(format!(
            "{driver} boundary value {sig:?} differs from its last trace entry {last:?}"
        )));
    }
    Ok(out)
}

/// Validate a net's new value against its previous one and produce a
/// `pin_edge` event for a digital level change.
fn check_transition(
    net: &str,
    driver: &str,
    prev_sig: Option<&PinSignal>,
    sig: &PinSignal,
    prev: Time,
    now: Time,
    initial: bool,
) -> Result<Option<Event>, SchedError> {
    let causality = |detail: String| SchedError::Causality {
        net: net.to_string(),
        detail,
    };
    match (prev_sig, sig) {
        (_, PinSignal::Digital { edge_time, .. }) if *edge_time > now => Err(causality(format!(
            "{driver} reports edge at {edge_time}, after boundary {now}"
        ))),
        (None, _) | (Some(PinSignal::Analog { .. }), PinSignal::Analog { .. }) => Ok(None),
        (
            Some(PinSignal::Digital {
                level: pl,
                edge_time: pe,
            }),
            PinSignal::Digital { level, edge_time },
        ) => {
            if pl == level {
                if pe != edge_time {
                    return Err(causality(format!(
                        "{driver} level unchanged but edge_time moved {pe} -> {edge_time}: \
                         more than one edge inside one grant (t_safe too large)"
                    )));
                }
                return Ok(None);
            }
            if initial || *edge_time <= prev {
                return Err(causality(format!(
                    "{driver} edge at {edge_time} outside grant ({prev}, {now}]"
                )));
            }
            Ok(Some(Event::PinEdge {
                t_fs: *edge_time,
                net: net.to_string(),
                driver: driver.to_string(),
                level: level.as_bit(),
            }))
        }
        (Some(p), s) => Err(causality(format!(
            "{driver} changed signal kind ({p:?} -> {s:?})"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order(n: usize, edges: &[(usize, usize)], bound: &[bool]) -> Plan {
        plan_order(n, &edges.iter().copied().collect(), bound)
    }

    #[test]
    fn plan_dag_puts_producers_first_with_index_ties() {
        // 2 -> 0, 2 -> 1, 3 isolated: [2, 0, 1, 3]; 0 and 1 tie by index.
        let p = order(4, &[(2, 0), (2, 1)], &[false; 4]);
        assert_eq!(p.order, vec![2, 0, 1, 3]);
        assert!(!p.cyclic);
        // A chain registered backwards: 2 -> 1 -> 0.
        let p = order(3, &[(2, 1), (1, 0)], &[false; 3]);
        assert_eq!(p.order, vec![2, 1, 0]);
        // No edges: registration order.
        assert_eq!(order(3, &[], &[true, false, true]).order, vec![0, 1, 2]);
    }

    #[test]
    fn plan_cycle_is_one_component_lattice_bound_first() {
        // 0 <-> 1 (cycle), 1 -> 2; lattice-bound 1 comes before 0.
        let p = order(3, &[(0, 1), (1, 0), (1, 2)], &[false, true, false]);
        assert_eq!(p.order, vec![1, 0, 2]);
        assert!(p.cyclic);
        // Same cycle without lattice-bound engines: registration index.
        let p = order(3, &[(0, 1), (1, 0), (1, 2)], &[false; 3]);
        assert_eq!(p.order, vec![0, 1, 2]);
        // A producer feeding the cycle comes first even if registered last.
        let p = order(3, &[(0, 1), (1, 0), (2, 0)], &[false; 3]);
        assert_eq!(p.order, vec![2, 0, 1]);
    }

    #[test]
    fn plan_self_loop_is_cyclic() {
        let p = order(2, &[(1, 1), (1, 0)], &[false; 2]);
        assert!(p.cyclic);
        assert_eq!(p.order, vec![1, 0]);
    }
}
