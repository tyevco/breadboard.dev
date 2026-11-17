//! ADR-014 gate G1 (the kernel part of `make multi-producer`): quiet grants
//! with several producers under the real `Scheduler`.
//!
//! The board: a mock lattice-bound CPU `U1` that drives a fixed PWM pattern
//! into the `dms/mechanism@2` device `M1` (whose inputs cannot end its
//! promise, ADR-014 D2), reads M1's HOME_N (an input that can end the CPU's
//! promise) and samples a mock analog engine `A` through a **sampled** input
//! (ADR-005 D3); `A` reads HOME_N too. The CPU never promises (it is awake),
//! so the only promise is M1's next sample, and with `Q = {M1}` no producer
//! needs an early stop (`|P| = 0`, ADR-014 D1): every engine advances to `G`
//! in the remaining graph's order `A -> U1 -> M1`.
//!
//! The gates:
//! - lookahead on equals the one-cell reference in every non-grant record,
//!   at `max_quantum` 1 ms and 10 ms and in the other registration order,
//!   with far fewer grants; the quiet grants have `Q = {M1}` and no stop;
//! - negative control 1: a device that declares promise-independent inputs
//!   but reacts to an input with an output timer before its promise fails
//!   the lookahead run with `PromiseBroken` (the audit, invariant 8); one
//!   that sets a pin in `on_pin_change` is refused by rule 1 in any run, and
//!   one that sets a pin in an internal timer by the engine (ADR-014 D2);
//! - negative control 2 (the scope rule): one CPU -> analog edge added makes
//!   the remaining graph cyclic, so every grant is one cell;
//! - a device output exactly at `G` (its sample on a lattice point) reaches a
//!   pure consumer the way the one-cell plan delivers it (ADR-014 Amendment
//!   1: the order of a grant without producers breaks ties by the static
//!   plan order, so the consumer, after the device in the plan, still gets
//!   the change before it advances).

use std::collections::BTreeMap;

use bbcore::{
    Direction, Engine, EngineError, EventSink, Level, LogLevel, LogRecord, PinSignal, PinSignals,
    SchedError, Scheduler, SchedulerConfig, Time, Transition,
};
use bbdev::{
    Ctx, Device, DeviceEngine, DeviceEngineConfig, DevicePart, DevicePin, ParamValue, PinDir,
};

const L: Time = Time::from_us(10);
const OUTS: [&str; 5] = ["P0", "P1", "P2", "P3", "PS"];
const DEV_INS: [&str; 5] = ["IN_A1", "IN_A2", "IN_B1", "IN_B2", "NSLEEP"];

fn ms(x: u64) -> Time {
    Time::from_ms(x)
}

fn us(x: u64) -> Time {
    Time::from_us(x)
}

fn ceil_l(t: Time) -> Time {
    Time::from_fs(t.as_fs().div_ceil(L.as_fs()) * L.as_fs())
}

/// The CPU's fixed drive: nSLEEP high at 1 ms, then a 25 kHz two-phase
/// pattern stepping the rotor (each full step 4 ms) until 60 ms, then coast
/// and sleep; every edge 17 fs after a whole number of ns (off the lattice).
fn drive() -> Vec<(Time, usize, Level)> {
    let mut ev = vec![(us(1_000) + Time::from_fs(17), 4, Level::High)];
    let period = us(40);
    let mut t = ms(2) + Time::from_fs(17);
    let mut k = 0u64;
    while t < ms(60) {
        // Full step k: coil A +, coil B + or -, as the phase advances.
        let ph = (k / 100) % 4;
        let (a, b) = [(true, true), (false, true), (false, false), (true, false)][ph as usize];
        let on = Time::from_ns(30_000 + 977 * (k % 7));
        let lv = Level::from_bool;
        ev.push((t, 0, lv(a)));
        ev.push((t, 1, lv(!a)));
        ev.push((t, 2, lv(b)));
        ev.push((t, 3, lv(!b)));
        // Slow decay for the rest of the period: brake (both high).
        ev.push((t + on, 0, Level::High));
        ev.push((t + on, 1, Level::High));
        ev.push((t + on, 2, Level::High));
        ev.push((t + on, 3, Level::High));
        t = t + period;
        k += 1;
    }
    for p in 0..4 {
        ev.push((ms(61) + Time::from_fs(17), p, Level::Low));
    }
    ev.push((ms(63) + Time::from_fs(17), 4, Level::Low));
    ev.sort_by_key(|e| e.0);
    ev
}

