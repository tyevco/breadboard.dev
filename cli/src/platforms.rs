//! Engine platform identities (ADR-006 D8): every Renode platform `bb`
//! provides has an id with a major version and a content hash, which
//! packages name (`[behavioral] platform`) and `bb.lock` pins.
//!
//! The content hash covers what decides the platform's behaviour: the
//! `.repl` description it loads and every source of Breadboard's Renode
//! plugin assembly (`PLUGIN_SOURCES`, the same inputs as ADR-005's plugin
//! cache key), hashed as a sorted `(file name, sha256)` list
//! (`bbpkg::lock::content_hash`). A behaviour change bumps the major and the
//! hash. Renode's own version is pinned (1.17.0) and recorded next to it.
//!
//! A new platform (the nRF52840 of ADR-008) adds a row here.

use bb_cpu_renode::{NRF52840_BB_REPL, PLUGIN_SOURCES, STM32F072_BB_REPL, STM32F103_OFFLINE_REPL};

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

    /// The content hash.
    pub fn content(&self) -> String {
        let mut files: Vec<(&str, &[u8])> = PLUGIN_SOURCES
            .iter()
            .map(|(n, s)| (*n, s.as_bytes()))
            .collect();
        files.push((self.repl.0, self.repl.1.as_bytes()));
        bbpkg::lock::content_hash(&files)
    }

    pub fn locked(&self) -> bbpkg::lock::LockedPlatform {
        bbpkg::lock::LockedPlatform {
            id: self.lock_id(),
            content: self.content(),
            renode: Some(RENODE_VERSION.to_string()),
        }
    }
}

/// A `bbdev` device's identity (ADR-006 D8, Amendment 6 A6.4): locked as
/// `device:<id>`, its content the list hash of its sources (the device API,
/// its engine and the device's own files, `bbdev::DEVICES`).
pub fn device_locked(def: &bbdev::DeviceDef) -> bbpkg::lock::LockedPlatform {
    let files: Vec<(&str, &[u8])> = def
        .sources
        .iter()
        .map(|(n, s)| (*n, s.as_bytes()))
        .collect();
    bbpkg::lock::LockedPlatform {
        id: format!("device:{}", def.id),
        content: bbpkg::lock::content_hash(&files),
        renode: None,
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
        assert!(l.renode.is_none());
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
    }
}
