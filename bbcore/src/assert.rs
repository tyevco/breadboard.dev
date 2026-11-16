//! Assertions (ADR-010 D5-D7, Amendment 3): the closed set of kinds, their
//! windows, and the kernel's evaluator.
//!
//! An assertion is data: a [`Spec`] of one [`Kind`] over a [`Window`]. The
//! kernel evaluates every assertion of a run deterministically
//! ([`Assertions::window`], called by the scheduler at every exchange) and
//! emits exactly one `assert` record per assertion at its **verdict time**,
//! the close of its window (its `to`, its `until` event, or the run's end).
//!
//! What a check reads:
//!
//! - **analog** values come from the producing engine's accepted solver
//!   points ([`crate::Engine::solver_points`]): an extremum is taken over the
//!   linear interpolant of the solver points (the points inside the window
//!   and its two interpolated ends), a crossing is interpolated between the
//!   two straddling points and rounded up to whole fs (the `digital_in`
//!   machinery, ADR-003 D3), and a value at an instant is the interpolation
//!   [`value_at`] that `sample_output` uses. The monitors are passive: they
//!   add no element, no input and no grant boundary, so their results are
//!   pure functions of the solver points, which the grant lattice makes
//!   partition independent (ADR-003 D2). A glitch narrower than one solver
//!   step (at most TMAX = L) is missed, exactly as for `digital_in`;
//! - **digital** checks read the `pin_edge` stream;
//! - `log` and `event` checks read the `log` and `reset` records.
//!
//! Values and limits are integers in base units (`uV`, `fs`, `count`), so
//! the stream stays free of floats: a voltage is rounded half away from
//! zero to whole µV ([`volts_to_microvolts`]), a crossing time up to whole
//! fs, and the comparison is made on the integers.
//!
//! [`replay`] evaluates the same specs on a recorded event stream (`bb
//! assert`): every kind that reads only records gives the record the run
//! gave; a kind that reads solver points is `not_evaluable` there.

use std::collections::BTreeMap;

use crate::engine::SolverPoint;
use crate::event::{AssertOutcome, Event, LogLevel};
use crate::signal::{volts_to_microvolts, NetValues, PinSignal};
use crate::time::Time;

/// The base unit of a value and its limits (ADR-010 D7).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Unit {
    Uv,
    Fs,
    Count,
}

impl Unit {
    pub fn as_str(self) -> &'static str {
        match self {
            Unit::Uv => "uV",
            Unit::Fs => "fs",
            Unit::Count => "count",
        }
    }

    fn fmt(self, v: i64) -> String {
        match self {
            Unit::Uv => fmt_uv(v),
            Unit::Fs => fmt_fs(v),
            Unit::Count => v.to_string(),
        }
    }
}

/// An analog quantity a check reads: `V(net)`, or `V(net) - V(minus)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signal {
    pub net: String,
    pub minus: Option<String>,
}

impl Signal {
    fn label(&self) -> String {
        match &self.minus {
            Some(m) => format!("V({}) - V({m})", self.net),
            None => format!("V({})", self.net),
        }
    }
}

/// A `log` pattern: a literal, optionally anchored with `^` (at the start of
/// the message) and `$` (at its end). The subset of a regular expression
/// this build implements; every other metacharacter is refused, so a later
/// full regex cannot change what a pattern written today means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pattern {
    pub literal: String,
    pub start: bool,
    pub end: bool,
}

/// Characters a pattern may not contain (outside the two anchors).
pub const PATTERN_META: &str = ".*+?[](){}|\\";

impl Pattern {
    pub fn parse(s: &str) -> Result<Pattern, String> {
        let start = s.starts_with('^');
        let rest = if start { &s[1..] } else { s };
        let end = rest.ends_with('$');
        let literal = if end { &rest[..rest.len() - 1] } else { rest };
        if let Some(c) = literal
            .chars()
            .find(|c| PATTERN_META.contains(*c) || *c == '^' || *c == '$')
        {
            return Err(format!(
                "pattern {s:?}: {c:?} is a regular-expression metacharacter; this build matches a \
                 literal anchored with ^ and $ only (ADR-010 Amendment 3)"
            ));
        }
        if literal.is_empty() {
            return Err(format!("pattern {s:?}: empty"));
        }
        Ok(Pattern {
            literal: literal.to_string(),
            start,
            end,
        })
    }

    pub fn matches(&self, msg: &str) -> bool {
        match (self.start, self.end) {
            (true, true) => msg == self.literal,
            (true, false) => msg.starts_with(&self.literal),
            (false, true) => msg.ends_with(&self.literal),
            (false, false) => msg.contains(&self.literal),
        }
    }

    fn text(&self) -> String {
        format!(
            "{}{}{}",
            if self.start { "^" } else { "" },
            self.literal,
            if self.end { "$" } else { "" }
        )
    }
}

/// An event a window or an `edge_timing` check is defined by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sel {
    /// A change of a digital net to `level`.
    Edge { net: String, level: u8 },
    /// A `log` record matching `pattern` (from `source`, if given).
    Log {
        pattern: Pattern,
        source: Option<String>,
    },
    /// An analog crossing of `threshold_uv` (downwards when `below`): the
    /// value goes from at or above the threshold to below it (`below`), or
    /// from at or below it to above it, between two solver points.
    Cross {
        sig: Signal,
        below: bool,
        threshold_uv: i64,
    },
}

impl Sel {
    fn label(&self) -> String {
        match self {
            Sel::Edge { net, level } => {
                format!("{net} {}", if *level == 1 { "rise" } else { "fall" })
            }
            Sel::Log { pattern, source } => match source {
                Some(s) => format!("{s} log {:?}", pattern.text()),
                None => format!("log {:?}", pattern.text()),
            },
            Sel::Cross {
                sig,
                below,
                threshold_uv,
            } => format!(
                "{} {} {}",
                sig.label(),
                if *below { "below" } else { "above" },
                fmt_uv(*threshold_uv)
            ),
        }
    }
}

/// When a check is evaluated (ADR-010 D5 windows). The window is the closed
/// interval from its opening to its close: it opens at `from` (default 0),
/// or with `after` at the first `after` event at or after `from` plus its
/// delay; it closes at `to`, at the first `until` event at or after the
/// opening, or at the run's end, whichever is first. With `while`, only the
/// parts of it where a digital net is at a level count.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Window {
    pub from: Time,
    pub to: Option<Time>,
    pub after: Option<(Sel, Time)>,
    pub until: Option<Sel>,
    pub while_level: Option<(String, u8)>,
}

/// A check's limits, in its unit; at least one is given.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub struct Limits {
    pub min: Option<i64>,
    pub max: Option<i64>,
}

impl Limits {
    fn holds(&self, v: i64) -> bool {
        self.min.is_none_or(|m| v >= m) && self.max.is_none_or(|m| v <= m)
    }

    /// The value to report from the instances' minimum and maximum: the
    /// side that is violated, else the side with the smaller margin (the
    /// maximum on a tie), with its limit.
    fn report(&self, lo: (i64, Time), hi: (i64, Time)) -> ((i64, Time), Option<i64>) {
        match (self.min, self.max) {
            (None, Some(mx)) => (hi, Some(mx)),
            (Some(mn), None) => (lo, Some(mn)),
            (Some(mn), Some(mx)) => {
                if hi.0 > mx {
                    (hi, Some(mx))
                } else if lo.0 < mn {
                    (lo, Some(mn))
                } else if (mx - hi.0) <= (lo.0 - mn) {
                    (hi, Some(mx))
                } else {
                    (lo, Some(mn))
                }
            }
            (None, None) => (hi, None),
        }
    }
}

/// What `at` reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AtWhat {
    Analog(Signal),
    /// A digital net's level (`count`).
    Digital(String),
}

/// When `at` reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AtWhen {
    /// At every change of a digital net to `level` in the window.
    Edge { net: String, level: u8 },
    /// At one instant.
    Time(Time),
}

/// One side of a `settle` band.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BandEdge {
    /// µV.
    Abs(i64),
    /// Parts per million of the value at the event.
    Rel(i64),
}

/// How `edge_timing` pairs its events.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Pair {
    /// Each `end` event with the last `start` event at or before it.
    Last,
    /// Each `end` event with the first `start` event since the `end` net
    /// last changed (either level; for a `log` or crossing `end`, since the
    /// previous `end` event).
    First,
}

/// What an `event` check counts.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EventWhat {
    /// `reset` records.
    Reset,
    /// `log` records of level `warn`.
    Warn,
    /// `log` records of level `error`.
    Error,
}

