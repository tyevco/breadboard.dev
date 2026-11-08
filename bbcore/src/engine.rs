//! The engine trait: the only way the kernel talks to a simulation engine.
//!
//! Protocol: `docs/adr/002-time-grant-protocol.md` and `spec/time-grant.md`.

use crate::event::LogRecord;
use crate::signal::PinSignals;
use crate::time::Time;

/// Error returned by an engine.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// I/O with the engine process or library failed.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// The engine reported or detected a protocol violation.
    #[error("protocol error: {0}")]
    Protocol(String),
    /// Anything else (solver failure, bad netlist, ...).
    #[error("{0}")]
    Other(String),
}

/// A simulation engine (CPU emulator, analog solver, ...).
///
/// The kernel drives every engine through this loop and nothing else:
///
/// 1. `next_safe_time(now)`: the engine promises that none of its outputs will
///    change in `(now, t_safe)` other than as already reported, i.e. it can be
///    advanced to any `t <= t_safe` without the kernel missing an interaction.
///    Must be strictly greater than `now`. Return [`Time::MAX`] when idle.
/// 2. `advance_to(g)`: advance internal time from `now` to exactly `g`, where
///    `now < g <= t_safe`. The engine must not simulate past `g` (no free-run).
/// 3. `read_outputs()`: values of the engine's output pins at `g`.
/// 4. `apply_inputs(..)`: values of the engine's input pins at `g`; they take
///    effect for the next grant.
///
/// Engines must be deterministic: identical calls produce identical results.
pub trait Engine {
    /// Stable, unique engine name (used in events as `engine:<name>` and
    /// `<name>.<pin>`).
    fn name(&self) -> &str;

    /// Earliest time at which this engine might need an exchange. `> now`.
    fn next_safe_time(&mut self, now: Time) -> Time;

    /// Advance to exactly `t`. Called only with `now < t <= next_safe_time(now)`.
    fn advance_to(&mut self, t: Time) -> Result<(), EngineError>;

    /// Output pin values at the current time, keyed by pin name. Pins omitted
    /// keep their previous value.
    fn read_outputs(&mut self) -> Result<PinSignals, EngineError>;

    /// Input pin values at the current time, keyed by pin name. Always the full
    /// set of bound, driven input pins (a snapshot, not a delta).
    fn apply_inputs(&mut self, inputs: &PinSignals) -> Result<(), EngineError>;

    /// Log lines produced since the last call (e.g. UART). Default: none.
    fn drain_logs(&mut self) -> Vec<LogRecord> {
        Vec::new()
    }
}
