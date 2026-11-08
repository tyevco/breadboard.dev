//! ADR-003 D5 (plan step 7): ngspice `advance_until`. With a grant lattice
//! and thresholded digital outputs the engine can stop early: at the first
//! level change of a watched pin, at `t_x`, it keeps stepping to the lattice
//! point `ceil_L(t_x)` and parks there. The lattice clamp (D2) puts a solver
//! point on every lattice point, so that is a real park point and the solver
//! makes exactly the steps of a one-cell run.
//!
//! ngspice is a process-global singleton: tests in this binary take turns.

use std::sync::Mutex;

use bb_analog_ngspice::{
    DigitalOutputBinding, Drive, InputBinding, NgspiceConfig, NgspiceEngine, OutputBinding,
};
use bbcore::{
    CsvWriter, DigitalIn, DigitalOut, Direction, Engine, EngineError, EventSink, Level, PinSignal,
    PinSignals, Probe, Scheduler, SchedulerConfig, Time, Transition,
};

static NGSPICE: Mutex<()> = Mutex::new(());

const L: Time = Time::from_us(10);

/// The `examples/feedback` input model: STM32-like thresholds, 5 pF.
const PB0: DigitalIn = DigitalIn {
    vih: 2.0,
    vil: 0.8,
    cin: 5e-12,
};

fn ceil_l(t: Time) -> Time {
    Time::from_fs(t.as_fs().div_ceil(L.as_fs()) * L.as_fs())
}

/// The feedback example's RC (as in `threshold.rs`): a 25 ohm STM32
/// output, 1 kOhm, 1 uF, plus the 5 pF `cin` glue.
fn rc_config(t_end: Time) -> NgspiceConfig {
    let mut cfg = NgspiceConfig::new(
        "R2 drv sense 1k\nC1 sense 0 1u\n.options RELTOL=1e-6 VNTOL=1e-9 ABSTOL=1e-14\n",
        t_end,
        L,
    );
    cfg.lattice = Some(L);
    cfg.inputs.push(InputBinding {
        pin: "U1.PA5".into(),
        source: "V_bb_U1_PA5".into(),
        drive: Drive::Digital {
            model: DigitalOut::STM32_GPIO_3V3,
            initial: Level::Low,
        },
        glue_node: Some("drv".into()),
    });
    cfg.outputs.push(OutputBinding {
        pin: "SENSE".into(),
        node: "sense".into(),
    });
    cfg.digital_outputs.push(DigitalOutputBinding {
        pin: "U1.PB0".into(),
        node: "sense".into(),
        model: PB0,
    });
    cfg
}

/// Everything a run produces, bit for bit: raw solver points of `node`
/// (time and value bits), the PB0 trace, and the early stops `(G, r)` with
/// `r < G`.
#[derive(Debug, PartialEq)]
struct Dump {
    points: Vec<(u64, u64)>,
    trace: Vec<Transition>,
    stops: Vec<(Time, Time)>,
    grants: usize,
}

/// How the run is partitioned.
#[derive(Copy, Clone)]
enum Mode {
    /// `advance_to` in grants of this length (the one-cell reference: `L`).
    Fixed(Time),
    /// `advance_until(min(now + quantum, next boundary input), watch)`.
    Until(Time),
}

