//! ADR-003 D2: with a grant lattice, ngspice's solver points do not depend on
//! the grant partition (`TMAX = L`, a lattice clamp, breakpoint release at
//! the cell start).
//!
//! ngspice is a process-global singleton: tests in this binary take turns.

use std::sync::Mutex;

use bb_analog_ngspice::{Drive, InputBinding, NgspiceConfig, NgspiceEngine, OutputBinding};
use bbcore::{DigitalOut, Engine, Level, PinSignal, PinSignals, Time};

static NGSPICE: Mutex<()> = Mutex::new(());

const L: Time = Time::from_us(10);
const T_END: Time = Time::from_ms(3);

/// RC with tau = 220 ohm * 10 nF = 2.2 us: around an edge the solver's
/// natural step is far below the 10 us lattice.
fn config() -> NgspiceConfig {
    let mut cfg = NgspiceConfig::new("R1 in out 220\nC1 out 0 10n\n", T_END, L);
    cfg.lattice = Some(L);
    cfg.inputs.push(InputBinding {
        pin: "IN".into(),
        source: "V_IN".into(),
        drive: Drive::Digital {
            model: DigitalOut {
                voh: 3.3,
                vol: 0.0,
                rise: Time::from_ns(5),
                fall: Time::from_ns(5),
                zout: 25.0,
            },
            initial: Level::Low,
        },
        glue_node: Some("in".into()),
    });
    cfg.outputs.push(OutputBinding {
        pin: "OUT".into(),
        node: "out".into(),
    });
    cfg
}

/// Edges in mid-cell (none on a lattice point), at most one per 1 ms grant.
const EDGES: [(Time, Level); 3] = [
    (Time::from_ps(123_456_789), Level::High),
    (Time::from_ps(1_500_003_300), Level::Low),
    (Time::from_ps(2_222_222_222), Level::High),
];

/// Every accepted solver point (time, v(out)) as raw f64 bits, running to
/// `T_END` in grants of `grant`. Each edge is delivered before the advance
/// through the grant that holds it, with its true edge_time (Gauss-Seidel,
/// as the kernel does for an engine registered after the producer).
fn solver_points(grant: Time) -> Vec<(u64, u64)> {
    let mut e = NgspiceEngine::new(config()).expect("engine");
    let mut pts: Vec<(u64, u64)> = e
        .solver_window_raw("out")
        .unwrap()
        .iter()
        .map(|&(t, v)| (t.to_bits(), v.to_bits()))
        .collect();
    let mut now = Time::ZERO;
    while now < T_END {
        let g = (now + grant).min(T_END);
        for &(t, level) in &EDGES {
            if now < t && t <= g {
                let mut ins = PinSignals::new();
                ins.insert("IN".into(), PinSignal::digital(level, t));
                e.apply_inputs(&ins).unwrap();
            }
        }
        e.advance_to(g).unwrap();
        // Skip the first point: it is the previous boundary, already taken.
        let win = e.solver_window_raw("out").unwrap();
        pts.extend(win[1..].iter().map(|&(t, v)| (t.to_bits(), v.to_bits())));
        now = g;
    }
    pts
}

/// ADR-003 step 2 gate: a raw solver-point dump is bit-identical for
/// one-cell (10 us) and 1 ms grants, on a netlist whose natural step is not
/// 10 us, with edges in mid-cell. Every lattice point is an accepted point.
#[test]
fn solver_points_do_not_depend_on_the_partition() {
    let _g = NGSPICE.lock().unwrap_or_else(|e| e.into_inner());
    let one_cell = solver_points(L);
    let long = solver_points(Time::from_ms(1));
    let medium = solver_points(Time::from_us(70));
    let cells = (T_END.as_fs() / L.as_fs()) as usize;
    println!(
        "solver points: {} (one-cell), {} (1 ms), {} (70 us); lattice cells {cells}",
        one_cell.len(),
        long.len(),
        medium.len()
    );
    assert!(
        one_cell.len() > cells + 20,
        "the edges must force steps below the lattice"
    );
    assert_eq!(one_cell.len(), long.len(), "point counts differ");
    assert_eq!(one_cell, long, "solver points differ: one-cell vs 1 ms");
    assert_eq!(one_cell, medium, "solver points differ: one-cell vs 70 us");
    // An accepted point on every lattice point.
    let times: Vec<f64> = long.iter().map(|p| f64::from_bits(p.0)).collect();
    for k in 0..=cells as u64 {
        let t = Time::from_fs(k * L.as_fs()).as_secs_f64();
        assert!(
            times
                .iter()
                .any(|&s| (s - t).abs() <= 4.0 * f64::EPSILON * t.max(1e-12)),
            "no solver point at lattice point {k}"
        );
    }
}

/// TMAX must divide the lattice.
#[test]
fn tmax_must_divide_the_lattice() {
    let _g = NGSPICE.lock().unwrap_or_else(|e| e.into_inner());
    let mut cfg = config();
    cfg.t_max = Time::from_us(3);
    let err = NgspiceEngine::new(cfg).err().expect("must fail");
    assert!(err.to_string().contains("divide"), "{err}");
    let mut cfg = config();
    cfg.t_max = Time::from_us(5);
    NgspiceEngine::new(cfg).expect("5 us divides 10 us");
}
