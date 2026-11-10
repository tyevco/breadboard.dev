//! Phase 1 M2 (ADR-008 D1-D4, D6): Breadboard's nRF52840 platform
//! (`nrf52840-bb@2`) under the Renode engine, with the bare-metal gate
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
