//! Integration tests against the real `libngspice.so.0`.
//!
//! ngspice is a process-global singleton, so every test takes `NGSPICE`
//! first: tests in this binary run one at a time.

use std::sync::Mutex;

use bb_analog_ngspice::{Drive, InputBinding, NgspiceConfig, NgspiceEngine, OutputBinding};
use bbcore::{DigitalOut, Engine, Level, PinSignal, PinSignals, Time};

static NGSPICE: Mutex<()> = Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    NGSPICE.lock().unwrap_or_else(|e| e.into_inner())
}

const RC_TAU_S: f64 = 1e3 * 1e-6; // 1 kOhm * 1 uF = 1 ms

/// 0 -> 1 V step, 1 ns rise, ideal source (zout = 0).
const STEP: DigitalOut = DigitalOut {
    voh: 1.0,
    vol: 0.0,
    rise: Time::from_ns(1),
    fall: Time::from_ns(1),
    zout: 0.0,
};

fn rc_config() -> NgspiceConfig {
    let mut cfg = NgspiceConfig::new(
        "R1 in out 1k\nC1 out 0 1u\n",
        Time::from_ms(5),
        Time::from_us(10),
    );
    cfg.inputs.push(InputBinding {
        pin: "IN".into(),
        source: "V_IN".into(),
        drive: Drive::Digital {
            model: STEP,
            initial: Level::Low,
        },
        glue_node: Some("in".into()),
    });
    cfg.outputs.push(OutputBinding {
        pin: "OUT".into(),
        node: "out".into(),
    });
    cfg.probes.push("in".into());
    cfg
}

/// Analytic response to a ramp of length `rise` starting at 0 is within
/// rise/2 of a step at rise/2; for t >> rise the step formula is exact to
/// well below 1e-6 relative.
fn analytic(t_s: f64) -> f64 {
    1.0 - (-(t_s - 0.5e-9) / RC_TAU_S).exp()
}

fn assert_close(t: Time, got: f64) {
    let want = analytic(t.as_secs_f64());
    let rel = (got - want).abs() / want;
    println!("t = {t}: v(out) = {got:.6}, analytic = {want:.6}, rel err = {rel:.2e}");
    assert!(rel < 0.01, "at {t}: got {got}, want {want} (rel {rel})");
}

const GRANTS: [Time; 3] = [Time::from_us(500), Time::from_ms(1), Time::from_ms(3)];

#[test]
fn rc_step_via_push_edge_matches_analytic_at_three_grants() {
    let _g = lock();
    let mut e = NgspiceEngine::new(rc_config()).expect("engine");
    assert_eq!(e.now(), Time::ZERO);
    assert!(
        e.voltage("out").unwrap().abs() < 1e-12,
        "DC op starts at 0 V"
    );

    e.push_edge("IN", Level::High, Time::ZERO).unwrap();
    for g in GRANTS {
        let t_safe = e.next_safe_time(e.now());
        assert!(t_safe >= g, "t_safe {t_safe} must allow the grant {g}");
        e.advance_to(g).unwrap();
        assert_eq!(e.now(), g);
        // No free-run: the newest solver point is exactly the grant.
        let win = e.solver_window("out").unwrap();
        assert_eq!(win.last().unwrap().0, g);
        assert!(win.iter().all(|&(t, _)| t <= g));
        assert_close(g, e.voltage("out").unwrap());
        let outs = e.read_outputs().unwrap();
        assert_eq!(
            outs.get("OUT").and_then(PinSignal::volts),
            Some(e.voltage("out").unwrap())
        );
        // Input source settled at 1 V.
        assert!((e.voltage("in").unwrap() - 1.0).abs() < 1e-9);
    }
    // Interpolated samples inside the last window (1 ms, 3 ms].
    for us in [1250u64, 2000, 2750] {
        let t = Time::from_us(us);
        assert_close(t, e.sample("out", t).unwrap());
    }
    assert!(
        e.sample("out", Time::from_us(900)).is_err(),
        "outside window"
    );
    // Cannot grant past the transient end.
    assert!(e.advance_to(Time::from_ms(6)).is_err());
    println!("solver points: {}", e.solver_points());
}

#[test]
fn rc_step_via_apply_inputs_and_falling_edge() {
    let _g = lock();
    let mut e = NgspiceEngine::new(rc_config()).expect("engine");
    let mut inp = PinSignals::new();
    inp.insert("IN".into(), PinSignal::digital(Level::High, Time::ZERO));
    e.apply_inputs(&inp).unwrap();
    for g in GRANTS {
        e.advance_to(g).unwrap();
        e.apply_inputs(&inp).unwrap(); // unchanged level: no new edge
        assert_close(g, e.voltage("out").unwrap());
    }
    // Falling edge delivered at the 3 ms boundary: discharge from v(3 ms).
    let v3 = e.voltage("out").unwrap();
    inp.insert(
        "IN".into(),
        PinSignal::digital(Level::Low, Time::from_ms(3)),
    );
    e.apply_inputs(&inp).unwrap();
    e.advance_to(Time::from_ms(4)).unwrap();
    let want = v3 * (-(1e-3 - 0.5e-9) / RC_TAU_S).exp();
    let got = e.voltage("out").unwrap();
    assert!(
        ((got - want) / want).abs() < 0.01,
        "discharge: got {got}, want {want}"
    );
    assert!(e.voltage("in").unwrap().abs() < 1e-9);
}

