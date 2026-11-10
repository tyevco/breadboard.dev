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

/// Net values at instants inside the last grant window `[prev, now]`,
/// provided by the scheduler (see [`crate::Engine::sample_output`]).
pub trait NetSampler {
    /// Value of `net` at `t` (`prev <= t <= now`), or `None` if the net has no
    /// value. Deterministic.
    fn sample(&self, net: &str, t: Time) -> Option<PinSignal>;

    /// Every level change of the digital net `net` inside the window
    /// `(prev, now]`, in time order (each `edge_time` is its time). With
    /// traces (ADR-003 D3) a net can change any number of times per window.
    /// Default: none known (a probe then falls back to the boundary value).
    fn digital_changes(&self, _net: &str) -> Vec<PinSignal> {
        Vec::new()
    }
}

/// Something that observes net values at exchange boundaries.
pub trait Probe {
    /// The next time (strictly after `now`) at which this probe needs a
    /// boundary, or `None` if it has no requirement. A requirement changes
    /// the grant partition, which can change what engines compute (e.g. when
    /// a Renode ISR runs; ADR-002 Amendment 3), so outputs that should not
    /// perturb the simulation return `None` and sample through
    /// [`Probe::on_window`] instead.
    fn next_sample_time(&self, now: Time) -> Option<Time>;

    /// Called after every exchange (including the initial one at t=0) with the
    /// current net values. `prev` is the previous boundary (equal to `now` at
    /// the initial exchange).
    fn on_boundary(&mut self, prev: Time, now: Time, nets: &NetValues) -> io::Result<()>;

