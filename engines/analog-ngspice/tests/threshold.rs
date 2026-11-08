//! ADR-003 D3 (plan step 4): `digital_in` thresholding by the producing
//! analog engine. A node voltage is thresholded with hysteresis at every
//! accepted solver point and every level change is reported in the output
//! trace, at the crossing time interpolated between the two straddling
//! points (rounded up to whole fs).
//!
//! ngspice is a process-global singleton: tests in this binary take turns.

use std::sync::Mutex;

use bb_analog_ngspice::{
    DigitalOutputBinding, Drive, InputBinding, NgspiceConfig, NgspiceEngine, OutputBinding,
};
use bbcore::{DigitalIn, DigitalOut, Engine, Level, PinSignal, PinSignals, Time, Transition};

static NGSPICE: Mutex<()> = Mutex::new(());

const L: Time = Time::from_us(10);

/// The `examples/feedback` input model: STM32-like thresholds, 5 pF.
const PB0: DigitalIn = DigitalIn {
    vih: 2.0,
    vil: 0.8,
    cin: 5e-12,
};

/// The feedback example's RC: a 25 ohm STM32 output, 1 kOhm, 1 uF, plus
/// the 5 pF `cin` glue the engine adds.
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

/// Run `cfg` to its end in grants of `grant`, delivering `edges` on input
/// `pin` before the advance through the grant holding each (true time).
/// Returns the concatenated output trace and the number of grants whose
/// boundary `read_outputs` disagreed with the trace's last level (must be
/// 0).
fn run(
    cfg: NgspiceConfig,
    grant: Time,
    pin: &str,
    edges: &[(Time, Level)],
) -> (Vec<Transition>, usize) {
    let t_end = cfg.t_stop;
    let mut e = NgspiceEngine::new(cfg).expect("engine");
    let init = e.read_outputs().unwrap();
    let mut level = init["U1.PB0"].level().unwrap();
    assert_eq!(level, Level::Low, "initial level from v(0) = 0");
    let mut all = Vec::new();
    let mut mismatches = 0;
    let mut now = Time::ZERO;
    while now < t_end {
        let g = (now + grant).min(t_end);
        for &(t, lv) in edges {
            if now < t && t <= g {
                let mut ins = PinSignals::new();
                ins.insert(pin.into(), PinSignal::digital(lv, t));
                e.apply_inputs(&ins).unwrap();
            }
        }
        e.advance_to(g).unwrap();
        let trace = e
            .read_output_trace()
            .unwrap()
            .expect("digital outputs give a trace");
        for tr in &trace {
            assert!(now < tr.t && tr.t <= g, "{tr:?} outside ({now}, {g}]");
            assert_eq!(
                tr.signal,
                PinSignal::digital(tr.signal.level().unwrap(), tr.t)
            );
            assert_ne!(tr.signal.level(), Some(level), "levels alternate");
            level = tr.signal.level().unwrap();
        }
        let out = e.read_outputs().unwrap();
        if out["U1.PB0"].level() != Some(level) {
            mismatches += 1;
        }
        // The analog output is not in the trace.
        assert!(trace.iter().all(|tr| tr.pin == "U1.PB0"));
        all.extend(trace);
        now = g;
    }
    (all, mismatches)
}

fn ceil_l(t: Time) -> Time {
    Time::from_fs(t.as_fs().div_ceil(L.as_fs()) * L.as_fs())
}

