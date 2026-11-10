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
use crate::import;
use crate::platforms;
use crate::project::{Project, Source, PROJECT_FILE};

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
    let target = target.unwrap_or_else(|| PathBuf::from("."));
    if target.is_dir() || target.file_name().is_some_and(|n| n == PROJECT_FILE) {
        let dir = import::project_dir(Some(&target))?;
        return open_project(&dir, check, update);
    }
    if target
        .extension()
        .is_some_and(|e| e == "kicad_pro" || e == "net")
    {
        let dir = target
            .parent()
            .map(|d| {
                if d.as_os_str().is_empty() {
                    Path::new(".")
                } else {
                    d
                }
            })
            .unwrap_or(Path::new("."))
            .to_path_buf();
        let toml_path = dir.join(PROJECT_FILE);
        let file = target
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("not a file name")?
            .to_string();
        if toml_path.is_file() {
            let p = Project::read(&dir)?;
            if !p.boards.iter().any(|b| match &b.source {
                Source::Netlist(f) | Source::Kicad(f) => *f == file,
            }) {
                return Err(format!(
                    "{}: {} exists and does not name {file}; open the project directory instead",
                    target.display(),
                    toml_path.display()
                ));
            }
        } else if !check {
            // The only time bb open writes bb.toml (ADR-007 D7).
            let key = if file.ends_with(".net") {
                "netlist"
            } else {
                "kicad"
            };
            let text = format!(
                "# bb.toml: a Breadboard project (ADR-007 D5, spec/project.md), created by `bb open {file}`.\n\
                 schema = \"bb-project/0\"\n\n[[board]]\nid = \"main\"\n{key} = {}\nground = \"GND\"\n",
                bbpkg::toml::quote(&file)
            );
            std::fs::write(&toml_path, text)
                .map_err(|e| format!("write {}: {e}", toml_path.display()))?;
            println!("bb open: wrote a minimal {}", toml_path.display());
        } else {
            return Err(format!("--check: {} is missing", toml_path.display()));
        }
        return open_project(&dir, check, update);
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

/// Write `path` only when its content changes (so a second `bb open` does
/// not touch the file's timestamp either).
fn write_if_changed(path: &Path, text: &str) -> Result<bool, String> {
    if std::fs::read(path).ok().as_deref() == Some(text.as_bytes()) {
        return Ok(false);
    }
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("create {}: {e}", d.display()))?;
    }
    std::fs::write(path, text).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(true)
}

/// `bb open` on a project (ADR-007 D7): import every board, resolve, and
/// write `board.bbm`, `bb.lock` and `.bb/` (`--check`: write nothing, fail
/// if `board.bbm` or `bb.lock` would change).
fn open_project(dir: &Path, check: bool, update: bool) -> Result<ExitCode, String> {
    use std::io::Write;
    let p = Project::read(dir)?;
    let imp = import::import(&p, import::Mode::Open)?;
    let mut out = std::io::stdout().lock();
    for w in &imp.warnings {
        eprintln!("bb open: warning: {w}");
    }
    let n_inst = imp.board.instances.len();
    let _ = writeln!(
        out,
        "bb open: {}: {} board{}, {} instances, {} nets",
        dir.display(),
        p.boards.len(),
        if p.boards.len() == 1 { "" } else { "s" },
        n_inst,
        imp.board.nets.len()
    );
    let _ = writeln!(
        out,
        "bb open: resolution stages: {}",
        imp.stage_counts
            .iter()
            .map(|(k, v)| format!("{k} {v}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let _ = writeln!(
        out,
        "bb open: statuses: {}",
        imp.status_counts
            .iter()
            .map(|(k, v)| format!("{k} {v}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    if !imp.unresolved.is_empty() {
        let _ = writeln!(
            out,
            "bb open: unresolved (unmodelled; stubs in .bb/unresolved.toml): {}",
            imp.unresolved.join(", ")
        );
    }
    // Bindings, probes and targets must name resolved instances (D7).
    let problems = crate::elab::check_names(&p, &imp.board);
    let bbm_text = imp.board.to_text();
    let bbm_path = dir.join(import::BOARD_FILE);
    let lp = dir.join("bb.lock");
    let reg = Registry::current();
    let lock = Lock::read(&lp, &reg)?;
    let d = lock::drift(lock.as_ref(), &imp.locked_packages, &imp.locked_platforms);
    for x in &d {
        let _ = writeln!(out, "bb open: {}", x.describe());
    }
    if check {
        let mut errs = Vec::new();
        if std::fs::read(&bbm_path).ok().as_deref() != Some(bbm_text.as_bytes()) {
            errs.push(format!("{} would change", bbm_path.display()));
        }
        if !d.is_empty() {
            errs.push(format!(
                "{} would change ({} entries)",
                lp.display(),
                d.len()
            ));
        }
        errs.extend(problems);
        if !errs.is_empty() {
            return Err(format!("--check: {}", errs.join("; ")));
        }
        let _ = writeln!(out, "bb open: --check: board.bbm and bb.lock are current");
        return Ok(ExitCode::SUCCESS);
    }
    let mismatches = d.iter().filter(|x| x.is_mismatch()).count();
    if mismatches > 0 && !update {
        return Err(format!(
            "refusing: {mismatches} entr{} of {} changed (ADR-006 D8); review the change, then run `bb open {} --update-lock`",
            if mismatches == 1 { "y" } else { "ies" },
            lp.display(),
            dir.display()
        ));
    }
    if !d.is_empty() {
        let new = lock::apply(lock.as_ref(), &d);
        std::fs::write(&lp, new.to_text()).map_err(|e| format!("write {}: {e}", lp.display()))?;
        let _ = writeln!(out, "bb open: wrote {}", lp.display());
    }
    let dot = dir.join(import::DOT_BB);
    for (id, text) in &imp.exported {
        write_if_changed(&dot.join(format!("{id}.net")), text)?;
    }
    write_if_changed(
        &dot.join("import.json"),
        &import::import_json_with_host(&imp.import_json),
    )?;
    write_if_changed(&dot.join("resolution.json"), &imp.resolution.to_canonical())?;
    write_if_changed(&dot.join("unresolved.toml"), &imp.unresolved_toml)?;
    let changed = write_if_changed(&bbm_path, &bbm_text)?;
    let _ = writeln!(
        out,
        "bb open: {} {} (sha256 {})",
        if changed { "wrote" } else { "unchanged:" },
        bbm_path.display(),
        bbpkg::sha256::hex_digest(bbm_text.as_bytes())
    );
    if !problems.is_empty() {
        for e in &problems {
            eprintln!("bb open: {e}");
        }
        return Ok(ExitCode::FAILURE);
    }
    Ok(ExitCode::SUCCESS)
}
