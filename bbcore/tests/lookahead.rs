//! ADR-003 step 6 gates: the per-grant plan, the early stop, the promise
//! audit and the query back-off (D5), with mock engines.
//!
//! The board is a feedback loop like `examples/feedback`: a lattice-bound
//! "CPU" `u` (output `P`, input `IN`) and an "analog" producer `n` that can
//! stop early (output `B`, input `A`), wired `u.P -> A` and `n.B -> IN`.
//! `u` toggles `P` autonomously at its `wakes` (a timer) and once at
//! `ceil_L(t) + 1 fs` after every rising edge of `IN` at `t` (an ISR, never
//! early). It promises quiet outputs until its next wake while no reaction
//! is pending. `n` toggles `B` at fixed times.

use std::cell::RefCell;
use std::rc::Rc;

use bbcore::{
    Direction, Engine, EngineError, Event, EventSink, GrantPlan, Level, PinSignal, PinSignals,
    SchedError, Scheduler, SchedulerConfig, Time, Transition,
};

type Journal = Rc<RefCell<Vec<String>>>;

const L: Time = Time::from_us(10);

fn ceil_l(t: Time) -> Time {
    Time::from_fs(t.as_fs().div_ceil(L.as_fs()) * L.as_fs())
}

fn us(x: u64) -> Time {
    Time::from_us(x)
}

fn ns(x: u64) -> Time {
    Time::from_ns(x)
}

/// The lattice-bound CPU-like engine.
struct Cpu {
    name: String,
    now: Time,
    wakes: Vec<Time>,
    /// Reactions scheduled at `ceil_L(t_in) + 1 fs`.
    pending: Vec<Time>,
    level: Level,
    edge_time: Time,
    trace: Vec<Transition>,
    input: Level,
    /// `quiet_until` returns this instead of the honest value (a lie).
    lie: Option<Time>,
    /// `quiet_until` never promises.
    never_quiet: bool,
    /// A wake without an output at this time, detected by the engine itself
    /// (like the Renode plugin's WFI-exit audit).
    wake_at: Option<Time>,
    promise: Option<Time>,
    journal: Journal,
}

impl Cpu {
    fn new(name: &str, wakes: &[Time], journal: &Journal) -> Cpu {
        Cpu {
            name: name.into(),
            now: Time::ZERO,
            wakes: wakes.to_vec(),
            pending: Vec::new(),
            level: Level::Low,
            edge_time: Time::ZERO,
            trace: Vec::new(),
            input: Level::Low,
            lie: None,
            never_quiet: false,
            wake_at: None,
            promise: None,
            journal: journal.clone(),
        }
    }
    fn log(&self, s: String) {
        self.journal.borrow_mut().push(format!("{} {s}", self.name));
    }
}

