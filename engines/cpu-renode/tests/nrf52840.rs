//! Phase 1 M2 (ADR-008 D1-D4, D6): Breadboard's nRF52840 platform
//! (`nrf52840-bb@4`) under the Renode engine, with the bare-metal gate
//! firmware in `tests/firmware/nrf52840/` and the prebuilt Zephyr gate image
//! in `tests/firmware/nrf52840/zephyr/`. `make nrf-gates` runs these
//! together with the board-level gate (`examples/nrf-feedback`).
//!
//! Like `lookahead.rs`, the tests drive one Renode engine directly with the
//! D5 grant rule: at every boundary ask `quiet_until` and grant
//! `floor_L(min(quiet_until, now + 10 ms, t_end))` when that is more than
//! one cell, else one cell.
//!
//! - `nrf-blinky`: the edge list is identical at one-cell grants, 10 ms
//!   grants, with the plugin armed and with lookahead on; the lookahead
//!   self-test status is exactly 0x3 (armed, self-test passed).
//! - `nrf-ppi`: TIMER1 -> PPI -> GPIOTE OUT; stock PPI's deferred task is
//!   deterministic, lags the compare by at most one cell, and lookahead is
//!   equivalent to the one-cell reference.
//! - `nrf-busywait` (D1): the virtual time of a 10 ms nrfx busy-wait (about
//!   6.67 ms at 64 MIPS) and of a 10 ms RTC sleep.
//! - `nrf-timer-sleep` (D6, Amendment 1): a TIMER compare followed by a long
//!   sleep gives a promise past `Time`'s range, clamped, with grants capped
//!   at the maximum quantum and the reference's edges.
//! - `nrf-pincnf` (D4): the end-of-run `PIN_CNF` audit's findings.
//! - `nrf-bor` (Phase 2 P5, ADR-009 D7): a brown-out's hold and release
//!   under External Control (`#bor`), the `reset` records, RESETREAS and
//!   GPREGRET 0, RAM kept, every output and `#oe` low while held.
//! - `nrf-zephyr`: an unmodified Zephyr image, with and without lookahead,
//!   and Zephyr's `k_busy_wait(10000)` and `k_msleep(10)`.
//!
//! Needs Renode and arm-none-eabi-gcc like `blinky.rs` (`BB_REQUIRE_RENODE`).

use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

use bb_cpu_renode::{
    find_renode, Fault, LookaheadStatus, NrfDrive, PinCnfAudit, PinCnfExpect, PinDirection,
    PinSpec, Platform, RenodeConfig, RenodeEngine,
};
use bbcore::{Engine, EngineError, LogLevel, PinSignal, Time};

/// The default grant lattice (`--renode-quantum`).
const L: Time = Time::from_us(10);
/// Grant cap of the lookahead and long-grant runs (`--max-quantum` default).
const MAX_GRANT: Time = Time::from_ms(10);

static BUILD: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn skip(msg: &str) {
    let required = std::env::var("BB_REQUIRE_RENODE").is_ok_and(|v| v == "1");
    assert!(!required, "BB_REQUIRE_RENODE=1 but {msg}");
    eprintln!("SKIP: {msg}");
}

fn fw_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/firmware/nrf52840")
}

/// `tests/firmware/nrf52840/build/<name>.elf`, built on demand.
fn firmware(name: &str) -> Option<PathBuf> {
    let _g = BUILD.lock().unwrap_or_else(|e| e.into_inner());
    let dir = fw_dir();
    let elf = dir.join(format!("build/{name}.elf"));
    let ok = Command::new("make")
        .arg("-s")
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
        skip("arm-none-eabi-gcc not installed (run `make setup`)");
        return None;
    }
    Some(elf)
}

fn config_elf(elf: Option<PathBuf>, outputs: &[&str]) -> Option<RenodeConfig> {
    let Some(renode) = find_renode() else {
        skip("renode not installed (run `make setup`, or set BB_RENODE)");
        return None;
    };
    let mut cfg = RenodeConfig::nrf52840(renode, elf?);
    cfg.name = "U1".into();
    cfg.pins = outputs
        .iter()
        .map(|p| PinSpec::nrf(p, PinDirection::Output).unwrap())
        .collect();
    cfg.quantum = L;
    assert_eq!(cfg.platform, Platform::Nrf52840Bb);
    Some(cfg)
}

