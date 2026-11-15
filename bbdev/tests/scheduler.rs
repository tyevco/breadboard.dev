//! ADR-006 Gate G6 (Amendment 6, A6.6): `DeviceEngine` under the real
//! `Scheduler`, with a mock lattice-bound CPU in a feedback loop with the
//! `dms/mechanism@1` device, as E3's homing will run (ADR-008 Amendment 9).
//!
//! The CPU `U1` microsteps the mechanism towards the park stop (8 microsteps
//! per full step, 25 kHz PWM with slow decay, at 446 PPS) until it reads
//! HOME_N low. It applies HOME_N at `ceil_L(edge_time)`, as Renode does,
//! and reacts 1 fs later: brake, 3 ms later sleep. At a fixed time it wakes
//! and steps 80 full steps back towards the click end at 313 PPS, so HOME_N
//! releases. It promises quiet outputs until its next planned edge.
//!
//! The gates: the one-cell reference (`lookahead` off) equals lookahead on
//! at `max_quantum` 1 ms and 10 ms and the other registration order, in every
//! non-grant record; the device's sample phase follows the seed; a quiet
//! grant stops early at the device's HOME_N edge.

use std::collections::VecDeque;

use bbcore::{
    Direction, Engine, EngineError, EventSink, Level, PinSignal, PinSignals, Scheduler,
    SchedulerConfig, Time, Transition,
};
use bbdev::{DeviceEngine, DeviceEngineConfig, DevicePart, ParamValue};

const L: Time = Time::from_us(10);
const OUTS: [&str; 5] = ["P0", "P1", "P2", "P3", "PS"];

fn ceil_l(t: Time) -> Time {
    Time::from_fs(t.as_fs().div_ceil(L.as_fs()) * L.as_fs())
}

fn ms(x: u64) -> Time {
    Time::from_ms(x)
}

/// The firmware's microstep drive as `(t, pin, level)` from `t0` (pins
/// 0-3 = IN_A1, IN_A2, IN_B1, IN_B2), levels starting at `levels`.
fn pwm(
    t0: Time,
    n_full: i64,
    d: f64,
    refresh: u64,
    toward: f64,
    levels: &mut [Level; 5],
) -> (Vec<(Time, usize, Level)>, Time) {
    let period = Time::from_us(40);
    let mut ev = Vec::new();
    let mut put = |ev: &mut Vec<(Time, usize, Level)>, t: Time, pin: usize, l: Level| {
        if levels[pin] != l {
            levels[pin] = l;
            ev.push((t, pin, l));
        }
    };
    let mut t = t0;
    for u in 0..(8 * n_full) {
        let th = toward * (u as f64 + 1.0) * std::f64::consts::FRAC_PI_2 / 8.0;
        let c = [d * th.cos(), d * th.sin()];
        for _ in 0..refresh {
            let mut off = Vec::new();
            for (k, ck) in c.iter().enumerate() {
                let on = Time::from_fs((period.as_fs() as f64 * ck.abs()).round() as u64);
                if on.is_zero() {
                    put(&mut ev, t, 2 * k, Level::High);
                    put(&mut ev, t, 2 * k + 1, Level::High);
                    continue;
                }
                put(&mut ev, t, 2 * k, Level::from_bool(*ck >= 0.0));
                put(&mut ev, t, 2 * k + 1, Level::from_bool(*ck < 0.0));
                if on < period {
                    off.push((on, k));
                }
            }
            off.sort();
            for (on, k) in off {
                put(&mut ev, t + on, 2 * k, Level::High);
                put(&mut ev, t + on, 2 * k + 1, Level::High);
            }
            t = t + period;
        }
    }
    ev.sort_by_key(|e| e.0);
    (ev, t)
}

/// The mock CPU.
struct Cpu {
    now: Time,
    planned: VecDeque<(Time, usize, Level)>,
    levels: [Level; 5],
    edges: [Time; 5],
    trace: Vec<Transition>,
    home_n: Level,
    /// HOME_N changes not yet applied: effect time, level.
    pending: Vec<(Time, Level)>,
    homed: bool,
    wake_at: Time,
}

impl Cpu {
    fn new(wake_at: Time) -> Cpu {
        let mut levels = [Level::Low; 5];
        let mut planned = vec![(Time::from_ns(1_003), 4, Level::High)];
        levels[4] = Level::High;
        let (ev, _) = pwm(ms(1) + Time::from_fs(17), 120, 0.88, 7, -1.0, &mut levels);
        planned.extend(ev);
        Cpu {
            now: Time::ZERO,
            planned: planned.into(),
            levels: [Level::Low; 5],
            edges: [Time::ZERO; 5],
            trace: Vec::new(),
            home_n: Level::High,
            pending: Vec::new(),
            homed: false,
            wake_at,
        }
    }