/// The closed set of kinds (ADR-010 D5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// An analog quantity within limits over the window (µV).
    Range { sig: Signal, lim: Limits },
    /// A value at each instant (µV, or a level as a count).
    At {
        what: AtWhat,
        when: AtWhen,
        lim: Limits,
    },
    /// After each change of `net` to `level`: inside the band within
    /// `within`, and staying inside until the net changes again (or the
    /// window closes). The value is the settle time (fs).
    Settle {
        sig: Signal,
        net: String,
        level: u8,
        within: Time,
        lo: Option<BandEdge>,
        hi: Option<BandEdge>,
    },
    /// The gap from paired `start` to `end` events (fs).
    EdgeTiming {
        start: Sel,
        end: Sel,
        pair: Pair,
        lim: Limits,
    },
    /// The number of changes of a digital net (to `level`, if given).
    Count {
        net: String,
        level: Option<u8>,
        lim: Limits,
    },
    /// The period between consecutive changes of a digital net to `level`
    /// (fs).
    Frequency { net: String, level: u8, lim: Limits },
    /// `log` records matching a pattern: their number (`count`), or with
    /// `field` the number after `<field>=` or `<field> ` in each one, in
    /// `unit`.
    Log {
        pattern: Pattern,
        source: Option<String>,
        field: Option<(String, Unit)>,
        lim: Limits,
    },
    /// The number of `reset`, `warn` or `error` records.
    Event { what: EventWhat, lim: Limits },
}

impl Kind {
    pub fn name(&self) -> &'static str {
        match self {
            Kind::Range { .. } => "range",
            Kind::At { .. } => "at",
            Kind::Settle { .. } => "settle",
            Kind::EdgeTiming { .. } => "edge_timing",
            Kind::Count { .. } => "count",
            Kind::Frequency { .. } => "frequency",
            Kind::Log { .. } => "log",
            Kind::Event { .. } => "event",
        }
    }

    fn unit(&self) -> Unit {
        match self {
            Kind::Range { .. } => Unit::Uv,
            Kind::At {
                what: AtWhat::Analog(_),
                ..
            } => Unit::Uv,
            Kind::At { .. } | Kind::Count { .. } | Kind::Event { .. } => Unit::Count,
            Kind::Settle { .. } | Kind::EdgeTiming { .. } | Kind::Frequency { .. } => Unit::Fs,
            Kind::Log { field, .. } => field.as_ref().map_or(Unit::Count, |f| f.1),
        }
    }
}

/// One assertion (ADR-010 D5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spec {
    pub name: String,
    pub kind: Kind,
    pub window: Window,
    /// The rung the check needs (ADR-010 D6): below it, `not_evaluable`.
    pub requires: Option<String>,
    /// A description, the start of the record's `msg`.
    pub msg: Option<String>,
}

impl Spec {
    /// The analog nets whose solver points the check reads.
    pub fn analog_nets(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut sig = |s: &Signal| {
            out.push(s.net.clone());
            if let Some(m) = &s.minus {
                out.push(m.clone());
            }
        };
        match &self.kind {
            Kind::Range { sig: s, .. } | Kind::Settle { sig: s, .. } => sig(s),
            Kind::At {
                what: AtWhat::Analog(s),
                ..
            } => sig(s),
            Kind::EdgeTiming { start, end, .. } => {
                for e in [start, end] {
                    if let Sel::Cross { sig: s, .. } = e {
                        sig(s);
                    }
                }
            }
            _ => {}
        }
        out
    }

    /// The digital nets the check reads (for their initial levels).
    pub fn digital_nets(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut sel = |s: &Sel| {
            if let Sel::Edge { net, .. } = s {
                out.push(net.clone());
            }
        };
        if let Some((s, _)) = &self.window.after {
            sel(s);
        }
        if let Some(s) = &self.window.until {
            sel(s);
        }
        match &self.kind {
            Kind::EdgeTiming { start, end, .. } => {
                sel(start);
                sel(end);
            }
            Kind::At { what, when, .. } => {
                if let AtWhat::Digital(n) = what {
                    out.push(n.clone());
                }
                if let AtWhen::Edge { net, .. } = when {
                    out.push(net.clone());
                }
            }
            Kind::Settle { net, .. } | Kind::Count { net, .. } | Kind::Frequency { net, .. } => {
                out.push(net.clone())
            }
            _ => {}
        }
        if let Some((n, _)) = &self.window.while_level {
            out.push(n.clone());
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Formatting (integers only, so a message never depends on float formatting)
// ---------------------------------------------------------------------------

/// `-0.728195 V` from µV.
pub fn fmt_uv(uv: i64) -> String {
    let sign = if uv < 0 { "-" } else { "" };
    let a = uv.unsigned_abs();
    format!("{sign}{}.{:06} V", a / 1_000_000, a % 1_000_000)
}

/// `45.26 ms` from fs: exact, trailing zeros trimmed.
pub fn fmt_fs(fs: i64) -> String {
    let sign = if fs < 0 { "-" } else { "" };
    let a = fs.unsigned_abs();
    let ms = a / 1_000_000_000_000;
    let frac = a % 1_000_000_000_000;
    if frac == 0 {
        format!("{sign}{ms} ms")
    } else {
        let f = format!("{frac:012}");
        format!("{sign}{ms}.{} ms", f.trim_end_matches('0'))
    }
}

fn fmt_t(t: Time) -> String {
    fmt_fs(t.as_fs() as i64)
}

// ---------------------------------------------------------------------------
// Solver-point arithmetic (bit-identical to the ngspice engine's)
// ---------------------------------------------------------------------------

/// The value at `t` of the linear interpolant through `pts` (sorted by
/// time): exactly at a point's time, that point (the last of several with the
/// same fs time); between two points, linear in the solver's own seconds,
/// as `NgspiceEngine::sample` interpolates. `None` outside the points.
pub fn value_at(pts: &[SolverPoint], t: Time) -> Option<f64> {
    let (first, last) = (pts.first()?, pts.last()?);
    if t < first.t || t > last.t {
        return None;
    }
    match pts.binary_search_by_key(&t, |p| p.t) {
        Ok(mut k) => {
            while k + 1 < pts.len() && pts[k + 1].t == t {
                k += 1;
            }
            Some(pts[k].v)
        }
        Err(k) => {
            let (a, b) = (&pts[k - 1], &pts[k]);
            if b.secs <= a.secs {
                return Some(b.v);
            }
            let x = (t.as_secs_f64() - a.secs) / (b.secs - a.secs);
            Some(a.v + (b.v - a.v) * x.clamp(0.0, 1.0))
        }
    }
}

/// The crossing of `threshold` between two solver points, in whole fs
/// rounded up, kept strictly after `a` and not after `b` (the `digital_in`
/// rule, ADR-003 D3).
fn crossing(a: &SolverPoint, b: &SolverPoint, threshold: f64) -> Time {
    let frac = if b.v == a.v {
        1.0
    } else {
        ((threshold - a.v) / (b.v - a.v)).clamp(0.0, 1.0)
    };
    let tx = a.secs + frac * (b.secs - a.secs);
    let fs = if tx <= 0.0 {
        0
    } else {
        (tx * 1e15).ceil() as u64
    };
    let lo = a.t.as_fs() + 1;
    Time::from_fs(fs.max(lo).min(b.t.as_fs().max(lo)))
}

/// The points of a signal: one net's, or the difference of two nets' (which
/// must share their solver times: the same engine).
fn signal_points(
    points: &BTreeMap<String, Vec<SolverPoint>>,
    sig: &Signal,
) -> Result<Vec<SolverPoint>, String> {
    let a = points
        .get(&sig.net)
        .ok_or_else(|| format!("no solver points for {}", sig.net))?;
    let Some(m) = &sig.minus else {
        return Ok(a.clone());
    };
    let b = points
        .get(m)
        .ok_or_else(|| format!("no solver points for {m}"))?;
    if a.len() != b.len()
        || a.iter()
            .zip(b)
            .any(|(x, y)| x.t != y.t || x.secs.to_bits() != y.secs.to_bits())
    {
        return Err(format!(
            "{} and {m} do not share their solver points (a difference needs both nets from one \
             analog engine)",
            sig.net
        ));
    }
    Ok(a.iter()
        .zip(b)
        .map(|(x, y)| SolverPoint {
            t: x.t,
            secs: x.secs,
            v: x.v - y.v,
        })
        .collect())
}

// ---------------------------------------------------------------------------
// The evaluator
// ---------------------------------------------------------------------------

/// A `pin_edge` of the window.
#[derive(Clone, Debug)]
pub struct EdgeRec<'a> {
    pub t: Time,
    pub net: &'a str,
    pub level: u8,
}

/// A `log` (or `reset`, `level = None`) record of the window.
#[derive(Clone, Debug)]
pub struct LogRec<'a> {
    pub t: Time,
    pub source: &'a str,
    /// `None` for a `reset` record.
    pub level: Option<LogLevel>,
    pub msg: &'a str,
}

/// What one exchange hands the evaluator: the window `(prev, now]` (at the
/// initial exchange `prev == now == 0`), its records in stream order, and
/// the solver points of every analog net the assertions read, from the
/// point at `prev` to the point at `now`.
pub struct WindowInput<'a> {
    pub prev: Time,
    pub now: Time,
    pub edges: Vec<EdgeRec<'a>>,
    pub logs: Vec<LogRec<'a>>,
    pub points: &'a BTreeMap<String, Vec<SolverPoint>>,
    /// The initial exchange's net values (digital levels at t = 0).
    pub initial: Option<&'a NetValues>,
}

