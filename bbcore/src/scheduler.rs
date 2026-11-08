//! Conservative lockstep scheduler (the time-grant protocol).
//!
//! Normative description: `spec/time-grant.md`; rationale:
//! `docs/adr/002-time-grant-protocol.md`.
//!
//! ```text
//! Init -> Query -> Grant -> Advance -> Exchange -> Query -> ... -> Done
//!                                                      \-> Error (any failure)
//! ```
//!
//! Each iteration: `g = min(t_safe_i, next probe/sample time, now + max_quantum,
//! t_end)`; every engine advances to exactly `g`; outputs are read, routed by
//! net name and applied as inputs. Only integer [`Time`] takes part in the
//! decision, and every collection is ordered, so runs are deterministic.
//!
//! Advance is Gauss-Seidel ordered (ADR-002 Amendment 2): engines advance in
//! registration order, each engine's outputs are read right after its
//! advance, and an engine whose bound input nets changed earlier in the same
//! grant receives `apply_inputs` *before* its own `advance_to(g)`, so it can
//! apply an edge at its true `edge_time` in `(now, g]`.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};

use crate::engine::{Engine, EngineError};
use crate::event::{Event, EventSink};
use crate::probe::{NetSampler, Probe};
use crate::signal::{volts_to_microvolts, NetValues, PinSignal, PinSignals};
use crate::time::Time;

/// Index of an engine registered with [`Scheduler::add_engine`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EngineId(usize);

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
    /// Upper bound on any single grant. Must be > 0.
    pub max_quantum: Time,
    /// Emit a `grant` record per grant.
    pub log_grants: bool,
    /// Emit `net_sample` records at a fixed rate (grants are aligned to it).
    pub event_sampling: Option<EventSampling>,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        SchedulerConfig {
            max_quantum: Time::from_ms(1),
            log_grants: true,
            event_sampling: None,
        }
    }
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
    #[error("causality violation on net {net}: {detail}")]
    Causality { net: String, detail: String },
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
}

/// What bounded a grant, in tie-break order.
enum Limiter {
    Engine(usize),
    Sample,
    MaxQuantum,
    TEnd,
}

