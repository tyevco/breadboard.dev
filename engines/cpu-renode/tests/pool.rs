//! The warm Renode pool (ADR-004 Part B) against a real Renode: gate G5.
//!
//! A pool lease gives the same edges as a fresh Renode; a lease whose
//! Renode is killed, or that ends with a protocol error or a broken
//! promise, discards the process and the next lease gets a fresh
//! pre-warmed one with the same results; `max_runs` recycles; a busy pool
//! or one that does not match the Renode or the plugin is not used; no
//! External Control listener is left after a lease (Amendment 2).
//!
//! Needs Renode and arm-none-eabi-gcc like `blinky.rs` and `lookahead.rs`
//! (`BB_REQUIRE_RENODE=1`: fail instead of skip).

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use bb_cpu_renode::manager::{Manager, PoolConfig};
use bb_cpu_renode::pool::{Lease, PoolKey};
use bb_cpu_renode::{
    find_renode, Fault, PinDirection, PinSpec, RenodeConfig, RenodeEngine, Verify,
};
use bbcore::{Engine, EngineError, PinSignal, Time};

const L: Time = Time::from_us(10);
const WARM: Duration = Duration::from_secs(300);

static BUILD: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn skip(msg: &str) {
    let required = std::env::var("BB_REQUIRE_RENODE").is_ok_and(|v| v == "1");
    assert!(!required, "BB_REQUIRE_RENODE=1 but {msg}");
    eprintln!("SKIP: {msg}");
}

fn build(dir: &str, elf: &str) -> Option<PathBuf> {
    let _g = BUILD.lock().unwrap_or_else(|e| e.into_inner());
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(dir);
    let ok = Command::new("make")
        .args(["-s", "-C"])
        .arg(&dir)
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        skip("building the firmware failed (run `make setup`)");
        return None;
    }
    Some(dir.join("build").join(elf))
}

/// A pool of `slots` processes in a private temporary directory, warm.
fn pool(tag: &str, slots: usize, max_runs: u32) -> Option<Manager> {
    let Some(renode) = find_renode() else {
        skip("renode not installed (run `make setup`, or set BB_RENODE)");
        return None;
    };
    let dir = std::env::temp_dir().join(format!("bb-pool-{tag}-{}", std::process::id()));
    let mut cfg = PoolConfig::new(renode, dir);
    cfg.slots = slots;
    cfg.max_runs = max_runs;
    let m = Manager::start(cfg).expect("start the pool");
    m.wait_warm(WARM).expect("pre-warm");
    Some(m)
}

/// The spike blinky on PA5, fresh (no pool) unless `pool` is given.
fn blinky(pool: Option<&Manager>) -> Option<RenodeConfig> {
    let elf = build("../../examples/spike/firmware", "blinky.elf")?;
    let mut cfg = RenodeConfig::stm32f103(find_renode()?, elf);
    cfg.quantum = L;
    cfg.pool = pool.map(|m| m.socket().to_path_buf());
    Some(cfg)
}

#[derive(Debug, Default)]
struct Run {
    edges: Vec<(Time, String, bool)>,
    err: Option<EngineError>,
    leased: bool,
    pid: u32,
    promises: u32,
}

/// Run `cfg` to `t_end` in 1 ms grants (or by the lookahead rule when it
/// is enabled); `kill_at`: SIGKILL the Renode process before that grant.
fn drive(cfg: &RenodeConfig, t_end: Time, fault: Option<Fault>, kill_at: Option<Time>) -> Run {
    let mut e = RenodeEngine::start(cfg).expect("start renode");
    if let Some(f) = fault {
        e.inject_fault(f);
    }
    let mut r = Run {
        leased: e.leased(),
        pid: e.renode_pid(),
        ..Run::default()
    };
    eprintln!(
        "drive: leased {} pid {} lookahead startup {:?}",
        r.leased,
        r.pid,
        e.lookahead_startup()
    );
    let mut now = Time::ZERO;
    while now < t_end {
        e.next_safe_time(now);
        let q = e.quiet_until(now);
        if q > now {
            r.promises += 1;
        }
        // Lookahead runs use one-cell grants unless the CPU promises
        // quiet (as the kernel does on a feedback loop); others 1 ms.
        let cap = now
            + if cfg.lookahead.enabled {
                L
            } else {
                Time::from_ms(1)
            };
        let g = if q > now {
            q.min(Time::from_ms(10) + now)
        } else {
            cap
        };
        let g = Time::from_fs(g.min(t_end).as_fs() / L.as_fs() * L.as_fs()).max(now + L);
        if kill_at.is_some_and(|k| g >= k) {
            let _ = Command::new("kill")
                .args(["-9", &r.pid.to_string()])
                .status();
            std::thread::sleep(Duration::from_millis(200));
        }
        if let Err(err) = e.advance_to(g) {
            r.err = Some(err);
            break;
        }
        for tr in e.read_output_trace().unwrap().unwrap_or_default() {
            let PinSignal::Digital { level, .. } = tr.signal else {
                panic!("analog output from renode")
            };
            r.edges.push((tr.t, tr.pin, level == bbcore::Level::High));
        }
        now = g;
    }
    if r.err.is_none() {
        e.end_of_run().expect("end-of-run verify");
    }
    r
}