/// One `settle` instance.
#[derive(Clone, Debug)]
struct SettleInst {
    t_ev: Time,
    lo: Option<f64>,
    hi: Option<f64>,
    deadline: Time,
    /// The last point seen (time, secs, value), from the value at the event.
    last: SolverPoint,
    /// When the signal last entered the band (or the event, if inside then).
    t_in: Option<Time>,
    /// Whether it left the band after entering it before the deadline.
    ended: Option<Time>,
}

#[derive(Clone, Debug, Default)]
struct State {
    open: Option<Time>,
    close: Option<Time>,
    /// The window never opened (an `after` event that did not happen).
    done: bool,
    /// Instance values `(value, t)` (at, edge_timing, frequency, log field).
    vals: Vec<(i64, Time)>,
    /// range: the extremes `(µV, t)` and the number of values read.
    lo: Option<(i64, Time)>,
    hi: Option<(i64, Time)>,
    n: i64,
    /// settle: instances not yet decided, and the decided ones
    /// `(outcome, settle time, event time)`.
    open_inst: Vec<SettleInst>,
    settled: Vec<(AssertOutcome, Option<i64>, Time)>,
    /// edge_timing: pairing state.
    last_start: Option<Time>,
    first_since: Option<Time>,
    unpaired: u64,
    /// frequency: the previous edge.
    prev_edge: Option<Time>,
    /// A crossing signal's last point (edge_timing `Cross`), per side.
    cross_last: [Option<SolverPoint>; 2],
    /// An error found while evaluating (reported as `fail`).
    error: Option<String>,
}

/// The kernel's assertion evaluator: one per run.
pub struct Assertions {
    specs: Vec<Spec>,
    /// The analog nets the specs read, sorted.
    analog: Vec<String>,
    states: Vec<State>,
    t_end: Time,
    rung: String,
    /// `bb assert`: no solver points; analog kinds are not evaluable.
    replay: bool,
    /// Digital levels at the start of the current window.
    levels: BTreeMap<String, u8>,
}

/// The fidelity ladder (ADR-006 D3), for `requires`.
pub const LADDER: &[&str] = &["fast", "pi", "si", "em"];

impl Assertions {
    /// `t_end` is the run's end (every open window closes there); `rung` is
    /// the run's rung (ADR-009 D1).
    pub fn new(specs: Vec<Spec>, t_end: Time, rung: &str) -> Result<Assertions, String> {
        let mut names: Vec<&str> = Vec::new();
        for s in &specs {
            if names.contains(&s.name.as_str()) {
                return Err(format!("assertion {:?} given twice", s.name));
            }
            names.push(&s.name);
            if let Some(r) = &s.requires {
                if !LADDER.contains(&r.as_str()) {
                    return Err(format!(
                        "{}: requires {r:?}: not a rung ({})",
                        s.name,
                        LADDER.join(", ")
                    ));
                }
            }
            if s.window.from > t_end {
                return Err(format!(
                    "{}: the window opens at {} , after the run's end {}",
                    s.name,
                    fmt_t(s.window.from),
                    fmt_t(t_end)
                ));
            }
        }
        if !LADDER.contains(&rung) {
            return Err(format!("rung {rung:?}: not on the ladder"));
        }
        let states = vec![State::default(); specs.len()];
        let mut analog: Vec<String> = specs.iter().flat_map(Spec::analog_nets).collect();
        analog.sort();
        analog.dedup();
        Ok(Assertions {
            specs,
            analog,
            states,
            t_end,
            rung: rung.to_string(),
            replay: false,
            levels: BTreeMap::new(),
        })
    }

    pub fn specs(&self) -> &[Spec] {
        &self.specs
    }

    /// The analog nets whose solver points the evaluator needs (sorted).
    pub fn analog_nets(&self) -> &[String] {
        &self.analog
    }

    fn needs_points(&self, k: usize) -> bool {
        !self.specs[k].analog_nets().is_empty()
    }

    /// Evaluate one exchange's window; returns the `assert` records decided
    /// in it, in assertion order: `t_fs` in `[prev, now)`, at the run's end
    /// `[prev, now]` (a window closing exactly at a boundary is decided at
    /// the next exchange, which may still bring a log at that instant).
    pub fn window(&mut self, w: &WindowInput<'_>) -> Vec<Event> {
        // Digital levels at the window's start.
        if let Some(nets) = w.initial {
            for s in &self.specs {
                for n in s.digital_nets() {
                    if let Some(PinSignal::Digital { level, .. }) = nets.get(&n) {
                        self.levels.insert(n, level.as_bit());
                    }
                }
            }
        }
        let start_levels = self.levels.clone();
        let mut by_net: BTreeMap<&str, Vec<(Time, u8)>> = BTreeMap::new();
        for e in &w.edges {
            by_net.entry(e.net).or_default().push((e.t, e.level));
        }
        let ctx = Ctx {
            w,
            start: &start_levels,
            by_net: &by_net,
        };
        let mut out = Vec::new();
        for k in 0..self.specs.len() {
            if let Some(ev) = self.step(k, &ctx) {
                out.push(ev);
            }
        }
        for (n, es) in &by_net {
            if let Some(&(_, lv)) = es.last() {
                self.levels.insert((*n).to_string(), lv);
            }
        }
        out
    }

    fn step(&mut self, k: usize, c: &Ctx<'_, '_>) -> Option<Event> {
        let spec = self.specs[k].clone();
        let replay_na = self.replay && self.needs_points(k);
        let st = &mut self.states[k];
        if st.done {
            return None;
        }
        let w = c.w;
        // The window's records lie in [prev, now]: a log reported at a
        // boundary keeps its time, the window's start. (An edge is always
        // after prev; the solver point at prev is the previous window's last,
        // read again only where that is harmless: an extremum, a crossing's
        // start point.)
        let lo_t = w.prev;
        // 1. The window's opening and close.
        if st.open.is_none() {
            match &spec.window.after {
                None => st.open = Some(spec.window.from),
                Some((sel, delay)) => {
                    if let Some(t) = c.first_event(sel, spec.window.from.max(lo_t), w.now) {
                        st.open = Some(t.saturating_add(*delay));
                    }
                }
            }
        }
        if let (Some(open), None) = (st.open, st.close) {
            let mut close = spec.window.to.unwrap_or(Time::MAX).min(self.t_end);
            if let Some(sel) = &spec.window.until {
                if let Some(t) = c.first_event(sel, open.max(lo_t), w.now) {
                    close = close.min(t);
                }
            }
            if close <= w.now {
                st.close = Some(close.max(open));
            }
        }
        // 2. The active intervals of this window, and the kind's update.
        if !replay_na && st.error.is_none() {
            if let Some(open) = st.open {
                let a = open.max(lo_t);
                let b = st.close.unwrap_or(Time::MAX).min(w.now);
                if a <= b && open <= self.t_end {
                    let ivs = c.intervals(&spec.window.while_level, a, b);
                    if let Err(e) = update(&spec, st, c, &ivs, (a, b)) {
                        st.error = Some(e);
                    }
                }
            }
        }
        // 3. The verdict, at the close (or at the run's end). A window that
        // closes exactly at this boundary waits for the next exchange, whose
        // records may include a log reported at that instant; its record
        // still has the close's time, so the stream does not depend on where
        // the boundaries fall.
        let close = match st.close {
            Some(t) if t < w.now || (t == w.now && w.now >= self.t_end) => t,
            _ if w.now >= self.t_end => self.t_end,
            _ => return None,
        };
        st.done = true;
        let rung = self.rung.clone();
        let mut rec = verdict(&spec, st, close, replay_na, self.t_end);
        if let Some(req) = &spec.requires {
            let need = LADDER.iter().position(|r| r == req).unwrap_or(0);
            let have = LADDER.iter().position(|r| *r == rung).unwrap_or(0);
            if have < need {
                rec = Verdict {
                    outcome: AssertOutcome::NotEvaluable,
                    value: None,
                    limit: None,
                    detail: format!("requires the {req} rung; the run's is {rung}"),
                };
            }
        }
        let unit = spec.kind.unit();
        let msg = match &spec.msg {
            Some(m) => format!("{m}: {}", rec.detail),
            None => rec.detail,
        };
        Some(Event::Assert {
            t_fs: close,
            name: spec.name.clone(),
            outcome: rec.outcome,
            pass: match rec.outcome {
                AssertOutcome::NotEvaluable => None,
                o => Some(o == AssertOutcome::Pass),
            },
            value: rec.value,
            limit: rec.limit,
            unit: (rec.value.is_some() || rec.limit.is_some()).then(|| unit.as_str().to_string()),
            rung,
            msg,
        })
    }
}

struct Ctx<'w, 'a> {
    w: &'w WindowInput<'a>,
    start: &'w BTreeMap<String, u8>,
    by_net: &'w BTreeMap<&'a str, Vec<(Time, u8)>>,
}

impl Ctx<'_, '_> {
    /// The level of a digital net at `t` (after its edges at or before `t`).
    fn level_at(&self, net: &str, t: Time) -> Option<u8> {
        let mut lv = self.start.get(net).copied();
        for &(te, l) in self.by_net.get(net).map(Vec::as_slice).unwrap_or(&[]) {
            if te > t {
                break;
            }
            lv = Some(l);
        }
        lv
    }