fn config(name: &str, outputs: &[&str]) -> Option<RenodeConfig> {
    config_elf(firmware(name), outputs)
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Mode {
    /// No plugin, one-cell grants: the reference.
    Reference,
    /// No plugin, 10 ms grants (an acyclic board's partition).
    Long,
    /// Plugin loaded and armed, lookahead off, one-cell grants.
    ArmedOff,
    /// Plugin loaded, lookahead on, D5 grants.
    On,
}

fn with_mode(cfg: &RenodeConfig, mode: Mode) -> RenodeConfig {
    let mut c = cfg.clone();
    c.lookahead.plugin = matches!(mode, Mode::ArmedOff | Mode::On);
    c.lookahead.enabled = mode == Mode::On;
    c
}

#[derive(Debug, Default)]
struct Drive {
    /// Every output transition: (t, pin, level).
    edges: Vec<(Time, String, bool)>,
    /// Every grant `(from, to)`.
    grants: Vec<(Time, Time)>,
    /// Every promise `(from, until)` given.
    promises: Vec<(Time, Time)>,
    /// The error that ended the run, with the boundary it started from.
    err: Option<(Time, EngineError)>,
    startup: Option<LookaheadStatus>,
    stats: bb_cpu_renode::LookaheadStats,
    wall_s: f64,
    findings: Option<Vec<bb_cpu_renode::PinCnfFinding>>,
    /// Every log record drained after an advance: (t, level, msg).
    logs: Vec<(Time, LogLevel, String)>,
    /// Whether the engine's Renode was leased from a pool, and its pid.
    leased: bool,
    pid: u32,
}

fn floor_l(t: Time) -> Time {
    Time::from_fs(t.as_fs() / L.as_fs() * L.as_fs())
}

fn ceil_l(t: Time) -> Time {
    Time::from_fs(t.as_fs().div_ceil(L.as_fs()) * L.as_fs())
}

/// Run `cfg` in `mode` to `t_end`.
fn drive(cfg: &RenodeConfig, mode: Mode, t_end: Time, fault: Option<Fault>) -> Drive {
    let cfg = with_mode(cfg, mode);
    let mut e = RenodeEngine::start(&cfg).expect("start renode");
    if let Some(f) = fault {
        e.inject_fault(f);
    }
    let findings = e.pin_cnf_findings();
    let mut d = Drive {
        startup: e.lookahead_startup(),
        ..Drive::default()
    };
    let t0 = Instant::now();
    let mut now = Time::ZERO;
    while now < t_end {
        assert_eq!(e.next_safe_time(now), Time::MAX);
        let q = e.quiet_until(now);
        assert!(q >= now);
        if q > now {
            d.promises.push((now, q));
        }
        let g = match mode {
            Mode::Long => floor_l((now + MAX_GRANT).min(t_end)),
            _ => floor_l(q.min(now + MAX_GRANT).min(t_end)).max(now + L),
        };
        if let Err(err) = e.advance_to(g) {
            d.err = Some((now, err));
            break;
        }
        for tr in e
            .read_output_trace()
            .unwrap()
            .expect("renode reports a trace")
        {
            let PinSignal::Digital { level, .. } = tr.signal else {
                panic!("analog output from renode")
            };
            // The kernel's causality rule: every change inside its grant.
            assert!(
                tr.t > now && tr.t <= g,
                "{} at {} outside the grant ({now}, {g}]",
                tr.pin,
                tr.t
            );
            d.edges.push((tr.t, tr.pin, level == bbcore::Level::High));
        }
        for r in e.drain_logs() {
            assert!(r.t > now && r.t <= g, "log at {} outside ({now}, {g}]", r.t);
            d.logs.push((r.t, r.level, r.msg));
        }
        d.grants.push((now, g));
        now = g;
    }
    if d.err.is_none() {
        e.end_of_run().expect("end-of-run verify");
    }
    d.wall_s = t0.elapsed().as_secs_f64();
    d.stats = e.lookahead_stats();
    d.findings = findings.lock().unwrap().clone();
    d
}

/// No reference edge lies strictly inside a promised window.
fn check_honest(name: &str, reference: &Drive, d: &Drive) {
    for (from, until) in &d.promises {
        if let Some(e) = reference
            .edges
            .iter()
            .find(|(t, _, _)| t > from && t < until)
        {
            panic!("{name}: promise ({from}, {until}) contains reference edge {e:?}");
        }
    }
}

/// The four runs of a gate firmware (reference, 10 ms grants, plugin armed,
/// lookahead on): the edges must be identical in all of them, the armed
/// plugin inert and its self-test status exactly 0x3, every promise honest.
/// Returns (reference, lookahead run).
fn all_modes(name: &str, cfg: &RenodeConfig, t_end: Time) -> (Drive, Drive) {
    let reference = drive(cfg, Mode::Reference, t_end, None);
    assert!(
        reference.err.is_none(),
        "{name}: reference: {:?}",
        reference.err
    );
    let long = drive(cfg, Mode::Long, t_end, None);
    assert!(long.err.is_none(), "{name}: 10 ms grants: {:?}", long.err);
    assert_eq!(
        long.edges, reference.edges,
        "{name}: 10 ms grants changed the edges"
    );
    let armed = drive(cfg, Mode::ArmedOff, t_end, None);
    assert!(armed.err.is_none(), "{name}: armed: {:?}", armed.err);
    assert_eq!(
        armed.edges, reference.edges,
        "{name}: the armed plugin changed the edges"
    );
    assert_eq!(armed.grants, reference.grants);
    assert!(armed.promises.is_empty() && armed.stats.reads == 0);
    let st = armed.startup.expect("plugin loaded");
    assert_eq!(
        st.status,
        LookaheadStatus::ARMED | LookaheadStatus::SELF_TEST_OK,
        "{name}: lookahead self-test status {:#x} (reasons {:#x})",
        st.status,
        st.self_test_failures()
    );
    let on = drive(cfg, Mode::On, t_end, None);
    assert!(on.err.is_none(), "{name}: lookahead on: {:?}", on.err);
    assert_eq!(
        on.edges, reference.edges,
        "{name}: lookahead changed the edges"
    );
    check_honest(name, &reference, &on);
    eprintln!(
        "{name}: {} edges; reference {} grants ({:.2} s), 10 ms grants {} ({:.2} s), armed {} \
         ({:.2} s), lookahead {} grants ({:.2} s, {} promises, {} reads, {} audits); startup \
         status {:#x}",
        reference.edges.len(),
        reference.grants.len(),
        reference.wall_s,
        long.grants.len(),
        long.wall_s,
        armed.grants.len(),
        armed.wall_s,
        on.grants.len(),
        on.wall_s,
        on.stats.promises,
        on.stats.reads,
        on.stats.audits,
        st.status
    );
    (reference, on)
}

fn edges_of<'a>(d: &'a Drive, pin: &str) -> Vec<&'a (Time, String, bool)> {
    d.edges.iter().filter(|e| e.1 == pin).collect()
}

/// `nrf-blinky` (ADR-008 D3): RTC0 toggles P0.13 every 1638 LFCLK ticks.
#[test]
fn nrf_blinky_edges_identical_in_every_mode() {
    let Some(cfg) = config("nrf_blinky", &["P0.13"]) else {
        return;
    };
    let t_end = Time::from_ms(300);
    let (reference, on) = all_modes("nrf-blinky", &cfg, t_end);
    let e = edges_of(&reference, "P0.13");
    assert_eq!(e.len(), 6, "{e:?}");
    // 1638 ticks of 32.768 kHz = 49.987793 ms per toggle; each edge lies
    // within one lattice cell (plus the lattice rule's 1 fs) of k times it.
    for (k, (t, _, level)) in e.iter().enumerate() {
        let ideal = Time::from_fs(1638 * (k as u64 + 1) * 1_000_000_000_000_000 / 32768);
        assert!(
            *t >= ideal && *t <= ceil_l(ideal) + L,
            "edge {k} at {t}, RTC compare at {ideal}"
        );
        assert_eq!(*level, k % 2 == 0);
    }
    // The CPU sleeps between compares: far fewer grants with lookahead.
    assert!(
        on.grants.len() * 20 < reference.grants.len(),
        "{}",
        on.grants.len()
    );
}

/// `nrf-ppi` (ADR-008 D3, Context 2): stock PPI defers the GPIOTE task to
/// the next synced state. Deterministic, at most one cell late, and the
/// lookahead run is equivalent.
#[test]
fn nrf_ppi_hop_is_deterministic_and_equivalent() {
    let Some(cfg) = config("nrf_ppi", &["P0.14", "P0.16"]) else {
        return;
    };
    let (reference, _on) = all_modes("nrf-ppi", &cfg, Time::from_ms(20));
    let ppi = edges_of(&reference, "P0.14");
    let cpu = edges_of(&reference, "P0.16");
    assert!(
        ppi.len() >= 19 && cpu.len() >= 19,
        "ppi {ppi:?}\ncpu {cpu:?}"
    );
    for (k, (t, _, _)) in ppi.iter().enumerate() {
        // TIMER1 COMPARE0 at (k + 1) ms; the task runs at a synced state
        // after it, at most one lattice cell (plus 1 fs) later.
        let compare = Time::from_ms(k as u64 + 1);
        assert!(
            *t >= compare && *t <= compare + L + Time::from_fs(1),
            "PPI edge {k} at {t}, compare at {compare}"
        );
    }
    eprintln!(
        "nrf-ppi: first PPI edges {:?}; first ISR edges {:?}",
        &ppi[..3],
        &cpu[..3]
    );
}

