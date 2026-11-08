//! Scheduler tests with mock engines.
//!
//! Properties checked (see spec/time-grant.md):
//! - every grant equals min(t_safe_i, sample, now + max_quantum, t_end);
//! - no engine is ever asked to advance past the grant or past its own t_safe;
//! - an idle engine gets grants of exactly max_quantum;
//! - event stream, CSV and VCD are byte-identical across two runs.

use std::cell::RefCell;
use std::rc::Rc;

use bbcore::{
    CsvWriter, Direction, Engine, EngineError, Event, EventSampling, EventSink, Level, LogLevel,
    LogRecord, Phase, PinSignal, PinSignals, Probe, SchedError, Scheduler, SchedulerConfig, Time,
    VcdKind, VcdWriter,
};

/// One call made by the scheduler to a mock.
#[derive(Clone, Debug, PartialEq)]
enum Call {
    Safe { now: Time, t_safe: Time },
    Advance { to: Time },
    Apply(PinSignals),
}

type Journal = Rc<RefCell<Vec<(String, Call)>>>;

/// Tiny deterministic PRNG (LCG) so "random" schedules are reproducible.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
}

/// Generic recording mock. Its t_safe policy is a closure; it enforces the
/// protocol itself (refuses to advance past t_safe or backwards).
struct Mock {
    name: String,
    now: Time,
    promised: Time,
    policy: Box<dyn FnMut(Time) -> Time>,
    journal: Journal,
    /// Optional square-wave output: (pin, half-period). Edges at k*half.
    toggle: Option<(String, Time)>,
    level: Level,
    edge_time: Time,
    /// Optional analog follower: output pin = 1.9 V when input pin is high.
    follow: Option<(String, String)>,
    input_high: bool,
    logs: Vec<LogRecord>,
}

impl Mock {
    fn new(name: &str, journal: &Journal, policy: impl FnMut(Time) -> Time + 'static) -> Mock {
        Mock {
            name: name.into(),
            now: Time::ZERO,
            promised: Time::ZERO,
            policy: Box::new(policy),
            journal: journal.clone(),
            toggle: None,
            level: Level::Low,
            edge_time: Time::ZERO,
            follow: None,
            input_high: false,
            logs: Vec::new(),
        }
    }
    fn record(&self, c: Call) {
        self.journal.borrow_mut().push((self.name.clone(), c));
    }
}

impl Engine for Mock {
    fn name(&self) -> &str {
        &self.name
    }

    fn next_safe_time(&mut self, now: Time) -> Time {
        assert_eq!(
            now, self.now,
            "{}: kernel and engine disagree on now",
            self.name
        );
        let t = (self.policy)(now);
        self.promised = t;
        self.record(Call::Safe { now, t_safe: t });
        t
    }

    fn advance_to(&mut self, t: Time) -> Result<(), EngineError> {
        self.record(Call::Advance { to: t });
        if t <= self.now {
            return Err(EngineError::Protocol(format!("advance backwards to {t}")));
        }
        if t > self.promised {
            return Err(EngineError::Protocol(format!(
                "asked to advance to {t} past t_safe {}",
                self.promised
            )));
        }
        if let Some((_, half)) = &self.toggle {
            let h = half.as_fs();
            let edges = t.as_fs() / h - self.now.as_fs() / h;
            if edges > 0 {
                if edges % 2 == 1 {
                    self.level = self.level.toggled();
                }
                self.edge_time = Time::from_fs(t.as_fs() / h * h);
                self.logs.push(LogRecord {
                    t: self.edge_time,
                    level: LogLevel::Info,
                    msg: format!("toggle -> {}", self.level.as_bit()),
                });
            }
        }
        self.now = t;
        Ok(())
    }

    fn read_outputs(&mut self) -> Result<PinSignals, EngineError> {
        let mut out = PinSignals::new();
        if let Some((pin, _)) = &self.toggle {
            out.insert(pin.clone(), PinSignal::digital(self.level, self.edge_time));
        }
        if let Some((_, pin)) = &self.follow {
            let v = if self.input_high { 1.9 } else { 0.0 };
            out.insert(pin.clone(), PinSignal::analog(v));
        }
        Ok(out)
    }