    /// The first event of `sel` in `[a, b]` (records only: an edge or a log).
    fn first_event(&self, sel: &Sel, a: Time, b: Time) -> Option<Time> {
        match sel {
            Sel::Edge { net, level } => self
                .w
                .edges
                .iter()
                .find(|e| e.t >= a && e.t <= b && e.net == net && e.level == *level)
                .map(|e| e.t),
            Sel::Log { pattern, source } => self
                .w
                .logs
                .iter()
                .find(|l| l.t >= a && l.t <= b && log_matches(l, pattern, source))
                .map(|l| l.t),
            Sel::Cross { .. } => None,
        }
    }

    /// The closed intervals of `[a, b]` where the `while` net is at its
    /// level (all of it without one).
    fn intervals(&self, wl: &Option<(String, u8)>, a: Time, b: Time) -> Vec<(Time, Time)> {
        let Some((net, level)) = wl else {
            return vec![(a, b)];
        };
        let mut out = Vec::new();
        let mut cur = self.level_at(net, a);
        let mut since = a;
        for &(te, l) in self
            .by_net
            .get(net.as_str())
            .map(Vec::as_slice)
            .unwrap_or(&[])
        {
            if te <= a || te > b {
                continue;
            }
            if cur == Some(*level) && l != *level {
                out.push((since, te));
            }
            if l == *level && cur != Some(*level) {
                since = te;
            }
            cur = Some(l);
        }
        if cur == Some(*level) {
            out.push((since, b));
        }
        out
    }

    fn inside(&self, ivs: &[(Time, Time)], t: Time) -> bool {
        ivs.iter().any(|&(a, b)| a <= t && t <= b)
    }
}

fn log_matches(l: &LogRec<'_>, pattern: &Pattern, source: &Option<String>) -> bool {
    l.level.is_some() && source.as_ref().is_none_or(|s| s == l.source) && pattern.matches(l.msg)
}

/// The number after `<field>=` or `<field> ` in `msg`, in `unit`: an
/// integer for a count, a decimal with a unit (`50 ms`, `400 us`, `1.2 V`)
/// for a time or a voltage, converted exactly.
pub fn log_field(msg: &str, field: &str, unit: Unit) -> Option<i64> {
    let mut from = 0;
    while let Some(p) = msg[from..].find(field) {
        let at = from + p;
        from = at + field.len();
        let before_ok = at == 0 || !msg.as_bytes()[at - 1].is_ascii_alphanumeric();
        let rest = &msg[at + field.len()..];
        let rest = match rest.strip_prefix('=').or_else(|| rest.strip_prefix(' ')) {
            Some(r) if before_ok => r,
            _ => continue,
        };
        return parse_quantity(rest, unit);
    }
    None
}

fn parse_quantity(s: &str, unit: Unit) -> Option<i64> {
    let b = s.as_bytes();
    let mut i = 0;
    let neg = b.first() == Some(&b'-');
    if neg {
        i += 1;
    }
    let int_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let int = &s[int_start..i];
    let mut frac = "";
    if i < b.len() && b[i] == b'.' {
        let f0 = i + 1;
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        frac = &s[f0..i];
    }
    if int.is_empty() && frac.is_empty() {
        return None;
    }
    let rest = s[i..].trim_start_matches(' ');
    let word: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    let exp10: i32 = match unit {
        Unit::Count => {
            if !frac.is_empty() {
                return None;
            }
            0
        }
        Unit::Fs => match word.as_str() {
            "fs" => 0,
            "ps" => 3,
            "ns" => 6,
            "us" => 9,
            "ms" => 12,
            "s" => 15,
            _ => return None,
        },
        Unit::Uv => match word.as_str() {
            "uV" => 0,
            "mV" => 3,
            "V" => 6,
            _ => return None,
        },
    };
    if frac.len() as i32 > exp10 {
        return None; // finer than the base unit
    }
    let digits = format!("{int}{frac}");
    let mut v: i64 = digits.parse().ok()?;
    for _ in 0..(exp10 - frac.len() as i32) {
        v = v.checked_mul(10)?;
    }
    Some(if neg { -v } else { v })
}

/// Apply this window's records and points to the kind's state.
fn update(
    spec: &Spec,
    st: &mut State,
    c: &Ctx<'_, '_>,
    ivs: &[(Time, Time)],
    (a, b): (Time, Time),
) -> Result<(), String> {
    let w = c.w;
    match &spec.kind {
        Kind::Range { sig, .. } => {
            let pts = signal_points(w.points, sig)?;
            for &(x, y) in ivs {
                let mut take = |v: f64, t: Time| {
                    let uv = volts_to_microvolts(v);
                    if st.lo.is_none_or(|(m, _)| uv < m) {
                        st.lo = Some((uv, t));
                    }
                    if st.hi.is_none_or(|(m, _)| uv > m) {
                        st.hi = Some((uv, t));
                    }
                    st.n += 1;
                };
                if let Some(v) = value_at(&pts, x) {
                    take(v, x);
                }
                for p in pts.iter().filter(|p| p.t > x && p.t < y) {
                    take(p.v, p.t);
                }
                if y > x {
                    if let Some(v) = value_at(&pts, y) {
                        take(v, y);
                    }
                }
            }
        }
        Kind::At { what, when, .. } => {
            let instants: Vec<Time> = match when {
                AtWhen::Edge { net, level } => w
                    .edges
                    .iter()
                    .filter(|e| e.net == net && e.level == *level && c.inside(ivs, e.t))
                    .map(|e| e.t)
                    .collect(),
                AtWhen::Time(t) => {
                    // Once: the previous window already read an instant at
                    // this window's start.
                    let fresh = *t > w.prev || w.prev == w.now;
                    if fresh && c.inside(ivs, *t) && *t >= a && *t <= b {
                        vec![*t]
                    } else {
                        vec![]
                    }
                }
            };
            for t in instants {
                let v = match what {
                    AtWhat::Analog(sig) => {
                        let pts = signal_points(w.points, sig)?;
                        let v = value_at(&pts, t)
                            .ok_or_else(|| format!("no solver point around {}", fmt_t(t)))?;
                        volts_to_microvolts(v)
                    }
                    AtWhat::Digital(net) => i64::from(
                        c.level_at(net, t)
                            .ok_or_else(|| format!("{net} has no level at {}", fmt_t(t)))?,
                    ),
                };
                st.vals.push((v, t));
            }
        }
        Kind::Settle {
            sig,
            net,
            level,
            within,
            lo,
            hi,
        } => {
            let pts = signal_points(w.points, sig)?;
            // Events of this window, then every instance over the points.
            let evs: Vec<Time> = w
                .edges
                .iter()
                .filter(|e| e.net == net && e.level == *level && c.inside(ivs, e.t))
                .map(|e| e.t)
                .collect();
            let changes: Vec<Time> = w
                .edges
                .iter()
                .filter(|e| e.net == net)
                .map(|e| e.t)
                .collect();
            let mut new = Vec::new();
            for t_ev in evs {
                let v = value_at(&pts, t_ev)
                    .ok_or_else(|| format!("no solver point around {}", fmt_t(t_ev)))?;
                let band = |e: &Option<BandEdge>| {
                    e.map(|e| match e {
                        BandEdge::Abs(uv) => uv as f64 / 1e6,
                        BandEdge::Rel(ppm) => v * ppm as f64 / 1e6,
                    })
                };
                let (blo, bhi) = (band(lo), band(hi));
                let inside = blo.is_none_or(|l| v >= l) && bhi.is_none_or(|h| v <= h);
                new.push(SettleInst {
                    t_ev,
                    lo: blo,
                    hi: bhi,
                    deadline: t_ev.saturating_add(*within),
                    last: SolverPoint {
                        t: t_ev,
                        secs: t_ev.as_secs_f64(),
                        v,
                    },
                    t_in: inside.then_some(t_ev),
                    ended: None,
                });
            }
            st.open_inst.extend(new);
            let mut keep = Vec::new();
            for mut inst in std::mem::take(&mut st.open_inst) {
                // The instance ends at the next change of its net, or the
                // window's end in this exchange.
                let end_here = changes.iter().copied().find(|&t| t > inst.t_ev);
                let stop = end_here.unwrap_or(b).min(b);
                let from = inst.last.t;
                for p in pts.iter().filter(|p| p.t > from && p.t <= stop) {
                    let inn =
                        |v: f64| inst.lo.is_none_or(|l| v >= l) && inst.hi.is_none_or(|h| v <= h);
                    let (was, now_in) = (inn(inst.last.v), inn(p.v));
                    if !was && now_in {
                        let thr = if inst.hi.is_some_and(|h| inst.last.v > h) {
                            inst.hi.unwrap()
                        } else {
                            inst.lo.unwrap_or(p.v)
                        };
                        inst.t_in = Some(crossing(&inst.last, p, thr));
                    } else if was && !now_in {
                        inst.t_in = None;
                    }
                    inst.last = *p;
                }
                if end_here.is_some_and(|t| t <= b) {
                    inst.ended = end_here;
                    st.settled
                        .push(settle_outcome(&inst, end_here.unwrap(), false, Time::MAX));
                } else {
                    keep.push(inst);
                }
            }
            st.open_inst = keep;
        }
        Kind::EdgeTiming {
            start, end, pair, ..
        } => {
            // Every start, end and reset event of this window, in time order:
            // resets (an `end` net's other level) first, then starts, then
            // ends at equal times.
            let mut evs: Vec<(Time, u8)> = Vec::new(); // 0 reset, 1 start, 2 end
            for (side, sel) in [(1u8, start), (2u8, end)] {
                match sel {
                    Sel::Edge { net, level } => {
                        for e in w
                            .edges
                            .iter()
                            .filter(|e| e.net == net && c.inside(ivs, e.t))
                        {
                            if e.level == *level {
                                evs.push((e.t, side));
                            } else if side == 2 {
                                evs.push((e.t, 0));
                            }
                        }
                    }
                    Sel::Log { pattern, source } => {
                        for l in w
                            .logs
                            .iter()
                            .filter(|l| c.inside(ivs, l.t) && log_matches(l, pattern, source))
                        {
                            evs.push((l.t, side));
                        }
                    }
                    Sel::Cross {
                        sig,
                        below,
                        threshold_uv,
                    } => {
                        let pts = signal_points(w.points, sig)?;
                        let thr = *threshold_uv as f64 / 1e6;
                        let slot = usize::from(side - 1);
                        for p in &pts {
                            if let Some(q) = st.cross_last[slot] {
                                if p.t > q.t {
                                    let hit = if *below {
                                        q.v >= thr && p.v < thr
                                    } else {
                                        q.v <= thr && p.v > thr
                                    };
                                    if hit {
                                        let t = crossing(&q, p, thr);
                                        if c.inside(ivs, t) {
                                            evs.push((t, side));
                                        }
                                    }
                                }
                            }
                            if st.cross_last[slot].is_none_or(|q| p.t > q.t) {
                                st.cross_last[slot] = Some(*p);
                            }
                        }
                    }
                }
            }
            evs.sort();
            for (t, kind) in evs {
                match kind {
                    0 => st.first_since = None,
                    1 => {
                        st.last_start = Some(t);
                        if st.first_since.is_none() {
                            st.first_since = Some(t);
                        }
                    }
                    _ => {
                        let s = match pair {
                            Pair::Last => st.last_start,
                            Pair::First => st.first_since,
                        };
                        match s {
                            Some(s) => st.vals.push(((t.as_fs() - s.as_fs()) as i64, t)),
                            None => st.unpaired += 1,
                        }
                        st.first_since = None;
                    }
                }
            }
        }
        Kind::Count { net, level, .. } => {
            st.n += w
                .edges
                .iter()
                .filter(|e| {
                    e.net == net && level.is_none_or(|l| e.level == l) && c.inside(ivs, e.t)
                })
                .count() as i64;
        }
        Kind::Frequency { net, level, .. } => {
            for e in w.edges.iter().filter(|e| e.net == net && e.level == *level) {
                if !c.inside(ivs, e.t) {
                    st.prev_edge = None;
                    continue;
                }
                if let Some(p) = st.prev_edge {
                    st.vals.push(((e.t.as_fs() - p.as_fs()) as i64, e.t));
                }
                st.prev_edge = Some(e.t);
            }
        }
        Kind::Log {
            pattern,
            source,
            field,
            ..
        } => {
            for l in w
                .logs
                .iter()
                .filter(|l| c.inside(ivs, l.t) && log_matches(l, pattern, source))
            {
                match field {
                    None => st.n += 1,
                    Some((f, u)) => {
                        let v = log_field(l.msg, f, *u).ok_or_else(|| {
                            format!(
                                "{:?} at {}: no {f} field in {}",
                                l.msg,
                                fmt_t(l.t),
                                u.as_str()
                            )
                        })?;
                        st.vals.push((v, l.t));
                    }
                }
            }
        }
        Kind::Event { what, .. } => {
            st.n += w
                .logs
                .iter()
                .filter(|l| c.inside(ivs, l.t))
                .filter(|l| match what {
                    EventWhat::Reset => l.level.is_none(),
                    EventWhat::Warn => l.level == Some(LogLevel::Warn),
                    EventWhat::Error => l.level == Some(LogLevel::Error),
                })
                .count() as i64;
        }
    }
    Ok(())
}

