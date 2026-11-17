//! `[[assert]]` (ADR-010 D5-D7, Amendment 3): reading the assertions of a
//! bench or of an assertion file (`bb run --assert`, `bb assert`), naming
//! their nets for the kernel, and the honesty rules of D6.
//!
//! The kinds and their semantics are `bbcore::assert`'s; this module reads
//! the TOML (`spec/project.md` section 3.4) into `bbcore::assert::Spec`
//! with the nets as written, and [`resolve`] maps them to kernel nets.

use std::collections::BTreeSet;
use std::path::Path;

use bbcore::assert::{
    AtWhat, AtWhen, BandEdge, EventWhat, Kind, Limits, Pair, Pattern, Sel, Signal, Spec, Unit,
    Window,
};
use bbcore::Time;
use bbpkg::obj::Obj;
use bbpkg::toml::{self, Value};
use bbpkg::value::{parse_quantity, Dim};

fn fs_time(fs: i128, at: &str) -> Result<Time, String> {
    u64::try_from(fs)
        .map(Time::from_fs)
        .map_err(|_| format!("{at}: a negative or too large time"))
}

fn time(text: &str, at: &str) -> Result<Time, String> {
    let fs = parse_quantity(text, Dim::Time).map_err(|e| format!("{at}: {e}"))?;
    fs_time(fs, at)
}

fn uv(text: &str, at: &str) -> Result<i64, String> {
    let v = parse_quantity(text, Dim::Voltage).map_err(|e| format!("{at}: {e}"))?;
    i64::try_from(v).map_err(|_| format!("{at}: too large"))
}

fn level(o: &mut Obj, key: &str, at: &str) -> Result<u8, String> {
    match o.get(key) {
        Some(Value::Integer(0)) => Ok(0),
        Some(Value::Integer(1)) => Ok(1),
        _ => Err(format!("{at}.{key}: 0 or 1 (an integer)")),
    }
}

/// `min` and `max` in `unit`: quantities (strings) for `Uv` and `Fs`,
/// integers for `Count`.
fn limits(o: &mut Obj, unit: Unit, at: &str) -> Result<Limits, String> {
    let mut one = |key: &str| -> Result<Option<i64>, String> {
        let p = format!("{at}.{key}");
        match (unit, o.get(key)) {
            (_, None) => Ok(None),
            (Unit::Count, Some(Value::Integer(n))) => Ok(Some(*n)),
            (Unit::Count, Some(_)) => Err(format!("{p}: an integer")),
            (Unit::Uv, Some(Value::String(s))) => uv(s, &p).map(Some),
            (Unit::Fs, Some(Value::String(s))) => {
                let v = parse_quantity(s, Dim::Time).map_err(|e| format!("{p}: {e}"))?;
                i64::try_from(v)
                    .map(Some)
                    .map_err(|_| format!("{p}: too large"))
            }
            (u, Some(_)) => Err(format!(
                "{p}: a quantity string (in {})",
                if u == Unit::Uv { "volts" } else { "seconds" }
            )),
        }
    };
    let l = Limits {
        min: one("min")?,
        max: one("max")?,
    };
    if l.min.is_none() && l.max.is_none() {
        return Err(format!("{at}: give min, max or both"));
    }
    if let (Some(a), Some(b)) = (l.min, l.max) {
        if a > b {
            return Err(format!("{at}: min is above max"));
        }
    }
    Ok(l)
}

fn signal(o: &mut Obj) -> Result<Signal, String> {
    Ok(Signal {
        net: o.str_req("net")?,
        minus: o.str_opt("ref")?,
    })
}