    fn emit(&mut self, t: Time, pin: usize, l: Level) {
        if self.levels[pin] != l {
            self.levels[pin] = l;
            self.edges[pin] = t;
            self.trace.push(Transition {
                t,
                pin: OUTS[pin].into(),
                signal: PinSignal::digital(l, t),
            });
        }
    }

    /// HOME_N reads `l` from `te` on (a lattice point): on the first low,
    /// stop stepping, brake at `te + 1 fs`, sleep 3 ms later, then wake at
    /// `wake_at` and step 80 full steps towards the click end at 313 PPS.
    fn input(&mut self, te: Time, l: Level) {
        self.home_n = l;
        if l == Level::Low && !self.homed {
            self.homed = true;
            self.planned.retain(|e| e.0 <= te);
            let b = te + Time::from_fs(1);
            let mut ev: Vec<(Time, usize, Level)> = (0..4).map(|p| (b, p, Level::High)).collect();
            let s = b + ms(3);
            ev.extend((0..4).map(|p| (s, p, Level::Low)));
            ev.push((s, 4, Level::Low));
            // Wake, energise, then step towards the click end.
            ev.push((self.wake_at, 4, Level::High));
            let mut lv = [Level::Low; 5];
            lv[4] = Level::High;
            let (back, end) = pwm(self.wake_at + ms(1), 80, 0.88, 10, 1.0, &mut lv);
            ev.extend(back);
            ev.extend((0..4).map(|p| (end, p, Level::High)));
            ev.extend((0..4).map(|p| (end + ms(3), p, Level::Low)));
            ev.push((end + ms(3), 4, Level::Low));
            ev.sort_by_key(|e| e.0);
            self.planned.extend(ev);
        }
    }

