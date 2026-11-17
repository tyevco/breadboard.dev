//! Registries in the CLI (ADR-011): which registries a project or board
//! file reaches, vendor files, and `bb registry`, the tools a registry
//! repository's CI runs (D8).
//!
//! - `bb registry init <dir> --key <minisign public key> [--dl <template>]`
//! - `bb registry publish <dir> --pubtime <YYYY-MM-DD> <package dir>...`
//! - `bb registry yank <dir> <id>@<vers> [--undo]`
//! - `bb registry remove <dir> <id>@<vers> --reason <text>`
//! - `bb registry snapshot <dir>`: bump the serial and write `snapshot.json`
//!   (sign it with `minisign -S -m <dir>/snapshot.json`)
//! - `bb registry check <dir> [--base <dir>] [--author <login>]
//!   [--approver <login>]...`: the PR checks
//! - `bb registry verify <url> --key <key>`: a client's full check of a
//!   published registry (every index file, every archive)
//!
//! None of them signs: the registry key is only ever used by `minisign`,
//! in the registry's CI (D3).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use bbpkg::lint::{self, Level};
use bbpkg::lock::{LockedPackage, VendorFile};
use bbpkg::package::{self, Package};
use bbpkg::registry::{
    self, Access, Config, Dep, IndexLine, IndexProvenance, Remote, Resolver, Snapshot,
};
use bbpkg::schema::Registry as Schemas;
use bbpkg::semver::Version;
use bbpkg::sha256;

use crate::platforms;
use crate::project::Project;

/// The public registry (ADR-011 hosting default 1: GitHub Pages under the
/// owner's account until the `breadboard.dev` domain is confirmed). Not yet
/// published: `docs/registry-publish.md` is the human's step.
pub const PUBLIC_URL: &str = "https://tyevco.github.io/bb-registry/";
/// The public registry's minisign key (key id 12C8A81058868A77). The
/// secret key never leaves the registry's CI secret (D3);
/// `spec/registry.md` section 8 is the rotation procedure.
pub const PUBLIC_KEY: &str = "RWR3ioZYEKjIEvJyXiAwNL6czjJWdX3+bxrhw4QWChjFKUCBHjYT6EGI";

/// The public registry this bb uses: the compiled one; or
/// `$BB_PUBLIC_REGISTRY`, a mirror (verified with the compiled key) or a
/// test fixture (with its own `$BB_PUBLIC_REGISTRY_KEY`); or none with
/// `BB_PUBLIC_REGISTRY=off`.
pub fn public_remote() -> Result<Option<Remote>, String> {
    let url = std::env::var("BB_PUBLIC_REGISTRY")
        .ok()
        .filter(|v| !v.is_empty());
    match url.as_deref() {
        Some("off") => Ok(None),
        Some(u) => {
            registry::check_url(u)?;
            let key = std::env::var("BB_PUBLIC_REGISTRY_KEY")
                .ok()
                .filter(|k| !k.is_empty())
                .unwrap_or_else(|| PUBLIC_KEY.to_string());
            registry::check_key(&key)?;
            Ok(Some(Remote {
                name: "public".into(),
                url: registry::normalize_url(u),
                keys: vec![key],
            }))
        }
        None => Ok(Some(Remote {
            name: "public".into(),
            url: PUBLIC_URL.into(),
            keys: vec![PUBLIC_KEY.into()],
        })),
    }
}