/// An event table: `{ edge = "<net>", level = 0|1 }`, `{ log = "<pattern>",
/// source = "<engine>" }`, or (where `cross` is allowed) `{ cross =
/// "<net>", ref = "<net>", below = "<V>" }` / `above`. `extra` are keys the
/// caller reads afterwards (a `delay`).
fn sel<'a>(mut t: Obj<'a>, cross: bool, extra: &[&str]) -> Result<(Sel, Obj<'a>), String> {
    let at = t.path.clone();
    let n = ["edge", "log", "cross"].iter().filter(|k| t.has(k)).count();
    if n != 1 {
        return Err(format!(
            "{at}: give exactly one of edge = \"<net>\", log = \"<pattern>\"{}",
            if cross { " or cross = \"<net>\"" } else { "" }
        ));
    }
    let s = if let Some(net) = t.str_opt("edge")? {
        let level = level(&mut t, "level", &at)?;
        Sel::Edge { net, level }
    } else if let Some(p) = t.str_opt("log")? {
        Sel::Log {
            pattern: Pattern::parse(&p).map_err(|e| format!("{at}.log: {e}"))?,
            source: t.str_opt("source")?,
        }
    } else {
        if !cross {
            return Err(format!(
                "{at}.cross: an analog crossing opens or closes no window (an edge or a log does)"
            ));
        }
        let net = t.str_opt("cross")?.expect("checked");
        let minus = t.str_opt("ref")?;
        let (below, thr) = match (t.str_opt("below")?, t.str_opt("above")?) {
            (Some(v), None) => (true, v),
            (None, Some(v)) => (false, v),
            _ => {
                return Err(format!(
                    "{at}: a crossing takes exactly one of below or above"
                ))
            }
        };
        Sel::Cross {
            sig: Signal { net, minus },
            below,
            threshold_uv: uv(
                &thr,
                &format!("{at}.{}", if below { "below" } else { "above" }),
            )?,
        }
    };
    let _ = extra;
    Ok((s, t))
}

fn window(o: &mut Obj, at: &str) -> Result<Window, String> {
    let mut w = Window::default();
    if let Some(f) = o.str_opt("from")? {
        w.from = time(&f, &format!("{at}.from"))?;
    }
    if let Some(t) = o.str_opt("to")? {
        w.to = Some(time(&t, &format!("{at}.to"))?);
    }
    if let Some(t) = o.table_opt("after")? {
        let (s, mut t) = sel(t, false, &["delay"])?;
        let delay = match t.str_opt("delay")? {
            Some(d) => time(&d, &format!("{at}.after.delay"))?,
            None => Time::ZERO,
        };
        t.done()?;
        w.after = Some((s, delay));
    }
    if let Some(t) = o.table_opt("until")? {
        let (s, t) = sel(t, false, &[])?;
        t.done()?;
        w.until = Some(s);
    }
    if let Some(mut t) = o.table_opt("while")? {
        let net = t.str_req("net")?;
        let lv = level(&mut t, "level", &format!("{at}.while"))?;
        t.done()?;
        w.while_level = Some((net, lv));
    }
    if let Some(to) = w.to {
        if to < w.from {
            return Err(format!("{at}: to is before from"));
        }
    }
    Ok(w)
}

/// A settle band edge: a voltage, or a percentage of the value at the event.
fn band(o: &mut Obj, key: &str, at: &str) -> Result<Option<BandEdge>, String> {
    let Some(s) = o.str_opt(key)? else {
        return Ok(None);
    };
    let p = format!("{at}.{key}");
    if s.trim_end().ends_with('%') {
        let ppm = parse_quantity(&s, Dim::Percent).map_err(|e| format!("{p}: {e}"))?;
        Ok(Some(BandEdge::Rel(
            i64::try_from(ppm).map_err(|_| format!("{p}: too large"))?,
        )))
    } else {
        Ok(Some(BandEdge::Abs(uv(&s, &p)?)))
    }
}