/// Run `cfg` to its end. Digital `edges` on `pin` are delivered before the
/// advance through the grant holding them (their true time, as the kernel
/// does); analog `steps` on `pin` are applied at a boundary, so every
/// partition ends a grant there.
fn run(
    cfg: NgspiceConfig,
    mode: Mode,
    node: &str,
    pin: &str,
    edges: &[(Time, Level)],
    steps: &[(Time, f64)],
    watch: &[&str],
) -> Dump {
    let t_end = cfg.t_stop;
    let mut e = NgspiceEngine::new(cfg).expect("engine");
    assert!(e.can_stop_early());
    let watch: Vec<String> = watch.iter().map(|s| s.to_string()).collect();
    let mut d = Dump {
        points: Vec::new(),
        trace: Vec::new(),
        stops: Vec::new(),
        grants: 0,
    };
    let mut now = Time::ZERO;
    while now < t_end {
        for &(t, v) in steps.iter().filter(|(t, _)| *t == now) {
            let _ = t;
            let mut ins = PinSignals::new();
            ins.insert(pin.into(), PinSignal::analog(v));
            e.apply_inputs(&ins).unwrap();
        }
        let next_step = steps
            .iter()
            .map(|(t, _)| *t)
            .filter(|&t| t > now)
            .min()
            .unwrap_or(t_end);
        let quantum = match mode {
            Mode::Fixed(q) | Mode::Until(q) => q,
        };
        let g = (now + quantum).min(t_end).min(next_step);
        for &(t, lv) in edges {
            if now < t && t <= g {
                let mut ins = PinSignals::new();
                ins.insert(pin.into(), PinSignal::digital(lv, t));
                e.apply_inputs(&ins).unwrap();
            }
        }
        let r = match mode {
            Mode::Fixed(_) => {
                e.advance_to(g).unwrap();
                g
            }
            Mode::Until(_) => {
                let r = e.advance_until(g, &watch).unwrap();
                assert!(now < r && r <= g, "r = {r} outside ({now}, {g}]");
                assert!(
                    r == g || r.as_fs().is_multiple_of(L.as_fs()),
                    "r = {r} off the lattice"
                );
                if r < g {
                    d.stops.push((g, r));
                }
                r
            }
        };
        assert_eq!(e.now(), r);
        let w = e.solver_window_raw(node).unwrap();
        d.points
            .extend(w[1..].iter().map(|&(t, v)| (t.to_bits(), v.to_bits())));
        let trace = e.read_output_trace().unwrap().unwrap();
        for tr in &trace {
            assert!(now < tr.t && tr.t <= r, "{tr:?} outside ({now}, {r}]");
        }
        if let (Mode::Until(_), Some(first), false) = (mode, trace.first(), watch.is_empty()) {
            // The stop is the first lattice point at or after the first
            // watched transition.
            assert_eq!(r, ceil_l(first.t).min(g), "{trace:?}");
        }
        d.trace.extend(trace);
        d.grants += 1;
        now = r;
    }
    d
}

/// Gates: the RC crossing parks at exactly the analytic `ceil_L(t_x)`; the
/// solver points (and the trace) are bit-identical to the one-cell run; two
/// runs are bit-identical.
#[test]
fn rc_crossing_parks_at_the_analytic_lattice_point() {
    let _g = NGSPICE.lock().unwrap_or_else(|e| e.into_inner());
    let fs1 = Time::from_fs(1);
    let edges = [
        (Time::from_ms(1) + fs1, Level::High),
        (Time::from_ms(8) + fs1, Level::Low),
    ];
    let t_end = Time::from_ms(12);
    let one_cell = run(
        rc_config(t_end),
        Mode::Fixed(L),
        "sense",
        "U1.PA5",
        &edges,
        &[],
        &[],
    );
    let until = |q: Time| {
        run(
            rc_config(t_end),
            Mode::Until(q),
            "sense",
            "U1.PA5",
            &edges,
            &[],
            &["U1.PB0"],
        )
    };
    let a = until(Time::from_ms(10));
    let b = until(Time::from_ms(10));

    // The analytic crossings (as in threshold.rs): tau = 1025 ohm * (1 uF +
    // 5 pF), the 5 ns drive ramp delays the exponential by half its length.
    let tau = 1025.0 * (1e-6 + 5e-12);
    let ramp = 2.5e-9;
    let v_fall = 3.3 * (1.0 - (-(7e-3f64) / tau).exp());
    let want = [
        1e-3 + ramp + tau * (3.3f64 / 1.3).ln(),
        8e-3 + ramp + tau * (v_fall / 0.8).ln(),
    ];
    let cells: Vec<Time> = want
        .iter()
        .map(|w| ceil_l(Time::from_fs((w * 1e15).ceil() as u64)))
        .collect();
    println!(
        "analytic crossings {:.9} ms, {:.9} ms -> cells end {} and {}; stops {:?}; \
         {} grants vs {} one-cell; {} solver points",
        want[0] * 1e3,
        want[1] * 1e3,
        cells[0],
        cells[1],
        a.stops,
        a.grants,
        one_cell.grants,
        a.points.len()
    );
    assert_eq!(
        a.stops.iter().map(|s| s.1).collect::<Vec<_>>(),
        cells,
        "parks at ceil_L of the analytic crossings"
    );
    assert_eq!(a.trace.len(), 2);
    for (tr, c) in a.trace.iter().zip(&cells) {
        assert_eq!(ceil_l(tr.t), *c);
    }
    // 1.96 ms (stop), 9.46 ms (stop, the fall edge delivered before it),
    // then t_end.
    assert_eq!(a.grants, 3);
    assert_eq!(one_cell.grants, 1200);
    assert!(
        a.points == one_cell.points,
        "solver points differ from the one-cell run"
    );
    assert_eq!(a.trace, one_cell.trace);
    assert_eq!(a, b, "two runs differ");

    // Without the watch nothing stops: the same points in 10 ms grants.
    let unwatched = run(
        rc_config(t_end),
        Mode::Until(Time::from_ms(10)),
        "sense",
        "U1.PA5",
        &edges,
        &[],
        &[],
    );
    assert!(unwatched.stops.is_empty());
    assert_eq!(unwatched.points, one_cell.points);
}

