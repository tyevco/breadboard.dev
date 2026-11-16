//! Package tree hashes and the lockfile `bb.lock` (ADR-006 D8, ADR-011 D5).
//!
//! **Tree hash:** sha256 over the sorted list of `"<relative path>\0<sha256
//! hex of content>\n"` for every regular file in the package directory,
//! excluding dotfiles (any path component starting with `.`) and
//! `tests/out/`. Paths use `/`; bytes are hashed as stored (`.gitattributes`
//! forces LF, so a CRLF checkout changes the hash). Written `sha256:<hex>`.
//!
//! **Platform content hash:** the same list form over the files that make
//! up an engine platform (the `.repl` it loads and the plugin sources it
//! declares, ADR-015 D6), named by file name.
//!
//! **`bb.lock`** (`bb-lock/1`, the Phase 2 schema epoch): TOML, `[[package]]`
//! (id, version, source, tree), `[[vendor_file]]` (url, sha256, license,
//! used_by) and `[[platform]]` (id, content, and the engine-neutral `engine`
//! and `engine_version`) entries sorted by id. `bb open` writes it; `bb open`
//! and `bb run` refuse a package or platform whose hash differs from it
//! unless `bb open --update-lock`, which rewrites only the entries that
//! changed. A `path+` source is informative: identity is the tree hash. `bb
//! open` and `bb run` warn when a locked package resolved somewhere other
//! than its recorded path ([`path_warnings`], ADR-015 D7). `bb-lock/0` (the
//! previous major: `path`, `renode`) is read and migrated in memory.

use std::path::{Path, PathBuf};

use crate::obj::Obj;
use crate::schema::Registry;
use crate::sha256;
use crate::toml::{self, quote};

/// The hash of a `(name, sha256 hex)` list: sha256 of the sorted lines
/// `"<name>\0<hex>\n"`, as `sha256:<hex>`.
pub fn list_hash(entries: &mut [(String, String)]) -> String {
    entries.sort();
    let mut h = sha256::Sha256::new();
    for (name, hex) in entries.iter() {
        h.update(name.as_bytes());
        h.update(b"\0");
        h.update(hex.as_bytes());
        h.update(b"\n");
    }
    format!("sha256:{}", sha256::hex(&h.finish()))
}

/// The content hash of named in-memory files (a platform).
pub fn content_hash(files: &[(&str, &[u8])]) -> String {
    let mut entries: Vec<(String, String)> = files
        .iter()
        .map(|(n, c)| (n.to_string(), sha256::hex_digest(c)))
        .collect();
    list_hash(&mut entries)
}

/// The files a tree hash covers: `(relative path with /, absolute path)`.
pub fn tree_files(dir: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let mut out = Vec::new();
    fn walk(root: &Path, d: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<(), String> {
        let rd = std::fs::read_dir(d).map_err(|e| format!("read {}: {e}", d.display()))?;
        for e in rd {
            let e = e.map_err(|e| format!("read {}: {e}", d.display()))?;
            let name = e.file_name();
            let name = name
                .to_str()
                .ok_or_else(|| format!("{}: a file name that is not UTF-8", e.path().display()))?;
            if name.starts_with('.') {
                continue;
            }
            let p = e.path();
            let rel = p
                .strip_prefix(root)
                .expect("under root")
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            if rel == "tests/out" {
                continue;
            }
            let ft = e.file_type().map_err(|e| format!("{}: {e}", p.display()))?;
            if ft.is_symlink() {
                return Err(format!(
                    "{}: symbolic links are not allowed in a package (the tree hash covers regular files only)",
                    p.display()
                ));
            } else if ft.is_dir() {
                walk(root, &p, out)?;
            } else if ft.is_file() {
                out.push((rel, p));
            }
        }
        Ok(())
    }
    walk(dir, dir, &mut out)?;
    out.sort();
    Ok(out)
}

/// The tree hash of a package directory.
pub fn tree_hash(dir: &Path) -> Result<String, String> {
    let mut entries = Vec::new();
    for (rel, p) in tree_files(dir)? {
        let bytes = std::fs::read(&p).map_err(|e| format!("read {}: {e}", p.display()))?;
        entries.push((rel, sha256::hex_digest(&bytes)));
    }
    Ok(list_hash(&mut entries))
}

/// The prefix of a path source (ADR-011 D5).
pub const PATH_SOURCE: &str = "path+";
/// The prefix of a registry source (ADR-011 D5, `spec/registry.md` section 6).
pub const REGISTRY_SOURCE: &str = "registry+";

/// A locked package. Its `source` (`bb-lock/1`, ADR-011 D5) is
/// `path+<path relative to the lock's directory>` for a local package (every
/// package found on the search path), or `registry+<url>` (one a registry
/// supplied, `crate::registry`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockedPackage {
    pub id: String,
    pub version: String,
    /// `path+<path>` (as found on the search path; informative: identity is
    /// the tree hash) or `registry+<url>`.
    pub source: String,
    pub tree: String,
}

