//! Board files for `bb run`: a SPICE circuit body plus `*bb` directives.
//!
//! Format (see `examples/spike/board.cir`): every line that starts with `*bb`
//! is a directive, `*bb <keyword> <name> key=value ...`, split on whitespace.
//! Every other line is SPICE and goes to the analog engine unchanged (the
//! directives are SPICE comments, so ngspice ignores them too).
//!
//! Directives:
//! - `*bb part <ref> engine=renode platform=stm32f103`
//! - `*bb pin <ref>.<pin> net=<net> model=digital_out voh=<V> vol=<V> rise=<t> fall=<t> zout=<ohm>`:
//!   an MCU output driving circuit node `<net>` through generated glue.
//! - `*bb pin <ref>.<pin> model=digital_out` (no `net=`, no electrical keys):
//!   an MCU output not connected to the circuit. Its kernel net
//!   `<ref>.<pin>` can still be probed; nothing loads it.
//! - `*bb pin <ref>.<pin> net=<net> model=digital_in vih=<V> vil=<V> cin=<F>`:
//!   an MCU input reading circuit node `<net>` through the `digital_in` model
//!   (hysteresis, `vil < vih`; `cin` in farads, e.g. `5e-12`). The analog
//!   engine thresholds the node and adds `cin` as glue (ADR-003 D3).
//! - `*bb pin <ref>.<pin> net=<net> model=adc_in vref=<V> bits=<n>`: an
//!   MCU ADC input reading circuit node `<net>` (ADR-003 D6). The analog
//!   engine pushes the node's value at lattice points; the ADC channel
//!   follows from the pin ([`adc_channel`]). Only on platforms with an ADC
//!   model (`stm32f072`).
//!
//! Platforms: `stm32f103` (Renode's STM32F103 without the SVD download, the
//! Phase 0 platform; no ADC) and `stm32f072` (Renode's
//! `platforms/cpus/stm32f072.repl`, whose ADC is `Analog.STM32F0_ADC`,
//! ADR-003 Amendment 5).
//!
//! **Parts from packages** (ADR-006, Phase 1 M3; ADR-007 D9):
//! - `*bb part <ref> package=<id>[@<version>]` takes the engine and platform
//!   from the package's `[behavioral]` layer (`renode`, a platform identity
//!   such as `stm32f103-offline@1`).
//! - `*bb pin <ref>.<pin> net=<net> model=<model> [set=<name>] [<key>=<value> ...]`
//!   on such a part takes the pin model's parameters from the package's
//!   `pins.toml` parameter set `set` (default `default`). The pin must be in
//!   `pins.toml` and allow the model. Explicit keys override the set, in the
//!   package value grammar (ADR-006 D5: `3.3`, `3.3V`, `5ns`, `5pF`), and a
//!   value that differs from the package's is reported as a warning. An
//!   `adc_in` pin's channel is the package's `adc = { channel }`.
//!
//! [`parse`] reads boards without packages; [`parse_with`] resolves
//! `package=` through a lookup (the package search path, ADR-006 D1).
//!
//! The parser is a tokenizer over a line-oriented grammar; no regex.

use std::collections::BTreeMap;

use bbcore::{AdcIn, DigitalIn, DigitalOut, Time};
use bbpkg::package::{param_set, parse_param, pin_model_params, Package, Param, ParamKind};
use bbpkg::value::{to_f64, Dim};

use crate::platforms;

/// The platforms `bb run` supports. `nrf52840` (Phase 1 M2, ADR-008 D1/D2)
/// is Breadboard's nRF52840 (`nrf52840-bb@1`: Renode's stock nRF52840
/// without the SVD download, at 64 MIPS). Its pins are written
/// `<ref>.P<port>.<nn>` (`U1.P0.13`; the reference ends at the first `.`);
/// P0.00-P0.31 and P1.00-P1.15. It has no ADC model yet (the SAADC is M8),
/// so `adc_in` is refused there.
pub const PLATFORMS: [&str; 3] = ["stm32f103", "stm32f072", "nrf52840"];

/// A part simulated by a CPU engine.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub reference: String,
    pub engine: String,
    pub platform: String,
    /// The package id when the part is given as `package=` (ADR-006).
    pub package: Option<String>,
}

/// The ADC channel that reads pin `pin` on `platform`, if any. STM32F0x2
/// (RM0091, DS9826 pinout): ADC_IN0..7 are PA0..PA7, ADC_IN8..9 PB0..PB1
/// and ADC_IN10..15 PC0..PC5.
pub fn adc_channel(platform: &str, pin: &str) -> Option<i32> {
    if platform != "stm32f072" {
        return None;
    }
    let rest = pin.strip_prefix('P')?;
    let mut chars = rest.chars();
    let port = chars.next()?;
    let n: i32 = chars.as_str().parse().ok()?;
    match (port, n) {
        ('A', 0..=7) => Some(n),
        ('B', 0..=1) => Some(8 + n),
        ('C', 0..=5) => Some(10 + n),
        _ => None,
    }
}