/// The mock CPU: replays `drive()`, applies HOME_N at `ceil_L` and logs it,
/// and samples the analog value `ADC` every 2 ms at `k 2 ms + 4321 ns` (the
/// latest push strictly before the instant, as Breadboard's ADC models do).
struct Cpu {
    now: Time,
    planned: Vec<(Time, usize, Level)>,
    next: usize,
    levels: [Level; 5],
    edges: [Time; 5],
    trace: Vec<Transition>,
    /// HOME_N changes to apply: (effect time, level).
    home: Vec<(Time, Level)>,
    home_level: Option<Level>,
    /// ADC pushes: (time, volts), in time order.
    adc: Vec<(Time, f64)>,
    next_sample: Time,
    logs: Vec<LogRecord>,
}

impl Cpu {
    fn new() -> Cpu {
        Cpu {
            now: Time::ZERO,
            planned: drive(),
            next: 0,
            levels: [Level::Low; 5],
            edges: [Time::ZERO; 5],
            trace: Vec::new(),
            home: Vec::new(),
            home_level: None,
            adc: Vec::new(),
            next_sample: ms(2) + Time::from_ns(4_321),
            logs: Vec::new(),
        }
    }

    fn log(&mut self, t: Time, msg: String) {
        self.logs.push(LogRecord {
            t,
            level: LogLevel::Info,
            msg,
        });
    }

    fn apply_home(&mut self, upto: Time) {
        while let Some(&(te, l)) = self.home.first() {
            if te > upto {
                break;
            }
            self.home.remove(0);
            if self.home_level != Some(l) {
                self.home_level = Some(l);
                self.log(te, format!("HOME_N {l:?}"));
            }
        }
    }

    fn sample(&mut self, s: Time) {
        let v = self
            .adc
            .iter()
            .rev()
            .find(|(t, _)| *t < s)
            .map_or(f64::NAN, |(_, v)| *v);
        self.log(s, format!("adc {v:.6}"));
    }
}

