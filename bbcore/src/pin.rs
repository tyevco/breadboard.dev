//! Pin models (the pin-boundary contract). See `spec/pin-model.md`.
//!
//! Every component pin has exactly one pin model. Electrical parameters are
//! `f64` (they feed analog math only); transition times are [`Time`].

use serde::{Deserialize, Serialize};

use crate::signal::Level;
use crate::time::Time;

/// A push-pull digital output.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DigitalOut {
    /// Output-high voltage (V).
    pub voh: f64,
    /// Output-low voltage (V).
    pub vol: f64,
    /// 0 -> 100 % rise time (linear ramp).
    pub rise: Time,
    /// 100 -> 0 % fall time (linear ramp).
    pub fall: Time,
    /// Output impedance (ohms), realized as a series resistor in SPICE glue.
    pub zout: f64,
}

impl DigitalOut {
    /// The Phase 0 STM32 GPIO model: 3.3 V, 5 ns edges, 25 ohm.
    pub const STM32_GPIO_3V3: DigitalOut = DigitalOut {
        voh: 3.3,
        vol: 0.0,
        rise: Time::from_ns(5),
        fall: Time::from_ns(5),
        zout: 25.0,
    };

    /// Steady-state voltage for a level.
    pub fn level_volts(&self, level: Level) -> f64 {
        match level {
            Level::High => self.voh,
            Level::Low => self.vol,
        }
    }

    /// Source voltage at time `t` for a transition `from -> to` that starts
    /// at `t_start` (a linear ramp over `rise` or `fall`).
    ///
    /// Purely a function of its integer-time inputs, so it is deterministic.
    pub fn ramp_volts(&self, from: Level, to: Level, t_start: Time, t: Time) -> f64 {
        let v0 = self.level_volts(from);
        let v1 = self.level_volts(to);
        if t <= t_start {
            return v0;
        }
        let elapsed = (t - t_start).as_fs();
        let dur = self.duration(to).as_fs();
        if elapsed >= dur {
            return v1;
        }
        v0 + (v1 - v0) * (elapsed as f64 / dur as f64)
    }

    fn duration(&self, to: Level) -> Time {
        match to {
            Level::High => self.rise,
            Level::Low => self.fall,
        }
    }
}

/// A digital input with hysteresis.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DigitalIn {
    /// Input-high threshold (V): rising above it reads High.
    pub vih: f64,
    /// Input-low threshold (V): falling below it reads Low.
    pub vil: f64,
    /// Input capacitance (F).
    pub cin: f64,
}

impl DigitalIn {
    /// Threshold `volts` with hysteresis, given the previously read level.
    pub fn threshold(&self, previous: Level, volts: f64) -> Level {
        match previous {
            Level::Low if volts >= self.vih => Level::High,
            Level::High if volts <= self.vil => Level::Low,
            p => p,
        }
    }
}

/// A power pin.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Power {
    pub vmin: f64,
    pub vmax: f64,
    pub imax: f64,
}

/// The pin model bound to one component pin.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PinModel {
    DigitalOut(DigitalOut),
    DigitalIn(DigitalIn),
    /// Bidirectional analog node; value exchanged as volts.
    Analog,
    Power(Power),
    /// Purely electrical (resistor, LED, ...); lives only inside the analog engine.
    Passive,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ramp_is_linear_and_clamped() {
        let m = DigitalOut::STM32_GPIO_3V3;
        let t0 = Time::from_us(10);
        assert_eq!(m.ramp_volts(Level::Low, Level::High, t0, t0), 0.0);
        let mid = m.ramp_volts(Level::Low, Level::High, t0, t0 + Time::from_ps(2500));
        assert!((mid - 1.65).abs() < 1e-12);
        assert_eq!(
            m.ramp_volts(Level::Low, Level::High, t0, t0 + Time::from_ns(5)),
            3.3
        );
        assert_eq!(
            m.ramp_volts(Level::High, Level::Low, t0, Time::from_us(1)),
            3.3
        );
        assert_eq!(
            m.ramp_volts(Level::High, Level::Low, t0, Time::from_us(11)),
            0.0
        );
    }

    #[test]
    fn hysteresis() {
        let i = DigitalIn {
            vih: 2.0,
            vil: 1.0,
            cin: 5e-12,
        };
        assert_eq!(i.threshold(Level::Low, 1.5), Level::Low);
        assert_eq!(i.threshold(Level::Low, 2.0), Level::High);
        assert_eq!(i.threshold(Level::High, 1.5), Level::High);
        assert_eq!(i.threshold(Level::High, 0.9), Level::Low);
    }
}
