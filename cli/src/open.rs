//! `bb open` (ADR-007 D7) for hand-written board files, and the lock check
//! of `bb run` (ADR-006 D8).
//!
//! Phase 1 M3 implements the `board.cir` front end only: `bb open
//! <board.cir>` resolves its `package=` parts on the search path (ADR-006
//! D1: breadboard.dev's first-party `packages/`), and writes `bb.lock` next
//! to the board file with each package's tree hash and each platform's
//! content hash. KiCad projects, `bb.toml` and `board.bbm` are M4.
//!
//! - A package or platform not yet in the lock is added.
//! - One whose hash differs from the lock is refused unless
//!   `--update-lock`, which rewrites only the entries that changed.
//! - `--check` writes nothing and fails if the lock would change (CI).
//!
//! `bb run --netlist <board.cir>` refuses a board whose packages or
//! platforms differ from its `bb.lock` (or that has none), and never
//! writes the lock.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use bbpkg::lock::{self, Drift, Lock, LockedPackage, LockedPlatform};
use bbpkg::schema::Registry;
use bbpkg::SearchPath;

use crate::board::{self, Board};
use crate::platforms;

const USAGE: &str = "\
usage: bb open <board.cir> [--check] [--update-lock]

  Resolves the board file's package= parts (ADR-006) on the package search
  path (breadboard.dev's packages/, or $BB_FIRST_PARTY_PACKAGES) and writes
  bb.lock next to it: each package's version and tree hash, each engine
  platform's content hash.
  --check        write nothing; fail if bb.lock is missing or would change
  --update-lock  accept packages or platforms whose hash changed (rewrites
                 only those entries; without it they are refused)
  KiCad projects (.kicad_pro, .net) and bb.toml are Phase 1 M4.";

/// The package search path for a board file (ADR-006 D1). A board file has
/// no `bb.toml`, so it is the first-party directory alone.
pub fn search_path() -> SearchPath {
    SearchPath {
        dirs: vec![bbpkg::first_party_dir()],
    }
}

/// Parse a board file, resolving packages on the search path. `fast`
/// (`bb run`): take a package at its conventional path without loading the
/// whole search path (the lock pins its content anyway); `bb open` loads
/// everything, so an id provided twice is an error.
pub fn parse_board(path: &Path, fast: bool) -> Result<Board, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let sp = search_path();
    let reg = Registry::current();
    let lookup = |spec: &str| {
        if fast {
            sp.find_fast(spec, &reg)
        } else {
            sp.find(spec, &reg)
        }
    };
    board::parse_with(&text, Some(&lookup)).map_err(|e| format!("{}: {e}", path.display()))
}

/// Where a board file's lock lives.
pub fn lock_path(board_file: &Path) -> PathBuf {
    board_file
        .parent()
        .map(|d| {
            if d.as_os_str().is_empty() {
                Path::new(".")
            } else {
                d
            }
        })
        .unwrap_or(Path::new("."))
        .join("bb.lock")
}

/// What the board resolves to now: its packages and platforms as lock
/// entries.
pub fn resolved(
    board_file: &Path,
    b: &Board,
) -> Result<(Vec<LockedPackage>, Vec<LockedPlatform>), String> {
    let proj = std::fs::canonicalize(lock_path(board_file).parent().unwrap())
        .map_err(|e| format!("{}: {e}", board_file.display()))?;
    let mut pk = Vec::new();
    let mut pl: Vec<LockedPlatform> = Vec::new();
    for p in &b.packages {
        let dir = std::fs::canonicalize(&p.dir).map_err(|e| format!("{}: {e}", p.dir.display()))?;
        pk.push(LockedPackage {
            id: p.part.id.clone(),
            version: p.part.version.clone(),
            path: lock::relative_path(&proj, &dir),
            tree: lock::tree_hash(&dir)?,
        });
        if let Some(pid) = p
            .part
            .behavioral
            .as_ref()
            .and_then(|x| x.platform.as_deref())
        {
            let def = platforms::find(pid).ok_or_else(|| format!("platform {pid}: unknown"))?;
            if !pl.iter().any(|x| x.id == def.lock_id()) {
                pl.push(def.locked());
            }
        }
    }
    Ok((pk, pl))
}