/// `nrf-busywait` (ADR-008 D1): a 10 ms nrfx busy-wait lasts about 2/3 of
/// its requested time at 64 MIPS; a 10 ms RTC sleep is exact to one tick.
#[test]
fn nrf_busywait_virtual_times() {
    let Some(cfg) = config("nrf_busywait", &["P0.13", "P0.14"]) else {
        return;
    };
    let (reference, _on) = all_modes("nrf-busywait", &cfg, Time::from_ms(40));
    let width = |pin: &str| {
        let e = edges_of(&reference, pin);
        assert_eq!(e.len(), 2, "{pin}: {e:?}");
        assert!(e[0].2 && !e[1].2);
        e[1].0 - e[0].0
    };
    let busy = width("P0.13");
    let sleep = width("P0.14");
    let ms = |t: Time| t.as_fs() as f64 / 1e12;
    eprintln!(
        "nrf-busywait: nrfx_coredep_delay_us(10000) took {:.6} ms of virtual time ({:.4} of \
         requested; expected 2*64/(3*64) = 0.6667); the 328-tick RTC sleep took {:.6} ms \
         (328/32768 s = 10.009766 ms)",
        ms(busy),
        ms(busy) / 10.0,
        ms(sleep)
    );
    // 640 000 loop iterations of 2 instructions = 1 280 000 instructions at
    // 64 MIPS = 6.667 ms, plus a few call instructions; the edges are on
    // the 10 us lattice, so allow one cell either side.
    assert!(
        busy >= Time::from_us(6_657) && busy <= Time::from_us(6_677),
        "busy-wait {busy}"
    );
    let rtc = Time::from_fs(328 * 1_000_000_000_000_000 / 32768);
    let tick = Time::from_fs(1_000_000_000_000_000 / 32768);
    assert!(
        sleep + tick + L >= rtc && sleep <= rtc + tick + L,
        "RTC sleep {sleep}, expected {rtc} within one tick"
    );
}

/// ADR-008 D6 (Amendment 1, moved to M2): after TIMER0's compare, the
/// 64-bit, never-wrapping timer's next limit is about 2^64 ticks away. The
/// promise is clamped to `MAX_PROMISE_NS` (it used to panic with "Time
/// overflow"), every grant stays capped at the maximum quantum, and the
/// edges are the reference's.
#[test]
fn nrf_timer_compare_then_long_sleep_promise_is_clamped() {
    let Some(cfg) = config("nrf_timer_sleep", &["P0.13"]) else {
        return;
    };
    let t_end = Time::from_ms(120);
    let (reference, on) = all_modes("nrf-timer-sleep", &cfg, t_end);
    let e = edges_of(&reference, "P0.13");
    assert_eq!(e.len(), 1, "{e:?}");
    assert!(e[0].0 >= Time::from_ms(1) && e[0].0 <= Time::from_ms(1) + L + Time::from_fs(1));
    let clamped = Time::from_ns(bb_cpu_renode::MAX_PROMISE_NS);
    let far: Vec<_> = on.promises.iter().filter(|(_, u)| *u == clamped).collect();
    eprintln!(
        "nrf-timer-sleep: {} promises, {} of them clamped to {clamped}; first promises {:?}; \
         grants {}",
        on.promises.len(),
        far.len(),
        &on.promises[..on.promises.len().min(4)],
        on.grants.len()
    );
    assert!(
        far.len() >= 10,
        "the sleeps after the compare must give the clamped promise: {:?}",
        on.promises
    );
    assert!(on.grants.iter().all(|(a, b)| *b - *a <= MAX_GRANT));
    assert!(on.grants.len() <= 25, "{}", on.grants.len());
}

/// `nrf-pincnf` (ADR-008 D4): the end-of-run `PIN_CNF` audit reports
/// exactly the disagreements `nrf_pincnf.c` sets up, the same with and
/// without lookahead, and reading it changes nothing.
#[test]
fn nrf_pin_cnf_audit_findings() {
    let Some(mut cfg) = config("nrf_pincnf", &["P0.13", "P0.14", "P0.15"]) else {
        return;
    };
    for p in ["P0.11", "P0.12", "P0.17"] {
        cfg.pins.push(PinSpec::nrf(p, PinDirection::Input).unwrap());
    }
    let out = |pin: &str, drive| PinCnfAudit {
        pin: pin.into(),
        expect: PinCnfExpect::DigitalOut { drive },
    };
    let audit = |pin: &str, expect| PinCnfAudit {
        pin: pin.into(),
        expect,
    };
    let s0s1 = Some(NrfDrive::S0S1);
    let mut audited = cfg.clone();
    audited.pin_cnf_audit = vec![
        audit("P0.12", PinCnfExpect::DigitalIn),
        out("P0.14", s0s1),
        out("P0.13", s0s1),
        audit("P0.11", PinCnfExpect::DigitalIn),
        out("P0.15", None),
        audit("P0.20", PinCnfExpect::Unbound),
        audit("P0.21", PinCnfExpect::Unbound),
        audit("P0.17", PinCnfExpect::DigitalIn),
    ];
    let t_end = Time::from_ms(30);
    let plain = drive(&cfg, Mode::Reference, t_end, None);
    let reference = drive(&audited, Mode::Reference, t_end, None);
    let long = drive(&audited, Mode::Long, t_end, None);
    let on = drive(&audited, Mode::On, t_end, None);
    for d in [&plain, &reference, &long, &on] {
        assert!(d.err.is_none(), "{:?}", d.err);
    }
    assert_eq!(plain.findings, None);
    assert!(plain.logs.is_empty());
    // The write hooks and the end-of-run read change no edge.
    assert_eq!(reference.edges, plain.edges, "the audit changed the edges");
    assert_eq!(long.edges, plain.edges);
    assert_eq!(on.edges, plain.edges);

    // Mid-run: each write that leaves a pin disagreeing, at the write's own
    // virtual time, identical whatever the grant partition.
    for (t, level, msg) in &reference.logs {
        eprintln!("nrf-pincnf: mid-run at {t}: {level:?} {msg}");
    }
    assert_eq!(
        long.logs, reference.logs,
        "10 ms grants changed the mid-run records"
    );
    assert_eq!(
        on.logs, reference.logs,
        "lookahead changed the mid-run records"
    );
    let mid: Vec<(Time, &str)> = reference
        .logs
        .iter()
        .map(|(t, _, m)| (*t, m.split_whitespace().nth(2).unwrap_or("")))
        .collect();
    let boot = Time::from_fs(1); // stamp 0 on the lattice: reported at 0 + 1 fs
    assert_eq!(
        mid[..4],
        [
            (boot, "U1.P0.13"),
            (boot, "U1.P0.11"),
            (boot, "U1.P0.15"),
            (boot, "U1.P0.20")
        ]
    );
    assert_eq!(mid.len(), 5, "{mid:?}");
    // P0.17's DIRSET in the first RTC0 compare's ISR (328 ticks, about
    // 10.0098 ms), reverted by the second: seen only mid-run.
    let (t17, p17) = mid[4];
    assert_eq!(p17, "U1.P0.17");
    assert!(
        t17 >= Time::from_us(10_009) && t17 <= Time::from_us(10_030),
        "P0.17 write at {t17}"
    );
    assert!(reference.logs[4].2.starts_with("PIN_CNF write: "));
    let f = reference.findings.clone().expect("the audit ran");
    for x in &f {
        eprintln!("nrf-pincnf: {:?} {}", x.level, x.msg);
    }
    let got: Vec<(&str, LogLevel)> = f.iter().map(|x| (x.pin.as_str(), x.level)).collect();
    assert_eq!(
        got,
        [
            ("P0.13", LogLevel::Warn),
            ("P0.11", LogLevel::Warn),
            ("P0.15", LogLevel::Error),
            ("P0.20", LogLevel::Warn),
        ]
    );
    assert!(f[0].msg.contains("drive S0S1") && f[0].msg.contains("drive H0H1"));
    assert_eq!(on.findings, reference.findings);
}