    fn apply_inputs(&mut self, inputs: &PinSignals) -> Result<(), EngineError> {
        self.record(Call::Apply(inputs.clone()));
        if let Some((pin, _)) = &self.follow {
            if let Some(sig) = inputs.get(pin) {
                self.input_high = sig.level() == Some(Level::High);
            }
        }
        Ok(())
    }

    fn drain_logs(&mut self) -> Vec<LogRecord> {
        std::mem::take(&mut self.logs)
    }
}

fn events(sink: EventSink<Vec<u8>>) -> Vec<Event> {
    String::from_utf8(sink.into_inner())
        .unwrap()
        .lines()
        .map(|l| Event::from_line(l).unwrap())
        .collect()
}

fn grants(evs: &[Event]) -> Vec<(Time, Time, String)> {
    evs.iter()
        .filter_map(|e| match e {
            Event::Grant {
                t_fs,
                until_fs,
                limited_by,
            } => Some((*t_fs, *until_fs, limited_by.clone())),
            _ => None,
        })
        .collect()
}

fn cfg(max_quantum: Time) -> SchedulerConfig {
    SchedulerConfig {
        max_quantum,
        log_grants: true,
        event_sampling: None,
    }
}

#[test]
fn grant_is_min_of_t_safe() {
    let j: Journal = Rc::default();
    let mut s = Scheduler::new(cfg(Time::from_ms(1))).unwrap();
    // Three engines with pseudo-random lookahead in [1, 300] us.
    for (name, seed) in [("a", 1u64), ("b", 2), ("c", 3)] {
        let mut rng = Lcg(seed);
        s.add_engine(Box::new(Mock::new(name, &j, move |now| {
            now + Time::from_us(1 + rng.next() % 300)
        })))
        .unwrap();
    }
    let t_end = Time::from_ms(20);
    let mut sink = EventSink::new(Vec::new());
    let summary = s.run(t_end, &mut sink, &mut []).unwrap();
    assert_eq!(summary.now, t_end);
    assert_eq!(s.phase(), Phase::Done);

    // Reconstruct, per grant, the t_safe each engine promised.
    let journal = j.borrow();
    let gs = grants(&events(sink));
    assert_eq!(gs.len() as u64, summary.grants);
    let safes: Vec<_> = journal
        .iter()
        .filter_map(|(n, c)| match c {
            Call::Safe { now, t_safe } => Some((n.clone(), *now, *t_safe)),
            _ => None,
        })
        .collect();
    assert_eq!(safes.len(), gs.len() * 3);
    for (k, (from, until, limited_by)) in gs.iter().enumerate() {
        let round = &safes[k * 3..k * 3 + 3];
        assert!(round.iter().all(|(_, now, _)| now == from));
        let (min_name, _, min_t) = round.iter().min_by_key(|(_, _, t)| *t).cloned().unwrap();
        let expected = min_t.min(t_end).min(*from + Time::from_ms(1));
        assert_eq!(*until, expected, "grant {k}");
        if min_t <= t_end {
            // Ties go to the first-registered engine (min_by_key keeps the first).
            assert_eq!(limited_by, &format!("engine:{min_name}"), "grant {k}");
        } else {
            assert_eq!(limited_by, "t_end");
        }
    }
    // Contiguous, increasing, ending at t_end.
    assert_eq!(gs.first().unwrap().0, Time::ZERO);
    for w in gs.windows(2) {
        assert_eq!(w[0].1, w[1].0);
        assert!(w[1].1 > w[1].0);
    }
    assert_eq!(gs.last().unwrap().1, t_end);
}

