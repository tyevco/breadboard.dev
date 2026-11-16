//! ADR-015 D5 / G4 (Phase 2 H3): External Control exposure.
//!
//! Renode connects out to `bb`'s listener (`CreateExternalControlClient`),
//! so nothing listens on Renode's side, and `bb`'s listener admits only the
//! connection whose socket belongs to its own Renode process.
//!
//! For each of N = 20 engine starts (4 fresh, run in parallel, and 16
//! leased from a warm two-process pool), a **foreign** client (a
//! separate `bash` process with `/dev/tcp`) connects to `bb`'s listener
//! before Renode does: before Renode is spawned (or the leased process runs
//! the script), and again before every reconnect (forced every few
//! messages). Each run must
//!
//! - refuse every foreign client, which reads no byte (`head -c 1` gets
//!   EOF): exactly one refusal per connection the engine opened;
//! - still start with its own Renode and give the same PA5 edges as a
//!   reference run without foreign clients or reconnects;
//! - see Renode listen on no TCP port (`/proc/net/tcp{,6}` against the
//!   process's fds: the exact form of a port scan) after start, after every
//!   grant (so across every reconnect) and at the end.
//!
//! Needs Renode and arm-none-eabi-gcc like `pool.rs` (`BB_REQUIRE_RENODE=1`:
//! fail instead of skip).

use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bb_cpu_renode::console::listening_ports;
use bb_cpu_renode::manager::{Manager, PoolConfig};
use bb_cpu_renode::{ec, find_renode, EcHook, EcPhase, RenodeConfig, RenodeEngine};
use bbcore::{Engine, PinSignal, Time};

const L: Time = Time::from_us(10);
const T_END: Time = Time::from_ms(60);
const WARM: Duration = Duration::from_secs(300);
/// Fresh starts (in parallel) and pooled starts: N = FRESH + POOLED = 20.
/// A fresh Renode's startup is CPU-bound (about 1.4 s alone, several
/// seconds when ten start at once), a lease's about 0.1 s, so most of the
/// N are leases; both paths are covered.
const FRESH: usize = 4;
const POOLED: usize = 16;
/// Pool processes (pooled runs go this many at a time).
const SLOTS: usize = 2;
/// A reconnect every few grants.
const EC_MESSAGES: u64 = 8;

static BUILD: Mutex<()> = Mutex::new(());

fn skip(msg: &str) {
    let required = std::env::var("BB_REQUIRE_RENODE").is_ok_and(|v| v == "1");
    assert!(!required, "BB_REQUIRE_RENODE=1 but {msg}");
    eprintln!("SKIP: {msg}");
}

fn blinky_elf() -> Option<PathBuf> {
    let _g = BUILD.lock().unwrap_or_else(|e| e.into_inner());
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/spike/firmware");
    let ok = Command::new("make")
        .args(["-s", "-C"])
        .arg(&dir)
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        skip("building the firmware failed (run `make setup`)");
        return None;
    }
    Some(dir.join("build/blinky.elf"))
}

fn config(pool: Option<&Manager>) -> Option<RenodeConfig> {
    let Some(renode) = find_renode() else {
        skip("renode not installed (run `make setup`, or set BB_RENODE)");
        return None;
    };
    let mut cfg = RenodeConfig::stm32f103(renode, blinky_elf()?);
    cfg.quantum = L;
    cfg.pool = pool.map(|m| m.socket().to_path_buf());
    Some(cfg)
}

/// The foreign clients one run started, by phase.
type Foreign = Arc<Mutex<Vec<(EcPhase, Child)>>>;

