//! The startup JIT profile of a fresh Renode (ADR-004 Amendment 6).
//!
//! With an empty cache the first `bb run` gathers the .NET multi-core JIT
//! profile and leaves exactly one `.prof` file (no temporary files); the
//! next run plays it back without rewriting it; all runs, and one with
//! `BB_RENODE_JIT_PROFILE=off`, give byte-identical outputs. Needs Renode
//! and arm-none-eabi-gcc; with `BB_REQUIRE_RENODE=1` a missing tool fails
//! the test instead of skipping it.

mod common;

use std::path::{Path, PathBuf};

use common::*;

fn files(dir: &Path) -> Vec<(String, u64, std::time::SystemTime)> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| {
                    let m = e.metadata().expect("metadata");
                    (
                        e.file_name().to_string_lossy().into_owned(),
                        m.len(),
                        m.modified().expect("mtime"),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn outputs(dir: &Path) -> Vec<Vec<u8>> {
    ["nets.csv", "nets.vcd", "events.jsonl"]
        .iter()
        .map(|f| std::fs::read(dir.join(f)).unwrap_or_else(|e| panic!("{f}: {e}")))
        .collect()
}

#[test]
fn gather_then_playback_same_outputs() {
    if !have_renode() {
        return;
    }
    let Some(elf) = spike_elf() else { return };
    let root = std::env::temp_dir().join(format!("bb-jitprof-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let cache = root.join("cache");
    let jit = cache.join("renode-jit");
    let bb = PathBuf::from(env!("CARGO_BIN_EXE_bb"));
    let run = |name: &str, off: bool| -> PathBuf {
        let out = root.join(name);
        let mut c = spike_run(&bb, &elf, &out, "20ms");
        c.env_remove("BB_RENODE_POOL")
            .env_remove("BB_RENODE_STOCK_RUNTIME")
            .env("BB_CACHE_DIR", &cache);
        if off {
            c.env("BB_RENODE_JIT_PROFILE", "off");
        } else {
            c.env_remove("BB_RENODE_JIT_PROFILE");
        }
        let st = c.status().expect("spawn bb");
        assert!(st.success(), "bb run {name}: {st}");
        out
    };

    let first = run("gather", false);
    let after_gather = files(&jit);
    assert_eq!(
        after_gather.len(),
        1,
        "one profile after the gather run: {after_gather:?}"
    );
    assert!(
        after_gather[0].0.starts_with("renode-") && after_gather[0].0.ends_with(".prof"),
        "profile name {:?}",
        after_gather[0].0
    );
    assert!(after_gather[0].1 > 0, "empty profile");

    let second = run("playback", false);
    assert_eq!(
        files(&jit),
        after_gather,
        "playback must not rewrite the profile"
    );

    let third = run("off", true);
    assert_eq!(
        files(&jit),
        after_gather,
        "BB_RENODE_JIT_PROFILE=off must not touch it"
    );

    let want = outputs(&first);
    assert!(
        outputs(&second) == want,
        "playback outputs differ from the gather run"
    );
    assert!(
        outputs(&third) == want,
        "profile-off outputs differ from the gather run"
    );
    let _ = std::fs::remove_dir_all(&root);
}