impl LockedPackage {
    /// A package found at `path` (relative to the lock's directory).
    pub fn at_path(id: &str, version: &str, path: &str, tree: &str) -> LockedPackage {
        LockedPackage {
            id: id.to_string(),
            version: version.to_string(),
            source: format!("{PATH_SOURCE}{path}"),
            tree: tree.to_string(),
        }
    }

    /// The path of a `path+` source (`None` for a registry source).
    pub fn path(&self) -> Option<&str> {
        self.source.strip_prefix(PATH_SOURCE)
    }
}

fn check_source(at: &str, s: &str) -> Result<(), String> {
    match (s.strip_prefix(PATH_SOURCE), s.strip_prefix(REGISTRY_SOURCE)) {
        (Some(p), _) if !p.is_empty() => Ok(()),
        (_, Some(u)) if u.starts_with("https://") || u.starts_with("file://") => Ok(()),
        _ => Err(format!(
            "{at}.source {s:?}: expected path+<path> or registry+<https:// or file:// url> \
             (ADR-011 D5)"
        )),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockedPlatform {
    /// `<engine>:<name>@<major>`, e.g. `renode:stm32f103-offline@1`.
    pub id: String,
    pub content: String,
    /// The engine and its version where it has one (`renode`, `1.17.0`):
    /// `bb-lock/1`'s engine-neutral keys (ADR-011 D5). `bb-lock/0` wrote
    /// `renode = "<version>"`. A device (`device:<id>@<major>`) has neither.
    pub engine: Option<String>,
    pub engine_version: Option<String>,
}

/// A vendor file fetched at use, never redistributed (`bb-lock/1`
/// `[[vendor_file]]`, ADR-011 D5, D7): one per `[behavioral] svd` of a
/// resolved package, fetched and checked by `bb open`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VendorFile {
    pub url: String,
    pub sha256: String,
    pub license: String,
    pub used_by: String,
}

/// The lock major this build writes (ADR-011 D5).
pub const LOCK_MAJOR: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lock {
    /// The major the file was read at: the current one, or 0 (the previous
    /// major, read with a migration hint). [`Lock::to_text`] always writes
    /// [`LOCK_MAJOR`].
    pub major: u32,
    pub packages: Vec<LockedPackage>,
    pub vendor_files: Vec<VendorFile>,
    pub platforms: Vec<LockedPlatform>,
}

impl Default for Lock {
    fn default() -> Lock {
        Lock {
            major: LOCK_MAJOR,
            packages: Vec::new(),
            vendor_files: Vec::new(),
            platforms: Vec::new(),
        }
    }
}

fn is_hex64(h: &str) -> bool {
    h.len() == 64 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn is_hash(s: &str) -> bool {
    s.strip_prefix("sha256:").is_some_and(is_hex64)
}

impl Lock {
    /// Parse `bb-lock/1`, or `bb-lock/0` (the previous major), migrated in
    /// memory (ADR-011 D5): `path` becomes `source = "path+<path>"`, and
    /// `renode = "<v>"` becomes `engine = "renode"`, `engine_version =
    /// "<v>"`. Every hash is kept as written.
    pub fn parse(text: &str, reg: &Registry) -> Result<Lock, String> {
        let t = toml::parse(text).map_err(|e| format!("bb.lock: {e}"))?;
        let mut o = Obj::new("", &t);
        let schema = o.str_req("schema")?;
        reg.check("bb-lock", &schema)?;
        let major = crate::schema::SchemaId::parse(&schema)?.major;
        let mut lock = Lock {
            major,
            ..Lock::default()
        };
        for mut p in o.tables("package")? {
            let at = p.path.clone();
            let id = p.str_req("id")?;
            let version = p.str_req("version")?;
            let source = if major == 0 {
                format!("{PATH_SOURCE}{}", p.str_req("path")?)
            } else {
                let s = p.str_req("source")?;
                check_source(&at, &s)?;
                s
            };
            let tree = p.str_req("tree")?;
            p.done()?;
            if !is_hash(&tree) {
                return Err(format!(
                    "{at}.tree {tree:?}: expected sha256:<64 hex digits>"
                ));
            }
            lock.packages.push(LockedPackage {
                id,
                version,
                source,
                tree,
            });
        }
        if major >= 1 {
            for mut v in o.tables("vendor_file")? {
                let at = v.path.clone();
                let f = VendorFile {
                    url: v.str_req("url")?,
                    sha256: v.str_req("sha256")?,
                    license: v.str_req("license")?,
                    used_by: v.str_req("used_by")?,
                };
                v.done()?;
                if !f.url.starts_with("https://") {
                    return Err(format!("{at}.url {:?}: expected an https:// url", f.url));
                }
                if !is_hex64(&f.sha256) {
                    return Err(format!("{at}.sha256: expected 64 lower-case hex digits"));
                }
                lock.vendor_files.push(f);
            }
        }
        for mut p in o.tables("platform")? {
            let at = p.path.clone();
            let id = p.str_req("id")?;
            let content = p.str_req("content")?;
            let (engine, engine_version) = if major == 0 {
                match p.str_opt("renode")? {
                    Some(v) => (Some("renode".to_string()), Some(v)),
                    None => (None, None),
                }
            } else {
                (p.str_opt("engine")?, p.str_opt("engine_version")?)
            };
            p.done()?;
            if engine.is_some() != engine_version.is_some() {
                return Err(format!(
                    "{at}: engine and engine_version come together (ADR-011 D5)"
                ));
            }
            if !is_hash(&content) {
                return Err(format!(
                    "{at}.content {content:?}: expected sha256:<64 hex digits>"
                ));
            }
            lock.platforms.push(LockedPlatform {
                id,
                content,
                engine,
                engine_version,
            });
        }
        o.done()?;
        for w in lock.packages.windows(2) {
            if w[0].id >= w[1].id {
                return Err(format!(
                    "bb.lock: packages not sorted by id, or {} twice",
                    w[1].id
                ));
            }
        }
        for w in lock.vendor_files.windows(2) {
            if (&w[0].used_by, &w[0].url) >= (&w[1].used_by, &w[1].url) {
                return Err(format!(
                    "bb.lock: vendor files not sorted by (used_by, url), or {} twice",
                    w[1].url
                ));
            }
        }
        for w in lock.platforms.windows(2) {
            if w[0].id >= w[1].id {
                return Err(format!(
                    "bb.lock: platforms not sorted by id, or {} twice",
                    w[1].id
                ));
            }
        }
        Ok(lock)
    }

    pub fn read(path: &Path, reg: &Registry) -> Result<Option<Lock>, String> {
        match std::fs::read_to_string(path) {
            Ok(t) => Lock::parse(&t, reg)
                .map(Some)
                .map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("read {}: {e}", path.display())),
        }
    }

    /// The migration hint of a lock read at the previous major.
    pub fn hint(&self) -> Option<String> {
        (self.major < LOCK_MAJOR).then(|| {
            format!(
                "bb.lock is bb-lock/{}, the previous major: bb reads it; `bb open` (or `bb pkg \
                 migrate --lock`) rewrites it as bb-lock/{LOCK_MAJOR} (ADR-011 D5)",
                self.major
            )
        })
    }

    /// Deterministic text at the current major: entries sorted by id
    /// (vendor files by `(used_by, url)`).
    pub fn to_text(&self) -> String {
        let mut s = format!(
            "# bb.lock: written by `bb open` (ADR-006 D8, ADR-011 D5). Do not edit; `bb open --update-lock`\n\
             # rewrites the entries whose package tree or platform content changed.\n\
             schema = \"bb-lock/{LOCK_MAJOR}\"\n"
        );
        let mut pk = self.packages.clone();
        pk.sort_by(|a, b| a.id.cmp(&b.id));
        for p in &pk {
            s.push_str(&format!(
                "\n[[package]]\nid = {}\nversion = {}\nsource = {}\ntree = {}\n",
                quote(&p.id),
                quote(&p.version),
                quote(&p.source),
                quote(&p.tree)
            ));
        }
        let mut vf = self.vendor_files.clone();
        vf.sort_by(|a, b| (&a.used_by, &a.url).cmp(&(&b.used_by, &b.url)));
        for v in &vf {
            s.push_str(&format!(
                "\n[[vendor_file]]\nurl = {}\nsha256 = {}\nlicense = {}\nused_by = {}\n",
                quote(&v.url),
                quote(&v.sha256),
                quote(&v.license),
                quote(&v.used_by)
            ));
        }
        let mut pl = self.platforms.clone();
        pl.sort_by(|a, b| a.id.cmp(&b.id));
        for p in &pl {
            s.push_str(&format!(
                "\n[[platform]]\nid = {}\ncontent = {}\n",
                quote(&p.id),
                quote(&p.content)
            ));
            if let (Some(e), Some(v)) = (&p.engine, &p.engine_version) {
                s.push_str(&format!(
                    "engine = {}\nengine_version = {}\n",
                    quote(e),
                    quote(v)
                ));
            }
        }
        s
    }
}

/// `bb-lock/0` -> `bb-lock/1` as text (ADR-011 D5): `path` becomes `source =
/// "path+<path>"`, `renode` becomes `engine` and `engine_version`; every
/// hash is kept. `bb pkg migrate --lock` and `bb open` also rewrite a
/// platform entry that holds the legacy all-sources hash to the scoped one
/// (ADR-015 Amendment 3), which needs `bb`'s platform table.
pub fn migrate_text_0_to_1(text: &str) -> Result<String, String> {
    let l = Lock::parse(text, &Registry::current())?;
    if l.major != 0 {
        return Err("bb.lock: not a bb-lock/0 file".into());
    }
    Ok(l.to_text())
}

/// One difference between the lock and what is resolved now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Drift {
    /// Not in the lock yet (added by `bb open` without `--update-lock`).
    NewPackage(LockedPackage),
    NewPlatform(LockedPlatform),
    /// In the lock with another version or tree: refused unless
    /// `--update-lock`.
    Package {
        locked: LockedPackage,
        now: LockedPackage,
    },
    Platform {
        locked: LockedPlatform,
        now: LockedPlatform,
    },
    /// The same version and tree from another kind of source (`path+` and
    /// `registry+`, or another registry): refused unless `--update-lock`
    /// (ADR-011 D5). A `path+` source with another path is only a warning
    /// ([`path_warnings`]).
    Source {
        locked: LockedPackage,
        now: LockedPackage,
    },
    /// A vendor file a package declares that the lock does not record yet.
    NewVendorFile(VendorFile),
    /// A vendor file whose declaration (sha256, licence) differs from the
    /// lock's: refused unless `--update-lock`.
    VendorFile {
        locked: VendorFile,
        now: VendorFile,
    },
}