impl Engine for Cpu {
    fn name(&self) -> &str {
        "U1"
    }
    fn next_safe_time(&mut self, _now: Time) -> Time {
        Time::MAX
    }
    fn grant_lattice(&self) -> Option<Time> {
        Some(L)
    }
    fn advance_to(&mut self, g: Time) -> Result<(), EngineError> {
        self.trace.clear();
        loop {
            let h = self.home.first().map(|h| h.0).filter(|t| *t <= g);
            let s = Some(self.next_sample).filter(|t| *t <= g);
            let e = self.planned.get(self.next).map(|e| e.0).filter(|t| *t <= g);
            let Some(t) = [h, s, e].into_iter().flatten().min() else {
                break;
            };
            if h == Some(t) {
                self.apply_home(t);
            } else if s == Some(t) {
                self.sample(t);
                self.next_sample = t + ms(2);
            } else {
                let (t, p, l) = self.planned[self.next];
                self.next += 1;
                if self.levels[p] != l {
                    self.levels[p] = l;
                    self.edges[p] = t;
                    self.trace.push(Transition {
                        t,
                        pin: OUTS[p].into(),
                        signal: PinSignal::digital(l, t),
                    });
                }
            }
        }
        self.now = g;
        Ok(())
    }
    fn read_outputs(&mut self) -> Result<PinSignals, EngineError> {
        Ok((0..5)
            .map(|p| {
                (
                    OUTS[p].to_string(),
                    PinSignal::digital(self.levels[p], self.edges[p]),
                )
            })
            .collect())
    }
    fn read_output_trace(&mut self) -> Result<Option<Vec<Transition>>, EngineError> {
        Ok(Some(self.trace.clone()))
    }
    fn drain_logs(&mut self) -> Vec<LogRecord> {
        let now = self.now;
        let (ready, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.logs)
            .into_iter()
            .partition(|r| r.t <= now);
        self.logs = later;
        ready
    }
    fn apply_inputs(&mut self, inputs: &PinSignals) -> Result<(), EngineError> {
        if let Some(l) = inputs.get("PH").and_then(PinSignal::level) {
            self.home.push((self.now, l));
            self.apply_home(self.now);
        }
        if let Some(PinSignal::Analog { volts }) = inputs.get("ADC") {
            if self.adc.last().is_none_or(|(_, v)| v != volts) {
                self.adc.push((self.now, *volts));
            }
        }
        Ok(())
    }
    fn apply_input_trace(
        &mut self,
        _base: &PinSignals,
        trace: &[Transition],
    ) -> Result<(), EngineError> {
        for tr in trace {
            match tr.pin.as_str() {
                "PH" => {
                    let l = tr.signal.level().unwrap();
                    if tr.t <= self.now {
                        self.home.push((self.now, l));
                        self.apply_home(self.now);
                    } else {
                        self.home.push((ceil_l(tr.t), l));
                    }
                }
                "ADC" => {
                    let PinSignal::Analog { volts } = tr.signal else {
                        panic!("ADC is analog")
                    };
                    self.adc.push((tr.t.max(self.now), volts));
                }
                _ => {}
            }
        }
        Ok(())
    }
    /// ADR-005 D3: the ADC is read only at the CPU's own sample instants.
    fn input_is_sampled(&self, pin: &str) -> bool {
        pin == "ADC"
    }
}

/// The mock analog engine: `V` changes at every 1 ms lattice point to
/// `1 + k/1000 + HOME/2`, from the HOME level it has applied; it reports
/// each change in its trace. `GLUE` (scope control) is read and ignored.
struct Analog {
    now: Time,
    home: f64,
    v: f64,
    trace: Vec<Transition>,
}

impl Engine for Analog {
    fn name(&self) -> &str {
        "A"
    }
    fn next_safe_time(&mut self, _now: Time) -> Time {
        Time::MAX
    }
    fn advance_to(&mut self, g: Time) -> Result<(), EngineError> {
        self.advance_until(g, &[]).map(|_| ())
    }
    fn can_stop_early(&self) -> bool {
        true
    }
    fn advance_until(&mut self, g: Time, _watch: &[String]) -> Result<Time, EngineError> {
        self.trace.clear();
        let mut k = self.now.as_fs() / ms(1).as_fs() + 1;
        while Time::from_fs(k * ms(1).as_fs()) <= g {
            let t = Time::from_fs(k * ms(1).as_fs());
            self.v = 1.0 + k as f64 / 1000.0 + self.home / 2.0;
            self.trace.push(Transition {
                t,
                pin: "V".into(),
                signal: PinSignal::Analog { volts: self.v },
            });
            k += 1;
        }
        self.now = g;
        Ok(g)
    }
    fn read_outputs(&mut self) -> Result<PinSignals, EngineError> {
        Ok([("V".to_string(), PinSignal::Analog { volts: self.v })].into())
    }
    fn read_output_trace(&mut self) -> Result<Option<Vec<Transition>>, EngineError> {
        Ok(Some(self.trace.clone()))
    }
    fn apply_inputs(&mut self, inputs: &PinSignals) -> Result<(), EngineError> {
        if let Some(l) = inputs.get("HOME").and_then(PinSignal::level) {
            self.home = if l == Level::High { 1.0 } else { 0.0 };
        }
        Ok(())
    }
    fn apply_input_trace(
        &mut self,
        _base: &PinSignals,
        trace: &[Transition],
    ) -> Result<(), EngineError> {
        // A HOME change inside a grant would be applied at the next lattice
        // point; with M1 quiet it only arrives at a boundary.
        for tr in trace.iter().filter(|tr| tr.pin == "HOME") {
            assert!(
                tr.t <= self.now,
                "HOME changed inside the analog engine's grant"
            );
            self.home = if tr.signal.level() == Some(Level::High) {
                1.0
            } else {
                0.0
            };
        }
        Ok(())
    }
}

