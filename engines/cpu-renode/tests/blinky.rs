//! Renode engine against the real STM32F103 blinky firmware.
//!
//! Needs Renode (`make setup`) and the firmware (`make firmware`; built here
//! on demand). Without Renode the tests print a SKIP line and pass, unless
//! `BB_REQUIRE_RENODE=1` (set by `make test`), which makes them fail.
//!
//! Run with `cargo test -p bb-cpu-renode -- --nocapture` to see wall times.

use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

use bb_cpu_renode::{
    find_renode, Fault, PinDirection, PinSpec, RenodeConfig, RenodeEngine, Verify,
};
use bbcore::{
    CsvWriter, Direction, Engine, EngineError, Event, EventSink, Level, PinSignal, PinSignals,
    Probe, SchedError, Scheduler, SchedulerConfig, Time, Transition,
};

/// The default grant lattice (Renode quantum, `--renode-quantum`).
const L: Time = Time::from_us(10);

/// SysTick runs at 72 MHz in Renode's stm32f103.repl: one tick is 13.89 ns.
const SYSTICK_TICK: Time = Time::from_fs(13_888_889);
const TOGGLE: Time = Time::from_ms(50);

/// Serializes on-demand firmware builds across test threads.
static BUILD: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// `examples/spike/firmware/build/<name>.elf`, built on demand.
fn firmware(name: &str) -> Option<PathBuf> {
    let _g = BUILD.lock().unwrap_or_else(|e| e.into_inner());
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/spike/firmware");
    let elf = dir.join(format!("build/{name}.elf"));
    if !elf.is_file() {
        let ok = Command::new("make")
            .arg("-C")
            .arg(&dir)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            let have_gcc = Command::new("arm-none-eabi-gcc")
                .arg("--version")
                .output()
                .is_ok();
            assert!(!have_gcc, "building {} failed", elf.display());
            skip(&format!(
                "arm-none-eabi-gcc not installed and {} missing (run `make setup && make firmware`)",
                elf.display()
            ));
            return None;
        }
    }
    Some(elf)
}

/// `BB_REQUIRE_RENODE=1` (set by `make test` / CI) turns a SKIP into a
/// failure, so the gate cannot pass without running these tests.
fn skip(msg: &str) {
    let required = std::env::var("BB_REQUIRE_RENODE").is_ok_and(|v| v == "1");
    assert!(!required, "BB_REQUIRE_RENODE=1 but {msg}");
    eprintln!("SKIP: {msg}");
}

fn setup() -> Option<RenodeConfig> {
    setup_fw("blinky")
}

fn setup_fw(name: &str) -> Option<RenodeConfig> {
    let Some(renode) = find_renode() else {
        skip("renode not installed (run `make setup`, or set BB_RENODE)");
        return None;
    };
    let elf = firmware(name)?;
    Some(RenodeConfig::stm32f103(renode, elf))
}

struct Run {
    edges: Vec<(Time, u8)>,
    events: Vec<u8>,
    csv: String,
    grants: u64,
    wall_s: f64,
}