/// How an MCU pin meets the circuit.
#[derive(Clone, Debug, PartialEq)]
pub enum PinKind {
    /// `digital_out` driving circuit node `net` through generated glue.
    Out { net: String, model: DigitalOut },
    /// `digital_out` not connected to the circuit (probe only).
    OutUnconnected,
    /// `digital_in` reading circuit node `net`.
    In { net: String, model: DigitalIn },
    /// `adc_in` reading circuit node `net` on ADC channel `channel`.
    Adc {
        net: String,
        model: AdcIn,
        channel: i32,
    },
}

/// An MCU pin bound through its pin model.
#[derive(Clone, Debug, PartialEq)]
pub struct PinBinding {
    /// Part reference, e.g. `U1`.
    pub part: String,
    /// Pin name on the part, e.g. `PA5`.
    pub pin: String,
    pub kind: PinKind,
}

impl PinBinding {
    /// Kernel net carrying the pin's digital signal: `<part>.<pin>`.
    pub fn kernel_net(&self) -> String {
        format!("{}.{}", self.part, self.pin)
    }
}

/// A parsed board file.
#[derive(Clone, Debug)]
pub struct Board {
    pub parts: Vec<Part>,
    pub pins: Vec<PinBinding>,
    /// The SPICE circuit body (directive lines included, as comments).
    pub spice: String,
    /// The packages the parts use, one per id, in id order.
    pub packages: Vec<Package>,
    /// Lint warnings (an explicit pin parameter that differs from the
    /// package's, ADR-007 D9).
    pub warnings: Vec<String>,
}

impl PartialEq for Board {
    fn eq(&self, o: &Board) -> bool {
        self.parts == o.parts && self.pins == o.pins && self.spice == o.spice
    }
}

/// Resolves `package=<spec>` to a loaded package.
pub type Lookup<'a> = &'a dyn Fn(&str) -> Result<Package, String>;

/// One pin-model parameter: the board file's own text (Phase 0 grammar), or
/// a value from the package (or an explicit key on a package part, parsed in
/// the package grammar).
enum Val<'a> {
    Text(&'a str),
    Pkg(Param),
}

/// The parameters of one pin binding.
struct Params<'a> {
    ctx: String,
    map: BTreeMap<&'a str, Val<'a>>,
    supply: Option<SupplyFn<'a>>,
}

impl<'a> Params<'a> {
    fn take(&mut self, key: &str) -> Result<Val<'a>, String> {
        let ctx = &self.ctx;
        self.map
            .remove(key)
            .ok_or_else(|| format!("{ctx}: missing {key}="))
    }
    fn volts(&mut self, key: &str) -> Result<f64, String> {
        let v = self.take(key)?;
        let ctx = &self.ctx;
        match v {
            Val::Text(v) => volts(ctx, key, v),
            Val::Pkg(Param::Quantity(q)) => Ok(to_f64(q, Dim::Voltage)),
            Val::Pkg(Param::Supply { pin, factor }) => match self.supply {
                Some(f) => {
                    let uv = f(&pin).map_err(|e| format!("{ctx}: {key} = {pin}: {e}"))?;
                    let d = bbpkg::value::Decimal {
                        mant: factor
                            .mant
                            .checked_mul(uv)
                            .ok_or_else(|| format!("{ctx}: {key}: out of range"))?,
                        exp: factor.exp - 6,
                    };
                    Ok(d.to_f64())
                }
                None => Err(format!(
                    "{ctx}: {key} is the supply reference {pin}, which needs a bench to \
                     resolve (a bb.toml project, ADR-007 D5); give {key}= a number here"
                )),
            },
            Val::Pkg(Param::Name(n)) => Err(format!("{ctx}: {key}={n}: not a number")),
        }
    }
    fn quantity(&mut self, key: &str, dim: Dim) -> Result<f64, String> {
        let v = self.take(key)?;
        let ctx = &self.ctx;
        match v {
            Val::Text(v) => volts(ctx, key, v),
            Val::Pkg(Param::Quantity(q)) => Ok(to_f64(q, dim)),
            Val::Pkg(_) => Err(format!("{ctx}: {key}: expected a number")),
        }
    }
    fn time(&mut self, key: &str) -> Result<Time, String> {
        let v = self.take(key)?;
        let ctx = &self.ctx;
        match v {
            Val::Text(v) => time(ctx, key, v),
            Val::Pkg(Param::Quantity(q)) => u64::try_from(q)
                .map(Time::from_fs)
                .map_err(|_| format!("{ctx}: {key} out of range")),
            Val::Pkg(_) => Err(format!("{ctx}: {key}: expected a time")),
        }
    }
    fn bits(&mut self, key: &str) -> Result<u32, String> {
        let v = self.take(key)?;
        let ctx = &self.ctx;
        let bad = |b: &str| format!("{ctx}: bits={b:?}: expected 1 to 16");
        match v {
            Val::Text(b) => b
                .parse()
                .ok()
                .filter(|b| (1..=16).contains(b))
                .ok_or_else(|| bad(b)),
            Val::Pkg(Param::Quantity(q)) => u32::try_from(q)
                .ok()
                .filter(|b| (1..=16).contains(b))
                .ok_or_else(|| bad(&q.to_string())),
            Val::Pkg(_) => Err(bad("?")),
        }
    }
    fn first_key(&self) -> Option<&str> {
        self.map.keys().next().copied()
    }
}

