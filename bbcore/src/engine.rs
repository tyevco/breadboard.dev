//! The engine trait: the only way the kernel talks to a simulation engine.
//!
//! Protocol: `docs/adr/002-time-grant-protocol.md` and `spec/time-grant.md`.

use crate::event::LogRecord;
use crate::signal::{PinSignal, PinSignals};
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
/// 1. `next_safe_time(now)`: the largest grant this engine accepts next. It
///    promises that if it is advanced to any `g <= t_safe`, every output
///    change in `(now, g]` is reported at the boundary `g` with its exact
///    `edge_time`, at most one change per digital pin (ADR-002 Amendment 3).
///    It does **not** promise that outputs stay constant inside the grant: a
///    CPU may toggle a pin at any instruction. Consumers registered *after*
///    the producer receive the change before they advance and apply it at
///    `edge_time` (Gauss-Seidel); consumers registered *before* it receive it
///    one grant late, at the boundary (Jacobi). Must be strictly greater than
///    `now`. Return [`Time::MAX`] when idle.
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

    /// The grant lattice `L` this engine requires (ADR-003 D2): every grant
    /// boundary must be a multiple of it, because the engine's results
    /// depend on where its advances end (Renode: its global quantum). The
    /// kernel rounds every grant down to the lattice and rejects a
    /// configuration whose `max_quantum`, `t_end` or aligned sample instants
    /// are off it. All declared lattices must be equal. A lattice-bound
    /// engine applies an input at `ceil_L(edge_time)`, never earlier.
    /// Default: `None` (not lattice-bound).
    fn grant_lattice(&self) -> Option<Time> {
        None
    }

    /// Value of output pin `pin` at `t`, for `prev <= t <= now` where `prev`
    /// is the previous boundary, if the engine can reconstruct it (e.g. by
    /// interpolating solver points). Must be a pure function of the engine
    /// state. Used by sampled probes, so that output sample instants need
    /// not be grant boundaries. Default: `None` (the kernel falls back to the
    /// boundary values and, for digital pins, `edge_time`).
    fn sample_output(&self, _pin: &str, _t: Time) -> Option<PinSignal> {
        None
    }

    /// Log lines produced since the last call (e.g. UART). Default: none.
    fn drain_logs(&mut self) -> Vec<LogRecord> {
        Vec::new()
    }

    /// Called once, in registration order, each time a `Scheduler::run`
    /// reaches its `t_end` (entry to Done). Only for engine self-checks,
    /// such as the Renode engine's end-of-run cross-check (ADR-003
    /// Amendment 1): it must not change anything that affects results,
    /// because a later `run` may resume. An error fails the run. Default:
    /// nothing.
    fn end_of_run(&mut self) -> Result<(), EngineError> {
        Ok(())
    }
}