/// Run the firmware under the scheduler to `t_end` on the grant lattice
/// `lattice` (Renode's quantum) with grants of `grant` (a multiple of it).
/// With `csv_period`, a sampled `CsvWriter` records PA5 (it must not change
/// the grant partition).
fn run_blinky(
    cfg: &RenodeConfig,
    lattice: Time,
    grant: Time,
    t_end: Time,
    csv_period: Option<Time>,
) -> Run {
    let mut cfg = cfg.clone();
    cfg.quantum = lattice;
    let engine = RenodeEngine::start(&cfg).expect("start renode");
    let mut sched = Scheduler::new(SchedulerConfig {
        max_quantum: grant,
        loop_quantum: None,
        lookahead: false,
        log_grants: false,
        event_sampling: None,
    })
    .unwrap();
    let id = sched.add_engine(Box::new(engine)).unwrap();
    sched.bind(id, "PA5", "PA5", Direction::Output).unwrap();
    let mut sink = EventSink::new(Vec::new());
    let mut csv =
        csv_period.map(|p| CsvWriter::sampled(Vec::new(), p, vec!["PA5".into()]).unwrap());
    let t0 = Instant::now();
    let summary = {
        let mut probes: Vec<&mut dyn Probe> = Vec::new();
        if let Some(c) = csv.as_mut() {
            probes.push(c);
        }
        sched.run(t_end, &mut sink, &mut probes).expect("run")
    };
    let wall_s = t0.elapsed().as_secs_f64();
    assert_eq!(summary.now, t_end);
    let events = sink.into_inner();
    let edges = String::from_utf8(events.clone())
        .unwrap()
        .lines()
        .filter_map(|l| match Event::from_line(l).unwrap() {
            Event::PinEdge { t_fs, level, .. } => Some((t_fs, level)),
            _ => None,
        })
        .collect();
    Run {
        edges,
        events,
        csv: csv
            .map(|c| String::from_utf8(c.into_inner()).unwrap())
            .unwrap_or_default(),
        grants: summary.grants,
        wall_s,
    }
}

/// Measured tolerance of the blinky edges against the ideal k * 50 ms, for
/// any of the partitions below (ADR-002 Amendment 3). Edges are never late by
/// more than the 1 fs boundary relabel. They run early by a drift that grows
/// with sim time (Renode's SysTick reload timing, about 130 ns by 500 ms) plus
/// a partition-dependent part of tens of ns.
const EARLY_TOL: Time = Time::from_ns(200);
const LATE_TOL: Time = Time::from_fs(1);

fn check_blinky_edges(edges: &[(Time, u8)], t_end: Time) {
    let expected = (t_end.as_fs() / TOGGLE.as_fs()) as usize;
    assert!(
        edges.len() == expected || edges.len() + 1 == expected,
        "expected {expected} edges (the last may land exactly on t_end), got {edges:?}"
    );
    assert!(edges.len() >= 4, "too few edges: {edges:?}");
    for (k, (t, level)) in edges.iter().enumerate() {
        let ideal = Time::from_fs(TOGGLE.as_fs() * (k as u64 + 1));
        assert!(
            *t + EARLY_TOL >= ideal && *t <= ideal + LATE_TOL,
            "edge {k} at {t}, expected {ideal} -{EARLY_TOL}/+{LATE_TOL}"
        );
        assert_eq!(*level, if k % 2 == 0 { 1 } else { 0 }, "edge {k} level");
    }
    for w in edges.windows(2) {
        let dt = w[1].0 - w[0].0;
        assert!(
            dt.as_fs().abs_diff(TOGGLE.as_fs()) <= 8 * SYSTICK_TICK.as_fs(),
            "edge spacing {dt}, expected 50ms +/- 8 SysTick ticks"
        );
    }
}

/// Edges whose time is not "previous boundary + 1 fs", i.e. reported from
/// inside the grant rather than relabelled at its start.
fn mid_grant_edges(edges: &[(Time, u8)], grant: Time) -> usize {
    edges
        .iter()
        .filter(|(t, _)| t.as_fs() % grant.as_fs() != 1)
        .count()
}

/// Golden edge list for the Phase 0 spike (lattice 10 us, 500 ms), Renode
/// 1.17.0. The first five SysTick expiries land on lattice points and are
/// reported +1 fs (lattice rule 1); the rest are timer-driven syncs off the
/// lattice and reported exactly (rule 2). ADR-003 D2: every partition of the
/// 10 us lattice gives this list.
const GOLDEN_10US: [u64; 10] = [
    50_000_000_000_001,
    100_000_000_000_001,
    150_000_000_000_001,
    200_000_000_000_001,
    250_000_000_000_001,
    299_999_930_000_000,
    349_999_910_000_000,
    399_999_900_000_000,
    449_999_890_000_000,
    499_999_870_000_000,
];