fn parse_kv<'a>(
    lineno: usize,
    tokens: impl Iterator<Item = &'a str>,
) -> Result<BTreeMap<&'a str, &'a str>, String> {
    let mut kv = BTreeMap::new();
    for tok in tokens {
        let (k, v) = tok
            .split_once('=')
            .ok_or_else(|| format!("line {lineno}: expected key=value, got {tok:?}"))?;
        if k.is_empty() || v.is_empty() {
            return Err(format!("line {lineno}: empty key or value in {tok:?}"));
        }
        if kv.insert(k, v).is_some() {
            return Err(format!("line {lineno}: duplicate key {k:?}"));
        }
    }
    Ok(kv)
}

fn take<'a>(
    lineno: usize,
    kv: &mut BTreeMap<&'a str, &'a str>,
    key: &str,
) -> Result<&'a str, String> {
    kv.remove(key)
        .ok_or_else(|| format!("line {lineno}: missing {key}="))
}

fn volts(ctx: &str, key: &str, v: &str) -> Result<f64, String> {
    let x: f64 = v
        .parse()
        .map_err(|_| format!("{ctx}: {key}={v:?} is not a number"))?;
    if !x.is_finite() {
        return Err(format!("{ctx}: {key}={v:?} is not finite"));
    }
    Ok(x)
}

fn time(ctx: &str, key: &str, v: &str) -> Result<Time, String> {
    v.parse().map_err(|e| format!("{ctx}: {key}: {e}"))
}

fn no_extra(lineno: usize, kv: &BTreeMap<&str, &str>) -> Result<(), String> {
    match kv.keys().next() {
        Some(k) => Err(format!("line {lineno}: unknown key {k:?}")),
        None => Ok(()),
    }
}

/// Parse a board file without packages (`package=` is an error).
#[cfg_attr(not(test), allow(dead_code))]
pub fn parse(text: &str) -> Result<Board, String> {
    parse_with(text, None)
}

/// The ADC channel an `adc_in` pin of a package reads: its `pins.toml`
/// `adc = { channel = "IN<n>" }`.
fn package_adc_channel(pkg: &Package, pin: &str) -> Option<i32> {
    pkg.pins
        .by_name(pin)?
        .adc_channel
        .as_deref()?
        .strip_prefix("IN")?
        .parse()
        .ok()
}

