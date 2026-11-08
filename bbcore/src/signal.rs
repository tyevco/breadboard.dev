//! Pin signals: the values that cross the pin boundary between engines.
//!
//! See `spec/pin-model.md`. The kernel routes a [`PinSignal`] from the engine
//! driving a net to every engine loading that net, unchanged. Conversion between
//! domains (digital -> PWL voltage, analog -> thresholded level) happens in the
//! consuming engine using the pin models from `crate::pin`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::time::Time;

/// A two-state logic level.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Low,
    High,
}

impl Level {
    /// `0` for [`Level::Low`], `1` for [`Level::High`].
    pub const fn as_bit(self) -> u8 {
        match self {
            Level::Low => 0,
            Level::High => 1,
        }
    }
    /// `true` maps to [`Level::High`].
    pub const fn from_bool(high: bool) -> Level {
        if high {
            Level::High
        } else {
            Level::Low
        }
    }
    /// The opposite level.
    pub const fn toggled(self) -> Level {
        match self {
            Level::Low => Level::High,
            Level::High => Level::Low,
        }
    }
}

/// The value of one pin (or net) at a boundary.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PinSignal {
    /// Digital level plus the absolute time of the most recent level change.
    ///
    /// `edge_time` must stay the same while the level is unchanged. At the
    /// boundary where the level changes, the new `edge_time` must lie in the
    /// half-open interval `(previous boundary, this boundary]`.
    Digital { level: Level, edge_time: Time },
    /// Analog node voltage in volts (relative to ground).
    Analog { volts: f64 },
}

impl PinSignal {
    /// Convenience constructor for a digital signal.
    pub const fn digital(level: Level, edge_time: Time) -> PinSignal {
        PinSignal::Digital { level, edge_time }
    }
    /// Convenience constructor for an analog signal.
    pub const fn analog(volts: f64) -> PinSignal {
        PinSignal::Analog { volts }
    }
    /// The digital level, if this is a digital signal.
    pub const fn level(&self) -> Option<Level> {
        match self {
            PinSignal::Digital { level, .. } => Some(*level),
            PinSignal::Analog { .. } => None,
        }
    }
    /// The voltage, if this is an analog signal.
    pub const fn volts(&self) -> Option<f64> {
        match self {
            PinSignal::Analog { volts } => Some(*volts),
            PinSignal::Digital { .. } => None,
        }
    }
    /// True for [`PinSignal::Digital`].
    pub const fn is_digital(&self) -> bool {
        matches!(self, PinSignal::Digital { .. })
    }
}

/// A set of pin signals keyed by pin name (engine side) or net name (kernel
/// side). A `BTreeMap` so that iteration order, and therefore every output
/// derived from it, is deterministic.
pub type PinSignals = BTreeMap<String, PinSignal>;

/// Current value of every net that has been driven, keyed by net name.
pub type NetValues = BTreeMap<String, PinSignal>;

/// Convert volts to integer microvolts, rounding half away from zero.
/// Used wherever a voltage is written to a text output so the text is stable.
pub fn volts_to_microvolts(volts: f64) -> i64 {
    (volts * 1e6).round() as i64
}