/// What a source is, for the source-change rule: `path` for every `path+`
/// source, else the registry source itself (its URL with one trailing `/`).
pub fn source_kind(s: &str) -> String {
    match s.strip_prefix(REGISTRY_SOURCE) {
        Some(u) => format!("{REGISTRY_SOURCE}{}/", u.trim_end_matches('/')),
        None => "path".into(),
    }
}

fn engine_note(p: &LockedPlatform) -> String {
    match (&p.engine, &p.engine_version) {
        (Some(e), Some(v)) => format!(" ({e} {v})"),
        _ => String::new(),
    }
}

impl Drift {
    pub fn is_mismatch(&self) -> bool {
        matches!(
            self,
            Drift::Package { .. }
                | Drift::Platform { .. }
                | Drift::Source { .. }
                | Drift::VendorFile { .. }
        )
    }

    pub fn describe(&self) -> String {
        match self {
            Drift::NewPackage(p) => format!("package {} {} not in bb.lock", p.id, p.version),
            Drift::NewPlatform(p) => format!("platform {} not in bb.lock", p.id),
            Drift::Package { locked, now } => format!(
                "package {}: locked {} {}, found {} {}",
                now.id, locked.version, locked.tree, now.version, now.tree
            ),
            Drift::Platform { locked, now } => format!(
                "platform {}: locked content {}{}, this bb has {}{}",
                now.id,
                locked.content,
                engine_note(locked),
                now.content,
                engine_note(now)
            ),
            Drift::Source { locked, now } => format!(
                "package {} {}: locked from {}, resolved now from {} (the same tree; a change of \
                 source needs --update-lock, ADR-011 D5)",
                now.id,
                now.version,
                source_kind(&locked.source),
                source_kind(&now.source)
            ),
            Drift::NewVendorFile(v) => {
                format!("vendor file {} of {} not in bb.lock", v.url, v.used_by)
            }
            Drift::VendorFile { locked, now } => format!(
                "vendor file {} of {}: locked sha256 {} ({}), declared {} ({})",
                now.url, now.used_by, locked.sha256, locked.license, now.sha256, now.license
            ),
        }
    }
}

