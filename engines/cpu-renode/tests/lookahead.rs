//! ADR-003 plan step 5: the Renode lookahead plugin (D5, Renode side).
//!
//! The kernel's per-grant plan is step 6, so these tests drive one Renode
//! engine directly with the D5 rule: at every boundary ask `quiet_until`,
//! and grant `floor_L(min(quiet_until, now + 10 ms, t_end))` when that is
//! more than one cell, else one cell. A promise lets the engine run ahead
//! of everything that depends on it, so it must be honest: the gates check
//!
//! - inertness: with the plugin loaded and armed but lookahead off, every
//!   firmware gives exactly the edges of a run without the plugin;
//! - each gate firmware (TIM1 PWM on PA8, DMA -> GPIOA_BSRR, UART TX,
//!   SLEEPONEXIT, WFE/SEV, silent wakes) is either equivalent to the
//!   one-cell reference with every promise honoured (no reference edge
//!   inside any promised window), or fails with `PromiseBroken`, or gets no
//!   promise at all (fallback). The expected outcome of each is pinned;
//! - an injected over-long promise across a wake that changes no output is
//!   caught by the wake audit (`PromiseBroken`);
//! - a failed self-test changes nothing but the grants;
//! - the compiled plugin is cached (`plugin_startup_overhead`, ignored by
//!   default: `make lookahead-startup`, the startup-cost gate).
//!
//! Needs Renode and arm-none-eabi-gcc like `blinky.rs` (`BB_REQUIRE_RENODE`).

use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

use bb_cpu_renode::{
    find_renode, Fault, LookaheadStatus, PinDirection, PinSpec, RenodeConfig, RenodeEngine,
};
use bbcore::{Engine, EngineError, PinSignal, Time};

/// The default grant lattice (`--renode-quantum`).
const L: Time = Time::from_us(10);
/// Grant cap of the lookahead runs (`--max-quantum` default).
const MAX_GRANT: Time = Time::from_ms(10);

static BUILD: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn skip(msg: &str) {
    let required = std::env::var("BB_REQUIRE_RENODE").is_ok_and(|v| v == "1");
    assert!(!required, "BB_REQUIRE_RENODE=1 but {msg}");
    eprintln!("SKIP: {msg}");
}

/// `tests/firmware/build/<name>.elf`, built on demand.
fn firmware(name: &str) -> Option<PathBuf> {
    let _g = BUILD.lock().unwrap_or_else(|e| e.into_inner());
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/firmware");
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

fn config(name: &str, pins: &[&str]) -> Option<RenodeConfig> {
    let Some(renode) = find_renode() else {
        skip("renode not installed (run `make setup`, or set BB_RENODE)");
        return None;
    };
    let mut cfg = RenodeConfig::stm32f103(renode, firmware(name)?);
    cfg.name = "U1".into();
    cfg.pins = pins
        .iter()
        .map(|p| PinSpec::stm32(p, PinDirection::Output).unwrap())
        .collect();
    cfg.quantum = L;
    Some(cfg)
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Mode {
    /// No plugin, one-cell grants: the reference.
    Reference,
    /// Plugin loaded and armed, lookahead off, one-cell grants.
    ArmedOff,
    /// Plugin loaded, lookahead on, D5 grants.
    On,
}

fn with_mode(cfg: &RenodeConfig, mode: Mode) -> RenodeConfig {
    let mut c = cfg.clone();
    c.lookahead.plugin = mode != Mode::Reference;
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
}

fn floor_l(t: Time) -> Time {
    Time::from_fs(t.as_fs() / L.as_fs() * L.as_fs())
}

/// Run `cfg` to `t_end`: one-cell grants, or (with lookahead on) the D5
/// grant rule for a quiet CPU.
fn drive(cfg: &RenodeConfig, t_end: Time, fault: Option<Fault>) -> Drive {
    let mut e = RenodeEngine::start(cfg).expect("start renode");
    if let Some(f) = fault {
        e.inject_fault(f);
    }
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
        let g = floor_l(q.min(now + MAX_GRANT).min(t_end)).max(now + L);
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
        d.grants.push((now, g));
        now = g;
    }
    if d.err.is_none() {
        e.end_of_run().expect("end-of-run verify");
    }
    d.wall_s = t0.elapsed().as_secs_f64();
    d.stats = e.lookahead_stats();
    d
}

/// What a gate firmware does under lookahead.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Outcome {
    /// Promises given and honoured; edges identical to the reference.
    Equivalent,
    /// No promise at all (the board keeps the loop cap).
    Fallback,
    /// The audit caught a broken promise.
    PromiseBroken,
}

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