/// Parse a board file, resolving `package=` parts through `lookup`.
pub fn parse_with(text: &str, lookup: Option<Lookup>) -> Result<Board, String> {
    let mut parts: Vec<Part> = Vec::new();
    let mut pins: Vec<PinBinding> = Vec::new();
    let mut packages: BTreeMap<String, Package> = BTreeMap::new();
    let mut warnings: Vec<String> = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let lineno = i + 1;
        let Some(rest) = line.trim_start().strip_prefix("*bb") else {
            continue;
        };
        if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
            continue; // e.g. "*bbox": an ordinary comment
        }
        let mut toks = rest.split_whitespace();
        let keyword = toks
            .next()
            .ok_or_else(|| format!("line {lineno}: empty *bb directive"))?;
        let name = toks
            .next()
            .ok_or_else(|| format!("line {lineno}: *bb {keyword} needs a name"))?;
        let mut kv = parse_kv(lineno, toks)?;
        match keyword {
            "part" => {
                if parts.iter().any(|p| p.reference == name) {
                    return Err(format!("line {lineno}: part {name} declared twice"));
                }
                if let Some(spec) = kv.remove("package") {
                    no_extra(lineno, &kv).map_err(|e| {
                        format!(
                            "{e} (a package part takes its engine and platform from the package)"
                        )
                    })?;
                    let lookup = lookup.ok_or_else(|| {
                        format!("line {lineno}: package={spec}: packages are not available here")
                    })?;
                    let pkg = lookup(spec).map_err(|e| format!("line {lineno}: {e}"))?;
                    let b = pkg.part.behavioral.as_ref().ok_or_else(|| {
                        format!(
                            "line {lineno}: package {}: no [behavioral] layer, so it cannot be a CPU part",
                            pkg.part.id
                        )
                    })?;
                    if b.engine != "renode" {
                        return Err(format!(
                            "line {lineno}: package {}: engine {} (bb run supports renode parts)",
                            pkg.part.id, b.engine
                        ));
                    }
                    let pid = b.platform.as_deref().unwrap_or_default();
                    let def = platforms::find(pid).ok_or_else(|| {
                        format!(
                            "line {lineno}: package {}: platform {pid} is not one this bb provides ({})",
                            pkg.part.id,
                            platforms::ids().join(", ")
                        )
                    })?;
                    parts.push(Part {
                        reference: name.to_string(),
                        engine: "renode".into(),
                        platform: def.board_name.to_string(),
                        package: Some(pkg.part.id.clone()),
                    });
                    packages.entry(pkg.part.id.clone()).or_insert(pkg);
                    continue;
                }
                let engine = take(lineno, &mut kv, "engine")?.to_string();
                let platform = take(lineno, &mut kv, "platform")?.to_string();
                no_extra(lineno, &kv)?;
                if engine != "renode" {
                    return Err(format!(
                        "line {lineno}: engine={engine} not supported (Phase 0: renode)"
                    ));
                }
                if !PLATFORMS.contains(&platform.as_str()) {
                    return Err(format!(
                        "line {lineno}: platform={platform} not supported ({})",
                        PLATFORMS.join(", ")
                    ));
                }
                parts.push(Part {
                    reference: name.to_string(),
                    engine,
                    platform,
                    package: None,
                });
            }
            "pin" => {
                let (part, pin) = name.split_once('.').ok_or_else(|| {
                    format!("line {lineno}: pin name must be <part>.<pin>, got {name:?}")
                })?;
                if !parts.iter().any(|p| p.reference == part) {
                    return Err(format!(
                        "line {lineno}: pin {name}: part {part} not declared (declare it first)"
                    ));
                }
                if pins.iter().any(|p| p.part == part && p.pin == pin) {
                    return Err(format!("line {lineno}: pin {name} bound twice"));
                }
                let net = kv.remove("net").map(str::to_string);
                let model_kind = take(lineno, &mut kv, "model")?;
                let set = kv.remove("set");
                let part_def = parts
                    .iter()
                    .find(|p| p.reference == part)
                    .expect("checked above")
                    .clone();
                let pkg = part_def.package.as_ref().map(|id| &packages[id]);
                let explicit: Vec<(&str, &str)> = std::mem::take(&mut kv).into_iter().collect();
                let kind = build_pin(
                    &PinReq {
                        ctx: &format!("line {lineno}"),
                        name,
                        part: &part_def,
                        pkg,
                        pin,
                        model: model_kind,
                        set,
                        net,
                        explicit: &explicit,
                        supply: None,
                    },
                    &mut warnings,
                )?;
                if let PinKind::Out { net, .. } = &kind {
                    if pins.iter().any(|p| {
                        matches!(&p.kind, PinKind::Out { net: n, .. } if n.eq_ignore_ascii_case(net))
                    }) {
                        return Err(format!(
                            "line {lineno}: net {net} already driven by another pin"
                        ));
                    }
                }
                pins.push(PinBinding {
                    part: part.to_string(),
                    pin: pin.to_string(),
                    kind,
                });
            }
            other => return Err(format!("line {lineno}: unknown directive *bb {other}")),
        }
    }
    Ok(Board {
        parts,
        pins,
        spice: text.to_string(),
        packages: packages.into_values().collect(),
        warnings,
    })
}

/// Resolves a supply pin reference (`VDD`, ADR-006 D4) to µV.
pub type SupplyFn<'a> = &'a dyn Fn(&str) -> Result<i128, String>;

/// One pin binding to build: a `*bb pin` line of a board file, or a
/// `[[bind]]` of a project (ADR-007 D5).
pub struct PinReq<'a> {
    /// Message prefix (`line 12`, `bb.toml [[bind]] U1.PA4`).
    pub ctx: &'a str,
    /// `<part>.<pin>`, for messages.
    pub name: &'a str,
    pub part: &'a Part,
    pub pkg: Option<&'a Package>,
    pub pin: &'a str,
    pub model: &'a str,
    pub set: Option<&'a str>,
    /// The circuit node (SPICE name), or `None` (an unconnected output).
    pub net: Option<String>,
    /// Explicit parameter keys, as text.
    pub explicit: &'a [(&'a str, &'a str)],
    /// Supply references resolve through this (a project's bench); `None`
    /// refuses them (a board file has no bench).
    pub supply: Option<SupplyFn<'a>>,
}