impl Engine for Cpu {
    fn name(&self) -> &str {
        &self.name
    }
    fn next_safe_time(&mut self, _now: Time) -> Time {
        Time::MAX
    }
    fn grant_lattice(&self) -> Option<Time> {
        Some(L)
    }
    fn quiet_until(&mut self, now: Time) -> Time {
        assert_eq!(now, self.now);
        let honest = if self.never_quiet || !self.pending.is_empty() {
            now
        } else {
            self.wakes
                .iter()
                .copied()
                .find(|&w| w > now)
                .unwrap_or(Time::MAX)
        };
        let q = self.lie.filter(|_| honest > now).unwrap_or(honest);
        self.promise = Some(q);
        self.log(format!("quiet {} -> {}", now.as_fs(), q.as_fs()));
        q
    }
    fn advance_to(&mut self, t: Time) -> Result<(), EngineError> {
        self.log(format!("advance {}", t.as_fs()));
        assert!(t > self.now && t.as_fs().is_multiple_of(L.as_fs()), "{t}");
        if let (Some(w), Some(q)) = (self.wake_at, self.promise.take()) {
            if q > self.now && self.now < w && w < t {
                return Err(EngineError::PromiseBroken { promised: q, at: w });
            }
        }
        let now = self.now;
        let mut ev: Vec<Time> = self
            .wakes
            .iter()
            .chain(&self.pending)
            .copied()
            .filter(|&e| now < e && e <= t)
            .collect();
        ev.sort();
        self.pending.retain(|&p| p > t);
        self.trace.clear();
        for e in ev {
            self.level = self.level.toggled();
            self.edge_time = e;
            self.trace.push(Transition {
                t: e,
                pin: "P".into(),
                signal: PinSignal::digital(self.level, e),
            });
        }
        self.now = t;
        Ok(())
    }
    fn read_outputs(&mut self) -> Result<PinSignals, EngineError> {
        let mut m = PinSignals::new();
        m.insert("P".into(), PinSignal::digital(self.level, self.edge_time));
        Ok(m)
    }
    fn read_output_trace(&mut self) -> Result<Option<Vec<Transition>>, EngineError> {
        Ok(Some(self.trace.clone()))
    }
    fn apply_inputs(&mut self, inputs: &PinSignals) -> Result<(), EngineError> {
        if let Some(PinSignal::Digital { level, edge_time }) = inputs.get("IN") {
            if *level != self.input {
                self.input = *level;
                self.log(format!("input {level:?} @{}", edge_time.as_fs()));
                if *level == Level::High {
                    // Never early: at the first lattice point at or after
                    // the edge (or now, if delivered at a boundary).
                    let at = ceil_l(*edge_time).max(self.now) + Time::from_fs(1);
                    self.pending.push(at);
                }
            }
        }
        Ok(())
    }
}

/// The analog-like producer that can stop early.
struct Stopper {
    name: String,
    now: Time,
    edges: Vec<Time>,
    level: Level,
    edge_time: Time,
    trace: Vec<Transition>,
    can_stop: bool,
    journal: Journal,
}

impl Stopper {
    fn new(name: &str, edges: &[Time], journal: &Journal) -> Stopper {
        Stopper {
            name: name.into(),
            now: Time::ZERO,
            edges: edges.to_vec(),
            level: Level::Low,
            edge_time: Time::ZERO,
            trace: Vec::new(),
            can_stop: true,
            journal: journal.clone(),
        }
    }
    fn log(&self, s: String) {
        self.journal.borrow_mut().push(format!("{} {s}", self.name));
    }
    fn run(&mut self, t: Time) {
        self.trace.clear();
        for &e in self.edges.iter().filter(|&&e| self.now < e && e <= t) {
            self.level = self.level.toggled();
            self.edge_time = e;
            self.trace.push(Transition {
                t: e,
                pin: "B".into(),
                signal: PinSignal::digital(self.level, e),
            });
        }
        self.now = t;
    }
}

impl Engine for Stopper {
    fn name(&self) -> &str {
        &self.name
    }
    fn next_safe_time(&mut self, _now: Time) -> Time {
        Time::MAX
    }
    fn advance_to(&mut self, t: Time) -> Result<(), EngineError> {
        self.log(format!("advance {}", t.as_fs()));
        self.run(t);
        Ok(())
    }
    fn can_stop_early(&self) -> bool {
        self.can_stop
    }
    fn advance_until(&mut self, t: Time, watch: &[String]) -> Result<Time, EngineError> {
        assert!(self.can_stop);
        let r = if watch.iter().any(|w| w == "B") {
            self.edges
                .iter()
                .copied()
                .find(|&e| self.now < e && e <= t)
                .map_or(t, |e| ceil_l(e).min(t))
        } else {
            t
        };
        self.log(format!("until {} {watch:?} -> {}", t.as_fs(), r.as_fs()));
        self.run(r);
        Ok(r)
    }
    fn read_outputs(&mut self) -> Result<PinSignals, EngineError> {
        let mut m = PinSignals::new();
        m.insert("B".into(), PinSignal::digital(self.level, self.edge_time));
        Ok(m)
    }
    fn read_output_trace(&mut self) -> Result<Option<Vec<Transition>>, EngineError> {
        Ok(Some(self.trace.clone()))
    }
    fn apply_inputs(&mut self, inputs: &PinSignals) -> Result<(), EngineError> {
        if let Some(PinSignal::Digital { level, edge_time }) = inputs.get("A") {
            self.log(format!("sees A {level:?} @{}", edge_time.as_fs()));
        }
        Ok(())
    }
}