/// The three runs of a gate firmware; returns the lookahead run's outcome.
fn gate(name: &str, pins: &[&str], t_end: Time) -> Option<(Outcome, Drive, Drive)> {
    let cfg = config(name, pins)?;
    let reference = drive(&with_mode(&cfg, Mode::Reference), t_end, None);
    assert!(
        reference.err.is_none(),
        "{name}: reference failed: {:?}",
        reference.err
    );
    let armed = drive(&with_mode(&cfg, Mode::ArmedOff), t_end, None);
    assert!(
        armed.err.is_none(),
        "{name}: armed-off failed: {:?}",
        armed.err
    );
    assert_eq!(
        armed.edges, reference.edges,
        "{name}: the armed plugin changed the edges (it must be inert)"
    );
    assert_eq!(armed.grants, reference.grants);
    assert!(armed.promises.is_empty() && armed.stats.reads == 0);
    let st = armed.startup.expect("plugin loaded");
    assert!(st.armed() && st.self_test_ok(), "{name}: self-test {st:?}");

    let on = drive(&with_mode(&cfg, Mode::On), t_end, None);
    let outcome = match &on.err {
        Some((_, EngineError::PromiseBroken { .. })) => Outcome::PromiseBroken,
        Some((at, e)) => panic!("{name}: lookahead run failed at {at}: {e}"),
        None => {
            assert_eq!(
                on.edges, reference.edges,
                "{name}: lookahead changed the edges"
            );
            if on.promises.is_empty() {
                Outcome::Fallback
            } else {
                Outcome::Equivalent
            }
        }
    };
    check_honest(name, &reference, &on);
    let long = on.grants.iter().filter(|(a, b)| *b - *a > L).count();
    eprintln!(
        "{name}: {outcome:?}; reference {} edges in {} grants ({:.2} s), lookahead {} grants \
         ({long} longer than one cell, {:.2} s), {} promises, {} queries, {} reads, {} audits{}",
        reference.edges.len(),
        reference.grants.len(),
        reference.wall_s,
        on.grants.len(),
        on.wall_s,
        on.stats.promises,
        on.stats.queries,
        on.stats.reads,
        on.stats.audits,
        on.err
            .as_ref()
            .map(|(at, e)| format!("; error after {at}: {e:.200}"))
            .unwrap_or_default()
    );
    Some((outcome, reference, on))
}

/// TIM1 drives PA8 through the alternate-function connection while the CPU
/// sleeps. Its compare and update events are clock limits, so every promise
/// ends at the next PWM edge.
#[test]
fn gate_tim1_pwm_on_pa8() {
    let Some((outcome, reference, on)) = gate("la_tim1pwm", &["PA5", "PA8"], Time::from_ms(60))
    else {
        return;
    };
    let pa8 = reference.edges.iter().filter(|e| e.1 == "PA8").count();
    assert!(pa8 >= 100, "TIM1 PWM must toggle PA8 (got {pa8} edges)");
    assert_eq!(outcome, Outcome::Equivalent);
    assert!(on.grants.len() < reference.grants.len());
}

/// DMA1 channel 2 writes GPIOA_BSRR on each TIM2 update; the TIM2 ISR only
/// clears UIF.
#[test]
fn gate_dma_to_bsrr() {
    let Some((outcome, reference, on)) = gate("la_dma", &["PA5"], Time::from_ms(60)) else {
        return;
    };
    assert!(
        reference.edges.len() >= 50,
        "DMA must toggle PA5 every 1 ms (got {} edges)",
        reference.edges.len()
    );
    assert_eq!(outcome, Outcome::Equivalent);
    assert!(on.grants.len() < reference.grants.len());
}

/// Interrupt-driven USART1 TX with no host backend.
#[test]
fn gate_uart_tx() {
    let Some((outcome, reference, on)) = gate("la_uart", &["PA5"], Time::from_ms(60)) else {
        return;
    };
    assert!(reference.edges.len() >= 10, "{:?}", reference.edges);
    assert_eq!(outcome, Outcome::Equivalent);
    assert!(on.grants.len() < reference.grants.len());
}

