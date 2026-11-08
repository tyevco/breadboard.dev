//! CPU engine backed by a headless Renode process driven through its
//! External Control API.
//!
//! Placeholder crate created in Phase 2 ("Foundations") so the workspace builds.
//! The Renode engine agent implements [`bbcore::Engine`] here, following
//! `docs/adr/002-time-grant-protocol.md` and `spec/time-grant.md`.
//!
//! Known bad idea: never let Renode's virtual time run ahead of the kernel
//! grant. Renode is advanced only by `RunFor(grant - now)`.

/// Name this engine reports through [`bbcore::Engine::name`].
pub const ENGINE_NAME: &str = "renode";
