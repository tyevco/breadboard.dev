//! No Renode outlives the `bb` that started it (ADR-004 approver decision 7).
//!
//! `bb run` is killed with SIGKILL in the middle of a run; its Renode child
//! must be gone soon after (`PR_SET_PDEATHSIG`, `bb_cpu_renode::renode_command`).
//! Needs Renode and arm-none-eabi-gcc; with `BB_REQUIRE_RENODE=1` a missing
//! tool fails the test instead of skipping it.

mod common;

use std::path::PathBuf;
use std::process::Child;
use std::time::Duration;

use common::*;

fn kill_and_reap(mut c: Child) {
    let _ = c.kill();
    let _ = c.wait();
}

#[test]
fn killed_bb_leaves_no_renode() {
    if !have_renode() {
        return;
    }
    let Some(elf) = spike_elf() else { return };
    let out = std::env::temp_dir().join(format!("bb-orphan-{}", std::process::id()));
    let bb = PathBuf::from(env!("CARGO_BIN_EXE_bb"));
    // Long enough that bb is still simulating when it is killed.
    let mut cmd = spike_run(&bb, &elf, &out, "60s");
    cmd.env_remove("BB_RENODE_POOL");
    let child = cmd.spawn().expect("spawn bb");
    let bb_pid = child.id();
    let mut renode = None;
    let found = wait_for(Duration::from_secs(60), || {
        renode = children(bb_pid)
            .into_iter()
            .find(|p| exe_name(*p) == "renode");
        renode.is_some()
    });
    if !found {
        kill_and_reap(child);
        panic!("bb started no renode child");
    }
    let renode = renode.unwrap();
    // Let Renode get into its startup or the run, then kill bb hard: no
    // Drop runs, only the kernel's parent-death signal can stop Renode.
    std::thread::sleep(Duration::from_millis(1500));
    kill_and_reap(child);
    let gone = wait_for(Duration::from_secs(10), || !alive(renode));
    let _ = std::fs::remove_dir_all(&out);
    assert!(gone, "renode {renode} outlived its killed bb");
}