/// SLEEPONEXIT: the CPU runs only the SysTick ISR and sleeps on exception
/// return.
#[test]
fn gate_sleeponexit() {
    let Some((outcome, reference, on)) = gate("la_sleeponexit", &["PA5", "PA6"], Time::from_ms(60))
    else {
        return;
    };
    assert!(
        reference.edges.iter().all(|e| e.1 == "PA5") && reference.edges.len() >= 10,
        "sleep-on-exit: only the ISR runs: {:?}",
        reference.edges
    );
    assert_eq!(outcome, Outcome::Equivalent);
    assert!(on.grants.len() < reference.grants.len());
}

/// WFE/SEV main loop.
#[test]
fn gate_wfe_sev() {
    let Some((outcome, reference, on)) = gate("la_wfe", &["PA5", "PA6"], Time::from_ms(60)) else {
        return;
    };
    let pa6 = reference.edges.iter().filter(|e| e.1 == "PA6").count();
    assert!(
        pa6 >= 10,
        "main's WFE must return after each ISR: {:?}",
        reference.edges
    );
    assert_eq!(outcome, Outcome::Equivalent);
    assert!(on.grants.len() < reference.grants.len());
}

/// Silent wakes and firmware accesses to the block.
#[test]
fn gate_silent_wakes_and_firmware_access() {
    let Some((outcome, reference, on)) = gate("la_silent", &["PA5", "PA6"], Time::from_ms(60))
    else {
        return;
    };
    assert!(
        reference.edges.iter().all(|e| e.1 == "PA5") && reference.edges.len() >= 5,
        "PA6 must stay low (the block reads 0 for firmware): {:?}",
        reference.edges
    );
    assert_eq!(outcome, Outcome::Equivalent);
    // One promise per silent wake at most: the grants stop at every 1 ms
    // SysTick limit.
    assert!(on.grants.len() < reference.grants.len());
    assert!(on.grants.iter().all(|(a, b)| *b - *a <= Time::from_ms(1)));
}

/// A promise stretched across a SysTick wake that changes no output: only
/// the wake audit can see it, and it must raise `PromiseBroken` at the wake.
#[test]
fn injected_early_wake_without_edge_is_promise_broken() {
    let Some(cfg) = config("la_silent", &["PA5", "PA6"]) else {
        return;
    };
    let cfg = with_mode(&cfg, Mode::On);
    let d = drive(
        &cfg,
        Time::from_ms(20),
        Some(Fault::StretchPromise { ns: 3_000_000 }),
    );
    let (from, err) = d.err.expect("the stretched promise must be caught");
    eprintln!("injected early wake: after {from}: {err}");
    let EngineError::PromiseBroken { promised, at } = err else {
        panic!("expected PromiseBroken, got {err}")
    };
    // The first promise (the CPU asleep after booting) ends at the 1 ms
    // SysTick; stretched by 3 ms, the grant reaches ~4 ms and the audit
    // sees the wake at ~1 ms. PA5 first toggles at 10 ms: no edge.
    assert_eq!(d.promises.len(), 1, "{:?}", d.promises);
    assert_eq!(d.promises[0].1, promised);
    assert!(from < Time::from_us(100), "{from}");
    assert!(
        at >= Time::from_us(990) && at <= Time::from_us(1010) && at < promised,
        "wake at {at}, promised {promised}"
    );
    assert!(d.edges.is_empty(), "{:?}", d.edges);
}