/// The vendor files `vendor` (what the resolved packages declare) against
/// the lock's (ADR-011 D5). An entry is matched by `(used_by, url)`.
pub fn vendor_drift(lock: Option<&Lock>, vendor: &[VendorFile]) -> Vec<Drift> {
    let empty = Lock::default();
    let lock = lock.unwrap_or(&empty);
    let mut out = Vec::new();
    for v in vendor {
        match lock
            .vendor_files
            .iter()
            .find(|l| l.used_by == v.used_by && l.url == v.url)
        {
            None => out.push(Drift::NewVendorFile(v.clone())),
            Some(l) if l != v => out.push(Drift::VendorFile {
                locked: l.clone(),
                now: v.clone(),
            }),
            Some(_) => {}
        }
    }
    out
}

/// Compare what is resolved now with the lock (`None`: no lock yet). A
/// package's `path+` source is informative ([`path_warnings`]); a change of
/// the kind of source, or of registry, is [`Drift::Source`] (ADR-011 D5).
pub fn drift(
    lock: Option<&Lock>,
    packages: &[LockedPackage],
    platforms: &[LockedPlatform],
) -> Vec<Drift> {
    let empty = Lock::default();
    let lock = lock.unwrap_or(&empty);
    let mut out = Vec::new();
    for p in packages {
        match lock.packages.iter().find(|l| l.id == p.id) {
            None => out.push(Drift::NewPackage(p.clone())),
            Some(l) if l.version != p.version || l.tree != p.tree => out.push(Drift::Package {
                locked: l.clone(),
                now: p.clone(),
            }),
            Some(l) if source_kind(&l.source) != source_kind(&p.source) => {
                out.push(Drift::Source {
                    locked: l.clone(),
                    now: p.clone(),
                })
            }
            Some(_) => {}
        }
    }
    for p in platforms {
        match lock.platforms.iter().find(|l| l.id == p.id) {
            None => out.push(Drift::NewPlatform(p.clone())),
            Some(l)
                if l.content != p.content
                    || l.engine != p.engine
                    || l.engine_version != p.engine_version =>
            {
                out.push(Drift::Platform {
                    locked: l.clone(),
                    now: p.clone(),
                })
            }
            Some(_) => {}
        }
    }
    out
}

