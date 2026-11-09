//! The warm Renode pool manager (ADR-004 Part B): what `bb pool` runs.
//!
//! The manager owns [`PoolConfig::slots`] Renode processes, each driven
//! through its console pipe ([`crate::console`]) by a long-lived slot
//! thread, and serves leases ([`crate::pool::Lease`]) over a Unix socket in
//! a private (0700) directory. It is not an engine: it holds no simulated
//! time, and between leases its processes hold no machine.
//!
//! - **Spawn and pre-warm** (slot thread): Renode with ADR-004 Part A and
//!   kill-on-parent-death (the slot thread lives as long as the manager);
//!   the cached lookahead assembly is loaded once (compiled and cached
//!   first if needed); then a throwaway machine of each platform `bb`
//!   knows (the offline STM32F103, the stock STM32F072) runs a few lattice
//!   cells of a `wfi` loop over External Control and is cleared. Every
//!   lease thus sees the same type registry, and the first one is warm.
//! - **Lease** (connection thread): `Clear`, `logLevel 1` (Renode's
//!   default) and `include @<the run's session.resc>`, with the console's
//!   output copied into the run's `renode.log`. At the end: wait until
//!   Renode has re-opened its External Control listener after the run's
//!   disconnect, `Clear`, and check that the process listens on no port
//!   (ADR-004 Amendment 2: Renode 1.17.0 leaks the listener when `Clear`
//!   races the re-listen).
//! - **Discard** (kill, and the slot thread spawns a pre-warmed
//!   replacement): a lease that ended with an error or without `END ok`,
//!   a console that fails or stops answering, a process that exited, a
//!   listener left after `Clear`, more than [`PoolConfig::rss_limit`]
//!   resident, or [`PoolConfig::max_runs`] leases served (recycle).

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::console::{Console, COMMAND_ERROR};
use crate::pool::{canonical_renode, PROTOCOL};
use crate::protocol::Client;
use crate::{
    free_port, plugin_cache_path, write_plugin_loader, LOOKAHEAD_ADDRESS, LOOKAHEAD_NAME,
    STM32F103_OFFLINE_REPL,
};

/// Configuration of a [`Manager`].
#[derive(Clone, Debug)]
pub struct PoolConfig {
    /// The `renode` launcher. Leases must name the same canonical path.
    pub renode: PathBuf,
    /// Number of Renode processes.
    pub slots: usize,
    /// Leases a process serves before it is replaced (recycle).
    pub max_runs: u32,
    /// Resident set size above which a process is replaced after a lease.
    pub rss_limit: u64,
    /// Private directory for the socket and the pool's files (created
    /// 0700; see [`private_dir`]).
    pub dir: PathBuf,
    /// Load the lookahead plugin at pre-warm, so leases of boards that use
    /// it match. Off: such leases get `NOMATCH` and start fresh.
    pub plugin: bool,
    /// Where the plugin is cached (`None`: the engine's default, so that
    /// the pool matches `bb run`).
    pub plugin_cache_dir: Option<PathBuf>,
    /// Timeout of one console batch (pre-warm, a lease's include, Clear).
    pub timeout: Duration,
}

impl PoolConfig {
    /// One slot, recycle after 100 leases or 2 GiB, plugin loaded.
    pub fn new(renode: PathBuf, dir: PathBuf) -> PoolConfig {
        PoolConfig {
            renode,
            slots: 1,
            max_runs: 100,
            rss_limit: 2 << 30,
            dir,
            plugin: true,
            plugin_cache_dir: None,
            timeout: Duration::from_secs(120),
        }
    }
}

/// The default pool directory: `$XDG_RUNTIME_DIR/bb`, else
/// `$BB_CACHE_DIR/pool`, else `$XDG_CACHE_HOME/bb/pool`, else
/// `~/.cache/bb/pool`.
pub fn default_dir() -> Option<PathBuf> {
    let env = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    env("XDG_RUNTIME_DIR")
        .map(|d| d.join("bb"))
        .or_else(|| env("BB_CACHE_DIR").map(|d| d.join("pool")))
        .or_else(|| env("XDG_CACHE_HOME").map(|d| d.join("bb/pool")))
        .or_else(|| env("HOME").map(|d| d.join(".cache/bb/pool")))
}

