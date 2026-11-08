//! ADR-003 plan step 9 (D6) gate, kernel side: "an analog-watchdog wake
//! inside a quiet window is caught by the watch, not the audit".
//!
//! The board is a feedback loop like `examples/adc`, with mock engines: a
//! lattice-bound "CPU" `u` (output `P`, ADC input `ADC`) and an "analog"
//! producer `n` (output `V`, pushed `Analog` values at lattice points, as
//! `adc_in` does; input `A`), wired `u.P -> n.A` and `n.V -> u.ADC`.
//!
//! `u` sleeps until its next timer wake (it toggles `P` there) and promises
//! quiet outputs until then while nothing is pending. Unlike Renode's ADC
//! model, which converts only at a timer limit, its analog watchdog is a
//! continuous comparator: a value outside `[LO, HI]`, applied at its
//! lattice point `ceil_L(t)`, wakes it and it toggles `P` at
//! `ceil_L(t) + 1 fs`. So an ADC change can wake it *inside* a promised
//! window, and only the producer's early stop on the ADC pin (the
//! code-change watch) keeps the quiet grant from running past the wake.

use std::cell::RefCell;
use std::rc::Rc;

use bbcore::{
    Direction, Engine, EngineError, Event, EventSink, Level, PinSignal, PinSignals, SchedError,
    Scheduler, SchedulerConfig, Time, Transition,
};

const L: Time = Time::from_us(10);
const LO: f64 = 1.0;
const HI: f64 = 2.0;

fn ceil_l(t: Time) -> Time {
    Time::from_fs(t.as_fs().div_ceil(L.as_fs()) * L.as_fs())
}

type Journal = Rc<RefCell<Vec<String>>>;

struct Mcu {
    now: Time,
    wakes: Vec<Time>,
    /// Watchdog reactions due at `ceil_L(t) + 1 fs`.
    pending: Vec<Time>,
    level: Level,
    edge_time: Time,
    trace: Vec<Transition>,
    /// The value applied last, and whether it was outside the window.
    outside: bool,
    journal: Journal,
}

impl Mcu {
    fn take_value(&mut self, volts: f64, t: Time) {
        let outside = !(LO..=HI).contains(&volts);
        if outside && !self.outside {
            // The comparator fires where Renode would see the value: at its
            // lattice point (never early).
            let at = ceil_l(t).max(self.now) + Time::from_fs(1);
            self.journal.borrow_mut().push(format!(
                "u awd {volts} @{} -> {}",
                t.as_fs(),
                at.as_fs()
            ));
            self.pending.push(at);
        }
        self.outside = outside;
    }
}

impl Engine for Mcu {
    fn name(&self) -> &str {
        "u"
    }
    fn next_safe_time(&mut self, _now: Time) -> Time {
        Time::MAX
    }
    fn grant_lattice(&self) -> Option<Time> {
        Some(L)
    }
    fn quiet_until(&mut self, now: Time) -> Time {
        if !self.pending.is_empty() {
            return now;
        }
        self.wakes
            .iter()
            .copied()
            .find(|&w| w > now)
            .unwrap_or(Time::MAX)
    }
    fn advance_to(&mut self, t: Time) -> Result<(), EngineError> {
        assert!(t > self.now && t.as_fs().is_multiple_of(L.as_fs()), "{t}");
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
        // A snapshot (Exchange): an ADC value carries no time; it holds now.
        if let Some(PinSignal::Analog { volts }) = inputs.get("ADC") {
            let now = self.now;
            self.take_value(*volts, now);
        }
        Ok(())
    }
    fn apply_input_trace(
        &mut self,
        _base: &PinSignals,
        trace: &[Transition],
    ) -> Result<(), EngineError> {
        for tr in trace {
            if let PinSignal::Analog { volts } = tr.signal {
                self.take_value(volts, tr.t);
            }
        }
        Ok(())
    }
}

/// The producer: pushes `(t, volts)` (lattice points) on `V`.
struct Pusher {
    now: Time,
    pushes: Vec<(Time, f64)>,
    value: f64,
    trace: Vec<Transition>,
    /// Stop early on a watched `V` push (the code-change watch). `false`:
    /// an engine that ignores the watch on its ADC pin.
    honour_watch: bool,
}

impl Pusher {
    fn run(&mut self, t: Time) {
        self.trace.clear();
        for &(at, v) in self
            .pushes
            .iter()
            .filter(|(at, _)| self.now < *at && *at <= t)
        {
            self.value = v;
            self.trace.push(Transition {
                t: at,
                pin: "V".into(),
                signal: PinSignal::analog(v),
            });
        }
        self.now = t;
    }
}

impl Engine for Pusher {
    fn name(&self) -> &str {
        "n"
    }
    fn next_safe_time(&mut self, _now: Time) -> Time {
        Time::MAX
    }
    fn advance_to(&mut self, t: Time) -> Result<(), EngineError> {
        self.run(t);
        Ok(())
    }
    fn can_stop_early(&self) -> bool {
        true
    }
    fn advance_until(&mut self, t: Time, watch: &[String]) -> Result<Time, EngineError> {
        let r = if self.honour_watch && watch.iter().any(|w| w == "V") {
            self.pushes
                .iter()
                .map(|(at, _)| *at)
                .find(|&at| self.now < at && at <= t)
                .unwrap_or(t)
        } else {
            t
        };
        self.run(r);
        Ok(r)
    }
    fn read_outputs(&mut self) -> Result<PinSignals, EngineError> {
        let mut m = PinSignals::new();
        m.insert("V".into(), PinSignal::analog(self.value));
        Ok(m)
    }
    fn read_output_trace(&mut self) -> Result<Option<Vec<Transition>>, EngineError> {
        Ok(Some(self.trace.clone()))
    }
    fn apply_inputs(&mut self, _inputs: &PinSignals) -> Result<(), EngineError> {
        Ok(())
    }
}