/// A settle instance's outcome when it ends at `end` (`run_end`: the run's
/// end, whose `t_end` is given).
fn settle_outcome(
    inst: &SettleInst,
    end: Time,
    run_end: bool,
    t_end: Time,
) -> (AssertOutcome, Option<i64>, Time) {
    let st = inst.t_in.map(|t| (t.as_fs() - inst.t_ev.as_fs()) as i64);
    if run_end && inst.deadline > t_end {
        return (AssertOutcome::NotEvaluable, st, inst.t_ev);
    }
    let _ = end;
    let ok = inst.t_in.is_some_and(|t| t <= inst.deadline);
    (
        if ok {
            AssertOutcome::Pass
        } else {
            AssertOutcome::Fail
        },
        st,
        inst.t_ev,
    )
}

struct Verdict {
    outcome: AssertOutcome,
    value: Option<i64>,
    limit: Option<i64>,
    detail: String,
}

fn lim_text(lim: &Limits, unit: Unit) -> String {
    match (lim.min, lim.max) {
        (Some(a), Some(b)) => format!("limits {} to {}", unit.fmt(a), unit.fmt(b)),
        (Some(a), None) => format!("limit >= {}", unit.fmt(a)),
        (None, Some(b)) => format!("limit <= {}", unit.fmt(b)),
        (None, None) => "no limit".into(),
    }
}

fn window_text(w: &Window, close: Time) -> String {
    let mut s = String::new();
    match &w.after {
        Some((sel, d)) if d.is_zero() => s.push_str(&format!("after the first {}", sel.label())),
        Some((sel, d)) => s.push_str(&format!("{} after the first {}", fmt_t(*d), sel.label())),
        None if w.from.is_zero() => s.push_str("from 0"),
        None => s.push_str(&format!("from {}", fmt_t(w.from))),
    }
    s.push_str(&format!(" to {}", fmt_t(close)));
    if let Some((n, l)) = &w.while_level {
        s.push_str(&format!(" while {n} = {l}"));
    }
    s
}

