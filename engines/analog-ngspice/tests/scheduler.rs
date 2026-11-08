//! The ngspice engine under the real `bbcore::Scheduler`: a mock digital
//! driver toggles a net every 1 ms, the RC follows, a `CsvWriter` samples at a
//! fixed sim-time rate. Two runs must produce byte-identical CSV and events.

use bb_analog_ngspice::{Drive, InputBinding, NgspiceConfig, NgspiceEngine, OutputBinding};
use bbcore::{
    CsvWriter, DigitalOut, Direction, Engine, EngineError, EventSink, Level, PinSignal, PinSignals,
    Probe, Scheduler, SchedulerConfig, Time,
};

/// Toggles pin `OUT` at every multiple of `period`; promises `t_safe` = next
/// toggle, so each edge lands exactly on a grant boundary.
struct Toggler {
    period: Time,
    now: Time,
    level: Level,
    edge: Time,
}

impl Engine for Toggler {
    fn name(&self) -> &str {
        "toggler"
    }
    fn next_safe_time(&mut self, now: Time) -> Time {
        Time::from_fs((now.as_fs() / self.period.as_fs() + 1) * self.period.as_fs())
    }
    fn advance_to(&mut self, t: Time) -> Result<(), EngineError> {
        if t.as_fs().is_multiple_of(self.period.as_fs()) {
            self.level = self.level.toggled();
            self.edge = t;
        }
        self.now = t;
        Ok(())
    }
    fn read_outputs(&mut self) -> Result<PinSignals, EngineError> {
        let mut m = PinSignals::new();
        m.insert("OUT".into(), PinSignal::digital(self.level, self.edge));
        Ok(m)
    }
    fn apply_inputs(&mut self, _: &PinSignals) -> Result<(), EngineError> {
        Ok(())
    }
}

/// `BB_NGSPICE_HANDOFF_SPIN_US=<n>` runs these with the ADR-003 D1.4 handoff
/// spin on; default off.
fn spin_from_env() -> std::time::Duration {
    std::env::var("BB_NGSPICE_HANDOFF_SPIN_US")
        .ok()
        .and_then(|v| v.parse().ok())
        .map(std::time::Duration::from_micros)
        .unwrap_or_default()
}

/// ngspice is a process singleton: tests in this binary take turns.
static NGSPICE: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn run_once() -> (String, String, f64) {
    let mut cfg = NgspiceConfig::new("C1 a 0 1u\n", Time::from_ms(10), Time::from_us(10));
    cfg.handoff_spin = spin_from_env();
    cfg.inputs.push(InputBinding {
        pin: "DRV".into(),
        source: "V_DRV".into(),
        drive: Drive::Digital {
            model: DigitalOut {
                voh: 3.3,
                vol: 0.0,
                rise: Time::from_ns(5),
                fall: Time::from_ns(5),
                zout: 1e3, // the RC's 1 kOhm
            },
            initial: Level::Low,
        },
        glue_node: Some("a".into()),
    });
    cfg.outputs.push(OutputBinding {
        pin: "A".into(),
        node: "a".into(),
    });
    let mut s = Scheduler::new(SchedulerConfig::default()).unwrap();
    let t = s
        .add_engine(Box::new(Toggler {
            period: Time::from_ms(1),
            now: Time::ZERO,
            level: Level::Low,
            edge: Time::ZERO,
        }))
        .unwrap();
    let n = s
        .add_engine(Box::new(NgspiceEngine::new(cfg).unwrap()))
        .unwrap();
    s.bind(t, "OUT", "DRV", Direction::Output).unwrap();
    s.bind(n, "DRV", "DRV", Direction::Input).unwrap();
    s.bind(n, "A", "A", Direction::Output).unwrap();

    let mut events = Vec::new();
    let mut sink = EventSink::new(&mut events);
    let mut csv = CsvWriter::new(
        Vec::new(),
        Time::from_us(100),
        vec!["DRV".into(), "A".into()],
    )
    .unwrap();
    {
        let mut probes: [&mut dyn Probe; 1] = [&mut csv];
        s.run(Time::from_ms(4), &mut sink, &mut probes).unwrap();
        probes[0].finish().unwrap();
    }
    let a_end = s.net("A").and_then(PinSignal::volts).unwrap();
    drop(s); // releases the ngspice singleton
    (
        String::from_utf8(csv.into_inner()).unwrap(),
        String::from_utf8(events).unwrap(),
        a_end,
    )
}

#[test]
fn scheduler_run_is_byte_identical_and_physical() {
    let _g = NGSPICE.lock().unwrap_or_else(|e| e.into_inner());
    let (csv1, ev1, a1) = run_once();
    let (csv2, ev2, _) = run_once();
    assert_eq!(csv1, csv2, "CSV differs between runs");
    assert_eq!(ev1, ev2, "events differ between runs");
    // High during (1 ms, 2 ms], low during (2, 3], high during (3, 4]: the
    // edge at 1 ms charges for 1 ms: 3.3 * (1 - e^-1) = 2.086 V at 2 ms.
    let row = |t_us: u64| -> f64 {
        let key = format!("{},", Time::from_us(t_us).as_fs());
        let line = csv1
            .lines()
            .find(|l| l.starts_with(&key))
            .unwrap_or_else(|| panic!("no CSV row at {t_us} us"));
        line.rsplit(',').next().unwrap().parse().unwrap()
    };
    let want_2ms = 3.3 * (1.0 - (-1.0f64).exp());
    let got_2ms = row(2000);
    assert!(
        ((got_2ms - want_2ms) / want_2ms).abs() < 0.01,
        "v(a) at 2 ms = {got_2ms}, want {want_2ms}"
    );
    assert!(row(1000).abs() < 1e-3, "still low before the first edge");
    assert!(a1 > 0.0);
    println!(
        "csv rows: {}, event lines: {}",
        csv1.lines().count(),
        ev1.lines().count()
    );
}