const T_END: Time = Time::from_ms(250);

fn fresh_reference() -> Option<Run> {
    let r = drive(&blinky(None)?, T_END, None, None);
    assert!(r.err.is_none() && !r.leased, "{:?}", r.err);
    assert!(r.edges.len() >= 4, "{:?}", r.edges);
    Some(r)
}

/// A clean lease: leased, same edges as fresh.
fn clean_lease(m: &Manager, reference: &Run) -> Run {
    m.wait_warm(WARM).expect("warm");
    let r = drive(&blinky(Some(m)).unwrap(), T_END, None, None);
    assert!(r.leased, "the run did not lease from the pool");
    assert!(r.err.is_none(), "{:?}", r.err);
    assert_eq!(r.edges, reference.edges, "pooled edges differ from fresh");
    r
}

#[test]
fn leases_match_fresh_and_recycle_after_max_runs() {
    let Some(m) = pool("recycle", 1, 2) else {
        return;
    };
    let Some(reference) = fresh_reference() else {
        return;
    };
    let a = clean_lease(&m, &reference);
    let b = clean_lease(&m, &reference);
    assert_eq!(a.pid, b.pid, "the second lease reuses the process");
    let c = clean_lease(&m, &reference);
    assert_ne!(
        b.pid, c.pid,
        "max_runs 2: the third lease gets a new process"
    );
    let (slots, st) = m.status();
    assert_eq!(
        (st.leases, st.kept, st.recycled, st.discarded),
        (3, 2, 1, 0),
        "{st:?}"
    );
    assert_eq!(st.leaked_listeners, 0, "{st:?}");
    // No machine and no listener in an idle pool process.
    m.wait_warm(WARM).unwrap();
    let pid = m.status().0[0].pid.unwrap();
    assert!(bb_cpu_renode::console::listening_ports(pid).is_empty());
    assert_eq!(slots.len(), 1);
    m.shutdown();
}

#[test]
fn killed_renode_is_discarded() {
    let Some(m) = pool("kill", 1, 100) else {
        return;
    };
    let Some(reference) = fresh_reference() else {
        return;
    };
    let r = drive(
        &blinky(Some(&m)).unwrap(),
        T_END,
        None,
        Some(Time::from_ms(20)),
    );
    assert!(r.leased);
    assert!(r.err.is_some(), "a killed Renode must fail the run");
    let next = clean_lease(&m, &reference);
    assert_ne!(next.pid, r.pid, "the killed process is replaced");
    let (_, st) = m.status();
    assert_eq!((st.discarded, st.kept), (1, 1), "{st:?}");
    m.shutdown();
}

#[test]
fn protocol_error_is_discarded() {
    let Some(m) = pool("proto", 1, 100) else {
        return;
    };
    let Some(reference) = fresh_reference() else {
        return;
    };
    let mut cfg = blinky(Some(&m)).unwrap();
    cfg.verify = Verify::Every;
    let fault = Fault::RunForExtraNs {
        at: Time::from_ms(5),
        ns: 10_000,
    };
    let r = drive(&cfg, T_END, Some(fault), None);
    assert!(r.leased);
    assert!(
        matches!(r.err, Some(EngineError::Protocol(_))),
        "expected a protocol error, got {:?}",
        r.err
    );
    let next = clean_lease(&m, &reference);
    assert_ne!(
        next.pid, r.pid,
        "the process of a failed lease is never reused"
    );
    assert_eq!(m.status().1.discarded, 1);
    m.shutdown();
}