fn verdict(spec: &Spec, st: &mut State, close: Time, replay_na: bool, t_end: Time) -> Verdict {
    let unit = spec.kind.unit();
    let na = |detail: String| Verdict {
        outcome: AssertOutcome::NotEvaluable,
        value: None,
        limit: None,
        detail,
    };
    if replay_na {
        return na(format!(
            "{} reads solver points; bb assert replays the recorded records only",
            spec.kind.name()
        ));
    }
    let fail = |detail: String| Verdict {
        outcome: AssertOutcome::Fail,
        value: None,
        limit: None,
        detail,
    };
    if let Some(e) = &st.error {
        return fail(e.clone());
    }
    if st.open.is_none_or(|o| o > t_end) {
        return fail(match &spec.window.after {
            Some((sel, _)) => format!(
                "the window never opened: no {} before the run's end",
                sel.label()
            ),
            None => "the window never opened".into(),
        });
    }
    let win = window_text(&spec.window, close);
    let of = |lim: &Limits, (v, t): (i64, Time), limit: Option<i64>, what: String| Verdict {
        outcome: if lim.holds(v) {
            AssertOutcome::Pass
        } else {
            AssertOutcome::Fail
        },
        value: Some(v),
        limit,
        detail: format!(
            "{what} {} at {} ({}; {win})",
            unit.fmt(v),
            fmt_t(t),
            lim_text(lim, unit)
        ),
    };
    // Instance values: the worst, with the number of instances.
    let worst = |vals: &[(i64, Time)], lim: &Limits| -> Option<((i64, Time), Option<i64>, usize)> {
        let lo = *vals.iter().min_by_key(|x| (x.0, x.1))?;
        let hi = *vals.iter().max_by_key(|x| (x.0, std::cmp::Reverse(x.1)))?;
        let (v, l) = lim.report(lo, hi);
        let bad = vals.iter().filter(|x| !lim.holds(x.0)).count();
        Some((v, l, bad))
    };
    match &spec.kind {
        Kind::Range { sig, lim } => {
            let (Some(lo), Some(hi)) = (st.lo, st.hi) else {
                return fail(format!(
                    "no solver point of {} in the window ({win})",
                    sig.label()
                ));
            };
            let (v, l) = lim.report(lo, hi);
            let side = if Some(v) == Some(hi) && l == lim.max {
                "max"
            } else {
                "min"
            };
            let mut r = of(lim, v, l, format!("{side} of {}", sig.label()));
            if lim.min.is_some() && lim.max.is_some() {
                r.detail = format!(
                    "{} (min {} at {}, max {} at {})",
                    r.detail,
                    unit.fmt(lo.0),
                    fmt_t(lo.1),
                    unit.fmt(hi.0),
                    fmt_t(hi.1)
                );
            }
            r
        }
        Kind::At { what, when, lim } => {
            let label = match what {
                AtWhat::Analog(s) => s.label(),
                AtWhat::Digital(n) => format!("{n}'s level"),
            };
            let at = match when {
                AtWhen::Edge { net, level } => {
                    format!("each {} {}", net, if *level == 1 { "rise" } else { "fall" })
                }
                AtWhen::Time(t) => fmt_t(*t),
            };
            let Some((v, l, bad)) = worst(&st.vals, lim) else {
                return fail(format!(
                    "{label} at {at}: no instance in the window ({win})"
                ));
            };
            let mut r = of(lim, v, l, format!("{label} at {at}: worst"));
            r.detail = format!("{} [{} instances, {bad} outside]", r.detail, st.vals.len());
            r
        }
        Kind::Settle {
            sig,
            net,
            level,
            within,
            ..
        } => {
            for inst in std::mem::take(&mut st.open_inst) {
                let at_end = close == t_end;
                st.settled.push(settle_outcome(&inst, close, at_end, t_end));
            }
            let ev = format!("{net} {}", if *level == 1 { "rise" } else { "fall" });
            if st.settled.is_empty() {
                return fail(format!(
                    "{} after {ev}: no {ev} in the window ({win})",
                    sig.label()
                ));
            }
            let evaluated: Vec<&(AssertOutcome, Option<i64>, Time)> = st
                .settled
                .iter()
                .filter(|x| x.0 != AssertOutcome::NotEvaluable)
                .collect();
            let fails = evaluated
                .iter()
                .filter(|x| x.0 == AssertOutcome::Fail)
                .count();
            let ne = st.settled.len() - evaluated.len();
            let notes: Vec<String> = st
                .settled
                .iter()
                .map(|(o, v, t)| match (o, v) {
                    (AssertOutcome::NotEvaluable, _) => format!(
                        "{ev} at {}: not evaluable (its deadline {} is after the run's end)",
                        fmt_t(*t),
                        fmt_t(t.saturating_add(*within))
                    ),
                    (_, Some(v)) => format!("{ev} at {}: settled after {}", fmt_t(*t), fmt_fs(*v)),
                    (_, None) => format!("{ev} at {}: not settled", fmt_t(*t)),
                })
                .collect();
            let worst_v = evaluated
                .iter()
                .filter(|x| fails == 0 || x.0 == AssertOutcome::Fail)
                .map(|x| x.1.unwrap_or(i64::MAX))
                .max();
            let detail = format!(
                "{} into its band after each {ev} ({} evaluated, {ne} not evaluable; limit {}): {}",
                sig.label(),
                evaluated.len(),
                fmt_fs(within.as_fs() as i64),
                notes.join("; ")
            );
            Verdict {
                outcome: if evaluated.is_empty() {
                    AssertOutcome::NotEvaluable
                } else if fails > 0 {
                    AssertOutcome::Fail
                } else {
                    AssertOutcome::Pass
                },
                value: worst_v.filter(|&v| v != i64::MAX),
                limit: Some(within.as_fs() as i64),
                detail,
            }
        }
        Kind::EdgeTiming {
            start, end, lim, ..
        } => {
            let what = format!("{} to {}", start.label(), end.label());
            let Some((v, l, bad)) = worst(&st.vals, lim) else {
                return fail(format!(
                    "{what}: no pair in the window ({win}; {} unpaired)",
                    st.unpaired
                ));
            };
            let mut r = of(lim, v, l, format!("{what}: worst gap"));
            r.detail = format!(
                "{} [{} pairs, {bad} outside, {} unpaired]",
                r.detail,
                st.vals.len(),
                st.unpaired
            );
            r
        }
        Kind::Frequency { net, level, lim } => {
            let what = format!(
                "{net} period ({} to {})",
                if *level == 1 { "rise" } else { "fall" },
                if *level == 1 { "rise" } else { "fall" }
            );
            let Some((v, l, bad)) = worst(&st.vals, lim) else {
                return fail(format!(
                    "{what}: fewer than two edges in the window ({win})"
                ));
            };
            let mut r = of(lim, v, l, format!("{what}: worst"));
            r.detail = format!("{} [{} periods, {bad} outside]", r.detail, st.vals.len());
            r
        }
        Kind::Log {
            pattern,
            source,
            field,
            lim,
        } => {
            let src = source.as_ref().map(|s| format!("{s} ")).unwrap_or_default();
            match field {
                None => {
                    let n = st.n;
                    Verdict {
                        outcome: if lim.holds(n) {
                            AssertOutcome::Pass
                        } else {
                            AssertOutcome::Fail
                        },
                        value: Some(n),
                        limit: lim.report((n, close), (n, close)).1,
                        detail: format!(
                            "{n} {src}log records matching {:?} ({}; {win})",
                            pattern.text(),
                            lim_text(lim, Unit::Count)
                        ),
                    }
                }
                Some((f, _)) => {
                    let what = format!("{f} of {src}log {:?}", pattern.text());
                    let Some((v, l, bad)) = worst(&st.vals, lim) else {
                        return fail(format!("{what}: no matching record in the window ({win})"));
                    };
                    let mut r = of(lim, v, l, format!("{what}: worst"));
                    r.detail = format!("{} [{} records, {bad} outside]", r.detail, st.vals.len());
                    r
                }
            }
        }
        Kind::Count { net, level, lim } => {
            let n = st.n;
            let what = match level {
                Some(1) => format!("{net} rises"),
                Some(_) => format!("{net} falls"),
                None => format!("{net} changes"),
            };
            Verdict {
                outcome: if lim.holds(n) {
                    AssertOutcome::Pass
                } else {
                    AssertOutcome::Fail
                },
                value: Some(n),
                limit: lim.report((n, close), (n, close)).1,
                detail: format!("{n} {what} ({}; {win})", lim_text(lim, Unit::Count)),
            }
        }
        Kind::Event { what, lim } => {
            let n = st.n;
            let w = match what {
                EventWhat::Reset => "reset records",
                EventWhat::Warn => "warn log records",
                EventWhat::Error => "error log records",
            };
            Verdict {
                outcome: if lim.holds(n) {
                    AssertOutcome::Pass
                } else {
                    AssertOutcome::Fail
                },
                value: Some(n),
                limit: lim.report((n, close), (n, close)).1,
                detail: format!("{n} {w} ({}; {win})", lim_text(lim, Unit::Count)),
            }
        }
    }
}

/// Evaluate `specs` on a recorded event stream (`bb assert`): the records
/// as the run emitted them, the run's end `t_end` (its last grant's
/// `until_fs`). Kinds that read solver points are `not_evaluable`; every
/// other kind gives the record the run gave, since it is a pure function of
/// the records (the initial digital levels inferred from each net's first
/// edge). Existing `assert` records in the stream are ignored.
pub fn replay(
    specs: Vec<Spec>,
    events: &[Event],
    t_end: Time,
    rung: &str,
) -> Result<Vec<Event>, String> {
    let mut a = Assertions::new(specs, t_end, rung)?;
    a.replay = true;
    let empty = BTreeMap::new();
    let mut out = Vec::new();
    let split = |pred: &dyn Fn(Time) -> bool| -> (Vec<EdgeRec<'_>>, Vec<LogRec<'_>>) {
        let mut edges = Vec::new();
        let mut logs = Vec::new();
        for e in events {
            match e {
                Event::PinEdge {
                    t_fs, net, level, ..
                } if pred(*t_fs) => edges.push(EdgeRec {
                    t: *t_fs,
                    net,
                    level: *level,
                }),
                Event::Log {
                    t_fs,
                    source,
                    level,
                    msg,
                } if pred(*t_fs) => logs.push(LogRec {
                    t: *t_fs,
                    source,
                    level: Some(*level),
                    msg,
                }),
                Event::Reset { t_fs, source, .. } if pred(*t_fs) => logs.push(LogRec {
                    t: *t_fs,
                    source,
                    level: None,
                    msg: "",
                }),
                _ => {}
            }
        }
        (edges, logs)
    };
    // Each net's level before its first edge, then the initial exchange (t =
    // 0) and one window (0, t_end].
    a.levels = initial_levels(events);
    let (edges0, logs0) = split(&|t| t.is_zero());
    out.extend(a.window(&WindowInput {
        prev: Time::ZERO,
        now: Time::ZERO,
        edges: edges0,
        logs: logs0,
        points: &empty,
        initial: None,
    }));
    if t_end > Time::ZERO {
        let (edges, logs) = split(&|t| !t.is_zero() && t <= t_end);
        out.extend(a.window(&WindowInput {
            prev: Time::ZERO,
            now: t_end,
            edges,
            logs,
            points: &empty,
            initial: None,
        }));
    }
    // As the scheduler emits them: by time, in assertion order at ties.
    out.sort_by_key(Event::time);
    Ok(out)
}

