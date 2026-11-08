//! Waveform outputs driven by the scheduler at exchange boundaries.
//!
//! A [`Probe`] can ask for boundaries at specific times
//! ([`Probe::next_sample_time`]); the scheduler caps every grant so those
//! times become exchange boundaries. That is how [`CsvWriter`] samples at a
//! fixed sim-time rate instead of at solver points, which keeps its output
//! independent of engine step sizes.

use std::io::{self, Write};

use crate::signal::{volts_to_microvolts, NetValues, PinSignal};
use crate::time::Time;

/// Something that observes net values at exchange boundaries.
pub trait Probe {
    /// The next time (strictly after `now`) at which this probe needs a
    /// boundary, or `None` if it has no requirement.
    fn next_sample_time(&self, now: Time) -> Option<Time>;

    /// Called after every exchange (including the initial one at t=0) with the
    /// current net values. `prev` is the previous boundary (equal to `now` at
    /// the initial exchange).
    fn on_boundary(&mut self, prev: Time, now: Time, nets: &NetValues) -> io::Result<()>;

    /// Flush buffered output. Called by the owner when the run ends.
    fn finish(&mut self) -> io::Result<()>;
}

fn invalid(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg)
}

/// Format a voltage as volts with 6 decimals (1 uV resolution), via integer
/// microvolts so the text never depends on float formatting details.
pub fn format_volts(volts: f64) -> String {
    let uv = volts_to_microvolts(volts);
    let sign = if uv < 0 { "-" } else { "" };
    let a = uv.unsigned_abs();
    format!("{sign}{}.{:06}", a / 1_000_000, a % 1_000_000)
}

/// Fixed-rate CSV sampler.
///
/// Columns: `t_fs`, then one column per net in the order given. Digital values
/// are `0`/`1`, analog values are volts with 6 decimals, an undriven net is
/// empty. Rows are written at `0, period, 2*period, ...` up to the last
/// boundary.
pub struct CsvWriter<W: Write> {
    w: W,
    period: Time,
    nets: Vec<String>,
    next: Time,
    header_done: bool,
}

impl<W: Write> CsvWriter<W> {
    pub fn new(w: W, period: Time, nets: Vec<String>) -> io::Result<Self> {
        if period.is_zero() {
            return Err(invalid("CSV sample period must be > 0".into()));
        }
        Ok(CsvWriter {
            w,
            period,
            nets,
            next: Time::ZERO,
            header_done: false,
        })
    }

    pub fn into_inner(self) -> W {
        self.w
    }
}

impl<W: Write> Probe for CsvWriter<W> {
    fn next_sample_time(&self, now: Time) -> Option<Time> {
        (self.next > now).then_some(self.next)
    }

    fn on_boundary(&mut self, _prev: Time, now: Time, nets: &NetValues) -> io::Result<()> {
        if !self.header_done {
            let mut header = String::from("t_fs");
            for n in &self.nets {
                header.push(',');
                header.push_str(n);
            }
            header.push('\n');
            self.w.write_all(header.as_bytes())?;
            self.header_done = true;
        }
        // The scheduler aligns boundaries to sample times, so `next == now`
        // here; `while` keeps the writer correct (zero-order hold) if not.
        while self.next <= now {
            let mut row = self.next.as_fs().to_string();
            for n in &self.nets {
                row.push(',');
                match nets.get(n) {
                    Some(PinSignal::Digital { level, .. }) => {
                        row.push(char::from(b'0' + level.as_bit()));
                    }
                    Some(PinSignal::Analog { volts }) => row.push_str(&format_volts(*volts)),
                    None => {}
                }
            }
            row.push('\n');
            self.w.write_all(row.as_bytes())?;
            match self.next.checked_add(self.period) {
                Some(t) => self.next = t,
                None => {
                    self.next = Time::MAX;
                    break;
                }
            }
        }
        Ok(())
    }

    fn finish(&mut self) -> io::Result<()> {
        self.w.flush()
    }
}

/// VCD variable kind.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum VcdKind {
    /// 1-bit wire for a digital net.
    Wire,
    /// `real` for an analog net (volts).
    Real,
}

struct VcdVar {
    net: String,
    kind: VcdKind,
    id: String,
    last: Option<String>,
}

/// Value-change-dump writer (timescale 1 fs).
///
/// Digital changes are stamped at their `edge_time`; analog changes at the
/// boundary time, and only when the 1 uV-rounded value changes. The header
/// contains no date, so output is deterministic.
pub struct VcdWriter<W: Write> {
    w: W,
    vars: Vec<VcdVar>,
    header_done: bool,
    last_t: Option<Time>,
}

