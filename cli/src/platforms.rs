//! Engine platform identities (ADR-006 D8): every Renode platform `bb`
//! provides has an id with a major version and a content hash, which
//! packages name (`[behavioral] platform`) and `bb.lock` pins.
//!
//! The content hash covers what the platform loads (ADR-015 D6): the `.repl`
//! description and the plugin sources it declares, its own models and the
//! shared ones (`bb_cpu_renode::platform_plugins`), hashed as a sorted
//! `(file name, sha256)` list (`bbpkg::lock::content_hash`). So a change to
//! an nRF model changes no STM32 platform's hash. A behaviour change bumps
//! the major and the hash. Renode's own version is pinned (1.17.0) and
//! recorded next to it.
//!
//! **The residual** (ADR-015 D6, `spec/package.md` section 9): at run time
//! one assembly holds every plugin source (ADR-005 D6), so a run on platform
//! A loads code that A's lock does not cover. A compile failure of the whole
//! assembly fails every run; a model A never instantiates cannot act on A's
//! run, because .NET runs a type's static initialiser only on first use and
//! the assembly has no module initialiser (`make plugin-scope`).
//!
//! **The legacy hash** (ADR-015 Amendment 3): before H4 the hash covered
//! every plugin source. A `bb-lock/0` entry that holds exactly that hash
//! pins strictly more than the scoped one (every source and the `.repl`
//! unchanged), so `bb run` accepts it from a `bb-lock/0` file as it stands.
//! Migrating the lock to `bb-lock/1` (`bb open`, `bb pkg migrate --lock`;
//! [`migrate_lock`]) writes it as the scoped hash: milestone S relocked
//! every committed lock that way. In a `bb-lock/1` file only the scoped
//! hash matches.
//!
//! A new platform (the nRF52840 of ADR-008) adds a row here and to
//! `bb_cpu_renode::PLATFORM_PLUGINS`.

use bb_cpu_renode::{NRF52840_BB_REPL, PLUGIN_SOURCES, STM32F072_BB_REPL, STM32F103_OFFLINE_REPL};
use bbpkg::lock::{Lock, LockedPlatform};

/// The Renode release every platform is built and pinned for.
pub const RENODE_VERSION: &str = "1.17.0";

/// One platform.
pub struct PlatformDef {
    /// `<name>@<major>`, as packages name it.
    pub id: &'static str,
    /// The `platform=` name of the Phase 0 board-file grammar.
    pub board_name: &'static str,
    /// The `.repl` it loads: `(file name, text)`.
    pub repl: (&'static str, &'static str),
}

pub const PLATFORMS: &[PlatformDef] = &[
    PlatformDef {
        id: "stm32f103-offline@1",
        board_name: "stm32f103",
        repl: ("stm32f103-offline.repl", STM32F103_OFFLINE_REPL),
    },
    PlatformDef {
        id: "stm32f072-bb@1",
        board_name: "stm32f072",
        repl: ("stm32f072-bb.repl", STM32F072_BB_REPL),
    },
    // ADR-008 D1 (Phase 1 M2): no SVD download, 64 MIPS; @2 (Phase 1 M8,
    // D1, D7, D8): Breadboard's SAADC, its host port and the UARTE tap; @3
    // (Phase 1 M10, D9): PWM0-3, the TEMP and NVMC stand-ins, their port.
    PlatformDef {
        id: "nrf52840-bb@3",
        board_name: "nrf52840",
        repl: ("nrf52840-bb.repl", NRF52840_BB_REPL),
    },
];

pub fn find(id: &str) -> Option<&'static PlatformDef> {
    PLATFORMS.iter().find(|p| p.id == id)
}

pub fn ids() -> Vec<String> {
    PLATFORMS.iter().map(|p| p.id.to_string()).collect()
}

impl PlatformDef {
    /// The lock id: `renode:<id>`.
    pub fn lock_id(&self) -> String {
        format!("renode:{}", self.id)
    }