/// Each net's level before its first edge (the initial level is not an
/// edge, and edges alternate).
fn initial_levels(events: &[Event]) -> BTreeMap<String, u8> {
    let mut lv: BTreeMap<String, u8> = BTreeMap::new();
    for e in events {
        if let Event::PinEdge { net, level, .. } = e {
            lv.entry(net.clone()).or_insert(1 - level);
        }
    }
    lv
}

#[cfg(test)]
mod tests {
    use super::*;

    const US: u64 = 1_000_000_000;

    fn pt(t_us: u64, v: f64) -> SolverPoint {
        let t = Time::from_fs(t_us * US);
        SolverPoint {
            t,
            secs: t.as_secs_f64(),
            v,
        }
    }

    /// A synthetic run: analog points of nets A and B every 10 us and at
    /// every window boundary (A a triangle peaking at 255 us, between two
    /// 10 us points, B constant 1 V), digital edges and logs; evaluated in
    /// windows ending at `bounds`.
    fn run(
        specs: Vec<Spec>,
        bounds: &[u64],
        edges: &[(u64, &str, u8)],
        logs: &[(u64, &str)],
    ) -> Vec<Event> {
        let t_end = Time::from_fs(*bounds.last().unwrap() * US);
        let mut a = Assertions::new(specs, t_end, "fast").unwrap();
        let wave = |t: u64| -> f64 {
            // up 1 mV/us to 1.255 V at 255 us, then down; with an extra point at 255 us
            let x = t as f64;
            if x <= 255.0 {
                1.0 + x * 1e-3
            } else {
                1.255 - (x - 255.0) * 1e-3
            }
        };
        let mut times: Vec<u64> = (0..=bounds.last().copied().unwrap()).step_by(10).collect();
        times.push(255);
        times.extend_from_slice(bounds);
        times.sort();
        times.dedup();
        let mut nets = NetValues::new();
        nets.insert(
            "D".into(),
            PinSignal::digital(crate::signal::Level::Low, Time::ZERO),
        );
        nets.insert(
            "E".into(),
            PinSignal::digital(crate::signal::Level::Low, Time::ZERO),
        );
        let mut out = Vec::new();
        let mut prev = 0u64;
        for (k, &now) in std::iter::once(&0u64).chain(bounds.iter()).enumerate() {
            let sel: Vec<u64> = times
                .iter()
                .copied()
                .filter(|&t| t >= prev && t <= now)
                .collect();
            let mut points = BTreeMap::new();
            points.insert(
                "A".to_string(),
                sel.iter().map(|&t| pt(t, wave(t))).collect::<Vec<_>>(),
            );
            points.insert(
                "B".to_string(),
                sel.iter().map(|&t| pt(t, 1.0)).collect::<Vec<_>>(),
            );
            let inw = |t: u64| if k == 0 { t == 0 } else { t > prev && t <= now };
            // A log at a boundary is reported at the next exchange, at its
            // own time (the window's start), as Renode's UART lines can be.
            let last = now == *bounds.last().unwrap();
            let inl = |t: u64| {
                if k == 0 {
                    t == 0
                } else {
                    (t >= prev && t < now) || (last && t == now)
                }
            };
            let e: Vec<EdgeRec> = edges
                .iter()
                .filter(|e| inw(e.0))
                .map(|e| EdgeRec {
                    t: Time::from_fs(e.0 * US),
                    net: e.1,
                    level: e.2,
                })
                .collect();
            let l: Vec<LogRec> = logs
                .iter()
                .filter(|l| inl(l.0) && !(k == 1 && l.0 == 0))
                .map(|l| LogRec {
                    t: Time::from_fs(l.0 * US),
                    source: "cpu",
                    level: Some(LogLevel::Info),
                    msg: l.1,
                })
                .collect();
            let mut v = a.window(&WindowInput {
                prev: Time::from_fs(prev * US),
                now: Time::from_fs(now * US),
                edges: e,
                logs: l,
                points: &points,
                initial: (k == 0).then_some(&nets),
            });
            // As the scheduler emits them: by time, assertion order at ties.
            v.sort_by_key(Event::time);
            out.extend(v);
            prev = now;
        }
        out
    }

    fn spec(name: &str, kind: Kind) -> Spec {
        Spec {
            name: name.into(),
            kind,
            window: Window::default(),
            requires: None,
            msg: None,
        }
    }

    fn sig(n: &str, m: Option<&str>) -> Signal {
        Signal {
            net: n.into(),
            minus: m.map(Into::into),
        }
    }

