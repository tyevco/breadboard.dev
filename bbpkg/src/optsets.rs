//! Named solver option sets (ADR-006 D6, `spec/package.md`). An option set
//! is part of lattice qualification: a view is qualified for a (lattice,
//! option set) pair, and a looser set is admissible only by evidence.

/// `(name, the exact .options card)`.
pub const OPTION_SETS: &[(&str, &str)] = &[
    // The options of every Phase 0 board.cir; the default for imported
    // boards (ADR-006 D6, approver decision A).
    ("phase0", ".options RELTOL=1e-5 VNTOL=1e-8 ABSTOL=1e-13"),
    // The dial-me-spinner suite's last numeric fallback
    // (`spicelib.NUMERIC_FALLBACKS[1]` at 6854d25: reltol 1e-3, Gear
    // integration, rshunt 1e12).
    (
        "suite_fallback",
        ".options RELTOL=1e-3 METHOD=GEAR RSHUNT=1e12",
    ),
];

/// The `.options` card of a named set.
pub fn get(name: &str) -> Option<&'static str> {
    OPTION_SETS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, c)| *c)
}

pub fn names() -> Vec<&'static str> {
    OPTION_SETS.iter().map(|(n, _)| *n).collect()
}

/// The default set for imported boards and test vectors.
pub const DEFAULT: &str = "phase0";