/// `nrf-oe` (ADR-009 D5, milestone P2's prototype): the `#oe` outputs, each
/// pin's `PIN_CNF.DIR` from the engine's `PIN_CNF` write hook, on
/// `nrf_pincnf.c`. P0.17's DIRSET in the first RTC0 compare's ISR, and its
/// `PIN_CNF` input write in the second, come a few instructions after that
/// ISR's P0.13 toggle: in program order, within one cell (here the next
/// slice; `nrf_oe_same_clock_as_gpio` checks adjacent stores, which share a
/// slice and a time). The boot writes (before the first WFI) are at
/// 1 fs (stamp 0, lattice rule 1); a pin configured as an input has no
/// `#oe` edge. Partition independent (one-cell, 10 ms grants, lookahead
/// on), the GPIO edges unchanged by the `#oe` pins, and the audit's
/// mid-run record for P0.17 at its `#oe` rise when both run.
#[test]
fn nrf_oe_trace_exact() {
    let Some(cfg) = config("nrf_pincnf", &["P0.13", "P0.14", "P0.15"]) else {
        return;
    };
    let mut with_oe = cfg.clone();
    for p in [
        "P0.11", "P0.12", "P0.13", "P0.14", "P0.15", "P0.17", "P0.20", "P0.21",
    ] {
        with_oe
            .pins
            .push(PinSpec::nrf_oe(&format!("{p}#oe")).unwrap());
    }
    let t_end = Time::from_ms(30);
    let plain = drive(&cfg, Mode::Reference, t_end, None);
    let reference = drive(&with_oe, Mode::Reference, t_end, None);
    let long = drive(&with_oe, Mode::Long, t_end, None);
    let on = drive(&with_oe, Mode::On, t_end, None);
    for d in [&plain, &reference, &long, &on] {
        assert!(d.err.is_none(), "{:?}", d.err);
    }
    for e in &reference.edges {
        eprintln!("nrf-oe: {} {} {}", e.0, e.1, u8::from(e.2));
    }
    // The #oe pins change no GPIO edge.
    let gpio = |d: &Drive| -> Vec<(Time, String, bool)> {
        d.edges
            .iter()
            .filter(|e| !e.1.ends_with("#oe"))
            .cloned()
            .collect()
    };
    assert_eq!(gpio(&reference), plain.edges, "#oe changed the GPIO edges");
    assert_eq!(long.edges, reference.edges, "10 ms grants changed #oe");
    assert_eq!(on.edges, reference.edges, "lookahead changed #oe");
    check_honest("nrf-oe", &reference, &on);
    let oe: Vec<(Time, &str, bool)> = reference
        .edges
        .iter()
        .filter(|e| e.1.ends_with("#oe"))
        .map(|e| (e.0, e.1.as_str(), e.2))
        .collect();
    let boot = Time::from_fs(1);
    // The boot configuration, in write order: P0.14, P0.13, P0.11, P0.15,
    // P0.20 outputs; P0.12, P0.21, P0.17 inputs (no change).
    assert_eq!(
        oe[..5],
        [
            (boot, "P0.14#oe", true),
            (boot, "P0.13#oe", true),
            (boot, "P0.11#oe", true),
            (boot, "P0.15#oe", true),
            (boot, "P0.20#oe", true),
        ]
    );
    let p13 = edges_of(&reference, "P0.13");
    let p17: Vec<_> = oe.iter().filter(|e| e.1 == "P0.17#oe").collect();
    assert_eq!(p17.len(), 2, "{p17:?}");
    assert!(p17[0].2 && !p17[1].2);
    // The ISR toggles P0.13 a few instructions before it writes P0.17's
    // direction: program order is kept, within one cell.
    for k in 0..2 {
        assert!(
            p17[k].0 >= p13[k].0 && p17[k].0 - p13[k].0 < L,
            "P0.17#oe at {} against the P0.13 edge at {}",
            p17[k].0,
            p13[k].0
        );
    }
    assert_eq!(oe.len(), 7, "{oe:?}");
    // With the audit as well: its mid-run record for P0.17 is at the rise.
    let mut both = with_oe.clone();
    both.pin_cnf_audit = vec![PinCnfAudit {
        pin: "P0.17".into(),
        expect: PinCnfExpect::DigitalIn,
    }];
    let b = drive(&both, Mode::On, t_end, None);
    assert!(b.err.is_none(), "{:?}", b.err);
    assert_eq!(b.edges, reference.edges, "the audit changed #oe");
    assert_eq!(b.logs.len(), 1, "{:?}", b.logs);
    assert_eq!(b.logs[0].0, p17[0].0);
    eprintln!(
        "nrf-oe: P0.17#oe {} / {}, P0.13 {} / {}; reference {:.2} s, plain {:.2} s",
        p17[0].0, p17[1].0, p13[0].0, p13[1].0, reference.wall_s, plain.wall_s
    );
}

