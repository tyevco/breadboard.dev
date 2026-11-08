//! Renode engine against the real STM32F103 blinky firmware.
//!
//! Needs Renode (`make setup`) and the firmware (`make firmware`; built here
//! on demand). Without Renode the tests print a SKIP line and pass.
//!
//! Run with `cargo test -p bb-cpu-renode -- --nocapture` to see wall times.

use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

use bb_cpu_renode::{find_renode, RenodeConfig, RenodeEngine};
use bbcore::{
    Direction, Engine, Event, EventSink, Level, PinSignal, Scheduler, SchedulerConfig, Time,
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
            eprintln!("SKIP: arm-none-eabi-gcc not installed and {} missing (run `make setup && make firmware`)", elf.display());
            return None;
        }
    }
    Some(elf)
}

fn setup() -> Option<RenodeConfig> {
    let Some(renode) = find_renode() else {
        eprintln!("SKIP: renode not installed (run `make setup`, or set BB_RENODE)");
        return None;
    };
    let elf = firmware()?;
    Some(RenodeConfig::stm32f103(renode, elf))
}

struct Run {
    edges: Vec<(Time, u8)>,
    events: Vec<u8>,
    grants: u64,
    wall_s: f64,
}

/// Run the blinky under the scheduler to `t_end` with grants of `grant`.
fn run_blinky(cfg: &RenodeConfig, grant: Time, t_end: Time) -> Run {
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
    let t0 = Instant::now();
    let summary = sched.run(t_end, &mut sink, &mut []).expect("run");
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
        grants: summary.grants,
        wall_s,
    }
}

fn check_blinky_edges(edges: &[(Time, u8)], t_end: Time) {
    // The firmware starts SysTick within the first microsecond; edge k is at
    // k * 50 ms (+ that start-up time), level alternating from high.
    let expected = (t_end.as_fs() / TOGGLE.as_fs()) as usize;
    assert!(
        edges.len() == expected || edges.len() + 1 == expected,
        "expected {expected} edges (the last may land exactly on t_end), got {edges:?}"
    );
    assert!(edges.len() >= 4, "too few edges: {edges:?}");
    for (k, (t, level)) in edges.iter().enumerate() {
        let ideal = Time::from_fs(TOGGLE.as_fs() * (k as u64 + 1));
        let err = t.as_fs().abs_diff(ideal.as_fs());
        assert!(
            err <= SYSTICK_TICK.as_fs(),
            "edge {k} at {t}, expected {ideal} +/- 1 SysTick tick"
        );
        assert_eq!(*level, if k % 2 == 0 { 1 } else { 0 }, "edge {k} level");
    }
    for w in edges.windows(2) {
        let dt = w[1].0 - w[0].0;
        assert!(
            dt.as_fs().abs_diff(TOGGLE.as_fs()) <= SYSTICK_TICK.as_fs(),
            "edge spacing {dt}, expected 50ms +/- 1 SysTick tick"
        );
    }
}

/// Two independent 250 ms runs in 10 us grants: edges every 50 ms within one
/// SysTick tick, identical edge lists and identical event streams.
#[test]
fn blinky_250ms_in_10us_grants_is_exact_and_deterministic() {
    let Some(cfg) = setup() else { return };
    let grant = Time::from_us(10);
    let t_end = Time::from_ms(250);
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let cfg = cfg.clone();
            std::thread::spawn(move || run_blinky(&cfg, grant, t_end))
        })
        .collect();
    let runs: Vec<Run> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    for (i, r) in runs.iter().enumerate() {
        eprintln!(
            "run {i}: 250ms sim in {} grants of {grant}: wall {:.2} s, edges {:?}",
            r.grants, r.wall_s, r.edges
        );
    }
    assert_eq!(runs[0].grants, 25_000);
    check_blinky_edges(&runs[0].edges, t_end);
    assert_eq!(
        runs[0].edges, runs[1].edges,
        "edge lists differ between runs"
    );
    assert_eq!(runs[0].events, runs[1].events, "event streams differ");
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

/// Coarser grants (1 ms) give edges within one tick of the same times.
/// Every SysTick expiry lands on a grant boundary here, so this also covers
/// the boundary-edge path. Records wall time.
#[test]
fn blinky_250ms_in_1ms_grants() {
    let Some(cfg) = setup() else { return };
    let t_end = Time::from_ms(250);
    let r = run_blinky(&cfg, Time::from_ms(1), t_end);
    eprintln!(
        "250ms sim in {} grants of 1ms: wall {:.2} s, edges {:?}",
        r.grants, r.wall_s, r.edges
    );
    check_blinky_edges(&r.edges, t_end);
}