/// `bb run`'s check: a board with packages needs a current `bb.lock`.
pub fn check_lock(board_file: &Path, b: &Board) -> Result<(), String> {
    if b.packages.is_empty() {
        return Ok(());
    }
    let (pk, pl) = resolved(board_file, b)?;
    let lp = lock_path(board_file);
    let lock = Lock::read(&lp, &Registry::current())?;
    let d = lock::drift(lock.as_ref(), &pk, &pl);
    if d.is_empty() {
        return Ok(());
    }
    let mut msg = format!(
        "{} does not match what {} resolves to (ADR-006 D8):",
        lp.display(),
        board_file.display()
    );
    if lock.is_none() {
        msg = format!(
            "{} is missing (a board with packages needs one, ADR-006 D8):",
            lp.display()
        );
    }
    for x in &d {
        msg.push_str(&format!("\n  {}", x.describe()));
    }
    msg.push_str(&format!(
        "\nrun `bb open {}`{}",
        board_file.display(),
        if d.iter().any(Drift::is_mismatch) {
            " --update-lock` if the change is intended"
        } else {
            ""
        }
    ));
    Err(msg.replace("``", "`"))
}

pub fn main(args: &[String]) -> ExitCode {
    match run(args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("bb open: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<ExitCode, String> {
    let mut check = false;
    let mut update = false;
    let mut target: Option<PathBuf> = None;
    for a in args {
        match a.as_str() {
            "--check" => check = true,
            "--update-lock" => update = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(ExitCode::SUCCESS);
            }
            s if s.starts_with('-') => return Err(format!("unknown option {s}\n{USAGE}")),
            s => {
                if target.replace(PathBuf::from(s)).is_some() {
                    return Err(format!("one board file at a time\n{USAGE}"));
                }
            }
        }
    }
    let target = target.ok_or_else(|| USAGE.to_string())?;
    if target.is_dir()
        || target
            .extension()
            .is_some_and(|e| e == "kicad_pro" || e == "net")
    {
        return Err(format!(
            "{}: KiCad projects and bb.toml projects are Phase 1 M4; this bb opens board files (*.cir)",
            target.display()
        ));
    }
    let b = parse_board(&target, false)?;
    for w in &b.warnings {
        eprintln!("bb open: warning: {}: {w}", target.display());
    }
    let mut out = std::io::stdout().lock();
    use std::io::Write;
    for p in &b.parts {
        let _ = writeln!(
            out,
            "bb open: {} -> {} (engine {}, platform {})",
            p.reference,
            p.package
                .as_deref()
                .unwrap_or("(no package: engine= platform= in the board file)"),
            p.engine,
            p.platform
        );
    }
    if b.packages.is_empty() {
        let _ = writeln!(out, "bb open: no package= parts, so no bb.lock is needed");
        return Ok(ExitCode::SUCCESS);
    }
    let (pk, pl) = resolved(&target, &b)?;
    let lp = lock_path(&target);
    let reg = Registry::current();
    let lock = Lock::read(&lp, &reg)?;
    let d = lock::drift(lock.as_ref(), &pk, &pl);
    for x in &d {
        let _ = writeln!(out, "bb open: {}", x.describe());
    }
    if d.is_empty() {
        let _ = writeln!(out, "bb open: {} is current", lp.display());
        return Ok(ExitCode::SUCCESS);
    }
    if check {
        return Err(format!(
            "--check: {} would change ({} entries)",
            lp.display(),
            d.len()
        ));
    }
    let mismatches: Vec<&Drift> = d.iter().filter(|x| x.is_mismatch()).collect();
    if !mismatches.is_empty() && !update {
        return Err(format!(
            "refusing: {} entr{} of {} changed (ADR-006 D8); review the change, then run `bb open {} --update-lock`",
            mismatches.len(),
            if mismatches.len() == 1 { "y" } else { "ies" },
            lp.display(),
            target.display()
        ));
    }
    let new = lock::apply(lock.as_ref(), &d);
    std::fs::write(&lp, new.to_text()).map_err(|e| format!("write {}: {e}", lp.display()))?;
    let _ = writeln!(
        out,
        "bb open: wrote {} ({} entr{} changed)",
        lp.display(),
        d.len(),
        if d.len() == 1 { "y" } else { "ies" }
    );
    Ok(ExitCode::SUCCESS)
}