/// An engine that only shapes the graph.
struct Idle(String);

impl Engine for Idle {
    fn name(&self) -> &str {
        &self.0
    }
    fn next_safe_time(&mut self, _now: Time) -> Time {
        Time::MAX
    }
    fn advance_to(&mut self, _t: Time) -> Result<(), EngineError> {
        Ok(())
    }
    fn read_outputs(&mut self) -> Result<PinSignals, EngineError> {
        Ok(PinSignals::new())
    }
    fn apply_inputs(&mut self, _: &PinSignals) -> Result<(), EngineError> {
        Ok(())
    }
}

const WAKES: [Time; 3] = [
    Time::from_ms(5),          // on the lattice: reported at q itself
    Time::from_ns(12_345_678), // off the lattice
    Time::from_ms(30),
];

fn n_edges() -> Vec<Time> {
    vec![
        ns(3_000_001),
        us(7_500),
        ns(20_000_005), // a sub-cell pulse: rises and falls in one cell
        ns(20_000_007),
        us(41_000),
        ns(41_234_567),
    ]
}

fn cfg(lookahead: bool) -> SchedulerConfig {
    SchedulerConfig {
        max_quantum: Time::from_ms(10),
        loop_quantum: None,
        lookahead,
        log_grants: true,
        event_sampling: None,
    }
}

struct Outcome {
    events: String,
    grants: Vec<(Time, Time, String)>,
    plans: Vec<GrantPlan>,
    journal: Vec<String>,
}

impl Outcome {
    /// Every record except `grant`.
    fn non_grant(&self) -> Vec<&str> {
        self.events
            .lines()
            .filter(|l| !l.contains("\"type\":\"grant\""))
            .collect()
    }
}

fn grants(events: &str) -> Vec<(Time, Time, String)> {
    events
        .lines()
        .filter_map(|l| match Event::from_line(l).unwrap() {
            Event::Grant {
                t_fs,
                until_fs,
                limited_by,
            } => Some((t_fs, until_fs, limited_by)),
            _ => None,
        })
        .collect()
}

/// The feedback board: `u` registered first, then `n`.
fn board(lookahead: bool, tweak: impl FnOnce(&mut Cpu)) -> (Scheduler, Journal) {
    let j: Journal = Rc::default();
    let mut s = Scheduler::new(cfg(lookahead)).unwrap();
    s.record_plans(true);
    let mut cpu = Cpu::new("u", &WAKES, &j);
    tweak(&mut cpu);
    let u = s.add_engine(Box::new(cpu)).unwrap();
    let n = s
        .add_engine(Box::new(Stopper::new("n", &n_edges(), &j)))
        .unwrap();
    s.bind(u, "P", "U.P", Direction::Output).unwrap();
    s.bind(n, "A", "U.P", Direction::Input).unwrap();
    s.bind(n, "B", "U.IN", Direction::Output).unwrap();
    s.bind(u, "IN", "U.IN", Direction::Input).unwrap();
    (s, j)
}

fn run(mut s: Scheduler, j: &Journal, t_end: Time) -> Result<Outcome, SchedError> {
    let mut sink = EventSink::new(Vec::new());
    let r = s.run(t_end, &mut sink, &mut []);
    let events = String::from_utf8(sink.into_inner()).unwrap();
    r.map(|_| Outcome {
        grants: grants(&events),
        events,
        plans: s.plan_log().to_vec(),
        journal: j.borrow().clone(),
    })
}

