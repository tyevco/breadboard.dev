//! A Renode process driven through its console pipe (ADR-004 Part B).
//!
//! `renode --disable-xwt --plain --console --port -1` reads monitor commands
//! from stdin and writes the monitor's output and its log to stdout. Facts
//! checked against Renode 1.17.0 over pipes (no TTY):
//!
//! - every command line is echoed, then its output, then the prompt
//!   (`(monitor) `, or `(<machine>) ` with a machine selected), without a
//!   newline;
//! - a failing command prints `There was an error executing command
//!   '<cmd>': ...`; inside an `include`, the rest of that script is skipped
//!   and the console goes on with its next line;
//! - an unknown command prints `No such command or device: <word>`.
//!
//! [`Console::run`] therefore ends every batch with an unknown command
//! `bbsync_<n>` and waits for its error line: the console runs its lines in
//! order, so that line appears only after everything before it finished.
//! Waiting for the prompt instead would race with log lines that other
//! Renode threads write after it. No TCP monitor port is opened, so nothing
//! on the network can reach the monitor (ADR-004 Part B).
//!
//! A reader thread copies everything Renode prints into the current
//! [`Console::set_log`] file (a lease's `renode.log`) and keeps the output
//! of the running batch for [`Console::run`].

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::renode_command;

/// The text Renode prints when a monitor command fails.
pub const COMMAND_ERROR: &str = "There was an error executing command";

/// Arguments of a console-driven Renode (no GUI, no TCP monitor).
pub const CONSOLE_ARGS: [&str; 5] = ["--disable-xwt", "--plain", "--console", "--port", "-1"];

/// An error of the console: Renode exited, or a batch did not finish in
/// time.
#[derive(Debug)]
pub enum ConsoleError {
    Io(std::io::Error),
    /// The console's stdout closed (Renode exited). Carries the output of
    /// the unfinished batch.
    Exited(String),
    /// The batch did not finish before the timeout. Carries its output so
    /// far.
    Timeout(String),
}

impl std::fmt::Display for ConsoleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConsoleError::Io(e) => write!(f, "renode console: {e}"),
            ConsoleError::Exited(out) => write!(f, "renode console: Renode exited; output:\n{out}"),
            ConsoleError::Timeout(out) => {
                write!(
                    f,
                    "renode console: no answer before the timeout; output:\n{out}"
                )
            }
        }
    }
}

impl std::error::Error for ConsoleError {}

impl From<std::io::Error> for ConsoleError {
    fn from(e: std::io::Error) -> Self {
        ConsoleError::Io(e)
    }
}

#[derive(Default)]
struct Shared {
    /// Output since the start of the running batch.
    buf: Vec<u8>,
    /// Where the reader copies the output (a lease's `renode.log`).
    log: Option<File>,
    /// The pipe closed.
    eof: bool,
}

/// One Renode process with its console on pipes.
pub struct Console {
    child: Child,
    stdin: ChildStdin,
    shared: Arc<(Mutex<Shared>, Condvar)>,
    seq: u64,
}

impl Console {
    /// Spawn `renode` ([`CONSOLE_ARGS`], ADR-004 Part A, kill on the death
    /// of the calling thread: see [`crate::renode_command`]) in `cwd` and
    /// wait until its console answers (`timeout`).
    pub fn spawn(renode: &Path, cwd: &Path, timeout: Duration) -> Result<Console, ConsoleError> {
        let (reader, writer) = std::io::pipe()?;
        let mut child = renode_command(renode)
            .args(CONSOLE_ARGS)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(writer.try_clone()?)
            .stderr(writer)
            .spawn()?;
        let stdin = child.stdin.take().expect("piped stdin");
        let shared = Arc::new((Mutex::new(Shared::default()), Condvar::new()));
        let s = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("renode-console".into())
            .spawn(move || pump(reader, &s))?;
        let mut c = Console {
            child,
            stdin,
            shared,
            seq: 0,
        };
        c.run(&[], timeout)?;
        Ok(c)
    }

