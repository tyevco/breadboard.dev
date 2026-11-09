//! `bb pool` end to end (ADR-004 Part B): `bb pool run` passes the socket
//! and the command's exit status and cleans up; a killed pool leaves no
//! Renode behind (approver decision 7); `bb pool start` / `status` / `stop`;
//! a pooled `bb run` gives the bytes of a fresh one.
//!
//! Needs Renode and arm-none-eabi-gcc (`BB_REQUIRE_RENODE=1`: fail instead
//! of skip).

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use common::*;

fn bb() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_bb"))
}

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("bb-clipool-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn renode_children(pid: u32) -> Vec<u32> {
    children(pid)
        .into_iter()
        .filter(|p| exe_name(*p) == "renode")
        .collect()
}

#[test]
fn pool_run_passes_socket_and_status() {
    if !have_renode() {
        return;
    }
    let d = tmp("run");
    let note = d.join("socket.txt");
    let st = Command::new(bb())
        .args(["pool", "run", "--slots", "1", "--dir"])
        .arg(d.join("pool"))
        .args(["--", "sh", "-c"])
        .arg(format!(
            "test -S \"$BB_RENODE_POOL\" || exit 9; printf %s \"$BB_RENODE_POOL\" > {}; exit 7",
            note.display()
        ))
        .env_remove("BB_RENODE_POOL")
        .status()
        .unwrap();
    assert_eq!(
        st.code(),
        Some(7),
        "bb pool run returns the command's status"
    );
    let sock = std::fs::read_to_string(&note).unwrap();
    assert!(!Path::new(&sock).exists(), "the socket is removed on exit");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn killed_pool_leaves_no_renode() {
    if !have_renode() {
        return;
    }
    let d = tmp("kill");
    let mut child = Command::new(bb())
        .args(["pool", "run", "--slots", "2", "--dir"])
        .arg(d.join("pool"))
        .args(["--", "sleep", "300"])
        .env_remove("BB_RENODE_POOL")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let pid = child.id();
    let mut renodes = Vec::new();
    let mut sleeper = None;
    let up = wait_for(Duration::from_secs(300), || {
        renodes = renode_children(pid);
        sleeper = children(pid).into_iter().find(|p| exe_name(*p) == "sleep");
        renodes.len() == 2 && sleeper.is_some()
    });
    let _ = child.kill();
    let _ = child.wait();
    if let Some(s) = sleeper {
        let _ = Command::new("kill").arg(s.to_string()).status();
    }
    assert!(up, "the pool started {renodes:?}");
    let gone = wait_for(Duration::from_secs(10), || {
        renodes.iter().all(|p| !alive(*p))
    });
    let _ = std::fs::remove_dir_all(&d);
    assert!(
        gone,
        "pool Renode processes {renodes:?} outlived the killed pool"
    );
}

fn outputs(dir: &Path) -> Vec<Vec<u8>> {
    ["nets.csv", "events.jsonl", "nets.vcd"]
        .iter()
        .map(|f| std::fs::read(dir.join(f)).unwrap())
        .collect()
}

#[test]
fn pool_start_status_stop_and_pooled_run() {
    if !have_renode() {
        return;
    }
    let Some(elf) = spike_elf() else { return };
    let d = tmp("daemon");
    let out = Command::new(bb())
        .args([
            "pool",
            "start",
            "--slots",
            "1",
            "--idle-exit",
            "600s",
            "--dir",
        ])
        .arg(d.join("pool"))
        .env_remove("BB_RENODE_POOL")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    let sock = text
        .lines()
        .find_map(|l| l.strip_prefix("export BB_RENODE_POOL="))
        .expect("export line")
        .to_string();
    let status = Command::new(bb())
        .args(["pool", "status", "--socket", &sock])
        .output()
        .unwrap();
    let status = String::from_utf8_lossy(&status.stdout).into_owned();
    let pid: u32 = status
        .lines()
        .find_map(|l| l.strip_prefix("slot 0 idle pid "))
        .and_then(|r| r.split_whitespace().next())
        .and_then(|p| p.parse().ok())
        .unwrap_or_else(|| panic!("no idle slot in {status:?}"));
    // A pooled run and a fresh run of the spike: identical bytes.
    let pooled = d.join("pooled");
    let o = spike_run(&bb(), &elf, &pooled, "30ms")
        .env("BB_RENODE_POOL", &sock)
        .stdout(Stdio::piped())
        .output()
        .unwrap();
    assert!(o.status.success());
    assert!(
        String::from_utf8_lossy(&o.stdout).contains(&format!("leased from the pool (pid {pid})")),
        "{}",
        String::from_utf8_lossy(&o.stdout)
    );
    let fresh = d.join("fresh");
    let o = spike_run(&bb(), &elf, &fresh, "30ms")
        .env_remove("BB_RENODE_POOL")
        .stdout(Stdio::piped())
        .output()
        .unwrap();
    assert!(o.status.success());
    assert!(String::from_utf8_lossy(&o.stdout).contains("started fresh"));
    assert_eq!(
        outputs(&pooled),
        outputs(&fresh),
        "pooled run differs from fresh"
    );
    // --renode-pool off ignores the pool.
    let off = d.join("off");
    let o = spike_run(&bb(), &elf, &off, "30ms")
        .env("BB_RENODE_POOL", &sock)
        .args(["--renode-pool", "off"])
        .stdout(Stdio::piped())
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&o.stdout).contains("started fresh"));
    let st = Command::new(bb())
        .args(["pool", "stop", "--socket", &sock])
        .status()
        .unwrap();
    assert!(st.success());
    assert!(!Path::new(&sock).exists());
    assert!(
        wait_for(Duration::from_secs(10), || !alive(pid)),
        "pool renode {pid} left"
    );
    let _ = std::fs::remove_dir_all(&d);
}