/// Gate: a slow ramp with ripple through the 0.8-2.0 V band gives exactly
/// one transition per direction, and so exactly one stop per direction;
/// the solver points equal the one-cell run's.
#[test]
fn hysteresis_gives_one_stop_per_direction() {
    let _g = NGSPICE.lock().unwrap_or_else(|e| e.into_inner());
    let t_end = Time::from_ms(10);
    let cfg = || {
        // v(n) = ramp + 0.3 V * sin(2 pi 10 kHz t).
        let mut cfg = NgspiceConfig::new("V_RIP n r SIN(0 0.3 10k)\nR_LOAD n 0 1meg\n", t_end, L);
        cfg.lattice = Some(L);
        cfg.inputs.push(InputBinding {
            pin: "RAMP".into(),
            source: "V_RAMP".into(),
            drive: Drive::Analog {
                initial: 0.0,
                slew: Time::from_ms(2),
            },
            glue_node: Some("r".into()),
        });
        cfg.probes.push("n".into());
        cfg.digital_outputs.push(DigitalOutputBinding {
            pin: "U1.PB0".into(),
            node: "n".into(),
            model: DigitalIn { cin: 0.0, ..PB0 },
        });
        cfg
    };
    let steps = [(Time::from_ms(1), 3.3), (Time::from_ms(5), 0.0)];
    let one_cell = run(cfg(), Mode::Fixed(L), "n", "RAMP", &[], &steps, &[]);
    let until = run(
        cfg(),
        Mode::Until(Time::from_ms(10)),
        "n",
        "RAMP",
        &[],
        &steps,
        &["U1.PB0"],
    );
    // A single 2.0 V threshold would cross several times.
    let crossings = until
        .points
        .windows(2)
        .filter(|w| (f64::from_bits(w[0].1) < 2.0) != (f64::from_bits(w[1].1) < 2.0))
        .count();
    println!(
        "{crossings} single-threshold crossings at 2.0 V; stops {:?}; trace {:?}",
        until.stops, until.trace
    );
    assert!(crossings > 2, "the ripple must recross vih");
    let levels: Vec<Level> = until
        .trace
        .iter()
        .map(|t| t.signal.level().unwrap())
        .collect();
    assert_eq!(levels, vec![Level::High, Level::Low]);
    assert_eq!(until.stops.len(), 2, "one stop per direction");
    for ((_, r), tr) in until.stops.iter().zip(&until.trace) {
        assert_eq!(*r, ceil_l(tr.t));
    }
    assert!(until.points == one_cell.points, "solver points differ");
    assert_eq!(until.trace, one_cell.trace);
}

