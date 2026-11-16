//! Breadboard component packages v0 (ADR-006, `spec/package.md`).
//!
//! - [`value`]: the value grammar and the integer units;
//! - [`toml`]: the TOML reader for the authored files;
//! - [`package`]: `part.toml` and `pins.toml`;
//! - [`lint`]: `bb pkg lint`;
//! - [`bexpr`]: B-expression lint against ngspice 42's function table;
//! - [`spice`]: view files, the `gnd` rule, deck assembly;
//! - [`pkgtest`]: test vectors through `NgspiceEngine` (`bb pkg test`);
//! - [`lock`]: tree hashes, platform content hashes and `bb.lock`;
//! - [`schema`]: schema ids, the versioning policy, migrations;
//! - [`optsets`]: named solver option sets;
//! - [`archive`]: deterministic package archives (ADR-011 D6);
//! - [`registry`]: registries, the signed snapshot, the cache and resolution
//!   (ADR-011);
//! - [`semver`]: versions and caret requirements (ADR-011 D4);
//! - [`pattern`]: `mpn_patterns`, anchored regular expressions (ADR-007 D4);
//! - [`sha256`].

pub mod archive;
pub mod bexpr;
pub mod lint;
pub mod lock;
pub mod obj;
pub mod optsets;
pub mod package;
pub mod pattern;
pub mod pirules;
pub mod pkgtest;
pub mod registry;
pub mod schema;
pub mod semver;
pub mod sha256;
pub mod spice;
pub mod toml;
pub mod value;

use std::path::{Path, PathBuf};

/// Where packages are found (ADR-006 D1): the project's `[packages] paths`
/// in order, then breadboard.dev's first-party `packages/`. The first
/// directory that provides an id wins; two packages with one id in one
/// directory are an error.
#[derive(Clone, Debug)]
pub struct SearchPath {
    pub dirs: Vec<PathBuf>,
}

impl SearchPath {
    /// Find package `id` (optionally `id@version`, which must then match),
    /// loading every package of each directory so that an id provided
    /// twice in one directory is an error (`bb open`, lint).
    pub fn find(&self, spec: &str, reg: &schema::Registry) -> Result<package::Package, String> {
        self.find_impl(spec, reg, false)
    }

    /// As [`SearchPath::find`], but a package at the conventional path
    /// `<dir>/<namespace>/<name>` is taken without loading the rest of the
    /// directory (`bb run`, whose result the lock pins anyway). Other
    /// layouts fall back to the full search.
    pub fn find_fast(
        &self,
        spec: &str,
        reg: &schema::Registry,
    ) -> Result<package::Package, String> {
        self.find_impl(spec, reg, true)
    }

    /// As [`SearchPath::find`] (`fast`: [`SearchPath::find_fast`]), but an id
    /// no directory provides is `Ok(None)`, so that a registry can be asked
    /// next (ADR-011 D4: local sources first). An id provided locally with
    /// another version than `id@version` asks for is still an error: a local
    /// package shadows the registry.
    pub fn try_find(
        &self,
        spec: &str,
        reg: &schema::Registry,
        fast: bool,
    ) -> Result<Option<package::Package>, String> {
        let id = spec.split_once('@').map(|(i, _)| i).unwrap_or(spec);
        match self.find_impl(spec, reg, fast) {
            Ok(p) => Ok(Some(p)),
            Err(e) if e.starts_with(&format!("package {id} not found (searched ")) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn find_impl(
        &self,
        spec: &str,
        reg: &schema::Registry,
        fast: bool,
    ) -> Result<package::Package, String> {
        let (id, want) = match spec.split_once('@') {
            Some((i, v)) => (i, Some(v)),
            None => (spec, None),
        };
        let check_version = |pkg: package::Package| -> Result<package::Package, String> {
            if let Some(v) = want {
                if pkg.part.version != v {
                    return Err(format!(
                        "package {id}@{v}: {} has version {}",
                        pkg.dir.display(),
                        pkg.part.version
                    ));
                }
            }
            Ok(pkg)
        };
        for d in &self.dirs {
            if !d.is_dir() {
                continue;
            }
            if fast && !id.contains("..") && d.join(id).join("part.toml").is_file() {
                let pkg = package::load(&d.join(id), reg)?;
                if pkg.part.id == id {
                    return check_version(pkg);
                }
            }
            let mut found: Vec<package::Package> = Vec::new();
            for p in package::discover(d)? {
                let pkg = package::load(&p, reg)?;
                if pkg.part.id == id {
                    found.push(pkg);
                }
            }
            match found.len() {
                0 => continue,
                1 => return check_version(found.pop().expect("one")),
                _ => {
                    return Err(format!(
                        "package {id} is provided twice in {}: {}",
                        d.display(),
                        found
                            .iter()
                            .map(|p| p.dir.display().to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                }
            }
        }
        Err(format!(
            "package {id} not found (searched {})",
            self.dirs
                .iter()
                .map(|d| d.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }
}

/// Breadboard's first-party package directory: `$BB_FIRST_PARTY_PACKAGES`,
/// else the `packages/` of the checkout this bb was built from.
pub fn first_party_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("BB_FIRST_PARTY_PACKAGES") {
        return PathBuf::from(d);
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../packages")
}