#[test]
fn broken_promise_is_discarded() {
    let Some(m) = pool("promise", 1, 100) else {
        return;
    };
    assert!(m.plugin_loaded(), "the pool loads the lookahead plugin");
    let Some(elf) = build("tests/firmware", "la_silent.elf") else {
        return;
    };
    let mut cfg = RenodeConfig::stm32f103(find_renode().unwrap(), elf);
    cfg.quantum = L;
    cfg.pins = ["PA5", "PA6"]
        .iter()
        .map(|p| PinSpec::stm32(p, PinDirection::Output).unwrap())
        .collect();
    cfg.lookahead.plugin = true;
    cfg.lookahead.enabled = true;
    cfg.pool = Some(m.socket().to_path_buf());
    let r = drive(
        &cfg,
        Time::from_ms(20),
        Some(Fault::StretchPromise { ns: 3_000_000 }),
        None,
    );
    assert!(r.promises >= 1, "no promise given");
    assert!(
        r.leased,
        "a plugin run leases when the pool loaded the plugin"
    );
    assert!(
        matches!(r.err, Some(EngineError::PromiseBroken { .. })),
        "expected PromiseBroken, got {:?}",
        r.err
    );
    // The same board without the fault: leased from a new process, and
    // equal to a fresh run.
    m.wait_warm(WARM).unwrap();
    let ok = drive(&cfg, Time::from_ms(20), None, None);
    assert!(ok.leased && ok.err.is_none(), "{:?}", ok.err);
    assert_ne!(ok.pid, r.pid);
    let mut fresh_cfg = cfg.clone();
    fresh_cfg.pool = None;
    let fresh = drive(&fresh_cfg, Time::from_ms(20), None, None);
    assert!(!fresh.leased);
    assert_eq!(ok.edges, fresh.edges);
    assert_eq!(m.status().1.discarded, 1);
    m.shutdown();
}

#[test]
fn busy_or_mismatched_pool_is_not_used() {
    let Some(m) = pool("match", 1, 100) else {
        return;
    };
    let renode = find_renode().unwrap();
    // Another Renode launcher: no match.
    let key = PoolKey {
        renode: "/nonexistent/renode-1.17.0/renode".into(),
        plugin: None,
    };
    assert!(Lease::acquire(m.socket(), &key, Duration::from_secs(10))
        .unwrap()
        .is_none());
    // A plugin cached elsewhere: no match, so the engine starts fresh.
    let Some(elf) = build("tests/firmware", "la_silent.elf") else {
        return;
    };
    let mut cfg = RenodeConfig::stm32f103(renode, elf);
    cfg.pool = Some(m.socket().to_path_buf());
    cfg.lookahead.plugin = true;
    let dir = std::env::temp_dir().join(format!("bb-pool-cache-{}", std::process::id()));
    cfg.lookahead.cache_dir = Some(dir.clone());
    let e = RenodeEngine::start(&cfg).expect("start");
    assert!(!e.leased(), "a plugin from another cache must not lease");
    drop(e);
    let _ = std::fs::remove_dir_all(&dir);
    // Busy: with the only process leased, a second engine starts fresh.
    cfg.lookahead.cache_dir = None;
    let a = RenodeEngine::start(&cfg).expect("start a");
    assert!(a.leased());
    let b = RenodeEngine::start(&cfg).expect("start b");
    assert!(!b.leased(), "a busy pool is never waited for");
    drop(b);
    drop(a);
    let (_, st) = m.status();
    assert_eq!(
        (st.nomatch, st.busy, st.leases, st.kept),
        (2, 1, 1, 1),
        "{st:?}"
    );
    // A socket without a pool: fresh.
    let mut gone = blinky(None).unwrap();
    gone.pool = Some(std::env::temp_dir().join("bb-no-such-pool.sock"));
    let e = RenodeEngine::start(&gone).expect("start without a pool");
    assert!(!e.leased());
    drop(e);
    m.shutdown();
}