/// The resolver of a project (its `[registries.<name>]`, routed) or of a
/// board file (`None`: the public registry only).
pub fn resolver(p: Option<&Project>, access: Access) -> Result<Resolver, String> {
    let routed = p
        .map(|p| {
            p.registries
                .iter()
                .map(|r| {
                    (
                        Remote {
                            name: r.name.clone(),
                            url: registry::normalize_url(&r.url),
                            keys: vec![r.key.clone()],
                        },
                        r.namespaces.clone(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    Resolver::new(routed, public_remote()?, access)
}

/// A locked package entry for a resolved package: its registry source when
/// the resolver supplied it, else `path+<path relative to base>`.
pub fn locked_entry(
    res: &Resolver,
    pkg: &Package,
    base: &Path,
    tree: &str,
) -> Result<LockedPackage, String> {
    let dir = std::fs::canonicalize(&pkg.dir).map_err(|e| format!("{}: {e}", pkg.dir.display()))?;
    Ok(match res.source_of(&dir) {
        Some(source) => LockedPackage {
            id: pkg.part.id.clone(),
            version: pkg.part.version.clone(),
            source,
            tree: tree.to_string(),
        },
        None => LockedPackage::at_path(
            &pkg.part.id,
            &pkg.part.version,
            &bbpkg::lock::relative_path(base, &dir),
            tree,
        ),
    })
}

/// The vendor files the packages declare (`[behavioral] svd`, ADR-011 D5).
pub fn vendor_files<'a>(pkgs: impl IntoIterator<Item = &'a Package>) -> Vec<VendorFile> {
    let mut out: Vec<VendorFile> = pkgs
        .into_iter()
        .filter_map(|p| {
            let s = p.part.behavioral.as_ref()?.svd.as_ref()?;
            Some(VendorFile {
                url: s.url.clone(),
                sha256: s.sha256.clone(),
                license: s.license.clone(),
                used_by: p.part.id.clone(),
            })
        })
        .collect();
    out.sort_by(|a, b| (&a.used_by, &a.url).cmp(&(&b.used_by, &b.url)));
    out.dedup();
    out
}

/// Fetch a vendor file into the cache (`<cache>/vendor/<sha256>`) and check
/// its sha256 (D5: fetched at use, never redistributed). `bb run` never
/// calls this.
pub fn fetch_vendor(v: &VendorFile, offline: bool) -> Result<PathBuf, String> {
    let root = registry::cache_root()?;
    let p = root.join("vendor").join(&v.sha256);
    if let Ok(b) = std::fs::read(&p) {
        if sha256::hex_digest(&b) == v.sha256 {
            return Ok(p);
        }
    }
    if offline {
        return Err(format!(
            "vendor file {} of {}: not in the cache (bb open --offline fetches nothing)",
            v.url, v.used_by
        ));
    }
    let b = registry::fetch(&v.url)?
        .ok_or_else(|| format!("vendor file {} of {}: not found", v.url, v.used_by))?;
    let got = sha256::hex_digest(&b);
    if got != v.sha256 {
        return Err(format!(
            "vendor file {} of {}: sha256 {got}, the package declares {}: refused (ADR-011 D5)",
            v.url, v.used_by, v.sha256
        ));
    }
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("create {}: {e}", d.display()))?;
    }
    std::fs::write(&p, &b).map_err(|e| format!("write {}: {e}", p.display()))?;
    Ok(p)
}

// --- bb registry -----------------------------------------------------------------

const USAGE: &str = "\
usage: bb registry init <dir> --key <minisign public key> [--dl <url template>]
       bb registry publish <dir> --pubtime <YYYY-MM-DD> <package dir>...
       bb registry yank <dir> <id>@<version> [--undo]
       bb registry remove <dir> <id>@<version> --reason <text>
       bb registry snapshot <dir>
       bb registry check <dir> [--base <dir>] [--author <login>] [--approver <login>]...
       bb registry verify <url> --key <minisign public key>

  The tools of a registry repository's CI (ADR-011 D8, spec/registry.md).
  publish appends index lines and archives (versions are immutable);
  snapshot bumps the serial and writes snapshot.json, which the registry's
  CI then signs: minisign -S -s <secret key> -m <dir>/snapshot.json.
  check runs the PR checks; verify is a client's full check of a
  published registry (signature, every index file and archive).";

pub fn main(args: &[String]) -> ExitCode {
    let r = match args.first().map(String::as_str) {
        Some("init") => init(&args[1..]),
        Some("publish") => publish_cmd(&args[1..]),
        Some("yank") => yank_cmd(&args[1..]),
        Some("remove") => remove_cmd(&args[1..]),
        Some("snapshot") => snapshot_cmd(&args[1..]),
        Some("check") => check_cmd(&args[1..]),
        Some("verify") => verify_cmd(&args[1..]),
        Some("-h" | "--help" | "help") => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        _ => Err(USAGE.to_string()),
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("bb registry: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Options and positional arguments.
struct Args {
    pos: Vec<String>,
    opts: BTreeMap<String, Vec<String>>,
    flags: BTreeSet<String>,
}

fn parse_args(args: &[String], with_value: &[&str], flags: &[&str]) -> Result<Args, String> {
    let mut a = Args {
        pos: Vec::new(),
        opts: BTreeMap::new(),
        flags: BTreeSet::new(),
    };
    let mut it = args.iter();
    while let Some(x) = it.next() {
        if with_value.contains(&x.as_str()) {
            let v = it.next().ok_or_else(|| format!("{x} needs a value"))?;
            a.opts.entry(x.clone()).or_default().push(v.clone());
        } else if flags.contains(&x.as_str()) {
            a.flags.insert(x.clone());
        } else if x.starts_with('-') {
            return Err(format!("unknown option {x}\n{USAGE}"));
        } else {
            a.pos.push(x.clone());
        }
    }
    Ok(a)
}

impl Args {
    fn one(&self, k: &str) -> Option<&str> {
        self.opts.get(k).and_then(|v| v.last()).map(String::as_str)
    }
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("create {}: {e}", d.display()))?;
    }
    std::fs::write(path, bytes).map_err(|e| format!("write {}: {e}", path.display()))
}

fn read_config(dir: &Path) -> Result<Config, String> {
    let p = dir.join("config.json");
    let t = std::fs::read_to_string(&p).map_err(|e| format!("read {}: {e}", p.display()))?;
    Config::parse(&t)
}

fn index_path(dir: &Path, id: &str) -> PathBuf {
    dir.join("index").join(format!("{id}.jsonl"))
}

fn read_index(dir: &Path, id: &str) -> Result<Vec<IndexLine>, String> {
    match std::fs::read_to_string(index_path(dir, id)) {
        Ok(t) => registry::parse_index(id, &t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("read index of {id}: {e}")),
    }
}

fn write_index(dir: &Path, id: &str, lines: &[IndexLine]) -> Result<(), String> {
    let mut t = String::new();
    for l in lines {
        t.push_str(&l.to_line());
        t.push('\n');
    }
    write(&index_path(dir, id), t.as_bytes())
}

/// Every index file of a registry directory: `(id, relative path)`.
fn index_files(dir: &Path) -> Result<Vec<(String, String)>, String> {
    let root = dir.join("index");
    let mut out = Vec::new();
    if !root.is_dir() {
        return Ok(out);
    }
    for ns in std::fs::read_dir(&root).map_err(|e| format!("read {}: {e}", root.display()))? {
        let ns = ns.map_err(|e| e.to_string())?;
        if !ns.path().is_dir() {
            return Err(format!(
                "{}: not a namespace directory",
                ns.path().display()
            ));
        }
        for f in std::fs::read_dir(ns.path()).map_err(|e| e.to_string())? {
            let f = f.map_err(|e| e.to_string())?;
            let name = f.file_name().to_string_lossy().into_owned();
            let stem = name
                .strip_suffix(".jsonl")
                .ok_or_else(|| format!("{}: not an index file", f.path().display()))?;
            let id = format!("{}/{stem}", ns.file_name().to_string_lossy());
            out.push((id.clone(), format!("index/{id}.jsonl")));
        }
    }
    out.sort();
    Ok(out)
}

fn split_spec(spec: &str) -> Result<(&str, &str), String> {
    spec.split_once('@')
        .filter(|(i, v)| i.contains('/') && Version::parse(v).is_ok())
        .ok_or_else(|| format!("{spec:?}: expected <namespace>/<name>@MAJOR.MINOR.PATCH"))
}

fn init(args: &[String]) -> Result<(), String> {
    let a = parse_args(args, &["--key", "--dl"], &[])?;
    let [dir] = a.pos.as_slice() else {
        return Err(USAGE.into());
    };
    let dir = Path::new(dir);
    let key = a
        .one("--key")
        .ok_or("--key <minisign public key> is required")?;
    let key = match std::fs::read_to_string(key) {
        // A minisign .pub file: the key is its last line.
        Ok(t) => t.lines().last().unwrap_or("").trim().to_string(),
        Err(_) => key.to_string(),
    };
    registry::check_key(&key)?;
    if dir.join("config.json").exists() {
        return Err(format!(
            "{}: already a registry (config.json exists)",
            dir.display()
        ));
    }
    let c = Config {
        dl: a.one("--dl").unwrap_or("dl/{id}/{vers}.tar").to_string(),
        keys: vec![key],
    };
    Config::parse(&c.to_json().to_canonical())?;
    write(
        &dir.join("config.json"),
        c.to_json().to_canonical().as_bytes(),
    )?;
    std::fs::create_dir_all(dir.join("index")).map_err(|e| e.to_string())?;
    println!("bb registry: initialised {}", dir.display());
    Ok(())
}

/// Lint a package as `bb pkg lint --publish` does (the D11 names, the D7
/// licence gate); errors only. `check_behavioral` is false where `bb registry
/// check` re-reads a version already in the index: versions are immutable
/// (D2), so a platform or device this bb has since retired is not an error
/// there; `bb registry publish` (and the PR check's `bb pkg lint --publish` on
/// the changed packages, D8) check it for every new version (ADR-011
/// Amendment 3).
fn publish_lint(
    pkg: &Package,
    schemas: &Schemas,
    chip: Option<&Package>,
    check_behavioral: bool,
) -> Vec<String> {
    let pl = platforms::ids();
    let devs = bbdev::ids();
    let ctx = lint::Context {
        platforms: &pl,
        devices: &devs,
        first_party: false,
        chip,
        publish: true,
        check_behavioral,
    };
    lint::lint(pkg, &ctx, schemas)
        .into_iter()
        .filter(|f| f.level == Level::Error)
        .map(|f| f.msg)
        .collect()
}

/// The index line's provenance summary: the least confident view's (the
/// package-level block for a package without views).
fn provenance_summary(pkg: &Package) -> IndexProvenance {
    let rank = |c: &str| match c {
        "unchecked" => 0,
        "self_checked" => 1,
        "datasheet_checked" => 2,
        "measured" => 3,
        _ => 0,
    };
    let p = &pkg.part;
    let mut provs: Vec<&package::Provenance> = p
        .analog
        .iter()
        .filter_map(|v| v.provenance.as_ref().or(p.provenance.as_ref()))
        .collect();
    if provs.is_empty() {
        provs.extend(p.provenance.as_ref());
    }
    let least = provs.iter().min_by_key(|x| rank(&x.confidence));
    IndexProvenance {
        source: least.map(|x| x.source.clone()),
        confidence: least.map(|x| x.confidence.clone()),
        // Written by the registry's CI from PR approvals (ADR-012, E2).
        reviewed_by: Vec::new(),
        generated_by: least.and_then(|x| x.generated_by.clone()),
        // The claim checker is milestone E2: null until it runs.
        unverified_claims: None,
    }
}

/// Publish one package into a registry directory. Returns its line and
/// whether it was new.
pub fn publish_one(
    dir: &Path,
    pkg_dir: &Path,
    pubtime: &str,
    schemas: &Schemas,
) -> Result<(IndexLine, bool), String> {
    let pkg = package::load(pkg_dir, schemas)?;
    let id = pkg.part.id.clone();
    let vers = pkg.part.version.clone();
    // A module's chip: a published version of it (D1: deps).
    let mut deps = Vec::new();
    let mut chip_pkg = None;
    if let Some(m) = &pkg.part.module {
        let lines = read_index(dir, &m.chip)?;
        let best = lines
            .iter()
            .filter(|l| l.available())
            .max_by_key(|l| l.version())
            .ok_or_else(|| {
                format!(
                    "{id} {vers}: its chip {} is not published here: publish it first",
                    m.chip
                )
            })?;
        deps.push(Dep {
            id: m.chip.clone(),
            req: format!("^{}", best.vers),
        });
        let tmp = std::env::temp_dir().join(format!(
            "bb-registry-chip-{}-{}",
            std::process::id(),
            best.tree.trim_start_matches("sha256:")
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        let bytes = std::fs::read(dir.join(format!("dl/{}/{}.tar", best.id, best.vers)))
            .map_err(|e| format!("read the archive of {} {}: {e}", best.id, best.vers))?;
        bbpkg::archive::extract(&bytes, &tmp)?;
        chip_pkg = Some(package::load(&tmp, schemas)?);
        let _ = std::fs::remove_dir_all(&tmp);
    }
    let errs = publish_lint(&pkg, schemas, chip_pkg.as_ref(), true);
    if !errs.is_empty() {
        return Err(format!(
            "{id} {vers} cannot be published:\n  {}",
            errs.join("\n  ")
        ));
    }
    let tar = bbpkg::archive::pack(pkg_dir)?;
    if tar.len() as u64 > registry::MAX_ARCHIVE {
        return Err(format!(
            "{id} {vers}: the archive is {} bytes, over the {} byte cap (ADR-011 D8)",
            tar.len(),
            registry::MAX_ARCHIVE
        ));
    }
    let tree = bbpkg::lock::tree_hash(pkg_dir)?;
    let mut lines = read_index(dir, &id)?;
    if let Some(l) = lines.iter().find(|l| l.vers == vers) {
        if l.tree == tree {
            return Ok((l.clone(), false));
        }
        return Err(format!(
            "{id} {vers} is already published with tree {}, and this one is {tree}: versions are \
             immutable, bump the version (ADR-011 D2)",
            l.tree
        ));
    }
    let platforms: Vec<String> = pkg
        .part
        .behavioral
        .as_ref()
        .and_then(|b| b.platform.as_deref())
        .and_then(platforms::find)
        .map(|d| vec![d.lock_id()])
        .unwrap_or_default();
    let line = IndexLine {
        id: id.clone(),
        vers: vers.clone(),
        tree,
        size: tar.len() as u64,
        license: pkg.part.license.clone(),
        schema: pkg.part.schema.clone(),
        deps,
        provenance: provenance_summary(&pkg),
        platforms,
        yanked: false,
        pubtime: pubtime.to_string(),
        removed: None,
    };
    write(&dir.join(format!("dl/{id}/{vers}.tar")), &tar)?;
    lines.push(line.clone());
    write_index(dir, &id, &lines)?;
    Ok((line, true))
}

fn check_pubtime(t: &str) -> Result<(), String> {
    let b = t.as_bytes();
    let ok = b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit());
    if ok {
        Ok(())
    } else {
        Err(format!("--pubtime {t:?}: expected YYYY-MM-DD"))
    }
}

fn publish_cmd(args: &[String]) -> Result<(), String> {
    let a = parse_args(args, &["--pubtime"], &[])?;
    let (dir, pkgs) = a.pos.split_first().ok_or(USAGE)?;
    let dir = Path::new(dir);
    read_config(dir)?;
    let pubtime = a
        .one("--pubtime")
        .ok_or("--pubtime <YYYY-MM-DD> is required")?;
    check_pubtime(pubtime)?;
    let schemas = Schemas::current();
    // Chips before the modules that depend on them.
    let mut order: Vec<PathBuf> = Vec::new();
    for p in pkgs {
        order.extend(package::discover(Path::new(p))?);
    }
    let is_module = |d: &Path| {
        package::load(d, &schemas)
            .map(|p| p.part.module.is_some())
            .unwrap_or(false)
    };
    order.sort_by_key(|d| is_module(d));
    if order.is_empty() {
        return Err("no package directory given".into());
    }
    for d in &order {
        let (l, new) = publish_one(dir, d, pubtime, &schemas)?;
        println!(
            "bb registry: {} {} {} ({}, {} bytes)",
            if new { "published" } else { "unchanged" },
            l.id,
            l.vers,
            l.tree,
            l.size
        );
    }
    Ok(())
}

fn yank_cmd(args: &[String]) -> Result<(), String> {
    let a = parse_args(args, &[], &["--undo"])?;
    let [dir, spec] = a.pos.as_slice() else {
        return Err(USAGE.into());
    };
    let dir = Path::new(dir);
    let (id, vers) = split_spec(spec)?;
    let mut lines = read_index(dir, id)?;
    let l = lines
        .iter_mut()
        .find(|l| l.vers == vers)
        .ok_or_else(|| format!("{spec}: not in the index"))?;
    l.yanked = !a.flags.contains("--undo");
    write_index(dir, id, &lines)?;
    println!(
        "bb registry: {spec} {}",
        if a.flags.contains("--undo") {
            "unyanked"
        } else {
            "yanked"
        }
    );
    Ok(())
}

fn remove_cmd(args: &[String]) -> Result<(), String> {
    let a = parse_args(args, &["--reason"], &[])?;
    let [dir, spec] = a.pos.as_slice() else {
        return Err(USAGE.into());
    };
    let dir = Path::new(dir);
    let reason = a.one("--reason").ok_or("--reason <text> is required")?;
    let (id, vers) = split_spec(spec)?;
    let mut lines = read_index(dir, id)?;
    let l = lines
        .iter_mut()
        .find(|l| l.vers == vers)
        .ok_or_else(|| format!("{spec}: not in the index"))?;
    l.removed = Some(reason.to_string());
    l.yanked = true;
    write_index(dir, id, &lines)?;
    let tar = dir.join(format!("dl/{id}/{vers}.tar"));
    match std::fs::remove_file(&tar) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("remove {}: {e}", tar.display())),
    }
    println!("bb registry: {spec} removed ({reason}); its archive is deleted");
    Ok(())
}

/// The snapshot of a registry directory as it stands, at `serial`.
fn snapshot_of(dir: &Path, serial: u64) -> Result<Snapshot, String> {
    let mut files = BTreeMap::new();
    let hash = |rel: &str| -> Result<String, String> {
        let b = std::fs::read(dir.join(rel)).map_err(|e| format!("read {rel}: {e}"))?;
        Ok(format!("sha256:{}", sha256::hex_digest(&b)))
    };
    files.insert("config.json".to_string(), hash("config.json")?);
    for (_, rel) in index_files(dir)? {
        files.insert(rel.clone(), hash(&rel)?);
    }
    Ok(Snapshot { serial, files })
}

fn read_snapshot(dir: &Path) -> Result<Option<Snapshot>, String> {
    match std::fs::read_to_string(dir.join("snapshot.json")) {
        Ok(t) => Snapshot::parse(&t).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("read snapshot.json: {e}")),
    }
}

fn snapshot_cmd(args: &[String]) -> Result<(), String> {
    let a = parse_args(args, &[], &[])?;
    let [dir] = a.pos.as_slice() else {
        return Err(USAGE.into());
    };
    let dir = Path::new(dir);
    read_config(dir)?;
    let serial = read_snapshot(dir)?.map(|s| s.serial + 1).unwrap_or(1);
    let s = snapshot_of(dir, serial)?;
    write(&dir.join("snapshot.json"), s.to_text().as_bytes())?;
    // The old signature no longer matches; CI signs the new snapshot.
    let _ = std::fs::remove_file(dir.join("snapshot.json.minisig"));
    println!(
        "bb registry: snapshot.json serial {serial}, {} files; sign it: minisign -S -s <secret key> -m {}",
        s.files.len(),
        dir.join("snapshot.json").display()
    );
    Ok(())
}

/// `owners.toml`: `default` (the namespaces not listed) and `[namespaces]`.
struct Owners {
    default: Vec<String>,
    namespaces: BTreeMap<String, Vec<String>>,
}

fn owners(dir: &Path) -> Result<Owners, String> {
    let p = dir.join("owners.toml");
    let t = std::fs::read_to_string(&p).map_err(|e| format!("read {}: {e}", p.display()))?;
    let t = bbpkg::toml::parse(&t).map_err(|e| format!("owners.toml: {e}"))?;
    let mut o = bbpkg::obj::Obj::new("owners.toml", &t);
    let default = o.strs("default")?;
    if default.is_empty() {
        return Err("owners.toml: default = [<login>...] is required (ADR-011 D10)".into());
    }
    let mut ns = BTreeMap::new();
    if let Some(mut n) = o.table_opt("namespaces")? {
        for k in n.keys() {
            let v = n.strs(&k)?;
            ns.insert(k, v);
        }
        n.done()?;
    }
    o.done()?;
    Ok(Owners {
        default,
        namespaces: ns,
    })
}

/// Check a registry directory: its files, every line's archive and the
/// publication gates on each extracted package; with `base`, the diff
/// rules of a PR (D2, D8).
pub fn check_dir(
    dir: &Path,
    base: Option<&Path>,
    author: Option<&str>,
    approvers: &[String],
) -> Result<Vec<String>, String> {
    let schemas = Schemas::current();
    let mut errs = Vec::new();
    let mut report = Vec::new();
    read_config(dir).map_err(|e| format!("config.json: {e}"))?;
    let files = index_files(dir)?;
    let mut all: BTreeMap<String, Vec<IndexLine>> = BTreeMap::new();
    let mut names: BTreeMap<String, (String, String)> = BTreeMap::new();
    let tmp_root = std::env::temp_dir().join(format!("bb-registry-check-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp_root);
    for (id, _) in &files {
        let lines = match read_index(dir, id) {
            Ok(l) => l,
            Err(e) => {
                errs.push(e);
                continue;
            }
        };
        for l in &lines {
            let tar = dir.join(format!("dl/{id}/{}.tar", l.vers));
            if l.removed.is_some() {
                if tar.exists() {
                    errs.push(format!(
                        "{id} {}: removed, but its archive is still there",
                        l.vers
                    ));
                }
                continue;
            }
            let bytes = match std::fs::read(&tar) {
                Ok(b) => b,
                Err(e) => {
                    errs.push(format!("{id} {}: archive {}: {e}", l.vers, tar.display()));
                    continue;
                }
            };
            if bytes.len() as u64 != l.size {
                errs.push(format!(
                    "{id} {}: archive is {} bytes, the line says {}",
                    l.vers,
                    bytes.len(),
                    l.size
                ));
            }
            if l.size > registry::MAX_ARCHIVE {
                errs.push(format!(
                    "{id} {}: over the {} byte cap (ADR-011 D8)",
                    l.vers,
                    registry::MAX_ARCHIVE
                ));
            }
            let x = tmp_root.join(format!("{}-{}", id.replace('/', "_"), l.vers));
            if let Err(e) = bbpkg::archive::extract(&bytes, &x) {
                errs.push(format!("{id} {}: {e}", l.vers));
                continue;
            }
            match bbpkg::lock::tree_hash(&x) {
                Ok(t) if t == l.tree => {}
                Ok(t) => errs.push(format!(
                    "{id} {}: the archive's tree is {t}, the line says {}",
                    l.vers, l.tree
                )),
                Err(e) => errs.push(e),
            }
            // Re-packing gives the same bytes (D6).
            if bbpkg::archive::pack(&x).ok().as_deref() != Some(&bytes[..]) {
                errs.push(format!(
                    "{id} {}: the archive is not in the deterministic form (ADR-011 D6)",
                    l.vers
                ));
            }
            match package::load(&x, &schemas) {
                Ok(p) => {
                    if p.part.id != *id
                        || p.part.version != l.vers
                        || p.part.schema != l.schema
                        || p.part.license != l.license
                    {
                        errs.push(format!("{id} {}: the line's id, version, schema or licence differs from its part.toml", l.vers));
                    }
                    // Lint needs the module's chip: published here.
                    let chip = p.part.module.as_ref().and_then(|m| {
                        let cl = read_index(dir, &m.chip).ok()?;
                        let best = cl
                            .iter()
                            .filter(|c| c.removed.is_none())
                            .max_by_key(|c| c.version())?;
                        let cx = tmp_root.join(format!(
                            "chip-{}-{}",
                            m.chip.replace('/', "_"),
                            best.vers
                        ));
                        if !cx.exists() {
                            let b = std::fs::read(
                                dir.join(format!("dl/{}/{}.tar", best.id, best.vers)),
                            )
                            .ok()?;
                            bbpkg::archive::extract(&b, &cx).ok()?;
                        }
                        package::load(&cx, &schemas).ok()
                    });
                    for e in publish_lint(&p, &schemas, chip.as_ref(), false) {
                        errs.push(format!("{id} {}: {e}", l.vers));
                    }
                    // D11: a top-level name defined by two packages.
                    for v in &p.part.analog {
                        let text = std::fs::read_to_string(x.join(&v.file)).unwrap_or_default();
                        for d in bbpkg::spice::definitions(&text).unwrap_or_default() {
                            match names.get(&d.name) {
                                Some((other, t)) if other != id && *t != d.text => {
                                    errs.push(format!(
                                        "{id} {}: view {} defines {}, which {other} defines \
                                         differently (ADR-011 D11)",
                                        l.vers, v.name, d.name
                                    ))
                                }
                                Some(_) => {}
                                None => {
                                    names.insert(d.name.clone(), (id.clone(), d.text.clone()));
                                }
                            }
                        }
                    }
                }
                Err(e) => errs.push(format!("{id} {}: {e}", l.vers)),
            }
        }
        all.insert(id.clone(), lines);
    }
    // An archive without a line.
    let dl = dir.join("dl");
    if dl.is_dir() {
        for f in walk(&dl)? {
            let rel = f
                .strip_prefix(&dl)
                .expect("under")
                .to_string_lossy()
                .replace('\\', "/");
            let ok = rel
                .strip_suffix(".tar")
                .and_then(|r| r.rsplit_once('/'))
                .is_some_and(|(id, v)| {
                    all.get(id).is_some_and(|ls| ls.iter().any(|l| l.vers == v))
                });
            if !ok {
                errs.push(format!("dl/{rel}: an archive without an index line"));
            }
        }
    }
    // The snapshot, when present, covers exactly these files. A PR (with
    // `base`) is checked before CI writes and signs a new one on merge (D8).
    if let Some(s) = read_snapshot(dir)?.filter(|_| base.is_none()) {
        let now = snapshot_of(dir, s.serial)?;
        if now.files != s.files {
            errs.push("snapshot.json does not match the index files and config.json: run bb registry snapshot".into());
        }
    }
    let _ = std::fs::remove_dir_all(&tmp_root);
    if let Some(base) = base {
        // D2: an index line never changes, except `yanked` (and `removed`
        // with its archive deleted); lines are appended only.
        let mut touched: BTreeSet<String> = BTreeSet::new();
        for (id, _) in index_files(base)? {
            let old = read_index(base, &id)?;
            let new = all.get(&id).cloned().unwrap_or_default();
            if new.len() < old.len() {
                errs.push(format!("index/{id}.jsonl: lines were deleted (ADR-011 D2)"));
                continue;
            }
            for (o, n) in old.iter().zip(&new) {
                let mut o2 = o.clone();
                o2.yanked = n.yanked;
                if o.removed.is_none() {
                    o2.removed = n.removed.clone();
                }
                if o2 != *n {
                    errs.push(format!(
                        "index/{id}.jsonl: the line of {} changed beyond yanked/removed (ADR-011 D2)",
                        o.vers
                    ));
                }
                if o != n {
                    touched.insert(id.clone());
                }
                if o.removed.is_none() && n.removed.is_none() {
                    let a = std::fs::read(base.join(format!("dl/{id}/{}.tar", o.vers))).ok();
                    let b = std::fs::read(dir.join(format!("dl/{id}/{}.tar", n.vers))).ok();
                    if a != b {
                        errs.push(format!(
                            "dl/{id}/{}.tar changed (archives are immutable, ADR-011 D2)",
                            o.vers
                        ));
                    }
                }
            }
            if new.len() > old.len() {
                touched.insert(id.clone());
            }
        }
        for id in all.keys() {
            if !base.join(format!("index/{id}.jsonl")).exists() {
                touched.insert(id.clone());
            }
        }
        if let (Some(b), Some(n)) = (read_snapshot(base)?, read_snapshot(dir)?) {
            if n.serial <= b.serial && n != b {
                errs.push(format!(
                    "snapshot.json: serial {} is not above the base's {} (ADR-011 D2)",
                    n.serial, b.serial
                ));
            }
        }
        // D8 item 8: owners.toml for each touched namespace.
        // The base's owners.toml decides: a PR cannot make itself an owner.
        let own = owners(base)?;
        let who: Vec<&str> = author
            .into_iter()
            .chain(approvers.iter().map(String::as_str))
            .collect();
        // A change to owners.toml or config.json is the project's (D10).
        for f in ["owners.toml", "config.json"] {
            if std::fs::read(base.join(f)).ok() != std::fs::read(dir.join(f)).ok()
                && !who.iter().any(|w| own.default.iter().any(|a| a == w))
            {
                errs.push(format!(
                    "{f} changed: only the default owners ({}) may approve that (ADR-011 D10)",
                    own.default.join(", ")
                ));
            }
        }
        for id in &touched {
            let n = id.split('/').next().unwrap_or("");
            let allowed = own.namespaces.get(n).unwrap_or(&own.default);
            if !who.iter().any(|w| allowed.iter().any(|a| a == w)) {
                errs.push(format!(
                    "{id}: namespace {n} is owned by {} (owners.toml); neither the author nor an approver ({}) is one (ADR-011 D8, D10)",
                    allowed.join(", "),
                    if who.is_empty() { "none given".to_string() } else { who.join(", ") }
                ));
            }
        }
        report.push(format!(
            "{} package(s) touched: {}",
            touched.len(),
            touched.iter().cloned().collect::<Vec<_>>().join(", ")
        ));
    }
    let n_lines: usize = all.values().map(Vec::len).sum();
    report.insert(0, format!("{} index files, {n_lines} lines", all.len()));
    if errs.is_empty() {
        Ok(report)
    } else {
        Err(errs.join("\n"))
    }
}

fn walk(d: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(d).map_err(|e| format!("read {}: {e}", d.display()))? {
        let p = e.map_err(|e| e.to_string())?.path();
        if p.is_dir() {
            out.extend(walk(&p)?);
        } else {
            out.push(p);
        }
    }
    out.sort();
    Ok(out)
}

fn check_cmd(args: &[String]) -> Result<(), String> {
    let a = parse_args(args, &["--base", "--author", "--approver"], &[])?;
    let [dir] = a.pos.as_slice() else {
        return Err(USAGE.into());
    };
    let approvers = a.opts.get("--approver").cloned().unwrap_or_default();
    let r = check_dir(
        Path::new(dir),
        a.one("--base").map(Path::new),
        a.one("--author"),
        &approvers,
    )?;
    for l in r {
        println!("bb registry check: {l}");
    }
    println!("bb registry check: PASS");
    Ok(())
}

fn verify_cmd(args: &[String]) -> Result<(), String> {
    let a = parse_args(args, &["--key"], &[])?;
    let [url] = a.pos.as_slice() else {
        return Err(USAGE.into());
    };
    let key = a.one("--key").ok_or("--key is required")?;
    registry::check_key(key)?;
    let remote = Remote {
        name: "verify".into(),
        url: registry::normalize_url(url),
        keys: vec![key.to_string()],
    };
    // A private, empty cache: everything is fetched and checked.
    let cache = std::env::temp_dir().join(format!("bb-registry-verify-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cache);
    let mut s = registry::Session::open(&remote, &cache, false)?;
    let ids: Vec<String> = s
        .snapshot
        .files
        .keys()
        .filter_map(|k| {
            k.strip_prefix("index/")?
                .strip_suffix(".jsonl")
                .map(str::to_string)
        })
        .collect();
    let mut n = 0;
    for id in &ids {
        for l in s.lines(id)?.unwrap_or_default() {
            if l.removed.is_some() {
                continue;
            }
            s.tree(&l)?;
            n += 1;
        }
    }
    let _ = std::fs::remove_dir_all(&cache);
    println!(
        "bb registry verify: {url}: serial {}, {} packages, {n} archives verified (signature, \
         index hashes, tree hashes)",
        s.snapshot.serial,
        ids.len()
    );
    Ok(())
}