fn feedback(lookahead: bool) -> Outcome {
    let (s, j) = board(lookahead, |_| {});
    run(s, &j, Time::from_ms(50)).unwrap()
}

/// Equivalence: lookahead changes only the partition. The quiet run gives
/// the one-cell run's records byte for byte, in far fewer grants.
#[test]
fn lookahead_changes_only_grant_records() {
    let off = feedback(false);
    let on = feedback(true);
    assert_eq!(off.grants.len(), 5000);
    assert!(off.grants.iter().all(|g| g.2 == "loop_quantum"));
    // Worked out by hand: 10 quiet grants, and 3 one-cell grants (one
    // asked, k = 1, then two skipped) after each of the 3 reactions (3.01,
    // 20.01, 41 ms) and at the 12.345678 ms wake, which floors to `now`.
    assert_eq!(on.grants.len(), 22, "{:#?}", on.grants);
    assert_eq!(on.non_grant(), off.non_grant());
    // Every edge made it: 3 wakes + 3 reactions on P, 6 edges on B.
    assert_eq!(on.non_grant().len(), 12, "{:#?}", on.non_grant());
    // Contiguous grants ending at t_end.
    for w in on.grants.windows(2) {
        assert_eq!(w[0].1, w[1].0);
    }
    assert_eq!(on.grants.last().unwrap().1, Time::from_ms(50));
}

/// Gate: a quiet producer (`u`, which drives `n`'s input) with its consumer
/// `n` advanced first. In the first grant `n` runs before `u`, which is quiet
/// until its 5 ms wake; `u`'s outputs did not change, so `n` saw exactly the
/// inputs of a one-cell run.
#[test]
fn a_quiet_producer_with_its_consumer_advanced_first() {
    let on = feedback(true);
    // The static plan puts the lattice-bound u first: [u, n].
    let first = &on.plans[0];
    assert_eq!(first.order, vec![1, 0]);
    assert_eq!(first.quiet, vec![0]);
    assert_eq!(first.stop, Some(1));
    assert_eq!(first.queried, vec![0]);
    let pos = |s: &str| on.journal.iter().position(|l| l == s).unwrap();
    assert!(pos("n until 5000000000000 [\"B\"] -> 3010000000000") < pos("u advance 3010000000000"));
    // One-cell grants use the static order [u, n].
    let one_cell = on.plans.iter().find(|p| p.quiet.is_empty()).unwrap();
    assert_eq!(one_cell.order, vec![0, 1]);
    assert_eq!(one_cell.until - one_cell.from, L);
}

/// Gate: an early stop shrinks `g` for every later engine. `n`'s edge at
/// 3.000001 ms stops it at ceil_L = 3.01 ms instead of the quiet bound 5 ms
/// (`u`'s promise), `u` advances to 3.01 ms, and the record says
/// `stop:n`. It receives the edge before it advances and reacts at
/// 3.01 ms + 1 fs, as in the one-cell run.
#[test]
fn an_early_stop_shrinks_the_grant_for_later_engines() {
    let on = feedback(true);
    assert_eq!(on.grants[0], (Time::ZERO, us(3_010), "stop:n".to_string()));
    assert!(on
        .journal
        .contains(&"u input High @3000001000000".to_string()));
    // The next grants are one cell while the reaction is pending, then the
    // quiet grant up to u's 5 ms wake is labelled by u's promise.
    assert_eq!(on.grants[1], (us(3_010), us(3_020), "loop_quantum".into()));
    assert!(on
        .grants
        .contains(&(us(3_040), us(5_000), "engine:u".to_string())));
    // A grant reaching the wake off the lattice is floored to it.
    assert!(on
        .grants
        .iter()
        .any(|g| g.1 == us(12_340) && g.2 == "engine:u"));
    // The sub-cell pulse stops n once, at the cell end; u sees both edges
    // there and reacts once.
    assert!(on
        .grants
        .iter()
        .any(|g| g.1 == us(20_010) && g.2 == "stop:n"));
    // max_quantum still caps quiet grants (u has no wake after 30 ms).
    assert!(on.grants.iter().any(|g| g.2 == "max_quantum"));
}

