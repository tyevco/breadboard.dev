//! Analog engine backed by the ngspice shared library (`libngspice.so.0`).
//!
//! Placeholder crate created in Phase 2 ("Foundations") so the workspace builds.
//! The ngspice engine agent implements [`bbcore::Engine`] here, following
//! `docs/adr/002-time-grant-protocol.md` (including the open sub-decision on the
//! pause mechanism) and `spec/time-grant.md`.
//!
//! Known bad idea: never step ngspice per CPU instruction.

/// Name this engine reports through [`bbcore::Engine::name`].
pub const ENGINE_NAME: &str = "ngspice";