/// The negative-control devices (ADR-014 D2): each declares
/// promise-independent inputs and breaks it in one way.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Neg {
    /// `on_pin_change` schedules an output timer 1 us later, before the
    /// device's promise (its next 5 ms sample): caught by the audit.
    ReactOutput,
    /// `on_pin_change` calls `set_pin`: refused by rule 1.
    SetInInput,
    /// An internal timer sets a pin: refused by the engine.
    InternalSets,
}

struct NegDevice {
    kind: Neg,
    out: Level,
}

const NEG_PINS: [DevicePin; 6] = [
    DevicePin {
        name: "IN_A1",
        dir: PinDir::In,
    },
    DevicePin {
        name: "IN_A2",
        dir: PinDir::In,
    },
    DevicePin {
        name: "IN_B1",
        dir: PinDir::In,
    },
    DevicePin {
        name: "IN_B2",
        dir: PinDir::In,
    },
    DevicePin {
        name: "NSLEEP",
        dir: PinDir::In,
    },
    DevicePin {
        name: "HOME_N",
        dir: PinDir::Out,
    },
];

impl Device for NegDevice {
    fn pins(&self) -> &[DevicePin] {
        &NEG_PINS
    }
    fn start(&mut self, ctx: &mut Ctx) {
        self.out = Level::High;
        ctx.set_pin(5, self.out);
        ctx.schedule_output(ms(5) + Time::from_ns(40), 1);
    }
    fn on_pin_change(&mut self, ctx: &mut Ctx, pin: usize, level: Level, t: Time) {
        if pin != 4 || level != Level::High {
            return;
        }
        match self.kind {
            Neg::ReactOutput => ctx.schedule_output(t + us(1), 2),
            Neg::SetInInput => ctx.set_pin(5, Level::Low),
            Neg::InternalSets => ctx.schedule(t + us(1), 2),
        }
    }
    fn on_timer(&mut self, ctx: &mut Ctx, id: u32, t: Time) {
        self.out = self.out.toggled();
        ctx.set_pin(5, self.out);
        if id == 1 {
            ctx.schedule_output(t + ms(5), 1);
        }
    }
    fn promise_independent_inputs(&self) -> bool {
        true
    }
}

struct Out {
    events: Vec<String>,
    grants: u64,
    quiet: usize,
    quiet_sets: Vec<(Vec<usize>, Option<usize>, Vec<usize>)>,
}

#[derive(Clone, Copy)]
struct Opts {
    lookahead: bool,
    max_quantum: Time,
    analog_first: bool,
    /// Scope control: the CPU's P0 also feeds the analog engine.
    glue: bool,
    neg: Option<Neg>,
}

const BASE: Opts = Opts {
    lookahead: true,
    max_quantum: Time::from_ms(10),
    analog_first: false,
    glue: false,
    neg: None,
};

