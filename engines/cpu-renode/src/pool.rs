//! The warm Renode pool, client side (ADR-004 Part B).
//!
//! A pool manager (`bb pool`, [`crate::manager`]) owns pre-warmed Renode
//! processes driven through their console pipes ([`crate::console`]). A
//! [`crate::RenodeEngine`] whose [`crate::RenodeConfig::pool`] names the
//! manager's socket asks it for a process ([`Lease::acquire`]). If none is
//! idle, or the pool does not match this Renode and plugin, the engine
//! spawns a fresh Renode as before: a lease is never waited for.
//!
//! The pool is not an engine. A lease is one run: the manager clears the
//! process, has it include the run's own `session.resc` (the script a fresh
//! run uses, minus the lookahead loader, which the pool ran at pre-warm),
//! and the engine then talks to that Renode's External Control server
//! exactly as it would to a fresh one. At the end of the lease the manager
//! clears the process again, so no machine and no simulated time exist in
//! a pool process between leases (ADR-004 "Lease" 4).
//!
//! # Wire protocol (one Unix stream per lease, text lines)
//!
//! ```text
//! engine  -> manager   LEASE 1 <renode> <plugin>        renode: canonical launcher path;
//!                                                     plugin: cached assembly path or -
//! manager -> engine    OK <pid> | BUSY | NOMATCH <why>
//! engine  -> manager   RUN <workdir> <script> <port>  after OK; paths are monitor-safe
//! manager -> engine    READY | ERR <message>          ERR: the process is discarded
//! engine  -> manager   END ok|failed                  after closing External Control
//! manager -> engine    DONE kept|discarded
//! ```
//!
//! A lease whose stream closes without `END ok` (the engine failed, or `bb`
//! died) discards the process.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Environment variable naming the pool socket (`bb pool run` sets it). It
/// is the only way a run finds a pool (ADR-004 approver decision 4).
pub const POOL_ENV: &str = "BB_RENODE_POOL";

/// Protocol version of the lease messages.
pub const PROTOCOL: u32 = 1;

/// The pool socket from [`POOL_ENV`], if set and not empty.
pub fn from_env() -> Option<PathBuf> {
    std::env::var_os(POOL_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// What a lease must match: the canonical Renode launcher, and the cached
/// lookahead assembly when the run loads the plugin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PoolKey {
    pub renode: String,
    /// `None`: the run does not load the plugin, any process serves it.
    pub plugin: Option<String>,
}

/// The canonical form of a Renode launcher path (the pool key and the
/// plugin cache key use it).
pub fn canonical_renode(renode: &Path) -> PathBuf {
    std::fs::canonicalize(renode).unwrap_or_else(|_| renode.to_path_buf())
}

fn word(s: &str) -> Result<&str, String> {
    if s.is_empty() || s.chars().any(char::is_whitespace) {
        return Err(format!("{s:?} cannot be sent in a pool message"));
    }
    Ok(s)
}

/// One run's hold on a pool process.
pub struct Lease {
    rd: BufReader<UnixStream>,
    wr: UnixStream,
    pid: u32,
    ended: bool,
}

impl Lease {
    /// Ask the pool at `socket` for an idle process matching `key`.
    /// `Ok(None)`: no pool there, or none idle or matching; the caller
    /// starts Renode fresh. Wall-clock timeouts only.
    pub fn acquire(
        socket: &Path,
        key: &PoolKey,
        timeout: Duration,
    ) -> Result<Option<Lease>, String> {
        let Ok(s) = UnixStream::connect(socket) else {
            return Ok(None); // no pool (a stale socket): fresh
        };
        let io = |e: std::io::Error| format!("pool {}: {e}", socket.display());
        s.set_read_timeout(Some(timeout)).map_err(io)?;
        s.set_write_timeout(Some(timeout)).map_err(io)?;
        let wr = s.try_clone().map_err(io)?;
        let mut l = Lease {
            rd: BufReader::new(s),
            wr,
            pid: 0,
            ended: false,
        };
        let plugin = key.plugin.as_deref().unwrap_or("-");
        l.send(&format!(
            "LEASE {PROTOCOL} {} {}",
            word(&key.renode)?,
            word(plugin)?
        ))?;
        let r = l.recv()?;
        let mut w = r.split_whitespace();
        match (w.next(), w.next()) {
            (Some("OK"), Some(pid)) => {
                l.pid = pid.parse().map_err(|_| format!("pool: bad answer {r:?}"))?;
                Ok(Some(l))
            }
            (Some("BUSY" | "NOMATCH"), _) => {
                l.ended = true;
                Ok(None)
            }
            _ => Err(format!("pool: unexpected answer {r:?}")),
        }
    }

    /// The leased Renode's process id.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Have the process run `script` (copying its output to
    /// `workdir/renode.log`); returns once the script has finished. The
    /// External Control server on `port` is then listening.
    pub fn run(
        &mut self,
        workdir: &str,
        script: &str,
        port: u16,
        timeout: Duration,
    ) -> Result<(), String> {
        self.rd
            .get_ref()
            .set_read_timeout(Some(timeout))
            .map_err(|e| format!("pool: {e}"))?;
        self.send(&format!("RUN {} {} {port}", word(workdir)?, word(script)?))?;
        let r = self.recv()?;
        match r.split_once(' ').map_or((r.as_str(), ""), |(a, b)| (a, b)) {
            ("READY", _) => Ok(()),
            ("ERR", m) => {
                self.ended = true;
                Err(m.replace("\\n", "\n"))
            }
            _ => Err(format!("pool: unexpected answer {r:?}")),
        }
    }

    /// End the lease: `ok` keeps the process for the next run (if the
    /// manager agrees), otherwise it is discarded. Waits for the manager to
    /// have cleared the process (bounded by the stream timeout).
    pub fn end(&mut self, ok: bool) -> Result<bool, String> {
        if self.ended {
            return Ok(false);
        }
        self.ended = true;
        let _ = self
            .rd
            .get_ref()
            .set_read_timeout(Some(Duration::from_secs(30)));
        self.send(if ok { "END ok" } else { "END failed" })?;
        let r = self.recv()?;
        Ok(r == "DONE kept")
    }

    fn send(&mut self, line: &str) -> Result<(), String> {
        self.wr
            .write_all(format!("{line}\n").as_bytes())
            .and_then(|()| self.wr.flush())
            .map_err(|e| format!("pool: send: {e}"))
    }

    fn recv(&mut self) -> Result<String, String> {
        let mut s = String::new();
        match self.rd.read_line(&mut s) {
            Ok(0) => Err("pool: the manager closed the lease".into()),
            Ok(_) => Ok(s.trim_end().to_string()),
            Err(e) => Err(format!("pool: receive: {e}")),
        }
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        // Closing the stream without END ok discards the process.
        let _ = self.wr.shutdown(std::net::Shutdown::Both);
    }
}