/// A failed self-test (a host backend on a UART) gives no promise: the run
/// is the one-cell reference, and only the grant sequence differs from a
/// run whose self-test passes.
#[test]
fn self_test_fallback_changes_only_grants() {
    let Some(cfg) = config("la_silent", &["PA5", "PA6"]) else {
        return;
    };
    let t_end = Time::from_ms(30);
    let pty = std::env::temp_dir().join(format!("bb-la-pty-{}", std::process::id()));
    let mut failing = with_mode(&cfg, Mode::On);
    failing.extra_monitor = vec![
        format!(
            "emulation CreateUartPtyTerminal \"bbterm\" \"{}\" true",
            pty.display()
        ),
        "connector Connect sysbus.usart1 bbterm".into(),
    ];
    let reference = drive(&with_mode(&cfg, Mode::Reference), t_end, None);
    let fallback = drive(&failing, t_end, None);
    let on = drive(&with_mode(&cfg, Mode::On), t_end, None);
    let _ = std::fs::remove_file(&pty);
    let st = fallback.startup.expect("plugin loaded");
    eprintln!(
        "self-test fallback: STATUS {:#x}, reasons {:#x}; grants: reference {}, fallback {}, on {}",
        st.status,
        st.self_test_failures(),
        reference.grants.len(),
        fallback.grants.len(),
        on.grants.len()
    );
    assert!(st.armed() && !st.self_test_ok());
    assert_eq!(st.self_test_failures(), 0x20, "host backend reason");
    assert!(fallback.err.is_none() && on.err.is_none());
    assert!(fallback.promises.is_empty() && fallback.stats.reads == 0);
    assert_eq!(fallback.edges, reference.edges);
    assert_eq!(fallback.grants, reference.grants);
    assert_eq!(on.edges, reference.edges);
    assert_ne!(on.grants, fallback.grants);
}

/// The first start compiles the block with Renode's compiler and caches the
/// assembly; later starts load the cached assembly.
#[test]
fn plugin_is_compiled_once_and_cached() {
    let Some(cfg) = config("la_silent", &["PA5"]) else {
        return;
    };
    let dir = std::env::temp_dir().join(format!("bb-la-cache-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut c = with_mode(&cfg, Mode::On);
    c.lookahead.cache_dir = Some(dir.clone());
    let t0 = Instant::now();
    let first = RenodeEngine::start(&c).expect("start (compile)");
    let compile_s = t0.elapsed().as_secs_f64();
    drop(first);
    let cached: Vec<_> = std::fs::read_dir(&dir)
        .expect("cache directory created")
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(cached.len(), 1, "{cached:?}");
    assert!(cached[0].starts_with("bb_lookahead-") && cached[0].ends_with(".dll"));
    let t0 = Instant::now();
    let mut second = RenodeEngine::start(&c).expect("start (cached)");
    let cached_s = t0.elapsed().as_secs_f64();
    let st = second.lookahead_query().unwrap();
    assert!(st.armed() && st.self_test_ok(), "{st:?}");
    eprintln!("plugin start: compile {compile_s:.2} s, cached {cached_s:.2} s");
    drop(second);
    let _ = std::fs::remove_dir_all(&dir);
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

/// Startup-cost gate (ADR-003 step 5): with the cached plugin, engine
/// startup is within 0.3 s of a start without it (measurement protocol:
/// interleaved, N runs each (`BB_LA_STARTUP_N`, default 7), medians).
/// Ignored by default (wall-clock): `make lookahead-startup`.
#[test]
#[ignore]
fn plugin_startup_overhead() {
    let Some(mut cfg) = config("la_silent", &["PA5"]) else {
        return;
    };
    // A fresh-start gate: never lease from a pool (ADR-004).
    cfg.pool = None;
    let n: usize = std::env::var("BB_LA_STARTUP_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(7);
    let with = with_mode(&cfg, Mode::ArmedOff);
    let without = with_mode(&cfg, Mode::Reference);
    drop(RenodeEngine::start(&with).expect("warm the plugin cache"));
    let (mut a, mut b) = (Vec::new(), Vec::new());
    for i in 0..n {
        for (plugin, v) in [(false, &mut a), (true, &mut b)] {
            let c = if plugin { &with } else { &without };
            let t0 = Instant::now();
            let e = RenodeEngine::start(c).expect("start");
            let s = t0.elapsed().as_secs_f64();
            drop(e);
            eprintln!("run {i}: plugin {plugin}: startup {s:.3} s");
            v.push(s);
        }
    }
    let load = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
    let (ma, mb) = (median(&mut a), median(&mut b));
    eprintln!(
        "startup median: without plugin {ma:.3} s, with plugin {mb:.3} s, difference {:.3} s \
         (N = {n}, load {})",
        mb - ma,
        load.trim()
    );
    assert!(
        mb - ma <= 0.3,
        "plugin startup overhead {:.3} s > 0.3 s",
        mb - ma
    );
}
