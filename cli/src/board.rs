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
//! **Digital-only nets** (ADR-014 D4): `*bb digital <net>` declares a net
//! that a bound `digital_out` drives digital-only. It gets no glue: its pins
//! are routed through the kernel net only (every reader must be a device or
//! read a device's output, the direct route of ADR-006 Amendment 6), the
//! analog engine has no input or output on it, and a SPICE element on it is
//! an error, so the directive cannot silently drop a part. A probe of it is
//! refused (probe its driver's pin). Without the directive nothing changes.
//!
//! [`parse`] reads boards without packages; [`parse_with`] resolves
//! `package=` through a lookup (the package search path, ADR-006 D1).
//!
//! The parser is a tokenizer over a line-oriented grammar; no regex.

use std::collections::{BTreeMap, BTreeSet};

use bbcore::{AdcIn, DigitalIn, DigitalOut, Level, Time};
use bbpkg::package::{param_set, parse_param, pin_model_params, Package, Param, ParamKind};
use bbpkg::value::{to_f64, Dim};

use crate::platforms;

/// The platforms `bb run` supports. `nrf52840` (Phase 1 M2, ADR-008 D1/D2)
/// is Breadboard's nRF52840 (`nrf52840-bb@4`: Renode's stock nRF52840
/// without the SVD download, at 64 MIPS, with Breadboard's SAADC). Its pins
/// are written `<ref>.P<port>.<nn>` (`U1.P0.13`; the reference ends at the
/// first `.`); P0.00-P0.31 and P1.00-P1.15. An `adc_in` reads an AIN pin or
/// `VDD` through the SAADC (Phase 1 M8, ADR-008 D7; [`nrf_adc_input`]),
/// with `load=<pin_load view>|none` and `trace_samples=true|false`.
pub const PLATFORMS: [&str; 3] = ["stm32f103", "stm32f072", "nrf52840"];

/// A part simulated by a CPU engine (`engine = "renode"`) or by the device
/// engine (`engine = "device"`, ADR-006 D7 and Amendment 6).
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub reference: String,
    pub engine: String,
    /// A Renode part: the board-file platform name (`nrf52840`); a device
    /// part: the device id (`dms/mechanism@2`).
    pub platform: String,
    /// The package id when the part is given as `package=` (ADR-006).
    pub package: Option<String>,
    /// A device part's parameters from its `*bb part` line (Amendment 6),
    /// in key order; empty for a Renode part.
    pub params: Vec<(String, bbdev::ParamValue)>,
}

impl Part {
    /// A device part (ADR-006 D7).
    pub fn is_device(&self) -> bool {
        self.engine == "device"
    }
}