extern "C" {
    fn getuid() -> u32;
}

/// Create `dir` with mode 0700 if missing; require that it is a directory
/// owned by this user, and make it 0700 if it is not.
pub fn private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let md = std::fs::metadata(dir)?;
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { getuid() };
    if !md.is_dir() || md.uid() != uid {
        return Err(std::io::Error::other(format!(
            "{} is not a directory owned by uid {uid}",
            dir.display()
        )));
    }
    if md.mode() & 0o077 != 0 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Counters of a pool (wall-clock free).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PoolStats {
    /// Processes started (pre-warmed) so far.
    pub spawned: u64,
    /// Leases granted.
    pub leases: u64,
    /// Leases after which the process was kept.
    pub kept: u64,
    /// Processes discarded after a failed or abnormal lease, a dead
    /// console or a leftover listener.
    pub discarded: u64,
    /// Processes replaced for `max_runs` or the RSS limit.
    pub recycled: u64,
    /// External Control listeners still open after a lease's `Clear`.
    pub leaked_listeners: u64,
    /// `BUSY` answers (no idle process).
    pub busy: u64,
    /// `NOMATCH` answers.
    pub nomatch: u64,
}

/// One slot's state as [`Manager::status`] reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotStatus {
    pub state: &'static str,
    pub pid: Option<u32>,
    /// Leases served by the current process.
    pub runs: u32,
}

enum Slot {
    Warming,
    Idle {
        con: Box<Console>,
        runs: u32,
    },
    Busy {
        pid: u32,
        runs: u32,
    },
    /// To be killed and replaced by the slot thread.
    Retire(Box<Console>),
    Failed(String),
}

struct Inner {
    cfg: PoolConfig,
    renode_key: String,
    /// The plugin assembly loaded at pre-warm (`None`: not loaded).
    plugin_key: Mutex<Option<String>>,
    plugin_cache: Option<PathBuf>,
    work: PathBuf,
    socket: PathBuf,
    slots: Mutex<Vec<Slot>>,
    cv: Condvar,
    stats: Mutex<PoolStats>,
    stop: AtomicBool,
    last_activity: Mutex<Instant>,
}

/// A running pool: the socket listener and the slot threads.
pub struct Manager {
    inner: Arc<Inner>,
    threads: Vec<JoinHandle<()>>,
}

