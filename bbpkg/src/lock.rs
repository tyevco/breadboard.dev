//! Package tree hashes and the lockfile `bb.lock` (ADR-006 D8).
//!
//! **Tree hash:** sha256 over the sorted list of `"<relative path>\0<sha256
//! hex of content>\n"` for every regular file in the package directory,
//! excluding dotfiles (any path component starting with `.`) and
//! `tests/out/`. Paths use `/`; bytes are hashed as stored (`.gitattributes`
//! forces LF, so a CRLF checkout changes the hash). Written `sha256:<hex>`.
//!
//! **Platform content hash:** the same list form over the files that make
//! up an engine platform (the `.repl` it loads and every plugin source),
//! named by file name.
//!
//! **`bb.lock`:** TOML, `schema = "bb-lock/0"`, `[[package]]` (id, version,
//! path, tree) and `[[platform]]` (id, content, renode) entries sorted by id.
//! `bb open` writes it; `bb open` and `bb run` refuse a package or platform
//! whose hash differs from it unless `bb open --update-lock`, which rewrites
//! only the entries that changed.

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockedPackage {
    pub id: String,
    pub version: String,
    /// As found on the search path, relative to the project (informative:
    /// identity is the tree hash).
    pub path: String,
    pub tree: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockedPlatform {
    /// `<engine>:<name>@<major>`, e.g. `renode:stm32f103-offline@1`.
    pub id: String,
    pub content: String,
    /// The engine's own version where it has one (Renode).
    pub renode: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Lock {
    pub packages: Vec<LockedPackage>,
    pub platforms: Vec<LockedPlatform>,
}

impl Lock {
    pub fn parse(text: &str, reg: &Registry) -> Result<Lock, String> {
        let t = toml::parse(text).map_err(|e| format!("bb.lock: {e}"))?;
        let mut o = Obj::new("", &t);
        let schema = o.str_req("schema")?;
        reg.check("bb-lock", &schema)?;
        let mut lock = Lock::default();
        for mut p in o.tables("package")? {
            lock.packages.push(LockedPackage {
                id: p.str_req("id")?,
                version: p.str_req("version")?,
                path: p.str_req("path")?,
                tree: p.str_req("tree")?,
            });
            p.done()?;
        }
        for mut p in o.tables("platform")? {
            lock.platforms.push(LockedPlatform {
                id: p.str_req("id")?,
                content: p.str_req("content")?,
                renode: p.str_opt("renode")?,
            });
            p.done()?;
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

    /// Deterministic text: entries sorted by id.
    pub fn to_text(&self) -> String {
        let mut s = String::from(
            "# bb.lock: written by `bb open` (ADR-006 D8). Do not edit; `bb open --update-lock`\n\
             # rewrites the entries whose package tree or platform content changed.\n\
             schema = \"bb-lock/0\"\n",
        );
        let mut pk = self.packages.clone();
        pk.sort_by(|a, b| a.id.cmp(&b.id));
        for p in &pk {
            s.push_str(&format!(
                "\n[[package]]\nid = {}\nversion = {}\npath = {}\ntree = {}\n",
                quote(&p.id),
                quote(&p.version),
                quote(&p.path),
                quote(&p.tree)
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
            if let Some(r) = &p.renode {
                s.push_str(&format!("renode = {}\n", quote(r)));
            }
        }
        s
    }
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
}

impl Drift {
    pub fn is_mismatch(&self) -> bool {
        matches!(self, Drift::Package { .. } | Drift::Platform { .. })
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
                locked
                    .renode
                    .as_ref()
                    .map(|r| format!(" (Renode {r})"))
                    .unwrap_or_default(),
                now.content,
                now.renode
                    .as_ref()
                    .map(|r| format!(" (Renode {r})"))
                    .unwrap_or_default()
            ),
        }
    }
}

/// Compare what is resolved now with the lock (`None`: no lock yet).
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
            Some(_) => {}
        }
    }
    for p in platforms {
        match lock.platforms.iter().find(|l| l.id == p.id) {
            None => out.push(Drift::NewPlatform(p.clone())),
            Some(l) if l.content != p.content || l.renode != p.renode => {
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
/// replaced; every other entry kept as it was.
pub fn apply(lock: Option<&Lock>, drift: &[Drift]) -> Lock {
    let mut l = lock.cloned().unwrap_or_default();
    for d in drift {
        match d {
            Drift::NewPackage(p) => l.packages.push(p.clone()),
            Drift::Package { now, .. } => {
                if let Some(e) = l.packages.iter_mut().find(|e| e.id == now.id) {
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
    l.platforms.sort_by(|a, b| a.id.cmp(&b.id));
    l
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

    fn pkg(id: &str, tree: &str) -> LockedPackage {
        LockedPackage {
            id: id.into(),
            version: "0.1.0".into(),
            path: format!("../packages/{id}"),
            tree: tree.into(),
        }
    }

    #[test]
    fn lock_round_trip_and_sorting() {
        let l = Lock {
            packages: vec![pkg("st/b", "sha256:2"), pkg("generic/a", "sha256:1")],
            platforms: vec![LockedPlatform {
                id: "renode:x@1".into(),
                content: "sha256:3".into(),
                renode: Some("1.17.0".into()),
            }],
        };
        let text = l.to_text();
        let back = Lock::parse(&text, &Registry::current()).unwrap();
        assert_eq!(back.packages[0].id, "generic/a");
        assert_eq!(back.to_text(), text);
        assert!(Lock::parse(
            &text.replace("bb-lock/0", "bb-lock/1"),
            &Registry::current()
        )
        .is_err());
    }

    #[test]
    fn drift_and_update_only_the_changed_entry() {
        // G5: a mismatch is reported (refused by the caller), and applying it
        // rewrites only that entry.
        let lock = Lock {
            packages: vec![pkg("a/x", "sha256:1"), pkg("b/y", "sha256:2")],
            platforms: vec![],
        };
        let now = [
            pkg("a/x", "sha256:1"),
            pkg("b/y", "sha256:CHANGED"),
            pkg("c/z", "sha256:3"),
        ];
        let d = drift(Some(&lock), &now, &[]);
        assert_eq!(d.len(), 2);
        assert!(d.iter().any(Drift::is_mismatch));
        let updated = apply(Some(&lock), &d);
        assert_eq!(updated.packages[0], lock.packages[0]);
        assert_eq!(updated.packages[1].tree, "sha256:CHANGED");
        assert_eq!(updated.packages[2].id, "c/z");
        assert!(drift(Some(&updated), &now, &[]).is_empty());
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