impl Scheduler {
    pub fn new(config: SchedulerConfig) -> Result<Self, SchedError> {
        if config.max_quantum.is_zero() {
            return Err(SchedError::Config("max_quantum must be > 0".into()));
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
        })
    }

    /// Register an engine. Engines are queried, advanced and exchanged in
    /// registration order.
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
            self.phase = Phase::Exchange;
            let mut events = Vec::new();
            let mut changed = BTreeSet::new();
            for i in 0..self.engines.len() {
                self.collect(i, Time::ZERO, Time::ZERO, true, &mut events, &mut changed)?;
            }
            self.exchange(Time::ZERO, events, sink, probes)?;
        }
        while self.now < t_end {
            // Query
            self.phase = Phase::Query;
            let now = self.now;
            self.prev_nets.clone_from(&self.nets);
            let mut g = t_end;
            let mut limiter = Limiter::TEnd;
            for (i, e) in self.engines.iter_mut().enumerate() {
                let t_safe = e.next_safe_time(now);
                if t_safe <= now {
                    return Err(SchedError::NoProgress {
                        engine: e.name().to_string(),
                        now,
                        t_safe,
                    });
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
                consider(t, Limiter::Sample, &mut g, &mut limiter);
            }
            consider(
                now.saturating_add(self.config.max_quantum),
                Limiter::MaxQuantum,
                &mut g,
                &mut limiter,
            );

            // Grant
            self.phase = Phase::Grant;
            self.grants += 1;
            if self.config.log_grants {
                let limited_by = match limiter {
                    Limiter::Engine(i) => format!("engine:{}", self.engines[i].name()),
                    Limiter::Sample => "sample".to_string(),
                    Limiter::MaxQuantum => "max_quantum".to_string(),
                    Limiter::TEnd => "t_end".to_string(),
                };
                sink.emit(&Event::Grant {
                    t_fs: now,
                    until_fs: g,
                    limited_by,
                })?;
            }

            // Advance (Gauss-Seidel order, ADR-002 Amendment 2)
            self.phase = Phase::Advance;
            let mut events: Vec<Event> = Vec::new();
            let mut changed: BTreeSet<String> = BTreeSet::new();
            for i in 0..self.engines.len() {
                // Intra-grant delivery: an input net changed by an engine
                // earlier in the order during this grant reaches this engine
                // before it advances, so it can act at the true edge_time.
                if self
                    .inputs
                    .iter()
                    .any(|b| b.engine == i && changed.contains(&b.net))
                {
                    let ins = self.input_snapshot(i);
                    let e = &mut self.engines[i];
                    e.apply_inputs(&ins).map_err(|source| SchedError::Engine {
                        engine: e.name().to_string(),
                        phase: Phase::Advance,
                        source,
                    })?;
                }
                let e = &mut self.engines[i];
                e.advance_to(g).map_err(|source| SchedError::Engine {
                    engine: e.name().to_string(),
                    phase: Phase::Advance,
                    source,
                })?;
                self.collect(i, now, g, false, &mut events, &mut changed)?;
            }
            self.now = g;

            // Exchange
            self.phase = Phase::Exchange;
            self.exchange(now, events, sink, probes)?;
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

    /// Drain logs and read outputs of engine `i`, which has just reached the
    /// boundary `at` (previous boundary `prev`). Updates the driven nets,
    /// validates digital transitions and records every net whose value
    /// changed in `changed`.
    fn collect(
        &mut self,
        i: usize,
        prev: Time,
        at: Time,
        initial: bool,
        events: &mut Vec<Event>,
        changed: &mut BTreeSet<String>,
    ) -> Result<(), SchedError> {
        let e = &mut self.engines[i];

        let name = e.name().to_string();
        for rec in e.drain_logs() {
            if rec.t > at {
                return Err(SchedError::Engine {
                    engine: name,
                    phase: Phase::Exchange,
                    source: EngineError::Protocol(format!(
                        "log record at {} is after boundary {at}",
                        rec.t
                    )),
                });
            }
            events.push(Event::Log {
                t_fs: rec.t.max(prev),
                source: name.clone(),
                level: rec.level,
                msg: rec.msg,
            });
        }
        let outs = e.read_outputs().map_err(|source| SchedError::Engine {
            engine: name.clone(),
            phase: Phase::Exchange,
            source,
        })?;
        for b in self.outputs.iter().filter(|b| b.engine == i) {
            let Some(sig) = outs.get(&b.pin) else {
                continue; // omitted: hold previous value
            };
            let driver = format!("{name}.{}", b.pin);
            let old = self.nets.get(&b.net);
            if let Some(ev) = check_transition(&b.net, &driver, old, sig, prev, at, initial)? {
                events.push(ev);
            }
            if old != Some(sig) {
                changed.insert(b.net.clone());
            }
            self.nets.insert(b.net.clone(), *sig);
        }
        Ok(())
    }

    /// Boundary exchange at `self.now`; `prev` is the previous boundary.
    /// `events` holds the log and edge records collected during Advance.
    fn exchange<W: Write>(
        &mut self,
        prev: Time,
        mut events: Vec<Event>,
        sink: &mut EventSink<W>,
        probes: &mut [&mut dyn Probe],
    ) -> Result<(), SchedError> {
        let now = self.now;

        // 1. Logs and outputs were collected per engine right after its
        //    advance (see `collect`).

        // 2. Route inputs (full snapshot) to every engine.
        for i in 0..self.engines.len() {
            let ins = self.input_snapshot(i);
            let e = &mut self.engines[i];
            e.apply_inputs(&ins).map_err(|source| SchedError::Engine {
                engine: e.name().to_string(),
                phase: Phase::Exchange,
                source,
            })?;
        }

        // 3. Periodic net samples.
        if let Some(s) = &self.config.event_sampling {
            while self.next_event_sample <= now {
                for net in &s.nets {
                    let (uv, level) = match self.nets.get(net) {
                        Some(PinSignal::Analog { volts }) => {
                            (Some(volts_to_microvolts(*volts)), None)
                        }
                        Some(PinSignal::Digital { level, .. }) => (None, Some(level.as_bit())),
                        None => (None, None),
                    };
                    events.push(Event::NetSample {
                        t_fs: self.next_event_sample,
                        net: net.clone(),
                        uv,
                        level,
                    });
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

        // 4. Emit in time order (stable: ties keep engine / pin order).
        events.sort_by_key(Event::time);
        for ev in &events {
            sink.emit(ev)?;
        }

        // 5. Probes.
        let sampler = WindowSampler {
            engines: &self.engines,
            outputs: &self.outputs,
            nets: &self.nets,
            prev_nets: &self.prev_nets,
        };
        for p in probes.iter_mut() {
            p.on_window(prev, now, &self.nets, &sampler)?;
        }
        Ok(())
    }
}

/// Net values inside the last grant window `[prev, now]`: the driving
/// engine's [`Engine::sample_output`] if it has one, otherwise the boundary
/// value, with a digital net read as its previous level before `edge_time`.
struct WindowSampler<'a> {
    engines: &'a [Box<dyn Engine>],
    outputs: &'a [Binding],
    nets: &'a NetValues,
    prev_nets: &'a NetValues,
}

impl NetSampler for WindowSampler<'_> {
    fn sample(&self, net: &str, t: Time) -> Option<PinSignal> {
        if let Some(b) = self.outputs.iter().find(|b| b.net == net) {
            if let Some(v) = self.engines[b.engine].sample_output(&b.pin, t) {
                return Some(v);
            }
        }
        match self.nets.get(net) {
            Some(PinSignal::Digital { edge_time, .. }) if t < *edge_time => {
                self.prev_nets.get(net).copied()
            }
            v => v.copied(),
        }
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
        Limiter::Engine(i) => (0, *i),
        Limiter::Sample => (1, 0),
        Limiter::MaxQuantum => (2, 0),
        Limiter::TEnd => (3, 0),
    }
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