/// One full run: boundary values plus 50 us interpolated samples, as bits.
fn run_bits() -> (Vec<u64>, usize) {
    let mut e = NgspiceEngine::new(rc_config()).expect("engine");
    e.push_edge("IN", Level::High, Time::ZERO).unwrap();
    let mut bits = Vec::new();
    let mut prev = Time::ZERO;
    for g in GRANTS {
        e.advance_to(g).unwrap();
        bits.push(e.voltage("out").unwrap().to_bits());
        let mut t = prev;
        while t <= g {
            bits.push(e.sample("out", t).unwrap().to_bits());
            t = t + Time::from_us(50);
        }
        prev = g;
    }
    (bits, e.solver_points())
}

#[test]
fn two_identical_runs_are_bit_identical() {
    let _g = lock();
    let (a, na) = run_bits();
    let (b, nb) = run_bits();
    assert_eq!(na, nb, "solver point count differs");
    assert_eq!(a.len(), b.len());
    assert_eq!(a, b, "sampled values differ between identical runs");
    println!(
        "{} values identical over 2 runs ({na} solver points)",
        a.len()
    );
}

#[test]
fn ngspice_is_a_singleton() {
    let _g = lock();
    let e = NgspiceEngine::new(rc_config()).expect("engine");
    assert!(NgspiceEngine::new(rc_config()).is_err());
    drop(e);
    let e2 = NgspiceEngine::new(rc_config()).expect("engine after drop");
    drop(e2);
}

#[test]
fn bad_netlist_is_rejected() {
    let _g = lock();
    let mut cfg = rc_config();
    cfg.netlist.push_str(".tran 1u 1m\n");
    assert!(NgspiceEngine::new(cfg).is_err(), "engine owns .tran");
    // Rejected by ngspice itself: a diode with an undefined model.
    let mut cfg = rc_config();
    cfg.netlist.push_str("D1 out 0 NOSUCHMODEL\n");
    let err = NgspiceEngine::new(cfg).err().expect("must fail");
    println!("rejected: {err}");
    // The singleton was released: a good netlist still loads.
    NgspiceEngine::new(rc_config()).expect("engine after failure");
}

#[test]
fn generated_glue_uses_zout_and_grant_can_end_at_t_stop() {
    let _g = lock();
    // Same RC, but the 1 kOhm is the pin model's zout in the generated glue.
    let mut cfg = NgspiceConfig::new("C1 out 0 1u\n", Time::from_ms(3), Time::from_us(10));
    cfg.inputs.push(InputBinding {
        pin: "IN".into(),
        source: "V_DRV".into(),
        drive: Drive::Digital {
            model: DigitalOut { zout: 1e3, ..STEP },
            initial: Level::Low,
        },
        glue_node: Some("out".into()),
    });
    cfg.outputs.push(OutputBinding {
        pin: "OUT".into(),
        node: "out".into(),
    });
    let mut e = NgspiceEngine::new(cfg).expect("engine");
    e.push_edge("IN", Level::High, Time::ZERO).unwrap();
    assert_eq!(e.next_safe_time(Time::ZERO), Time::from_ms(3));
    e.advance_to(Time::from_ms(1)).unwrap();
    assert_close(Time::from_ms(1), e.voltage("out").unwrap());
    e.advance_to(Time::from_ms(3)).unwrap();
    assert_close(Time::from_ms(3), e.voltage("out").unwrap());
}

#[test]
fn lookahead_and_breakpoints_bound_t_safe() {
    let _g = lock();
    let mut cfg = rc_config();
    cfg.lookahead = Some(Time::from_us(100));
    let mut e = NgspiceEngine::new(cfg).expect("engine");
    assert_eq!(e.next_safe_time(Time::ZERO), Time::from_us(100));
    e.add_breakpoint(Time::from_us(40));
    assert_eq!(e.next_safe_time(Time::ZERO), Time::from_us(40));
    e.advance_to(Time::from_us(40)).unwrap();
    assert_eq!(e.next_safe_time(e.now()), Time::from_us(140));
}

/// Regression (Phase 0 verification): above ~2.3 s one f64 ulp exceeds
/// 0.5 fs, so the solver landing on `target_s` no longer rounds back to the
/// integer grant. Run 12 s in 1 ms grants with edges every second (some
/// mid-grant) and check every boundary and the settled levels.
#[test]
fn long_run_past_ten_seconds_in_1ms_grants() {
    let _g = lock();
    let mut cfg = NgspiceConfig::new(
        "R1 in out 1k\nC1 out 0 1u\n",
        Time::from_secs(12),
        Time::from_ms(1),
    );
    cfg.inputs = rc_config().inputs;
    cfg.outputs = rc_config().outputs;
    let mut e = NgspiceEngine::new(cfg).expect("engine");
    let grant = Time::from_ms(1);
    let mut level = Level::Low;
    let mut now = Time::ZERO;
    while now < Time::from_secs(12) {
        let g = now + grant;
        let ms = now.as_fs() / Time::from_ms(1).as_fs();
        if ms.is_multiple_of(1000) {
            // Toggle once a second, half a millisecond into the grant.
            level = level.toggled();
            e.push_edge("IN", level, now + Time::from_us(500)).unwrap();
        }
        e.advance_to(g)
            .unwrap_or_else(|err| panic!("grant to {g} failed: {err}"));
        assert_eq!(e.now(), g);
        let win = e.solver_window("out").unwrap();
        assert_eq!(win.last().unwrap().0, g);
        assert!(win.windows(2).all(|w| w[0].0 <= w[1].0), "monotone at {g}");
        if ms % 1000 == 999 {
            // 999.5 ms (~1000 tau) after the edge: settled.
            let want = if level == Level::High { 1.0 } else { 0.0 };
            let got = e.voltage("out").unwrap();
            assert!((got - want).abs() < 1e-3, "at {g}: {got} != {want}");
        }
        now = g;
    }
    assert_eq!(now, Time::from_secs(12));
}
