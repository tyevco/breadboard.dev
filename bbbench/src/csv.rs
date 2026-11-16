//! The canonical bench CSV (ADR-010 D3): the one runtime format of a
//! playback, a current profile or a digital stream.
//!
//! - A header `t,<name>`; then one `<t>,<value>` row per point.
//! - `t` is in seconds, a plain decimal string, converted to whole fs exactly
//!   (decimal arithmetic, round half up); times strictly increase after that
//!   conversion.
//! - A value is in the package value grammar (`3.3V`, `50mA`, `3.3`) or, for
//!   a digital stream, `0` or `1`.
//! - Lines starting with `#` (the importer's provenance comment) and empty
//!   lines are ignored; a trailing `\r` is ignored.
//!
//! Reading does not depend on float formatting: each value is an exact
//! decimal, made an `f64` once (correctly rounded).

use bbcore::{Level, Time};
use bbpkg::value::{parse_decimal, seconds_to_fs_half_up, Dim};

/// What the value column holds.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Column {
    /// Volts (an analog playback).
    Volts,
    /// Amps (a current profile).
    Amps,
    /// `0` or `1` (a digital stream into an MCU pin).
    Level,
}

/// A table read from a bench CSV.
#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    /// The header's value column name.
    pub name: String,
    /// `(t, value)`: V, A, or 0.0 / 1.0 for a level.
    pub points: Vec<(Time, f64)>,
}

impl Table {
    /// The levels of a digital table.
    pub fn levels(&self) -> Vec<(Time, Level)> {
        self.points
            .iter()
            .map(|&(t, v)| (t, if v > 0.5 { Level::High } else { Level::Low }))
            .collect()
    }
}

/// Parse a bench CSV.
pub fn parse(text: &str, col: Column) -> Result<Table, String> {
    let mut name: Option<String> = None;
    let mut points: Vec<(Time, f64)> = Vec::new();
    for (k, raw) in text.split('\n').enumerate() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let at = k + 1;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (a, b) = line
            .split_once(',')
            .filter(|(_, b)| !b.contains(','))
            .ok_or_else(|| format!("line {at}: expected two columns, t,<value>"))?;
        if name.is_none() {
            if a != "t" || b.is_empty() {
                return Err(format!(
                    "line {at}: the header must be t,<name> (ADR-010 D3), got {line:?}"
                ));
            }
            name = Some(b.to_string());
            continue;
        }
        let t = Time::from_fs(seconds_to_fs_half_up(a).map_err(|e| format!("line {at}: t {e}"))?);
        let v = match col {
            Column::Level => match b.trim() {
                "0" => 0.0,
                "1" => 1.0,
                x => return Err(format!("line {at}: a level is 0 or 1, got {x:?}")),
            },
            Column::Volts | Column::Amps => {
                let dim = if col == Column::Volts {
                    Dim::Voltage
                } else {
                    Dim::Current
                };
                parse_decimal(b, dim)
                    .map_err(|e| format!("line {at}: {e}"))?
                    .to_f64()
            }
        };
        if let Some(&(prev, _)) = points.last() {
            if t <= prev {
                return Err(format!(
                    "line {at}: t = {a} ({} fs) does not increase after {} fs (times are whole fs, \
                     rounded half up)",
                    t.as_fs(),
                    prev.as_fs()
                ));
            }
        }
        points.push((t, v));
    }
    let name = name.ok_or("no header: a bench CSV starts with t,<name> (ADR-010 D3)")?;
    if points.is_empty() {
        return Err("no rows after the header".into());
    }
    Ok(Table { name, points })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_csv() {
        let t = parse(
            "# sha256:00 (bb capture import)\nt,VIN\n0,0\n1.5e-15,1\n0.00001,1.5V\n2e-5,3300mV\r\n\n",
            Column::Volts,
        )
        .unwrap();
        assert_eq!(t.name, "VIN");
        assert_eq!(
            t.points,
            vec![
                (Time::ZERO, 0.0),
                // 1.5 fs rounds half up to 2 fs.
                (Time::from_fs(2), 1.0),
                (Time::from_us(10), 1.5),
                (Time::from_us(20), 3.3),
            ]
        );
        let l = parse("t,btn\n0,1\n1e-3,0\n", Column::Level).unwrap();
        assert_eq!(
            l.levels(),
            vec![(Time::ZERO, Level::High), (Time::from_ms(1), Level::Low)]
        );
        let a = parse("t,i\n0,50mA\n", Column::Amps).unwrap();
        assert_eq!(a.points, vec![(Time::ZERO, 0.05)]);
        // Errors: no header, three columns, non-increasing times after the
        // fs rounding, a unit of another dimension, a level other than 0/1,
        // a prefixed time, a negative time.
        let err = |s: &str, c| parse(s, c).unwrap_err();
        assert!(err("0,1\n", Column::Volts).contains("header"));
        assert!(err("t,a,b\n", Column::Volts).contains("two columns"));
        assert!(err("t,a\n1e-15,0\n1.4e-15,1\n", Column::Volts).contains("does not increase"));
        assert!(err("t,a\n0,1A\n", Column::Volts).contains("unit"));
        assert!(err("t,a\n0,2\n", Column::Level).contains("0 or 1"));
        assert!(err("t,a\n1ms,2\n", Column::Volts).contains("plain decimal"));
        assert!(err("t,a\n-1,2\n", Column::Volts).contains("negative"));
        assert!(err("t,a\n", Column::Volts).contains("no rows"));
    }
}