/// The `[[assert]]` tables of a TOML table (a bench, or an assertion file).
pub fn parse_tables(o: &mut Obj) -> Result<Vec<Spec>, String> {
    let mut out: Vec<Spec> = Vec::new();
    for mut a in o.tables("assert")? {
        let at = a.path.clone();
        let name = a.str_req("name")?;
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_.:-".contains(c))
        {
            return Err(format!(
                "{at}.name {name:?}: letters, digits and _ . : - only (a stable identifier)"
            ));
        }
        if out.iter().any(|s| s.name == name) {
            return Err(format!("{at}.name {name:?}: given twice"));
        }
        let kind_name = a.str_req("kind")?;
        let msg = a.str_opt("msg")?;
        let requires = a.str_opt("requires")?;
        let w = window(&mut a, &at)?;
        let kind = match kind_name.as_str() {
            "range" => {
                let sig = signal(&mut a)?;
                Kind::Range {
                    sig,
                    lim: limits(&mut a, Unit::Uv, &at)?,
                }
            }
            "at" => {
                let net = a.str_req("net")?;
                let minus = a.str_opt("ref")?;
                let when = match a.get("at") {
                    Some(Value::String(s)) => AtWhen::Time(time(s, &format!("{at}.at"))?),
                    Some(Value::Table(_)) => {
                        let mut t = a.table_opt("at")?.expect("a table");
                        let net = t.str_req("edge")?;
                        let level = level(&mut t, "level", &format!("{at}.at"))?;
                        t.done()?;
                        AtWhen::Edge { net, level }
                    }
                    _ => {
                        return Err(format!(
                            "{at}.at: a time (\"50ms\") or {{ edge = \"<net>\", level = 0|1 }}"
                        ))
                    }
                };
                // A digital net's level is read with integer limits, an
                // analog value with voltages.
                let digital = matches!(a.get("min").or(a.get("max")), Some(Value::Integer(_)));
                let (what, unit) = if digital {
                    if minus.is_some() {
                        return Err(format!("{at}.ref: a level has no difference"));
                    }
                    (AtWhat::Digital(net), Unit::Count)
                } else {
                    (AtWhat::Analog(Signal { net, minus }), Unit::Uv)
                };
                Kind::At {
                    what,
                    when,
                    lim: limits(&mut a, unit, &at)?,
                }
            }
            "settle" => {
                let sig = signal(&mut a)?;
                let mut e = a.table_opt("event")?.ok_or_else(|| {
                    format!("{at}.event: required ({{ edge = \"<net>\", level }})")
                })?;
                let net = e.str_req("edge")?;
                let lv = level(&mut e, "level", &format!("{at}.event"))?;
                e.done()?;
                let within = time(&a.str_req("within")?, &format!("{at}.within"))?;
                let lo = band(&mut a, "min", &at)?;
                let hi = band(&mut a, "max", &at)?;
                if lo.is_none() && hi.is_none() {
                    return Err(format!("{at}: a band needs min, max or both"));
                }
                Kind::Settle {
                    sig,
                    net,
                    level: lv,
                    within,
                    lo,
                    hi,
                }
            }
            "edge_timing" => {
                let start = a
                    .table_opt("start")?
                    .ok_or_else(|| format!("{at}.start: required"))?;
                let (start, t) = sel(start, true, &[])?;
                t.done()?;
                let end = a
                    .table_opt("end")?
                    .ok_or_else(|| format!("{at}.end: required"))?;
                let (end, t) = sel(end, true, &[])?;
                t.done()?;
                let pair = match a.str_opt("pair")?.as_deref() {
                    None | Some("last") => Pair::Last,
                    Some("first") => Pair::First,
                    Some(p) => return Err(format!("{at}.pair {p:?}: last or first")),
                };
                Kind::EdgeTiming {
                    start,
                    end,
                    pair,
                    lim: limits(&mut a, Unit::Fs, &at)?,
                }
            }
            "count" => {
                let net = a.str_req("net")?;
                let lv = if a.has("level") {
                    Some(level(&mut a, "level", &at)?)
                } else {
                    None
                };
                Kind::Count {
                    net,
                    level: lv,
                    lim: limits(&mut a, Unit::Count, &at)?,
                }
            }
            "frequency" => {
                let net = a.str_req("net")?;
                let lv = if a.has("level") {
                    level(&mut a, "level", &at)?
                } else {
                    1
                };
                let lim = match (a.str_opt("period")?, a.str_opt("tol")?) {
                    (Some(p), Some(tol)) => {
                        if a.has("min") || a.has("max") {
                            return Err(format!("{at}: period and tol, or min and max"));
                        }
                        let p = parse_quantity(&p, Dim::Time)
                            .map_err(|e| format!("{at}.period: {e}"))?;
                        let ppm = parse_quantity(&tol, Dim::Percent)
                            .map_err(|e| format!("{at}.tol: {e}"))?;
                        // The band, rounded outwards to whole fs.
                        let d = p * ppm;
                        let lo = (p * 1_000_000 - d).div_euclid(1_000_000);
                        let hi = -((-(p * 1_000_000 + d)).div_euclid(1_000_000));
                        Limits {
                            min: Some(i64::try_from(lo).map_err(|_| format!("{at}: too large"))?),
                            max: Some(i64::try_from(hi).map_err(|_| format!("{at}: too large"))?),
                        }
                    }
                    (None, None) => limits(&mut a, Unit::Fs, &at)?,
                    _ => return Err(format!("{at}: period needs tol")),
                };
                Kind::Frequency {
                    net,
                    level: lv,
                    lim,
                }
            }
            "log" => {
                let pattern =
                    Pattern::parse(&a.str_req("match")?).map_err(|e| format!("{at}.match: {e}"))?;
                let source = a.str_opt("source")?;
                let field = a.str_opt("field")?;
                let present = a.bool_opt("present")?;
                match (field, present) {
                    (Some(f), None) => {
                        // The limits' type gives the field's unit.
                        let probe = a.get("min").or(a.get("max")).cloned();
                        let unit = match probe {
                            Some(Value::Integer(_)) => Unit::Count,
                            Some(Value::String(s)) => {
                                if parse_quantity(&s, Dim::Time).is_ok() && !s.trim().ends_with('V')
                                {
                                    Unit::Fs
                                } else {
                                    Unit::Uv
                                }
                            }
                            _ => return Err(format!("{at}: a field needs min, max or both")),
                        };
                        Kind::Log {
                            pattern,
                            source,
                            field: Some((f, unit)),
                            lim: limits(&mut a, unit, &at)?,
                        }
                    }
                    (Some(_), Some(_)) => return Err(format!("{at}: field or present, not both")),
                    (None, Some(p)) => {
                        if a.has("min") || a.has("max") {
                            return Err(format!("{at}: present, or min and max counts, not both"));
                        }
                        Kind::Log {
                            pattern,
                            source,
                            field: None,
                            lim: if p {
                                Limits {
                                    min: Some(1),
                                    max: None,
                                }
                            } else {
                                Limits {
                                    min: None,
                                    max: Some(0),
                                }
                            },
                        }
                    }
                    (None, None) => Kind::Log {
                        pattern,
                        source,
                        field: None,
                        lim: limits(&mut a, Unit::Count, &at)?,
                    },
                }
            }
            "event" => {
                let ev = a.str_req("event")?;
                // `phase` (ADR-009 Amendment 12, decision D-close 1): count
                // only the `reset` records of one phase; `enter` counts
                // brown-outs (each writes `enter`, then `exit`).
                let phase = a.str_opt("phase")?;
                let what = match (ev.as_str(), phase) {
                    ("reset", None) => EventWhat::Reset,
                    ("reset", Some(p)) => EventWhat::ResetPhase(
                        bbcore::event::ResetPhase::parse(&p)
                            .ok_or_else(|| format!("{at}.phase {p:?}: enter, exit or stop"))?,
                    ),
                    (_, Some(_)) => return Err(format!("{at}.phase: only with event = \"reset\"")),
                    ("warn", None) => EventWhat::Warn,
                    ("error", None) => EventWhat::Error,
                    (e, None) => return Err(format!("{at}.event {e:?}: reset, warn or error")),
                };
                Kind::Event {
                    what,
                    lim: limits(&mut a, Unit::Count, &at)?,
                }
            }
            k => {
                return Err(format!(
                "{at}.kind {k:?}: one of range, at, settle, edge_timing, count, frequency, log, \
                     event (the closed set of ADR-010 D5)"
            ))
            }
        };
        a.done()?;
        // `limit:` names Breadboard's own package-limit assertions (ADR-009
        // D3); a file cannot take one.
        if name.starts_with("limit:") {
            return Err(format!(
                "{at}.name {name:?}: limit:<ref>.<pin>:<key> names the automatic package-limit \
                 assertions (ADR-009 D3); choose another name"
            ));
        }
        out.push(Spec {
            name,
            kind,
            window: w,
            requires,
            msg,
            not_evaluable: None,
        });
    }
    Ok(out)
}