fn run(o: Opts) -> Result<Out, SchedError> {
    let mut s = Scheduler::new(SchedulerConfig {
        max_quantum: o.max_quantum,
        loop_quantum: None,
        lookahead: o.lookahead,
        log_grants: true,
        event_sampling: None,
    })
    .unwrap();
    s.record_plans(true);
    let device: Box<dyn Device> = match o.neg {
        None => bbdev::find("dms/mechanism@2")
            .unwrap()
            .instantiate(&[
                ("s0".into(), ParamValue::Real(330.0)),
                ("ts".into(), ParamValue::Time(ms(5))),
            ])
            .unwrap(),
        Some(kind) => Box::new(NegDevice {
            kind,
            out: Level::Low,
        }),
    };
    let devices = DeviceEngine::new(
        DeviceEngineConfig {
            name: "devices".into(),
            seed: 1,
            stop_lattice: Some(L),
        },
        vec![DevicePart {
            reference: "M1".into(),
            device,
        }],
    )
    .unwrap();
    let analog = Analog {
        now: Time::ZERO,
        home: 0.0,
        v: 1.0,
        trace: Vec::new(),
    };
    // Registration: U1, A, devices (or A first).
    let (a, c) = if o.analog_first {
        let a = s.add_engine(Box::new(analog)).unwrap();
        (a, s.add_engine(Box::new(Cpu::new())).unwrap())
    } else {
        let c = s.add_engine(Box::new(Cpu::new())).unwrap();
        (s.add_engine(Box::new(analog)).unwrap(), c)
    };
    let d = s.add_engine(Box::new(devices)).unwrap();
    for (cpu_pin, dev_pin) in OUTS.iter().zip(DEV_INS) {
        let net = format!("U1.{cpu_pin}");
        s.bind(c, cpu_pin, &net, Direction::Output).unwrap();
        s.bind(d, &format!("M1.{dev_pin}"), &net, Direction::Input)
            .unwrap();
    }
    if o.glue {
        s.bind(a, "GLUE", "U1.P0", Direction::Input).unwrap();
    }
    s.bind(d, "M1.HOME_N", "M1.HOME_N", Direction::Output)
        .unwrap();
    s.bind(c, "PH", "M1.HOME_N", Direction::Input).unwrap();
    s.bind(a, "HOME", "M1.HOME_N", Direction::Input).unwrap();
    s.bind(a, "V", "VSENSE", Direction::Output).unwrap();
    s.bind(c, "ADC", "VSENSE", Direction::Input).unwrap();
    let mut sink = EventSink::new(Vec::new());
    let sum = s.run(ms(100), &mut sink, &mut [])?;
    let plans = s.plan_log();
    let quiet: Vec<_> = plans.iter().filter(|p| !p.quiet.is_empty()).collect();
    let text = String::from_utf8(sink.into_inner()).unwrap();
    Ok(Out {
        events: text
            .lines()
            .filter(|l| !l.contains("\"type\":\"grant\""))
            .map(str::to_string)
            .collect(),
        grants: sum.grants,
        quiet: quiet.len(),
        quiet_sets: quiet
            .iter()
            .map(|p| (p.quiet.clone(), p.stop, p.order.clone()))
            .collect(),
    })
}

/// A pure consumer of HOME_N (as ngspice is on `tests/device-smoke`): it
/// logs each change with how it was delivered, before its advance (`pre`) or
/// at a boundary it had already reached (`boundary`). An analog engine
/// treats the two differently (a ramp corner ahead of its time, or a source
/// change at its current time), so the delivery must not depend on the
/// partition.
struct Consumer {
    now: Time,
    logs: Vec<LogRecord>,
}

impl Engine for Consumer {
    fn name(&self) -> &str {
        "C"
    }
    fn next_safe_time(&mut self, _now: Time) -> Time {
        Time::MAX
    }
    fn advance_to(&mut self, g: Time) -> Result<(), EngineError> {
        self.now = g;
        Ok(())
    }
    fn read_outputs(&mut self) -> Result<PinSignals, EngineError> {
        Ok(PinSignals::new())
    }
    fn drain_logs(&mut self) -> Vec<LogRecord> {
        let now = self.now;
        let (ready, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.logs)
            .into_iter()
            .partition(|r| r.t <= now);
        self.logs = later;
        ready
    }
    fn apply_inputs(&mut self, _: &PinSignals) -> Result<(), EngineError> {
        Ok(())
    }
    fn apply_input_trace(
        &mut self,
        _base: &PinSignals,
        trace: &[Transition],
    ) -> Result<(), EngineError> {
        for tr in trace {
            let how = if tr.t > self.now { "pre" } else { "boundary" };
            self.logs.push(LogRecord {
                t: tr.t.max(self.now),
                level: LogLevel::Info,
                msg: format!(
                    "{} {:?} at {} ({how})",
                    tr.pin,
                    tr.signal.level(),
                    tr.t.as_fs()
                ),
            });
        }
        Ok(())
    }
}

