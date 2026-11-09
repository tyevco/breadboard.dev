//! `bb pool`: a warm Renode pool (ADR-004 Part B).
//!
//! ```text
//! bb pool run [options] -- <command> [args...]
//!     start a pool, wait for its pre-warm, run <command> with
//!     BB_RENODE_POOL set, stop the pool; exits with the command's status
//! bb pool start [options] [--idle-exit 1800s]
//!     start a pool in the background and print `export BB_RENODE_POOL=...`
//! bb pool stop [--socket <path>]      (default: $BB_RENODE_POOL)
//! bb pool status [--socket <path>]
//!
//! options: --slots N (default 1)  --max-runs K (100)  --rss-limit-mb M (2048)
//!          --renode <path>  --dir <private dir>  --no-wait
//! ```
//!
//! Nothing finds a pool unless `BB_RENODE_POOL` names its socket (ADR-004
//! approver decision 4). A `bb run` that finds no idle, matching process
//! starts Renode fresh; `--renode-pool off` never uses a pool.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use bb_cpu_renode::find_renode;
use bb_cpu_renode::manager::{default_dir, private_dir, Manager, PoolConfig};
use bb_cpu_renode::pool::POOL_ENV;
use bbcore::Time;

pub const USAGE: &str = "\
usage: bb pool run [--slots N] [--max-runs K] [--rss-limit-mb M] [--renode <path>]
                   [--dir <dir>] [--no-wait] -- <command> [args...]
       bb pool start [--slots N] [--max-runs K] [--rss-limit-mb M] [--renode <path>]
                     [--dir <dir>] [--idle-exit 1800s]
       bb pool stop [--socket <path>]
       bb pool status [--socket <path>]

  A warm Renode pool (ADR-004 Part B): pre-warmed Renode processes that
  `bb run` (and the Renode engine tests) lease instead of starting Renode,
  when BB_RENODE_POOL names the pool's socket. A run that finds no idle
  process starts Renode fresh. Outputs are identical either way.

  run          run <command> with a pool, then stop it; exits with the
               command's status
  start        start a pool in the background; prints the export line.
               It stops after --idle-exit without a lease (default 1800s, or never)
  stop         stop the pool at --socket (default $BB_RENODE_POOL)
  status       print the pool's slots and counters
  --slots      Renode processes (default 1)
  --max-runs   leases a process serves before it is replaced (default 100)
  --rss-limit-mb
               replace a process whose RSS exceeds this after a lease (2048)
  --renode     the renode launcher (default: $BB_RENODE, PATH, ~/.local/bin)
  --dir        private (0700) directory for the socket (default
               $XDG_RUNTIME_DIR/bb, else $BB_CACHE_DIR/pool, else ~/.cache/bb/pool)
  --no-wait    run <command> without waiting for the pre-warm";