/// An assertion file (`--assert`): a `bb-bench/1` file of `[[assert]]`
/// tables and nothing else (no stimulus: a run's stimulus is its bench's or
/// its board file's).
pub fn read_file(path: &Path) -> Result<(Vec<Spec>, String), String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let specs = parse_file(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok((specs, text))
}

pub fn parse_file(text: &str) -> Result<Vec<Spec>, String> {
    let t = toml::parse(text).map_err(|e| e.to_string())?;
    let mut o = Obj::new("", &t);
    let schema = o.str_req("schema")?;
    let id = bbpkg::schema::SchemaId::parse(&schema)?;
    if id.name != "bb-bench" || id.major < 1 {
        return Err(format!(
            "schema {schema:?}: an assertion file is a bb-bench/1 file of [[assert]] tables"
        ));
    }
    bbpkg::schema::Registry::current().check("bb-bench", &schema)?;
    for k in o.keys() {
        if k != "schema" && k != "assert" {
            return Err(format!(
                "{k}: an assertion file holds [[assert]] tables only (the stimulus is the run's \
                 bench or board file)"
            ));
        }
    }
    let specs = parse_tables(&mut o)?;
    o.done()?;
    if specs.is_empty() {
        return Err("no [[assert]] table".into());
    }
    Ok(specs)
}

