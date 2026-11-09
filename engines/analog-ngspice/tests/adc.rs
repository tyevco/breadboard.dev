//! ADR-003 plan step 9 (D6): the `adc_in` push model in ngspice. At every
//! lattice point the engine reads the ADC node at the solver point the
//! lattice clamp put there and emits an `Analog` transition at that lattice
//! point when the delivered whole millivolts change (the default), or at
//! every lattice point with the value in whole microvolts (the reference).
//! The pushes are a function of the solver points alone, so they do not
//! depend on the partition, and `advance_until` watching the ADC pin stops
//! exactly at the first push.
//!
//! ngspice is a process-global singleton: tests in this binary take turns.

use std::sync::Mutex;

use bb_analog_ngspice::{
    AdcOutputBinding, AdcPush, Drive, InputBinding, NgspiceConfig, NgspiceEngine, OutputBinding,
};
use bbcore::{AdcIn, AdcKey, DigitalOut, Engine, Level, PinSignal, PinSignals, Time, Transition};

static NGSPICE: Mutex<()> = Mutex::new(());

const L: Time = Time::from_us(10);
const ADC: &str = "U1.PA1";

/// The `examples/adc` divider: a 25 ohm STM32 output, 10k into ADC_IN,
/// 33k and 470 nF to ground (tau = 3.614 ms, 2.531087 V high).
/// The stock model's key (whole millivolts, ADR-003 Amendment 10).
const STOCK: AdcIn = AdcIn {
    key: AdcKey::Millivolts,
    ..AdcIn::STM32_12BIT_3V3
};

fn divider(t_end: Time, push: AdcPush) -> NgspiceConfig {
    let mut cfg = NgspiceConfig::new(
        "R1 drv adc_in 10k\nR2 adc_in 0 33k\nC1 adc_in 0 470n\n\
         .options RELTOL=1e-5 VNTOL=1e-8 ABSTOL=1e-13\n",
        t_end,
        L,
    );
    cfg.lattice = Some(L);
    cfg.inputs.push(InputBinding {
        pin: "U1.PA4".into(),
        source: "V_bb_U1_PA4".into(),
        drive: Drive::Digital {
            model: DigitalOut::STM32_GPIO_3V3,
            initial: Level::Low,
        },
        glue_node: Some("drv".into()),
    });
    cfg.outputs.push(OutputBinding {
        pin: "ADC_IN".into(),
        node: "adc_in".into(),
    });
    cfg.adc_outputs.push(AdcOutputBinding {
        pin: ADC.into(),
        node: "adc_in".into(),
        model: STOCK,
    });
    cfg.adc_push = push;
    cfg
}

/// PA4 rises at 1 ms + 1 fs and falls at 31 ms + 1 fs (Renode's lattice
/// +1 fs stamps), delivered before the grant that holds each edge.
fn edges() -> Vec<(Time, Level)> {
    let fs1 = Time::from_fs(1);
    vec![
        (Time::from_ms(1) + fs1, Level::High),
        (Time::from_ms(31) + fs1, Level::Low),
    ]
}

#[derive(Debug, PartialEq)]
struct Dump {
    /// Raw solver points of adc_in (time and value bits).
    points: Vec<(u64, u64)>,
    trace: Vec<Transition>,
    /// The value at each boundary.
    boundary: Vec<(Time, PinSignal)>,
    stops: Vec<Time>,
    grants: usize,
}

#[derive(Copy, Clone)]
enum Mode {
    Fixed(Time),
    Until(Time),
}

fn run(cfg: NgspiceConfig, mode: Mode) -> Dump {
    let t_end = cfg.t_stop;
    let mut e = NgspiceEngine::new(cfg).expect("engine");
    assert!(
        e.can_stop_early(),
        "an ADC output alone allows an early stop"
    );
    let mut d = Dump {
        points: Vec::new(),
        trace: Vec::new(),
        boundary: Vec::new(),
        stops: Vec::new(),
        grants: 0,
    };
    let mut now = Time::ZERO;
    let watch = vec![ADC.to_string()];
    while now < t_end {
        let q = match mode {
            Mode::Fixed(q) | Mode::Until(q) => q,
        };
        let g = (now + q).min(t_end);
        for (t, lv) in edges() {
            if now < t && t <= g {
                let mut ins = PinSignals::new();
                ins.insert("U1.PA4".into(), PinSignal::digital(lv, t));
                e.apply_inputs(&ins).unwrap();
            }
        }
        let r = match mode {
            Mode::Fixed(_) => {
                e.advance_to(g).unwrap();
                g
            }
            Mode::Until(_) => e.advance_until(g, &watch).unwrap(),
        };
        assert!(now < r && r <= g && r.as_fs().is_multiple_of(L.as_fs()));
        let trace = e.read_output_trace().unwrap().unwrap();
        for tr in &trace {
            assert!(now < tr.t && tr.t <= r, "{tr:?} outside ({now}, {r}]");
            assert!(
                tr.t.as_fs().is_multiple_of(L.as_fs()),
                "{tr:?} off the lattice"
            );
            // The value in effect inside the window is the last push.
            assert_eq!(e.sample_output(ADC, tr.t), Some(tr.signal));
        }
        if let Mode::Until(_) = mode {
            match trace.first() {
                Some(first) => assert_eq!(r, first.t, "stop at the first push"),
                None => assert_eq!(r, g),
            }
            if r < g {
                d.stops.push(r);
            }
        }
        let out = e.read_outputs().unwrap();
        let b = out[ADC];
        if let Some(last) = trace.last() {
            assert_eq!(last.signal, b, "boundary value = last push");
        }
        d.boundary.push((r, b));
        let w = e.solver_window_raw("adc_in").unwrap();
        d.points
            .extend(w[1..].iter().map(|&(t, v)| (t.to_bits(), v.to_bits())));
        d.trace.extend(trace);
        d.grants += 1;
        now = r;
    }
    d
}