/// U1 <-> M1 (M1 sampling every 5 ms exactly on lattice points), and M1's
/// HOME_N into the pure consumer C; registration U1, C, devices (as `bb`
/// registers CPU, analog, devices). Returns the non-grant records and the
/// number of quiet grants.
fn run_consumer(lookahead: bool) -> (Vec<String>, usize) {
    let mut s = Scheduler::new(SchedulerConfig {
        max_quantum: ms(10),
        loop_quantum: None,
        lookahead,
        log_grants: true,
        event_sampling: None,
    })
    .unwrap();
    s.record_plans(true);
    let dev = bbdev::find("dms/mechanism@2")
        .unwrap()
        .instantiate(&[
            // device-smoke's corner: |Bz| crosses BOP/BRP within a step of s0,
            // so HOME_N changes at samples while the rotor moves.
            ("s0".into(), ParamValue::Real(289.0)),
            ("phase0".into(), ParamValue::Real(3.0)),
            ("bop".into(), ParamValue::Real(2.75)),
            ("brp".into(), ParamValue::Real(2.745)),
            ("ts".into(), ParamValue::Time(ms(5))),
            ("phase".into(), ParamValue::Time(ms(2))),
        ])
        .unwrap();
    let devices = DeviceEngine::new(
        DeviceEngineConfig {
            name: "devices".into(),
            seed: 1,
            stop_lattice: Some(L),
        },
        vec![DevicePart {
            reference: "M1".into(),
            device: dev,
        }],
    )
    .unwrap();
    let c = s.add_engine(Box::new(Cpu::new())).unwrap();
    let k = s
        .add_engine(Box::new(Consumer {
            now: Time::ZERO,
            logs: Vec::new(),
        }))
        .unwrap();
    let d = s.add_engine(Box::new(devices)).unwrap();
    for (cpu_pin, dev_pin) in OUTS.iter().zip(DEV_INS) {
        let net = format!("U1.{cpu_pin}");
        s.bind(c, cpu_pin, &net, Direction::Output).unwrap();
        s.bind(d, &format!("M1.{dev_pin}"), &net, Direction::Input)
            .unwrap();
    }
    s.bind(d, "M1.HOME_N", "M1.HOME_N", Direction::Output)
        .unwrap();
    s.bind(c, "PH", "M1.HOME_N", Direction::Input).unwrap();
    s.bind(k, "HOME", "M1.HOME_N", Direction::Input).unwrap();
    let mut sink = EventSink::new(Vec::new());
    s.run(ms(100), &mut sink, &mut []).unwrap();
    let quiet = s.plan_log().iter().filter(|p| !p.quiet.is_empty()).count();
    let text = String::from_utf8(sink.into_inner()).unwrap();
    (
        text.lines()
            .filter(|l| !l.contains("\"type\":\"grant\""))
            .map(str::to_string)
            .collect(),
        quiet,
    )
}

#[test]
fn a_device_change_at_the_grant_end_reaches_a_later_consumer_before_it_advances() {
    let (reference, _) = run_consumer(false);
    // M1's samples (2 ms + 40 us + k 5 ms) are lattice points, and HOME_N
    // does change at some of them: the consumer sees each change before it
    // advances, as the one-cell plan [U1, devices, C] delivers it.
    let seen: Vec<&String> = reference
        .iter()
        .filter(|l| l.contains("\"source\":\"C\""))
        .collect();
    assert!(
        !seen.is_empty(),
        "HOME_N never changed: the test is vacuous"
    );
    assert!(seen.iter().all(|l| l.contains("(pre)")), "{seen:?}");
    let (on, quiet) = run_consumer(true);
    assert!(quiet > 0);
    assert_eq!(on, reference);
}

/// Records sorted, for a run whose registration order differs (records of
/// one instant are ordered by registration index, ADR-003 D3).
fn sorted(v: &[String]) -> Vec<String> {
    let mut v = v.to_vec();
    v.sort();
    v
}

