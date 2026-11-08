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

use bb_cpu_renode::{find_renode, Fault, RenodeConfig, RenodeEngine, Verify};
use bbcore::{
    CsvWriter, Direction, Engine, Event, EventSink, Level, PinSignal, Probe, SchedError, Scheduler,
    SchedulerConfig, Time,
};

/// SysTick runs at 72 MHz in Renode's stm32f103.repl: one tick is 13.89 ns.
const SYSTICK_TICK: Time = Time::from_fs(13_888_889);
const TOGGLE: Time = Time::from_ms(50);

fn firmware() -> Option<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/spike/firmware");
    let elf = dir.join("build/blinky.elf");
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
    let Some(renode) = find_renode() else {
        skip("renode not installed (run `make setup`, or set BB_RENODE)");
        return None;
    };
    let elf = firmware()?;
    Some(RenodeConfig::stm32f103(renode, elf))
}

struct Run {
    edges: Vec<(Time, u8)>,
    events: Vec<u8>,
    csv: String,
    grants: u64,
    wall_s: f64,
}

/// Run the blinky under the scheduler to `t_end` with grants of `grant`.
/// With `csv_period`, a sampled `CsvWriter` records PA5 (it must not change
/// the grant partition).
fn run_blinky(cfg: &RenodeConfig, grant: Time, t_end: Time, csv_period: Option<Time>) -> Run {
    let mut cfg = cfg.clone();
    cfg.quantum = grant;
    let engine = RenodeEngine::start(&cfg).expect("start renode");
    let mut sched = Scheduler::new(SchedulerConfig {
        max_quantum: grant,
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

/// Golden edge list for the Phase 0 partition (10 us grants, 500 ms), Renode
/// 1.17.0. The first five SysTick expiries are deferred to a boundary and
/// relabelled +1 fs; the rest are reported mid-grant.
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
            std::thread::spawn(move || run_blinky(&cfg, grant, t_end, None))
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

/// A 70 us partition: no multiple of 50 ms (k < 7) is a grant boundary, so
/// SysTick expiries fall mid-grant and the normal in-grant edge path is
/// exercised. Edges stay within the measured tolerance of the ideal times and
/// of the 10 us golden list (the partition dependence of Amendment 3).
#[test]
fn blinky_500ms_in_70us_grants_mid_grant_edges() {
    let Some(cfg) = setup() else { return };
    let grant = Time::from_us(70);
    let t_end = Time::from_ms(500);
    let r = run_blinky(&cfg, grant, t_end, None);
    eprintln!(
        "500ms sim in {} grants of {grant}: wall {:.2} s, edges {:?}",
        r.grants, r.wall_s, r.edges
    );
    check_blinky_edges(&r.edges, t_end);
    let mid = mid_grant_edges(&r.edges, grant);
    assert!(
        mid >= 8,
        "only {mid} edges reported mid-grant: {:?}",
        r.edges
    );
    for ((t, _), g) in r.edges.iter().zip(GOLDEN_10US) {
        let d = t.as_fs().abs_diff(g);
        assert!(
            d <= Time::from_ns(100).as_fs(),
            "edge at {t} is {d} fs from the 10 us partition's {g}"
        );
    }
}

/// Drive the engine directly with uneven grants, including ones that are not
/// whole nanoseconds: Renode's virtual time must equal floor(grant) after
/// every advance, and outputs must never claim an edge after the boundary.
#[test]
fn virtual_time_tracks_every_grant() {
    let Some(cfg) = setup() else { return };
    let mut e = RenodeEngine::start(&cfg).expect("start renode");
    let mut now = Time::ZERO;
    let steps = [
        Time::from_us(10),
        Time::from_ns(1),
        Time::from_fs(2_500_000), // 2.5 ns
        Time::from_fs(1_500_000), // 1.5 ns: crosses a ns boundary
        Time::from_us(7),
        Time::from_fs(333_333_333), // 333.333333 ns
    ];
    for k in 0..200 {
        let t_safe = e.next_safe_time(now);
        assert!(t_safe > now);
        let g = (now + steps[k % steps.len()]).min(t_safe);
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
    // Asking for more than t_safe is refused, not clamped.
    let t_safe = e.next_safe_time(now);
    assert!(e.advance_to(t_safe + Time::from_ns(1)).is_err());
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
            std::thread::spawn(move || run_blinky(&cfg, grant, t_end, Some(p)))
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
            std::thread::spawn(move || run_blinky(&cfg, grant, t_end, None))
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
    let t_safe = e.next_safe_time(Time::ZERO);
    e.advance_to(t_safe).unwrap();
    // SYST_RVR = 72 MHz / 1000 * 50 - 1.
    assert_eq!(e.sysbus_read_u32s(0xE000_E014, 1).unwrap(), vec![3_599_999]);
    e.sysbus_write_u32(0x2000_1000, 0xC0FF_EE00).unwrap();
    e.sysbus_write_u32(0x2000_1004, 7).unwrap();
    assert_eq!(
        e.sysbus_read_u32s(0x2000_1000, 2).unwrap(),
        vec![0xC0FF_EE00, 7]
    );
}