fn uv(s: &PinSignal) -> u32 {
    (s.volts().unwrap() * 1e6).round() as u32
}

/// Gates: (1) the default pushes are exactly the whole-millivolt changes
/// of the every-lattice-point reference, (2) they and the solver points do
/// not depend on the partition (one cell, 1 ms, 10 ms), (3) `advance_until`
/// watching the ADC pin stops at every push and nowhere else, with the
/// solver points of the one-cell run.
#[test]
fn pushes_are_partition_independent_and_stop_the_watch() {
    let _g = NGSPICE.lock().unwrap_or_else(|e| e.into_inner());
    let t_end = Time::from_ms(60);
    let every = run(divider(t_end, AdcPush::EveryLattice), Mode::Fixed(L));
    let one = run(divider(t_end, AdcPush::OnChange), Mode::Fixed(L));

    // The reference has an entry at every lattice point.
    assert_eq!(every.trace.len() as u64, t_end.as_fs() / L.as_fs());
    assert_eq!(
        every.points, one.points,
        "the push mode never changes the solver"
    );
    let a = STOCK;
    // OnChange = the reference's entries where the whole mV change,
    // delivered as exactly that many millivolts.
    let mut key = a.push_key(uv(&every.boundary[0].1));
    let mut expect = Vec::new();
    for tr in &every.trace {
        let k = a.push_key(uv(&tr.signal));
        assert!(uv(&tr.signal) <= 3_300_000);
        if k != key {
            key = k;
            expect.push((tr.t, k * 1000));
        }
    }
    let got: Vec<(Time, u32)> = one.trace.iter().map(|tr| (tr.t, uv(&tr.signal))).collect();
    assert_eq!(got, expect);
    // The divider heads for 2531.087 mV (code 3141); 30 ms (8.3 tau) of PA4
    // high reaches 2530 mV. A push is a lattice point where the whole mV
    // changed (several mV per cell on the steep part), and every value is
    // whole millivolts.
    let top = got.iter().map(|&(_, u)| u).max().unwrap();
    assert_eq!(top, 2_530_000);
    assert!(a.code(top) > 2482, "above the watchdog window");
    // At most one push per lattice point: about 1060 per edge here.
    assert!(got.len() > 2000, "{} pushes", got.len());
    assert!(got.iter().all(|&(_, u)| u % 1000 == 0));
    // Before PA4 rises nothing changes; the first push is in the first
    // cell after the rise.
    assert_eq!(got[0].0, Time::from_us(1_010));

    for q in [Time::from_ms(1), Time::from_ms(10)] {
        let long = run(divider(t_end, AdcPush::OnChange), Mode::Fixed(q));
        assert_eq!(long.points, one.points, "solver points, {q} grants");
        assert_eq!(long.trace, one.trace, "pushes, {q} grants");
    }

    let until = run(
        divider(t_end, AdcPush::OnChange),
        Mode::Until(Time::from_ms(10)),
    );
    assert_eq!(until.points, one.points);
    assert_eq!(until.trace, one.trace);
    // Every push stops the grant at its own lattice point, unless the
    // push is the grant end anyway.
    assert!(until
        .stops
        .iter()
        .all(|s| one.trace.iter().any(|tr| tr.t == *s)));
    assert!(
        until.stops.len() + 10 >= one.trace.len(),
        "{} stops",
        until.stops.len()
    );
    // A second run is bit-identical.
    let again = run(
        divider(t_end, AdcPush::OnChange),
        Mode::Until(Time::from_ms(10)),
    );
    assert_eq!(until, again);
}

/// The contract: an ADC output needs a lattice; watching it is allowed and
/// an unknown watch is an error.
#[test]
fn adc_output_contract() {
    let _g = NGSPICE.lock().unwrap_or_else(|e| e.into_inner());
    let mut cfg = divider(Time::from_ms(1), AdcPush::OnChange);
    cfg.lattice = None;
    assert!(
        NgspiceEngine::new(cfg).is_err(),
        "ADC output without a lattice"
    );
    let mut cfg = divider(Time::from_ms(1), AdcPush::OnChange);
    cfg.adc_outputs[0].model.vref = 0.0;
    assert!(NgspiceEngine::new(cfg).is_err(), "vref 0");
    let mut e = NgspiceEngine::new(divider(Time::from_ms(1), AdcPush::OnChange)).unwrap();
    assert!(e.can_stop_early());
    // At t = 0 the pin holds 0 V (PA4 low).
    assert_eq!(e.read_outputs().unwrap()[ADC], PinSignal::analog(0.0));
    assert!(e.advance_until(L, &["nope".to_string()]).is_err());
}