/// The lock with `drift` applied: new entries added, changed entries
/// replaced; every other entry kept as it was. The result is at the current
/// major.
pub fn apply(lock: Option<&Lock>, drift: &[Drift]) -> Lock {
    let mut l = lock.cloned().unwrap_or_default();
    l.major = LOCK_MAJOR;
    for d in drift {
        match d {
            Drift::NewPackage(p) => l.packages.push(p.clone()),
            Drift::Package { now, .. } | Drift::Source { now, .. } => {
                if let Some(e) = l.packages.iter_mut().find(|e| e.id == now.id) {
                    *e = now.clone();
                }
            }
            Drift::NewVendorFile(v) => l.vendor_files.push(v.clone()),
            Drift::VendorFile { now, .. } => {
                if let Some(e) = l
                    .vendor_files
                    .iter_mut()
                    .find(|e| e.used_by == now.used_by && e.url == now.url)
                {
                    *e = now.clone();
                }
            }
            Drift::NewPlatform(p) => l.platforms.push(p.clone()),
            Drift::Platform { now, .. } => {
                if let Some(e) = l.platforms.iter_mut().find(|e| e.id == now.id) {
                    *e = now.clone();
                }
            }
        }
    }
    l.packages.sort_by(|a, b| a.id.cmp(&b.id));
    l.vendor_files
        .sort_by(|a, b| (&a.used_by, &a.url).cmp(&(&b.used_by, &b.url)));
    l.platforms.sort_by(|a, b| a.id.cmp(&b.id));
    l
}