#[test]
fn quiet_grants_with_a_device_and_a_sampled_analog_producer() {
    let reference = run(Opts {
        lookahead: false,
        ..BASE
    })
    .unwrap();
    assert_eq!(
        reference.grants, 10_000,
        "one-cell lockstep: 100 ms / 10 us"
    );
    assert_eq!(reference.quiet, 0);
    // The run did something: PWM edges into M1, HOME_N moved, ADC samples.
    let count = |s: &str| reference.events.iter().filter(|l| l.contains(s)).count();
    assert!(count("\"net\":\"U1.P0\"") > 1000);
    assert!(count("adc 1.") >= 49, "{}", count("adc 1."));
    let mut by_mq = BTreeMap::new();
    for mq in [ms(1), ms(10)] {
        let r = run(Opts {
            max_quantum: mq,
            ..BASE
        })
        .unwrap();
        assert_eq!(r.events, reference.events, "max_quantum {mq}");
        assert!(r.quiet > 0);
        // Q = {M1} (registration index 2), no early stop (|P| = 0), and the
        // remaining graph's order A -> U1 -> M1.
        for (q, stop, order) in &r.quiet_sets {
            assert_eq!(q, &vec![2], "{q:?}");
            assert_eq!(*stop, None);
            assert_eq!(order, &vec![1, 0, 2]);
        }
        by_mq.insert(mq.as_fs(), r.grants);
    }
    // 100 ms in 10 ms grants between M1's 5 ms samples: a few grants per
    // sample instead of 500 cells.
    let g10 = by_mq[&ms(10).as_fs()];
    assert!(g10 <= 200, "{g10} grants at 10 ms");
    // The other registration order: the same records.
    let r = run(Opts {
        analog_first: true,
        ..BASE
    })
    .unwrap();
    assert_eq!(sorted(&r.events), sorted(&reference.events));
    assert!(r.quiet > 0);
}

#[test]
fn a_device_that_reacts_before_its_promise_is_a_broken_promise() {
    // One-cell: no promise is used, the device's reaction is just output.
    assert!(run(Opts {
        lookahead: false,
        neg: Some(Neg::ReactOutput),
        ..BASE
    })
    .is_ok());
    // With lookahead the quiet grant relies on the declaration: the audit
    // (invariant 8) fails the run at the reaction, 1 us after nSLEEP rose.
    match run(Opts {
        neg: Some(Neg::ReactOutput),
        ..BASE
    }) {
        Err(SchedError::PromiseBroken {
            engine,
            promised,
            at,
        }) => {
            assert_eq!(engine, "devices");
            assert_eq!(at, us(1_000) + Time::from_fs(17) + us(1));
            assert_eq!(promised, ms(5) + Time::from_ns(40));
        }
        Err(e) => panic!("expected PromiseBroken, got {e}"),
        Ok(_) => panic!("expected PromiseBroken, the run passed"),
    }
}

#[test]
fn rule_one_and_internal_timers_are_refused_in_any_run() {
    for lookahead in [false, true] {
        let e = run(Opts {
            lookahead,
            neg: Some(Neg::SetInInput),
            ..BASE
        })
        .err()
        .expect("set_pin in on_pin_change must fail");
        assert!(e.to_string().contains("rule 1"), "{e}");
        let e = run(Opts {
            lookahead,
            neg: Some(Neg::InternalSets),
            ..BASE
        })
        .err()
        .expect("set_pin in an internal timer must fail");
        assert!(e.to_string().contains("internal timer"), "{e}");
    }
}

#[test]
fn a_cpu_edge_into_the_analog_engine_falls_back_to_one_cell() {
    // The scope rule (ADR-014 Scope): U1 -> A and A -> U1 is a cycle that
    // no promise of M1 cuts, so every grant is one cell.
    let r = run(Opts { glue: true, ..BASE }).unwrap();
    assert_eq!(r.quiet, 0);
    assert_eq!(r.grants, 10_000);
    let reference = run(Opts {
        glue: true,
        lookahead: false,
        ..BASE
    })
    .unwrap();
    assert_eq!(r.events, reference.events);
}