/// The RC crossings land at the analytic times (ADR-003 "Feedback-path
/// correctness"): PA5 rises at 1 ms + 1 fs, SENSE crosses vih after
/// tau * ln(3.3 / 1.3); PA5 falls at 8 ms + 1 fs, SENSE crosses vil after
/// tau * ln(v / 0.8), with tau = 1025 ohm * (1 uF + 5 pF). Each lands in
/// the analytic lattice cell, and the trace is bit-identical for one-cell,
/// 70 us and 1 ms grants.
#[test]
fn rc_crossings_are_analytic_and_partition_independent() {
    let _g = NGSPICE.lock().unwrap_or_else(|e| e.into_inner());
    let fs1 = Time::from_fs(1);
    let edges = [
        (Time::from_ms(1) + fs1, Level::High),
        (Time::from_ms(8) + fs1, Level::Low),
    ];
    let t_end = Time::from_ms(12);
    let (one_cell, m1) = run(rc_config(t_end), L, "U1.PA5", &edges);
    let (medium, m2) = run(rc_config(t_end), Time::from_us(70), "U1.PA5", &edges);
    let (long, m3) = run(rc_config(t_end), Time::from_ms(1), "U1.PA5", &edges);
    assert_eq!(
        (m1, m2, m3),
        (0, 0, 0),
        "read_outputs agrees with the trace"
    );
    assert_eq!(
        one_cell.len(),
        2,
        "one transition per direction: {one_cell:?}"
    );
    assert_eq!(one_cell, medium, "70 us grants change the trace");
    assert_eq!(one_cell, long, "1 ms grants change the trace");

    let tau = 1025.0 * (1e-6 + 5e-12);
    // The 5 ns linear ramp delays the exponential by half its length.
    let ramp = 2.5e-9;
    // SENSE when PA5 falls (7 ms = 6.8 tau after the rise: not quite 3.3 V).
    let v_fall = 3.3 * (1.0 - (-(7e-3f64) / tau).exp());
    let want = [
        1e-3 + ramp + tau * (3.3f64 / 1.3).ln(),
        8e-3 + ramp + tau * (v_fall / 0.8).ln(),
    ];
    for (tr, (w, lv)) in one_cell
        .iter()
        .zip(want.iter().zip([Level::High, Level::Low]))
    {
        let got = tr.t.as_secs_f64();
        println!(
            "U1.PB0 -> {lv:?} at {} (analytic {:.9} ms, diff {:.1} ns), cell ends {}",
            tr.t,
            w * 1e3,
            (got - w) * 1e9,
            ceil_l(tr.t)
        );
        assert_eq!(tr.signal.level(), Some(lv));
        assert!((got - w).abs() < 1e-6, "crossing {got} s vs analytic {w} s");
        assert_eq!(
            ceil_l(tr.t),
            ceil_l(Time::from_fs((w * 1e15).ceil() as u64)),
            "crossing in the analytic cell"
        );
    }
}

/// A slow ramp with ripple through the 0.8-2.0 V band: a single threshold
/// would see several crossings each way, hysteresis exactly one per
/// direction.
#[test]
fn hysteresis_gives_one_transition_per_direction() {
    let _g = NGSPICE.lock().unwrap_or_else(|e| e.into_inner());
    let t_end = Time::from_ms(10);
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
    let mut e = NgspiceEngine::new(cfg).expect("engine");
    let mut trace = Vec::new();
    let mut raw = Vec::new();
    let mut now = Time::ZERO;
    while now < t_end {
        let g = now + Time::from_ms(1);
        let mut ins = PinSignals::new();
        if now == Time::from_ms(1) {
            ins.insert("RAMP".into(), PinSignal::analog(3.3));
        } else if now == Time::from_ms(5) {
            ins.insert("RAMP".into(), PinSignal::analog(0.0));
        }
        if !ins.is_empty() {
            e.apply_inputs(&ins).unwrap();
        }
        e.advance_to(g).unwrap();
        trace.extend(e.read_output_trace().unwrap().unwrap());
        raw.extend(e.solver_window("n").unwrap().into_iter().skip(1));
        now = g;
    }
    // Without hysteresis: crossings of the midpoint 1.4 V and of vih 2.0 V.
    let crossings = |th: f64| {
        raw.windows(2)
            .filter(|w| (w[0].1 < th) != (w[1].1 < th))
            .count()
    };
    println!(
        "single-threshold crossings: {} at 1.4 V, {} at 2.0 V; hysteresis trace: {trace:?}",
        crossings(1.4),
        crossings(2.0)
    );
    assert!(crossings(2.0) > 2, "the ripple must recross vih");
    let levels: Vec<Level> = trace.iter().map(|t| t.signal.level().unwrap()).collect();
    assert_eq!(levels, vec![Level::High, Level::Low], "{trace:?}");
    assert!(trace[0].t > Time::from_ms(1) && trace[1].t > Time::from_ms(5));
}