    /// What the scheduler actually calls after every exchange: like
    /// [`Probe::on_boundary`], plus a sampler for instants in `[prev, now]`.
    /// Default: `on_boundary`.
    fn on_window(
        &mut self,
        prev: Time,
        now: Time,
        nets: &NetValues,
        _sampler: &dyn NetSampler,
    ) -> io::Result<()> {
        self.on_boundary(prev, now, nets)
    }

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
///
/// Two modes:
/// - [`CsvWriter::new`] (aligned): sample instants are forced to be grant
///   boundaries and rows hold the boundary values. Changing the period
///   changes the grant partition.
/// - [`CsvWriter::sampled`]: sample instants never constrain grants; each row
///   is taken from the [`NetSampler`] inside the grant window that contains
///   it (engine interpolation, digital levels from `edge_time`). Changing the
///   period changes only this file.
pub struct CsvWriter<W: Write> {
    w: W,
    period: Time,
    nets: Vec<String>,
    next: Time,
    header_done: bool,
    sampled: bool,
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
            sampled: false,
        })
    }

    /// A writer that samples inside grant windows instead of forcing
    /// boundaries (see the type docs).
    pub fn sampled(w: W, period: Time, nets: Vec<String>) -> io::Result<Self> {
        let mut c = CsvWriter::new(w, period, nets)?;
        c.sampled = true;
        Ok(c)
    }

    pub fn into_inner(self) -> W {
        self.w
    }

    fn write_rows(
        &mut self,
        now: Time,
        value: &dyn Fn(&str, Time) -> Option<PinSignal>,
    ) -> io::Result<()> {
        if !self.header_done {
            let mut header = String::from("t_fs");
            for n in &self.nets {
                header.push(',');
                header.push_str(&csv_field(n));
            }
            header.push('\n');
            self.w.write_all(header.as_bytes())?;
            self.header_done = true;
        }
        while self.next <= now {
            let mut row = self.next.as_fs().to_string();
            for n in &self.nets {
                row.push(',');
                match value(n, self.next) {
                    Some(PinSignal::Digital { level, .. }) => {
                        row.push(char::from(b'0' + level.as_bit()));
                    }
                    Some(PinSignal::Analog { volts }) => row.push_str(&format_volts(volts)),
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
}

impl<W: Write> Probe for CsvWriter<W> {
    fn next_sample_time(&self, now: Time) -> Option<Time> {
        (!self.sampled && self.next > now).then_some(self.next)
    }

    fn on_window(
        &mut self,
        prev: Time,
        now: Time,
        nets: &NetValues,
        sampler: &dyn NetSampler,
    ) -> io::Result<()> {
        if !self.sampled {
            return self.on_boundary(prev, now, nets);
        }
        // Rows due in [prev, now] lie inside this window (earlier ones were
        // written at earlier boundaries).
        self.write_rows(now, &|n, t| sampler.sample(n, t))
    }

    fn on_boundary(&mut self, _prev: Time, now: Time, nets: &NetValues) -> io::Result<()> {
        // Aligned mode: the scheduler aligns boundaries to sample times, so
        // `next == now` here; rows before `now` (zero-order hold) only occur
        // when the writer is driven by hand.
        self.write_rows(now, &|n, _| nets.get(n).copied())
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
    /// The `$var` reference name (the net name unless mapped).
    reference: String,
    kind: VcdKind,
    id: String,
    last: Option<String>,
}

/// A `nets.csv` header field: RFC 4180 quoting when the name holds a
/// comma, a double quote or a line break (ADR-007 D6); other names as they
/// are, so every Phase 0 header is unchanged.
pub fn csv_field(name: &str) -> String {
    if name.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", name.replace('"', "\"\""))
    } else {
        name.to_string()
    }
}

/// Value-change-dump writer (timescale 1 fs).
///
/// Digital changes are stamped at their `edge_time`. Analog changes are
/// written only when the 1 uV-rounded value changes, stamped at:
/// - [`VcdWriter::new`] (boundary mode): the boundary time. The file then
///   depends on the grant partition.
/// - [`VcdWriter::sampled`]: fixed sample instants `0, period, 2*period,
///   ...`, read through the [`NetSampler`], and every digital change of a
///   window comes from it too (any number per window, ADR-003 D3). The file
///   then does not depend on the grant partition.
///
/// The header contains no date, so output is deterministic.
pub struct VcdWriter<W: Write> {
    w: W,
    vars: Vec<VcdVar>,
    header_done: bool,
    last_t: Option<Time>,
    /// `Some(period)`: sampled mode.
    period: Option<Time>,
    /// Next analog sample instant (sampled mode).
    next: Time,
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
                reference: (*net).to_string(),
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
            period: None,
            next: Time::ZERO,
        }
    }

    /// A writer that stamps analog values at fixed sample instants and takes
    /// digital changes from the sampler (see the type docs). It never asks
    /// for boundaries.
    pub fn sampled(w: W, nets: &[(&str, VcdKind)], period: Time) -> io::Result<Self> {
        if period.is_zero() {
            return Err(invalid("VCD sample period must be > 0".into()));
        }
        let mut v = VcdWriter::new(w, nets);
        v.period = Some(period);
        Ok(v)
    }

    pub fn into_inner(self) -> W {
        self.w
    }

    /// Give the variables reference names other than their net names, in
    /// `nets` order (ADR-007 D6, `spec/event-stream.md`: a project's nets
    /// are referenced by their SPICE names, since a display name may hold
    /// spaces). When any differs, the header carries a `$comment` block
    /// that maps each such reference to its net name, sorted.
    pub fn with_references(mut self, refs: &[String]) -> io::Result<Self> {
        if refs.len() != self.vars.len() {
            return Err(invalid("VCD references: one per variable".into()));
        }
        for (v, r) in self.vars.iter_mut().zip(refs) {
            if r.is_empty() || r.chars().any(|c| !c.is_ascii_graphic()) {
                return Err(invalid(format!(
                    "VCD reference {r:?}: printable ASCII, no spaces"
                )));
            }
            v.reference = r.clone();
        }
        Ok(self)
    }

    /// Write `changes` (time, var index, text) in time order; stamps never
    /// go backwards.
    fn write_changes(&mut self, mut changes: Vec<(Time, usize, String)>) -> io::Result<()> {
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

    fn write_header(&mut self) -> io::Result<()> {
        let mut h = String::new();
        h.push_str("$version breadboard bbcore $end\n");
        h.push_str("$timescale 1 fs $end\n");
        let mut mapped: Vec<(&str, &str)> = self
            .vars
            .iter()
            .filter(|v| v.reference != v.net)
            .map(|v| (v.reference.as_str(), v.net.as_str()))
            .collect();
        if !mapped.is_empty() {
            mapped.sort();
            h.push_str("$comment\n");
            for (r, n) in mapped {
                h.push_str(&format!("  {r} = {n}\n"));
            }
            h.push_str("$end\n");
        }
        h.push_str("$scope module board $end\n");
        for v in &self.vars {
            let decl = match v.kind {
                VcdKind::Wire => "wire 1",
                VcdKind::Real => "real 64",
            };
            h.push_str(&format!("$var {decl} {} {} $end\n", v.id, v.reference));
        }
        h.push_str("$upscope $end\n$enddefinitions $end\n");
        self.w.write_all(h.as_bytes())
    }
}

fn wire_text(v: &VcdVar, sig: &PinSignal) -> io::Result<(Time, String)> {
    match sig {
        PinSignal::Digital { level, edge_time } => {
            Ok((*edge_time, format!("{}{}", level.as_bit(), v.id)))
        }
        other => Err(invalid(format!(
            "VCD net {} declared Wire but got {other:?}",
            v.net
        ))),
    }
}

fn real_text(v: &VcdVar, sig: &PinSignal) -> io::Result<String> {
    match sig {
        PinSignal::Analog { volts } => Ok(format!("r{} {}", format_volts(*volts), v.id)),
        other => Err(invalid(format!(
            "VCD net {} declared Real but got {other:?}",
            v.net
        ))),
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
            let (t, text) = match v.kind {
                VcdKind::Wire => wire_text(v, sig)?,
                VcdKind::Real => (now, real_text(v, sig)?),
            };
            if v.last.as_deref() != Some(text.as_str()) {
                changes.push((t, i, text));
            }
        }
        // Stamps never go backwards: an initial value is written at the first
        // boundary, later edges lie in (prev, now].
        self.write_changes(changes)
    }

    fn on_window(
        &mut self,
        prev: Time,
        now: Time,
        nets: &NetValues,
        sampler: &dyn NetSampler,
    ) -> io::Result<()> {
        let Some(period) = self.period else {
            return self.on_boundary(prev, now, nets);
        };
        if !self.header_done {
            self.write_header()?;
            self.header_done = true;
        }
        // Analog sample instants due in [prev, now] (earlier ones were
        // written at earlier boundaries).
        let mut instants = Vec::new();
        while self.next <= now {
            instants.push(self.next);
            match self.next.checked_add(period) {
                Some(t) => self.next = t,
                None => {
                    self.next = Time::MAX;
                    break;
                }
            }
        }
        let mut changes: Vec<(Time, usize, String)> = Vec::new();
        for (i, v) in self.vars.iter().enumerate() {
            let mut last = v.last.clone();
            let mut push = |t: Time, text: String| {
                if last.as_deref() != Some(text.as_str()) {
                    last = Some(text.clone());
                    changes.push((t, i, text));
                }
            };
            match v.kind {
                VcdKind::Wire => {
                    let ch = sampler.digital_changes(&v.net);
                    if ch.is_empty() {
                        // No change in the window: the boundary value (an
                        // initial value, or a net first driven now).
                        if let Some(sig) = nets.get(&v.net) {
                            let (t, text) = wire_text(v, sig)?;
                            push(t, text);
                        }
                    }
                    for sig in &ch {
                        let (t, text) = wire_text(v, sig)?;
                        push(t, text);
                    }
                }
                VcdKind::Real => {
                    for &s in &instants {
                        if let Some(sig) = sampler.sample(&v.net, s) {
                            push(s, real_text(v, &sig)?);
                        }
                    }
                }
            }
        }
        self.write_changes(changes)
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
    fn csv_header_fields_quote_as_rfc4180() {
        // ADR-007 D6: names with a comma or a quote are quoted; Phase 0
        // names are unchanged.
        assert_eq!(csv_field("LED_A"), "LED_A");
        assert_eq!(csv_field("U1.PA5"), "U1.PA5");
        assert_eq!(csv_field("/LED ring/LED_DIN"), "/LED ring/LED_DIN");
        assert_eq!(
            csv_field("/MCU, flash & debug/XL1"),
            "\"/MCU, flash & debug/XL1\""
        );
        assert_eq!(csv_field("a\"b"), "\"a\"\"b\"");
        let mut c = CsvWriter::new(
            Vec::new(),
            Time::from_us(10),
            vec!["/MCU, flash/X".into(), "A".into()],
        )
        .unwrap();
        c.on_boundary(Time::ZERO, Time::ZERO, &NetValues::new())
            .unwrap();
        let s = String::from_utf8(c.into_inner()).unwrap();
        assert!(s.starts_with("t_fs,\"/MCU, flash/X\",A\n"), "{s}");
    }

    #[test]
    fn vcd_references_and_their_mapping_comment() {
        // Phase 0: no references given, no comment (unchanged header).
        let mut plain = VcdWriter::new(Vec::new(), &[("A", VcdKind::Real)]);
        plain.write_header().unwrap();
        let p = String::from_utf8(plain.into_inner()).unwrap();
        assert!(
            !p.contains("$comment") && p.contains("$var real 64 ! A $end"),
            "{p}"
        );
        // A project: SPICE names as references, a sorted mapping comment.
        let mut v = VcdWriter::new(
            Vec::new(),
            &[
                ("/LED ring/LED_DIN", VcdKind::Real),
                ("U1.PA4", VcdKind::Wire),
                ("+3V3", VcdKind::Real),
            ],
        )
        .with_references(&["LED_ring_LED_DIN".into(), "U1.PA4".into(), "n3V3".into()])
        .unwrap();
        v.write_header().unwrap();
        let h = String::from_utf8(v.into_inner()).unwrap();
        assert!(
            h.contains("$timescale 1 fs $end\n$comment\n  LED_ring_LED_DIN = /LED ring/LED_DIN\n  n3V3 = +3V3\n$end\n"),
            "{h}"
        );
        assert!(
            h.contains("$var real 64 ! LED_ring_LED_DIN $end")
                && h.contains("$var wire 1 \" U1.PA4 $end")
        );
        assert!(VcdWriter::new(Vec::new(), &[("A", VcdKind::Real)])
            .with_references(&["a b".into()])
            .is_err());
    }

    #[test]
    fn vcd_ids_unique() {
        let ids: std::collections::BTreeSet<_> = (0..10_000).map(vcd_id).collect();
        assert_eq!(ids.len(), 10_000);
    }

    struct Ramp;
    impl NetSampler for Ramp {
        fn sample(&self, net: &str, t: Time) -> Option<PinSignal> {
            // A = t in ms as volts; D high from 15 us on.
            match net {
                "A" => Some(PinSignal::analog(t.as_fs() as f64 / 1e12)),
                "D" if t >= Time::from_us(15) => {
                    Some(PinSignal::digital(Level::High, Time::from_us(15)))
                }
                "D" => Some(PinSignal::digital(Level::Low, Time::ZERO)),
                _ => None,
            }
        }
    }

    #[test]
    fn sampled_csv_does_not_ask_for_boundaries_and_samples_inside_windows() {
        let mut c = CsvWriter::sampled(Vec::new(), Time::from_us(10), vec!["A".into(), "D".into()])
            .unwrap();
        let nets = NetValues::new();
        assert_eq!(c.next_sample_time(Time::ZERO), None);
        c.on_window(Time::ZERO, Time::ZERO, &nets, &Ramp).unwrap();
        // One 25 us window: rows at 10 and 20 us come from the sampler.
        c.on_window(Time::ZERO, Time::from_us(25), &nets, &Ramp)
            .unwrap();
        assert_eq!(c.next_sample_time(Time::from_us(25)), None);
        let s = String::from_utf8(c.into_inner()).unwrap();
        assert_eq!(
            s,
            "t_fs,A,D\n0,0.000000,0\n10000000000,0.010000,0\n20000000000,0.020000,1\n"
        );
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
