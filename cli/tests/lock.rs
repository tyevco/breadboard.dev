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
    // this bb's platforms (`make packages` runs the same with --check). In
    // place, every recorded path is where the package resolves: no warning.
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
        assert!(!text(&o).contains("records path"), "{b}: {}", text(&o));
    }
}

/// `bb run --netlist <board>` up to its firmware check, which comes after
/// the lock check: its output.
fn run_board(board: &Path) -> Output {
    Command::new(bb())
        .args(["run", "--netlist"])
        .arg(board)
        .args([
            "--firmware",
            "/nonexistent.elf",
            "--target",
            "U1",
            "--for",
            "1ms",
            "--probe",
            "net:LED_A",
            "--out",
            "/tmp/never",
        ])
        .env("BB_FIRST_PARTY_PACKAGES", repo().join("packages"))
        .output()
        .unwrap()
}

fn open_board(args: &[&str], board: &Path) -> Output {
    Command::new(bb())
        .arg("open")
        .args(args)
        .arg(board)
        .env("BB_FIRST_PARTY_PACKAGES", repo().join("packages"))
        .output()
        .unwrap()
}

/// ADR-015 G6 (D7), the package-path trap: a copy of `examples/spike` with
/// a modified `st/stm32f103c8` beside it, where its bb.lock's recorded path
/// now points. Without a directive the first-party package is used (its
/// tree matches the lock) and `bb open` and `bb run` say that the package
/// at the recorded path is not; with `*bb packages path=` the copy is used:
/// `bb open` refuses its tree until `--update-lock`, which records the
/// copy's tree hash.
#[test]
fn a_modified_package_beside_a_board_copy_is_not_ignored_silently() {
    let root = std::env::temp_dir().join(format!("bb-lock-path-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let _t = Tmp(root.clone());
    let spike = root.join("examples/spike");
    std::fs::create_dir_all(&spike).unwrap();
    for f in ["board.cir", "bb.lock"] {
        std::fs::copy(repo().join("examples/spike").join(f), spike.join(f)).unwrap();
    }
    let board = spike.join("board.cir");
    let lock = spike.join("bb.lock");
    let committed_lock = std::fs::read_to_string(&lock).unwrap();
    assert!(committed_lock.contains("path = \"../../packages/st/stm32f103c8\""));
    let copy = root.join("packages/st/stm32f103c8");
    let recorded = "bb.lock records path ../../packages/st/stm32f103c8, but it resolved to";

    // 1. Nothing at the recorded path: accepted, and the warning says so.
    let o = open_board(&["--check"], &board);
    assert!(o.status.success(), "{}", text(&o));
    assert!(text(&o).contains(recorded), "{}", text(&o));
    assert!(
        text(&o).contains("no package st/stm32f103c8 is at the recorded path"),
        "{}",
        text(&o)
    );

    // 2. An identical copy there: still the first-party one, and said so.
    copy_dir(&repo().join("packages/st/stm32f103c8"), &copy);
    let o = open_board(&["--check"], &board);
    assert!(o.status.success(), "{}", text(&o));
    assert!(
        text(&o).contains("an identical copy at the recorded path is not used"),
        "{}",
        text(&o)
    );

    // 3. A modified copy: accepted (the first-party tree is the locked one)
    // but no longer silently: bb open and bb run name the copy's tree.
    let pins = copy.join("pins.toml");
    let mut t = std::fs::read_to_string(&pins).unwrap();
    t.push_str("# a local edit\n");
    std::fs::write(&pins, t).unwrap();
    let copy_tree = bbpkg::lock::tree_hash(&std::fs::canonicalize(&copy).unwrap()).unwrap();
    let not_used = format!(
        "a different st/stm32f103c8 (tree {copy_tree}) is at the recorded path and is NOT used"
    );
    for o in [open_board(&["--check"], &board), open_board(&[], &board)] {
        assert!(o.status.success(), "{}", text(&o));
        assert!(text(&o).contains(&not_used), "{}", text(&o));
        assert!(text(&o).contains("*bb packages path="), "{}", text(&o));
    }
    let o = run_board(&board);
    assert!(!o.status.success());
    let e = String::from_utf8_lossy(&o.stderr);
    assert!(
        e.contains("bb run: warning: package st/stm32f103c8") && e.contains(&not_used),
        "{e}"
    );
    assert!(
        e.contains("/nonexistent.elf not found"),
        "the lock check passed: {e}"
    );
    assert_eq!(std::fs::read_to_string(&lock).unwrap(), committed_lock);

    // 4. Named by the directive: the copy is used. bb open and bb run refuse
    // its tree against the lock, and --update-lock records exactly it.
    let text0 = std::fs::read_to_string(&board).unwrap();
    std::fs::write(&board, format!("*bb packages path=../../packages\n{text0}")).unwrap();
    let o = open_board(&[], &board);
    assert!(!o.status.success());
    assert!(text(&o).contains("refusing"), "{}", text(&o));
    assert!(
        text(&o).contains(&format!("found 0.1.0 {copy_tree}")),
        "{}",
        text(&o)
    );
    let o = run_board(&board);
    assert!(!o.status.success());
    assert!(
        text(&o).contains("package st/stm32f103c8: locked"),
        "{}",
        text(&o)
    );
    let o = open_board(&["--update-lock"], &board);
    assert!(o.status.success(), "{}", text(&o));
    let updated = std::fs::read_to_string(&lock).unwrap();
    assert!(
        updated.contains(&format!("tree = \"{copy_tree}\"")),
        "{updated}"
    );
    // Only the package's tree line changed; the platform entry (the legacy
    // hash, ADR-015 Amendment 3) is kept as it stands.
    let diff: Vec<(&str, &str)> = committed_lock
        .lines()
        .zip(updated.lines())
        .filter(|(a, b)| a != b)
        .collect();
    assert_eq!(diff.len(), 1, "{diff:?}");
    for o in [open_board(&["--check"], &board), run_board(&board)] {
        assert!(!text(&o).contains("records path"), "{}", text(&o));
    }
    assert!(open_board(&["--check"], &board).status.success());
    assert!(
        String::from_utf8_lossy(&run_board(&board).stderr).contains("/nonexistent.elf not found")
    );

    // 5. A directive naming no directory is an error, not a skipped entry.
    std::fs::write(&board, format!("*bb packages path=../nowhere\n{text0}")).unwrap();
    let o = open_board(&["--check"], &board);
    assert!(!o.status.success());
    assert!(text(&o).contains("is not a directory"), "{}", text(&o));
}

/// ADR-015 G6 (D7) on the project route: a copy of `examples/bench`
/// elsewhere resolves its first-party packages away from their recorded
/// paths, and `bb open` says so for each, but not for its own packages.
#[test]
fn a_project_copy_warns_for_each_moved_package() {
    let root = std::env::temp_dir().join(format!("bb-lock-proj-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let _t = Tmp(root.clone());
    let proj = root.join("a/b/bench");
    copy_dir(&repo().join("examples/bench"), &proj);
    let _ = std::fs::remove_dir_all(proj.join(".bb"));
    let o = Command::new(bb())
        .args(["open", "--check"])
        .arg(&proj)
        .env("BB_FIRST_PARTY_PACKAGES", repo().join("packages"))
        .output()
        .unwrap();
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    for id in ["generic/capacitor", "generic/resistor", "st/stm32f103c8"] {
        assert!(
            t.contains(&format!(
                "warning: package {id}: bb.lock records path ../../packages/{id}"
            )),
            "{id}: {t}"
        );
    }
    assert_eq!(t.matches("records path").count(), 3, "{t}");
}