/// Two independent 500 ms runs in 10 us grants (the spike partition): the
/// exact golden edge list, identical event streams.
#[test]
fn blinky_500ms_in_10us_grants_is_exact_and_deterministic() {
    let Some(cfg) = setup() else { return };
    let grant = Time::from_us(10);
    let t_end = Time::from_ms(500);
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let cfg = cfg.clone();
            std::thread::spawn(move || run_blinky(&cfg, L, grant, t_end, None))
        })
        .collect();
    let runs: Vec<Run> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    for (i, r) in runs.iter().enumerate() {
        eprintln!(
            "run {i}: 500ms sim in {} grants of {grant}: wall {:.2} s, edges {:?}",
            r.grants, r.wall_s, r.edges
        );
    }
    assert_eq!(runs[0].grants, 50_000);
    check_blinky_edges(&runs[0].edges, t_end);
    let got: Vec<u64> = runs[0].edges.iter().map(|e| e.0.as_fs()).collect();
    assert_eq!(got, GOLDEN_10US, "edge times differ from the golden list");
    assert!(mid_grant_edges(&runs[0].edges, grant) >= 5);
    assert_eq!(
        runs[0].edges, runs[1].edges,
        "edge lists differ between runs"
    );
    assert_eq!(runs[0].events, runs[1].events, "event streams differ");
}

/// `pin_edge` and `log` records of an event stream (everything but `grant`).
fn non_grant_records(events: &[u8]) -> Vec<String> {
    String::from_utf8(events.to_vec())
        .unwrap()
        .lines()
        .filter(|l| !l.contains("\"type\":\"grant\""))
        .map(String::from)
        .collect()
}

/// ADR-003 D2/D3 gate: on a fixed 10 us lattice, the 10, 70 and 100 us and
/// 1, 10, 50 and 500 ms partitions give the identical `pin_edge` list (the
/// golden one), identical non-grant records and an identical sampled CSV. The
/// "70 us anomaly" (first edges at 50 ms instead of 50 ms + 1 fs when the
/// quantum was the grant) is gone: a stamp on a lattice point is reported
/// +1 fs wherever grants end. From 100 ms on a grant holds several edges of
/// PA5 (the trace, D3); the 500 ms run holds all ten in one grant.
#[test]
fn blinky_partitions_of_one_lattice_are_identical() {
    let Some(cfg) = setup() else { return };
    let t_end = Time::from_ms(500);
    let grants = [
        Time::from_us(10),
        Time::from_us(70),
        Time::from_us(100),
        Time::from_ms(1),
        Time::from_ms(10),
        Time::from_ms(50),
        Time::from_ms(500),
    ];
    let csv = Some(Time::from_us(50));
    let handles: Vec<_> = grants
        .iter()
        .map(|&g| {
            let cfg = cfg.clone();
            std::thread::spawn(move || run_blinky(&cfg, L, g, t_end, csv))
        })
        .collect();
    let runs: Vec<Run> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    for (r, g) in runs.iter().zip(grants) {
        eprintln!(
            "lattice {L}, grants of {g}: {} grants, wall {:.2} s, edges {:?}",
            r.grants, r.wall_s, r.edges
        );
    }
    for (r, g) in runs.iter().zip(grants) {
        assert_eq!(
            r.grants,
            t_end.as_fs().div_ceil(g.as_fs()),
            "grant count at {g}"
        );
        let got: Vec<u64> = r.edges.iter().map(|e| e.0.as_fs()).collect();
        assert_eq!(got, GOLDEN_10US, "edges at {g} grants differ from golden");
        assert_eq!(
            non_grant_records(&r.events),
            non_grant_records(&runs[0].events),
            "non-grant records at {g}"
        );
        assert_eq!(r.csv, runs[0].csv, "nets.csv at {g}");
    }
    // The 70 us anomaly is gone: 50 ms is not a 70 us boundary, yet the
    // first edge is 50 ms + 1 fs as in every other partition.
    assert_eq!(runs[1].edges[0].0, Time::from_ms(50) + Time::from_fs(1));
    assert_eq!(runs[6].grants, 1, "one 500 ms grant carries all ten edges");
}