/// Raises `OUT` once at `at`, which lies strictly inside a grant (t_safe is
/// always now + 1 ms, edges are not on grant boundaries).
struct OneEdge {
    at: Time,
    now: Time,
    level: Level,
    edge: Time,
}

impl Engine for OneEdge {
    fn name(&self) -> &str {
        "one_edge"
    }
    fn next_safe_time(&mut self, now: Time) -> Time {
        now + Time::from_ms(1)
    }
    fn advance_to(&mut self, t: Time) -> Result<(), EngineError> {
        if self.now < self.at && self.at <= t {
            self.level = Level::High;
            self.edge = self.at;
        }
        self.now = t;
        Ok(())
    }
    fn read_outputs(&mut self) -> Result<PinSignals, EngineError> {
        let mut m = PinSignals::new();
        m.insert("OUT".into(), PinSignal::digital(self.level, self.edge));
        Ok(m)
    }
    fn apply_inputs(&mut self, _: &PinSignals) -> Result<(), EngineError> {
        Ok(())
    }
}

/// v(a) at 1 ms and 2 ms for an edge at 0.3 ms, with the driver registered
/// before (`cpu_first`) or after the ngspice engine.
fn mid_grant_run(cpu_first: bool) -> (f64, f64) {
    let mut cfg = NgspiceConfig::new("C1 a 0 1u\n", Time::from_ms(10), Time::from_us(10));
    cfg.handoff_spin = spin_from_env();
    cfg.inputs.push(InputBinding {
        pin: "DRV".into(),
        source: "V_DRV".into(),
        drive: Drive::Digital {
            model: DigitalOut {
                voh: 3.3,
                vol: 0.0,
                rise: Time::from_ns(5),
                fall: Time::from_ns(5),
                zout: 1e3,
            },
            initial: Level::Low,
        },
        glue_node: Some("a".into()),
    });
    cfg.outputs.push(OutputBinding {
        pin: "A".into(),
        node: "a".into(),
    });
    let mut s = Scheduler::new(SchedulerConfig {
        log_grants: false,
        ..SchedulerConfig::default()
    })
    .unwrap();
    let drv = OneEdge {
        at: Time::from_us(300),
        now: Time::ZERO,
        level: Level::Low,
        edge: Time::ZERO,
    };
    let ng = NgspiceEngine::new(cfg).unwrap();
    let (t, n) = if cpu_first {
        let t = s.add_engine(Box::new(drv)).unwrap();
        (t, s.add_engine(Box::new(ng)).unwrap())
    } else {
        let n = s.add_engine(Box::new(ng)).unwrap();
        (s.add_engine(Box::new(drv)).unwrap(), n)
    };
    s.bind(t, "OUT", "DRV", Direction::Output).unwrap();
    s.bind(n, "DRV", "DRV", Direction::Input).unwrap();
    s.bind(n, "A", "A", Direction::Output).unwrap();
    let mut sink = EventSink::new(Vec::new());
    s.run(Time::from_ms(1), &mut sink, &mut []).unwrap();
    let v1 = s.net("A").and_then(PinSignal::volts).unwrap();
    s.run(Time::from_ms(2), &mut sink, &mut []).unwrap();
    let v2 = s.net("A").and_then(PinSignal::volts).unwrap();
    (v1, v2)
}

/// ADR-002 Amendment 2: an edge inside a grant, from an engine registered
/// before ngspice, is applied at its true edge_time (Gauss-Seidel). From an
/// engine registered after ngspice it is applied at the next boundary
/// (Jacobi, D4.4).
#[test]
fn mid_grant_edge_is_applied_at_its_true_time() {
    let _g = NGSPICE.lock().unwrap_or_else(|e| e.into_inner());
    let rc = |dt_ms: f64| 3.3 * (1.0 - (-dt_ms).exp()); // RC = 1 ms
    let (v1, v2) = mid_grant_run(true);
    assert!(
        ((v1 - rc(0.7)) / rc(0.7)).abs() < 0.01,
        "v(1ms) = {v1}, want {}",
        rc(0.7)
    );
    assert!(
        ((v2 - rc(1.7)) / rc(1.7)).abs() < 0.01,
        "v(2ms) = {v2}, want {}",
        rc(1.7)
    );
    let (j1, j2) = mid_grant_run(false);
    assert!(j1.abs() < 1e-3, "Jacobi: v(1ms) = {j1}, want 0");
    assert!(
        ((j2 - rc(1.0)) / rc(1.0)).abs() < 0.01,
        "Jacobi: v(2ms) = {j2}, want {}",
        rc(1.0)
    );
    println!("gauss-seidel v(1ms)={v1:.6} v(2ms)={v2:.6}; jacobi v(1ms)={j1:.6} v(2ms)={j2:.6}");
}