/// `nrf-oe` exactness (ADR-009 D5): `nrf_oe.c` writes P0.17's direction
/// and the GPIO outputs in adjacent stores. A `#oe` edge and a GPIO edge read
/// the same clock (the sync point that opened the write's slice) by the same
/// lattice rule, so the stores of one slice get one time and program order
/// is never inverted: `#oe` rises at or before the P0.14 / P0.17 rise of its
/// ISR and falls at or after their fall. Partition independent.
#[test]
fn nrf_oe_same_clock_as_gpio() {
    let Some(mut cfg) = config("nrf_oe", &["P0.14", "P0.17"]) else {
        return;
    };
    cfg.pins.push(PinSpec::nrf_oe("P0.17#oe").unwrap());
    let t_end = Time::from_ms(60);
    let reference = drive(&cfg, Mode::Reference, t_end, None);
    let long = drive(&cfg, Mode::Long, t_end, None);
    let on = drive(&cfg, Mode::On, t_end, None);
    for d in [&reference, &long, &on] {
        assert!(d.err.is_none(), "{:?}", d.err);
    }
    assert_eq!(
        long.edges, reference.edges,
        "10 ms grants changed the edges"
    );
    assert_eq!(on.edges, reference.edges, "lookahead changed the edges");
    check_honest("nrf-oe", &reference, &on);
    let oe = edges_of(&reference, "P0.17#oe");
    let p14 = edges_of(&reference, "P0.14");
    let p17 = edges_of(&reference, "P0.17");
    assert!(oe.len() >= 5, "{oe:?}");
    assert_eq!(oe.len(), p14.len());
    assert_eq!(oe.len(), p17.len());
    let mut equal = 0;
    for k in 0..oe.len() {
        let (o, a, b) = (oe[k], p14[k], p17[k]);
        assert_eq!(o.2, a.2, "edge {k}: {o:?} {a:?}");
        assert_eq!(a.0, b.0, "one OUTSET/OUTCLR store: {a:?} {b:?}");
        if o.2 {
            assert!(
                o.0 <= a.0 && a.0 - o.0 < L,
                "rise {k}: #oe {} after P0.14 {}",
                o.0,
                a.0
            );
        } else {
            assert!(
                o.0 >= a.0 && o.0 - a.0 < L,
                "fall {k}: #oe {} before P0.14 {}",
                o.0,
                a.0
            );
        }
        equal += usize::from(o.0 == a.0);
    }
    eprintln!(
        "nrf-oe: {} #oe edges, {equal} at the same time as their ISR's GPIO edge (the rest \
         one slice apart, in program order); first rise {} / P0.14 {}",
        oe.len(),
        oe[0].0,
        p14[0].0
    );
}

/// The prebuilt Zephyr gate image (`tests/firmware/nrf52840/zephyr/`).
fn zephyr_elf() -> Option<PathBuf> {
    let elf = fw_dir().join("zephyr/prebuilt/nrf_zephyr_gate.elf");
    assert!(elf.is_file(), "{} missing", elf.display());
    Some(elf)
}