struct Opts {
    slots: usize,
    max_runs: u32,
    rss_mb: u64,
    renode: Option<PathBuf>,
    dir: Option<PathBuf>,
    wait: bool,
    idle_exit: Option<Duration>,
    socket: Option<PathBuf>,
    command: Vec<String>,
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts {
        slots: 1,
        max_runs: 100,
        rss_mb: 2048,
        renode: None,
        dir: None,
        wait: true,
        idle_exit: Some(Duration::from_secs(30 * 60)),
        socket: None,
        command: Vec::new(),
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = |n: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{n} needs a value"))
        };
        match a.as_str() {
            "--slots" => {
                o.slots = val(a)?
                    .parse()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or("--slots: expected a positive number")?
            }
            "--max-runs" => {
                o.max_runs = val(a)?
                    .parse()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or("--max-runs: expected a positive number")?
            }
            "--rss-limit-mb" => {
                o.rss_mb = val(a)?
                    .parse()
                    .map_err(|_| "--rss-limit-mb: expected a number")?
            }
            "--renode" => o.renode = Some(PathBuf::from(val(a)?)),
            "--dir" => o.dir = Some(PathBuf::from(val(a)?)),
            "--socket" => o.socket = Some(PathBuf::from(val(a)?)),
            "--no-wait" => o.wait = false,
            "--idle-exit" => {
                let v = val(a)?;
                o.idle_exit = if v == "never" {
                    None
                } else {
                    let t: Time = v.parse().map_err(|e| format!("--idle-exit: {e}"))?;
                    Some(Duration::from_nanos(t.as_fs() / 1_000_000))
                }
            }
            "--" => {
                o.command = it.cloned().collect();
                break;
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    Ok(o)
}

fn config(o: &Opts) -> Result<PoolConfig, String> {
    let renode = match &o.renode {
        Some(p) => p.clone(),
        None => find_renode().ok_or("renode not found (run `make setup` or pass --renode)")?,
    };
    let dir = match &o.dir {
        Some(d) => d.clone(),
        None => default_dir().ok_or("no pool directory (set XDG_RUNTIME_DIR or HOME)")?,
    };
    let mut cfg = PoolConfig::new(renode, dir);
    cfg.slots = o.slots;
    cfg.max_runs = o.max_runs;
    cfg.rss_limit = o.rss_mb << 20;
    Ok(cfg)
}

fn start_manager(o: &Opts) -> Result<Manager, String> {
    let cfg = config(o)?;
    let dir = cfg.dir.clone();
    Manager::start(cfg).map_err(|e| format!("start the pool in {}: {e}", dir.display()))
}

fn warm(m: &Manager) {
    let t0 = Instant::now();
    match m.wait_warm(Duration::from_secs(600)) {
        Ok(()) => eprintln!(
            "bb pool: {} warm Renode process(es) in {:.2} s{}",
            m.status().0.len(),
            t0.elapsed().as_secs_f64(),
            if m.plugin_loaded() {
                ", lookahead plugin loaded"
            } else {
                ""
            }
        ),
        Err(e) => eprintln!("bb pool: warning: {e} (runs start Renode fresh)"),
    }
}

fn run(o: &Opts) -> Result<ExitCode, String> {
    if o.command.is_empty() {
        return Err("bb pool run: no command (bb pool run [options] -- <command>)".into());
    }
    let m = start_manager(o)?;
    if o.wait {
        warm(&m);
    }
    let status = Command::new(&o.command[0])
        .args(&o.command[1..])
        .env(POOL_ENV, m.socket())
        .status();
    let (_, st) = m.status();
    eprintln!(
        "bb pool: {} leases ({} kept, {} discarded, {} recycled), {} busy, {} no match, \
         {} leaked listeners",
        st.leases, st.kept, st.discarded, st.recycled, st.busy, st.nomatch, st.leaked_listeners
    );
    m.shutdown();
    let status = status.map_err(|e| format!("run {}: {e}", o.command[0]))?;
    Ok(match status.code() {
        Some(c) => ExitCode::from(u8::try_from(c).unwrap_or(1)),
        None => {
            use std::os::unix::process::ExitStatusExt;
            ExitCode::from(128u8.wrapping_add(status.signal().unwrap_or(0) as u8))
        }
    })
}

/// `bb pool serve`: the background process of `bb pool start`.
fn serve(o: &Opts) -> Result<ExitCode, String> {
    let m = start_manager(o)?;
    println!("socket {}", m.socket().display());
    let _ = std::io::stdout().flush();
    warm(&m);
    m.serve(o.idle_exit);
    m.shutdown();
    Ok(ExitCode::SUCCESS)
}

extern "C" {
    fn setsid() -> i32;
}

fn start(o: &Opts) -> Result<ExitCode, String> {
    let cfg = config(o)?;
    private_dir(&cfg.dir).map_err(|e| format!("{}: {e}", cfg.dir.display()))?;
    let log_path = cfg.dir.join(format!("serve-{}.log", std::process::id()));
    let log = std::fs::File::create(&log_path).map_err(|e| e.to_string())?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(exe);
    cmd.arg("pool").arg("serve");
    cmd.args(["--slots", &o.slots.to_string()]);
    cmd.args(["--max-runs", &o.max_runs.to_string()]);
    cmd.args(["--rss-limit-mb", &o.rss_mb.to_string()]);
    cmd.arg("--renode").arg(&cfg.renode);
    cmd.arg("--dir").arg(&cfg.dir);
    match o.idle_exit {
        Some(d) => cmd.args(["--idle-exit", &format!("{}s", d.as_secs().max(1))]),
        None => cmd.args(["--idle-exit", "never"]),
    };
    cmd.stdin(Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log);
    // A daemon on purpose: its own session, so the terminal's signals do
    // not reach it; `bb pool stop` or --idle-exit end it, and its Renode
    // processes die with it.
    use std::os::unix::process::CommandExt;
    // SAFETY: setsid is async-signal-safe and the closure does not allocate.
    unsafe {
        cmd.pre_exec(|| {
            setsid();
            Ok(())
        });
    }
    let child = cmd
        .spawn()
        .map_err(|e| format!("spawn bb pool serve: {e}"))?;
    let socket = cfg.dir.join(format!("pool-{}-0.sock", child.id()));
    let deadline = Instant::now() + Duration::from_secs(600);
    loop {
        if let Ok(lines) = request(&socket, "STATUS") {
            if !lines.iter().any(|l| l.contains(" warming ")) {
                break;
            }
        }
        if Instant::now() > deadline {
            return Err(format!(
                "the pool did not start; see {}",
                log_path.display()
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    println!("export {POOL_ENV}={}", socket.display());
    Ok(ExitCode::SUCCESS)
}

fn request(socket: &PathBuf, what: &str) -> std::io::Result<Vec<String>> {
    let mut s = UnixStream::connect(socket)?;
    s.set_read_timeout(Some(Duration::from_secs(10)))?;
    s.write_all(format!("{what}\n").as_bytes())?;
    let mut lines = Vec::new();
    for l in BufReader::new(s).lines() {
        let l = l?;
        let end = l == "END" || l == "OK";
        lines.push(l);
        if end {
            break;
        }
    }
    Ok(lines)
}

fn socket_of(o: &Opts) -> Result<PathBuf, String> {
    o.socket
        .clone()
        .or_else(bb_cpu_renode::pool::from_env)
        .ok_or_else(|| format!("no pool: pass --socket or set {POOL_ENV}"))
}

fn stop(o: &Opts) -> Result<ExitCode, String> {
    let s = socket_of(o)?;
    request(&s, "STOP").map_err(|e| format!("{}: {e}", s.display()))?;
    let deadline = Instant::now() + Duration::from_secs(30);
    while s.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    if s.exists() {
        return Err(format!("the pool at {} did not stop", s.display()));
    }
    Ok(ExitCode::SUCCESS)
}

fn status(o: &Opts) -> Result<ExitCode, String> {
    let s = socket_of(o)?;
    for l in request(&s, "STATUS").map_err(|e| format!("{}: {e}", s.display()))? {
        if l != "END" {
            println!("{l}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

pub fn main(args: &[String]) -> ExitCode {
    let r = match args.first().map(String::as_str) {
        Some(sub @ ("run" | "start" | "serve" | "stop" | "status")) => {
            parse(&args[1..]).and_then(|o| match sub {
                "run" => run(&o),
                "start" => start(&o),
                "serve" => serve(&o),
                "stop" => stop(&o),
                _ => status(&o),
            })
        }
        Some("-h" | "--help" | "help") => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match r {
        Ok(c) => c,
        Err(e) => {
            eprintln!("bb pool: {e}");
            ExitCode::FAILURE
        }
    }
}