#[test]
fn no_engine_advances_past_grant_or_t_safe() {
    let j: Journal = Rc::default();
    let mut s = Scheduler::new(cfg(Time::from_us(500))).unwrap();
    let mut rng = Lcg(42);
    s.add_engine(Box::new(Mock::new("jitter", &j, move |now| {
        now + Time::from_ns(1 + rng.next() % 200_000)
    })))
    .unwrap();
    s.add_engine(Box::new(Mock::new("idle", &j, |_| Time::MAX)))
        .unwrap();
    s.add_engine(Box::new(Mock::new("fixed", &j, |now| {
        now + Time::from_us(77)
    })))
    .unwrap();
    let mut sink = EventSink::new(Vec::new());
    // Mocks return Err if asked past their t_safe, so success already proves
    // the property; verify against the grant log too.
    s.run(Time::from_ms(10), &mut sink, &mut []).unwrap();
    let gs = grants(&events(sink));
    let journal = j.borrow();
    for name in ["jitter", "idle", "fixed"] {
        let advances: Vec<Time> = journal
            .iter()
            .filter(|(n, _)| n == name)
            .filter_map(|(_, c)| match c {
                Call::Advance { to } => Some(*to),
                _ => None,
            })
            .collect();
        let expected: Vec<Time> = gs.iter().map(|g| g.1).collect();
        assert_eq!(
            advances, expected,
            "{name} advances must equal the grants exactly"
        );
        // Ordering: each Advance is preceded by a Safe with t_safe >= target.
        let mut last_safe = None;
        for (_, c) in journal.iter().filter(|(n, _)| n == name) {
            match c {
                Call::Safe { t_safe, .. } => last_safe = Some(*t_safe),
                Call::Advance { to } => {
                    let ts = last_safe.take().expect("advance without a fresh t_safe");
                    assert!(*to <= ts);
                }
                Call::Apply(_) => {}
            }
        }
    }
}

#[test]
fn idle_engine_gets_max_quantum_grants() {
    let j: Journal = Rc::default();
    let mut s = Scheduler::new(cfg(Time::from_ms(1))).unwrap();
    s.add_engine(Box::new(Mock::new("idle", &j, |_| Time::MAX)))
        .unwrap();
    let mut sink = EventSink::new(Vec::new());
    s.run(Time::from_ms(10), &mut sink, &mut []).unwrap();
    let gs = grants(&events(sink));
    assert_eq!(gs.len(), 10);
    for (k, (from, until, by)) in gs.iter().enumerate() {
        assert_eq!(*from, Time::from_ms(k as u64));
        assert_eq!(*until - *from, Time::from_ms(1));
        assert_eq!(by, "max_quantum");
    }

    // An idle MCU next to a busy-but-far analog engine: the grant is the
    // analog engine's lookahead, capped by max_quantum and t_end.
    let mut s = Scheduler::new(cfg(Time::from_ms(100))).unwrap();
    s.add_engine(Box::new(Mock::new("mcu", &j, |_| Time::MAX)))
        .unwrap();
    s.add_engine(Box::new(Mock::new("analog", &j, |now| {
        now + Time::from_ms(30)
    })))
    .unwrap();
    let mut sink = EventSink::new(Vec::new());
    s.run(Time::from_ms(70), &mut sink, &mut []).unwrap();
    let gs = grants(&events(sink));
    let spans: Vec<(u64, u64, &str)> = gs
        .iter()
        .map(|(a, b, c)| {
            (
                a.as_fs() / 1_000_000_000_000,
                b.as_fs() / 1_000_000_000_000,
                c.as_str(),
            )
        })
        .collect();
    assert_eq!(
        spans,
        vec![
            (0, 30, "engine:analog"),
            (30, 60, "engine:analog"),
            (60, 70, "t_end")
        ]
    );
}

