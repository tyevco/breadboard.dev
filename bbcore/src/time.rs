//! Simulation time: an integer count of femtoseconds.
//!
//! All scheduling decisions in the kernel are made with [`Time`]. It is a
//! `u64` of femtoseconds, so the range is about 18 446 s (~5.1 h) of simulated
//! time. Floating point is never used to decide a grant; [`Time::as_secs_f64`]
//! exists only for human-facing output and analog value computation.

use std::fmt;
use std::ops::{Add, Sub};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Femtoseconds per picosecond.
pub const FS_PER_PS: u64 = 1_000;
/// Femtoseconds per nanosecond.
pub const FS_PER_NS: u64 = 1_000_000;
/// Femtoseconds per microsecond.
pub const FS_PER_US: u64 = 1_000_000_000;
/// Femtoseconds per millisecond.
pub const FS_PER_MS: u64 = 1_000_000_000_000;
/// Femtoseconds per second.
pub const FS_PER_S: u64 = 1_000_000_000_000_000;

/// Absolute simulation time (or a duration) in integer femtoseconds.
///
/// Serialized as a bare JSON integer of femtoseconds.
#[derive(
    Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Time(u64);

const fn mul_or_panic(n: u64, unit: u64) -> Time {
    match n.checked_mul(unit) {
        Some(v) => Time(v),
        None => panic!("Time overflow: value does not fit in u64 femtoseconds"),
    }
}

impl Time {
    /// Time zero.
    pub const ZERO: Time = Time(0);
    /// Largest representable time. Engines with nothing scheduled return this
    /// from `next_safe_time` ("idle").
    pub const MAX: Time = Time(u64::MAX);

    /// From femtoseconds.
    pub const fn from_fs(fs: u64) -> Time {
        Time(fs)
    }
    /// From picoseconds. Panics on overflow.
    pub const fn from_ps(ps: u64) -> Time {
        mul_or_panic(ps, FS_PER_PS)
    }
    /// From nanoseconds. Panics on overflow.
    pub const fn from_ns(ns: u64) -> Time {
        mul_or_panic(ns, FS_PER_NS)
    }
    /// From microseconds. Panics on overflow.
    pub const fn from_us(us: u64) -> Time {
        mul_or_panic(us, FS_PER_US)
    }
    /// From milliseconds. Panics on overflow.
    pub const fn from_ms(ms: u64) -> Time {
        mul_or_panic(ms, FS_PER_MS)
    }
    /// From whole seconds. Panics on overflow.
    pub const fn from_secs(s: u64) -> Time {
        mul_or_panic(s, FS_PER_S)
    }

    /// Raw femtosecond count.
    pub const fn as_fs(self) -> u64 {
        self.0
    }

    /// Seconds as `f64`. **Output / analog math only**; never use the result
    /// to make a scheduling decision.
    pub fn as_secs_f64(self) -> f64 {
        // Split to keep full precision for large values.
        (self.0 / FS_PER_S) as f64 + (self.0 % FS_PER_S) as f64 * 1e-15
    }

    /// Checked addition.
    pub const fn checked_add(self, rhs: Time) -> Option<Time> {
        match self.0.checked_add(rhs.0) {
            Some(v) => Some(Time(v)),
            None => None,
        }
    }
    /// Checked subtraction.
    pub const fn checked_sub(self, rhs: Time) -> Option<Time> {
        match self.0.checked_sub(rhs.0) {
            Some(v) => Some(Time(v)),
            None => None,
        }
    }
    /// Saturating addition (clamps at [`Time::MAX`]).
    pub const fn saturating_add(self, rhs: Time) -> Time {
        Time(self.0.saturating_add(rhs.0))
    }
    /// Saturating subtraction (clamps at [`Time::ZERO`]).
    pub const fn saturating_sub(self, rhs: Time) -> Time {
        Time(self.0.saturating_sub(rhs.0))
    }
    /// True if this is [`Time::ZERO`].
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }
}

impl Add for Time {
    type Output = Time;
    /// Panics on overflow (in every build profile).
    fn add(self, rhs: Time) -> Time {
        self.checked_add(rhs).expect("Time addition overflowed")
    }
}

impl Sub for Time {
    type Output = Time;
    /// Panics on underflow (in every build profile).
    fn sub(self, rhs: Time) -> Time {
        self.checked_sub(rhs).expect("Time subtraction underflowed")
    }
}

const UNITS: [(&str, u64); 6] = [
    ("s", FS_PER_S),
    ("ms", FS_PER_MS),
    ("us", FS_PER_US),
    ("ns", FS_PER_NS),
    ("ps", FS_PER_PS),
    ("fs", 1),
];