/// The `examples/feedback-open` circuit (an internal PULSE source, no
/// inputs): raw solver points, samples and the PB0 trace are bit-identical
/// for one-cell and 10 ms grants.
#[test]
fn pulse_board_is_partition_independent() {
    let _g = NGSPICE.lock().unwrap_or_else(|e| e.into_inner());
    let t_end = Time::from_ms(120);
    let body = "V_DRV DRV 0 PULSE(0 3.3 50m 5n 5n 49.999995m 100m)\nR_DRV DRV DRVZ 25\n\
                R2 DRVZ SENSE 1k\nC1 SENSE 0 1u\n.options RELTOL=1e-5 VNTOL=1e-8 ABSTOL=1e-13\n";
    let dump = |grant: Time| {
        let mut cfg = NgspiceConfig::new(body, t_end, L);
        cfg.lattice = Some(L);
        cfg.outputs.push(OutputBinding {
            pin: "SENSE".into(),
            node: "sense".into(),
        });
        cfg.digital_outputs.push(DigitalOutputBinding {
            pin: "U1.PB0".into(),
            node: "sense".into(),
            model: PB0,
        });
        let mut e = NgspiceEngine::new(cfg).expect("engine");
        let mut pts = Vec::new();
        let mut samples = Vec::new();
        let mut trace = Vec::new();
        let mut now = Time::ZERO;
        let mut next_sample = Time::from_us(10);
        while now < t_end {
            let g = (now + grant).min(t_end);
            e.advance_to(g).unwrap();
            let w = e.solver_window_raw("sense").unwrap();
            pts.extend(w[1..].iter().map(|&(t, v)| (t.to_bits(), v.to_bits())));
            while next_sample <= g {
                let s = e.sample_output("SENSE", next_sample).unwrap();
                samples.push((next_sample, s.volts().unwrap().to_bits()));
                next_sample = next_sample + Time::from_us(10);
            }
            trace.extend(e.read_output_trace().unwrap().unwrap());
            now = g;
        }
        (pts, samples, trace)
    };
    let (p1, s1, t1) = dump(L);
    let (p2, s2, t2) = dump(Time::from_ms(10));
    println!(
        "{} solver points, {} samples, trace {t1:?}",
        p1.len(),
        s1.len()
    );
    if p1 != p2 {
        let k = p1
            .iter()
            .zip(&p2)
            .position(|(a, b)| a != b)
            .unwrap_or(p1.len().min(p2.len()));
        let show = |p: &[(u64, u64)]| -> Vec<(f64, f64)> {
            p[k.saturating_sub(2)..(k + 3).min(p.len())]
                .iter()
                .map(|&(t, v)| (f64::from_bits(t), f64::from_bits(v)))
                .collect()
        };
        panic!(
            "solver points differ at {k} ({} vs {}): one-cell {:?} vs 10 ms {:?}",
            p1.len(),
            p2.len(),
            show(&p1),
            show(&p2)
        );
    }
    for (a, b) in s1.iter().zip(&s2) {
        assert_eq!(
            a,
            b,
            "sample at {} differs: {} vs {}",
            a.0,
            f64::from_bits(a.1),
            f64::from_bits(b.1)
        );
    }
    assert_eq!(t1, t2);
}