/// ADR-015 D7: the locked packages whose recorded `path+` source differs
/// from where they resolved now, one warning each. `base` is the directory
/// both paths are relative to (the lock's). The tree hash still decides
/// acceptance; the warning says what was not used: a package with the same
/// id at the recorded path (a copy beside a copied board, say) is named with
/// its tree hash when it differs from the one used.
pub fn path_warnings(lock: Option<&Lock>, packages: &[LockedPackage], base: &Path) -> Vec<String> {
    let Some(lock) = lock else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for p in packages {
        let Some(l) = lock.packages.iter().find(|l| l.id == p.id) else {
            continue;
        };
        let (Some(lpath), Some(ppath)) = (l.path(), p.path()) else {
            continue;
        };
        if lpath == ppath {
            continue;
        }
        let recorded = base.join(lpath);
        let canon = |q: &Path| std::fs::canonicalize(q).ok();
        let used = canon(&base.join(ppath));
        if used.is_some() && canon(&recorded) == used {
            continue; // the same directory, spelled differently
        }
        let mut msg = format!(
            "package {}: bb.lock records path {lpath}, but it resolved to {ppath} (the tree hash \
             decides acceptance, ADR-015 D7)",
            p.id
        );
        let there = recorded
            .join("part.toml")
            .is_file()
            .then(|| crate::package::load(&recorded, &Registry::current()).ok())
            .flatten()
            .filter(|q| q.part.id == p.id);
        match there {
            Some(_) => match tree_hash(&recorded) {
                Ok(t) if t != p.tree => msg.push_str(&format!(
                    "; a different {} (tree {t}) is at the recorded path and is NOT used: name \
                     its directory with `*bb packages path=` (a board file) or `[packages] \
                     paths` (bb.toml)",
                    p.id
                )),
                Ok(_) => msg.push_str("; an identical copy at the recorded path is not used"),
                Err(e) => msg.push_str(&format!("; the recorded path is unreadable: {e}")),
            },
            None => msg.push_str(&format!("; no package {} is at the recorded path", p.id)),
        }
        out.push(msg);
    }
    out
}

