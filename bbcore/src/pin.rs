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

/// A digital input with hysteresis. When an analog engine drives the net,
/// that (producing) engine applies [`DigitalIn::threshold`] at every solver
/// point and reports the level changes in its trace (ADR-003 D3,
/// `spec/pin-model.md`).
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

/// What an `adc_in` push key is: the finest value the consumer's converter
/// can see (ADR-005 D5, `spec/pin-model.md`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdcKey {
    /// The 12-bit code [`AdcIn::code12`] of the delivered microvolts, which
    /// are delivered raw: Breadboard's ADC model (ADR-005), the default.
    #[default]
    Code12,
    /// Whole millivolts, delivered as exactly that many millivolts:
    /// Renode 1.17.0's stock `STM32_ADC_Common`, which converts
    /// `floor(uV / 1000)` (ADR-003 Amendments 5 and 10; `bb run
    /// --adc-model stock`).
    Millivolts,
}

/// An ADC input (ADR-003 D6, ADR-005 D5). A *push* model: the producing
/// analog engine reads the net at every lattice point and emits an `Analog`
/// transition, at that lattice point, whenever the value the consumer's
/// converter can see changes: [`AdcIn::push_key`] of
/// [`AdcIn::delivered_microvolts`] (clamped to `[0, vref]`), delivered as
/// [`AdcIn::key_microvolts`]. With [`AdcKey::Code12`] the key is the 12-bit
/// code and the raw microvolts are delivered, so every resolution and
/// alignment converts what the every-lattice-point reference converts; with
/// [`AdcKey::Millivolts`] (the stock model) the key is whole millivolts.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AdcIn {
    /// Reference voltage (V), the full-scale input.
    pub vref: f64,
    /// Converter resolution (bits); documents the code mapping
    /// ([`AdcIn::code`]), never the push rule.
    pub bits: u32,
    /// The push key (which converter consumes the pushes).
    #[serde(default)]
    pub key: AdcKey,
}

impl AdcIn {
    /// The STM32 12-bit ADC at 3.3 V, on Breadboard's ADC model.
    pub const STM32_12BIT_3V3: AdcIn = AdcIn {
        vref: 3.3,
        bits: 12,
        key: AdcKey::Code12,
    };

    /// `vref` in whole microvolts.
    pub fn vref_microvolts(&self) -> u64 {
        u64::try_from(crate::signal::volts_to_microvolts(self.vref).max(1)).unwrap_or(1)
    }

    /// Breadboard's ADC model conversion (ADR-005 D5.2, approver decision
    /// 2): `min(4095, round_half_up(uV * 4095 / Vref_uV))` in integers,
    /// whatever `bits` (lower resolutions use `code12` with low bits
    /// cleared).
    pub fn code12(&self, uv: u32) -> u32 {
        let v = self.vref_microvolts();
        let c = (2 * u64::from(uv) * 4095 + v) / (2 * v);
        u32::try_from(c.min(4095)).unwrap_or(4095)
    }

    /// The push key of a delivered value ([`AdcKey`]).
    pub fn push_key(&self, uv: u32) -> u32 {
        match self.key {
            AdcKey::Code12 => self.code12(uv),
            AdcKey::Millivolts => uv / 1000,
        }
    }

    /// The value delivered at a push of `uv`: `uv` itself
    /// ([`AdcKey::Code12`]) or its whole millivolts ([`AdcKey::Millivolts`]).
    pub fn key_microvolts(&self, uv: u32) -> u32 {
        match self.key {
            AdcKey::Code12 => uv,
            AdcKey::Millivolts => uv / 1000 * 1000,
        }
    }

    /// The voltage delivered to the ADC, in integer microvolts: `volts`
    /// rounded half away from zero ([`crate::volts_to_microvolts`]) and
    /// clamped to `[0, vref]`.
    pub fn delivered_microvolts(&self, volts: f64) -> u32 {
        let max = crate::signal::volts_to_microvolts(self.vref).max(0);
        let uv = crate::signal::volts_to_microvolts(volts).clamp(0, max);
        u32::try_from(uv).unwrap_or(u32::MAX)
    }

    /// The conversion of `uv` by Renode 1.17.0's `STM32_ADC_Common`
    /// (Amendment 5, fact 1): `round_half_even(floor(uV / 1000) / (vref *
    /// 1000) * (2^bits - 1))` in IEEE double. For checks and documentation;
    /// the engines never convert.
    pub fn code(&self, uv: u32) -> u32 {
        let mv = f64::from(uv / 1000);
        let full = f64::from((1u32 << self.bits) - 1);
        (mv / (self.vref * 1000.0) * full).round_ties_even() as u32
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
    /// ADC input: pushed `Analog` transitions at lattice points.
    AdcIn(AdcIn),
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

    #[test]
    fn adc_push_key_and_code() {
        let a = AdcIn::STM32_12BIT_3V3;
        // Clamped to [0, vref]; rounded to whole microvolts.
        assert_eq!(a.delivered_microvolts(-0.2), 0);
        assert_eq!(a.delivered_microvolts(3.4), 3_300_000);
        assert_eq!(a.delivered_microvolts(1.2345674), 1_234_567);
        assert_eq!(a.delivered_microvolts(0.0019999996), 2_000);
        // The stock key: whole millivolts, delivered as such.
        let mv = AdcIn {
            key: AdcKey::Millivolts,
            ..a
        };
        assert_eq!(mv.push_key(2_000_999), 2000);
        assert_eq!(mv.push_key(999_999), 999);
        assert_eq!(mv.key_microvolts(2_531_087), 2_531_000);
        // Breadboard's model: code12, round half up, saturated; raw uV.
        assert_eq!(a.key_microvolts(2_531_087), 2_531_087);
        for (uv, code) in [
            (0, 0),
            (402, 0),
            (403, 1),
            (329_999, 409),
            (330_000, 410),
            (1_650_000, 2048),
            (2_531_087, 3141),
            (3_299_999, 4095),
            (3_300_000, 4095),
            (4_000_000, 4095),
        ] {
            assert_eq!(a.code12(uv), code, "{uv} uV");
            assert_eq!(a.push_key(uv), code);
        }
        // Amendment 5's pinned conversions.
        for (uv, code) in [
            (1_500_000, 1861),
            (600_000, 745),
            (1_650_000, 2048),
            (2_500_000, 3102),
            (2_000_999, 2482),
            (999_999, 1240),
            (1_000_000, 1241),
            (0, 0),
        ] {
            assert_eq!(a.code(uv), code, "{uv} uV");
        }
    }
}