    /// Renode's process id.
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Copy everything Renode prints from now on into `log` (`None`: drop
    /// it).
    pub fn set_log(&mut self, log: Option<File>) {
        let (m, _) = &*self.shared;
        let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(f) = g.log.as_mut() {
            let _ = f.flush();
        }
        g.log = log;
    }

    /// Send `lines` and wait until the console has run them all (the sync
    /// line after them answered). Returns what Renode printed meanwhile,
    /// without the sync line. The caller decides whether the output holds
    /// an error ([`COMMAND_ERROR`]).
    pub fn run(&mut self, lines: &[&str], timeout: Duration) -> Result<String, ConsoleError> {
        self.seq += 1;
        let tag = format!("bbsync_{}", self.seq);
        let marker = format!("No such command or device: {tag}");
        let mut text = String::new();
        for l in lines {
            debug_assert!(!l.contains('\n'));
            text.push_str(l);
            text.push('\n');
        }
        text.push_str(&tag);
        text.push('\n');
        let (m, cv) = &*self.shared;
        m.lock().unwrap_or_else(|e| e.into_inner()).buf.clear();
        self.stdin.write_all(text.as_bytes())?;
        self.stdin.flush()?;
        let deadline = Instant::now() + timeout;
        let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            let out = String::from_utf8_lossy(&g.buf);
            if let Some(i) = out.find(&marker) {
                // Everything before the echo of the sync line.
                let end = out[..i].rfind(&tag).unwrap_or(i);
                let s = out[..end].to_string();
                g.buf.clear();
                return Ok(s);
            }
            if g.eof {
                return Err(ConsoleError::Exited(out.into_owned()));
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(ConsoleError::Timeout(out.into_owned()));
            }
            g = cv
                .wait_timeout(g, deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// The process has exited.
    pub fn exited(&mut self) -> bool {
        !matches!(self.child.try_wait(), Ok(None))
    }

    /// Resident set size of the process in bytes (`/proc/<pid>/status`).
    pub fn rss_bytes(&self) -> Option<u64> {
        let s = std::fs::read_to_string(format!("/proc/{}/status", self.pid())).ok()?;
        let kb: u64 = s
            .lines()
            .find_map(|l| l.strip_prefix("VmRSS:"))?
            .trim()
            .strip_suffix("kB")?
            .trim()
            .parse()
            .ok()?;
        Some(kb * 1024)
    }

    /// TCP ports this process listens on (`/proc/net/tcp{,6}` entries in
    /// state LISTEN whose socket inode is one of the process's fds).
    pub fn listening_ports(&self) -> Vec<u16> {
        listening_ports(self.pid())
    }

    /// Kill the process and reap it.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Console {
    fn drop(&mut self) {
        self.kill();
    }
}

fn pump(mut r: std::io::PipeReader, shared: &(Mutex<Shared>, Condvar)) {
    let (m, cv) = shared;
    let mut chunk = [0u8; 16 * 1024];
    loop {
        let n = match r.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
        g.buf.extend_from_slice(&chunk[..n]);
        if let Some(f) = g.log.as_mut() {
            let _ = f.write_all(&chunk[..n]);
        }
        cv.notify_all();
    }
    let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
    g.eof = true;
    if let Some(f) = g.log.as_mut() {
        let _ = f.flush();
    }
    cv.notify_all();
}

/// TCP ports process `pid` listens on (see [`Console::listening_ports`]).
pub fn listening_ports(pid: u32) -> Vec<u16> {
    let mut inodes = std::collections::BTreeSet::new();
    if let Ok(rd) = std::fs::read_dir(format!("/proc/{pid}/fd")) {
        for e in rd.flatten() {
            if let Ok(t) = std::fs::read_link(e.path()) {
                let t = t.to_string_lossy();
                if let Some(i) = t.strip_prefix("socket:[").and_then(|s| s.strip_suffix(']')) {
                    inodes.insert(i.to_string());
                }
            }
        }
    }
    let mut ports = std::collections::BTreeSet::new();
    for f in ["/proc/net/tcp", "/proc/net/tcp6"] {
        let Ok(s) = std::fs::read_to_string(f) else {
            continue;
        };
        for l in s.lines().skip(1) {
            let w: Vec<&str> = l.split_whitespace().collect();
            // sl local rem st tx:rx tr:when retrnsmt uid timeout inode
            if w.len() > 9 && w[3] == "0A" && inodes.contains(w[9]) {
                if let Some(p) = w[1]
                    .rsplit(':')
                    .next()
                    .and_then(|h| u16::from_str_radix(h, 16).ok())
                {
                    ports.insert(p);
                }
            }
        }
    }
    ports.into_iter().collect()
}