    /// The plugin sources this platform declares (ADR-015 D6).
    pub fn plugins(&self) -> Vec<&'static str> {
        bb_cpu_renode::platform_plugins(self.repl.0)
            .unwrap_or_else(|| panic!("{}: no row in PLATFORM_PLUGINS", self.repl.0))
    }

    /// The content hash (ADR-015 D6): the `.repl` and the declared plugin
    /// sources.
    pub fn content(&self) -> String {
        self.content_from(PLUGIN_SOURCES)
    }

    /// The content hash with the plugin sources taken from `sources`
    /// (`(file name, text)`): [`PlatformDef::content`] of a `bb` built from
    /// them.
    pub fn content_from(&self, sources: &[(&str, &str)]) -> String {
        let mut files: Vec<(&str, &[u8])> = self
            .plugins()
            .into_iter()
            .map(|n| {
                let s = sources
                    .iter()
                    .find(|(f, _)| *f == n)
                    .unwrap_or_else(|| panic!("plugin source {n} missing"));
                (n, s.1.as_bytes())
            })
            .collect();
        files.push((self.repl.0, self.repl.1.as_bytes()));
        bbpkg::lock::content_hash(&files)
    }

    /// The pre-H4 content hash: the `.repl` and every plugin source
    /// (ADR-015 Amendment 3). Only ever compared, never written.
    pub fn legacy_content(&self) -> String {
        let mut files: Vec<(&str, &[u8])> = PLUGIN_SOURCES
            .iter()
            .map(|(n, s)| (*n, s.as_bytes()))
            .collect();
        files.push((self.repl.0, self.repl.1.as_bytes()));
        bbpkg::lock::content_hash(&files)
    }

    /// The lock entry this `bb` writes for the platform (`bb-lock/1`: the
    /// scoped hash, and the engine-neutral `engine`/`engine_version`).
    pub fn locked(&self) -> LockedPlatform {
        LockedPlatform {
            id: self.lock_id(),
            content: self.content(),
            engine: Some("renode".to_string()),
            engine_version: Some(RENODE_VERSION.to_string()),
        }
    }

    /// Does `e` hold the legacy hash of these very files, for this Renode?
    fn is_legacy(&self, e: &LockedPlatform) -> bool {
        let now = self.locked();
        e.id == now.id
            && e.content == self.legacy_content()
            && e.engine == now.engine
            && e.engine_version == now.engine_version
    }

    /// The platform as `lock` pins it. A `bb-lock/0` file's entry that holds
    /// the legacy hash of these very files (which pins every plugin source,
    /// so more than [`PlatformDef::content`] does) is accepted as it stands,
    /// so `bb run` reads an unmigrated lock (ADR-015 Amendment 3). Otherwise,
    /// and always for a `bb-lock/1` file (milestone S wrote every committed
    /// entry with the scoped hash), [`PlatformDef::locked`], and `drift`
    /// reports any difference.
    pub fn locked_for(&self, lock: Option<&Lock>) -> LockedPlatform {
        let now = self.locked();
        match lock
            .filter(|l| l.major == 0)
            .and_then(|l| l.platforms.iter().find(|e| e.id == now.id))
        {
            Some(e) if self.is_legacy(e) => e.clone(),
            _ => now,
        }
    }
}

/// Migrate a `bb-lock/0` lock in memory to `bb-lock/1` (ADR-011 D5, ADR-015
/// Amendment 3): each Renode platform entry that holds the legacy hash of
/// this `bb`'s files becomes the scoped entry (the legacy hash pins strictly
/// more, so no pin is weakened); an entry for a Renode platform this `bb`
/// does not provide (an old platform major no package names any more, which
/// cannot be written with a scoped hash) is dropped; every other entry is
/// kept as it was, so `drift` still reports a stale one. Returns whether the
/// lock was at the previous major.
pub fn migrate_lock(lock: &mut Lock) -> bool {
    if lock.major == bbpkg::lock::LOCK_MAJOR {
        return false;
    }
    lock.platforms
        .retain(|e| !e.id.starts_with("renode:") || PLATFORMS.iter().any(|p| p.lock_id() == e.id));
    for e in &mut lock.platforms {
        if let Some(def) = PLATFORMS.iter().find(|p| p.lock_id() == e.id) {
            if def.is_legacy(e) {
                *e = def.locked();
            }
        }
    }
    lock.major = bbpkg::lock::LOCK_MAJOR;
    true
}

