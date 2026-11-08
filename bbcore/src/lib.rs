//! `bbcore`: the Breadboard kernel.
//!
//! - [`Time`]: integer femtoseconds; the only type scheduling decisions use.
//! - [`PinSignal`] / [`PinSignals`]: values crossing the pin boundary.
//! - [`Engine`]: the trait every simulation engine implements.
//! - [`Scheduler`]: conservative lockstep time-grant scheduler with net routing.
//! - [`EventSink`]: deterministic JSON-lines event stream.
//! - [`CsvWriter`] / [`VcdWriter`]: waveform outputs.
//!
//! Protocol: `docs/adr/002-time-grant-protocol.md`, `spec/time-grant.md`.

pub mod engine;
pub mod event;
pub mod pin;
pub mod probe;
pub mod scheduler;
pub mod signal;
pub mod time;

pub use engine::{Engine, EngineError};
pub use event::{Event, EventSink, LogLevel, LogRecord};
pub use pin::{DigitalIn, DigitalOut, PinModel, Power};
pub use probe::{format_volts, CsvWriter, NetSampler, Probe, VcdKind, VcdWriter};
pub use scheduler::{
    Direction, EngineId, EventSampling, GrantPlan, Phase, Plan, RunSummary, SchedError, Scheduler,
    SchedulerConfig,
};
pub use signal::{volts_to_microvolts, Level, NetValues, PinSignal, PinSignals, Transition};
pub use time::{Time, TimeParseError};
