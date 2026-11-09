//! Helpers of the `bb` integration tests that run Renode.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub fn skip(msg: &str) {
    let required = std::env::var("BB_REQUIRE_RENODE").is_ok_and(|v| v == "1");
    assert!(!required, "BB_REQUIRE_RENODE=1 but {msg}");
    eprintln!("SKIP: {msg}");
}

pub fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// The spike firmware, built on demand.
pub fn spike_elf() -> Option<PathBuf> {
    let dir = repo().join("examples/spike/firmware");
    let ok = Command::new("make")
        .args(["-s", "-C"])
        .arg(&dir)
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        skip("building the spike firmware failed (run `make setup`)");
        return None;
    }
    Some(dir.join("build/blinky.elf"))
}

pub fn have_renode() -> bool {
    if bb_cpu_renode::find_renode().is_none() {
        skip("renode not installed (run `make setup`, or set BB_RENODE)");
        return false;
    }
    true
}

/// Direct children of `pid` (from `/proc/*/stat`).
pub fn children(pid: u32) -> Vec<u32> {
    let mut v = Vec::new();
    for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Ok(p) = e.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        if ppid(p) == Some(pid) {
            v.push(p);
        }
    }
    v
}

fn stat(pid: u32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()
}

fn ppid(pid: u32) -> Option<u32> {
    let s = stat(pid)?;
    // pid (comm) state ppid ...: comm may contain spaces, so split after ')'.
    let rest = &s[s.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

/// The process exists and is not a zombie.
pub fn alive(pid: u32) -> bool {
    stat(pid).is_some_and(|s| {
        s.rfind(')')
            .and_then(|i| s[i + 1..].split_whitespace().next().map(|st| st != "Z"))
            .unwrap_or(false)
    })
}

pub fn exe_name(pid: u32) -> String {
    std::fs::read_link(format!("/proc/{pid}/exe"))
        .map(|p| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        })
        .unwrap_or_default()
}

/// Wait until `f` holds (polling every 20 ms) or `limit` passes.
pub fn wait_for(limit: Duration, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + limit;
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    f()
}

/// `bb run` of the spike for `dur` into `out`.
pub fn spike_run(bb: &Path, elf: &Path, out: &Path, dur: &str) -> Command {
    let mut c = Command::new(bb);
    c.current_dir(repo())
        .arg("run")
        .arg("--netlist")
        .arg("examples/spike/board.cir")
        .arg("--firmware")
        .arg(elf)
        .args(["--target", "U1", "--for", dur])
        .args(["--probe", "net:LED_A", "--out"])
        .arg(out)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    c
}