/// A `bbdev` device's identity (ADR-006 D8, Amendment 6 A6.4): locked as
/// `device:<id>`, its content the list hash of its sources (the device API,
/// its engine and the device's own files, `bbdev::DEVICES`).
pub fn device_locked(def: &bbdev::DeviceDef) -> LockedPlatform {
    let files: Vec<(&str, &[u8])> = def
        .sources
        .iter()
        .map(|(n, s)| (*n, s.as_bytes()))
        .collect();
    LockedPlatform {
        id: format!("device:{}", def.id),
        content: bbpkg::lock::content_hash(&files),
        engine: None,
        engine_version: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_identities() {
        let d = bbdev::find("dms/mechanism@1").unwrap();
        let l = device_locked(d);
        assert_eq!(l.id, "device:dms/mechanism@1");
        assert!(l.content.starts_with("sha256:") && l.content.len() == 71);
        assert!(l.engine.is_none() && l.engine_version.is_none());
        assert!(PLATFORMS.iter().all(|p| p.content() != l.content));
    }

    #[test]
    fn ids_are_unique_and_hashes_differ() {
        let ids = ids();
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len());
        let c: Vec<String> = PLATFORMS.iter().map(PlatformDef::content).collect();
        assert_ne!(c[0], c[1]);
        assert!(c.iter().all(|h| h.starts_with("sha256:") && h.len() == 71));
        assert_eq!(find("stm32f072-bb@1").unwrap().board_name, "stm32f072");
        assert_eq!(find("nrf52840-bb@3").unwrap().board_name, "nrf52840");
        assert!(c[2] != c[0] && c[2] != c[1]);
        // The scoped hash is not the legacy one (no platform declares every
        // source), so a relocked entry is told apart from a legacy one.
        assert!(PLATFORMS.iter().all(|p| p.content() != p.legacy_content()));
    }

    #[test]
    fn the_declarations_are_complete_and_name_real_sources() {
        // Every plugin source is declared by some platform, and every name a
        // platform declares is a plugin source.
        for (n, _) in PLUGIN_SOURCES {
            assert!(
                PLATFORMS.iter().any(|p| p.plugins().contains(n)),
                "{n} is declared by no platform"
            );
        }
        for p in PLATFORMS {
            let pl = p.plugins();
            assert!(pl.contains(&"bb_lookahead.cs"), "{}", p.id);
            for n in &pl {
                assert!(bb_cpu_renode::plugin_source(n).is_some(), "{}: {n}", p.id);
            }
        }
        assert_eq!(
            find("stm32f103-offline@1").unwrap().plugins(),
            ["bb_lookahead.cs"]
        );
        assert_eq!(
            find("stm32f072-bb@1").unwrap().plugins(),
            ["bb_lookahead.cs", "bb_stm32_timer.cs", "bb_stm32f0_adc.cs"]
        );
        assert_eq!(
            find("nrf52840-bb@3").unwrap().plugins(),
            ["bb_lookahead.cs", "bb_nrf52_saadc.cs", "bb_nrf52_pwm.cs"]
        );
    }

    /// ADR-015 G5: an edit to one plugin source changes exactly the hashes
    /// of the platforms that declare it; editing `bb_nrf52_saadc.cs` changes
    /// no STM32 platform's hash. The legacy hash, for contrast, changes on
    /// every platform (the Phase 1 lock churn, `ee40df0`).
    #[test]
    fn an_edit_changes_only_the_platforms_that_declare_the_source() {
        for (edited, text) in PLUGIN_SOURCES {
            let changed = format!("{text}// an edit\n");
            let sources: Vec<(&str, &str)> = PLUGIN_SOURCES
                .iter()
                .map(|(n, s)| {
                    if n == edited {
                        (*n, changed.as_str())
                    } else {
                        (*n, *s)
                    }
                })
                .collect();
            for p in PLATFORMS {
                let declared = p.plugins().contains(edited);
                assert_eq!(
                    p.content_from(&sources) != p.content(),
                    declared,
                    "{} after an edit to {edited}",
                    p.id
                );
            }
        }
        let saadc = format!("{}// an edit\n", bb_cpu_renode::BB_NRF52_SAADC_CS);
        let sources: Vec<(&str, &str)> = PLUGIN_SOURCES
            .iter()
            .map(|(n, s)| {
                if *n == "bb_nrf52_saadc.cs" {
                    (*n, saadc.as_str())
                } else {
                    (*n, *s)
                }
            })
            .collect();
        for id in ["stm32f103-offline@1", "stm32f072-bb@1"] {
            let p = find(id).unwrap();
            assert_eq!(p.content_from(&sources), p.content(), "{id}");
        }
        let nrf = find("nrf52840-bb@3").unwrap();
        assert_ne!(nrf.content_from(&sources), nrf.content());
    }

    #[test]
    fn a_legacy_entry_is_kept_only_in_a_bb_lock_0_file_when_it_matches_exactly() {
        let p = find("stm32f103-offline@1").unwrap();
        let entry = |major: u32, content: String, version: Option<&str>| Lock {
            major,
            platforms: vec![LockedPlatform {
                id: p.lock_id(),
                content,
                engine: version.map(|_| "renode".to_string()),
                engine_version: version.map(str::to_string),
            }],
            ..Lock::default()
        };
        // No lock, or no entry: the scoped hash.
        assert_eq!(p.locked_for(None), p.locked());
        assert_eq!(p.locked_for(Some(&Lock::default())), p.locked());
        // The legacy hash of these files in a bb-lock/0 file: kept as it
        // stands (bb run reads an unmigrated lock).
        let legacy = entry(0, p.legacy_content(), Some(RENODE_VERSION));
        assert_eq!(p.locked_for(Some(&legacy)), legacy.platforms[0]);
        assert!(bbpkg::lock::drift(Some(&legacy), &[], &[p.locked_for(Some(&legacy))]).is_empty());
        // In a bb-lock/1 file the legacy hash is a mismatch: the epoch's
        // relock wrote every entry scoped.
        let legacy1 = entry(1, p.legacy_content(), Some(RENODE_VERSION));
        assert_eq!(p.locked_for(Some(&legacy1)), p.locked());
        assert!(bbpkg::lock::drift(Some(&legacy1), &[], &[p.locked()])[0].is_mismatch());
        // The scoped hash: itself.
        let scoped = entry(1, p.content(), Some(RENODE_VERSION));
        assert_eq!(p.locked_for(Some(&scoped)), p.locked());
        // Anything else (another legacy hash, another Renode): the scoped
        // entry, which drift reports as a mismatch.
        for other in [
            entry(
                0,
                format!("sha256:{}", "0".repeat(64)),
                Some(RENODE_VERSION),
            ),
            entry(0, p.legacy_content(), Some("1.16.0")),
            entry(
                0,
                find("stm32f072-bb@1").unwrap().legacy_content(),
                Some(RENODE_VERSION),
            ),
        ] {
            let now = p.locked_for(Some(&other));
            assert_eq!(now, p.locked());
            let d = bbpkg::lock::drift(Some(&other), &[], &[now]);
            assert!(d.len() == 1 && d[0].is_mismatch(), "{d:?}");
        }
    }

    #[test]
    fn migrating_a_lock_scopes_exactly_the_legacy_entries() {
        // ADR-015 Amendment 3, ADR-011 D5: the bb-lock/1 migration writes a
        // matching legacy entry with the scoped hash, keeps a stale one (so
        // drift still reports it) and keeps device entries.
        let a = find("stm32f103-offline@1").unwrap();
        let b = find("nrf52840-bb@3").unwrap();
        let dev = device_locked(bbdev::find("dms/mechanism@1").unwrap());
        let stale = LockedPlatform {
            content: format!("sha256:{}", "1".repeat(64)),
            ..b.locked()
        };
        let legacy = LockedPlatform {
            content: a.legacy_content(),
            ..a.locked()
        };
        let mut lock = Lock {
            major: 0,
            // Sorted by id, as a lock file is. renode:nrf52840-bb@1 is an old
            // platform major this bb does not provide: dropped.
            platforms: vec![
                dev.clone(),
                LockedPlatform {
                    id: "renode:nrf52840-bb@1".into(),
                    ..stale.clone()
                },
                stale.clone(),
                legacy,
            ],
            ..Lock::default()
        };
        assert!(migrate_lock(&mut lock));
        assert_eq!(lock.major, 1);
        assert_eq!(lock.platforms, vec![dev, stale, a.locked()]);
        assert!(!migrate_lock(&mut lock));
        let back = Lock::parse(&lock.to_text(), &bbpkg::schema::Registry::current()).unwrap();
        assert_eq!(back.platforms, lock.platforms);
    }
}