/// How a net a check reads is used.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Use {
    Analog,
    Digital,
}

/// Every net of a spec with its use, mutably (for [`resolve`]).
fn nets_mut(s: &mut Spec) -> Vec<(&mut String, Use)> {
    let mut out: Vec<(&mut String, Use)> = Vec::new();
    fn sig(s: &mut Signal) -> Vec<(&mut String, Use)> {
        let mut v = vec![(&mut s.net, Use::Analog)];
        if let Some(m) = s.minus.as_mut() {
            v.push((m, Use::Analog));
        }
        v
    }
    fn selm(s: &mut Sel) -> Vec<(&mut String, Use)> {
        match s {
            Sel::Edge { net, .. } => vec![(net, Use::Digital)],
            Sel::Log { .. } => vec![],
            Sel::Cross { sig: g, .. } => sig(g),
        }
    }
    if let Some((s, _)) = s.window.after.as_mut() {
        out.extend(selm(s));
    }
    if let Some(s) = s.window.until.as_mut() {
        out.extend(selm(s));
    }
    if let Some((n, _)) = s.window.while_level.as_mut() {
        out.push((n, Use::Digital));
    }
    match &mut s.kind {
        Kind::Range { sig: g, .. } => out.extend(sig(g)),
        Kind::At { what, when, .. } => {
            match what {
                AtWhat::Analog(g) => out.extend(sig(g)),
                AtWhat::Digital(n) => out.push((n, Use::Digital)),
            }
            if let AtWhen::Edge { net, .. } = when {
                out.push((net, Use::Digital));
            }
        }
        Kind::Settle { sig: g, net, .. } => {
            out.extend(sig(g));
            out.push((net, Use::Digital));
        }
        Kind::EdgeTiming { start, end, .. } => {
            out.extend(selm(start));
            out.extend(selm(end));
        }
        Kind::Count { net, .. } | Kind::Frequency { net, .. } => out.push((net, Use::Digital)),
        Kind::Log { .. } | Kind::Event { .. } => {}
    }
    out
}