    fn get(recs: &[Event], name: &str) -> (AssertOutcome, Option<i64>, Time) {
        recs.iter()
            .find_map(|r| match r {
                Event::Assert {
                    name: n,
                    outcome,
                    value,
                    t_fs,
                    ..
                } if n == name => Some((*outcome, *value, *t_fs)),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no record {name}"))
    }

    const EDGES: &[(u64, &str, u8)] = &[
        (100, "D", 1),
        (150, "E", 1),
        (300, "D", 0),
        (310, "E", 0),
        (420, "D", 1),
        (480, "E", 1),
    ];
    const LOGS: &[(u64, &str)] = &[
        (50, "boot ok"),
        (120, "code=7 t 20 us"),
        (400, "code=9 t 30 us"),
    ];

    fn all_specs() -> Vec<Spec> {
        vec![
            spec(
                "range",
                Kind::Range {
                    sig: sig("A", Some("B")),
                    lim: Limits {
                        min: None,
                        max: Some(250_000),
                    },
                },
            ),
            spec(
                "at",
                Kind::At {
                    what: AtWhat::Analog(sig("A", None)),
                    when: AtWhen::Edge {
                        net: "D".into(),
                        level: 1,
                    },
                    lim: Limits {
                        min: Some(1_000_000),
                        max: None,
                    },
                },
            ),
            spec(
                "level",
                Kind::At {
                    what: AtWhat::Digital("D".into()),
                    when: AtWhen::Edge {
                        net: "E".into(),
                        level: 1,
                    },
                    lim: Limits {
                        min: Some(1),
                        max: None,
                    },
                },
            ),
            spec(
                "settle",
                Kind::Settle {
                    sig: sig("A", None),
                    net: "D".into(),
                    level: 0,
                    within: Time::from_us(100),
                    lo: None,
                    hi: Some(BandEdge::Rel(950_000)),
                },
            ),
            spec(
                "timing",
                Kind::EdgeTiming {
                    start: Sel::Edge {
                        net: "D".into(),
                        level: 1,
                    },
                    end: Sel::Edge {
                        net: "E".into(),
                        level: 1,
                    },
                    pair: Pair::Last,
                    lim: Limits {
                        min: Some(40 * US as i64),
                        max: None,
                    },
                },
            ),
            spec(
                "cross",
                Kind::EdgeTiming {
                    start: Sel::Cross {
                        sig: sig("A", None),
                        below: true,
                        threshold_uv: 1_205_000,
                    },
                    end: Sel::Edge {
                        net: "E".into(),
                        level: 0,
                    },
                    pair: Pair::First,
                    lim: Limits {
                        min: Some(0),
                        max: Some(10 * US as i64),
                    },
                },
            ),
            spec(
                "count",
                Kind::Count {
                    net: "D".into(),
                    level: Some(1),
                    lim: Limits {
                        min: Some(2),
                        max: Some(2),
                    },
                },
            ),
            spec(
                "freq",
                Kind::Frequency {
                    net: "D".into(),
                    level: 1,
                    lim: Limits {
                        min: Some(300 * US as i64),
                        max: Some(330 * US as i64),
                    },
                },
            ),
            spec(
                "log",
                Kind::Log {
                    pattern: Pattern::parse("^code=").unwrap(),
                    source: None,
                    field: Some(("t".into(), Unit::Fs)),
                    lim: Limits {
                        min: None,
                        max: Some(25 * US as i64),
                    },
                },
            ),
            spec(
                "present",
                Kind::Log {
                    pattern: Pattern::parse("boot ok$").unwrap(),
                    source: Some("cpu".into()),
                    field: None,
                    lim: Limits {
                        min: Some(1),
                        max: None,
                    },
                },
            ),
            spec(
                "events",
                Kind::Event {
                    what: EventWhat::Warn,
                    lim: Limits {
                        min: None,
                        max: Some(0),
                    },
                },
            ),
        ]
    }

    #[test]
    fn kinds_give_their_values() {
        let r = run(all_specs(), &[600], EDGES, LOGS);
        // The peak between the 10 us points (255 us): 0.255 V, above the 0.25 V limit.
        assert_eq!(
            get(&r, "range"),
            (AssertOutcome::Fail, Some(255_000), Time::from_us(600))
        );
        // A at D rises (100 us: 1.1 V, 420 us: 1.09 V): the minimum.
        assert_eq!(get(&r, "at").1, Some(1_090_000));
        assert_eq!(get(&r, "at").0, AssertOutcome::Pass);
        assert_eq!(
            get(&r, "level"),
            (AssertOutcome::Pass, Some(1), Time::from_us(600))
        );
        // D falls at 300 us (A = 1.210 V); 95 % is 1.1495 V, reached at 360.5 us.
        let s = get(&r, "settle");
        assert_eq!(s.0, AssertOutcome::Pass);
        assert_eq!(s.1, Some(60_500_000_000));
        // E rises 50 us and 60 us after D: the minimum gap.
        assert_eq!(
            get(&r, "timing"),
            (
                AssertOutcome::Pass,
                Some(50 * US as i64),
                Time::from_us(600)
            )
        );
        // A crosses 1.205 V downwards at 305 us (interpolated, rounded up to
        // whole fs), E falls at 310 us: a gap of 5 us.
        let g = get(&r, "cross").1.unwrap();
        assert!((4_999_999_000..=5_000_000_000).contains(&g), "{g}");
        assert_eq!(
            get(&r, "count"),
            (AssertOutcome::Pass, Some(2), Time::from_us(600))
        );
        assert_eq!(
            get(&r, "freq"),
            (
                AssertOutcome::Pass,
                Some(320 * US as i64),
                Time::from_us(600)
            )
        );
        // Fields 20 us and 30 us: the 30 us one is over 25 us.
        assert_eq!(
            get(&r, "log"),
            (
                AssertOutcome::Fail,
                Some(30 * US as i64),
                Time::from_us(600)
            )
        );
        assert_eq!(get(&r, "present").1, Some(1));
        assert_eq!(
            get(&r, "events"),
            (AssertOutcome::Pass, Some(0), Time::from_us(600))
        );
    }

    #[test]
    fn verdicts_do_not_depend_on_the_partition() {
        let one = run(all_specs(), &[600], EDGES, LOGS);
        let cells: Vec<u64> = (1..=60).map(|k| k * 10).collect();
        let many = run(all_specs(), &cells, EDGES, LOGS);
        let odd = run(all_specs(), &[30, 250, 260, 310, 590, 600], EDGES, LOGS);
        assert_eq!(one, many);
        assert_eq!(one, odd);
    }

    #[test]
    fn a_log_or_a_close_at_a_boundary_does_not_depend_on_the_partition() {
        // The log at 120 us opens the window and the D fall at 300 us closes
        // it; with boundaries at 120 and 300 the log is reported at the next
        // exchange (at its own time) and the close falls on a boundary.
        let mut s = spec(
            "w",
            Kind::Log {
                pattern: Pattern::parse("^code=").unwrap(),
                source: None,
                field: Some(("code".into(), Unit::Count)),
                lim: Limits {
                    min: Some(7),
                    max: Some(9),
                },
            },
        );
        s.window.after = Some((
            Sel::Log {
                pattern: Pattern::parse("boot").unwrap(),
                source: None,
            },
            Time::ZERO,
        ));
        s.window.until = Some(Sel::Log {
            pattern: Pattern::parse("^code=9").unwrap(),
            source: None,
        });
        let mut t = spec(
            "u",
            Kind::Count {
                net: "E".into(),
                level: None,
                lim: Limits {
                    min: Some(1),
                    max: None,
                },
            },
        );
        t.window.after = Some((
            Sel::Log {
                pattern: Pattern::parse("^code=7").unwrap(),
                source: None,
            },
            Time::ZERO,
        ));
        t.window.until = Some(Sel::Edge {
            net: "D".into(),
            level: 0,
        });
        let specs = vec![s, t];
        let a = run(specs.clone(), &[600], EDGES, LOGS);
        let b = run(specs.clone(), &[50, 120, 300, 400, 600], EDGES, LOGS);
        let c = run(
            specs,
            &(1..=60).map(|k| k * 10).collect::<Vec<_>>(),
            EDGES,
            LOGS,
        );
        assert_eq!(a, b);
        assert_eq!(a, c);
        assert_eq!(
            get(&a, "w"),
            (AssertOutcome::Pass, Some(9), Time::from_us(400))
        );
        assert_eq!(
            get(&a, "u"),
            (AssertOutcome::Pass, Some(1), Time::from_us(300))
        );
    }

    #[test]
    fn windows_open_close_and_while() {
        let mut s = spec(
            "w",
            Kind::Count {
                net: "E".into(),
                level: None,
                lim: Limits {
                    min: Some(1),
                    max: Some(1),
                },
            },
        );
        // After the log at 120 us until D falls (300 us): E rises at 150 us only.
        s.window.after = Some((
            Sel::Log {
                pattern: Pattern::parse("^code=7").unwrap(),
                source: None,
            },
            Time::ZERO,
        ));
        s.window.until = Some(Sel::Edge {
            net: "D".into(),
            level: 0,
        });
        let r = run(vec![s.clone()], &[50, 200, 600], EDGES, LOGS);
        assert_eq!(
            get(&r, "w"),
            (AssertOutcome::Pass, Some(1), Time::from_us(300))
        );
        // While D is high: E changes at 150 us and 480 us (310 us is while D is low).
        let mut w = s.clone();
        w.window.after = None;
        w.window.until = None;
        w.window.while_level = Some(("D".into(), 1));
        w.kind = Kind::Count {
            net: "E".into(),
            level: None,
            lim: Limits {
                min: Some(2),
                max: Some(2),
            },
        };
        let r = run(vec![w], &[600], EDGES, LOGS);
        assert_eq!(get(&r, "w").1, Some(2));
        // A window that never opens fails.
        let mut n = s;
        n.window.after = Some((
            Sel::Log {
                pattern: Pattern::parse("never").unwrap(),
                source: None,
            },
            Time::ZERO,
        ));
        let r = run(vec![n], &[600], EDGES, LOGS);
        assert_eq!(get(&r, "w").0, AssertOutcome::Fail);
    }

    #[test]
    fn a_settle_past_the_run_end_is_not_evaluable() {
        let s = spec(
            "late",
            Kind::Settle {
                sig: sig("A", None),
                net: "D".into(),
                level: 0,
                within: Time::from_us(100),
                lo: None,
                hi: Some(BandEdge::Abs(1_000_000)),
            },
        );
        // D falls at 300 us; the run ends at 350 us, before its deadline.
        let r = run(vec![s], &[350], EDGES, LOGS);
        assert_eq!(get(&r, "late").0, AssertOutcome::NotEvaluable);
        let mut q = spec(
            "rung",
            Kind::Count {
                net: "D".into(),
                level: None,
                lim: Limits {
                    min: Some(0),
                    max: None,
                },
            },
        );
        q.requires = Some("pi".into());
        let r = run(vec![q], &[350], EDGES, LOGS);
        assert_eq!(get(&r, "rung").0, AssertOutcome::NotEvaluable);
    }

    #[test]
    fn replay_gives_the_record_kinds_and_not_the_analog_ones() {
        let specs = all_specs();
        let inrun = run(specs.clone(), &[600], EDGES, LOGS);
        let mut events = vec![Event::Grant {
            t_fs: Time::ZERO,
            until_fs: Time::from_us(600),
            limited_by: "t_end".into(),
        }];
        for (t, n, l) in EDGES {
            events.push(Event::PinEdge {
                t_fs: Time::from_us(*t),
                net: (*n).into(),
                driver: "x.y".into(),
                level: *l,
            });
        }
        for (t, m) in LOGS {
            events.push(Event::Log {
                t_fs: Time::from_us(*t),
                source: "cpu".into(),
                level: LogLevel::Info,
                msg: (*m).into(),
            });
        }
        events.sort_by_key(Event::time);
        let rep = replay(specs.clone(), &events, Time::from_us(600), "fast").unwrap();
        for (s, (a, b)) in specs.iter().zip(inrun.iter().zip(&rep)) {
            if s.analog_nets().is_empty() {
                assert_eq!(a, b, "{}", s.name);
            } else {
                assert!(
                    matches!(
                        b,
                        Event::Assert {
                            outcome: AssertOutcome::NotEvaluable,
                            ..
                        }
                    ),
                    "{}",
                    s.name
                );
            }
        }
    }

    #[test]
    fn patterns_and_fields() {
        let p = Pattern::parse("^vsense: n=3 ").unwrap();
        assert!(p.matches("vsense: n=3 code=1067"));
        assert!(!p.matches("vsense: n=31 code=1067"));
        assert!(Pattern::parse("leds=off$").unwrap().matches("x leds=off"));
        assert!(Pattern::parse("^a$").unwrap().matches("a"));
        assert!(Pattern::parse("a.b").is_err());
        assert_eq!(
            log_field("vsense: n=3 code=1067 vbat", "code", Unit::Count),
            Some(1067)
        );
        assert_eq!(log_field("barcode=5 code=7", "code", Unit::Count), Some(7));
        let m = "M1: DRV5032 tS 50 ms, tactive 40 us, BOP 3 mT";
        assert_eq!(log_field(m, "tS", Unit::Fs), Some(50_000_000_000_000));
        assert_eq!(log_field(m, "tactive", Unit::Fs), Some(40_000_000_000));
        assert_eq!(log_field("v=1.25 V", "v", Unit::Uv), Some(1_250_000));
        assert_eq!(log_field("x", "code", Unit::Count), None);
        assert_eq!(fmt_fs(45_261_750_000_000), "45.26175 ms");
        assert_eq!(fmt_uv(-792_874), "-0.792874 V");
    }
}