/// The Phase 0 topology in miniature: a toggling "MCU" pin drives a net that
/// an analog "follower" reads and answers on another net.
fn blinky_run(t_end: Time) -> (Vec<u8>, Vec<u8>, Vec<u8>, Journal) {
    let j: Journal = Rc::default();
    let config = SchedulerConfig {
        max_quantum: Time::from_ms(5),
        log_grants: true,
        event_sampling: Some(EventSampling {
            period: Time::from_ms(10),
            nets: vec!["LED_A".into(), "PA5".into()],
        }),
    };
    let mut s = Scheduler::new(config).unwrap();
    let mut mcu = Mock::new("mcu", &j, |now| {
        // Exact lookahead: next toggle boundary at a multiple of 50 ms.
        let p = Time::from_ms(50).as_fs();
        Time::from_fs((now.as_fs() / p + 1) * p)
    });
    mcu.toggle = Some(("PA5".into(), Time::from_ms(50)));
    let mut analog = Mock::new("analog", &j, |_| Time::MAX);
    analog.follow = Some(("IN".into(), "OUT".into()));
    let mcu = s.add_engine(Box::new(mcu)).unwrap();
    let an = s.add_engine(Box::new(analog)).unwrap();
    s.bind(mcu, "PA5", "PA5", Direction::Output).unwrap();
    s.bind(an, "IN", "PA5", Direction::Input).unwrap();
    s.bind(an, "OUT", "LED_A", Direction::Output).unwrap();

    let mut csv = CsvWriter::new(
        Vec::new(),
        Time::from_ms(1),
        vec!["PA5".into(), "LED_A".into()],
    )
    .unwrap();
    let mut vcd = VcdWriter::new(
        Vec::new(),
        &[("PA5", VcdKind::Wire), ("LED_A", VcdKind::Real)],
    );
    let mut sink = EventSink::new(Vec::new());
    {
        let mut probes: [&mut dyn Probe; 2] = [&mut csv, &mut vcd];
        s.run(t_end, &mut sink, &mut probes).unwrap();
        for p in probes.iter_mut() {
            p.finish().unwrap();
        }
    }
    (sink.into_inner(), csv.into_inner(), vcd.into_inner(), j)
}

