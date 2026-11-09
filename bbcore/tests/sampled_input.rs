//! ADR-005 plan step 3 (D3, test plan 5): sampled inputs, kernel side.
//!
//! The board is a feedback loop like `examples/adc` on Breadboard's ADC
//! model, with mock engines: a lattice-bound "CPU" `u` (output `P`, ADC input
//! `ADC`) and an "analog" producer `n` (output `V`, pushed `Analog` values at
//! lattice points; input `A`), wired `u.P -> n.A` and `n.V -> u.ADC`.
//!
//! `u` converts only at its own sample instants (on and off the lattice):
//! the value of the latest push strictly before the instant, and `P` shows
//! whether it is above 1.5 V, changing at the sample instant. It promises
//! quiet outputs until its next sample instant. With `ADC` declared
//! sampled (`Engine::input_is_sampled`), the kernel leaves `n.V` out of the
//! early-stop watch, so the quiet grants run to `G` (no `stop:n`) and the
//! records equal the one-cell run's. The control declares nothing and
//! stops at every push, with the same records. A dishonest engine declares
//! the input sampled but reacts to a push at once: the kernel's audit
//! (invariant 8) or the engine's own wake check fails the run with
//! `PromiseBroken`.

use bbcore::{
    Direction, Engine, EngineError, Event, EventSink, Level, PinSignal, PinSignals, SchedError,
    Scheduler, SchedulerConfig, Time, Transition,
};

const L: Time = Time::from_us(10);