fn vcd_id(mut i: usize) -> String {
    // Printable ASCII '!'..='~' (94 symbols), little-endian base 94.
    let mut s = String::new();
    loop {
        s.push(char::from(b'!' + (i % 94) as u8));
        i /= 94;
        if i == 0 {
            break s;
        }
        i -= 1;
    }
}

impl<W: Write> VcdWriter<W> {
    pub fn new(w: W, nets: &[(&str, VcdKind)]) -> Self {
        let vars = nets
            .iter()
            .enumerate()
            .map(|(i, (net, kind))| VcdVar {
                net: (*net).to_string(),
                kind: *kind,
                id: vcd_id(i),
                last: None,
            })
            .collect();
        VcdWriter {
            w,
            vars,
            header_done: false,
            last_t: None,
        }
    }

    pub fn into_inner(self) -> W {
        self.w
    }

    fn write_header(&mut self) -> io::Result<()> {
        let mut h = String::new();
        h.push_str("$version breadboard bbcore $end\n");
        h.push_str("$timescale 1 fs $end\n");
        h.push_str("$scope module board $end\n");
        for v in &self.vars {
            let decl = match v.kind {
                VcdKind::Wire => "wire 1",
                VcdKind::Real => "real 64",
            };
            h.push_str(&format!("$var {decl} {} {} $end\n", v.id, v.net));
        }
        h.push_str("$upscope $end\n$enddefinitions $end\n");
        self.w.write_all(h.as_bytes())
    }
}

impl<W: Write> Probe for VcdWriter<W> {
    fn next_sample_time(&self, _now: Time) -> Option<Time> {
        None
    }

    fn on_boundary(&mut self, _prev: Time, now: Time, nets: &NetValues) -> io::Result<()> {
        if !self.header_done {
            self.write_header()?;
            self.header_done = true;
        }
        let mut changes: Vec<(Time, usize, String)> = Vec::new();
        for (i, v) in self.vars.iter().enumerate() {
            let Some(sig) = nets.get(&v.net) else {
                continue;
            };
            let (t, text) = match (v.kind, sig) {
                (VcdKind::Wire, PinSignal::Digital { level, edge_time }) => {
                    (*edge_time, format!("{}{}", level.as_bit(), v.id))
                }
                (VcdKind::Real, PinSignal::Analog { volts }) => {
                    (now, format!("r{} {}", format_volts(*volts), v.id))
                }
                (kind, sig) => {
                    return Err(invalid(format!(
                        "VCD net {} declared {kind:?} but got {sig:?}",
                        v.net
                    )))
                }
            };
            if v.last.as_deref() != Some(text.as_str()) {
                changes.push((t, i, text));
            }
        }
        // Stamps never go backwards: an initial value is written at the first
        // boundary, later edges lie in (prev, now].
        changes.sort_by_key(|c| (c.0, c.1));
        for (t, i, text) in changes {
            let t = self.last_t.map_or(t, |l| t.max(l));
            if self.last_t != Some(t) {
                writeln!(self.w, "#{}", t.as_fs())?;
                self.last_t = Some(t);
            }
            writeln!(self.w, "{text}")?;
            self.vars[i].last = Some(text);
        }
        Ok(())
    }

    fn finish(&mut self) -> io::Result<()> {
        self.w.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::Level;

    #[test]
    fn volts_format_is_fixed_point() {
        assert_eq!(format_volts(1.9), "1.900000");
        assert_eq!(format_volts(-0.0000004), "0.000000");
        assert_eq!(format_volts(-0.25), "-0.250000");
        assert_eq!(format_volts(3.3), "3.300000");
    }

    #[test]
    fn vcd_ids_unique() {
        let ids: std::collections::BTreeSet<_> = (0..10_000).map(vcd_id).collect();
        assert_eq!(ids.len(), 10_000);
    }

    #[test]
    fn csv_rows_at_fixed_rate() {
        let mut c =
            CsvWriter::new(Vec::new(), Time::from_us(10), vec!["A".into(), "D".into()]).unwrap();
        let mut nets = NetValues::new();
        nets.insert("A".into(), PinSignal::analog(0.5));
        nets.insert("D".into(), PinSignal::digital(Level::Low, Time::ZERO));
        c.on_boundary(Time::ZERO, Time::ZERO, &nets).unwrap();
        assert_eq!(c.next_sample_time(Time::ZERO), Some(Time::from_us(10)));
        nets.insert(
            "D".into(),
            PinSignal::digital(Level::High, Time::from_us(10)),
        );
        c.on_boundary(Time::ZERO, Time::from_us(10), &nets).unwrap();
        let s = String::from_utf8(c.into_inner()).unwrap();
        assert_eq!(s, "t_fs,A,D\n0,0.500000,0\n10000000000,0.500000,1\n");
    }
}