/// Drive the engine directly with uneven lattice-aligned grants: Renode's
/// virtual time must equal the grant after every advance, and outputs must
/// never claim an edge after the boundary. A grant off the lattice is
/// refused, not clamped, and Renode is not touched. `t_safe` is `Time::MAX`
/// (ADR-003 D4), and one long advance reports every edge in its trace.
#[test]
fn virtual_time_tracks_every_grant() {
    let Some(cfg) = setup() else { return };
    let mut e = RenodeEngine::start(&cfg).expect("start renode");
    assert_eq!(e.grant_lattice(), Some(L));
    let mut now = Time::ZERO;
    let steps = [
        Time::from_us(10),
        Time::from_us(30),
        Time::from_us(70),
        Time::from_us(300),
        Time::from_us(20),
    ];
    for k in 0..100 {
        let t_safe = e.next_safe_time(now);
        assert_eq!(t_safe, Time::MAX);
        let g = now + steps[k % steps.len()];
        e.advance_to(g).unwrap();
        now = g;
        let vt = e.query_renode_time_ns().unwrap();
        assert_eq!(vt, now.as_fs() / 1_000_000, "after grant {k} to {now}");
        assert_eq!(e.renode_time_ns(), vt);
        match e.read_outputs().unwrap()["PA5"] {
            PinSignal::Digital { edge_time, level } => {
                assert!(edge_time <= now);
                assert_eq!(level, Level::Low, "no edge expected before 50 ms");
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    assert!(now < Time::from_ms(50));
    assert_eq!(e.read_output_trace().unwrap(), Some(vec![]));
    e.next_safe_time(now);
    // Off the lattice: refused.
    let err = e.advance_to(now + Time::from_us(5)).unwrap_err();
    assert!(err.to_string().contains("lattice"), "{err}");
    assert!(e.advance_to(now + Time::from_ns(1)).is_err());
    assert_eq!(e.query_renode_time_ns().unwrap(), now.as_fs() / 1_000_000);
    // One advance to 180 ms: the trace holds the 50, 100 and 150 ms edges,
    // alternating, and its last entry is the boundary value.
    e.advance_to(Time::from_ms(180)).unwrap();
    let trace = e.read_output_trace().unwrap().unwrap();
    let got: Vec<(u64, Option<Level>)> = trace
        .iter()
        .map(|tr| (tr.t.as_fs(), tr.signal.level()))
        .collect();
    assert_eq!(
        got,
        vec![
            (GOLDEN_10US[0], Some(Level::High)),
            (GOLDEN_10US[1], Some(Level::Low)),
            (GOLDEN_10US[2], Some(Level::High)),
        ]
    );
    assert_eq!(e.read_outputs().unwrap()["PA5"], trace[2].signal);
}

/// Regression (Phase 0 verification): the CSV sample rate must not change
/// the firmware's edge times. 1 kHz puts every 50 ms on a sample instant,
/// 1024 Hz puts almost none there; with sampled probes both runs use the same
/// 1 ms partition and give identical edges, and each CSV row agrees with the
/// edge list.
#[test]
fn csv_sample_rate_does_not_change_edges() {
    let Some(cfg) = setup() else { return };
    let grant = Time::from_ms(1);
    let t_end = Time::from_ms(350);
    let periods = [Time::from_ms(1), Time::from_fs(976_562_500_000)]; // 1 kHz, 1024 Hz
    let handles: Vec<_> = periods
        .iter()
        .map(|&p| {
            let cfg = cfg.clone();
            std::thread::spawn(move || run_blinky(&cfg, L, grant, t_end, Some(p)))
        })
        .collect();
    let runs: Vec<Run> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    for (r, p) in runs.iter().zip(periods) {
        eprintln!("csv period {p}: {} grants, edges {:?}", r.grants, r.edges);
    }
    assert_eq!(runs[0].grants, runs[1].grants, "partition changed");
    assert_eq!(runs[0].edges, runs[1].edges, "edges depend on the CSV rate");
    assert_eq!(runs[0].events, runs[1].events);
    check_blinky_edges(&runs[0].edges, t_end);
    // ADR-003 D2: 1 ms grants on the 10 us lattice reproduce the 10 us edges.
    let got: Vec<u64> = runs[0].edges.iter().map(|e| e.0.as_fs()).collect();
    assert_eq!(got, GOLDEN_10US[..got.len()]);
    for (r, p) in runs.iter().zip(periods) {
        let mut rows = 0;
        for line in r.csv.lines().skip(1) {
            let (t, v) = line.split_once(',').unwrap();
            let t = Time::from_fs(t.parse().unwrap());
            let want = r.edges.iter().filter(|e| e.0 <= t).count() % 2;
            assert_eq!(v, want.to_string(), "PA5 at {t} (period {p})");
            rows += 1;
        }
        assert_eq!(
            rows as u64,
            t_end.as_fs() / p.as_fs() + 1,
            "rows (period {p})"
        );
    }
}

/// Blinky in 100 us grants to `t_end` with an injected fault; returns the
/// run result and the last completed boundary.
fn run_faulty(
    cfg: &RenodeConfig,
    verify: Verify,
    fault: Fault,
    t_end: Time,
) -> (Result<u64, SchedError>, Time) {
    let grant = Time::from_us(100);
    let mut cfg = cfg.clone();
    cfg.quantum = grant;
    cfg.verify = verify;
    let mut engine = RenodeEngine::start(&cfg).expect("start renode");
    engine.inject_fault(fault);
    let mut sched = Scheduler::new(SchedulerConfig {
        max_quantum: grant,
        loop_quantum: None,
        lookahead: false,
        log_grants: false,
        event_sampling: None,
    })
    .unwrap();
    let id = sched.add_engine(Box::new(engine)).unwrap();
    sched.bind(id, "PA5", "PA5", Direction::Output).unwrap();
    let mut sink = EventSink::new(Vec::new());
    let r = sched.run(t_end, &mut sink, &mut []).map(|s| s.grants);
    (r, sched.now())
}

/// ADR-003 D1.2 gate: an injected time mismatch (one RUN_FOR 1 ns too long)
/// and an injected GPIO mismatch (a lost edge event) are caught by every
/// verify mode: `Every` at the faulty grant, `Edges` at the next grant with
/// an edge or at the end of the run, `Final` at the end of the run. Without
/// a fault, all three modes complete with identical event streams.
#[test]
fn verify_modes_catch_injected_mismatches() {
    let Some(cfg) = setup() else { return };
    let t_end = Time::from_ms(70);
    let time_fault = Fault::RunForExtraNs {
        at: Time::from_ms(20),
        ns: 1,
    };
    let gpio_fault = Fault::DropGpioEvent { at_ns: 40_000_000 };
    let cases = [
        (Verify::Every, time_fault),
        (Verify::Edges, time_fault),
        (Verify::Final, time_fault),
        (Verify::Every, gpio_fault),
        (Verify::Edges, gpio_fault),
        (Verify::Final, gpio_fault),
    ];
    let handles: Vec<_> = cases
        .iter()
        .map(|&(v, f)| {
            let cfg = cfg.clone();
            std::thread::spawn(move || run_faulty(&cfg, v, f, t_end))
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    for ((v, f), (r, now)) in cases.iter().zip(&results) {
        let err = match r {
            Ok(g) => panic!("{v:?} / {f:?}: run completed ({g} grants), fault not caught"),
            Err(e) => e.to_string(),
        };
        eprintln!("{v:?} / {f:?}: caught after boundary {now}: {err:.160}");
        let at_end = *now == t_end;
        match (v, f) {
            // Caught at the faulty grant itself: the advance to 20 ms.
            (Verify::Every, Fault::RunForExtraNs { .. }) => {
                assert_eq!(*now, Time::from_ms(20) - Time::from_us(100));
                assert!(err.contains("verify: Renode virtual time"), "{err}");
            }
            // Caught at the grant that carries the 50 ms edge.
            (Verify::Edges, Fault::RunForExtraNs { .. }) => {
                assert!(*now >= Time::from_ms(49) && *now < Time::from_ms(51));
                assert!(err.contains("verify: Renode virtual time"), "{err}");
            }
            // The lost 50 ms edge is caught at its own grant.
            (Verify::Every, Fault::DropGpioEvent { .. }) => {
                assert!(*now >= Time::from_ms(49) && *now < Time::from_ms(51));
                assert!(err.contains("verify: PA5: Renode reports true"), "{err}");
            }
            // No event reached the engine, so only the end-of-run check sees it.
            (Verify::Edges, Fault::DropGpioEvent { .. }) => {
                assert!(at_end, "{now}");
                assert!(err.contains("verify: PA5: Renode reports true"), "{err}");
                assert!(err.contains("Done"), "{err}");
            }
            (Verify::Final, Fault::RunForExtraNs { .. }) => {
                assert!(at_end, "{now}");
                assert!(err.contains("verify: Renode virtual time"), "{err}");
            }
            (Verify::Final, Fault::DropGpioEvent { .. }) => {
                assert!(at_end, "{now}");
                assert!(err.contains("verify: PA5: Renode reports true"), "{err}");
            }
        }
    }
}

/// Every verify mode gives the same event stream (the checks are pure
/// reads), and `Edges`/`Final` run far fewer checks than `Every`.
#[test]
fn verify_modes_do_not_change_results() {
    let Some(cfg) = setup() else { return };
    let grant = Time::from_us(10);
    let t_end = Time::from_ms(120);
    let handles: Vec<_> = [Verify::Every, Verify::Edges, Verify::Final]
        .into_iter()
        .map(|v| {
            let mut cfg = cfg.clone();
            cfg.verify = v;
            std::thread::spawn(move || run_blinky(&cfg, L, grant, t_end, None))
        })
        .collect();
    let runs: Vec<Run> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    for (r, v) in runs.iter().zip(["every", "edges", "final"]) {
        eprintln!("verify {v}: {} grants, wall {:.2} s", r.grants, r.wall_s);
    }
    assert_eq!(runs[0].events, runs[1].events, "every vs edges");
    assert_eq!(runs[0].events, runs[2].events, "every vs final");
    let got: Vec<u64> = runs[0].edges.iter().map(|e| e.0.as_fs()).collect();
    assert_eq!(got, GOLDEN_10US[..2]);
}

/// The SYSTEM_BUS helpers (ADR-003 D1.3, for Part B) against real Renode:
/// read the SysTick reload value the firmware programmed, and write then
/// read back a word of SRAM.
#[test]
fn sysbus_helpers_read_and_write() {
    let Some(mut cfg) = setup() else { return };
    cfg.quantum = Time::from_ms(1);
    let mut e = RenodeEngine::start(&cfg).expect("start renode");
    e.next_safe_time(Time::ZERO);
    e.advance_to(Time::from_ms(1)).unwrap();
    // SYST_RVR = 72 MHz / 1000 * 50 - 1.
    assert_eq!(e.sysbus_read_u32s(0xE000_E014, 1).unwrap(), vec![3_599_999]);
    e.sysbus_write_u32(0x2000_1000, 0xC0FF_EE00).unwrap();
    e.sysbus_write_u32(0x2000_1004, 7).unwrap();
    assert_eq!(
        e.sysbus_read_u32s(0x2000_1000, 2).unwrap(),
        vec![0xC0FF_EE00, 7]
    );
}

/// ADR-003 step 2 gate (fidelity): a busy-loop firmware that never sleeps
/// has no timer-driven syncs, so Renode stamps its writes with the lattice
/// point that opens the slice: every edge is a lattice point + 1 fs. A
/// 100 us lattice gives coarser stamps than a 10 us one (`L` is the fidelity
/// knob), and on one lattice a coarser partition changes nothing.
#[test]
fn busy_loop_edges_land_on_the_lattice() {
    let Some(cfg) = setup_fw("busyloop") else {
        return;
    };
    let t_end = Time::from_ms(30);
    let cases = [
        (Time::from_us(10), Time::from_us(10)),
        (Time::from_us(10), Time::from_us(50)),
        (Time::from_us(100), Time::from_us(100)),
    ];
    let handles: Vec<_> = cases
        .iter()
        .map(|&(l, g)| {
            let cfg = cfg.clone();
            std::thread::spawn(move || run_blinky(&cfg, l, g, t_end, None))
        })
        .collect();
    let runs: Vec<Run> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    for (r, (l, g)) in runs.iter().zip(cases) {
        eprintln!(
            "busy loop, lattice {l}, grants {g}: {} edges {:?}",
            r.edges.len(),
            r.edges.iter().take(6).collect::<Vec<_>>()
        );
        assert!(r.edges.len() >= 4, "too few busy-loop edges: {:?}", r.edges);
        for (t, _) in &r.edges {
            assert_eq!(
                t.as_fs() % l.as_fs(),
                1,
                "edge {t} not on lattice {l} + 1 fs"
            );
        }
    }
    assert_eq!(runs[0].edges, runs[1].edges, "partition changed the edges");
    // The 100 us lattice: coarser, at most one coarse cell earlier.
    let (fine, coarse) = (&runs[0].edges, &runs[2].edges);
    assert!(
        fine.iter()
            .any(|(t, _)| t.as_fs() % Time::from_us(100).as_fs() != 1),
        "the 10 us lattice should resolve edges between 100 us points"
    );
    assert!(fine.len().abs_diff(coarse.len()) <= 1);
    for ((tf, lf), (tc, lc)) in fine.iter().zip(coarse) {
        assert_eq!(lf, lc);
        // Measured: the firmware's timing does not depend on L here, and the
        // coarse stamp is the 100 us point at or before the fine one.
        assert!(
            tc <= tf && *tf - *tc < Time::from_us(100),
            "10 us edge {tf} vs 100 us edge {tc}"
        );
    }
}

/// Drives `OUT` through the transitions `edges` (time, level, alternating),
/// any number per grant, reported as a trace (ADR-003 D3); t_safe is
/// unbounded.
struct Driver {
    edges: Vec<(Time, Level)>,
    now: Time,
    level: Level,
    edge_time: Time,
    trace: Vec<Transition>,
}

impl Engine for Driver {
    fn name(&self) -> &str {
        "drv"
    }
    fn next_safe_time(&mut self, _now: Time) -> Time {
        Time::MAX
    }
    fn advance_to(&mut self, t: Time) -> Result<(), EngineError> {
        self.trace.clear();
        for &(at, level) in &self.edges {
            if self.now < at && at <= t {
                self.level = level;
                self.edge_time = at;
                self.trace.push(Transition {
                    t: at,
                    pin: "OUT".into(),
                    signal: PinSignal::digital(level, at),
                });
            }
        }
        self.now = t;
        Ok(())
    }
    fn read_outputs(&mut self) -> Result<PinSignals, EngineError> {
        let mut m = PinSignals::new();
        m.insert("OUT".into(), PinSignal::digital(self.level, self.edge_time));
        Ok(m)
    }
    fn read_output_trace(&mut self) -> Result<Option<Vec<Transition>>, EngineError> {
        Ok(Some(self.trace.clone()))
    }
    fn apply_inputs(&mut self, _: &PinSignals) -> Result<(), EngineError> {
        Ok(())
    }
}

/// PA6 edges of the copy firmware when a driver toggles PB0 at `edges`, in
/// grants of `grant`. The driver advances before Renode (the plan order,
/// ADR-003 D3), so Renode gets each edge before it advances, with its true
/// edge_time, whether the driver is registered first or not.
fn run_copy(
    cfg: &RenodeConfig,
    grant: Time,
    driver_first: bool,
    edges: &[(Time, Level)],
) -> Vec<(Time, u8)> {
    let mut cfg = cfg.clone();
    cfg.verify = Verify::Every;
    cfg.pins = vec![
        PinSpec::stm32("PB0", PinDirection::Input).unwrap(),
        PinSpec::stm32("PA6", PinDirection::Output).unwrap(),
    ];
    let renode = Box::new(RenodeEngine::start(&cfg).expect("start renode"));
    let mut sched = Scheduler::new(SchedulerConfig {
        max_quantum: grant,
        loop_quantum: None,
        lookahead: false,
        log_grants: false,
        event_sampling: None,
    })
    .unwrap();
    let driver = Box::new(Driver {
        edges: edges.to_vec(),
        now: Time::ZERO,
        level: Level::Low,
        edge_time: Time::ZERO,
        trace: Vec::new(),
    });
    let (d, r) = if driver_first {
        let d = sched.add_engine(driver).unwrap();
        (d, sched.add_engine(renode).unwrap())
    } else {
        let r = sched.add_engine(renode).unwrap();
        (sched.add_engine(driver).unwrap(), r)
    };
    sched.bind(d, "OUT", "PB0", Direction::Output).unwrap();
    sched.bind(r, "PB0", "PB0", Direction::Input).unwrap();
    sched.bind(r, "PA6", "PA6", Direction::Output).unwrap();
    let mut sink = EventSink::new(Vec::new());
    sched
        .run(Time::from_ms(3), &mut sink, &mut [])
        .expect("run");
    String::from_utf8(sink.into_inner())
        .unwrap()
        .lines()
        .filter_map(|l| match Event::from_line(l).unwrap() {
            Event::PinEdge {
                t_fs, net, level, ..
            } if net == "PA6" => Some((t_fs, level)),
            _ => None,
        })
        .collect()
}

/// ADR-003 step 2 gate (latent bug of Context fact 6): an input transition
/// with an `edge_time` mid-grant is applied at `ceil_L(t)`, never earlier.
/// The copy firmware polls PB0 and writes PA6 in the slice after the input
/// appears, so PA6 follows at exactly `ceil_L(t) + 1 fs`, both in 1 ms
/// grants (Renode splits its advance at `ceil_L(t)`) and in one-cell grants.
/// Step 3 (D3): the same with Renode registered before its driver, and in
/// one 3 ms grant holding all three inputs.
#[test]
fn inputs_take_effect_at_ceil_lattice_never_early() {
    let Some(mut cfg) = setup_fw("copy") else {
        return;
    };
    cfg.quantum = L;
    let edges = [
        (Time::from_ps(123_456_789), Level::High), // mid-cell: ceil = 130 us
        (Time::from_us(1200), Level::Low),         // on a lattice point
        (Time::from_ms(2) + Time::from_fs(1), Level::High), // 1 fs past: ceil = 2.01 ms
    ];
    let want: Vec<(Time, u8)> = [
        (Time::from_us(130), 1),
        (Time::from_us(1200), 0),
        (Time::from_us(2010), 1),
    ]
    .iter()
    .map(|&(t, l)| (t + Time::from_fs(1), l))
    .collect();
    let cases = [
        (Time::from_ms(1), true),
        (L, true),
        (Time::from_ms(1), false),
        (Time::from_ms(3), false),
    ];
    let handles: Vec<_> = cases
        .into_iter()
        .map(|(g, first)| {
            let cfg = cfg.clone();
            std::thread::spawn(move || run_copy(&cfg, g, first, &edges))
        })
        .collect();
    let runs: Vec<Vec<(Time, u8)>> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    for ((g, first), run) in cases.iter().zip(&runs) {
        eprintln!("PA6 edges, {g} grants, driver registered first: {first}: {run:?}");
    }
    for run in &runs {
        assert_eq!(run, &want);
        for ((t_out, _), (t_in, _)) in run.iter().zip(edges) {
            assert!(*t_out > t_in, "PA6 at {t_out} before the input at {t_in}");
        }
    }
}
