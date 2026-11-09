//! ADR-006 gate G5: `bb.lock` (D8). `bb open <board.cir>` writes it; a
//! package whose tree hash, or a platform whose content hash, differs from
//! it is refused by `bb open` (and `--check`) and by `bb run`; `bb open
//! --update-lock` rewrites only the changed entry. No Renode is needed: `bb
//! run` checks the lock before it starts any engine.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn bb() -> &'static str {
    env!("CARGO_BIN_EXE_bb")
}

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

struct Tmp(PathBuf);
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let p = e.path();
        if p.is_dir() {
            copy_dir(&p, &to.join(e.file_name()));
        } else {
            std::fs::copy(&p, to.join(e.file_name())).unwrap();
        }
    }
}

/// A project dir with a board file using two packages, and a private copy
/// of the two STM32 packages as the first-party directory.
fn setup(name: &str) -> (Tmp, PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("bb-lock-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pk = root.join("packages");
    for id in ["st/stm32f103c8", "st/stm32f072rb"] {
        copy_dir(&repo().join("packages").join(id), &pk.join(id));
    }
    let proj = root.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let board = proj.join("board.cir");
    std::fs::write(
        &board,
        "* two parts from packages\n*bb part U1 package=st/stm32f103c8\n*bb pin U1.PA5 net=A model=digital_out\n\
         *bb part U2 package=st/stm32f072rb\n*bb pin U2.PA1 net=B model=adc_in\nR1 A B 1k\n.end\n",
    )
    .unwrap();
    (Tmp(root), pk, board)
}

fn run(pk: &Path, args: &[&str]) -> Output {
    Command::new(bb())
        .args(args)
        .env("BB_FIRST_PARTY_PACKAGES", pk)
        .output()
        .unwrap()
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

#[test]
fn lock_mismatch_is_refused_and_update_rewrites_only_that_entry() {
    let (_t, pk, board) = setup("g5");
    let b = board.to_str().unwrap();
    let lock = board.with_file_name("bb.lock");

    // bb run refuses a package board without a lock (before any engine).
    let o = run(
        &pk,
        &[
            "run",
            "--netlist",
            b,
            "--firmware",
            "/nonexistent.elf",
            "--target",
            "U1",
            "--for",
            "1ms",
            "--probe",
            "net:A",
            "--out",
            "/tmp/never",
        ],
    );
    assert!(!o.status.success());
    assert!(text(&o).contains("bb.lock is missing"), "{}", text(&o));

    // bb open writes it; --check then passes; a second open changes nothing.
    let o = run(&pk, &["open", b]);
    assert!(o.status.success(), "{}", text(&o));
    let first = std::fs::read_to_string(&lock).unwrap();
    assert_eq!(first.matches("[[package]]").count(), 2);
    assert_eq!(first.matches("[[platform]]").count(), 2);
    assert!(run(&pk, &["open", b, "--check"]).status.success());
    assert!(run(&pk, &["open", b]).status.success());
    assert_eq!(std::fs::read_to_string(&lock).unwrap(), first);

    // Change one package's content: refused by open, --check and run; the
    // lock is untouched.
    let pins = pk.join("st/stm32f072rb/pins.toml");
    let mut t = std::fs::read_to_string(&pins).unwrap();
    t.push_str("# an edit\n");
    std::fs::write(&pins, t).unwrap();
    let o = run(&pk, &["open", b]);
    assert!(!o.status.success());
    assert!(text(&o).contains("refusing"), "{}", text(&o));
    assert!(!run(&pk, &["open", b, "--check"]).status.success());
    let o = run(
        &pk,
        &[
            "run",
            "--netlist",
            b,
            "--firmware",
            "/nonexistent.elf",
            "--target",
            "U1",
            "--for",
            "1ms",
            "--probe",
            "net:A",
            "--out",
            "/tmp/never",
        ],
    );
    assert!(!o.status.success());
    assert!(
        text(&o).contains("package st/stm32f072rb: locked"),
        "{}",
        text(&o)
    );
    assert_eq!(std::fs::read_to_string(&lock).unwrap(), first);

    // --update-lock rewrites exactly that entry's tree line.
    let o = run(&pk, &["open", b, "--update-lock"]);
    assert!(o.status.success(), "{}", text(&o));
    let second = std::fs::read_to_string(&lock).unwrap();
    let diff: Vec<(&str, &str)> = first
        .lines()
        .zip(second.lines())
        .filter(|(a, b)| a != b)
        .collect();
    assert_eq!(first.lines().count(), second.lines().count());
    assert_eq!(diff.len(), 1, "{diff:?}");
    assert!(diff[0].0.starts_with("tree = ") && second.contains("st/stm32f103c8"));
    let f103 = |s: &str| {
        s.split("[[")
            .find(|b| b.contains("stm32f103c8") && b.starts_with("package"))
            .unwrap()
            .to_string()
    };
    assert_eq!(f103(&first), f103(&second));

    // A platform mismatch (a lock from another bb) is refused the same way,
    // and --update-lock restores only the platform entry.
    let platform_line = second
        .lines()
        .find(|l| l.starts_with("content = "))
        .unwrap()
        .to_string();
    let forged = second.replacen(
        &platform_line,
        "content = \"sha256:0000000000000000000000000000000000000000000000000000000000000000\"",
        1,
    );
    std::fs::write(&lock, &forged).unwrap();
    let o = run(&pk, &["open", b]);
    assert!(!o.status.success());
    assert!(text(&o).contains("platform renode:"), "{}", text(&o));
    assert!(run(&pk, &["open", b, "--update-lock"]).status.success());
    assert_eq!(std::fs::read_to_string(&lock).unwrap(), second);
}

#[test]
fn committed_example_locks_are_current() {
    // Every example board's bb.lock matches this checkout's packages and
    // this bb's platforms (`make packages` runs the same with --check).
    for b in [
        "examples/spike/board.cir",
        "examples/feedback/board.cir",
        "examples/feedback-open/board.cir",
        "examples/adc/board.cir",
        "examples/adc/board-open.cir",
    ] {
        let o = Command::new(bb())
            .args(["open", "--check"])
            .arg(repo().join(b))
            .env("BB_FIRST_PARTY_PACKAGES", repo().join("packages"))
            .output()
            .unwrap();
        assert!(o.status.success(), "{b}: {}", text(&o));
    }
}