/// `nrf-zephyr` (ADR-008 D3, D1): an unmodified Zephyr image on the
/// platform. Its main thread toggles P0.13 around `k_busy_wait(10000)` and
/// P0.14 around `k_msleep(10)`, then toggles P0.15 every 50 ms with
/// `k_msleep`. Identical edges at one-cell grants, 10 ms grants, plugin
/// armed and lookahead on.
#[test]
fn nrf_zephyr_image_identical_with_and_without_lookahead() {
    let Some(cfg) = config_elf(zephyr_elf(), &["P0.13", "P0.14", "P0.15"]) else {
        return;
    };
    let (reference, on) = all_modes("nrf-zephyr", &cfg, Time::from_ms(300));
    let width = |pin: &str| {
        let e = edges_of(&reference, pin);
        assert!(e.len() >= 2 && e[0].2 && !e[1].2, "{pin}: {e:?}");
        e[1].0 - e[0].0
    };
    let busy = width("P0.13");
    let sleep = width("P0.14");
    let ms = |t: Time| t.as_fs() as f64 / 1e12;
    let blink = edges_of(&reference, "P0.15");
    eprintln!(
        "nrf-zephyr: k_busy_wait(10000) took {:.6} ms of virtual time, k_msleep(10) {:.6} ms; \
         {} P0.15 edges, first at {}; lookahead {} grants against {}",
        ms(busy),
        ms(sleep),
        blink.len(),
        blink.first().map(|e| e.0).unwrap_or_default(),
        on.grants.len(),
        reference.grants.len()
    );
    // Zephyr's arch_busy_wait is nrfx_coredep_delay_us: about 2/3.
    assert!(
        busy >= Time::from_us(6_600) && busy <= Time::from_us(6_800),
        "k_busy_wait {busy}"
    );
    // k_msleep(10) at CONFIG_SYS_CLOCK_TICKS_PER_SEC = 32768: 328 ticks,
    // plus at most one tick of rounding to the next tick boundary.
    let tick = Time::from_fs(1_000_000_000_000_000 / 32768);
    assert!(
        sleep >= Time::from_ms(10) && sleep <= Time::from_ms(10) + tick + tick + L,
        "k_msleep(10) {sleep}"
    );
    assert!(blink.len() >= 4, "{blink:?}");
    assert!(on.grants.len() * 5 < reference.grants.len());
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// ADR-008 D1 ("M2 re-measures them at 64"): fresh engine startup and
/// per-grant cost of the nRF52840 blinky at 64 MIPS against the STM32F103
/// spike blinky (measurement protocol: interleaved, N runs each,
/// `BB_NRF_COST_N`, default 5, medians). Reports only. Ignored by default
/// (wall clock): `make nrf-costs`.
#[test]
#[ignore]
fn nrf_startup_and_grant_costs() {
    let Some(mut nrf) = config("nrf_blinky", &["P0.13"]) else {
        return;
    };
    nrf.pool = None;
    let renode = nrf.renode.clone();
    let stm_elf = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/spike/firmware/build/blinky.elf");
    assert!(
        stm_elf.is_file(),
        "make firmware first: {}",
        stm_elf.display()
    );
    let mut stm = RenodeConfig::stm32f103(renode, stm_elf);
    stm.pool = None;
    stm.quantum = L;
    let n: usize = std::env::var("BB_NRF_COST_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);
    // (startup s, one-cell grant ms, 10 ms grant ms) per platform.
    let mut res = [
        [Vec::new(), Vec::new(), Vec::new()],
        [Vec::new(), Vec::new(), Vec::new()],
    ];
    for i in 0..n {
        for (k, cfg) in [&stm, &nrf].into_iter().enumerate() {
            let t0 = Instant::now();
            let mut e = RenodeEngine::start(cfg).expect("start");
            let startup = t0.elapsed().as_secs_f64();
            let _ = e.next_safe_time(Time::ZERO);
            // 1 000 one-cell grants, then 20 grants of 10 ms.
            let t1 = Instant::now();
            let mut now = Time::ZERO;
            for _ in 0..1000 {
                now = now + L;
                e.advance_to(now).unwrap();
                let _ = e.read_output_trace().unwrap();
            }
            let cell = t1.elapsed().as_secs_f64() * 1e3 / 1000.0;
            let t2 = Instant::now();
            for _ in 0..20 {
                now = now + MAX_GRANT;
                e.advance_to(now).unwrap();
                let _ = e.read_output_trace().unwrap();
            }
            let long = t2.elapsed().as_secs_f64() * 1e3 / 20.0;
            drop(e);
            eprintln!(
                "run {i} {}: startup {startup:.3} s, one-cell grant {cell:.3} ms, 10 ms grant \
                 {long:.2} ms",
                ["stm32f103", "nrf52840"][k]
            );
            res[k][0].push(startup);
            res[k][1].push(cell);
            res[k][2].push(long);
        }
    }
    let load = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
    for (k, name) in ["STM32F103 blinky", "nRF52840 blinky (64 MIPS)"]
        .iter()
        .enumerate()
    {
        let [a, b, c] = &mut res[k];
        eprintln!(
            "cost: {name}: fresh startup {:.2} s, one-cell grant {:.3} ms, 10 ms grant {:.2} ms \
             (N = {n}, medians, load {})",
            median(a),
            median(b),
            median(c),
            load.trim()
        );
    }
}

/// One instruction at the platform's 64 MIPS (`nrf52840-bb@4`).
const INSTR: Time = Time::from_fs(15_625_000);

/// The instant an edge reports (the lattice rule of GPIO events, ADR-003
/// D2: an instant on a lattice point is reported 1 fs after it).
fn instant(t: Time) -> Time {
    if t.as_fs() % L.as_fs() == 1 {
        Time::from_fs(t.as_fs() - 1)
    } else {
        t
    }
}

/// `nrf-state` (ADR-009 D4, Phase 2 milestone P3): the CPU state outputs at
/// instants derived by hand from the gate firmware (`nrf_state.c`). With
/// PRIMASK set, a pending RTC0 compare or SAADC END wakes WFI and no handler
/// runs, so the code between two WFIs is a fixed straight line of
/// instructions (one inline asm block):
///
/// - `#run` rises at each wake: at the SAADC's END, exactly `a + TACQ +
///   tCONV` (22 us after the acquisition start `a`, the `#smp` edge, ADR-008
///   D7), where `#saadc` falls; at an RTC compare, within Renode's clock
///   grid (125 ns) after `k * 33 / 32768 s`;
/// - `#run` falls exactly 9 instructions after an RTC wake (8 after the WFI
///   and the second WFI itself) and exactly 7 after an END wake, at 15.625
///   ns each: the instant after the WFI instruction, to the fs;
/// - `#hfxo` rises at the HFCLKSTART store and falls at the HFCLKSTOP store
///   of each cycle, in the block of the wake (Renode's write time), in
///   program order before the sleep that follows;
/// - `#nvmc` is high for exactly the NVMC model's tERASEPAGE, 85 ms, from
///   the ERASEPAGE store before the first WFI;
/// - `#saadc` rises with `#smp` (START and SAMPLE in one block) and falls
///   at `a + 22 us`.
///
/// Identical at one-cell grants, 10 ms grants, plugin armed and lookahead
/// on (no promise contains an edge); the GPIO and `#smp` edges are the same
/// with and without the state outputs bound (they do not perturb the run).
#[test]
fn nrf_state_outputs_exact() {
    let Some(mut base) = config("nrf_state", &["P0.13"]) else {
        return;
    };
    base.pins.push(PinSpec::adc_sample(
        "P0.02#smp",
        bb_cpu_renode::BB_SAADC_NAME,
        0,
    ));
    let mut cfg = base.clone();
    for s in ["#run", "#hfxo", "#nvmc", "#saadc"] {
        cfg.pins.push(PinSpec::nrf_state(s).unwrap());
    }
    let t_end = Time::from_ms(120);
    let (reference, _on) = all_modes("nrf-state", &cfg, t_end);
    let without = drive(&base, Mode::Reference, t_end, None);
    assert!(without.err.is_none(), "{:?}", without.err);
    let not_state: Vec<_> = reference
        .edges
        .iter()
        .filter(|e| !e.1.starts_with('#'))
        .cloned()
        .collect();
    assert_eq!(
        not_state, without.edges,
        "binding the state outputs changed the GPIO or #smp edges"
    );

    let run = edges_of(&reference, "#run");
    let smp = edges_of(&reference, "P0.02#smp");
    let saadc = edges_of(&reference, "#saadc");
    let hfxo = edges_of(&reference, "#hfxo");
    let nvmc = edges_of(&reference, "#nvmc");
    // #nvmc: one erase, 85 ms exactly, started before the first WFI.
    assert_eq!(nvmc.len(), 2, "{nvmc:?}");
    assert!(nvmc[0].2 && !nvmc[1].2);
    assert_eq!(nvmc[1].0 - nvmc[0].0, Time::from_ms(85), "tERASEPAGE");
    assert!(
        nvmc[0].0 < run[0].0,
        "the erase starts before the first WFI"
    );
    // #run: the first edge is the first WFI after reset; then each cycle
    // is RTC wake, sleep, END wake, sleep.
    assert!(!run[0].2, "{:?}", run[0]);
    let cycles = (run.len() - 1) / 4;
    assert!(cycles >= 115, "{} #run edges", run.len());
    assert_eq!(smp.len(), cycles + usize::from((run.len() - 1) % 4 >= 2));
    let mut checked = 0;
    for k in 0..cycles {
        let j = 1 + 4 * k;
        assert!(
            run[j].2 && !run[j + 1].2 && run[j + 2].2 && !run[j + 3].2,
            "cycle {k}"
        );
        let (wake_a, fall_b, wake_b, fall_a) = (
            instant(run[j].0),
            instant(run[j + 1].0),
            instant(run[j + 2].0),
            instant(run[j + 3].0),
        );
        // The RTC wakes about every 33 LFCLK ticks (Renode's RTC model
        // times them; its own accuracy is not this gate's subject).
        assert!(
            wake_a > instant(run[j - 1].0) + Time::from_us(900),
            "cycle {k}: RTC wake {wake_a}"
        );
        // The WFI instants, to the fs.
        assert_eq!(
            fall_b,
            wake_a + Time::from_fs(9 * INSTR.as_fs()),
            "cycle {k}: sleep B"
        );
        assert_eq!(
            fall_a,
            wake_b + Time::from_fs(7 * INSTR.as_fs()),
            "cycle {k}: sleep A"
        );
        // The SAADC: a, START with SAMPLE, END at a + 22 us = the wake.
        let a = instant(smp[k].0);
        assert!(a >= wake_a && a < fall_b, "cycle {k}: a {a}");
        assert_eq!(instant(saadc[2 * k].0), a, "cycle {k}: #saadc rise");
        assert!(saadc[2 * k].2 && !saadc[2 * k + 1].2);
        assert_eq!(
            instant(saadc[2 * k + 1].0),
            a + Time::from_us(22),
            "cycle {k}: END"
        );
        assert_eq!(
            wake_b,
            instant(saadc[2 * k + 1].0),
            "cycle {k}: the END wake"
        );
        // #hfxo: the stores' write times, in program order.
        assert!(hfxo[2 * k].2 && !hfxo[2 * k + 1].2);
        let (up, down) = (instant(hfxo[2 * k].0), instant(hfxo[2 * k + 1].0));
        assert!(up >= wake_a && up <= a, "cycle {k}: HFCLKSTART {up}");
        assert!(
            down >= wake_b && down < fall_a,
            "cycle {k}: HFCLKSTOP {down}"
        );
        checked += 1;
    }
    eprintln!(
        "nrf-state: {checked} cycles exact; first WFI at {} ({} instructions from reset); RTC \
         wake {} -> sleep {} (+9 instr), END wake {} -> sleep {} (+7 instr); #nvmc {} .. {}",
        run[0].0,
        run[0].0.as_fs() / INSTR.as_fs(),
        run[1].0,
        run[2].0,
        run[3].0,
        run[4].0,
        nvmc[0].0,
        nvmc[1].0
    );
}

/// Drive `cfg` in `mode` to `t_end`, delivering the `#bor` transitions
/// `bor` (time, level) as the kernel does: each in the input trace of the
/// grant that contains it (the engine applies it at `ceil_L`). Grants end at
/// most at each transition's `ceil_L`, as a producer that runs first stops
/// there (ADR-003 D5). Returns the drive and the `reset` records.
fn drive_bor(
    cfg: &RenodeConfig,
    mode: Mode,
    t_end: Time,
    bor: &[(Time, bool)],
) -> (Drive, Vec<(Time, bbcore::ResetPhase)>) {
    drive_bor_fault(cfg, mode, t_end, bor, None)
}

/// [`drive_bor`] with an injected fault.
fn drive_bor_fault(
    cfg: &RenodeConfig,
    mode: Mode,
    t_end: Time,
    bor: &[(Time, bool)],
    fault: Option<Fault>,
) -> (Drive, Vec<(Time, bbcore::ResetPhase)>) {
    let cfg = with_mode(cfg, mode);
    let mut e = RenodeEngine::start(&cfg).expect("start renode");
    if let Some(f) = fault {
        e.inject_fault(f);
    }
    let mut d = Drive {
        startup: e.lookahead_startup(),
        leased: e.leased(),
        pid: e.renode_pid(),
        ..Drive::default()
    };
    let mut resets = Vec::new();
    let mut now = Time::ZERO;
    let mut next = 0;
    let base: bbcore::PinSignals = [(
        bb_cpu_renode::NRF_BOR.to_string(),
        PinSignal::digital(bbcore::Level::Low, Time::ZERO),
    )]
    .into_iter()
    .collect();
    e.apply_inputs(&base).unwrap();
    while now < t_end {
        assert_eq!(e.next_safe_time(now), Time::MAX);
        let q = e.quiet_until(now);
        if q > now {
            d.promises.push((now, q));
        }
        let mut g = match mode {
            Mode::Long => floor_l((now + MAX_GRANT).min(t_end)),
            _ => floor_l(q.min(now + MAX_GRANT).min(t_end)).max(now + L),
        };
        if let Some((t, _)) = bor.get(next) {
            g = g.min(ceil_l(*t).max(now + L));
        }
        let mut trace = Vec::new();
        while let Some((t, high)) = bor.get(next) {
            if *t > g {
                break;
            }
            trace.push(bbcore::Transition {
                t: *t,
                pin: bb_cpu_renode::NRF_BOR.to_string(),
                signal: PinSignal::digital(bbcore::Level::from_bool(*high), *t),
            });
            next += 1;
        }
        e.apply_input_trace(&Default::default(), &trace).unwrap();
        if let Err(err) = e.advance_to(g) {
            d.err = Some((now, err));
            break;
        }
        for tr in e.read_output_trace().unwrap().expect("a trace") {
            let PinSignal::Digital { level, .. } = tr.signal else {
                panic!("analog output from renode")
            };
            assert!(
                tr.t > now && tr.t <= g,
                "{} at {} outside ({now}, {g}]",
                tr.pin,
                tr.t
            );
            d.edges.push((tr.t, tr.pin, level == bbcore::Level::High));
        }
        for r in e.drain_logs() {
            d.logs.push((r.t, r.level, r.msg));
        }
        for r in e.drain_resets() {
            assert!(
                r.t >= now && r.t <= g,
                "reset at {} outside [{now}, {g}]",
                r.t
            );
            resets.push((r.t, r.phase));
        }
        d.grants.push((now, g));
        now = g;
    }
    if d.err.is_none() {
        e.end_of_run().expect("end-of-run verify");
    }
    d.stats = e.lookahead_stats();
    (d, resets)
}

/// ADR-009 D7 and P5's pool rule (approver decision D-close 7; ADR-009
/// Amendment 12): a pool process is never reused after a reset that failed.
/// A private 1-slot pool; `nrf_bor.elf` with a brown-out at 10.0034 ms whose
/// read-back is made to disagree (`Fault::BorReadback`, the engine's fault
/// knob): the leased run fails with the engine's "did not hold" error, the
/// pool discards that process, and the same run without the fault leases a
/// new process and equals a fresh Renode's edges and reset records.
#[test]
fn nrf_bor_failed_reset_is_discarded() {
    let Some(mut cfg) = config("nrf_bor", &["P0.13", "P0.14", "P0.15", "P0.16"]) else {
        return;
    };
    cfg.pins.push(PinSpec::nrf_oe("P0.13#oe").unwrap());
    cfg.pins.push(PinSpec::nrf_oe("P0.15#oe").unwrap());
    cfg.pins.push(PinSpec::nrf_state("#run").unwrap());
    cfg.pins.push(PinSpec::nrf_bor());
    let dir = std::env::temp_dir().join(format!("bb-pool-bor-{}", std::process::id()));
    let mut pc = bb_cpu_renode::manager::PoolConfig::new(cfg.renode.clone(), dir);
    pc.slots = 1;
    let m = bb_cpu_renode::manager::Manager::start(pc).expect("start the pool");
    let warm = std::time::Duration::from_secs(300);
    m.wait_warm(warm).expect("pre-warm");
    let mut pooled = cfg.clone();
    pooled.pool = Some(m.socket().to_path_buf());
    cfg.pool = None;
    let bor = [
        (Time::from_ns(10_003_400), true),
        (Time::from_ns(12_007_100), false),
    ];
    let t_end = Time::from_ms(15);
    let (bad, _) = drive_bor_fault(&pooled, Mode::On, t_end, &bor, Some(Fault::BorReadback));
    assert!(bad.leased, "the faulted run did not lease from the pool");
    match &bad.err {
        Some((t, EngineError::Protocol(msg))) => {
            assert!(
                msg.contains("did not hold the machine"),
                "unexpected protocol error at {t}: {msg}"
            );
        }
        other => panic!("expected the failed hold's protocol error, got {other:?}"),
    }
    m.wait_warm(warm).expect("warm again");
    let (ok, r_ok) = drive_bor(&pooled, Mode::On, t_end, &bor);
    assert!(ok.err.is_none(), "{:?}", ok.err);
    assert!(ok.leased, "the run after the failure did not lease");
    assert_ne!(ok.pid, bad.pid, "the process of a failed reset was reused");
    let (fresh, r_fresh) = drive_bor(&cfg, Mode::On, t_end, &bor);
    assert!(fresh.err.is_none() && !fresh.leased, "{:?}", fresh.err);
    assert_eq!(
        ok.edges, fresh.edges,
        "the new lease's edges against a fresh Renode"
    );
    assert_eq!(r_ok, r_fresh, "the reset records");
    assert_eq!(r_ok.len(), 2, "one brown-out: its hold and its release");
    assert_eq!(m.status().1.discarded, 1, "{:?}", m.status());
    eprintln!(
        "nrf-bor-pool: failed reset on pid {} discarded; the next lease pid {}",
        bad.pid, ok.pid
    );
    m.shutdown();
}

/// `nrf-bor` (ADR-009 D7, Phase 2 P5; the prototype of risk R3 made a gate):
/// a brown-out under External Control. `#bor` rises at 10.0034 ms and falls
/// at 12.0071 ms (off the lattice), then rises at 20.0005 ms and falls at
/// 20.5 ms (a hold of 50 cells). The engine holds the machine in reset from
/// `ceil_L` of each rise to `ceil_L` of each fall and resets it again there:
/// every output and `#oe` low and no change while held, `#run` low; at the
/// release `#run` rises and the firmware boots as from power-on (P0.13's
/// first toggle the same 1.00725 ms after it), RAM kept (P0.15: the boot
/// count survived), RESETREAS 0 (P0.14 low) and GPREGRET cleared (P0.16
/// low). One-cell grants, 10 ms grants, the armed plugin, lookahead on (a
/// held machine promises quiet until the release) and a pooled Renode (when
/// `BB_RENODE_POOL` is set) give identical edges and `reset` records.
#[test]
fn nrf_bor_hold_and_release() {
    let Some(mut cfg) = config("nrf_bor", &["P0.13", "P0.14", "P0.15", "P0.16"]) else {
        return;
    };
    cfg.pins.push(PinSpec::nrf_oe("P0.13#oe").unwrap());
    cfg.pins.push(PinSpec::nrf_oe("P0.15#oe").unwrap());
    cfg.pins.push(PinSpec::nrf_state("#run").unwrap());
    cfg.pins.push(PinSpec::nrf_bor());
    let bor = [
        (Time::from_ns(10_003_400), true),
        (Time::from_ns(12_007_100), false),
        (Time::from_ns(20_000_500), true),
        (Time::from_ns(20_500_000), false),
    ];
    let t_end = Time::from_ms(30);
    let (reference, r_ref) = drive_bor(&cfg, Mode::Reference, t_end, &bor);
    assert!(reference.err.is_none(), "reference: {:?}", reference.err);
    use bbcore::ResetPhase::{Enter, Exit};
    assert_eq!(
        r_ref,
        vec![
            (Time::from_us(10_010), Enter),
            (Time::from_us(12_010), Exit),
            (Time::from_us(20_010), Enter),
            (Time::from_us(20_500), Exit),
        ],
        "the reset records at ceil_L of each crossing"
    );
    let at = |t: Time| t + Time::from_fs(1);
    for (start, end) in [
        (Time::from_us(10_010), Time::from_us(12_010)),
        (Time::from_us(20_010), Time::from_us(20_500)),
    ] {
        // While held: every output and #oe low at the reset, nothing else.
        let held: Vec<_> = reference
            .edges
            .iter()
            .filter(|e| e.0 > start && e.0 <= end)
            .collect();
        assert!(!held.is_empty(), "the hold at {start} lowered nothing");
        for e in &held {
            assert!(e.0 == at(start) && !e.2, "a change while held: {e:?}");
        }
        for pin in ["P0.13#oe", "P0.15#oe"] {
            assert!(
                held.iter().any(|e| e.1 == pin),
                "{pin} did not fall at the hold {start}: {held:?}"
            );
        }
        // The release: #run rises, the boot configures the pins at once,
        // P0.15 high (RAM kept), P0.14 and P0.16 stay low.
        let boot: Vec<_> = reference.edges.iter().filter(|e| e.0 == at(end)).collect();
        for (pin, level) in [
            ("#run", true),
            ("P0.13#oe", true),
            ("P0.15#oe", true),
            ("P0.15", true),
        ] {
            assert!(
                boot.iter().any(|e| e.1 == pin && e.2 == level),
                "{pin} at the release {end}: {boot:?}"
            );
        }
        let first = reference
            .edges
            .iter()
            .find(|e| e.1 == "P0.13" && e.0 > end)
            .expect("P0.13 toggles after the release");
        assert_eq!(
            first.0,
            end + Time::from_ns(1_007_250),
            "the RTC restarts with the boot"
        );
    }
    for pin in ["P0.14", "P0.16"] {
        assert!(
            edges_of(&reference, pin).is_empty(),
            "{pin}: RESETREAS and GPREGRET are 0 after a brown-out"
        );
    }
    // At power-on the boot count is 1: P0.15 rises only after the first release.
    let p15 = edges_of(&reference, "P0.15");
    assert_eq!(p15[0].0, at(Time::from_us(12_010)), "{p15:?}");
    // The same in every mode.
    for mode in [Mode::Long, Mode::ArmedOff, Mode::On] {
        let (d, r) = drive_bor(&cfg, mode, t_end, &bor);
        assert!(d.err.is_none(), "{mode:?}: {:?}", d.err);
        assert_eq!(d.edges, reference.edges, "{mode:?}: the edges");
        assert_eq!(r, r_ref, "{mode:?}: the reset records");
        if mode == Mode::On {
            // A held machine promises quiet until the release.
            assert!(
                d.promises.iter().any(|(f, u)| *f <= Time::from_us(10_020)
                    && *f >= Time::from_us(10_010)
                    && *u >= Time::from_us(12_010)),
                "no hold promise: {:?}",
                d.promises
            );
            eprintln!(
                "nrf-bor: {} edges, {} reference grants, {} lookahead grants ({} promises)",
                reference.edges.len(),
                reference.grants.len(),
                d.grants.len(),
                d.stats.promises
            );
        }
    }
}
