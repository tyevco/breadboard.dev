//! Breadboard component packages v0 (ADR-006, `spec/package.md`).
//!
//! - [`value`]: the value grammar and the integer units;
//! - [`toml`]: the TOML reader for the authored files;
//! - [`package`]: `part.toml` and `pins.toml`;
//! - [`lint`]: `bb pkg lint`;
//! - [`spice`]: view files, the `gnd` rule, deck assembly;
//! - [`pkgtest`]: test vectors through `NgspiceEngine` (`bb pkg test`);
//! - [`lock`]: tree hashes, platform content hashes and `bb.lock`;
//! - [`schema`]: schema ids, the versioning policy, migrations;
//! - [`optsets`]: named solver option sets;
//! - [`sha256`].

pub mod lint;
pub mod lock;
pub mod obj;
pub mod optsets;
pub mod package;
pub mod pkgtest;
pub mod schema;
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