impl Manager {
    /// Bind `<dir>/pool-<pid>.sock` and start the slot threads (which spawn
    /// and pre-warm in the background; [`Manager::wait_warm`]).
    pub fn start(cfg: PoolConfig) -> std::io::Result<Manager> {
        if cfg.slots == 0 {
            return Err(std::io::Error::other("a pool needs at least one slot"));
        }
        private_dir(&cfg.dir)?;
        let id = format!("pool-{}-{}", std::process::id(), next_id());
        let socket = cfg.dir.join(format!("{id}.sock"));
        let work = cfg.dir.join(format!("{id}.d"));
        let _ = std::fs::remove_file(&socket);
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work)?;
        std::fs::write(work.join("platform.repl"), STM32F103_OFFLINE_REPL)?;
        let plugin_cache = if cfg.plugin {
            let cache = plugin_cache_path(&cfg.renode, cfg.plugin_cache_dir.as_deref())
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            write_plugin_loader(&work, &cache, &work.join("bb_lookahead.err"))
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            Some(cache)
        } else {
            None
        };
        let listener = UnixListener::bind(&socket)?;
        let inner = Arc::new(Inner {
            renode_key: canonical_renode(&cfg.renode).to_string_lossy().into_owned(),
            plugin_key: Mutex::new(None),
            plugin_cache,
            slots: Mutex::new((0..cfg.slots).map(|_| Slot::Warming).collect()),
            cfg,
            work,
            socket,
            cv: Condvar::new(),
            stats: Mutex::new(PoolStats::default()),
            stop: AtomicBool::new(false),
            last_activity: Mutex::new(Instant::now()),
        });
        let mut threads = Vec::new();
        for i in 0..inner.cfg.slots {
            let inn = Arc::clone(&inner);
            threads.push(
                std::thread::Builder::new()
                    .name(format!("pool-slot-{i}"))
                    .spawn(move || slot_thread(&inn, i))?,
            );
        }
        let inn = Arc::clone(&inner);
        threads.push(
            std::thread::Builder::new()
                .name("pool-accept".into())
                .spawn(move || accept_loop(&inn, &listener))?,
        );
        Ok(Manager { inner, threads })
    }

    /// The socket to put in [`crate::pool::POOL_ENV`].
    pub fn socket(&self) -> &Path {
        &self.inner.socket
    }

    /// Wait until no slot is warming (each is idle, busy or failed).
    /// Returns the failures, if any.
    pub fn wait_warm(&self, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        let mut g = lock(&self.inner.slots);
        loop {
            let warming = g
                .iter()
                .any(|s| matches!(s, Slot::Warming | Slot::Retire(_)));
            if !warming {
                let failed: Vec<String> = g
                    .iter()
                    .filter_map(|s| match s {
                        Slot::Failed(e) => Some(e.clone()),
                        _ => None,
                    })
                    .collect();
                return if failed.is_empty() {
                    Ok(())
                } else {
                    Err(failed.join("\n"))
                };
            }
            let now = Instant::now();
            if now >= deadline {
                return Err("the pool is still warming".into());
            }
            g = self
                .inner
                .cv
                .wait_timeout(g, deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// The slots and counters.
    pub fn status(&self) -> (Vec<SlotStatus>, PoolStats) {
        status(&self.inner)
    }

    /// Whether the plugin was loaded at pre-warm.
    pub fn plugin_loaded(&self) -> bool {
        lock(&self.inner.plugin_key).is_some()
    }

    /// A `STOP` request arrived or [`Manager::shutdown`] was called.
    pub fn stopping(&self) -> bool {
        self.inner.stop.load(Ordering::SeqCst)
    }

    /// Time since the last lease ended or began.
    pub fn idle_for(&self) -> Duration {
        lock(&self.inner.last_activity).elapsed()
    }

    /// Block until a `STOP` request arrives, or `idle` passes without a
    /// lease (`None`: never).
    pub fn serve(&self, idle: Option<Duration>) {
        let mut g = lock(&self.inner.slots);
        while !self.stopping() {
            if idle.is_some_and(|d| self.idle_for() >= d && !busy(&g)) {
                break;
            }
            g = self
                .inner
                .cv
                .wait_timeout(g, Duration::from_secs(1))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// Stop: kill every process, remove the socket and the pool's files.
    pub fn shutdown(mut self) {
        self.stop_now();
    }

    fn stop_now(&mut self) {
        self.inner.stop.store(true, Ordering::SeqCst);
        self.inner.cv.notify_all();
        // Unblock accept().
        let _ = UnixStream::connect(&self.inner.socket);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
        let _ = std::fs::remove_file(&self.inner.socket);
        let _ = std::fs::remove_dir_all(&self.inner.work);
    }
}

impl Drop for Manager {
    fn drop(&mut self) {
        if !self.threads.is_empty() {
            self.stop_now();
        }
    }
}

fn next_id() -> u64 {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn busy(slots: &[Slot]) -> bool {
    slots.iter().any(|s| matches!(s, Slot::Busy { .. }))
}

fn status(inner: &Inner) -> (Vec<SlotStatus>, PoolStats) {
    let g = lock(&inner.slots);
    let slots = g
        .iter()
        .map(|s| match s {
            Slot::Warming => SlotStatus {
                state: "warming",
                pid: None,
                runs: 0,
            },
            Slot::Idle { con, runs } => SlotStatus {
                state: "idle",
                pid: Some(con.pid()),
                runs: *runs,
            },
            Slot::Busy { pid, runs } => SlotStatus {
                state: "busy",
                pid: Some(*pid),
                runs: *runs,
            },
            Slot::Retire(c) => SlotStatus {
                state: "retiring",
                pid: Some(c.pid()),
                runs: 0,
            },
            Slot::Failed(_) => SlotStatus {
                state: "failed",
                pid: None,
                runs: 0,
            },
        })
        .collect();
    (slots, lock(&inner.stats).clone())
}

/// Spawn, pre-warm, hand the process to the slot, wait for it to be
/// retired, repeat. This thread spawns every process of its slot, so a
/// process dies with it (`PR_SET_PDEATHSIG`) when the manager stops.
fn slot_thread(inner: &Inner, i: usize) {
    let mut backoff = Duration::from_secs(1);
    while !inner.stop.load(Ordering::SeqCst) {
        lock(&inner.slots)[i] = Slot::Warming;
        match warm_process(inner) {
            Ok(con) => {
                lock(&inner.stats).spawned += 1;
                lock(&inner.slots)[i] = Slot::Idle {
                    con: Box::new(con),
                    runs: 0,
                };
                inner.cv.notify_all();
                backoff = Duration::from_secs(1);
            }
            Err(e) => {
                eprintln!("bb pool: slot {i}: {e}");
                lock(&inner.slots)[i] = Slot::Failed(e);
                inner.cv.notify_all();
                let g = lock(&inner.slots);
                let _ = inner
                    .cv
                    .wait_timeout_while(g, backoff, |_| !inner.stop.load(Ordering::SeqCst));
                backoff = (backoff * 2).min(Duration::from_secs(30));
                continue;
            }
        }
        // Wait until the process is retired (or the pool stops).
        let mut g = lock(&inner.slots);
        loop {
            if matches!(g[i], Slot::Retire(_)) || inner.stop.load(Ordering::SeqCst) {
                break;
            }
            g = inner.cv.wait(g).unwrap_or_else(|e| e.into_inner());
        }
        let old = std::mem::replace(&mut g[i], Slot::Warming);
        drop(g);
        if let Slot::Retire(mut c) | Slot::Idle { con: mut c, .. } = old {
            c.kill();
        }
    }
    // Stopping: a busy process dies with this thread.
}

/// A fresh Renode, pre-warmed (see the module docs).
fn warm_process(inner: &Inner) -> Result<Console, String> {
    let t = inner.cfg.timeout;
    let mut con = Console::spawn(&inner.cfg.renode, &inner.work, t)
        .map_err(|e| format!("spawn {}: {e}", inner.cfg.renode.display()))?;
    let ok = |out: &str, what: &str| -> Result<(), String> {
        if out.contains(COMMAND_ERROR) {
            return Err(format!("pre-warm {what}: {out}"));
        }
        Ok(())
    };
    let mut plugin = false;
    if let Some(cache) = &inner.plugin_cache {
        let loader = inner.work.join("bb_lookahead_load.py");
        match con.run(&[&format!("include @{}", loader.display())], t) {
            Ok(out) if !out.contains(COMMAND_ERROR) => {
                plugin = true;
                *lock(&inner.plugin_key) = Some(cache.to_string_lossy().into_owned());
            }
            r => {
                let err = std::fs::read_to_string(inner.work.join("bb_lookahead.err"))
                    .unwrap_or_default();
                eprintln!(
                    "bb pool: loading the lookahead plugin failed ({}); \
                     runs that use it start Renode fresh",
                    if err.is_empty() {
                        format!("{r:?}")
                    } else {
                        err
                    }
                );
                *lock(&inner.plugin_key) = None;
                if con.exited() {
                    return Err("renode exited while loading the lookahead plugin".into());
                }
            }
        }
    }
    let f103 = inner.work.join("platform.repl");
    let platforms = [
        (f103.to_string_lossy().into_owned(), false),
        ("platforms/cpus/stm32f072.repl".to_string(), true),
    ];
    for (repl, has_adc) in &platforms {
        let port = free_port().map_err(|e| e.to_string())?;
        let mut lines = vec![
            "Clear".to_string(),
            "logLevel 1".to_string(),
            "mach create \"bbwarm\"".to_string(),
            format!("machine LoadPlatformDescription @{repl}"),
        ];
        if plugin {
            lines.push(format!(
                "machine LoadPlatformDescriptionFromString \"{LOOKAHEAD_NAME}: \
                 Miscellaneous.BbLookahead @ sysbus {LOOKAHEAD_ADDRESS:#x}\""
            ));
        }
        // A vector table and `wfi; b .-2` in flash: the CPU sleeps.
        lines.extend(
            [
                "sysbus WriteDoubleWord 0x08000000 0x20002000",
                "sysbus WriteDoubleWord 0x08000004 0x08000009",
                "sysbus WriteDoubleWord 0x08000008 0xE7FDBF30",
                "cpu VectorTableOffset 0x08000000",
                "emulation SetSeed 0",
                "emulation SetGlobalQuantum \"0.000010000\"",
                "emulation SetGlobalSerialExecution true",
                "emulation SetGlobalAdvanceImmediately true",
            ]
            .map(String::from),
        );
        if plugin {
            lines.push(format!("sysbus.{LOOKAHEAD_NAME} Arm"));
        }
        lines.push("logLevel 2".into());
        lines.push(format!(
            "emulation CreateExternalControlServer \"bb\" {port}"
        ));
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let out = con.run(&refs, t).map_err(|e| e.to_string())?;
        ok(&out, repl)?;
        warm_external_control(port, *has_adc, plugin, t)
            .map_err(|e| format!("pre-warm {repl}: {e}"))?;
        end_session(&mut con, port, t)?;
    }
    let out = con.run(&["logLevel 1"], t).map_err(|e| e.to_string())?;
    ok(&out, "logLevel")?;
    Ok(con)
}

/// The External Control calls an engine makes, once, so they are compiled.
fn warm_external_control(
    port: u16,
    adc: bool,
    plugin: bool,
    t: Duration,
) -> Result<(), crate::protocol::ProtocolError> {
    let s = TcpStream::connect(("127.0.0.1", port))?;
    let mut c = Client::handshake(s, t)?;
    let md = c.get_machine("bbwarm")?;
    let gpio = c.get_gpio(md, "gpioPortA")?;
    let _ = c.gpio_get_state(gpio, 5)?;
    c.gpio_register_event(gpio, 5, 0)?;
    c.gpio_set_state(gpio, 0, true)?;
    if adc {
        let a = c.get_adc(md, "adc")?;
        if c.adc_channel_count(a)? > 1 {
            c.adc_set_value(a, 1, 1_000_000)?;
            let _ = c.adc_get_value(a, 1)?;
        }
    }
    let sb = c.get_sysbus(md, "sysbus")?;
    if plugin {
        let _ = c.sysbus_read_u32s(sb, LOOKAHEAD_ADDRESS, 8)?;
    }
    for _ in 0..5 {
        c.run_for(10_000)?;
    }
    let _ = c.get_time()?;
    let _ = c.take_events();
    c.close();
    Ok(())
}

/// After the client of the External Control server on `port` disconnected:
/// wait for Renode to listen again, `Clear`, and require that no listener
/// is left (ADR-004 Amendment 2).
fn end_session(con: &mut Console, port: u16, t: Duration) -> Result<(), String> {
    // Renode 1.17.0 restarts the server after a disconnect
    // (`ExternalControlSocket.Disconnect(Unconnected)`). A Clear that
    // arrives while it does so leaves a listener no one closes; one that
    // finds the server back in Accept closes it.
    wait(Duration::from_secs(2), || {
        con.listening_ports().contains(&port)
    });
    let out = con
        .run(&["Clear"], t.min(Duration::from_secs(20)))
        .map_err(|e| e.to_string())?;
    if out.contains(COMMAND_ERROR) {
        return Err(format!("Clear: {out}"));
    }
    if !wait(Duration::from_secs(2), || con.listening_ports().is_empty()) {
        return Err(format!(
            "a listener is left after Clear: ports {:?}",
            con.listening_ports()
        ));
    }
    Ok(())
}

fn wait(limit: Duration, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + limit;
    loop {
        if f() {
            return true;
        }
        if Instant::now() >= end {
            return false;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn accept_loop(inner: &Arc<Inner>, listener: &UnixListener) {
    for s in listener.incoming() {
        if inner.stop.load(Ordering::SeqCst) {
            break;
        }
        let Ok(s) = s else { continue };
        let inn = Arc::clone(inner);
        let _ = std::thread::Builder::new()
            .name("pool-lease".into())
            .spawn(move || {
                let _ = handle(&inn, s);
            });
    }
}

fn reply(w: &mut UnixStream, line: &str) -> std::io::Result<()> {
    w.write_all(format!("{line}\n").as_bytes())?;
    w.flush()
}

fn handle(inner: &Arc<Inner>, s: UnixStream) -> std::io::Result<()> {
    s.set_read_timeout(Some(inner.cfg.timeout))?;
    let mut w = s.try_clone()?;
    let mut r = BufReader::new(s);
    let mut line = String::new();
    if r.read_line(&mut line)? == 0 {
        return Ok(());
    }
    let words: Vec<&str> = line.split_whitespace().collect();
    match words.as_slice() {
        ["STATUS"] => {
            let (slots, st) = status(inner);
            for (i, s) in slots.iter().enumerate() {
                let pid = s.pid.map_or("-".to_string(), |p| p.to_string());
                reply(
                    &mut w,
                    &format!("slot {i} {} pid {pid} runs {}", s.state, s.runs),
                )?;
            }
            reply(
                &mut w,
                &format!(
                    "stats spawned {} leases {} kept {} discarded {} recycled {} \
                     leaked_listeners {} busy {} nomatch {}",
                    st.spawned,
                    st.leases,
                    st.kept,
                    st.discarded,
                    st.recycled,
                    st.leaked_listeners,
                    st.busy,
                    st.nomatch
                ),
            )?;
            reply(&mut w, "END")
        }
        ["STOP"] => {
            inner.stop.store(true, Ordering::SeqCst);
            inner.cv.notify_all();
            reply(&mut w, "OK")
        }
        ["LEASE", v, renode, plugin] => lease(inner, &mut r, &mut w, v, renode, plugin),
        _ => reply(
            &mut w,
            &format!("ERR unknown request {:?}", line.trim_end()),
        ),
    }
}

fn lease(
    inner: &Arc<Inner>,
    r: &mut BufReader<UnixStream>,
    w: &mut UnixStream,
    v: &str,
    renode: &str,
    plugin: &str,
) -> std::io::Result<()> {
    let why = if v.parse::<u32>().ok() != Some(PROTOCOL) {
        Some(format!("protocol {v}, pool speaks {PROTOCOL}"))
    } else if renode != inner.renode_key {
        Some(format!("renode {renode}, pool runs {}", inner.renode_key))
    } else if plugin != "-" && lock(&inner.plugin_key).as_deref() != Some(plugin) {
        Some(format!("plugin {plugin} is not loaded in the pool"))
    } else {
        None
    };
    if let Some(why) = why {
        lock(&inner.stats).nomatch += 1;
        return reply(w, &format!("NOMATCH {why}"));
    }
    if inner.stop.load(Ordering::SeqCst) {
        return reply(w, "BUSY");
    }
    // Take an idle process, never wait for one.
    let taken = {
        let mut g = lock(&inner.slots);
        let i = g.iter().position(|s| matches!(s, Slot::Idle { .. }));
        i.map(|i| {
            let Slot::Idle { con, runs } = std::mem::replace(&mut g[i], Slot::Warming) else {
                unreachable!()
            };
            g[i] = Slot::Busy {
                pid: con.pid(),
                runs,
            };
            (i, con, runs)
        })
    };
    let Some((i, mut con, runs)) = taken else {
        lock(&inner.stats).busy += 1;
        return reply(w, "BUSY");
    };
    lock(&inner.stats).leases += 1;
    *lock(&inner.last_activity) = Instant::now();
    let outcome = serve_lease(inner, r, w, &mut con);
    con.set_log(None);
    let keep = match &outcome {
        Ok(true) => {
            let mut st = lock(&inner.stats);
            if runs + 1 >= inner.cfg.max_runs
                || con.rss_bytes().is_some_and(|b| b > inner.cfg.rss_limit)
            {
                st.recycled += 1;
                false
            } else {
                st.kept += 1;
                true
            }
        }
        _ => {
            lock(&inner.stats).discarded += 1;
            false
        }
    };
    if !keep {
        // Kill now, so a broken process does no more work; the slot
        // thread spawns the replacement.
        con.kill();
    }
    {
        let mut g = lock(&inner.slots);
        g[i] = if keep {
            Slot::Idle {
                con,
                runs: runs + 1,
            }
        } else {
            Slot::Retire(con)
        };
    }
    inner.cv.notify_all();
    *lock(&inner.last_activity) = Instant::now();
    if outcome.is_ok() {
        let _ = reply(w, if keep { "DONE kept" } else { "DONE discarded" });
    }
    Ok(())
}

/// Serve one lease on `con`. `Ok(true)`: the lease ended with `END ok` and
/// the process is clean; `Ok(false)`: it ended with `END failed` or the
/// process is not clean; `Err`: the engine went away.
fn serve_lease(
    inner: &Inner,
    r: &mut BufReader<UnixStream>,
    w: &mut UnixStream,
    con: &mut Console,
) -> std::io::Result<bool> {
    let t = inner.cfg.timeout;
    reply(w, &format!("OK {}", con.pid()))?;
    let mut line = String::new();
    if r.read_line(&mut line)? == 0 {
        return Err(std::io::ErrorKind::UnexpectedEof.into());
    }
    let words: Vec<&str> = line.split_whitespace().collect();
    let ["RUN", workdir, script, port] = words.as_slice() else {
        reply(w, &format!("ERR expected RUN, got {:?}", line.trim_end()))?;
        return Err(std::io::ErrorKind::InvalidData.into());
    };
    let port: u16 = port
        .parse()
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidData))?;
    let log = File::create(Path::new(workdir).join("renode.log"))?;
    con.set_log(Some(log));
    let lines = ["Clear", "logLevel 1", &format!("include @{script}")];
    let err = match con.run(&lines, t) {
        Ok(out) if !out.contains(COMMAND_ERROR) => None,
        Ok(out) => Some(out),
        Err(e) => Some(e.to_string()),
    };
    if let Some(e) = err {
        let msg: String = e
            .lines()
            .filter(|l| !l.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\\n");
        reply(
            w,
            &format!("ERR the script failed in the pooled Renode: {msg}"),
        )?;
        return Err(std::io::ErrorKind::Other.into());
    }
    reply(w, "READY")?;
    // The run: no timeout, it may be long.
    r.get_ref().set_read_timeout(None)?;
    line.clear();
    if r.read_line(&mut line)? == 0 {
        return Err(std::io::ErrorKind::UnexpectedEof.into());
    }
    r.get_ref().set_read_timeout(Some(t))?;
    match line.trim_end() {
        "END ok" => {}
        "END failed" => return Ok(false),
        _ => return Err(std::io::ErrorKind::InvalidData.into()),
    }
    if con.exited() {
        return Ok(false);
    }
    match end_session(con, port, t) {
        Ok(()) => Ok(!con.exited()),
        Err(e) => {
            if e.contains("listener") {
                lock(&inner.stats).leaked_listeners += 1;
            }
            eprintln!("bb pool: pid {}: {e}", con.pid());
            Ok(false)
        }
    }
}