    fn next_planned(&self) -> Option<Time> {
        self.planned.front().map(|e| e.0)
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
            let pe = self.pending.first().map(|p| p.0).filter(|t| *t <= g);
            let ne = self.next_planned().filter(|t| *t <= g);
            match (pe, ne) {
                (Some(a), b) if b.is_none_or(|b| a <= b) => {
                    let (te, l) = self.pending.remove(0);
                    self.input(te, l);
                }
                (_, Some(_)) => {
                    let (t, p, l) = self.planned.pop_front().unwrap();
                    self.emit(t, p, l);
                }
                _ => break,
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
    fn apply_inputs(&mut self, inputs: &PinSignals) -> Result<(), EngineError> {
        if let Some(l) = inputs.get("PH").and_then(PinSignal::level) {
            let last = self.pending.last().map_or(self.home_n, |p| p.1);
            if l != last {
                self.pending.push((self.now, l));
                // At a boundary: at once.
                self.advance_pending_now();
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
            let l = tr.signal.level().unwrap();
            if tr.t <= self.now {
                self.pending.push((self.now, l));
                self.advance_pending_now();
            } else {
                self.pending.push((ceil_l(tr.t), l));
            }
        }
        Ok(())
    }
    fn quiet_until(&mut self, now: Time) -> Time {
        match self.next_planned() {
            Some(t) if t > now => t,
            Some(_) => now,
            None => Time::MAX,
        }
    }
}

impl Cpu {
    fn advance_pending_now(&mut self) {
        while let Some(&(te, l)) = self.pending.first() {
            if te > self.now {
                break;
            }
            self.pending.remove(0);
            self.input(te, l);
        }
    }
}

struct Run {
    events: Vec<String>,
    grants: u64,
    quiet: usize,
    stops: usize,
}

fn run(lookahead: bool, max_quantum: Time, device_first: bool, seed: u64) -> Run {
    let mut s = Scheduler::new(SchedulerConfig {
        max_quantum,
        loop_quantum: None,
        lookahead,
        log_grants: true,
        event_sampling: None,
    })
    .unwrap();
    s.record_plans(true);
    let def = bbdev::find("dms/mechanism@1").unwrap();
    let dev = def
        .instantiate(&[("s0".into(), ParamValue::Real(330.0))])
        .unwrap();
    let devices = DeviceEngine::new(
        DeviceEngineConfig {
            name: "devices".into(),
            seed,
            stop_lattice: Some(L),
        },
        vec![DevicePart {
            reference: "M1".into(),
            device: dev,
        }],
    )
    .unwrap();
    let cpu = Cpu::new(ms(400) + Time::from_fs(3));
    let (c, d) = if device_first {
        let d = s.add_engine(Box::new(devices)).unwrap();
        (s.add_engine(Box::new(cpu)).unwrap(), d)
    } else {
        let c = s.add_engine(Box::new(cpu)).unwrap();
        (c, s.add_engine(Box::new(devices)).unwrap())
    };
    for (cpu_pin, dev_pin) in OUTS
        .iter()
        .zip(["IN_A1", "IN_A2", "IN_B1", "IN_B2", "NSLEEP"])
    {
        let net = format!("U1.{cpu_pin}");
        s.bind(c, cpu_pin, &net, Direction::Output).unwrap();
        s.bind(d, &format!("M1.{dev_pin}"), &net, Direction::Input)
            .unwrap();
    }
    s.bind(d, "M1.HOME_N", "M1.HOME_N", Direction::Output)
        .unwrap();
    s.bind(c, "PH", "M1.HOME_N", Direction::Input).unwrap();
    let mut sink = EventSink::new(Vec::new());
    let sum = s.run(ms(800), &mut sink, &mut []).unwrap();
    let plans = s.plan_log();
    let quiet = plans.iter().filter(|p| !p.quiet.is_empty()).count();
    let stops = plans
        .iter()
        .filter(|p| !p.quiet.is_empty() && p.stop.is_some())
        .count();
    let text = String::from_utf8(sink.into_inner()).unwrap();
    Run {
        events: text
            .lines()
            .filter(|l| !l.contains("\"type\":\"grant\""))
            .map(str::to_string)
            .collect(),
        grants: sum.grants,
        quiet,
        stops,
    }
}

fn home_edges(r: &Run) -> Vec<String> {
    r.events
        .iter()
        .filter(|l| l.contains("\"net\":\"M1.HOME_N\""))
        .cloned()
        .collect()
}

#[test]
fn device_loop_is_partition_and_order_independent() {
    let reference = run(false, ms(10), false, 1);
    // One-cell lockstep: 800 ms / 10 us grants.
    assert_eq!(reference.grants, 80_000);
    assert_eq!(reference.quiet, 0);
    let edges = home_edges(&reference);
    // HOME_N falls on the way to the park stop and rises on the way back.
    assert_eq!(edges.len(), 2, "{edges:?}");
    assert!(edges[0].contains("\"level\":0") && edges[1].contains("\"level\":1"));
    for (lookahead, mq, first) in [
        (true, ms(1), false),
        (true, ms(10), false),
        (true, ms(10), true),
        (false, ms(1), true),
    ] {
        let r = run(lookahead, mq, first, 1);
        assert_eq!(
            r.events.len(),
            reference.events.len(),
            "lookahead {lookahead} max_quantum {mq} device first {first}"
        );
        // The device's records (dms/mechanism@1's field and stop records)
        // are made at the CPU's edge instants; the kernel orders records of
        // one instant by registration order (ADR-003 D3), so with the device
        // registered first they come before the CPU's pin edges at that
        // instant. The records themselves, and every time, are the same.
        let (want, got) = if first {
            let mut a = reference.events.clone();
            let mut b = r.events.clone();
            a.sort();
            b.sort();
            (a, b)
        } else {
            (reference.events.clone(), r.events.clone())
        };
        for (a, b) in want.iter().zip(&got) {
            assert_eq!(
                a, b,
                "lookahead {lookahead} max_quantum {mq} device first {first}"
            );
        }
        if lookahead {
            // Quiet grants while the CPU sleeps or holds; the device stops
            // early at its HOME_N edges.
            // While the PWM runs every edge ends a promise (about one cell
            // per grant, ADR-008 Amendment 9); asleep or holding, the grants
            // are long. About half of the 800 ms run steps.
            assert!(
                r.quiet > 0 && r.grants < reference.grants * 3 / 5,
                "{}",
                r.grants
            );
            assert!(r.stops > 0);
        }
    }
}

#[test]
fn the_seed_moves_the_sample_phase_only() {
    let a = run(true, ms(10), false, 1);
    let b = run(true, ms(10), false, 1);
    assert_eq!(a.events, b.events);
    let c = run(true, ms(10), false, 2);
    let (ea, ec) = (home_edges(&a), home_edges(&c));
    assert_eq!(ec.len(), 2);
    assert_ne!(
        ea, ec,
        "a different seed gives another DRV5032 sample phase"
    );
    // The CPU's own edges before HOME_N first falls are the same (the
    // device's records state the seeded phase, so they differ).
    let edges_only = |r: &Run| -> Vec<String> {
        r.events
            .iter()
            .filter(|l| !l.contains("\"type\":\"log\""))
            .cloned()
            .collect()
    };
    let (pa, pc) = (edges_only(&a), edges_only(&c));
    let first_fall = |e: &[String]| {
        e.iter()
            .position(|l| l.contains("\"net\":\"M1.HOME_N\""))
            .unwrap()
    };
    let k = first_fall(&pa).min(first_fall(&pc));
    assert_eq!(pa[..k], pc[..k]);
}