/// The analog nets assertions read: `(kernel net, circuit node)`.
pub type AnalogNodes = Vec<(String, String)>;

/// The nets of `specs` as the kernel names them: `f(name, use)` gives the
/// kernel net (and, for an analog net, its circuit node). Returns the specs
/// and the analog nets `(kernel net, node)` they read, in first-use order.
/// A current through a 0 V source is refused (ADR-010 D6).
pub fn resolve(
    mut specs: Vec<Spec>,
    mut f: impl FnMut(&str, Use) -> Result<(String, Option<String>), String>,
) -> Result<(Vec<Spec>, AnalogNodes), String> {
    let mut analog: AnalogNodes = Vec::new();
    for s in &mut specs {
        let name = s.name.clone();
        for (n, u) in nets_mut(s) {
            let l = n.to_ascii_lowercase();
            if l.starts_with("i(") || l.contains("#branch") {
                return Err(format!(
                    "assertion {name}: {n} is a branch current; a current is computable only from \
                     node voltages (Norton sources, B-source views), and an assertion on a current \
                     through a 0 V source is refused (ADR-010 D6)"
                ));
            }
            let (k, node) = f(n, u).map_err(|e| format!("assertion {name}: {n}: {e}"))?;
            if u == Use::Analog {
                let node = node.unwrap_or_else(|| k.clone());
                if !analog.iter().any(|(x, _)| *x == k) {
                    analog.push((k.clone(), node));
                }
            }
            *n = k;
        }
    }
    Ok((specs, analog))
}

