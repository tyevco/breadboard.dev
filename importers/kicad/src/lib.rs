//! Breadboard's KiCad importer (ADR-007 D1-D3).
//!
//! - [`sexp`]: an S-expression reader (no regex);
//! - [`netlist`]: the netlist model (format "E") and its version gate;
//! - [`schematic`]: the schematic version gate, sheet discovery and the
//!   Phase 1 refusals;
//! - [`cli`]: the `kicad-cli` runner, on a temporary copy of the project.
//!
//! Resolution, `board.bbm` and the project file are in `bb` (`cli/src`),
//! which has the packages.

pub mod cli;
pub mod netlist;
pub mod schematic;
pub mod sexp;
