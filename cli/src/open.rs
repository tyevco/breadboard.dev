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
//!
//! ADR-015 D7: a board file's search path starts with its `*bb packages
//! path=<dir>` lines, and `bb open` and `bb run` warn when a locked package
//! resolved somewhere other than its recorded `path`.
//!
//! ADR-011 (Phase 2 E1): an id no local source provides is resolved on a
//! registry (a project's routed `[registries]`, else the public one) and
//! fetched into the cache, only by `bb open` (`--offline`: the cache only);
//! `bb run` takes a locked registry package from the cache by its tree
//! hash and never fetches. A change of source for a locked id needs
//! `--update-lock`, and so does a changed vendor file.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use bbpkg::lock::{self, Drift, Lock, LockedPackage, LockedPlatform, VendorFile};
use bbpkg::registry::{Access, Resolver};
use bbpkg::schema::Registry;
use bbpkg::SearchPath;

use crate::board::{self, Board};
use crate::import;
use crate::platforms;
use crate::project::{Project, Source, PROJECT_FILE};
use crate::registry;

const USAGE: &str = "\
usage: bb open <board.cir> [--check] [--update-lock] [--offline]

  Resolves the board file's package= parts (ADR-006) on the package search
  path (the directories of its `*bb packages path=<dir>` lines, relative to
  the board file, then breadboard.dev's packages/, or
  $BB_FIRST_PARTY_PACKAGES), then, for an id none of them provides, on the
  public registry (ADR-011 D4; a project's [registries] route namespaces),
  and writes bb.lock next to it: each package's version, source and tree
  hash, each vendor file, each engine platform's content hash.
  --check        write nothing; fail if bb.lock is missing or would change
  --update-lock  accept packages, sources, vendor files or platforms that
                 changed (rewrites only those entries; without it they are
                 refused)
  --offline      fetch nothing: registry packages and vendor files from the
                 cache only (bb run never fetches)
  KiCad projects (.kicad_pro, .net) and bb.toml are Phase 1 M4.";

/// The package search path for a board file (ADR-006 D1, ADR-015 D7): the
/// directories its `*bb packages path=` lines name, relative to the board
/// file and in order, then the first-party directory. A named directory
/// that does not exist is an error (a silently skipped one would be the
/// package-path trap again).
pub fn search_path(board_file: &Path, text: &str) -> Result<SearchPath, String> {
    let base = board_file.parent().unwrap_or(Path::new("."));
    let mut dirs = Vec::new();
    for p in board::package_paths(text).map_err(|e| format!("{}: {e}", board_file.display()))? {
        let d = base.join(&p);
        if !d.is_dir() {
            return Err(format!(
                "{}: *bb packages path={p}: {} is not a directory",
                board_file.display(),
                d.display()
            ));
        }
        dirs.push(d);
    }
    dirs.push(bbpkg::first_party_dir());
    Ok(SearchPath { dirs })
}

/// Parse a board file, resolving packages on the search path, then (ADR-011
/// D4) through `res` for an id no local directory provides: the public
/// registry (`bb open`), or the lock's registry trees in the cache (`bb
/// run`, [`Access::Locked`]). `fast` (`bb run`): take a package at its
/// conventional path without loading the whole search path (the lock pins
/// its content anyway); `bb open` loads everything, so an id provided twice
/// is an error.
pub fn parse_board(
    path: &Path,
    fast: bool,
    res: &RefCell<Resolver>,
    lock: Option<&Lock>,
) -> Result<Board, String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let sp = search_path(path, &text)?;
    let reg = Registry::current();
    let lookup = |spec: &str| match sp.try_find(spec, &reg, fast)? {
        Some(p) => Ok(p),
        None => {
            let id = spec.split_once('@').map(|(i, _)| i).unwrap_or(spec);
            let locked = lock.and_then(|l| l.packages.iter().find(|p| p.id == id));
            res.borrow_mut().resolve(spec, locked, &reg).map_err(|e| {
                format!(
                    "{e} (searched {} first)",
                    sp.dirs
                        .iter()
                        .map(|d| d.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
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

/// What a board resolves to now, as lock entries.
pub struct Resolved {
    pub packages: Vec<LockedPackage>,
    pub vendor_files: Vec<VendorFile>,
    pub platforms: Vec<LockedPlatform>,
}

impl Resolved {
    /// Every difference from `lock`.
    pub fn drift(&self, lock: Option<&Lock>) -> Vec<Drift> {
        let mut d = lock::drift(lock, &self.packages, &self.platforms);
        d.extend(lock::vendor_drift(lock, &self.vendor_files));
        d
    }
}

/// What the board resolves to now: its packages (a registry package with
/// its registry source, from `res`), vendor files and platforms as lock
/// entries (a platform as `lock` pins it, `PlatformDef::locked_for`).
pub fn resolved(
    board_file: &Path,
    b: &Board,
    lock: Option<&Lock>,
    res: &Resolver,
) -> Result<Resolved, String> {
    let proj = std::fs::canonicalize(lock_path(board_file).parent().unwrap())
        .map_err(|e| format!("{}: {e}", board_file.display()))?;
    let mut pk = Vec::new();
    let mut pl: Vec<LockedPlatform> = Vec::new();
    for p in &b.packages {
        let dir = std::fs::canonicalize(&p.dir).map_err(|e| format!("{}: {e}", p.dir.display()))?;
        pk.push(registry::locked_entry(
            res,
            p,
            &proj,
            &lock::tree_hash(&dir)?,
        )?);
        if let Some(pid) = p
            .part
            .behavioral
            .as_ref()
            .and_then(|x| x.platform.as_deref())
        {
            let def = platforms::find(pid).ok_or_else(|| format!("platform {pid}: unknown"))?;
            if !pl.iter().any(|x| x.id == def.lock_id()) {
                pl.push(def.locked_for(lock));
            }
        }
        // A device package (ADR-006 D7, Amendment 6): its device identity.
        if let Some(did) = p
            .part
            .behavioral
            .as_ref()
            .filter(|x| x.engine == "device")
            .and_then(|x| x.device.as_deref())
        {
            let def = bbdev::find(did).ok_or_else(|| format!("device {did}: unknown"))?;
            let l = platforms::device_locked(def);
            if !pl.iter().any(|x| x.id == l.id) {
                pl.push(l);
            }
        }
    }
    Ok(Resolved {
        packages: pk,
        vendor_files: registry::vendor_files(&b.packages),
        platforms: pl,
    })
}

/// The lock path warnings of a board file (ADR-015 D7).
fn path_warnings(board_file: &Path, lock: Option<&Lock>, pk: &[LockedPackage]) -> Vec<String> {
    let base = lock_path(board_file);
    let base = base.parent().unwrap_or(Path::new("."));
    match std::fs::canonicalize(base) {
        Ok(b) => lock::path_warnings(lock, pk, &b),
        Err(_) => Vec::new(),
    }
}

/// The lock path warnings of a project (ADR-015 D7): its packages' paths
/// are relative to the project directory.
pub fn project_path_warnings(dir: &Path, lock: Option<&Lock>, pk: &[LockedPackage]) -> Vec<String> {
    match std::fs::canonicalize(dir) {
        Ok(b) => lock::path_warnings(lock, pk, &b),
        Err(_) => Vec::new(),
    }
}

/// `bb run`'s board file: parsed with the lock's registry packages from
/// the cache (never fetched), and checked against its current `bb.lock`.
/// Returns the board and its warnings (path, yank; ADR-015 D7, ADR-011 D2).
pub fn board_for_run(board_file: &Path) -> Result<(Board, Vec<String>), String> {
    let lp = lock_path(board_file);
    let lock = Lock::read(&lp, &Registry::current())?;
    let res = RefCell::new(registry::resolver(None, Access::Locked)?);
    let b = parse_board(board_file, true, &res, lock.as_ref())?;
    let mut w = check_lock(board_file, &b, lock.as_ref(), &res.borrow())?;
    w.extend(res.borrow().warnings.iter().cloned());
    Ok((b, w))
}

/// `bb run`'s check: a board with packages needs a current `bb.lock`.
/// Returns the lock's path warnings (ADR-015 D7).
pub fn check_lock(
    board_file: &Path,
    b: &Board,
    lock: Option<&Lock>,
    res: &Resolver,
) -> Result<Vec<String>, String> {
    if b.packages.is_empty() {
        return Ok(Vec::new());
    }
    let lp = lock_path(board_file);
    let r = resolved(board_file, b, lock, res)?;
    let d = r.drift(lock);
    if d.is_empty() {
        // A bb-lock/0 file is read (its legacy platform hashes too) with the
        // migration hint (ADR-011 D5, ADR-015 Amendment 3).
        let mut w: Vec<String> = lock.and_then(Lock::hint).into_iter().collect();
        w.extend(path_warnings(board_file, lock, &r.packages));
        return Ok(w);
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
    for w in path_warnings(board_file, lock, &r.packages) {
        msg.push_str(&format!("\n  {w}"));
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

/// Fetch (or, offline, find in the cache) every vendor file (ADR-011 D5).
fn fetch_vendor_files(v: &[VendorFile], offline: bool) -> Result<(), String> {
    for f in v {
        registry::fetch_vendor(f, offline)?;
    }
    Ok(())
}

fn run(args: &[String]) -> Result<ExitCode, String> {
    let mut check = false;
    let mut update = false;
    let mut offline = false;
    let mut target: Option<PathBuf> = None;
    for a in args {
        match a.as_str() {
            "--check" => check = true,
            "--update-lock" => update = true,
            "--offline" => offline = true,
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
    let access = if offline {
        Access::Offline
    } else {
        Access::Online
    };
    let target = target.unwrap_or_else(|| PathBuf::from("."));
    if target.is_dir() || target.file_name().is_some_and(|n| n == PROJECT_FILE) {
        let dir = import::project_dir(Some(&target))?;
        return open_project(&dir, check, update, access);
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
                 schema = \"bb-project/1\"\n\n[[board]]\nid = \"main\"\n{key} = {}\nground = \"GND\"\n",
                bbpkg::toml::quote(&file)
            );
            std::fs::write(&toml_path, text)
                .map_err(|e| format!("write {}: {e}", toml_path.display()))?;
            println!("bb open: wrote a minimal {}", toml_path.display());
        } else {
            return Err(format!("--check: {} is missing", toml_path.display()));
        }
        return open_project(&dir, check, update, access);
    }
    let lp = lock_path(&target);
    let reg = Registry::current();
    let mut lock = Lock::read(&lp, &reg)?;
    // A bb-lock/0 file is migrated in memory (ADR-011 D5): its legacy
    // platform hashes become the scoped ones (ADR-015 Amendment 3), and the
    // lock is written as bb-lock/1 even when nothing else changed.
    let migrated = lock.as_mut().is_some_and(platforms::migrate_lock);
    let res = RefCell::new(registry::resolver(None, access)?);
    let b = parse_board(&target, false, &res, lock.as_ref())?;
    for w in &b.warnings {
        eprintln!("bb open: warning: {}: {w}", target.display());
    }
    for w in &res.borrow().warnings {
        eprintln!("bb open: warning: {w}");
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
    let r = resolved(&target, &b, lock.as_ref(), &res.borrow())?;
    for p in &r.packages {
        if !p.source.starts_with(lock::PATH_SOURCE) {
            let _ = writeln!(out, "bb open: {} {} from {}", p.id, p.version, p.source);
        }
    }
    fetch_vendor_files(&r.vendor_files, offline)?;
    for w in path_warnings(&target, lock.as_ref(), &r.packages) {
        eprintln!("bb open: warning: {w}");
    }
    let d = r.drift(lock.as_ref());
    for x in &d {
        let _ = writeln!(out, "bb open: {}", x.describe());
    }
    if d.is_empty() && !migrated {
        let _ = writeln!(out, "bb open: {} is current", lp.display());
        return Ok(ExitCode::SUCCESS);
    }
    if check {
        return Err(format!(
            "--check: {} would change ({} entries{})",
            lp.display(),
            d.len(),
            if migrated {
                "; it is bb-lock/0, the previous major"
            } else {
                ""
            }
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
        "bb open: wrote {} ({} entr{} changed{})",
        lp.display(),
        d.len(),
        if d.len() == 1 { "y" } else { "ies" },
        if migrated {
            "; migrated from bb-lock/0"
        } else {
            ""
        }
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
fn open_project(dir: &Path, check: bool, update: bool, access: Access) -> Result<ExitCode, String> {
    use std::io::Write;
    let p = Project::read(dir)?;
    if let Some(h) = &p.schema_hint {
        eprintln!("bb open: warning: {h}");
    }
    let lp = dir.join("bb.lock");
    let reg = Registry::current();
    let mut lock = Lock::read(&lp, &reg)?;
    // A bb-lock/0 file is migrated in memory (ADR-011 D5, ADR-015
    // Amendment 3), so board.bbm gets the scoped platform hashes too.
    let migrated = lock.as_mut().is_some_and(platforms::migrate_lock);
    let mut res = registry::resolver(Some(&p), access)?;
    let imp = import::import(&p, import::Mode::Open, lock.as_ref(), &mut res)?;
    let mut out = std::io::stdout().lock();
    for w in &imp.warnings {
        eprintln!("bb open: warning: {w}");
    }
    for w in &res.warnings {
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
    for l in &imp.locked_packages {
        if !l.source.starts_with(lock::PATH_SOURCE) {
            let _ = writeln!(out, "bb open: {} {} from {}", l.id, l.version, l.source);
        }
    }
    if !imp.unresolved.is_empty() {
        let _ = writeln!(
            out,
            "bb open: unresolved (unmodelled; stubs in .bb/unresolved.toml): {}",
            imp.unresolved.join(", ")
        );
    }
    // Systems (ADR-007 D5, Amendment 5): what each connection mates.
    for lines in &imp.connections {
        for l in lines {
            let _ = writeln!(out, "bb open: {l}");
        }
    }
    // Bindings, probes and targets must name resolved instances (D7).
    let mut problems = crate::elab::check_names(&p, &imp.board);
    // Devices (ADR-013 D1): parameters, pins and models, as bb run builds them.
    let mut dev_warnings = Vec::new();
    match crate::elab::plan_devices(&p, &imp, &mut dev_warnings) {
        Ok(devs) => {
            for d in &devs {
                let _ = writeln!(
                    out,
                    "bb open: device {}:{} -> {} ({}{}), {} pin{} bound",
                    d.board,
                    d.part.reference,
                    d.package.part.id,
                    d.part.platform,
                    if imp
                        .board
                        .instance(&d.board, &d.part.reference)
                        .is_some_and(|i| i.is_virtual)
                    {
                        ", virtual"
                    } else {
                        ""
                    },
                    d.pins.len(),
                    if d.pins.len() == 1 { "" } else { "s" }
                );
            }
        }
        Err(e) => problems.push(e),
    }
    for w in &dev_warnings {
        eprintln!("bb open: warning: {w}");
    }
    fetch_vendor_files(&imp.vendor_files, access == Access::Offline)?;
    let bbm_text = imp.board.to_text();
    let bbm_path = dir.join(import::BOARD_FILE);
    for w in project_path_warnings(dir, lock.as_ref(), &imp.locked_packages) {
        eprintln!("bb open: warning: {w}");
    }
    let d = imp.drift(lock.as_ref());
    for x in &d {
        let _ = writeln!(out, "bb open: {}", x.describe());
    }
    if check {
        let mut errs = Vec::new();
        if std::fs::read(&bbm_path).ok().as_deref() != Some(bbm_text.as_bytes()) {
            errs.push(format!("{} would change", bbm_path.display()));
        }
        if !d.is_empty() || migrated {
            errs.push(format!(
                "{} would change ({} entries{})",
                lp.display(),
                d.len(),
                if migrated {
                    "; it is bb-lock/0, the previous major"
                } else {
                    ""
                }
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
    if !d.is_empty() || migrated {
        let new = lock::apply(lock.as_ref(), &d);
        std::fs::write(&lp, new.to_text()).map_err(|e| format!("write {}: {e}", lp.display()))?;
        let _ = writeln!(
            out,
            "bb open: wrote {}{}",
            lp.display(),
            if migrated {
                " (migrated from bb-lock/0)"
            } else {
                ""
            }
        );
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