/// Exact, round-trippable rendering: the largest unit that divides the value
/// exactly, e.g. `500ms`, `1500us`, `7fs`, `0s`.
impl fmt::Display for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 == 0 {
            return f.write_str("0s");
        }
        for (name, unit) in UNITS {
            if self.0.is_multiple_of(unit) {
                return write!(f, "{}{name}", self.0 / unit);
            }
        }
        unreachable!("fs divides everything")
    }
}

/// Error parsing a [`Time`] literal.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid time literal {input:?}: {reason}")]
pub struct TimeParseError {
    input: String,
    reason: &'static str,
}

/// Parses `<int>[.<frac>]<unit>` with unit in `s, ms, us, µs, ns, ps, fs`
/// using exact integer arithmetic, e.g. `500ms`, `2.5us`, `10ns`.
impl FromStr for Time {
    type Err = TimeParseError;

    fn from_str(s: &str) -> Result<Time, TimeParseError> {
        let err = |reason| TimeParseError {
            input: s.to_string(),
            reason,
        };
        let t = s.trim();
        let split = t
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .ok_or_else(|| err("missing unit (s, ms, us, ns, ps, fs)"))?;
        let (num, unit) = t.split_at(split);
        let unit_fs = match unit.trim() {
            "s" => FS_PER_S,
            "ms" => FS_PER_MS,
            "us" | "\u{b5}s" | "\u{3bc}s" => FS_PER_US,
            "ns" => FS_PER_NS,
            "ps" => FS_PER_PS,
            "fs" => 1,
            _ => return Err(err("unknown unit")),
        };
        let (int_part, frac_part) = match num.split_once('.') {
            Some((i, f)) => (i, f),
            None => (num, ""),
        };
        if int_part.is_empty() && frac_part.is_empty() {
            return Err(err("missing number"));
        }
        if frac_part.contains('.') {
            return Err(err("more than one decimal point"));
        }
        let int_val: u64 = if int_part.is_empty() {
            0
        } else {
            int_part
                .parse()
                .map_err(|_| err("integer part too large"))?
        };
        let mut total = int_val
            .checked_mul(unit_fs)
            .ok_or_else(|| err("value overflows u64 femtoseconds"))?;
        // Fractional digits: each digit is worth unit_fs / 10^k femtoseconds.
        let mut place = unit_fs;
        for c in frac_part.chars() {
            if !place.is_multiple_of(10) {
                return Err(err("finer than 1 fs"));
            }
            place /= 10;
            let d = u64::from(c.to_digit(10).ok_or_else(|| err("bad digit"))?);
            total = d
                .checked_mul(place)
                .and_then(|v| total.checked_add(v))
                .ok_or_else(|| err("value overflows u64 femtoseconds"))?;
        }
        Ok(Time(total))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_and_units() {
        assert_eq!(Time::from_ns(1).as_fs(), 1_000_000);
        assert_eq!(Time::from_us(10), Time::from_ns(10_000));
        assert_eq!(Time::from_ms(50), Time::from_us(50_000));
        assert_eq!(Time::from_secs(1), Time::from_ms(1000));
    }

    #[test]
    fn display_is_exact_and_round_trips() {
        for (t, s) in [
            (Time::ZERO, "0s"),
            (Time::from_ms(500), "500ms"),
            (Time::from_us(1500), "1500us"),
            (Time::from_fs(7), "7fs"),
            (Time::from_secs(2), "2s"),
        ] {
            assert_eq!(t.to_string(), s);
            assert_eq!(s.parse::<Time>().unwrap(), t);
        }
    }

    #[test]
    fn parse_fractions_exactly() {
        assert_eq!("2.5us".parse::<Time>().unwrap(), Time::from_ns(2500));
        assert!("0.001fs".parse::<Time>().is_err());
        assert!("1.5fs".parse::<Time>().is_err());
        assert_eq!(".5ms".parse::<Time>().unwrap(), Time::from_us(500));
        assert!("10".parse::<Time>().is_err());
        assert!("10 parsecs".parse::<Time>().is_err());
        assert!("99999999s".parse::<Time>().is_err());
    }

    #[test]
    fn arithmetic() {
        let a = Time::from_us(3);
        let b = Time::from_us(1);
        assert_eq!(a - b, Time::from_us(2));
        assert_eq!(a + b, Time::from_us(4));
        assert_eq!(Time::MAX.saturating_add(a), Time::MAX);
        assert_eq!(b.saturating_sub(a), Time::ZERO);
        assert_eq!(Time::MAX.checked_add(b), None);
    }

    #[test]
    #[should_panic(expected = "underflowed")]
    fn sub_underflow_panics() {
        let _ = Time::from_fs(1) - Time::from_fs(2);
    }

    #[test]
    fn serializes_as_integer_fs() {
        assert_eq!(serde_json::to_string(&Time::from_ns(5)).unwrap(), "5000000");
    }
}
