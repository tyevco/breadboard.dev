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

fn run_once() -> (String, String, f64) {
    let mut cfg = NgspiceConfig::new("C1 a 0 1u\n", Time::from_ms(10), Time::from_us(10));
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