/// Build a pin's model from the package's parameter set `set` (or the
/// explicit keys of a Phase 0 part), explicit keys overriding the set.
pub fn build_pin(r: &PinReq, warnings: &mut Vec<String>) -> Result<PinKind, String> {
    let ctx = r.ctx;
    let name = r.name;
    let pin = r.pin;
    let model_kind = r.model;
    let set = r.set;
    let net = r.net.clone();
    if set.is_some() && r.pkg.is_none() {
        return Err(format!(
            "{ctx}: set= needs a part given by package= (a parameter set of its pins.toml)"
        ));
    }
    // The parameters: the board's own text, or (a package part) the
    // package's set overridden by explicit keys.
    let mut params = Params {
        ctx: ctx.to_string(),
        map: BTreeMap::new(),
        supply: r.supply,
    };
    match r.pkg {
        None => {
            for (k, v) in r.explicit {
                params.map.insert(k, Val::Text(v));
            }
        }
        Some(pkg) => {
            let pin_def = pkg.pins.by_name(pin).ok_or_else(|| {
                format!(
                    "{ctx}: {name}: {} has no pin {pin} in pins.toml",
                    pkg.part.id
                )
            })?;
            if !pin_def.models.iter().any(|m| m == model_kind) {
                return Err(format!(
                    "{ctx}: {name}: pin {pin} of {} does not allow model {model_kind} (it allows: {})",
                    pkg.part.id,
                    if pin_def.models.is_empty() {
                        "none".to_string()
                    } else {
                        pin_def.models.join(", ")
                    }
                ));
            }
            let spec = pin_model_params(model_kind).ok_or_else(|| {
                format!("{ctx}: model={model_kind} not supported (digital_out, digital_in, adc_in)")
            })?;
            // A digital_out without net= is unconnected: no parameters
            // from the package either.
            let connected = net.is_some() || model_kind != "digital_out";
            let mut from_pkg = if connected {
                param_set(pkg, model_kind, set).map_err(|e| format!("{ctx}: {e}"))?
            } else {
                BTreeMap::new()
            };
            // `model` and `load` (adc_in, ADR-008 D7) are not Phase 0
            // parameters: refuse what bb run cannot honour.
            for k in ["model", "load"] {
                if from_pkg.remove(k).is_some() {
                    return Err(format!(
                        "{ctx}: {name}: the package's adc_in set has {k}=, which bb run does not support yet"
                    ));
                }
            }
            for (k, v) in r.explicit {
                let pk = spec
                    .iter()
                    .find(|(p, ..)| p == k)
                    .map(|(_, kind, _)| *kind)
                    .ok_or_else(|| format!("{ctx}: unknown key {k:?}"))?;
                if matches!(pk, ParamKind::Name) {
                    return Err(format!("{ctx}: {k}= is not supported here"));
                }
                let explicit = parse_param(v, pk).map_err(|e| format!("{ctx}: {k}: {e}"))?;
                if let Some(pv) = from_pkg.get(*k) {
                    if *pv != explicit {
                        warnings.push(format!(
                            "{ctx}: {name}: {k}={v} overrides {}'s value (set {})",
                            pkg.part.id,
                            set.unwrap_or("default")
                        ));
                    }
                }
                from_pkg.insert(k.to_string(), explicit);
            }
            for (k, v) in from_pkg {
                let key: &'static str = spec
                    .iter()
                    .find(|(p, ..)| *p == k)
                    .map(|(p, ..)| *p)
                    .expect("a known parameter");
                params.map.insert(key, Val::Pkg(v));
            }
        }
    }
    let part_def = r.part;
    let kind = match (model_kind, net) {
        ("digital_out", Some(net)) => {
            let model = DigitalOut {
                voh: params.volts("voh")?,
                vol: params.volts("vol")?,
                rise: params.time("rise")?,
                fall: params.time("fall")?,
                zout: params.quantity("zout", Dim::Resistance)?,
            };
            if model.zout < 0.0 {
                return Err(format!("{ctx}: zout must be >= 0"));
            }
            PinKind::Out { net, model }
        }
        ("digital_out", None) => {
            if let Some(k) = params.first_key() {
                return Err(format!(
                    "{ctx}: {k}=: a digital_out without net= is not \
                     connected to the circuit and takes no electrical keys"
                ));
            }
            PinKind::OutUnconnected
        }
        ("digital_in", Some(net)) => {
            let model = DigitalIn {
                vih: params.volts("vih")?,
                vil: params.volts("vil")?,
                cin: params.quantity("cin", Dim::Capacitance)?,
            };
            if model.vil >= model.vih {
                return Err(format!("{ctx}: digital_in needs vil < vih"));
            }
            if model.cin < 0.0 {
                return Err(format!("{ctx}: cin must be >= 0"));
            }
            PinKind::In { net, model }
        }
        ("digital_in", None) => return Err(format!("{ctx}: digital_in needs net=")),
        ("adc_in", Some(net)) => {
            let platform = &part_def.platform;
            let channel = match r.pkg {
                Some(pkg) if platform == "stm32f072" => package_adc_channel(pkg, pin),
                _ => adc_channel(platform, pin),
            }
            .ok_or_else(|| {
                format!(
                    "{ctx}: {name}: no ADC channel on this pin of \
                     platform={platform} (adc_in needs stm32f072: PA0-7, PB0-1, PC0-5)"
                )
            })?;
            let vref = params.volts("vref")?;
            if vref <= 0.0 {
                return Err(format!("{ctx}: vref must be > 0"));
            }
            let bits = params.bits("bits")?;
            PinKind::Adc {
                net,
                model: AdcIn {
                    vref,
                    bits,
                    key: bbcore::AdcKey::Code12,
                },
                channel,
            }
        }
        ("adc_in", None) => return Err(format!("{ctx}: adc_in needs net=")),
        (other, _) => {
            return Err(format!(
                "{ctx}: model={other} not supported (digital_out, digital_in, adc_in)"
            ))
        }
    };
    if let Some(k) = params.first_key() {
        return Err(format!("{ctx}: unknown key {k:?}"));
    }
    Ok(kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bbpkg::schema::Registry;

    /// Parse with the first-party packages (packages/).
    fn parse_pkg(text: &str) -> Result<Board, String> {
        let sp = bbpkg::SearchPath {
            dirs: vec![bbpkg::first_party_dir()],
        };
        let reg = Registry::current();
        let lookup = |spec: &str| sp.find(spec, &reg);
        parse_with(text, Some(&lookup))
    }

    /// The Phase 0 directives of the examples, as committed before Phase 1
    /// M3 expressed them through packages (every parameter written out).
    const PHASE0_SPIKE: &str = "*bb part U1 engine=renode platform=stm32f103
*bb pin U1.PA5 net=PA5 model=digital_out voh=3.3 vol=0 rise=5ns fall=5ns zout=25
";
    const PHASE0_FEEDBACK: &str = "*bb part U1 engine=renode platform=stm32f103
*bb pin U1.PA5 net=PA5 model=digital_out voh=3.3 vol=0 rise=5ns fall=5ns zout=25
*bb pin U1.PB0 net=SENSE model=digital_in vih=2.0 vil=0.8 cin=5e-12
*bb pin U1.PA6 net=PA6 model=digital_out voh=3.3 vol=0 rise=5ns fall=5ns zout=25
";
    const PHASE0_FEEDBACK_OPEN: &str = "*bb part U1 engine=renode platform=stm32f103
*bb pin U1.PB0 net=SENSE model=digital_in vih=2.0 vil=0.8 cin=5e-12
*bb pin U1.PA6 model=digital_out
";
    const PHASE0_ADC: &str = "*bb part U1 engine=renode platform=stm32f072
*bb pin U1.PA4 net=DRIVE model=digital_out voh=3.3 vol=0 rise=5ns fall=5ns zout=25
*bb pin U1.PA1 net=ADC_IN model=adc_in vref=3.3 bits=12
*bb pin U1.PA5 net=ALARM model=digital_out voh=3.3 vol=0 rise=5ns fall=5ns zout=25
*bb pin U1.PA6 model=digital_out
*bb pin U1.PA7 model=digital_out
";
    const PHASE0_ADC_OPEN: &str = "*bb part U1 engine=renode platform=stm32f072
*bb pin U1.PA1 net=ADC_IN model=adc_in vref=3.3 bits=12
*bb pin U1.PA4 model=digital_out
*bb pin U1.PA5 model=digital_out
*bb pin U1.PA6 model=digital_out
*bb pin U1.PA7 model=digital_out
";

    /// ADR-006 G2 at the parser: every example expressed through packages
    /// gives exactly the parts (but the package id), pins and pin models of
    /// its Phase 0 directives, bit for bit (the f64 pin parameters too).
    #[test]
    fn examples_through_packages_equal_phase0_directives() {
        for (file, text, phase0) in [
            (
                "spike",
                include_str!("../../examples/spike/board.cir"),
                PHASE0_SPIKE,
            ),
            (
                "feedback",
                include_str!("../../examples/feedback/board.cir"),
                PHASE0_FEEDBACK,
            ),
            (
                "feedback-open",
                include_str!("../../examples/feedback-open/board.cir"),
                PHASE0_FEEDBACK_OPEN,
            ),
            (
                "adc",
                include_str!("../../examples/adc/board.cir"),
                PHASE0_ADC,
            ),
            (
                "adc-open",
                include_str!("../../examples/adc/board-open.cir"),
                PHASE0_ADC_OPEN,
            ),
        ] {
            let pkg = parse_pkg(text).unwrap_or_else(|e| panic!("{file}: {e}"));
            let old = parse(phase0).unwrap();
            assert_eq!(pkg.pins, old.pins, "{file}");
            assert_eq!(pkg.parts.len(), 1);
            assert_eq!(
                (
                    &pkg.parts[0].reference,
                    &pkg.parts[0].engine,
                    &pkg.parts[0].platform
                ),
                (
                    &old.parts[0].reference,
                    &old.parts[0].engine,
                    &old.parts[0].platform
                ),
                "{file}"
            );
            assert!(
                pkg.parts[0].package.is_some() && pkg.warnings.is_empty(),
                "{file}"
            );
            // Bit-level equality of every f64 (PartialEq would accept -0.0 = 0.0).
            for (a, b) in pkg.pins.iter().zip(&old.pins) {
                assert_eq!(format!("{:?}", a.kind), format!("{:?}", b.kind), "{file}");
                if let (PinKind::Out { model: x, .. }, PinKind::Out { model: y, .. }) =
                    (&a.kind, &b.kind)
                {
                    assert_eq!(
                        (x.voh.to_bits(), x.vol.to_bits(), x.zout.to_bits()),
                        (y.voh.to_bits(), y.vol.to_bits(), y.zout.to_bits())
                    );
                }
                if let (PinKind::In { model: x, .. }, PinKind::In { model: y, .. }) =
                    (&a.kind, &b.kind)
                {
                    assert_eq!(
                        (x.vih.to_bits(), x.vil.to_bits(), x.cin.to_bits()),
                        (y.vih.to_bits(), y.vil.to_bits(), y.cin.to_bits())
                    );
                }
                if let (PinKind::Adc { model: x, .. }, PinKind::Adc { model: y, .. }) =
                    (&a.kind, &b.kind)
                {
                    assert_eq!(x.vref.to_bits(), y.vref.to_bits());
                }
            }
        }
    }

    #[test]
    fn package_parts_check_pins_models_and_overrides() {
        let f1 = "*bb part U1 package=st/stm32f103c8\n";
        let ok = |s: &str| parse_pkg(&format!("{f1}{s}\n"));
        // Not a pin of the package; a model the pin does not allow; no ADC
        // on the F103; an unknown set; set= on a Phase 0 part.
        assert!(ok("*bb pin U1.PA16 net=X model=digital_out")
            .unwrap_err()
            .contains("no pin PA16"));
        assert!(ok("*bb pin U1.NRST net=X model=digital_out")
            .unwrap_err()
            .contains("does not allow"));
        assert!(ok("*bb pin U1.PA1 net=X model=adc_in").is_err());
        assert!(ok("*bb pin U1.PA5 net=X model=digital_out set=H0H1")
            .unwrap_err()
            .contains("no parameter set"));
        assert!(parse("*bb part U1 engine=renode platform=stm32f103\n*bb pin U1.PA5 net=X model=digital_out set=x\n").is_err());
        // package= with engine= is refused; packages need a lookup.
        assert!(parse_pkg("*bb part U1 package=st/stm32f103c8 engine=renode\n").is_err());
        assert!(parse("*bb part U1 package=st/stm32f103c8\n").is_err());
        assert!(parse_pkg("*bb part U1 package=st/nope\n")
            .unwrap_err()
            .contains("not found"));
        assert!(parse_pkg("*bb part U1 package=st/stm32f103c8@9.9.9\n").is_err());
        // An explicit key overrides the package's value, with a warning; the
        // package grammar applies (units, no bare M).
        let b = ok("*bb pin U1.PA5 net=X model=digital_out zout=50").unwrap();
        assert!(matches!(&b.pins[0].kind, PinKind::Out { model, .. } if model.zout == 50.0));
        assert_eq!(b.warnings.len(), 1, "{:?}", b.warnings);
        let b = ok("*bb pin U1.PA5 net=X model=digital_out voh=3.3V rise=5ns").unwrap();
        assert!(b.warnings.is_empty(), "same values: no warning");
        assert!(ok("*bb pin U1.PA5 net=X model=digital_out zout=1M").is_err());
        assert!(ok("*bb pin U1.PA5 net=X model=digital_out voh=3.3s").is_err());
        assert!(ok("*bb pin U1.PA5 net=X model=digital_out colour=red").is_err());
        // A generic passive has no behavioural layer: not a CPU part.
        assert!(parse_pkg("*bb part R1 package=generic/resistor\n")
            .unwrap_err()
            .contains("behavioral"));
        // The F072's ADC channel comes from pins.toml.
        let b =
            parse_pkg("*bb part U1 package=st/stm32f072rb\n*bb pin U1.PC5 net=X model=adc_in\n")
                .unwrap();
        assert!(matches!(b.pins[0].kind, PinKind::Adc { channel: 15, .. }));
    }

    #[test]
    fn parses_spike_board() {
        let b = parse(PHASE0_SPIKE).unwrap();
        assert_eq!(b.parts.len(), 1);
        assert_eq!(b.parts[0].reference, "U1");
        assert_eq!(b.pins.len(), 1);
        let p = &b.pins[0];
        assert_eq!((p.part.as_str(), p.pin.as_str()), ("U1", "PA5"));
        assert_eq!(
            p.kind,
            PinKind::Out {
                net: "PA5".into(),
                model: DigitalOut::STM32_GPIO_3V3
            }
        );
        assert_eq!(p.kernel_net(), "U1.PA5");
    }

    #[test]
    fn parses_feedback_boards() {
        let b = parse(PHASE0_FEEDBACK).unwrap();
        let kinds: Vec<(&str, &PinKind)> =
            b.pins.iter().map(|p| (p.pin.as_str(), &p.kind)).collect();
        assert_eq!(kinds.len(), 3);
        assert_eq!(
            kinds[1],
            (
                "PB0",
                &PinKind::In {
                    net: "SENSE".into(),
                    model: DigitalIn {
                        vih: 2.0,
                        vil: 0.8,
                        cin: 5e-12
                    }
                }
            )
        );
        assert!(matches!(kinds[2], ("PA6", PinKind::Out { net, .. }) if net == "PA6"));
        let open = parse(PHASE0_FEEDBACK_OPEN).unwrap();
        let kinds: Vec<(&str, &PinKind)> = open
            .pins
            .iter()
            .map(|p| (p.pin.as_str(), &p.kind))
            .collect();
        assert!(matches!(kinds[0], ("PB0", PinKind::In { .. })));
        assert_eq!(kinds[1], ("PA6", &PinKind::OutUnconnected));
    }

    #[test]
    fn digital_in_and_unconnected_pins() {
        let part = "*bb part U1 engine=renode platform=stm32f103\n";
        let ok = |s: &str| parse(&format!("{part}{s}\n"));
        assert!(ok("*bb pin U1.PB0 net=S model=digital_in vih=2 vil=0.8 cin=5e-12").is_ok());
        assert!(ok("*bb pin U1.PB0 model=digital_in vih=2 vil=0.8 cin=0").is_err());
        assert!(ok("*bb pin U1.PB0 net=S model=digital_in vih=2 vil=2 cin=0").is_err());
        assert!(ok("*bb pin U1.PB0 net=S model=digital_in vih=2 vil=1 cin=-1").is_err());
        assert!(ok("*bb pin U1.PB0 net=S model=digital_in vih=2 vil=1").is_err());
        assert!(ok("*bb pin U1.PA6 model=digital_out").is_ok());
        assert!(ok("*bb pin U1.PA6 model=digital_out voh=3.3").is_err());
        assert!(ok("*bb pin U1.PA6 net=S model=analog").is_err());
        // Two inputs may read one node; an input may read a driven node.
        let out = "*bb pin U1.PA5 net=S model=digital_out voh=3.3 vol=0 rise=5ns fall=5ns zout=25";
        let b = ok(&format!(
            "{out}\n*bb pin U1.PB0 net=S model=digital_in vih=2 vil=1 cin=0\n\
             *bb pin U1.PB1 net=S model=digital_in vih=2 vil=1 cin=0"
        ))
        .unwrap();
        assert_eq!(b.pins.len(), 3);
        assert!(ok(&format!("{out}\n{}", out.replace("PA5", "PA6"))).is_err());
    }

    #[test]
    fn adc_pins() {
        let f0 = "*bb part U1 engine=renode platform=stm32f072\n";
        let ok = |s: &str| parse(&format!("{f0}{s}\n"));
        let b = ok("*bb pin U1.PA1 net=ADC_IN model=adc_in vref=3.3 bits=12").unwrap();
        assert_eq!(
            b.pins[0].kind,
            PinKind::Adc {
                net: "ADC_IN".into(),
                model: AdcIn::STM32_12BIT_3V3,
                channel: 1
            }
        );
        assert!(ok("*bb pin U1.PA1 model=adc_in vref=3.3 bits=12").is_err());
        assert!(ok("*bb pin U1.PA1 net=X model=adc_in vref=0 bits=12").is_err());
        assert!(ok("*bb pin U1.PA1 net=X model=adc_in vref=3.3 bits=17").is_err());
        assert!(ok("*bb pin U1.PA1 net=X model=adc_in vref=3.3").is_err());
        assert!(ok("*bb pin U1.PA8 net=X model=adc_in vref=3.3 bits=12").is_err());
        // No ADC model on the F103 platform (ADR-003 Context fact 8).
        let f1 = "*bb part U1 engine=renode platform=stm32f103\n";
        assert!(parse(&format!(
            "{f1}*bb pin U1.PA1 net=X model=adc_in vref=3.3 bits=12\n"
        ))
        .is_err());
        assert_eq!(adc_channel("stm32f072", "PA0"), Some(0));
        assert_eq!(adc_channel("stm32f072", "PB1"), Some(9));
        assert_eq!(adc_channel("stm32f072", "PC5"), Some(15));
        assert_eq!(adc_channel("stm32f072", "PC6"), None);
        assert_eq!(adc_channel("stm32f103", "PA1"), None);
        assert!(parse("*bb part U1 engine=renode platform=stm32f4\n").is_err());
    }

    /// Phase 1 M2 (ADR-008 D2): nRF52840 pins `<ref>.P<port>.<nn>` split at
    /// the first `.`; no `adc_in` until the SAADC model (M8).
    #[test]
    fn nrf52840_pins() {
        let part = "*bb part U1 engine=renode platform=nrf52840\n";
        let b = parse(&format!(
            "{part}*bb pin U1.P0.13 net=OUT model=digital_out voh=3.3 vol=0 rise=5ns fall=5ns zout=25\n\
             *bb pin U1.P0.11 net=SENSE model=digital_in vih=2.0 vil=0.8 cin=5e-12\n"
        ))
        .unwrap();
        assert_eq!(b.parts[0].platform, "nrf52840");
        assert_eq!(
            (b.pins[0].part.as_str(), b.pins[0].pin.as_str()),
            ("U1", "P0.13")
        );
        assert_eq!(b.pins[1].kernel_net(), "U1.P0.11");
        assert!(parse(&format!(
            "{part}*bb pin U1.P0.02 net=X model=adc_in vref=3.6 bits=12\n"
        ))
        .is_err());
    }

    #[test]
    fn rejects_bad_directives() {
        let part = "*bb part U1 engine=renode platform=stm32f103\n";
        assert!(parse("*bb pin U1.PA5 net=X model=digital_out\n").is_err());
        assert!(parse(&format!(
            "{part}*bb pin U1.PA5 net=X model=digital_out voh=3.3\n"
        ))
        .is_err());
        assert!(parse(&format!("{part}{part}")).is_err());
        assert!(parse("*bb part U1 engine=qemu platform=stm32f103\n").is_err());
        assert!(parse("*bb wire A B\n").is_err());
        assert!(parse(&format!("{part}*bb pin U1PA5 net=X\n")).is_err());
        // Not directives.
        assert_eq!(
            parse("*bbox\n* bb part\nR1 a 0 1k\n").unwrap().parts.len(),
            0
        );
    }
}
