//! `bb`: Breadboard command-line interface.
//!
//! Placeholder created in Phase 2 ("Foundations") so the workspace builds. The
//! integration agent implements
//! `bb run --netlist <file> --firmware <elf> --for <t> --probe net:X --out <dir>`.

use std::process::ExitCode;

fn main() -> ExitCode {
    eprintln!(
        "bb {}: no commands implemented yet (Phase 0 placeholder; time unit = {} fs/ns)",
        env!("CARGO_PKG_VERSION"),
        bbcore::Time::from_ns(1).as_fs()
    );
    ExitCode::from(2)
}