/// ADR-003 Amendment 12: past ~2.3 s one ulp of the solver time exceeds
/// 0.5 fs, so the solver point the lattice clamp puts on `kL` can sit a
/// fraction of a fs (or more) away from `kL` while still being recognised
/// as on it. Its fs time must be `kL` whether or not a grant ends there, or
/// a threshold reached exactly at `kL` is reported at `kL` in one-cell
/// grants and at `kL + 1 fs` (one lattice cell later at the consumer) in
/// long grants. A PULSE whose ramps end exactly at vih and at vil on lattice
/// points after 4 s: the trace, the samples at every lattice point and the
/// early stops of `advance_until` are identical for one-cell, 10 ms and 1 s
/// grants, and the transitions are exactly at the lattice points.
#[test]
fn lattice_point_crossings_after_2s_are_partition_independent() {
    let _g = NGSPICE.lock().unwrap_or_else(|e| e.into_inner());
    let t_end = Time::from_ms(4170);
    let (from, to) = (Time::from_ms(4120), Time::from_ms(4160));
    let body = "V_N n 0 PULSE(0.8 2.0 4.13 7m 7m 9m 1)\nR_LOAD n 0 1meg\n";
    let cfg = || {
        let mut cfg = NgspiceConfig::new(body, t_end, L);
        cfg.lattice = Some(L);
        cfg.outputs.push(OutputBinding {
            pin: "N".into(),
            node: "n".into(),
        });
        cfg.digital_outputs.push(DigitalOutputBinding {
            pin: "U1.PB0".into(),
            node: "n".into(),
            model: DigitalIn { cin: 0.0, ..PB0 },
        });
        cfg
    };
    // `next(now)` is the next grant end; `until` uses advance_until on PB0
    // inside [from, to]. Returns the trace, the samples of N at every
    // lattice point of [from, to] and the grant ends inside [from, to].
    let dump = |next: &dyn Fn(Time) -> Time, until: bool| {
        let mut e = NgspiceEngine::new(cfg()).expect("engine");
        let watch = ["U1.PB0".to_string()];
        let (mut trace, mut samples, mut ends) = (Vec::new(), Vec::new(), Vec::new());
        let mut now = Time::ZERO;
        let mut next_sample = from;
        while now < t_end {
            let g = next(now).min(t_end);
            let r = if until && now >= from && now < to {
                e.advance_until(g, &watch).unwrap()
            } else {
                e.advance_to(g).unwrap();
                g
            };
            while next_sample <= r && next_sample <= to {
                if next_sample > now {
                    let v = e.sample_output("N", next_sample).unwrap();
                    samples.push((next_sample, v.volts().unwrap().to_bits()));
                }
                next_sample = next_sample + L;
            }
            if r > from && r <= to {
                ends.push(r);
            }
            trace.extend(e.read_output_trace().unwrap().unwrap());
            now = r;
        }
        (trace, samples, ends)
    };
    let one_cell = |now: Time| {
        if now >= from && now < to {
            now + L
        } else {
            now + Time::from_ms(10)
        }
    };
    let (t1, s1, _) = dump(&one_cell, false);
    let (t2, s2, _) = dump(&|now| now + Time::from_ms(10), false);
    let (t3, s3, _) = dump(&|now| now + Time::from_secs(1), false);
    let (t4, s4, stops) = dump(
        &|now| {
            let g = now + Time::from_secs(1);
            if now < from {
                g.min(from)
            } else {
                g
            }
        },
        true,
    );
    println!("one-cell trace {t1:?}");
    // The rise's corner value comes out a hair under 2.0 V, so PB0 rises
    // at the next solver point, inside the cell after 4.137 s. The fall ends
    // on V1 = 0.8 V exactly: vil is reached exactly on the lattice point.
    let got: Vec<(Time, Level)> = t1
        .iter()
        .map(|tr| (tr.t, tr.signal.level().unwrap()))
        .collect();
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!(got[0].1, Level::High);
    assert!(got[0].0 > Time::from_ms(4137) && got[0].0 <= Time::from_ms(4137) + L);
    assert_eq!(
        got[1],
        (Time::from_ms(4153), Level::Low),
        "vil reached exactly at the lattice point"
    );
    let want: Vec<Time> = got.iter().map(|&(t, _)| t).collect();
    assert_eq!(t1, t2, "10 ms grants change the trace");
    assert_eq!(t1, t3, "1 s grants change the trace");
    assert_eq!(t1, t4, "advance_until changes the trace");
    assert_eq!(s1.len(), 4001);
    assert_eq!(s1, s2, "10 ms grants change the samples");
    assert_eq!(s1, s3, "1 s grants change the samples");
    assert_eq!(s1, s4, "advance_until changes the samples");
    // The early stops are the transitions' lattice points (ceil_L of
    // exactly kL is kL), not one cell later.
    let want_stops: Vec<Time> = want.iter().map(|&t| ceil_l(t)).collect();
    assert_eq!(&stops[..2], &want_stops[..], "early stops {stops:?}");
}

/// The engine adds `cin` as a capacitor to ground and rejects bad models.
#[test]
fn cin_glue_and_model_checks() {
    let _g = NGSPICE.lock().unwrap_or_else(|e| e.into_inner());
    let mut bad = rc_config(Time::from_ms(1));
    bad.digital_outputs[0].model.vil = 2.5;
    let err = NgspiceEngine::new(bad).err().expect("vil >= vih must fail");
    assert!(err.to_string().contains("vil < vih"), "{err}");
    let mut bad = rc_config(Time::from_ms(1));
    bad.digital_outputs[0].model.cin = -1.0;
    assert!(NgspiceEngine::new(bad).is_err());
    let mut dup = rc_config(Time::from_ms(1));
    dup.digital_outputs[0].pin = "SENSE".into();
    assert!(NgspiceEngine::new(dup).is_err(), "pin bound twice");
    // A 1 uF node with or without 5 pF: the crossing moves by ~5 ppm of tau
    // (about 5 ns), so a 5 nF cin must move it measurably (~5 us).
    let edges = [(Time::from_us(100) + Time::from_fs(1), Level::High)];
    let t_end = Time::from_ms(2);
    let (small, _) = run(rc_config(t_end), L, "U1.PA5", &edges);
    let mut big = rc_config(t_end);
    big.digital_outputs[0].model.cin = 5e-9;
    let (big, _) = run(big, L, "U1.PA5", &edges);
    let d = big[0].t.as_secs_f64() - small[0].t.as_secs_f64();
    println!("crossing delay from cin 5 nF vs 5 pF: {:.1} ns", d * 1e9);
    assert!(d > 3e-6 && d < 7e-6, "cin glue missing: {d}");
}