/// D6's tautology rule: no check reads a net an ideal bench source forces
/// (`forced`: kernel nets, with the source that forces each).
pub fn check_forced(specs: &[Spec], forced: &[(String, String)]) -> Result<(), String> {
    for s in specs {
        let nets: BTreeSet<String> = s.analog_nets().into_iter().collect();
        for (n, src) in forced {
            if nets.contains(n) {
                return Err(format!(
                    "assertion {}: {n} is forced by the bench's ideal {src}; a check on it checks the \
                     source, not the circuit (a tautology, refused: ADR-010 D6). Give the source an \
                     rs, or check another net",
                    s.name
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(t: &str) -> Result<Vec<Spec>, String> {
        parse_file(&format!("schema = \"bb-bench/1\"\n{t}"))
    }

    #[test]
    fn every_kind_parses() {
        let s = parse(
            r#"
[[assert]]
name = "LSW-3"
kind = "range"
net = "LED_DIN"
ref = "VLED"
max = "0.7V"
[[assert]]
name = "V4"
kind = "at"
net = "VLED"
at = { edge = "U10.P0.06", level = 1 }
min = "2.67V"
[[assert]]
name = "LVL"
kind = "at"
net = "U10.P1.04"
at = { edge = "U10.P0.06", level = 1 }
min = 1
[[assert]]
name = "LSW-2"
kind = "settle"
net = "VLED"
event = { edge = "U10.P1.04", level = 0 }
within = "30ms"
max = "10%"
[[assert]]
name = "FW"
kind = "edge_timing"
start = { edge = "U10.P1.04", level = 1 }
end = { edge = "U10.P0.06", level = 1 }
min = "10ms"
[[assert]]
name = "SHED"
kind = "edge_timing"
start = { cross = "VSYS_SENSE", below = "1.127V" }
end = { edge = "U10.P1.04", level = 0 }
pair = "first"
max = "1s"
[[assert]]
name = "N"
kind = "count"
net = "U10.P1.04"
level = 1
min = 2
max = 2
[[assert]]
name = "F"
kind = "frequency"
net = "PA5"
period = "1s"
tol = "2%"
[[assert]]
name = "CODE"
kind = "log"
match = "^vsense: n=3 "
field = "code"
min = 1067
max = 1067
[[assert]]
name = "TS"
kind = "log"
match = "^M1: DRV5032 tS"
source = "devices"
field = "tS"
min = "50ms"
max = "50ms"
[[assert]]
name = "UP"
kind = "log"
match = "boot"
present = true
from = "1ms"
after = { log = "^homing: H1 skipped", delay = "0s" }
until = { edge = "M1.HOME_N", level = 1 }
while = { net = "U10.P1.04", level = 1 }
[[assert]]
name = "R"
kind = "event"
event = "reset"
min = 2
"#,
        )
        .unwrap();
        assert_eq!(s.len(), 12);
        assert!(matches!(
            &s[3].kind,
            Kind::Settle {
                hi: Some(BandEdge::Rel(100_000)),
                ..
            }
        ));
        match &s[7].kind {
            Kind::Frequency { lim, .. } => {
                assert_eq!(lim.min, Some(980_000_000_000_000));
                assert_eq!(lim.max, Some(1_020_000_000_000_000));
            }
            _ => panic!(),
        }
        assert!(
            matches!(&s[9].kind, Kind::Log { field: Some((_, Unit::Fs)), lim, .. } if lim.min == Some(50_000_000_000_000))
        );
        assert!(matches!(
            &s[2].kind,
            Kind::At {
                what: AtWhat::Digital(_),
                ..
            }
        ));
        assert_eq!(s[10].window.from, Time::from_ms(1));
    }

    #[test]
    fn mistakes_are_refused() {
        for (t, want) in [
            ("[[assert]]\nname = \"a\"\nkind = \"maximum\"\nnet = \"X\"\nmax = \"1V\"", "the closed set"),
            ("[[assert]]\nname = \"a\"\nkind = \"range\"\nnet = \"X\"", "min, max or both"),
            ("[[assert]]\nname = \"a b\"\nkind = \"range\"\nnet = \"X\"\nmax = \"1V\"", "stable identifier"),
            ("[[assert]]\nname = \"a\"\nkind = \"log\"\nmatch = \"x.*y\"\npresent = true", "metacharacter"),
            ("[[assert]]\nname = \"a\"\nkind = \"range\"\nnet = \"X\"\nmax = \"1V\"\nwhat = 1", "what"),
            ("[[assert]]\nname = \"a\"\nkind = \"range\"\nnet = \"X\"\nmax = \"1V\"\n[[assert]]\nname = \"a\"\nkind = \"range\"\nnet = \"X\"\nmax = \"1V\"", "given twice"),
            ("[[supply]]\nnet = \"X\"\nvolts = \"1V\"", "[[assert]] tables only"),
        ] {
            let e = parse(t).unwrap_err();
            assert!(e.contains(want), "{t}: {e}");
        }
    }

    #[test]
    fn honesty_refusals() {
        // ADR-010 D6 (G6): a current through a 0 V source, and a net an ideal
        // bench source forces.
        let s = parse(
            "[[assert]]\nname = \"I\"\nkind = \"range\"\nnet = \"i(V_bench_VSYS)\"\nmax = \"1V\"",
        )
        .unwrap();
        let e = resolve(s, |n, _| Ok((n.to_string(), None))).unwrap_err();
        assert!(
            e.contains("a current through a 0 V source is refused"),
            "{e}"
        );
        let s = parse("[[assert]]\nname = \"T\"\nkind = \"range\"\nnet = \"VSYS\"\nmin = \"2V\"")
            .unwrap();
        let (s, an) = resolve(s, |n, _| Ok((n.to_string(), Some(n.to_lowercase())))).unwrap();
        assert_eq!(an, vec![("VSYS".to_string(), "vsys".to_string())]);
        let e =
            check_forced(&s, &[("VSYS".into(), "[[supply]] VSYS (rs = 0)".into())]).unwrap_err();
        assert!(e.contains("a tautology, refused"), "{e}");
        assert!(check_forced(&s, &[("+3V3".into(), "x".into())]).is_ok());
    }
}