/// The fallbacks of D5.2: the board grants one cell (Part A) and nobody is
/// asked for a promise.
fn assert_one_cell(o: &Outcome) {
    assert!(
        o.grants.iter().all(|g| g.2 == "loop_quantum"),
        "{:?}",
        &o.grants[..3]
    );
    assert!(o
        .plans
        .iter()
        .all(|p| p.quiet.is_empty() && p.queried.is_empty()));
    assert!(!o.journal.iter().any(|l| l.contains("quiet")));
}

/// Gate: a stopping engine that is not first in the remaining graph's
/// topological order (an unconnected engine registered before it wins the
/// index tie) falls back to one cell.
#[test]
fn a_stopping_engine_that_is_not_first_falls_back() {
    let j: Journal = Rc::default();
    let mut s = Scheduler::new(cfg(true)).unwrap();
    s.record_plans(true);
    s.add_engine(Box::new(Idle("x".into()))).unwrap();
    let u = s.add_engine(Box::new(Cpu::new("u", &WAKES, &j))).unwrap();
    let n = s
        .add_engine(Box::new(Stopper::new("n", &n_edges(), &j)))
        .unwrap();
    s.bind(u, "P", "U.P", Direction::Output).unwrap();
    s.bind(n, "A", "U.P", Direction::Input).unwrap();
    s.bind(n, "B", "U.IN", Direction::Output).unwrap();
    s.bind(u, "IN", "U.IN", Direction::Input).unwrap();
    let o = run(s, &j, Time::from_ms(1)).unwrap();
    assert_one_cell(&o);
    assert_eq!(o.grants.len(), 100);

    // A producer that cannot stop early falls back too.
    let j: Journal = Rc::default();
    let mut s = Scheduler::new(cfg(true)).unwrap();
    s.record_plans(true);
    let u = s.add_engine(Box::new(Cpu::new("u", &WAKES, &j))).unwrap();
    let mut st = Stopper::new("n", &n_edges(), &j);
    st.can_stop = false;
    let n = s.add_engine(Box::new(st)).unwrap();
    s.bind(u, "P", "U.P", Direction::Output).unwrap();
    s.bind(n, "A", "U.P", Direction::Input).unwrap();
    s.bind(n, "B", "U.IN", Direction::Output).unwrap();
    s.bind(u, "IN", "U.IN", Direction::Input).unwrap();
    assert_one_cell(&run(s, &j, Time::from_ms(1)).unwrap());
}

/// Gate: a cycle left after cutting `Q`'s out-edges (n <-> m) falls back.
#[test]
fn a_remaining_cycle_falls_back() {
    let (mut s, j) = {
        let j: Journal = Rc::default();
        let mut s = Scheduler::new(cfg(true)).unwrap();
        s.record_plans(true);
        let u = s.add_engine(Box::new(Cpu::new("u", &WAKES, &j))).unwrap();
        let n = s
            .add_engine(Box::new(Stopper::new("n", &n_edges(), &j)))
            .unwrap();
        let m = s.add_engine(Box::new(Idle("m".into()))).unwrap();
        s.bind(u, "P", "U.P", Direction::Output).unwrap();
        s.bind(n, "A", "U.P", Direction::Input).unwrap();
        s.bind(n, "B", "U.IN", Direction::Output).unwrap();
        s.bind(u, "IN", "U.IN", Direction::Input).unwrap();
        // n <-> m.
        s.bind(m, "MO", "M", Direction::Output).unwrap();
        s.bind(n, "MI", "M", Direction::Input).unwrap();
        s.bind(n, "NO", "N2M", Direction::Output).unwrap();
        s.bind(m, "NI", "N2M", Direction::Input).unwrap();
        (s, j)
    };
    s.record_plans(true);
    assert_one_cell(&run(s, &j, Time::from_ms(1)).unwrap());
}