/// A device part's `key=value` parameters (spec/package.md section 12): each
/// key one of the device's, each value in the package value grammar of its
/// kind. The device is built once to check the values' ranges.
pub(crate) fn device_params(
    def: &bbdev::DeviceDef,
    kv: &BTreeMap<&str, &str>,
) -> Result<Vec<(String, bbdev::ParamValue)>, String> {
    use bbdev::{ParamKind, ParamValue};
    let mut out = Vec::new();
    for (k, v) in kv {
        let spec = def.params.iter().find(|p| p.key == *k).ok_or_else(|| {
            format!(
                "device {}: unknown parameter {k:?} (it takes: {})",
                def.id,
                def.params
                    .iter()
                    .map(|p| p.key)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
        let bad = |e: String| format!("device {}: {k}={v}: {e}", def.id);
        let pv = match spec.kind {
            ParamKind::Time => {
                let fs =
                    bbpkg::value::parse_quantity(v, Dim::Time).map_err(|e| bad(e.to_string()))?;
                ParamValue::Time(Time::from_fs(
                    u64::try_from(fs).map_err(|_| bad("out of range".into()))?,
                ))
            }
            ParamKind::Resistance | ParamKind::Inductance => {
                let dim = if spec.kind == ParamKind::Resistance {
                    Dim::Resistance
                } else {
                    Dim::Inductance
                };
                let q = bbpkg::value::parse_quantity(v, dim).map_err(|e| bad(e.to_string()))?;
                ParamValue::Real(to_f64(q, dim))
            }
            ParamKind::Number => ParamValue::Real(
                bbpkg::value::parse_number(v)
                    .map_err(|e| bad(e.to_string()))?
                    .to_f64(),
            ),
        };
        out.push((k.to_string(), pv));
    }
    def.instantiate(&out)?;
    Ok(out)
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
    /// `adc_in` reading circuit node `net` on ADC channel `channel` (on the
    /// nRF52840: the SAADC input, AIN0-7 = 0-7, VDD = 8).
    Adc {
        net: String,
        model: AdcIn,
        channel: i32,
        /// The loading `adc_in` (ADR-008 D7): the part package's `pin_load`
        /// view that loads the pad, or `None` (`load = "none"`: ideal, no
        /// glue).
        load: Option<String>,
        /// `trace_samples`: every sample becomes a `debug` log record
        /// (ADR-008 D7).
        trace: bool,
    },
}

/// The nRF52840 SAADC input that reads pin `pin` (ADR-008 D2, D7): AIN0-3
/// are P0.02-P0.05, AIN4-7 P0.28-P0.31; `VDD` is input 8 (`PSELP = VDD` and
/// the `VDD1_4` reference).
pub fn nrf_adc_input(pin: &str) -> Option<i32> {
    if pin == "VDD" {
        return Some(8);
    }
    let (port, n) = bb_cpu_renode::nrf_pin(pin)?;
    match (port, n) {
        (0, 2..=5) => Some(n as i32 - 2),
        (0, 28..=31) => Some(n as i32 - 24),
        _ => None,
    }
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
    /// The nRF52840 `PIN_CNF` audit list of a project (ADR-008 D4, Phase 1
    /// M6): every bound pin with the drive its parameter set names, and
    /// every unbound GPIO on an analog-scope net, by chip pin name. `None`
    /// for a board file, which names neither (the bound pins are audited
    /// without a drive).
    pub audit: Option<Vec<(String, AuditExpect)>>,
    /// Digital-only nets, by SPICE node (ADR-014 D4, ADR-013 D1): a project's
    /// bound digital nets outside the bench's `[analog] nets`, or a board
    /// file's `*bb digital` nets (spelled as their driver's `net=`). Their
    /// pins are routed through the kernel net only; the analog engine has no
    /// glue, input or output on them.
    pub digital_only: BTreeSet<String>,
    /// The `pi` glue of every MCU `digital_out` on a circuit net (ADR-009
    /// D5, milestone P2), by kernel net (`U10.P0.06`); empty at `fast`.
    pub pi_glue: BTreeMap<String, PiGlue>,
    /// The CPU state outputs that drive the target's supply_load view at
    /// `pi` (ADR-009 D4, milestone P3), and its power-fail comparator's
    /// threshold ports (`#pofthr0..3`, ADR-009 D6, milestone P4); empty at
    /// `fast`.
    pub cpu_states: Vec<CpuState>,
    /// The target's power-fail comparator at `pi` (ADR-009 D6, milestone
    /// P4; glue rule 13); `None` at `fast` or without one.
    pub pof: Option<PofGlue>,
    /// The regulator the target's supply_load view draws at `pi` (ADR-009
    /// Amendment 12, decision D-close 3: `Some(true)` DC/DC, `Some(false)`
    /// the LDO), which the nRF52840's CPU engine checks against the
    /// firmware's `POWER.DCDCEN` at the run's end; `None` without one.
    pub regulator_dcdc: Option<bool>,
}

/// The power-fail comparator of the CPU target at `pi` (ADR-009 D6): its
/// output node, thresholded by the analog engine into the CPU engine's
/// watched `digital_in` `#pofwarn` (kernel net `<ref>.#pofwarn`), and the
/// CPU engine's `#pofen` output (kernel net `<ref>.#pofen`), which drives
/// no circuit node (the comparator is not gated by POF; the POWER model
/// applies it).
#[derive(Clone, Debug, PartialEq)]
pub struct PofGlue {
    pub node: String,
    pub kernel_net: String,
    pub pofen_net: String,
    /// The brownout reset detector's output node and its kernel net
    /// `<ref>.#bor` (ADR-009 D7, milestone P5), thresholded like `#pofwarn`
    /// into the CPU engine's watched `digital_in` `#bor`; `None` when the
    /// view has no `bor` port.
    pub bor: Option<(String, String)>,
}

/// The `digital_in` thresholds of `#pofwarn` (glue rule 13): the
/// comparator view's output reaches 1 V where V(VDD) falls to VPOF and 0 V
/// where it rises to VPOF + VPOFHYST, so the trip and the release are these
/// two levels, and no input capacitance is added.
pub const POFWARN_IN: bbcore::DigitalIn = bbcore::DigitalIn {
    vih: 1.0,
    vil: 0.0,
    cin: 0.0,
};

/// One CPU state output into a supply_load's state node (ADR-009 D4): the
/// CPU engine's output (`#run`), its kernel net (`U10.#run`), the deck's
/// `EXTERNAL` source on the node, and its level after reset.
#[derive(Clone, Debug, PartialEq)]
pub struct CpuState {
    pub cpu_pin: String,
    pub kernel_net: String,
    pub source: String,
    pub initial: Level,
}

/// The default high-Z leakage of a `pi` `digital_out` (ADR-009 D5 and its
/// default 6): 100 MΩ, a numerics floor (33 nA at 3.3 V).
pub const RHIZ_DEFAULT: f64 = 100e6;

/// Where a `pi` `digital_out` drives its high level from (ADR-009 D5).
#[derive(Clone, Debug, PartialEq)]
pub enum PiRail {
    /// The net of the package pin `rail` names (a project): the SPICE node,
    /// the display net and the chip pin.
    Net {
        node: String,
        net: String,
        pin: String,
    },
    /// A board file names no rail: a stand-in ideal source at the binding's
    /// `voh`, Phase 0's fixed level (used only by `bb run
    /// --operating-point`, gate G9, where `oe = 0` makes the rail carry no
    /// current).
    StandIn,
}

/// The `pi` form of one MCU `digital_out` (ADR-009 D5).
#[derive(Clone, Debug, PartialEq)]
pub struct PiGlue {
    pub rail: PiRail,
    /// The SPICE node of the low side (the package's ground pins' net;
    /// `0` for a board file).
    pub vss: String,
    /// The high-Z leakage, Ω.
    pub rhiz: f64,
}

/// The `pi` keys of a `digital_out` binding (ADR-009 D5; `spec/package.md`
/// section 4): `rail` (a pin name) and `rhiz` (Ω), from the explicit keys
/// over the package's parameter set; `None` where neither gives one.
pub fn digital_out_pi_keys(
    pkg: Option<&Package>,
    set: Option<&str>,
    explicit: &[(&str, &str)],
) -> Result<(Option<String>, Option<f64>), String> {
    let mut from: BTreeMap<String, Param> = match pkg {
        Some(p) => param_set(p, "digital_out", set)?,
        None => BTreeMap::new(),
    };
    for &(k, v) in explicit {
        let kind = match k {
            "rail" => ParamKind::Pin,
            "rhiz" => ParamKind::Q {
                dim: Dim::Resistance,
                supply_ok: false,
            },
            _ => continue,
        };
        from.insert(
            k.into(),
            parse_param(v, kind).map_err(|e| format!("{k}: {e}"))?,
        );
    }
    let rail = match from.remove("rail") {
        Some(Param::Name(n)) => Some(n),
        Some(_) => return Err("rail: expected a pin name".into()),
        None => None,
    };
    let rhiz = match from.remove("rhiz") {
        Some(Param::Quantity(q)) => {
            let r = to_f64(q, Dim::Resistance);
            if r <= 0.0 {
                return Err("rhiz must be > 0".into());
            }
            Some(r)
        }
        Some(_) => return Err("rhiz: expected a resistance".into()),
        None => None,
    };
    Ok((rail, rhiz))
}

/// What the `PIN_CNF` audit expects of one pin (ADR-008 D4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuditExpect {
    /// Bound as `digital_out`; the drive (`S0S1`, `H0H1`, ...) its set names.
    Out(Option<String>),
    /// Bound as `digital_in`.
    In,
    /// Connected to an analog-scope net and not bound: must not be an output.
    Unbound,
}

/// The `PIN_CNF` drive a `digital_out` parameter set of the nRF52840 stands
/// for (ADR-008 Amendment 3): the set name up to its first `_` (`H0H1_max`
/// is `H0H1`), and `default` is `S0S1`, DRIVE's reset value. `None` when the
/// name does not start with a drive.
pub fn nrf_set_drive(set: Option<&str>) -> Option<String> {
    let s = set.unwrap_or("default");
    if s == "default" {
        return Some("S0S1".into());
    }
    let head = s.split('_').next().unwrap_or_default();
    const DRIVES: [&str; 8] = [
        "S0S1", "H0S1", "S0H1", "H0H1", "D0S1", "D0H1", "S0D1", "H0D1",
    ];
    DRIVES.contains(&head).then(|| head.to_string())
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
    /// An optional identifier parameter (`model`, `load`).
    fn name(&mut self, key: &str) -> Result<Option<String>, String> {
        let ctx = &self.ctx;
        match self.map.remove(key) {
            None => Ok(None),
            Some(Val::Text(v)) => Ok(Some(v.to_string())),
            Some(Val::Pkg(Param::Name(n))) => Ok(Some(n)),
            Some(Val::Pkg(_)) => Err(format!("{ctx}: {key}: expected a name")),
        }
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

/// One `*bb packages path=<dir>` line (ADR-015 D7, `spec/package.md`
/// section 12): the directory, relative to the board file.
fn package_path<'a>(lineno: usize, toks: impl Iterator<Item = &'a str>) -> Result<&'a str, String> {
    let mut kv = parse_kv(lineno, toks)?;
    let path = take(lineno, &mut kv, "path")?;
    no_extra(lineno, &kv)?;
    if std::path::Path::new(path).is_absolute() || path.starts_with('/') {
        return Err(format!(
            "line {lineno}: *bb packages path={path}: the path must be relative to the board file"
        ));
    }
    Ok(path)
}

/// The board file's package directories, in order: every `*bb packages
/// path=<dir>` line (ADR-015 D7). They come before the first-party
/// directory on its search path, as a `bb.toml`'s `[packages] paths` do.
pub fn package_paths(text: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let Some(rest) = line.trim_start().strip_prefix("*bb") else {
            continue;
        };
        let mut toks = rest.split_whitespace();
        if !rest.starts_with(char::is_whitespace) || toks.next() != Some("packages") {
            continue;
        }
        out.push(package_path(i + 1, toks)?.to_string());
    }
    Ok(out)
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

/// The SAADC input of an nRF52840 package pin: its `pins.toml`
/// `adc = { channel = "AIN<n>" }` (ADR-008 D2).
fn package_nrf_input(pkg: &Package, pin: &str) -> Option<i32> {
    pkg.pins
        .by_name(pin)?
        .adc_channel
        .as_deref()?
        .strip_prefix("AIN")?
        .parse()
        .ok()
        .filter(|n| (0..8).contains(n))
}

/// Parse a board file, resolving `package=` parts through `lookup`.
pub fn parse_with(text: &str, lookup: Option<Lookup>) -> Result<Board, String> {
    let mut parts: Vec<Part> = Vec::new();
    let mut pins: Vec<PinBinding> = Vec::new();
    let mut packages: BTreeMap<String, Package> = BTreeMap::new();
    let mut warnings: Vec<String> = Vec::new();
    // `*bb digital <net>` (ADR-014 D4): (line, net), checked after the pins.
    let mut digital: Vec<(usize, String)> = Vec::new();
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
        if keyword == "packages" {
            // The package search path (ADR-015 D7): read before the parts by
            // `package_paths`; checked here once more.
            package_path(lineno, toks)?;
            continue;
        }
        let name = toks
            .next()
            .ok_or_else(|| format!("line {lineno}: *bb {keyword} needs a name"))?;
        let mut kv = parse_kv(lineno, toks)?;
        match keyword {
            "digital" => {
                if let Some(k) = kv.keys().next() {
                    return Err(format!(
                        "line {lineno}: *bb digital {name}: unknown key {k} (the directive takes \
                         only the net)"
                    ));
                }
                if digital.iter().any(|(_, n)| n.eq_ignore_ascii_case(name)) {
                    return Err(format!("line {lineno}: *bb digital {name}: declared twice"));
                }
                digital.push((lineno, name.to_string()));
            }
            "part" => {
                if parts.iter().any(|p| p.reference == name) {
                    return Err(format!("line {lineno}: part {name} declared twice"));
                }
                if let Some(spec) = kv.remove("package") {
                    for k in ["engine", "platform"] {
                        if kv.contains_key(k) {
                            return Err(format!(
                                "line {lineno}: unknown key {k:?} (a package part takes its engine and \
                                 platform from the package)"
                            ));
                        }
                    }
                    let lookup = lookup.ok_or_else(|| {
                        format!("line {lineno}: package={spec}: packages are not available here")
                    })?;
                    let pkg = lookup(spec).map_err(|e| format!("line {lineno}: {e}"))?;
                    let b = pkg.part.behavioral.as_ref().ok_or_else(|| {
                        format!(
                            "line {lineno}: package {}: no [behavioral] layer, so it cannot be a CPU \
                             or device part",
                            pkg.part.id
                        )
                    })?;
                    if b.engine == "device" {
                        // ADR-006 D7, Amendment 6 (spec/package.md section 12).
                        let did = b.device.as_deref().unwrap_or_default();
                        let def = bbdev::find(did).ok_or_else(|| {
                            format!(
                                "line {lineno}: package {}: device {did} is not one this bb provides ({})",
                                pkg.part.id,
                                bbdev::ids().join(", ")
                            )
                        })?;
                        let params =
                            device_params(def, &kv).map_err(|e| format!("line {lineno}: {e}"))?;
                        parts.push(Part {
                            reference: name.to_string(),
                            engine: "device".into(),
                            platform: def.id.to_string(),
                            package: Some(pkg.part.id.clone()),
                            params,
                        });
                        packages.entry(pkg.part.id.clone()).or_insert(pkg);
                        continue;
                    }
                    no_extra(lineno, &kv).map_err(|e| {
                        format!(
                            "{e} (a package part takes its engine and platform from the package)"
                        )
                    })?;
                    if b.engine != "renode" {
                        return Err(format!(
                            "line {lineno}: package {}: engine {} (bb run supports renode and device parts)",
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
                        params: Vec::new(),
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
                    params: Vec::new(),
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
                if part_def.is_device() {
                    device_pin(&part_def, pin, model_kind)
                        .map_err(|e| format!("line {lineno}: pin {name}: {e}"))?;
                }
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
    let digital_only = digital_nets(&digital, &parts, &pins, text)?;
    Ok(Board {
        parts,
        pins,
        spice: text.to_string(),
        packages: packages.into_values().collect(),
        warnings,
        audit: None,
        digital_only,
        pi_glue: BTreeMap::new(),
        cpu_states: Vec::new(),
        pof: None,
        regulator_dcdc: None,
    })
}

/// The `*bb digital` nets (ADR-014 D4), each spelled as its driver's
/// `net=`: a bound `digital_out` drives it, every `digital_in` on it is
/// routed directly (its part or the driver's is a device), no `adc_in`
/// reads it, and no SPICE element names it.
fn digital_nets(
    decl: &[(usize, String)],
    parts: &[Part],
    pins: &[PinBinding],
    spice: &str,
) -> Result<BTreeSet<String>, String> {
    let is_dev = |part: &str| parts.iter().any(|p| p.reference == part && p.is_device());
    let mut out = BTreeSet::new();
    for (lineno, net) in decl {
        let ctx = format!("line {lineno}: *bb digital {net}");
        let driver = pins
            .iter()
            .find(|p| matches!(&p.kind, PinKind::Out { net: n, .. } if n.eq_ignore_ascii_case(net)))
            .ok_or_else(|| format!("{ctx}: no bound digital_out drives it"))?;
        let PinKind::Out { net: spelled, .. } = &driver.kind else {
            unreachable!("matched above")
        };
        for r in pins {
            match &r.kind {
                PinKind::Adc { net: n, .. } if n.eq_ignore_ascii_case(net) => {
                    return Err(format!(
                        "{ctx}: {} reads it as adc_in (a digital-only net has no analog value)",
                        r.kernel_net()
                    ));
                }
                PinKind::In { net: n, .. }
                    if n.eq_ignore_ascii_case(net) && !is_dev(&r.part) && !is_dev(&driver.part) =>
                {
                    return Err(format!(
                        "{ctx}: {} reads it through the analog engine's threshold (only a \
                         device, or a pin reading a device's output, reads a digital-only net \
                         directly)",
                        r.kernel_net()
                    ));
                }
                _ => {}
            }
        }
        if let Some((l, text)) = element_on(spice, net) {
            return Err(format!(
                "{ctx}: line {l} puts a SPICE element on it ({}); a digital-only net has no \
                 analog side, so remove the element or the directive (ADR-014 D4)",
                text.trim()
            ));
        }
        out.insert(spelled.clone());
    }
    Ok(out)
}

/// The first SPICE element line (outside `.subckt` definitions) that names
/// node `net` (case-insensitive), as `(line number, line)`. An element's
/// tokens after its name are split on whitespace, parentheses, commas and
/// `=`, so a B-source expression `V(net)` counts too.
fn element_on(spice: &str, net: &str) -> Option<(usize, String)> {
    let mut depth = 0usize;
    let mut found: Option<(usize, String)> = None;
    let mut current: Option<(usize, String)> = None;
    let names = |line: &str| -> bool {
        let line = line.split(['$', ';']).next().unwrap_or("");
        line.split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | ',' | '=' | '{' | '}'))
            .filter(|t| !t.is_empty())
            .skip(1)
            .any(|t| t.eq_ignore_ascii_case(net))
    };
    for (i, raw) in spice.lines().enumerate() {
        let t = raw.trim_start();
        if let Some(cont) = t.strip_prefix('+') {
            if let Some((_, l)) = current.as_mut() {
                l.push(' ');
                l.push_str(cont);
            }
            continue;
        }
        if let Some((l, text)) = current.take() {
            if found.is_none() && names(&text) {
                found = Some((l, text));
            }
        }
        let lower = t.to_ascii_lowercase();
        if lower.starts_with(".subckt") {
            depth += 1;
            continue;
        }
        if lower.starts_with(".ends") {
            depth = depth.saturating_sub(1);
            continue;
        }
        if t.is_empty() || t.starts_with('*') || t.starts_with('.') || depth > 0 {
            continue;
        }
        current = Some((i + 1, t.to_string()));
    }
    if let Some((l, text)) = current {
        if found.is_none() && names(&text) {
            found = Some((l, text));
        }
    }
    found
}

/// A device part's pin (spec/package.md section 12): one of the device's
/// pins, `digital_in` on an input and `digital_out` on an output.
pub(crate) fn device_pin(part: &Part, pin: &str, model: &str) -> Result<(), String> {
    let def = bbdev::find(&part.platform)
        .ok_or_else(|| format!("device {}: not provided by this bb", part.platform))?;
    let dev = def.instantiate(&part.params)?;
    let p = dev.pins().iter().find(|p| p.name == pin).ok_or_else(|| {
        format!(
            "device {} has no pin {pin} (it has: {})",
            def.id,
            dev.pins()
                .iter()
                .map(|p| p.name)
                .collect::<Vec<_>>()
                .join(", ")
        )
    })?;
    match (p.dir, model) {
        (bbdev::PinDir::In, "digital_in") | (bbdev::PinDir::Out, "digital_out") => Ok(()),
        (bbdev::PinDir::In, _) => Err(format!(
            "model={model}: {pin} is an input of device {}: digital_in",
            def.id
        )),
        (bbdev::PinDir::Out, _) => Err(format!(
            "model={model}: {pin} is an output of device {}: digital_out",
            def.id
        )),
    }
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
    let nrf = r.part.platform == "nrf52840";
    // `trace_samples` is a binding option, not a pin-model parameter
    // (ADR-008 D7): every sample of the SAADC input becomes a log record.
    let mut trace = false;
    let mut explicit: Vec<(&str, &str)> = Vec::new();
    for &(k, v) in r.explicit {
        if k == "trace_samples" {
            trace = match v {
                "1" | "true" => true,
                "0" | "false" => false,
                _ => {
                    return Err(format!(
                        "{ctx}: trace_samples={v:?}: expected true or false"
                    ))
                }
            };
            if !(nrf && model_kind == "adc_in") {
                return Err(format!(
                    "{ctx}: trace_samples is for an adc_in on the nRF52840's SAADC"
                ));
            }
        } else {
            explicit.push((k, v));
        }
    }
    // The SAADC's VDD input (ADR-008 D7): an adc_in on the supply pin VDD
    // pushes the net's voltage to the model (PSELP = VDD, REFSEL = VDD1_4).
    // A supply pin has no pin models in pins.toml, so its parameters are
    // the binding's own.
    let vdd_sense = nrf && pin == "VDD" && model_kind == "adc_in";
    let pkg = if vdd_sense { None } else { r.pkg };
    // The parameters: the board's own text, or (a package part) the
    // package's set overridden by explicit keys.
    let mut params = Params {
        ctx: ctx.to_string(),
        map: BTreeMap::new(),
        supply: r.supply,
    };
    match pkg {
        // The VDD input of a package part (a board file's `package=` part or a
        // project's target): its keys are in the package value grammar
        // (spec/package.md section 6: "3.6V"), as every other binding of a
        // package part.
        None if vdd_sense && r.pkg.is_some() => {
            let spec = pin_model_params(model_kind).expect("adc_in parameters");
            for (k, v) in explicit.iter().copied() {
                let (key, pk) = spec
                    .iter()
                    .find(|(p, ..)| *p == k)
                    .map(|(p, kind, _)| (*p, *kind))
                    .ok_or_else(|| format!("{ctx}: unknown key {k:?}"))?;
                if matches!(pk, ParamKind::Name) && !matches!(k, "load" | "model") {
                    return Err(format!("{ctx}: {k}= is not supported here"));
                }
                let pv = parse_param(v, pk).map_err(|e| format!("{ctx}: {k}: {e}"))?;
                params.map.insert(key, Val::Pkg(pv));
            }
        }
        None => {
            for (k, v) in explicit.iter().copied() {
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
            // `model` and `load` (adc_in, ADR-008 D7) are honoured on the
            // nRF52840's SAADC only: refuse what bb run cannot honour.
            for k in ["model", "load"] {
                if !nrf && from_pkg.remove(k).is_some() {
                    return Err(format!(
                        "{ctx}: {name}: the package's adc_in set has {k}=, which bb run supports \
                         on the nRF52840's SAADC only"
                    ));
                }
            }
            for (k, v) in explicit.iter().copied() {
                let pk = spec
                    .iter()
                    .find(|(p, ..)| *p == k)
                    .map(|(_, kind, _)| *kind)
                    .ok_or_else(|| format!("{ctx}: unknown key {k:?}"))?;
                if matches!(pk, ParamKind::Name) && !(nrf && matches!(k, "load" | "model")) {
                    return Err(format!("{ctx}: {k}= is not supported here"));
                }
                let explicit = parse_param(v, pk).map_err(|e| format!("{ctx}: {k}: {e}"))?;
                if let Some(pv) = from_pkg.get(k) {
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
        ("adc_in", Some(net)) if nrf => {
            // Breadboard's nRF52840 SAADC (ADR-008 D7): the channel is the
            // SAADC input (the package's `adc = { channel = "AIN<n>" }`, or
            // VDD); `vref` is the clamp `[0, vref]` of the pushed value (the
            // ADR's `vmax`), `bits` documents only; pushes are raw uV.
            let channel = if vdd_sense {
                Some(8)
            } else {
                match r.pkg {
                    Some(pkg) => package_nrf_input(pkg, pin),
                    None => nrf_adc_input(pin),
                }
            }
            .ok_or_else(|| {
                format!(
                    "{ctx}: {name}: no SAADC input on this pin of platform=nrf52840 (AIN0-3 = \
                     P0.02-P0.05, AIN4-7 = P0.28-P0.31, and VDD)"
                )
            })?;
            let vref = params.volts("vref")?;
            if vref <= 0.0 {
                return Err(format!("{ctx}: vref must be > 0"));
            }
            let bits = if params.map.contains_key("bits") {
                params.bits("bits")?
            } else {
                12
            };
            if let Some(m) = params.name("model")? {
                if m != "nrf_saadc" {
                    return Err(format!(
                        "{ctx}: {name}: model={m}: the nRF52840's adc_in model is nrf_saadc"
                    ));
                }
            }
            let load = params.name("load")?.filter(|l| l != "none");
            if let Some(l) = &load {
                let Some(pkg) = r.pkg.filter(|_| !vdd_sense) else {
                    return Err(format!(
                        "{ctx}: {name}: load={l}: a loading adc_in needs a part given by \
                         package= (its pin_load view) on an AIN pin; or load=none"
                    ));
                };
                if !pkg
                    .part
                    .analog
                    .iter()
                    .any(|v| v.name == *l && v.kind == "pin_load")
                {
                    return Err(format!(
                        "{ctx}: {name}: load={l}: {} has no pin_load view {l}",
                        pkg.part.id
                    ));
                }
            }
            PinKind::Adc {
                net,
                model: AdcIn {
                    vref,
                    bits,
                    key: bbcore::AdcKey::Microvolts,
                },
                channel,
                load,
                trace,
            }
        }
        ("adc_in", Some(net)) => {
            let platform = &part_def.platform;
            let channel = match r.pkg {
                Some(pkg) if platform == "stm32f072" => package_adc_channel(pkg, pin),
                _ => adc_channel(platform, pin),
            }
            .ok_or_else(|| {
                format!(
                    "{ctx}: {name}: no ADC channel on this pin of \
                     platform={platform} (adc_in needs stm32f072: PA0-7, PB0-1, PC0-5, or \
                     nrf52840)"
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
                load: None,
                trace: false,
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

    #[test]
    fn packages_directive() {
        // ADR-015 D7: `*bb packages path=<dir>`, relative, in file order.
        let t = "*bb packages path=../packages\n* x\n*bb packages path=vendor/pk\n\
                 *bb part U1 engine=renode platform=stm32f103\n";
        assert_eq!(package_paths(t).unwrap(), ["../packages", "vendor/pk"]);
        assert!(parse(t).is_ok());
        assert_eq!(
            package_paths("*bbox packages path=x\n").unwrap(),
            Vec::<String>::new()
        );
        for (bad, why) in [
            ("*bb packages path=/abs/pk\n", "relative"),
            ("*bb packages\n", "missing path="),
            ("*bb packages path=a other=b\n", "unknown key"),
            ("*bb packages dir=a\n", "missing path="),
            ("*bb packages path=a path=b\n", "duplicate key"),
        ] {
            let e = package_paths(bad).unwrap_err();
            assert!(e.contains(why), "{bad}: {e}");
            let e = parse(bad).unwrap_err();
            assert!(e.contains(why), "{bad}: {e}");
        }
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
                channel: 1,
                load: None,
                trace: false,
            }
        );
        assert!(ok("*bb pin U1.PA1 net=X model=adc_in vref=3.3 bits=12 trace_samples=1").is_err());
        assert!(ok("*bb pin U1.PA1 net=X model=adc_in vref=3.3 bits=12 load=x").is_err());
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
        // The SAADC (Phase 1 M8, ADR-008 D7): AIN inputs, VDD, raw uV pushes.
        let b = parse(&format!(
            "{part}*bb pin U1.P0.02 net=X model=adc_in vref=3.6 trace_samples=1\n\
             *bb pin U1.P0.31 net=Y model=adc_in vref=3.6 bits=12 load=none\n\
             *bb pin U1.VDD net=VDD model=adc_in vref=3.6\n"
        ))
        .unwrap();
        let chans: Vec<(i32, bool)> = b
            .pins
            .iter()
            .map(|p| match &p.kind {
                PinKind::Adc {
                    channel,
                    model,
                    load: None,
                    trace,
                    ..
                } => {
                    assert_eq!(model.key, bbcore::AdcKey::Microvolts);
                    (*channel, *trace)
                }
                k => panic!("{k:?}"),
            })
            .collect();
        assert_eq!(chans, [(0, true), (7, false), (8, false)]);
        // Not an AIN pin; a load without a package; trace on a digital pin.
        for bad in [
            "*bb pin U1.P0.06 net=X model=adc_in vref=3.6",
            "*bb pin U1.P0.02 net=X model=adc_in vref=3.6 load=saadc_se",
            "*bb pin U1.P0.13 net=X model=digital_in vih=2 vil=1 cin=0 trace_samples=1",
        ] {
            assert!(parse(&format!("{part}{bad}\n")).is_err(), "{bad}");
        }
        assert_eq!(nrf_adc_input("P0.05"), Some(3));
        assert_eq!(nrf_adc_input("P0.28"), Some(4));
        assert_eq!(nrf_adc_input("P0.27"), None);
    }

    /// ADR-014 D4 (gate G5): `*bb digital <net>`.
    #[test]
    fn digital_only_directive() {
        let base = "*bb part U1 package=nordic/nrf52840-qiaa\n\
                    *bb part M1 package=dms/detent-mechanism s0=500\n\
                    *bb pin U1.P0.26 net=MOT_A_IN1 model=digital_out voh=3.3\n\
                    *bb pin U1.P1.14 net=HOME_N model=digital_in vih=2.31 vil=0.99\n\
                    *bb pin M1.IN_A1 net=MOT_A_IN1 model=digital_in\n\
                    *bb pin M1.HOME_N net=HOME_N model=digital_out\n";
        // Without the directive nothing is digital-only.
        let b = parse_pkg(&format!("{base}R1 HOME_N 0 100k\n")).unwrap();
        assert!(b.digital_only.is_empty());
        // With it the net is, spelled as its driver's net=.
        let b = parse_pkg(&format!("{base}*bb digital mot_a_in1\nR1 HOME_N 0 100k\n")).unwrap();
        assert_eq!(b.digital_only.iter().collect::<Vec<_>>(), vec!["MOT_A_IN1"]);
        // HOME_N too: U1 reads a device's output directly.
        let b = parse_pkg(&format!("{base}*bb digital HOME_N\n")).unwrap();
        assert!(b.digital_only.contains("HOME_N"));
        for (extra, why) in [
            // An element on a digital-only net is refused (also through a
            // continuation line and inside a B expression).
            (
                "*bb digital MOT_A_IN1\nRPD MOT_A_IN1 0 100k\n",
                "SPICE element",
            ),
            (
                "*bb digital MOT_A_IN1\nRPD X\n+ mot_a_in1 0 100k\n",
                "SPICE element",
            ),
            (
                "*bb digital MOT_A_IN1\nB1 X 0 V = V(MOT_A_IN1)*2\n",
                "SPICE element",
            ),
            ("*bb digital HOME_N\nR4 HOME_N 0 100k\n", "SPICE element"),
            ("*bb digital NOPE\n", "no bound digital_out"),
            (
                "*bb digital MOT_A_IN1\n*bb digital MOT_A_IN1\n",
                "declared twice",
            ),
            ("*bb digital MOT_A_IN1 x=1\n", "unknown key"),
        ] {
            let e = parse_pkg(&format!("{base}{extra}")).unwrap_err();
            assert!(e.contains(why), "{extra}: {e}");
        }
        // A node of the same name inside a .subckt definition is not on it.
        let sub = ".subckt S MOT_A_IN1 b\nR1 MOT_A_IN1 b 1k\n.ends\n";
        assert!(parse_pkg(&format!("{base}*bb digital MOT_A_IN1\n{sub}")).is_ok());
        // A CPU input reading a CPU output's net goes through ngspice's
        // threshold, which a digital-only net does not have.
        let cpu = "*bb part U1 package=nordic/nrf52840-qiaa\n\
                   *bb pin U1.P0.26 net=A model=digital_out voh=3.3\n\
                   *bb pin U1.P0.27 net=A model=digital_in vih=2.31 vil=0.99\n\
                   *bb pin U1.P0.02 net=B model=adc_in vref=3.6\n\
                   *bb pin U1.P0.28 net=B model=digital_out voh=3.3\n";
        let e = parse_pkg(&format!("{cpu}*bb digital A\n")).unwrap_err();
        assert!(e.contains("threshold"), "{e}");
        let e = parse_pkg(&format!("{cpu}*bb digital B\n")).unwrap_err();
        assert!(e.contains("adc_in"), "{e}");
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