/// A hook that connects a foreign process to the listener and returns only
/// once its connection is established, so it is queued before Renode's.
fn foreign_hook(foreign: &Foreign) -> EcHook {
    let f = Arc::clone(foreign);
    EcHook(Arc::new(move |phase, port| {
        let child = Command::new("bash")
            .args([
                "-c",
                &format!("exec 3<>/dev/tcp/127.0.0.1/{port}; head -c 1 <&3 | wc -c"),
            ])
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn the foreign client");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ec::connected_to(child.id(), port) {
            assert!(
                Instant::now() < deadline,
                "the foreign client did not connect ({phase:?})"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        f.lock().unwrap().push((phase, child));
    }))
}

#[derive(Debug, PartialEq)]
struct Outcome {
    edges: Vec<(Time, bool)>,
    leased: bool,
    connections: u64,
    refused: u64,
}

/// Run `cfg` to [`T_END`] in 5 ms grants, checking after start, after
/// every grant and at the end that Renode listens on no port.
fn drive(cfg: &RenodeConfig) -> Outcome {
    let mut e = RenodeEngine::start(cfg).expect("start renode");
    let pid = e.renode_pid();
    let no_listener = |when: &str| {
        let ports = listening_ports(pid);
        assert!(
            ports.is_empty(),
            "Renode (pid {pid}) listens on {ports:?} {when}"
        );
    };
    no_listener("after start");
    let mut edges = Vec::new();
    let mut now = Time::ZERO;
    while now < T_END {
        let g = (now + Time::from_ms(5)).min(T_END);
        e.next_safe_time(now);
        e.advance_to(g).expect("advance");
        for tr in e.read_output_trace().unwrap().unwrap_or_default() {
            let PinSignal::Digital { level, .. } = tr.signal else {
                panic!("analog output from renode")
            };
            edges.push((tr.t, level == bbcore::Level::High));
        }
        no_listener(&format!("at {g}"));
        now = g;
    }
    e.end_of_run().expect("end-of-run verify");
    no_listener("at the end");
    Outcome {
        edges,
        leased: e.leased(),
        connections: 1 + e.ec_reconnects(),
        refused: e.ec_refused(),
    }
}

/// One run with foreign clients; returns the outcome and how many foreign
/// clients were refused without reading a byte.
fn attacked(base: &RenodeConfig) -> (Outcome, usize) {
    let foreign: Foreign = Arc::default();
    let mut cfg = base.clone();
    cfg.ec_messages_per_connection = EC_MESSAGES;
    cfg.ec_hook = Some(foreign_hook(&foreign));
    let out = drive(&cfg);
    let mut clean = 0;
    for (phase, mut c) in std::mem::take(&mut *foreign.lock().unwrap()) {
        let mut s = String::new();
        c.stdout.take().unwrap().read_to_string(&mut s).unwrap();
        assert!(c.wait().unwrap().success());
        assert_eq!(s.trim(), "0", "the foreign client of {phase:?} read a byte");
        clean += 1;
    }
    (out, clean)
}

fn check(kind: &str, i: usize, reference: &Outcome, out: &Outcome, clean: usize) {
    assert!(
        out.connections >= 3,
        "{kind} {i}: too few reconnects: {out:?}"
    );
    assert_eq!(
        out.refused, out.connections,
        "{kind} {i}: one foreign client per connection, each refused"
    );
    assert_eq!(clean as u64, out.connections, "{kind} {i}: foreign clients");
    assert_eq!(out.edges, reference.edges, "{kind} {i}: edges differ");
    eprintln!(
        "ec-exposure: {kind} run {i}: {} connections, {} foreign clients refused, \
         none read a byte; Renode listened on no port; edges as the reference",
        out.connections, out.refused
    );
}

/// Join a run's thread; its panic fails the test with its message.
fn join<T>(h: std::thread::ScopedJoinHandle<'_, T>) -> T {
    match h.join() {
        Ok(v) => v,
        Err(p) => std::panic::resume_unwind(p),
    }
}

#[test]
fn foreign_clients_are_refused_fresh_and_pooled() {
    let Some(base) = config(None) else {
        return;
    };
    let t0 = Instant::now();
    // The pool warms in the background meanwhile.
    let dir = std::env::temp_dir().join(format!("bb-pool-ecx-{}", std::process::id()));
    let mut pcfg = PoolConfig::new(base.renode.clone(), dir);
    pcfg.slots = SLOTS;
    let m = Manager::start(pcfg).expect("start the pool");

    let mut pooled = base.clone();
    pooled.pool = Some(m.socket().to_path_buf());
    // Everything at once: the reference, FRESH fresh engines (each thread
    // owns its Renode) and POOLED leases, SLOTS at a time once the pool is
    // warm. The outcomes are compared at the end.
    let (base, pooled, mr) = (&base, &pooled, &m);
    let (reference, runs) = std::thread::scope(|s| {
        let reference = s.spawn(move || drive(base));
        let fresh: Vec<_> = (0..FRESH)
            .map(|i| s.spawn(move || vec![("fresh", i, attacked(base))]))
            .collect();
        let leased: Vec<_> = (0..SLOTS)
            .map(|k| {
                s.spawn(move || {
                    mr.wait_warm(WARM).expect("pre-warm");
                    (k..POOLED)
                        .step_by(SLOTS)
                        .map(|i| {
                            mr.wait_warm(WARM).expect("warm");
                            ("pooled", i, attacked(pooled))
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let reference = join(reference);
        let runs: Vec<_> = fresh.into_iter().chain(leased).flat_map(join).collect();
        (reference, runs)
    });
    eprintln!(
        "ec-exposure: runs done at {:.1} s",
        t0.elapsed().as_secs_f64()
    );
    assert_eq!(reference.refused, 0);
    assert!(!reference.edges.is_empty(), "{reference:?}");
    assert_eq!(runs.len(), FRESH + POOLED);
    for (kind, i, (out, clean)) in &runs {
        assert_eq!(
            out.leased,
            *kind == "pooled",
            "{kind} run {i}: leased {}",
            out.leased
        );
        check(kind, *i, &reference, out, *clean);
    }
    let (_, st) = m.status();
    assert_eq!(st.leases, POOLED as u64, "{st:?}");
    assert_eq!(st.discarded, 0, "{st:?}");
    assert_eq!(st.leaked_listeners, 0, "{st:?}");
    m.shutdown();
    eprintln!(
        "ec-exposure: PASS: {n}/{n} starts refused their foreign client (and every reconnect's)",
        n = FRESH + POOLED
    );
}