/// Timer wakes of `u` (on and off the lattice).
fn wakes() -> Vec<Time> {
    vec![
        Time::from_ms(10),
        Time::from_ns(23_456_789),
        Time::from_ms(40),
    ]
}

/// `V`: inside the window, out of it at 3.21 ms (inside u's first promised
/// window, (0, 10 ms)), back in, out again at 31.05 ms (inside (23.46, 40)
/// ms), plus in-window changes that wake nobody.
fn pushes() -> Vec<(Time, f64)> {
    vec![
        (Time::from_us(1_000), 1.5),
        (Time::from_us(3_210), 2.4), // out of the window: AWD wake
        (Time::from_us(5_000), 1.7),
        (Time::from_us(15_000), 1.9),
        (Time::from_us(31_050), 0.4), // out of the window: AWD wake
        (Time::from_us(33_000), 1.2),
    ]
}

fn board(lookahead: bool, honour_watch: bool) -> (Scheduler, Journal) {
    let j: Journal = Rc::default();
    let mut s = Scheduler::new(SchedulerConfig {
        max_quantum: Time::from_ms(10),
        loop_quantum: None,
        lookahead,
        log_grants: true,
        event_sampling: None,
    })
    .unwrap();
    let u = s
        .add_engine(Box::new(Mcu {
            now: Time::ZERO,
            wakes: wakes(),
            pending: Vec::new(),
            level: Level::Low,
            edge_time: Time::ZERO,
            trace: Vec::new(),
            outside: false,
            journal: j.clone(),
        }))
        .unwrap();
    let n = s
        .add_engine(Box::new(Pusher {
            now: Time::ZERO,
            pushes: pushes(),
            value: 1.5,
            trace: Vec::new(),
            honour_watch,
        }))
        .unwrap();
    s.bind(u, "P", "U.P", Direction::Output).unwrap();
    s.bind(n, "A", "U.P", Direction::Input).unwrap();
    s.bind(n, "V", "U.ADC", Direction::Output).unwrap();
    s.bind(u, "ADC", "U.ADC", Direction::Input).unwrap();
    (s, j)
}

struct Outcome {
    events: String,
    journal: Vec<String>,
}

impl Outcome {
    fn grants(&self) -> Vec<(Time, Time, String)> {
        self.events
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
    fn non_grant(&self) -> Vec<&str> {
        self.events
            .lines()
            .filter(|l| !l.contains("\"type\":\"grant\""))
            .collect()
    }
}

fn run(lookahead: bool, honour_watch: bool) -> Result<Outcome, SchedError> {
    let (mut s, j) = board(lookahead, honour_watch);
    let mut sink = EventSink::new(Vec::new());
    let r = s.run(Time::from_ms(50), &mut sink, &mut []);
    let events = String::from_utf8(sink.into_inner()).unwrap();
    r.map(|_| Outcome {
        events,
        journal: j.borrow().clone(),
    })
}

/// The gate. With the watch, both watchdog wakes that fall inside a
/// promised window end the quiet grant at their push (`stop:n` at 3.21 ms
/// and 31.05 ms), `u` receives the value before it advances and wakes at
/// `ceil_L(t) + 1 fs`, exactly as in the one-cell run; no promise is broken
/// (the audit never fires) and the records equal the one-cell run's.
#[test]
fn an_awd_wake_inside_a_quiet_window_is_caught_by_the_watch() {
    let off = run(false, true).expect("one-cell run");
    let on = run(true, true).expect("lookahead run: no PromiseBroken");
    assert_eq!(off.grants().len(), 5000);
    assert_eq!(on.non_grant(), off.non_grant());
    // The wakes: 3 timer edges and 2 watchdog edges on P.
    let edges: Vec<&str> = on.non_grant();
    assert_eq!(edges.len(), 5, "{edges:#?}");
    assert!(edges[0].contains("\"t_fs\":3210000000001"), "{}", edges[0]);
    let g = on.grants();
    let fs1 = Time::from_fs(1);
    // The first quiet grant was promised to 10 ms and stopped at the first
    // push (1 ms, in the window: no wake), the next at the out-of-window push.
    assert_eq!(g[0], (Time::ZERO, Time::from_ms(1), "stop:n".to_string()));
    assert!(g.contains(&(Time::from_ms(1), Time::from_us(3_210), "stop:n".to_string())));
    assert!(g
        .iter()
        .any(|x| x.1 == Time::from_us(31_050) && x.2 == "stop:n"));
    // Both watchdog wakes happened inside a window u had promised.
    for w in [Time::from_us(3_210) + fs1, Time::from_us(31_050) + fs1] {
        assert!(
            on.journal
                .iter()
                .any(|l| l.ends_with(&format!("-> {}", w.as_fs()))),
            "{:#?}",
            on.journal
        );
    }
    assert!(g.len() < 60, "{} grants", g.len());
}

/// The control: a producer that ignores the watch on its ADC pin would run
/// the quiet grant past the push, and `u` would wake inside it. The kernel
/// does not let that through: its early-stop check (`r` must be `ceil_L`
/// of the first watched change, invariant 8) fails the run right after the
/// producer's advance, before the quiet `u` advances.
#[test]
fn ignoring_the_adc_watch_fails_the_run() {
    match run(true, false) {
        Err(SchedError::Engine { engine, source, .. }) => {
            assert_eq!(engine, "n");
            assert!(
                matches!(&source, EngineError::Protocol(m) if m.contains("D5.3")),
                "{source:?}"
            );
        }
        Err(e) => panic!("unexpected error {e}"),
        Ok(_) => panic!("a producer ignoring the ADC watch must fail the run"),
    }
}