/// `to` relative to `from` (both absolute and canonical), with `/`.
pub fn relative_path(from: &Path, to: &Path) -> String {
    let f: Vec<_> = from.components().collect();
    let t: Vec<_> = to.components().collect();
    let common = f.iter().zip(&t).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = Vec::new();
    for _ in common..f.len() {
        parts.push("..".into());
    }
    for c in &t[common..] {
        parts.push(c.as_os_str().to_string_lossy().into_owned());
    }
    if parts.is_empty() {
        ".".into()
    } else {
        parts.join("/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(c: char) -> String {
        format!("sha256:{}", c.to_string().repeat(64))
    }

    fn pkg(id: &str, tree: &str) -> LockedPackage {
        LockedPackage::at_path(id, "0.1.0", &format!("../packages/{id}"), tree)
    }

    fn renode(id: &str, content: &str) -> LockedPlatform {
        LockedPlatform {
            id: id.into(),
            content: content.into(),
            engine: Some("renode".into()),
            engine_version: Some("1.17.0".into()),
        }
    }

    #[test]
    fn lock_round_trip_and_sorting() {
        let l = Lock {
            packages: vec![pkg("st/b", &h('2')), pkg("generic/a", &h('1'))],
            platforms: vec![renode("renode:x@1", &h('3'))],
            ..Lock::default()
        };
        let text = l.to_text();
        assert!(text.contains("schema = \"bb-lock/1\""), "{text}");
        assert!(
            text.contains("source = \"path+../packages/generic/a\""),
            "{text}"
        );
        assert!(
            text.contains("engine = \"renode\"\nengine_version = \"1.17.0\"\n"),
            "{text}"
        );
        let back = Lock::parse(&text, &Registry::current()).unwrap();
        assert_eq!(back.major, 1);
        assert_eq!(back.packages[0].id, "generic/a");
        assert_eq!(back.packages[0].path(), Some("../packages/generic/a"));
        assert_eq!(back.to_text(), text);
        assert!(back.hint().is_none());
        // A newer major is refused; an unsorted file too.
        assert!(Lock::parse(
            &text.replace("bb-lock/1", "bb-lock/2"),
            &Registry::current()
        )
        .unwrap_err()
        .contains("upgrade bb"));
        // bb-lock/1 has no path or renode key, and engine needs its version.
        for (from, to) in [
            ("source = \"path+../packages/generic/a\"", "path = \"x\""),
            ("engine = \"renode\"\n", ""),
            (
                "engine_version = \"1.17.0\"\n",
                "engine_version = \"1.17.0\"\nrenode = \"1.17.0\"\n",
            ),
            (
                "source = \"path+../packages/generic/a\"",
                "source = \"../packages/generic/a\"",
            ),
            (
                "source = \"path+../packages/generic/a\"",
                "source = \"registry+http://x/\"",
            ),
        ] {
            assert!(
                Lock::parse(&text.replacen(from, to, 1), &Registry::current()).is_err(),
                "{from} -> {to}"
            );
        }
    }

    #[test]
    fn bb_lock_0_is_read_and_migrated() {
        // ADR-011 D5: path -> source = "path+<path>", renode -> engine and
        // engine_version; every hash kept.
        let v0 = format!(
            "# old\nschema = \"bb-lock/0\"\n\n[[package]]\nid = \"st/x\"\nversion = \"0.1.0\"\n\
             path = \"../../packages/st/x\"\ntree = \"{}\"\n\n[[platform]]\nid = \"device:d@1\"\n\
             content = \"{}\"\n\n[[platform]]\nid = \"renode:x@1\"\ncontent = \"{}\"\nrenode = \"1.17.0\"\n",
            h('a'),
            h('b'),
            h('c')
        );
        let l = Lock::parse(&v0, &Registry::current()).unwrap();
        assert_eq!(l.major, 0);
        assert!(l.hint().unwrap().contains("bb-lock/1"));
        assert_eq!(l.packages[0].source, "path+../../packages/st/x");
        assert_eq!(l.platforms[0].engine, None);
        assert_eq!(l.platforms[1], renode("renode:x@1", &h('c')));
        let v1 = migrate_text_0_to_1(&v0).unwrap();
        let back = Lock::parse(&v1, &Registry::current()).unwrap();
        assert_eq!(back.major, 1);
        assert_eq!(
            (back.packages.clone(), back.platforms.clone()),
            (l.packages.clone(), l.platforms.clone())
        );
        assert!(migrate_text_0_to_1(&v1).is_err());
        // A bb-lock/0 key is not a bb-lock/1 key and the other way round.
        assert!(Lock::parse(&v0.replace("renode =", "engine ="), &Registry::current()).is_err());
        // apply() writes the current major.
        assert_eq!(apply(Some(&l), &[]).major, 1);
    }

    #[test]
    fn vendor_files_and_registry_sources_are_kept() {
        // ADR-011 D5, D7: [[vendor_file]] and registry+ sources are read and
        // written back (inert until milestone E1).
        let mut l = Lock {
            packages: vec![LockedPackage {
                id: "st/x".into(),
                version: "0.2.0".into(),
                source: "registry+https://example.invalid/".into(),
                tree: h('d'),
            }],
            vendor_files: vec![
                VendorFile {
                    url: "https://example.invalid/b.svd".into(),
                    sha256: "e".repeat(64),
                    license: "LicenseRef-ST-EULA".into(),
                    used_by: "st/x".into(),
                },
                VendorFile {
                    url: "https://example.invalid/a.svd".into(),
                    sha256: "f".repeat(64),
                    license: "LicenseRef-ST-EULA".into(),
                    used_by: "st/x".into(),
                },
            ],
            ..Lock::default()
        };
        let text = l.to_text();
        let back = Lock::parse(&text, &Registry::current()).unwrap();
        assert_eq!(back.packages[0].path(), None);
        l.vendor_files.reverse();
        assert_eq!(back.vendor_files, l.vendor_files);
        assert_eq!(back.to_text(), text);
        assert!(Lock::parse(
            &text.replace("https://example.invalid/a", "http://x/a"),
            &Registry::current()
        )
        .is_err());
        assert!(Lock::parse(
            &text.replacen(&"f".repeat(64), "F", 1),
            &Registry::current()
        )
        .is_err());
    }

    #[test]
    fn drift_and_update_only_the_changed_entry() {
        // G5: a mismatch is reported (refused by the caller), and applying it
        // rewrites only that entry.
        let lock = Lock {
            packages: vec![pkg("a/x", &h('1')), pkg("b/y", &h('2'))],
            ..Lock::default()
        };
        let now = [
            pkg("a/x", &h('1')),
            pkg("b/y", &h('9')),
            pkg("c/z", &h('3')),
        ];
        let d = drift(Some(&lock), &now, &[]);
        assert_eq!(d.len(), 2);
        assert!(d.iter().any(Drift::is_mismatch));
        let updated = apply(Some(&lock), &d);
        assert_eq!(updated.packages[0], lock.packages[0]);
        assert_eq!(updated.packages[1].tree, h('9'));
        assert_eq!(updated.packages[2].id, "c/z");
        assert!(drift(Some(&updated), &now, &[]).is_empty());
        // A platform's engine version is part of what is pinned.
        let pl = Lock {
            platforms: vec![renode("renode:x@1", &h('3'))],
            ..Lock::default()
        };
        let mut other = renode("renode:x@1", &h('3'));
        other.engine_version = Some("1.16.0".into());
        assert!(drift(Some(&pl), &[], &[other])[0].is_mismatch());
    }

    #[test]
    fn a_change_of_source_and_a_changed_vendor_file_are_mismatches() {
        // ADR-011 D5: path+ -> registry+ (same tree) needs --update-lock; a
        // path+ source with another path is not drift (only a warning).
        let lock = Lock {
            packages: vec![pkg("a/x", &h('1'))],
            ..Lock::default()
        };
        let mut moved = pkg("a/x", &h('1'));
        moved.source = "path+elsewhere/a/x".into();
        assert!(drift(Some(&lock), &[moved], &[]).is_empty());
        let mut reg = pkg("a/x", &h('1'));
        reg.source = "registry+https://r.example/".into();
        let d = drift(Some(&lock), std::slice::from_ref(&reg), &[]);
        assert!(
            matches!(d[0], Drift::Source { .. }) && d[0].is_mismatch(),
            "{d:?}"
        );
        assert!(d[0].describe().contains("--update-lock"));
        let updated = apply(Some(&lock), &d);
        assert_eq!(updated.packages[0].source, reg.source);
        // Another registry, or the same one spelled with a trailing slash.
        let mut reg2 = reg.clone();
        reg2.source = "registry+https://r.example".into();
        assert!(drift(Some(&updated), &[reg2], &[]).is_empty());
        let mut reg3 = reg.clone();
        reg3.source = "registry+https://other.example/".into();
        assert!(drift(Some(&updated), &[reg3], &[])[0].is_mismatch());
        // Vendor files: a new one is added, a changed declaration refused.
        let v = VendorFile {
            url: "https://v.example/a.svd".into(),
            sha256: "a".repeat(64),
            license: "LicenseRef-ST-EULA".into(),
            used_by: "a/x".into(),
        };
        let d = vendor_drift(Some(&lock), std::slice::from_ref(&v));
        assert!(matches!(d[0], Drift::NewVendorFile(_)) && !d[0].is_mismatch());
        let with = apply(Some(&lock), &d);
        assert_eq!(with.vendor_files, vec![v.clone()]);
        assert!(vendor_drift(Some(&with), std::slice::from_ref(&v)).is_empty());
        let mut changed = v.clone();
        changed.sha256 = "b".repeat(64);
        assert!(vendor_drift(Some(&with), &[changed])[0].is_mismatch());
    }

    #[test]
    fn tree_hash_excludes_dotfiles_and_test_outputs() {
        let base = std::env::temp_dir().join(format!("bbpkg-tree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("tests/out")).unwrap();
        std::fs::create_dir_all(base.join("analog")).unwrap();
        std::fs::write(base.join("part.toml"), "a").unwrap();
        std::fs::write(base.join("analog/r.lib"), "b").unwrap();
        let h1 = tree_hash(&base).unwrap();
        std::fs::write(base.join(".swp"), "x").unwrap();
        std::fs::write(base.join("tests/out/run.txt"), "x").unwrap();
        assert_eq!(tree_hash(&base).unwrap(), h1);
        std::fs::write(base.join("analog/r.lib"), "b\r").unwrap();
        assert_ne!(tree_hash(&base).unwrap(), h1);
        // The documented form: sorted "<path>\0<sha256>\n" lines.
        std::fs::write(base.join("analog/r.lib"), "b").unwrap();
        let mut list = vec![
            ("part.toml".to_string(), sha256::hex_digest(b"a")),
            ("analog/r.lib".to_string(), sha256::hex_digest(b"b")),
        ];
        assert_eq!(list_hash(&mut list), h1);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn relative_paths() {
        assert_eq!(
            relative_path(
                Path::new("/r/examples/spike"),
                Path::new("/r/packages/st/x")
            ),
            "../../packages/st/x"
        );
        assert_eq!(relative_path(Path::new("/r"), Path::new("/r")), ".");
    }
}