/// `can_stop_early` needs a lattice and a digital output; a watched pin
/// must be a thresholded digital output.
#[test]
fn advance_until_contract() {
    let _g = NGSPICE.lock().unwrap_or_else(|e| e.into_inner());
    let mut cfg = rc_config(Time::from_ms(1));
    cfg.lattice = None;
    let e = NgspiceEngine::new(cfg).unwrap();
    assert!(!e.can_stop_early());
    drop(e);
    let mut cfg = rc_config(Time::from_ms(1));
    cfg.digital_outputs.clear();
    let e = NgspiceEngine::new(cfg).unwrap();
    assert!(!e.can_stop_early());
    drop(e);
    let mut e = NgspiceEngine::new(rc_config(Time::from_ms(1))).unwrap();
    let err = e
        .advance_until(Time::from_us(100), &["SENSE".to_string()])
        .unwrap_err();
    assert!(err.to_string().contains("SENSE"), "{err}");
    // Nothing happened: the engine still advances normally.
    assert_eq!(
        e.advance_until(Time::from_us(100), &["U1.PB0".to_string()])
            .unwrap(),
        Time::from_us(100)
    );
}

/// A lattice-bound mock of the `examples/feedback` firmware: PA5 toggles at
/// each `wakes` time + 1 fs (a timer ISR in the slice after a lattice sync),
/// and PA6 copies PB0 at `ceil_L(t) + 1 fs` (an EXTI ISR). It promises quiet
/// outputs until its next output while nothing is pending.
struct FeedbackCpu {
    now: Time,
    /// Pending output changes: (time, pin, level), sorted by time.
    pending: Vec<(Time, &'static str, Level)>,
    levels: [(&'static str, Level, Time); 2],
    trace: Vec<Transition>,
    pb0: Level,
}

impl FeedbackCpu {
    fn new(wakes: &[Time]) -> FeedbackCpu {
        let mut level = Level::Low;
        let pending = wakes
            .iter()
            .map(|&w| {
                level = level.toggled();
                (w + Time::from_fs(1), "PA5", level)
            })
            .collect();
        FeedbackCpu {
            now: Time::ZERO,
            pending,
            levels: [
                ("PA5", Level::Low, Time::ZERO),
                ("PA6", Level::Low, Time::ZERO),
            ],
            trace: Vec::new(),
            pb0: Level::Low,
        }
    }
}

impl Engine for FeedbackCpu {
    fn name(&self) -> &str {
        "U1"
    }
    fn next_safe_time(&mut self, _now: Time) -> Time {
        Time::MAX
    }
    fn grant_lattice(&self) -> Option<Time> {
        Some(L)
    }
    fn quiet_until(&mut self, now: Time) -> Time {
        self.pending.first().map_or(Time::MAX, |p| p.0).max(now)
    }
    fn advance_to(&mut self, t: Time) -> Result<(), EngineError> {
        self.trace.clear();
        while self.pending.first().is_some_and(|p| p.0 <= t) {
            let (at, pin, level) = self.pending.remove(0);
            let slot = self.levels.iter_mut().find(|s| s.0 == pin).unwrap();
            if slot.1 != level {
                *slot = (pin, level, at);
                self.trace.push(Transition {
                    t: at,
                    pin: pin.into(),
                    signal: PinSignal::digital(level, at),
                });
            }
        }
        self.now = t;
        Ok(())
    }
    fn read_outputs(&mut self) -> Result<PinSignals, EngineError> {
        Ok(self
            .levels
            .iter()
            .map(|&(p, l, e)| (p.to_string(), PinSignal::digital(l, e)))
            .collect())
    }
    fn read_output_trace(&mut self) -> Result<Option<Vec<Transition>>, EngineError> {
        Ok(Some(self.trace.clone()))
    }
    fn apply_inputs(&mut self, inputs: &PinSignals) -> Result<(), EngineError> {
        if let Some(PinSignal::Digital { level, edge_time }) = inputs.get("PB0") {
            if *level != self.pb0 {
                self.pb0 = *level;
                let at = ceil_l(*edge_time).max(self.now) + Time::from_fs(1);
                let k = self.pending.partition_point(|p| p.0 <= at);
                self.pending.insert(k, (at, "PA6", *level));
            }
        }
        Ok(())
    }
}

/// Steps 6 and 7 together, through the kernel: the feedback loop (a mock
/// CPU + the real ngspice RC) with lookahead on gives the one-cell run's
/// records and sampled SENSE byte for byte. ngspice stops at each PB0
/// crossing (`stop:ngspice`), and the CPU's promises lift the loop cap.
#[test]
fn kernel_quiet_grants_with_ngspice_match_one_cell() {
    let _g = NGSPICE.lock().unwrap_or_else(|e| e.into_inner());
    let t_end = Time::from_ms(20);
    let wakes = [Time::from_ms(1), Time::from_ms(8), Time::from_ms(15)];
    let run = |lookahead: bool| {
        let mut s = Scheduler::new(SchedulerConfig {
            max_quantum: Time::from_ms(10),
            loop_quantum: None,
            lookahead,
            log_grants: true,
            event_sampling: None,
        })
        .unwrap();
        let u = s.add_engine(Box::new(FeedbackCpu::new(&wakes))).unwrap();
        let n = s
            .add_engine(Box::new(NgspiceEngine::new(rc_config(t_end)).unwrap()))
            .unwrap();
        s.bind(u, "PA5", "U1.PA5", Direction::Output).unwrap();
        s.bind(u, "PA6", "U1.PA6", Direction::Output).unwrap();
        s.bind(n, "U1.PA5", "U1.PA5", Direction::Input).unwrap();
        s.bind(n, "SENSE", "SENSE", Direction::Output).unwrap();
        s.bind(n, "U1.PB0", "U1.PB0", Direction::Output).unwrap();
        s.bind(u, "PB0", "U1.PB0", Direction::Input).unwrap();
        let mut csv =
            CsvWriter::sampled(Vec::new(), Time::from_us(10), vec!["SENSE".into()]).unwrap();
        let mut sink = EventSink::new(Vec::new());
        {
            let mut probes: [&mut dyn Probe; 1] = [&mut csv];
            s.run(t_end, &mut sink, &mut probes).unwrap();
        }
        let events = String::from_utf8(sink.into_inner()).unwrap();
        (events, csv.into_inner())
    };
    let (e_off, c_off) = run(false);
    let (e_on, c_on) = run(true);
    let split = |e: &str| -> (Vec<String>, Vec<String>) {
        e.lines()
            .map(String::from)
            .partition(|l| l.contains("\"type\":\"grant\""))
    };
    let (g_off, r_off) = split(&e_off);
    let (g_on, r_on) = split(&e_on);
    let stops = g_on.iter().filter(|l| l.contains("stop:ngspice")).count();
    println!(
        "grants: {} one-cell, {} with lookahead ({stops} stop:ngspice); {} non-grant records",
        g_off.len(),
        g_on.len(),
        r_on.len()
    );
    for l in r_on.iter().filter(|l| l.contains("pin_edge")) {
        println!("  {l}");
    }
    assert_eq!(g_off.len(), 2000);
    assert!(g_on.len() < 40, "{g_on:#?}");
    // PB0 rises and falls once per PA5 edge before t_end (three PA5 edges,
    // the last one's crossing at 15.95 ms), and PA6 follows each.
    assert_eq!(stops, 3, "{g_on:#?}");
    assert_eq!(r_on, r_off, "lookahead changed a non-grant record");
    assert!(c_on == c_off, "lookahead changed the sampled SENSE");
    assert_eq!(r_on.iter().filter(|l| l.contains("pin_edge")).count(), 9);
}