#[test]
fn routing_edges_and_outputs() {
    let (ev, csv, vcd, j) = blinky_run(Time::from_ms(250));
    let ev = String::from_utf8(ev).unwrap();
    let evs: Vec<Event> = ev.lines().map(|l| Event::from_line(l).unwrap()).collect();

    // Edges at 50, 100, 150, 200, 250 ms with alternating levels.
    let edges: Vec<(Time, u8)> = evs
        .iter()
        .filter_map(|e| match e {
            Event::PinEdge {
                t_fs,
                net,
                driver,
                level,
            } => {
                assert_eq!(net, "PA5");
                assert_eq!(driver, "mcu.PA5");
                Some((*t_fs, *level))
            }
            _ => None,
        })
        .collect();
    let expected: Vec<(Time, u8)> = (1..=5u64)
        .map(|k| (Time::from_ms(50 * k), (k % 2) as u8))
        .collect();
    assert_eq!(edges, expected);

    // Stream time never goes backwards.
    for w in evs.windows(2) {
        assert!(w[0].time() <= w[1].time(), "{:?} then {:?}", w[0], w[1]);
    }

    // The analog engine saw PA5 through its own pin name "IN".
    let journal = j.borrow();
    let saw_high = journal.iter().any(|(n, c)| {
        n == "analog"
            && matches!(c, Call::Apply(p) if p.get("IN").and_then(|s| s.level()) == Some(Level::High))
    });
    assert!(saw_high);

    // net_sample every 10 ms for both nets (0..=250 ms -> 26 instants).
    let samples = evs
        .iter()
        .filter(|e| matches!(e, Event::NetSample { .. }))
        .count();
    assert_eq!(samples, 26 * 2);
    assert!(
        ev.contains(r#"{"type":"net_sample","t_fs":60000000000000,"net":"LED_A","uv":1900000}"#)
    );

    // CSV: header + 251 rows at 1 ms, analog follows with one-grant latency.
    let csv = String::from_utf8(csv).unwrap();
    let rows: Vec<&str> = csv.lines().collect();
    assert_eq!(rows[0], "t_fs,PA5,LED_A");
    assert_eq!(rows.len(), 1 + 251);
    assert_eq!(rows[1], "0,0,0.000000");
    assert_eq!(rows[51], "50000000000000,1,0.000000");
    assert_eq!(rows[52], "51000000000000,1,1.900000");

    // VCD: deterministic header and the first edge.
    let vcd = String::from_utf8(vcd).unwrap();
    assert!(vcd.starts_with("$version breadboard bbcore $end\n$timescale 1 fs $end\n"));
    assert!(vcd.contains("#50000000000000\n1!\n"));
    assert!(!vcd.contains("$date"));
}

#[test]
fn outputs_are_byte_identical_across_runs() {
    let (e1, c1, v1, _) = blinky_run(Time::from_ms(500));
    let (e2, c2, v2, _) = blinky_run(Time::from_ms(500));
    assert!(!e1.is_empty() && !c1.is_empty() && !v1.is_empty());
    assert_eq!(e1, e2, "events.jsonl differs between runs");
    assert_eq!(c1, c2, "nets.csv differs between runs");
    assert_eq!(v1, v2, "nets.vcd differs between runs");
}

#[test]
fn csv_sampling_aligns_grants() {
    let j: Journal = Rc::default();
    let mut s = Scheduler::new(cfg(Time::from_ms(10))).unwrap();
    s.add_engine(Box::new(Mock::new("idle", &j, |_| Time::MAX)))
        .unwrap();
    let mut csv = CsvWriter::new(Vec::new(), Time::from_us(300), vec![]).unwrap();
    let mut sink = EventSink::new(Vec::new());
    s.run(Time::from_ms(1), &mut sink, &mut [&mut csv]).unwrap();
    let gs = grants(&events(sink));
    let untils: Vec<u64> = gs.iter().map(|g| g.1.as_fs() / 1_000_000_000).collect();
    assert_eq!(untils, vec![300, 600, 900, 1000]);
    assert_eq!(gs[0].2, "sample");
    assert_eq!(gs[3].2, "t_end");
}

#[test]
fn run_resumes_with_larger_t_end() {
    let j: Journal = Rc::default();
    let mut s = Scheduler::new(cfg(Time::from_ms(1))).unwrap();
    s.add_engine(Box::new(Mock::new("idle", &j, |_| Time::MAX)))
        .unwrap();
    let mut sink = EventSink::new(Vec::new());
    s.run(Time::from_ms(2), &mut sink, &mut []).unwrap();
    let r = s.run(Time::from_ms(5), &mut sink, &mut []).unwrap();
    assert_eq!(r.now, Time::from_ms(5));
    assert_eq!(r.grants, 5);
}

#[test]
fn rejects_protocol_violations() {
    let j: Journal = Rc::default();

    // t_safe not after now.
    let mut s = Scheduler::new(cfg(Time::from_ms(1))).unwrap();
    s.add_engine(Box::new(Mock::new("stuck", &j, |now| now)))
        .unwrap();
    let mut sink = EventSink::new(Vec::new());
    assert!(matches!(
        s.run(Time::from_ms(1), &mut sink, &mut []),
        Err(SchedError::NoProgress { .. })
    ));
    assert_eq!(s.phase(), Phase::Error);
    assert!(matches!(
        s.run(Time::from_ms(1), &mut sink, &mut []),
        Err(SchedError::Halted)
    ));

    // Two drivers on one net.
    let mut s = Scheduler::new(cfg(Time::from_ms(1))).unwrap();
    let a = s
        .add_engine(Box::new(Mock::new("a", &j, |_| Time::MAX)))
        .unwrap();
    let b = s
        .add_engine(Box::new(Mock::new("b", &j, |_| Time::MAX)))
        .unwrap();
    s.bind(a, "X", "N", Direction::Output).unwrap();
    assert!(matches!(
        s.bind(b, "Y", "N", Direction::Output),
        Err(SchedError::MultipleDrivers { .. })
    ));
    // Duplicate engine name.
    assert!(matches!(
        s.add_engine(Box::new(Mock::new("a", &j, |_| Time::MAX))),
        Err(SchedError::DuplicateEngine(_))
    ));
    // Zero quantum.
    assert!(Scheduler::new(cfg(Time::ZERO)).is_err());

    // Two edges inside one grant (t_safe too optimistic) is detected.
    let mut s = Scheduler::new(cfg(Time::from_ms(200))).unwrap();
    let mut m = Mock::new("liar", &j, |now| now + Time::from_ms(120));
    m.toggle = Some(("P".into(), Time::from_ms(50)));
    let id = s.add_engine(Box::new(m)).unwrap();
    s.bind(id, "P", "P", Direction::Output).unwrap();
    let mut sink = EventSink::new(Vec::new());
    let err = s.run(Time::from_ms(400), &mut sink, &mut []).unwrap_err();
    assert!(matches!(err, SchedError::Causality { .. }), "{err}");
}