/// Gate: two producers feeding `Q` fall back.
#[test]
fn two_producers_feeding_q_fall_back() {
    let j: Journal = Rc::default();
    let mut s = Scheduler::new(cfg(true)).unwrap();
    s.record_plans(true);
    let u = s.add_engine(Box::new(Cpu::new("u", &WAKES, &j))).unwrap();
    let n = s
        .add_engine(Box::new(Stopper::new("n", &n_edges(), &j)))
        .unwrap();
    let n2 = s.add_engine(Box::new(Stopper::new("n2", &[], &j))).unwrap();
    s.bind(u, "P", "U.P", Direction::Output).unwrap();
    s.bind(n, "A", "U.P", Direction::Input).unwrap();
    s.bind(n, "B", "U.IN", Direction::Output).unwrap();
    s.bind(u, "IN", "U.IN", Direction::Input).unwrap();
    s.bind(n2, "B", "U.IN2", Direction::Output).unwrap();
    s.bind(u, "IN2", "U.IN2", Direction::Input).unwrap();
    assert_one_cell(&run(s, &j, Time::from_ms(1)).unwrap());
}

/// Gate: a broken promise (an output change inside the quiet grant before
/// the promised time) and a broken wake (the engine reports leaving its
/// sleep early, without an output) each raise `PromiseBroken`.
#[test]
fn broken_promises_and_wakes_raise_promise_broken() {
    // u wakes at 5 ms but claims to be quiet until 9 ms; n has no edge before
    // 7.5 ms... except the 3.000001 ms one, so use a board where n is silent.
    let (s, j) = board(true, |c| {
        c.lie = Some(Time::from_ms(9));
        c.wakes = vec![us(2_500)];
    });
    let _ = j;
    let mut s = s;
    let mut sink = EventSink::new(Vec::new());
    let e = s.run(Time::from_ms(10), &mut sink, &mut []).unwrap_err();
    match e {
        SchedError::PromiseBroken {
            engine,
            promised,
            at,
        } => {
            assert_eq!(engine, "u");
            assert_eq!(promised, Time::from_ms(9));
            assert_eq!(at, us(2_500));
        }
        e => panic!("{e}"),
    }
    // The records of completed grants were flushed; the failed grant's
    // grant record was emitted (the stop was known) but none of its edges.
    let events = String::from_utf8(sink.into_inner()).unwrap();
    assert!(!events.contains("pin_edge"), "{events}");

    // A wake without an output at 1 ms, inside a grant that relied on the
    // promise.
    let (s, j) = board(true, |c| c.wake_at = Some(us(1_000)));
    match run(s, &j, Time::from_ms(10)) {
        Err(SchedError::PromiseBroken {
            engine,
            promised,
            at,
        }) => {
            assert_eq!(engine, "u");
            assert_eq!(promised, Time::from_ms(5));
            assert_eq!(at, us(1_000));
        }
        Err(e) => panic!("{e}"),
        Ok(_) => panic!("the broken wake was not caught"),
    }
}