fn ceil_l(t: Time) -> Time {
    Time::from_fs(t.as_fs().div_ceil(L.as_fs()) * L.as_fs())
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Honesty {
    /// Reads the input at sample instants only.
    Sampled,
    /// Declares the input sampled but toggles `P` at `ceil_L(t) + 1 fs` of
    /// every push (as a comparator would); the kernel audit must catch it.
    WakesOnPush,
    /// The same, but the engine reports its own early wake from the
    /// advance (`EngineError::PromiseBroken`), like Renode's WFI audit.
    ReportsWake,
}

struct Mcu {
    now: Time,
    samples: Vec<Time>,
    honesty: Honesty,
    declare_sampled: bool,
    /// Timed input values `(t, volts)`, `t` increasing.
    inputs: Vec<(Time, f64)>,
    /// Push reactions (dishonest variants).
    pending: Vec<Time>,
    /// The promise of the last `quiet_until`.
    promised: Option<(Time, Time)>,
    level: Level,
    edge_time: Time,
    trace: Vec<Transition>,
}

impl Mcu {
    fn value_before(&self, s: Time) -> f64 {
        self.inputs
            .iter()
            .rev()
            .find(|(t, _)| *t < s)
            .map_or(0.0, |(_, v)| *v)
    }

    fn take(&mut self, t: Time, volts: f64) {
        if self.inputs.last().is_some_and(|(_, v)| *v == volts) {
            return;
        }
        self.inputs.push((t, volts));
        if self.honesty != Honesty::Sampled && t > Time::ZERO {
            self.pending
                .push(ceil_l(t).max(self.now) + Time::from_fs(1));
        }
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
    fn input_is_sampled(&self, pin: &str) -> bool {
        self.declare_sampled && pin == "ADC"
    }
    fn quiet_until(&mut self, now: Time) -> Time {
        self.promised = None;
        if !self.pending.is_empty() {
            return now;
        }
        let q = self
            .samples
            .iter()
            .copied()
            .find(|&s| s > now)
            .unwrap_or(Time::MAX);
        self.promised = Some((now, q));
        q
    }
    fn advance_to(&mut self, t: Time) -> Result<(), EngineError> {
        assert!(t > self.now && t.as_fs().is_multiple_of(L.as_fs()), "{t}");
        let now = self.now;
        let promise = self.promised.take().filter(|(from, _)| *from == now);
        let mut ev: Vec<(Time, Option<Level>)> = self
            .samples
            .iter()
            .filter(|&&s| now < s && s <= t)
            .map(|&s| {
                let lv = Level::from_bool(self.value_before(s) > 1.5);
                (s, Some(lv))
            })
            .chain(
                self.pending
                    .iter()
                    .filter(|&&p| now < p && p <= t)
                    .map(|&p| (p, None)),
            )
            .collect();
        ev.sort_by_key(|e| e.0);
        if self.honesty == Honesty::ReportsWake {
            if let (Some((_, q)), Some(&(p, None))) = (promise, ev.iter().find(|e| e.1.is_none())) {
                if p < q {
                    return Err(EngineError::PromiseBroken { promised: q, at: p });
                }
            }
        }
        self.pending.retain(|&p| p > t);
        self.trace.clear();
        for (e, lv) in ev {
            let new = lv.unwrap_or(self.level.toggled());
            if new == self.level {
                continue;
            }
            self.level = new;
            self.edge_time = e;
            self.trace.push(Transition {
                t: e,
                pin: "P".into(),
                signal: PinSignal::digital(new, e),
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
        if let Some(PinSignal::Analog { volts }) = inputs.get("ADC") {
            let now = self.now;
            self.take(now, *volts);
        }
        Ok(())
    }
    fn apply_input_trace(
        &mut self,
        base: &PinSignals,
        trace: &[Transition],
    ) -> Result<(), EngineError> {
        self.apply_inputs(base)?;
        for tr in trace {
            if let PinSignal::Analog { volts } = tr.signal {
                self.take(tr.t, volts);
            }
        }
        Ok(())
    }
}

/// The producer: pushes `(t, volts)` (lattice points) on `V`, and stops
/// early at the first push when `V` is watched.
struct Pusher {
    now: Time,
    pushes: Vec<(Time, f64)>,
    value: f64,
    trace: Vec<Transition>,
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
        let r = if watch.iter().any(|w| w == "V") {
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

/// Sample instants: every 999 us plus 1.063 us (off the lattice), and two on
/// lattice points.
fn samples() -> Vec<Time> {
    let mut s: Vec<Time> = (1..50)
        .map(|k| Time::from_ns(k * 999_000 + 1_063))
        .collect();
    s.push(Time::from_us(20_000));
    s.push(Time::from_us(33_330));
    s.sort();
    s
}

/// Pushes at lattice points, many between samples, two exactly on the
/// on-lattice sample instants (they must not be seen there) and one right
/// before a sample's lattice cell.
fn pushes() -> Vec<(Time, f64)> {
    let mut p = Vec::new();
    for k in 0..300u64 {
        let t = Time::from_us(10 + k * 160);
        let v = if (k / 25) % 2 == 0 { 1.0 } else { 2.0 };
        p.push((t, v + (k % 7) as f64 * 0.01));
    }
    p.push((Time::from_us(20_000), 0.2));
    p.push((Time::from_us(33_330), 2.9));
    p.sort_by_key(|x| x.0);
    p.dedup_by_key(|x| x.0);
    p
}

fn board(lookahead: bool, honesty: Honesty, declare_sampled: bool) -> Scheduler {
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
            samples: samples(),
            honesty,
            declare_sampled,
            inputs: Vec::new(),
            pending: Vec::new(),
            promised: None,
            level: Level::Low,
            edge_time: Time::ZERO,
            trace: Vec::new(),
        }))
        .unwrap();
    let n = s
        .add_engine(Box::new(Pusher {
            now: Time::ZERO,
            pushes: pushes(),
            value: 1.0,
            trace: Vec::new(),
        }))
        .unwrap();
    s.bind(u, "P", "U.P", Direction::Output).unwrap();
    s.bind(n, "A", "U.P", Direction::Input).unwrap();
    s.bind(n, "V", "U.ADC", Direction::Output).unwrap();
    s.bind(u, "ADC", "U.ADC", Direction::Input).unwrap();
    s
}

struct Outcome {
    events: String,
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
    fn stops(&self) -> usize {
        self.grants().iter().filter(|g| g.2 == "stop:n").count()
    }
}

fn run(lookahead: bool, honesty: Honesty, declare_sampled: bool) -> Result<Outcome, SchedError> {
    let mut s = board(lookahead, honesty, declare_sampled);
    let mut sink = EventSink::new(Vec::new());
    let r = s.run(Time::from_ms(50), &mut sink, &mut []);
    let events = String::from_utf8(sink.into_inner()).unwrap();
    r.map(|_| Outcome { events })
}

/// The gate: the sampled input is not watched, the quiet grants run to `G`
/// (no early stop at a push), and every record but the grants equals the
/// one-cell run's.
#[test]
fn a_sampled_input_is_not_watched_and_equals_the_one_cell_run() {
    let off = run(false, Honesty::Sampled, true).expect("one-cell run");
    let on = run(true, Honesty::Sampled, true).expect("lookahead run");
    assert_eq!(off.grants().len(), 5000);
    assert_eq!(on.non_grant(), off.non_grant());
    let edges = on.non_grant();
    assert!(edges.len() >= 10, "{edges:#?}");
    // The pushes exactly on the on-lattice samples are not seen there: the
    // 20 ms sample reads the 19.85 ms value (no 0.2 V).
    assert_eq!(on.stops(), 0);
    let g = on.grants();
    assert!(g.len() < 250, "{} grants", g.len());
    assert!(
        g.iter().any(|x| x.2 == "engine:u"),
        "no grant ends at a promise: {g:?}"
    );
}

/// The control: the same engine without the declaration keeps the
/// code-change watch, stopping the producer at the pushes, with the same
/// records.
#[test]
fn a_non_sampled_input_still_stops_the_producer() {
    let off = run(false, Honesty::Sampled, false).expect("one-cell run");
    let on = run(true, Honesty::Sampled, false).expect("lookahead run");
    assert_eq!(on.non_grant(), off.non_grant());
    assert!(on.stops() > 100, "{} stop:n grants", on.stops());
    let sampled = run(true, Honesty::Sampled, true).unwrap();
    assert_eq!(sampled.non_grant(), off.non_grant());
    eprintln!(
        "grants: sampled {}, watched {}",
        sampled.grants().len(),
        on.grants().len()
    );
    assert!(sampled.grants().len() * 2 < on.grants().len());
}

/// A dishonest engine: declares `ADC` sampled but changes `P` at a push.
/// The quiet grant runs past the push (nothing is watched) and the kernel's
/// audit catches the output change before the promise.
#[test]
fn a_wake_on_a_sampled_input_is_a_broken_promise() {
    match run(true, Honesty::WakesOnPush, true) {
        Err(SchedError::PromiseBroken {
            engine,
            promised,
            at,
        }) => {
            assert_eq!(engine, "u");
            assert!(at < promised, "{at} {promised}");
            assert_eq!(at.as_fs() % L.as_fs(), 1, "{at}");
        }
        Err(e) => panic!("unexpected error {e}"),
        Ok(_) => panic!("a wake on a sampled input must fail the run"),
    }
    // The engine's own wake check (Renode's WFI audit) reports it first.
    match run(true, Honesty::ReportsWake, true) {
        Err(SchedError::PromiseBroken { engine, .. }) => assert_eq!(engine, "u"),
        Err(e) => panic!("unexpected error {e}"),
        Ok(_) => panic!("a reported wake must fail the run"),
    }
    // Without lookahead nothing is promised and the same engine runs.
    run(false, Honesty::WakesOnPush, true).expect("one-cell run");
}