/// ADR-008 D6 (Phase 1 M1): a CPU that promises `Time::MAX` (the clamp of
/// a promise past `Time`'s range) gets grants capped at `max_quantum` and
/// at the producer's early stops, never past them, and the run's records
/// are the one-cell run's except `grant`.
#[test]
fn a_promise_of_time_max_is_capped_by_max_quantum() {
    let t_end = Time::from_ms(50);
    let (s, j) = board(true, |c| c.wakes.clear());
    let on = run(s, &j, t_end).unwrap();
    let (s, j) = board(false, |c| c.wakes.clear());
    let off = run(s, &j, t_end).unwrap();
    assert_eq!(on.non_grant(), off.non_grant());
    assert!(on
        .journal
        .iter()
        .any(|l| l.ends_with(&format!("-> {}", Time::MAX.as_fs()))));
    assert!(on
        .grants
        .iter()
        .all(|(a, b, _)| *b - *a <= Time::from_ms(10)));
    assert!(on.grants.iter().any(|(_, _, l)| l == "max_quantum"));
    assert!(on.grants.len() < off.grants.len());
    assert_eq!(on.grants.last().unwrap().1, t_end);
}

/// Gate: the query back-off is a function of the simulated history only.
/// A CPU that never promises is asked at grants 1, 4, 9, 18, 35, 68, 133,
/// 198, ...: after k consecutive misses the next min(2^k, 64) one-cell grants
/// skip the Query. A promise resets k.
#[test]
fn the_back_off_depends_only_on_history() {
    let asked = |o: &Outcome| -> Vec<usize> {
        o.plans
            .iter()
            .enumerate()
            .filter(|(_, p)| !p.queried.is_empty())
            .map(|(k, _)| k + 1)
            .collect()
    };
    let never = || {
        let (s, j) = board(true, |c| c.never_quiet = true);
        run(s, &j, Time::from_us(2_700)).unwrap()
    };
    let a = never();
    assert_eq!(a.grants.len(), 270);
    assert_eq!(asked(&a), vec![1, 4, 9, 18, 35, 68, 133, 198, 263]);
    assert_eq!(a.non_grant(), feedback_prefix(Time::from_us(2_700)));
    let b = never();
    assert_eq!(asked(&a), asked(&b));
    assert_eq!(a.events, b.events);

    // With promises: asked at the first grant (a promise), then after the
    // reaction at 3.01 ms is pending: a miss (k = 1), two skipped grants,
    // asked again at 3.04 ms (a promise, k = 0).
    let on = feedback(true);
    let from_asked: Vec<Time> = on
        .plans
        .iter()
        .filter(|p| !p.queried.is_empty())
        .map(|p| p.from)
        .take(3)
        .collect();
    assert_eq!(from_asked, vec![Time::ZERO, us(3_010), us(3_040)]);
}

/// The non-grant records of the one-cell feedback run up to `t_end`.
fn feedback_prefix(t_end: Time) -> Vec<String> {
    let (s, j) = board(false, |_| {});
    let o = run(s, &j, t_end).unwrap();
    o.non_grant().into_iter().map(String::from).collect()
}

/// Gate: identical plans (and outputs) across two runs.
#[test]
fn plans_are_identical_across_two_runs() {
    let a = feedback(true);
    let b = feedback(true);
    assert_eq!(a.plans, b.plans);
    assert_eq!(a.events, b.events);
    assert_eq!(a.journal, b.journal);
    assert!(a.plans.iter().any(|p| p.stop == Some(1)));
}

/// The quiet grant never skips an aligned sample or `t_end`, and lookahead
/// is inert on an acyclic graph (no loop cap to lift: nobody is asked).
#[test]
fn quiet_grants_respect_other_bounds() {
    let j: Journal = Rc::default();
    let mut s = Scheduler::new(cfg(true)).unwrap();
    s.record_plans(true);
    let u = s.add_engine(Box::new(Cpu::new("u", &WAKES, &j))).unwrap();
    let n = s
        .add_engine(Box::new(Stopper::new("n", &n_edges(), &j)))
        .unwrap();
    s.bind(n, "B", "U.IN", Direction::Output).unwrap();
    s.bind(u, "IN", "U.IN", Direction::Input).unwrap();
    let o = run(s, &j, Time::from_ms(50)).unwrap();
    assert!(o.plans.iter().all(|p| p.queried.is_empty()));
    assert_eq!(o.grants.len(), 5);
}
