//! `part.toml` and `pins.toml` v0 (ADR-006 D2-D4, `spec/package.md`):
//! loading, with strict keys, reserved keys refused, enums and formats
//! checked. Deeper cross-checks (pins against views, parameter sets,
//! SPICE names) are in [`crate::lint`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::obj::Obj;
use crate::schema::{Registry, Version};
use crate::toml;
use crate::value::{parse_quantity, Decimal, Dim};

/// `part.toml`'s `[part] kind`.
pub const KINDS: &[&str] = &[
    "soc",
    "module",
    "power",
    "analog_ic",
    "logic",
    "discrete",
    "passive",
    "connector",
    "mechanical",
    "testpoint",
];

/// Provenance `source`.
pub const SOURCES: &[&str] = &[
    "datasheet",
    "vendor_model",
    "vendor_derived",
    "measured",
    "estimated",
    "agent_generated",
];

/// Provenance `confidence` (an enum, not a number: ADR-006 D2, approver
/// decision A).
pub const CONFIDENCES: &[&str] = &["unchecked", "self_checked", "datasheet_checked", "measured"];

/// `pins.toml` roles.
pub const ROLES: &[&str] = &[
    "power_in",
    "power_out",
    "ground",
    "io",
    "input",
    "output",
    "analog",
    "nc",
    "passive",
];

/// Behavioural engines Phase 1 accepts (D3); others need an ADR.
pub const ENGINES: &[&str] = &["renode", "device"];

/// The fidelity ladder, lowest first (D3).
pub const RUNGS: &[&str] = &["fast", "pi", "si", "em"];

/// The SPDX id that forbids committing a package to a shared repository
/// (ADR-006 D9, approver decision A).
pub const LOCAL_ONLY: &str = "LicenseRef-local-only";

/// Top-level keys still reserved in bb-part/1 (ADR-006 D11): refused until
/// an ADR gives them a meaning, when they become accepted within the major.
pub const RESERVED_TOP: &[&str] = &["layers", "thermal", "rtl"];

/// What a reserved key is reserved for (the refusal's message).
pub const RESERVED_FOR: &str = "a later milestone (a reserved key of bb-part/1, ADR-006 D11)";

/// The view kinds: `circuit`, `pin_load`, and in bb-part/1 `switch`
/// (ADR-010 D2), `supply_load` (ADR-009 D4) and `pof_comparator` (ADR-009
/// D6, Phase 2 P4).
pub const VIEW_KINDS: &[&str] = &[
    "circuit",
    "pin_load",
    "switch",
    "supply_load",
    "pof_comparator",
];

/// A `pof_comparator` view's state ports (ADR-009 D6): the POFCON.THRESHOLD
/// code's bits, the CPU engine's outputs `#pofthr0..3`.
pub const POF_THRESHOLD_STATES: &[&str] = &["pofthr0", "pofthr1", "pofthr2", "pofthr3"];

/// A `pof_comparator` view's observe port (ADR-009 D6): its output, the
/// CPU engine's watched `digital_in` `#pofwarn`.
pub const POF_OUTPUT_SIGNAL: &str = "pofwarn";

/// A `pof_comparator` view's optional second observe port (ADR-009 D7,
/// Phase 2 P5): the brownout reset detector's output, the CPU engine's
/// watched `digital_in` `#bor`.
pub const POF_BOR_SIGNAL: &str = "bor";

/// A named corner's name (ADR-009 D8): `typ`, `min`, `max`, `suite_compat`
/// or `worst_<x>`.
pub fn corner_name_ok(n: &str) -> bool {
    matches!(n, "typ" | "min" | "max" | "suite_compat")
        || n.strip_prefix("worst_").is_some_and(|x| {
            !x.is_empty()
                && x.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        })
}

/// A `[[document]]` (bb-part/1, ADR-012 D3): a hashed document the
/// package's claims cite. Inert until milestone E2 (the claim checker).
#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    pub id: String,
    pub title: Option<String>,
    pub revision: Option<String>,
    /// Where it was fetched (informative; the sha256 is the identity).
    pub url: Option<String>,
    /// 64 lower-case hex digits: the PDF the claims were checked against.
    pub sha256: String,
    /// `agent:<who>` or `user:<who>`.
    pub fetched_by: Option<String>,
}

/// One `[power] idd` state (bb-part/1, ADR-009 D4): a supply current.
#[derive(Clone, Debug, PartialEq)]
pub struct Idd {
    pub state: String,
    pub typ: String,
    pub max: Option<String>,
    pub reference: String,
}

/// `[power]` (bb-part/1, ADR-009 D4, D5): the supply currents per CPU
/// state, the operating VDD minimum, and the rail `digital_out` drives from
/// at `pi`. Inert until milestones P2 and P3 use it.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Power {
    pub idd: Vec<Idd>,
    pub vdd_min: Option<String>,
    pub io_rail: Option<String>,
}

/// A named corner of a view (`[[analog.corners]]`, bb-part/1, ADR-009 D8):
/// parameter overrides with a reference. Inert until a bench selects one
/// (milestone P1).
#[derive(Clone, Debug, PartialEq)]
pub struct Corner {
    pub name: String,
    pub params: Vec<(String, String)>,
    pub reference: String,
}

/// A relational limit of `pins.toml` (`limits`, bb-pins/1, ADR-009 D3): an
/// absolute maximum rating as `<value>` or `<pin> + <value>` / `<pin> -
/// <value>`. Inert until milestone P1 makes limits assertions.
#[derive(Clone, Debug, PartialEq)]
pub struct Limit {
    pub pin: String,
    pub min: Option<String>,
    pub max: Option<String>,
    pub source: String,
}

/// A limit's bound, parsed: an optional reference pin and a voltage offset
/// (µV).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bound {
    pub pin: Option<String>,
    pub offset_uv: i128,
}

/// Parse a limit bound: `<value>`, `<PIN> + <value>` or `<PIN> - <value>`
/// (one supply-plus-offset term, no general expressions; ADR-009 D3).
pub fn parse_bound(text: &str) -> Result<Bound, String> {
    let t = text.trim();
    let starts_num = t.starts_with(|c: char| c.is_ascii_digit() || matches!(c, '.' | '-' | '+'));
    if starts_num {
        return parse_quantity(t, Dim::Voltage)
            .map(|v| Bound {
                pin: None,
                offset_uv: v,
            })
            .map_err(|e| format!("{text:?}: {e}"));
    }
    let (pin, sign, rest) = match (t.find(" + "), t.find(" - ")) {
        (Some(i), None) => (&t[..i], 1, &t[i + 3..]),
        (None, Some(i)) => (&t[..i], -1, &t[i + 3..]),
        (None, None) if is_pin_name(t) => (t, 1, "0V"),
        _ => {
            return Err(format!(
                "{text:?}: expected <value>, <pin> + <value> or <pin> - <value> (ADR-009 D3)"
            ))
        }
    };
    if !is_pin_name(pin.trim()) {
        return Err(format!("{text:?}: {:?} is not a pin name", pin.trim()));
    }
    let v = parse_quantity(rest.trim(), Dim::Voltage).map_err(|e| format!("{text:?}: {e}"))?;
    Ok(Bound {
        pin: Some(pin.trim().to_string()),
        offset_uv: sign * v,
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct Datasheet {
    pub url: String,
    pub rev: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Provenance {
    pub source: String,
    pub base: Option<String>,
    pub refs: Vec<String>,
    pub deviations: Vec<String>,
    pub not_modelled: Vec<String>,
    pub origin: Option<String>,
    pub generated_by: Option<String>,
    pub reviewed_by: Vec<String>,
    pub confidence: String,
    /// bb-part/1 (ADR-011 D7, ADR-012): the upstream model's SPDX licence
    /// of a `vendor_derived` view; the publication licence gate checks it
    /// (`lint::licence_gate`).
    pub upstream_license: Option<String>,
}

/// A port of an analog view.
#[derive(Clone, Debug, PartialEq)]
pub enum Port {
    /// A `circuit` view's port: a pin name from `pins.toml`.
    Pin(String),
    /// A `pin_load` view's typed port (D4).
    Pad,
    Ctrl {
        signal: String,
    },
    Observe {
        signal: String,
    },
    OtherPin {
        pin: String,
    },
    /// A `supply_load` view's state port (bb-part/1, ADR-009 D4): the CPU
    /// state output `#<state>`, a node in 0..1.
    State {
        state: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Qualified {
    pub lattice: String,
    pub options: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub name: String,
    /// `circuit`, `pin_load` or (bb-part/1, ADR-010 D2) `switch`.
    pub kind: String,
    pub purpose: Option<String>,
    pub fidelity: Vec<String>,
    pub default_for: Vec<String>,
    pub file: String,
    pub subckt: String,
    pub ports: Vec<Port>,
    pub params: Vec<(String, String)>,
    pub qualified: Vec<Qualified>,
    pub provenance: Option<Provenance>,
    /// bb-part/1 (ADR-009): the view's supply ports, informative, for lint.
    pub rails: Vec<String>,
    /// bb-part/1 (ADR-009 D8): `[[analog.corners]]`.
    pub corners: Vec<Corner>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Behavioral {
    pub engine: String,
    pub platform: Option<String>,
    pub device: Option<String>,
    /// bb-part/1 (ADR-011 D5, D7; Amendment 2): the vendor SVD, fetched at
    /// use by `bb open` and recorded as a `bb.lock` `[[vendor_file]]`; never
    /// part of the package.
    pub svd: Option<VendorRef>,
}

/// A vendor file a package names but never contains (ADR-011 D5, D7): an
/// `https://` url, its sha256 (64 lower-case hex digits) and its licence
/// (one SPDX id; a `LicenseRef-` one too, since it is never redistributed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VendorRef {
    pub url: String,
    pub sha256: String,
    pub license: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Module {
    pub chip: String,
    pub pinmap: String,
}

/// `part.toml`.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub schema: String,
    pub id: String,
    pub version: String,
    pub kind: String,
    pub manufacturer: Option<String>,
    pub mpn: Vec<String>,
    pub mpn_patterns: Vec<String>,
    pub lcsc: Vec<String>,
    pub datasheet: Option<Datasheet>,
    pub license: String,
    pub description: Option<String>,
    pub footprints_kicad: Vec<String>,
    pub footprint_pinmap: Option<String>,
    pub pins_file: String,
    pub module: Option<Module>,
    pub behavioral: Option<Behavioral>,
    pub analog: Vec<View>,
    pub provenance: Option<Provenance>,
    pub tests_dir: Option<String>,
    /// bb-part/1 (ADR-009 D4): `[power]`.
    pub power: Option<Power>,
    /// bb-part/1 (ADR-012 D3): `[[document]]`.
    pub documents: Vec<Document>,
    /// bb-part/1 (ADR-012 D4): `[claims] file`, the package's `claims.toml`
    /// (`bb-claims/0`, milestone E2; not read by this build).
    pub claims_file: Option<String>,
}

/// A `pins.toml` parameter value (D4): a number with a unit, or a supply
/// pin reference scaled by a decimal factor (`VDD`, `0.7*VDD`), resolved at
/// elaboration against the bench.
#[derive(Clone, Debug, PartialEq)]
pub enum Param {
    /// Integer sub-units of the parameter's dimension.
    Quantity(i128),
    Supply {
        factor: Decimal,
        pin: String,
    },
    /// An identifier (`model`, `load` of `adc_in`).
    Name(String),
}

/// What a pin model's parameters are.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ParamKind {
    /// A quantity, or a supply reference when `supply_ok`.
    Q {
        dim: Dim,
        supply_ok: bool,
    },
    Name,
    /// A pin name of the part (`rail`, bb-pins/1, ADR-009 D5).
    Pin,
}

/// The pin models and their parameters (`spec/pin-model.md`): `(model,
/// [(param, kind, required)])`.
/// One parameter of a pin model: `(name, kind, required)`.
pub type ParamSpec = (&'static str, ParamKind, bool);

pub const PIN_MODELS: &[(&str, &[ParamSpec])] = &[
    (
        "digital_out",
        &[
            (
                "voh",
                ParamKind::Q {
                    dim: Dim::Voltage,
                    supply_ok: true,
                },
                true,
            ),
            (
                "vol",
                ParamKind::Q {
                    dim: Dim::Voltage,
                    supply_ok: true,
                },
                true,
            ),
            (
                "rise",
                ParamKind::Q {
                    dim: Dim::Time,
                    supply_ok: false,
                },
                true,
            ),
            (
                "fall",
                ParamKind::Q {
                    dim: Dim::Time,
                    supply_ok: false,
                },
                true,
            ),
            (
                "zout",
                ParamKind::Q {
                    dim: Dim::Resistance,
                    supply_ok: false,
                },
                true,
            ),
            // bb-pins/1 (ADR-009 D5): the rail a `pi` output drives from and
            // its high-Z leakage. Optional, and unused at `fast` (milestone
            // P2 implements the `pi` glue).
            ("rail", ParamKind::Pin, false),
            (
                "rhiz",
                ParamKind::Q {
                    dim: Dim::Resistance,
                    supply_ok: false,
                },
                false,
            ),
        ],
    ),
    (
        "digital_in",
        &[
            (
                "vih",
                ParamKind::Q {
                    dim: Dim::Voltage,
                    supply_ok: true,
                },
                true,
            ),
            (
                "vil",
                ParamKind::Q {
                    dim: Dim::Voltage,
                    supply_ok: true,
                },
                true,
            ),
            (
                "cin",
                ParamKind::Q {
                    dim: Dim::Capacitance,
                    supply_ok: false,
                },
                true,
            ),
        ],
    ),
    (
        "adc_in",
        &[
            (
                "vref",
                ParamKind::Q {
                    dim: Dim::Voltage,
                    supply_ok: true,
                },
                true,
            ),
            (
                "bits",
                ParamKind::Q {
                    dim: Dim::Count,
                    supply_ok: false,
                },
                true,
            ),
            ("model", ParamKind::Name, false),
            ("load", ParamKind::Name, false),
        ],
    ),
    // bb-pins/1 (ADR-009 D3, Amendment 11): a supply pin's ratings, each
    // optional (lint wants at least one). A `power_in`/`power_out` pin
    // takes the set named after it, else `default`; no binding chooses one,
    // since a supply pin never crosses an engine boundary.
    (
        "power",
        &[
            (
                "vmin",
                ParamKind::Q {
                    dim: Dim::Voltage,
                    supply_ok: false,
                },
                false,
            ),
            (
                "vmax",
                ParamKind::Q {
                    dim: Dim::Voltage,
                    supply_ok: false,
                },
                false,
            ),
            (
                "imax",
                ParamKind::Q {
                    dim: Dim::Current,
                    supply_ok: false,
                },
                false,
            ),
        ],
    ),
];

/// The `power` pin model (ADR-009 D3): the parameter set of the supply pin
/// `pin`, the set named after the pin if there is one, else `default`.
/// `Ok(None)` when the pin does not take the model.
#[allow(clippy::type_complexity)]
pub fn power_params(
    pkg: &Package,
    pin: &str,
) -> Result<Option<(String, BTreeMap<String, Param>)>, String> {
    let Some(p) = pkg.pins.by_name(pin) else {
        return Ok(None);
    };
    if !p.models.iter().any(|m| m == POWER_MODEL) {
        return Ok(None);
    }
    let named = pkg
        .pins
        .defaults
        .get(POWER_MODEL)
        .is_some_and(|s| s.contains_key(pin));
    let set = if named { pin } else { "default" };
    param_set(pkg, POWER_MODEL, Some(set)).map(|m| Some((set.to_string(), m)))
}

pub fn pin_model_params(model: &str) -> Option<&'static [ParamSpec]> {
    PIN_MODELS
        .iter()
        .find(|(m, _)| *m == model)
        .map(|(_, p)| *p)
}

/// Parse one parameter string of `kind`.
pub fn parse_param(text: &str, kind: ParamKind) -> Result<Param, String> {
    match kind {
        ParamKind::Name => {
            if text.is_empty() || !text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err(format!("{text:?}: expected an identifier"));
            }
            Ok(Param::Name(text.to_string()))
        }
        ParamKind::Pin => {
            if !is_pin_name(text) {
                return Err(format!("{text:?}: expected a pin name"));
            }
            Ok(Param::Name(text.to_string()))
        }
        ParamKind::Q { dim, supply_ok } => {
            let t = text.trim();
            let starts_num =
                t.starts_with(|c: char| c.is_ascii_digit() || matches!(c, '.' | '-' | '+'));
            if supply_ok {
                // <factor>*<PIN> or <PIN>
                if let Some((f, pin)) = t.split_once('*') {
                    let factor = crate::value::parse_number(f.trim())
                        .map_err(|e| format!("{text:?}: factor: {e}"))?;
                    let pin = pin.trim();
                    if !is_pin_name(pin) {
                        return Err(format!("{text:?}: {pin:?} is not a supply pin name"));
                    }
                    return Ok(Param::Supply {
                        factor,
                        pin: pin.to_string(),
                    });
                }
                if !starts_num && is_pin_name(t) {
                    return Ok(Param::Supply {
                        factor: Decimal { mant: 1, exp: 0 },
                        pin: t.to_string(),
                    });
                }
            }
            parse_quantity(t, dim)
                .map(Param::Quantity)
                .map_err(|e| e.to_string())
        }
    }
}

fn is_pin_name(s: &str) -> bool {
    !s.is_empty()
        && s.starts_with(|c: char| c.is_ascii_alphabetic())
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '+' | '-'))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pin {
    pub number: String,
    pub name: String,
    pub role: String,
    pub supply: Option<String>,
    pub alt: Vec<String>,
    pub models: Vec<String>,
    pub adc_channel: Option<String>,
}

/// `pins.toml`.
#[derive(Clone, Debug, PartialEq)]
pub struct Pins {
    pub schema: String,
    pub pins: Vec<Pin>,
    /// `[defaults.<model>.<set>]`: model -> set -> [(param, text)].
    pub defaults: BTreeMap<String, BTreeMap<String, Vec<(String, String)>>>,
    /// bb-pins/1 (ADR-009 D3): `limits`.
    pub limits: Vec<Limit>,
}

impl Pins {
    pub fn by_name(&self, name: &str) -> Option<&Pin> {
        self.pins.iter().find(|p| p.name == name)
    }
}

/// A loaded package directory.
#[derive(Clone, Debug)]
pub struct Package {
    pub dir: PathBuf,
    pub part: Part,
    pub pins: Pins,
    /// Migration hints (a previous-major file).
    pub notes: Vec<String>,
}

fn check_id(id: &str) -> Result<(), String> {
    let ok_seg = |s: &str| {
        !s.is_empty()
            && s.chars().all(|c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-')
            })
    };
    match id.split_once('/') {
        Some((ns, name)) if ok_seg(ns) && ok_seg(name) && !name.contains('/') => Ok(()),
        _ => Err(format!(
            "part.id {id:?}: expected <namespace>/<name>, lower case [a-z0-9._-]"
        )),
    }
}

fn check_semver(v: &str) -> Result<(), String> {
    let parts: Vec<&str> = v.split('.').collect();
    let ok = parts.len() == 3
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.chars().all(|c| c.is_ascii_digit())
                && (p.len() == 1 || !p.starts_with('0'))
        });
    if ok {
        Ok(())
    } else {
        Err(format!("part.version {v:?}: expected MAJOR.MINOR.PATCH"))
    }
}

fn one_of(what: &str, v: &str, allowed: &[&str]) -> Result<(), String> {
    if allowed.contains(&v) {
        Ok(())
    } else {
        Err(format!(
            "{what} {v:?}: expected one of {}",
            allowed.join(", ")
        ))
    }
}

fn provenance(mut o: Obj, major: u32) -> Result<Provenance, String> {
    let p = Provenance {
        source: o.str_req("source")?,
        base: o.str_opt("base")?,
        refs: o.strs("refs")?,
        deviations: o.strs("deviations")?,
        not_modelled: o.strs("not_modelled")?,
        origin: o.str_opt("origin")?,
        generated_by: o.str_opt("generated_by")?,
        reviewed_by: o.strs("reviewed_by")?,
        confidence: o.str_req("confidence")?,
        upstream_license: o.str_opt("upstream_license")?,
    };
    let path = o.path.clone();
    o.done()?;
    one_of(&format!("{path}.source"), &p.source, SOURCES)?;
    one_of(&format!("{path}.confidence"), &p.confidence, CONFIDENCES)?;
    if p.upstream_license.is_some() && major < 1 {
        return Err(format!(
            "{path}.upstream_license: new in bb-part/1 (ADR-012); run `bb pkg migrate`"
        ));
    }
    Ok(p)
}

/// A key new in bb-part/1, met in a bb-part/0 file.
fn new_in_part_1(major: u32, at: &str, what: &str) -> Result<(), String> {
    if major < 1 {
        return Err(format!(
            "{at}: new in bb-part/1 ({what}); run `bb pkg migrate`"
        ));
    }
    Ok(())
}

/// A CPU state name (`#<state>`, ADR-009 D4) or an idd state.
fn is_state_name(s: &str) -> bool {
    !s.is_empty()
        && s.starts_with(|c: char| c.is_ascii_lowercase())
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn rungs(what: &str, v: &[String]) -> Result<(), String> {
    for r in v {
        one_of(what, r, RUNGS)?;
    }
    Ok(())
}

/// The subcircuit every `switch` view instantiates (ADR-010 D2): one B
/// conductance between the two contact ports, `ctl` an `EXTERNAL` 0/1
/// source the bench drives (0: open, `roff`; 1: closed, `ron`). Generated by
/// `bb`, never read from a file; [`switch_definition`] gives its text.
pub const SWITCH_SUBCKT: &str = "bb_switch";

/// The `switch` view kind's default parameters (ADR-010 D2): 50 mΩ closed,
/// 100 MΩ open, so both states keep a DC conductance (ADR-009 D2 rule 2).
pub const SWITCH_RON: &str = "50m";
pub const SWITCH_ROFF: &str = "100Meg";

/// The parameters a `supply_load` view's instance takes from the package
/// (ADR-009 D4, milestone P3): `i_<state>` = `[power] idd.<state>.typ` for
/// each of the view's state ports, and `vdd_min` = `[power] vdd_min` when
/// the package gives one; then the view's own `params`, which may restate or
/// override them by name (a view, its corners or a vector can set a current
/// the table does not give). Value-grammar strings, in that order, each name
/// once.
pub fn supply_load_params(pkg: &Package, view: &View) -> Result<Vec<(String, String)>, String> {
    let power = pkg.part.power.as_ref();
    let mut out = Vec::new();
    for p in &view.ports {
        if let Port::State { state } = p {
            let idd = power
                .and_then(|w| w.idd.iter().find(|i| &i.state == state))
                .ok_or_else(|| {
                    format!(
                        "view {}: state port {state} has no [power] idd.{state} (ADR-009 D4)",
                        view.name
                    )
                })?;
            out.push((format!("i_{state}"), idd.typ.clone()));
        }
    }
    if let Some(v) = power.and_then(|w| w.vdd_min.as_ref()) {
        out.push(("vdd_min".into(), v.clone()));
    }
    for (k, v) in &view.params {
        match out.iter_mut().find(|(x, _)| x == k) {
            Some(e) => e.1 = v.clone(),
            None => out.push((k.clone(), v.clone())),
        }
    }
    Ok(out)
}

/// The definition of [`SWITCH_SUBCKT`] (ADR-010 D2), in the deck's view
/// section like any view definition: `I = (V(a) - V(b)) * (1/roff +
/// V(ctl) * (1/ron - 1/roff))`, with no branch-current unknown.
pub fn switch_definition() -> String {
    format!(
        "* bb: the switch view kind (ADR-010 D2): ctl 0 = open (roff), 1 = closed (ron)\n\
         .subckt {SWITCH_SUBCKT} a b ctl ron=0.05 roff=1e8\n\
         B1 a b I = (V(a) - V(b)) * (1 / {{roff}} + V(ctl) * (1 / {{ron}} - 1 / {{roff}}))\n\
         .ends {SWITCH_SUBCKT}\n"
    )
}

/// A `switch` view's `ron` and `roff` (µΩ): its params or the defaults.
pub fn switch_params(v: &View) -> Result<(i128, i128), String> {
    let get = |k: &str, d: &str| -> Result<i128, String> {
        let text = v
            .params
            .iter()
            .find(|(n, _)| n == k)
            .map_or(d, |(_, x)| x.as_str());
        parse_quantity(text, Dim::Resistance)
            .map_err(|e| format!("view {} params.{k}: {e}", v.name))
    };
    let (ron, roff) = (get("ron", SWITCH_RON)?, get("roff", SWITCH_ROFF)?);
    if ron <= 0 || roff <= ron {
        return Err(format!(
            "view {}: a switch needs 0 < ron < roff (ADR-010 D2)",
            v.name
        ));
    }
    Ok((ron, roff))
}

/// A `switch` view's `bounce_max` (fs), if it declares one.
pub fn switch_bounce_max(v: &View) -> Result<Option<i128>, String> {
    v.params
        .iter()
        .find(|(n, _)| n == "bounce_max")
        .map(|(_, x)| {
            parse_quantity(x, Dim::Time)
                .map_err(|e| format!("view {} params.bounce_max: {e}", v.name))
        })
        .transpose()
}

fn view(mut o: Obj, major: u32) -> Result<View, String> {
    let path = o.path.clone();
    let kind = o.str_opt("kind")?.unwrap_or_else(|| "circuit".into());
    // The switch (ADR-010 D2) and supply_load (ADR-009 D4) view kinds are
    // new in bb-part/1.
    match kind.as_str() {
        "switch" => new_in_part_1(major, &format!("{path}.kind \"switch\""), "ADR-010 D2")?,
        "supply_load" => {
            new_in_part_1(major, &format!("{path}.kind \"supply_load\""), "ADR-009 D4")?
        }
        "pof_comparator" => new_in_part_1(
            major,
            &format!("{path}.kind \"pof_comparator\""),
            "ADR-009 D6",
        )?,
        _ => {}
    }
    one_of(&format!("{path}.kind"), &kind, VIEW_KINDS)?;
    let mut ports = Vec::new();
    match o.get("ports") {
        None => return Err(format!("{path}.ports: required")),
        Some(toml::Value::Array(a)) => {
            for (i, v) in a.iter().enumerate() {
                let at = format!("{path}.ports[{i}]");
                match (kind.as_str(), v) {
                    ("circuit", toml::Value::String(s)) => ports.push(Port::Pin(s.clone())),
                    // ADR-010 D2: a switch's two contacts are typed `pin`
                    // ports, as a pin_load view has.
                    ("switch", toml::Value::Table(t)) => {
                        let mut po = Obj::new(at.clone(), t);
                        let role = po.str_req("role")?;
                        if role != "pin" {
                            return Err(format!(
                                "{at}.role {role:?}: a switch view's ports are its two contacts, \
                                 role = \"pin\" (ADR-010 D2)"
                            ));
                        }
                        ports.push(Port::OtherPin {
                            pin: po.str_req("pin")?,
                        });
                        po.done()?;
                    }
                    ("switch", _) => {
                        return Err(format!(
                            "{at}: a switch view's ports are typed tables {{ role = \"pin\", pin = \"<name>\" }} (ADR-010 D2)"
                        ))
                    }
                    // ADR-009 D4: a supply_load view's ports are the
                    // package's supply pins and its CPU state ports.
                    ("supply_load", toml::Value::Table(t)) => {
                        let mut po = Obj::new(at.clone(), t);
                        let role = po.str_req("role")?;
                        let port = match role.as_str() {
                            "pin" => Port::OtherPin {
                                pin: po.str_req("pin")?,
                            },
                            "state" => {
                                let state = po.str_req("state")?;
                                if !is_state_name(&state) {
                                    return Err(format!(
                                        "{at}.state {state:?}: a state name is lower case \
                                         [a-z0-9_] (its node is #<state>, ADR-009 D4)"
                                    ));
                                }
                                Port::State { state }
                            }
                            r => {
                                return Err(format!(
                                    "{at}.role {r:?}: a supply_load view's ports are role \"pin\" \
                                     or \"state\" (ADR-009 D4)"
                                ))
                            }
                        };
                        po.done()?;
                        ports.push(port);
                    }
                    ("supply_load", _) => {
                        return Err(format!(
                            "{at}: a supply_load view's ports are typed tables (ADR-009 D4)"
                        ))
                    }
                    // ADR-009 D6 (Phase 2 P4): the power-fail comparator's
                    // ports are the package's supply pins, the threshold
                    // code's state ports and its output.
                    ("pof_comparator", toml::Value::Table(t)) => {
                        let mut po = Obj::new(at.clone(), t);
                        let role = po.str_req("role")?;
                        let port = match role.as_str() {
                            "pin" => Port::OtherPin {
                                pin: po.str_req("pin")?,
                            },
                            "state" => Port::State {
                                state: po.str_req("state")?,
                            },
                            "observe" => Port::Observe {
                                signal: po.str_req("signal")?,
                            },
                            r => {
                                return Err(format!(
                                    "{at}.role {r:?}: a pof_comparator view's ports are role \
                                     \"pin\", \"state\" or \"observe\" (ADR-009 D6)"
                                ))
                            }
                        };
                        po.done()?;
                        ports.push(port);
                    }
                    ("pof_comparator", _) => {
                        return Err(format!(
                            "{at}: a pof_comparator view's ports are typed tables (ADR-009 D6)"
                        ))
                    }
                    ("pin_load", toml::Value::Table(t)) => {
                        let mut po = Obj::new(at.clone(), t);
                        let role = po.str_req("role")?;
                        let port = match role.as_str() {
                            "pad" => Port::Pad,
                            "ctrl" => Port::Ctrl {
                                signal: po.str_req("signal")?,
                            },
                            "observe" => Port::Observe {
                                signal: po.str_req("signal")?,
                            },
                            "pin" => Port::OtherPin {
                                pin: po.str_req("pin")?,
                            },
                            r => {
                                return Err(format!(
                                    "{at}.role {r:?}: expected pad, ctrl, observe or pin"
                                ))
                            }
                        };
                        po.done()?;
                        ports.push(port);
                    }
                    ("circuit", _) => {
                        return Err(format!(
                            "{at}: a circuit view's ports are pin names (strings)"
                        ))
                    }
                    _ => {
                        return Err(format!(
                            "{at}: a pin_load view's ports are typed tables (D4)"
                        ))
                    }
                }
            }
        }
        Some(v) => {
            return Err(format!(
                "{path}.ports: expected an array, got {}",
                v.type_name()
            ))
        }
    }
    let qualified = o
        .tables("qualified")?
        .into_iter()
        .map(|mut q| {
            let r = Qualified {
                lattice: q.str_req("lattice")?,
                options: q.str_req("options")?,
            };
            q.done()?;
            Ok(r)
        })
        .collect::<Result<Vec<_>, String>>()?;
    // bb-part/1 (ADR-009 D8): named corners, parameter overrides with a ref.
    let mut corners = Vec::new();
    for mut c in o.tables("corners")? {
        new_in_part_1(major, &c.path, "ADR-009 D8")?;
        let at = c.path.clone();
        let corner = Corner {
            name: c.str_req("name")?,
            params: c.str_map("params")?,
            reference: c.str_req("ref")?,
        };
        c.done()?;
        if !corner_name_ok(&corner.name) {
            return Err(format!(
                "{at}.name {:?}: a corner is typ, min, max, suite_compat or worst_<x> (ADR-009 D8)",
                corner.name
            ));
        }
        corners.push(corner);
    }
    let rails = o.strs("rails")?;
    if !rails.is_empty() {
        new_in_part_1(major, &format!("{path}.rails"), "ADR-009")?;
    }
    // A switch view's element is generated (switch_definition): it has no
    // file and no subckt of its own.
    let (file, subckt) = if kind == "switch" {
        if o.has("file") || o.has("subckt") {
            return Err(format!(
                "{path}: a switch view has no file or subckt; bb generates its element \
                 (ADR-010 D2)"
            ));
        }
        (String::new(), SWITCH_SUBCKT.to_string())
    } else {
        (o.str_req("file")?, o.str_req("subckt")?)
    };
    let v = View {
        name: o.str_req("name")?,
        kind,
        purpose: o.str_opt("purpose")?,
        fidelity: o.strs("fidelity")?,
        default_for: o.strs("default_for")?,
        file,
        subckt,
        ports,
        params: o.str_map("params")?,
        qualified,
        provenance: o
            .table_opt("provenance")?
            .map(|p| provenance(p, major))
            .transpose()?,
        rails,
        corners,
    };
    o.done()?;
    if v.kind == "switch" {
        if v.ports.len() != 2 {
            return Err(format!(
                "{path}.ports: a switch view has exactly two contact ports (ADR-010 D2)"
            ));
        }
        if let Some((k, _)) = v
            .params
            .iter()
            .find(|(k, _)| !matches!(k.as_str(), "ron" | "roff" | "bounce_max"))
        {
            return Err(format!(
                "{path}.params.{k}: a switch view takes ron, roff and bounce_max (ADR-010 D2)"
            ));
        }
        switch_params(&v).map_err(|e| format!("{path}: {e}"))?;
        switch_bounce_max(&v).map_err(|e| format!("{path}: {e}"))?;
    }
    if v.fidelity.is_empty() {
        return Err(format!(
            "{path}.fidelity: required (the rungs this view is valid for)"
        ));
    }
    rungs(&format!("{path}.fidelity"), &v.fidelity)?;
    rungs(&format!("{path}.default_for"), &v.default_for)?;
    Ok(v)
}

/// Parse `part.toml` text (schema checked against `reg`).
pub fn parse_part(text: &str, reg: &Registry) -> Result<(Part, Option<String>), String> {
    let t = toml::parse(text).map_err(|e| format!("part.toml: {e}"))?;
    let mut o = Obj::new("", &t);
    o.reserved(RESERVED_TOP, RESERVED_FOR)?;
    let schema = o.str_req("schema")?;
    let note = match reg.check("bb-part", &schema)? {
        Version::Current => None,
        Version::Previous { hint } => Some(hint),
    };
    let major = crate::schema::SchemaId::parse(&schema)?.major;
    let mut p = o
        .table_opt("part")?
        .ok_or_else(|| "[part]: required".to_string())?;
    let datasheet = match p.table_opt("datasheet")? {
        Some(mut d) => {
            let r = Datasheet {
                url: d.str_req("url")?,
                rev: d.str_opt("rev")?,
            };
            d.done()?;
            Some(r)
        }
        None => None,
    };
    let id = p.str_req("id")?;
    let version = p.str_req("version")?;
    let kind = p.str_req("kind")?;
    let manufacturer = p.str_opt("manufacturer")?;
    let mpn = p.strs("mpn")?;
    let mpn_patterns = p.strs("mpn_patterns")?;
    let lcsc = p.strs("lcsc")?;
    let license = p.str_req("license")?;
    let description = p.str_opt("description")?;
    p.done()?;
    check_id(&id)?;
    check_semver(&version)?;
    one_of("part.kind", &kind, KINDS)?;
    let (footprints_kicad, footprint_pinmap) = match o.table_opt("footprints")? {
        Some(mut f) => {
            let r = (f.strs("kicad")?, f.str_opt("pinmap")?);
            f.done()?;
            r
        }
        None => (Vec::new(), None),
    };
    let mut pins = o
        .table_opt("pins")?
        .ok_or_else(|| "[pins]: required (file = \"pins.toml\")".to_string())?;
    pins.reserved(&["ibis"], RESERVED_FOR)?;
    let pins_file = pins.str_req("file")?;
    pins.done()?;
    let module = match o.table_opt("module")? {
        Some(mut m) => {
            let r = Module {
                chip: m.str_req("chip")?,
                pinmap: m.str_req("pinmap")?,
            };
            m.done()?;
            check_id(&r.chip).map_err(|e| format!("module.chip: {e}"))?;
            Some(r)
        }
        None => None,
    };
    if (kind == "module") != module.is_some() {
        return Err("[module] is required for kind = \"module\" and only for it".into());
    }
    let behavioral = match o.table_opt("behavioral")? {
        Some(mut b) => {
            b.reserved(&["cpu"], RESERVED_FOR)?;
            let svd = match b.table_opt("svd")? {
                Some(mut v) => {
                    new_in_part_1(major, "behavioral.svd", "ADR-011 D5")?;
                    let r = VendorRef {
                        url: v.str_req("url")?,
                        sha256: v.str_req("sha256")?,
                        license: v.str_req("license")?,
                    };
                    v.done()?;
                    if !r.url.starts_with("https://") {
                        return Err(format!(
                            "behavioral.svd.url {:?}: a vendor file is fetched over https:// only \
                             (ADR-011 D4)",
                            r.url
                        ));
                    }
                    if !(r.sha256.len() == 64
                        && r.sha256
                            .bytes()
                            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)))
                    {
                        return Err(
                            "behavioral.svd.sha256: expected 64 lower-case hex digits".into()
                        );
                    }
                    if r.license.trim().is_empty() || r.license.contains(char::is_whitespace) {
                        return Err("behavioral.svd.license: one SPDX licence id".into());
                    }
                    Some(r)
                }
                None => None,
            };
            let r = Behavioral {
                engine: b.str_req("engine")?,
                platform: b.str_opt("platform")?,
                device: b.str_opt("device")?,
                svd,
            };
            b.done()?;
            if !ENGINES.contains(&r.engine.as_str()) {
                return Err(format!(
                    "behavioral.engine {:?}: Phase 1 accepts renode or device; another engine needs an ADR",
                    r.engine
                ));
            }
            match (r.engine.as_str(), &r.platform, &r.device) {
                ("renode", Some(p), None) => check_engine_id("behavioral.platform", p)?,
                ("device", None, Some(d)) => check_engine_id("behavioral.device", d)?,
                ("renode", ..) => {
                    return Err("behavioral: engine renode needs platform (and no device)".into())
                }
                _ => return Err("behavioral: engine device needs device (and no platform)".into()),
            }
            Some(r)
        }
        None => None,
    };
    let analog = o
        .tables("analog")?
        .into_iter()
        .map(|v| view(v, major))
        .collect::<Result<Vec<_>, String>>()?;
    let provenance = o
        .table_opt("provenance")?
        .map(|p| provenance(p, major))
        .transpose()?;
    let tests_dir = match o.table_opt("tests")? {
        Some(mut t) => {
            let d = t.str_req("dir")?;
            t.done()?;
            Some(d)
        }
        None => None,
    };
    // bb-part/1 (ADR-009 D4, D5): [power].
    let power = match o.table_opt("power")? {
        Some(mut pw) => {
            new_in_part_1(major, "power", "ADR-009 D4")?;
            let mut idd = Vec::new();
            if let Some(mut it) = pw.table_opt("idd")? {
                for s in it.keys() {
                    let mut so = it
                        .table_opt(&s)?
                        .ok_or_else(|| format!("power.idd.{s}: expected a table"))?;
                    if !is_state_name(&s) {
                        return Err(format!(
                            "power.idd.{s}: a state name is lower case [a-z0-9_] (ADR-009 D4)"
                        ));
                    }
                    let e = Idd {
                        state: s.clone(),
                        typ: so.str_req("typ")?,
                        max: so.str_opt("max")?,
                        reference: so.str_req("ref")?,
                    };
                    so.done()?;
                    for (k, v) in [("typ", Some(&e.typ)), ("max", e.max.as_ref())] {
                        if let Some(v) = v {
                            parse_quantity(v, Dim::Current)
                                .map_err(|x| format!("power.idd.{s}.{k}: {x}"))?;
                        }
                    }
                    idd.push(e);
                }
                it.done()?;
            }
            let r = Power {
                idd,
                vdd_min: pw.str_opt("vdd_min")?,
                io_rail: pw.str_opt("io_rail")?,
            };
            pw.done()?;
            if let Some(v) = &r.vdd_min {
                parse_quantity(v, Dim::Voltage).map_err(|x| format!("power.vdd_min: {x}"))?;
            }
            if let Some(p) = &r.io_rail {
                if !is_pin_name(p) {
                    return Err(format!("power.io_rail {p:?}: expected a pin name"));
                }
            }
            Some(r)
        }
        None => None,
    };
    // bb-part/1 (ADR-012 D3, D4): [[document]] and [claims] file.
    let mut documents = Vec::new();
    for mut d in o.tables("document")? {
        new_in_part_1(major, &d.path, "ADR-012 D3")?;
        let at = d.path.clone();
        let doc = Document {
            id: d.str_req("id")?,
            title: d.str_opt("title")?,
            revision: d.str_opt("revision")?,
            url: d.str_opt("url")?,
            sha256: d.str_req("sha256")?,
            fetched_by: d.str_opt("fetched_by")?,
        };
        d.done()?;
        if doc.id.is_empty()
            || !doc.id.chars().all(|c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-')
            })
        {
            return Err(format!(
                "{at}.id {:?}: lower case [a-z0-9._-] (ADR-012 D3)",
                doc.id
            ));
        }
        if doc.sha256.len() != 64
            || !doc
                .sha256
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err(format!(
                "{at}.sha256: 64 lower-case hex digits, the document's identity (ADR-012 D3)"
            ));
        }
        if let Some(f) = &doc.fetched_by {
            let ok = ["agent:", "user:"]
                .iter()
                .any(|p| f.strip_prefix(p).is_some_and(|w| !w.is_empty()));
            if !ok {
                return Err(format!(
                    "{at}.fetched_by {f:?}: expected agent:<who> or user:<who> (ADR-012 D3)"
                ));
            }
        }
        if documents.iter().any(|x: &Document| x.id == doc.id) {
            return Err(format!("{at}.id {:?}: used twice", doc.id));
        }
        documents.push(doc);
    }
    let claims_file = match o.table_opt("claims")? {
        Some(mut c) => {
            new_in_part_1(major, "claims", "ADR-012 D4")?;
            let f = c.str_req("file")?;
            c.done()?;
            Some(f)
        }
        None => None,
    };
    o.done()?;
    Ok((
        Part {
            schema,
            id,
            version,
            kind,
            manufacturer,
            mpn,
            mpn_patterns,
            lcsc,
            datasheet,
            license,
            description,
            footprints_kicad,
            footprint_pinmap,
            pins_file,
            module,
            behavioral,
            analog,
            provenance,
            tests_dir,
            power,
            documents,
            claims_file,
        },
        note,
    ))
}

/// `<name>@<major>`, e.g. `stm32f103-offline@1`.
fn check_engine_id(what: &str, s: &str) -> Result<(), String> {
    match s.split_once('@') {
        Some((n, m))
            if !n.is_empty()
                && n.chars().all(|c| {
                    c.is_ascii_lowercase()
                        || c.is_ascii_digit()
                        || matches!(c, '-' | '_' | '/' | '.')
                })
                && !m.is_empty()
                && m.chars().all(|c| c.is_ascii_digit()) =>
        {
            Ok(())
        }
        _ => Err(format!("{what} {s:?}: expected <name>@<major>")),
    }
}

/// Parse `pins.toml` text.
pub fn parse_pins(text: &str, reg: &Registry) -> Result<(Pins, Option<String>), String> {
    let t = toml::parse(text).map_err(|e| format!("pins.toml: {e}"))?;
    let mut o = Obj::new("", &t);
    let schema = o.str_req("schema")?;
    let note = match reg.check("bb-pins", &schema)? {
        Version::Current => None,
        Version::Previous { hint } => Some(hint),
    };
    let major = crate::schema::SchemaId::parse(&schema)?.major;
    // bb-pins/1 (ADR-009 D3): relational limits for absolute maximum ratings.
    let mut limits = Vec::new();
    for mut l in o.tables("limits")? {
        let at = l.path.clone();
        if major < 1 {
            return Err(format!(
                "{at}: limits are new in bb-pins/1 (ADR-009 D3); run `bb pkg migrate`"
            ));
        }
        let lim = Limit {
            pin: l.str_req("pin")?,
            min: l.str_opt("min")?,
            max: l.str_opt("max")?,
            source: l.str_req("source")?,
        };
        l.done()?;
        if lim.min.is_none() && lim.max.is_none() {
            return Err(format!("{at}: a limit needs min, max or both"));
        }
        for (k, v) in [("min", &lim.min), ("max", &lim.max)] {
            if let Some(v) = v {
                parse_bound(v).map_err(|e| format!("{at}.{k}: {e}"))?;
            }
        }
        limits.push(lim);
    }
    let mut pins = Vec::new();
    for mut p in o.tables("pin")? {
        let path = p.path.clone();
        let adc_channel = match p.table_opt("adc")? {
            Some(mut a) => {
                let c = a.str_req("channel")?;
                a.done()?;
                Some(c)
            }
            None => None,
        };
        let pin = Pin {
            number: p.str_req("number")?,
            name: p.str_req("name")?,
            role: p.str_req("role")?,
            supply: p.str_opt("supply")?,
            alt: p.strs("alt")?,
            models: p.strs("models")?,
            adc_channel,
        };
        p.done()?;
        one_of(&format!("{path}.role"), &pin.role, ROLES)?;
        for m in &pin.models {
            if m == POWER_MODEL && major < 1 {
                return Err(format!("{path}.models: {POWER_MODEL_NEW}"));
            }
            if pin_model_params(m).is_none() {
                return Err(format!(
                    "{path}.models: {m:?} is not a pin model (digital_out, digital_in, adc_in, \
                     power)"
                ));
            }
        }
        pins.push(pin);
    }
    let mut defaults = BTreeMap::new();
    if let Some(d) = o.get("defaults") {
        let d = d
            .as_table()
            .ok_or_else(|| "defaults: expected a table".to_string())?;
        for (model, sets) in d {
            if model == POWER_MODEL && major < 1 {
                return Err(format!("defaults.{model}: {POWER_MODEL_NEW}"));
            }
            if pin_model_params(model).is_none() {
                return Err(format!("defaults.{model}: not a pin model"));
            }
            let sets = sets
                .as_table()
                .ok_or_else(|| format!("defaults.{model}: expected tables of parameter sets"))?;
            let mut m = BTreeMap::new();
            for (set, params) in sets {
                let mut po = Obj::new(
                    format!("defaults.{model}.{set}"),
                    params.as_table().ok_or_else(|| {
                        format!("defaults.{model}.{set}: expected a table of parameters")
                    })?,
                );
                let kv = {
                    let all: Vec<String> = params.as_table().unwrap().keys().cloned().collect();
                    let mut kv = Vec::new();
                    for k in all {
                        if major < 1 && PINS_1_PARAMS.contains(&k.as_str()) {
                            return Err(format!(
                                "defaults.{model}.{set}.{k}: new in bb-pins/1 (ADR-009 D5); run \
                                 `bb pkg migrate`"
                            ));
                        }
                        kv.push((k.clone(), po.str_req(&k)?));
                    }
                    kv
                };
                po.done()?;
                m.insert(set.clone(), kv);
            }
            defaults.insert(model.clone(), m);
        }
    }
    o.done()?;
    Ok((
        Pins {
            schema,
            pins,
            defaults,
            limits,
        },
        note,
    ))
}

/// The supply pin model of `bb-pins/1` (ADR-009 D3, Amendment 11:
/// `power{vmin, vmax, imax}`, from `[defaults.power.<pin or default>]`).
pub const POWER_MODEL: &str = "power";
const POWER_MODEL_NEW: &str = "the power pin model is new in bb-pins/1 (ADR-009 D3); run `bb pkg \
                               migrate`";

/// The `digital_out` parameters new in `bb-pins/1` (ADR-009 D5).
pub const PINS_1_PARAMS: &[&str] = &["rail", "rhiz"];

/// Load the package in `dir` (its `part.toml` and the `pins.toml` it names).
pub fn load(dir: &Path, reg: &Registry) -> Result<Package, String> {
    let read =
        |p: &Path| std::fs::read_to_string(p).map_err(|e| format!("read {}: {e}", p.display()));
    let part_path = dir.join("part.toml");
    let (part, n1) =
        parse_part(&read(&part_path)?, reg).map_err(|e| format!("{}: {e}", part_path.display()))?;
    let pins_path = dir.join(&part.pins_file);
    let (pins, n2) =
        parse_pins(&read(&pins_path)?, reg).map_err(|e| format!("{}: {e}", pins_path.display()))?;
    Ok(Package {
        dir: dir.to_path_buf(),
        part,
        pins,
        notes: n1.into_iter().chain(n2).collect(),
    })
}

/// A module's pad -> chip pin map (`[module] pinmap`, `spec/package.md`
/// section 4): a CSV file with the header `pad,chip_pin` and one row per
/// module pad (the module's pin number, then the chip package's pin name).
/// Returns the rows in file order; the cross-checks against the module's
/// and the chip's pins are lint's ([`crate::lint`]).
pub fn module_pinmap(pkg: &Package) -> Result<Vec<(String, String)>, String> {
    let m = pkg
        .part
        .module
        .as_ref()
        .ok_or_else(|| format!("{}: not a module", pkg.part.id))?;
    let path = pkg.dir.join(&m.pinmap);
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("pad,chip_pin") {
        return Err(format!(
            "{}: the first line must be the header pad,chip_pin",
            m.pinmap
        ));
    }
    let mut out = Vec::new();
    for (i, l) in lines.enumerate() {
        if l.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = l.split(',').map(str::trim).collect();
        if f.len() != 2 || f[0].is_empty() || f[1].is_empty() {
            return Err(format!(
                "{} line {}: expected <pad>,<chip pin>",
                m.pinmap,
                i + 2
            ));
        }
        out.push((f[0].to_string(), f[1].to_string()));
    }
    Ok(out)
}

/// Every package directory (one with a `part.toml`) under `root`, sorted.
/// A package directory is not searched further (its `tests/` are not
/// packages).
pub fn discover(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    fn walk(d: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
        if d.join("part.toml").is_file() {
            out.push(d.to_path_buf());
            return Ok(());
        }
        let mut entries: Vec<PathBuf> = std::fs::read_dir(d)
            .map_err(|e| format!("read {}: {e}", d.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.is_dir()
                    && !p
                        .file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with('.'))
            })
            .collect();
        entries.sort();
        for e in entries {
            walk(&e, out)?;
        }
        Ok(())
    }
    walk(root, &mut out)?;
    Ok(out)
}

/// The resolved parameters of one pin binding: the package's parameter set
/// `set` (or `default`) for `model`, each parsed.
pub fn param_set(
    pkg: &Package,
    model: &str,
    set: Option<&str>,
) -> Result<BTreeMap<String, Param>, String> {
    let spec = pin_model_params(model).ok_or_else(|| format!("{model}: not a pin model"))?;
    let sets = pkg.pins.defaults.get(model);
    let name = set.unwrap_or("default");
    let kv = sets.and_then(|s| s.get(name)).ok_or_else(|| {
        format!(
            "{}: no parameter set defaults.{model}.{name} in pins.toml{}",
            pkg.part.id,
            match sets {
                Some(s) if !s.is_empty() => format!(
                    " (sets: {})",
                    s.keys().cloned().collect::<Vec<_>>().join(", ")
                ),
                _ => String::new(),
            }
        )
    })?;
    let mut out = BTreeMap::new();
    for (k, v) in kv {
        let kind = spec
            .iter()
            .find(|(p, ..)| p == k)
            .map(|(_, kind, _)| *kind)
            .ok_or_else(|| format!("defaults.{model}.{name}.{k}: not a parameter of {model}"))?;
        out.insert(
            k.clone(),
            parse_param(v, kind).map_err(|e| format!("defaults.{model}.{name}.{k}: {e}"))?,
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PART: &str = r#"
schema = "bb-part/0"
[part]
id = "ti/tps22917"
version = "0.1.0"
kind = "analog_ic"
mpn = ["TPS22917DBVR"]
license = "Apache-2.0"
datasheet = { url = "https://www.ti.com/lit/ds/symlink/tps22917.pdf", rev = "SLVSDW8B" }
[footprints]
kicad = ["Package_TO_SOT_SMD:SOT-23-6"]
[pins]
file = "pins.toml"
[[analog]]
name = "lsw"
fidelity = ["fast", "pi"]
default_for = ["fast"]
file = "analog/x.lib"
subckt = "X"
ports = ["VIN", "GND"]
params = { ron_add = "0" }
[[analog.qualified]]
lattice = "10us"
options = "phase0"
[analog.provenance]
source = "datasheet"
confidence = "datasheet_checked"
"#;

    #[test]
    fn parses_a_part() {
        // bb-part/0 is the previous major (B1's draft of the epoch): read,
        // with the migration hint as a note.
        let (p, note) = parse_part(PART, &Registry::current()).unwrap();
        assert!(note.unwrap().contains("bb pkg migrate"));
        let (p1, note) = parse_part(
            &PART.replacen("bb-part/0", "bb-part/1", 1),
            &Registry::current(),
        )
        .unwrap();
        assert!(note.is_none());
        assert_eq!(p1.analog, p.analog);
        assert_eq!(p.id, "ti/tps22917");
        assert_eq!(
            p.analog[0].ports,
            vec![Port::Pin("VIN".into()), Port::Pin("GND".into())]
        );
        assert_eq!(p.analog[0].qualified[0].options, "phase0");
        assert_eq!(p.datasheet.unwrap().rev.as_deref(), Some("SLVSDW8B"));
    }

    #[test]
    fn strict_keys_reserved_keys_and_enums() {
        let reg = Registry::current();
        let with = |from: &str, to: &str| PART.replacen(from, to, 1);
        let e = parse_part(
            &with(
                "kind = \"analog_ic\"",
                "kind = \"analog_ic\"\ncolour = \"red\"",
            ),
            &reg,
        )
        .unwrap_err();
        assert!(e.contains("part.colour: unknown key"), "{e}");
        let e = parse_part(&format!("{PART}[thermal]\nx = 1\n"), &reg).unwrap_err();
        assert!(e.contains("thermal: reserved for a later milestone"), "{e}");
        let e = parse_part(
            &with(
                "file = \"pins.toml\"",
                "file = \"pins.toml\"\nibis = \"x.ibs\"",
            ),
            &reg,
        )
        .unwrap_err();
        assert!(e.contains("pins.ibis: reserved"), "{e}");
        // A view's rails are a bb-part/1 key (milestone S): refused in a /0
        // file, accepted in a /1 one.
        let e = parse_part(
            &with("name = \"lsw\"", "name = \"lsw\"\nrails = [\"VIN\"]"),
            &reg,
        )
        .unwrap_err();
        assert!(e.contains("rails: new in bb-part/1"), "{e}");
        assert!(parse_part(
            &with("name = \"lsw\"", "name = \"lsw\"\nrails = [\"VIN\"]").replacen(
                "bb-part/0",
                "bb-part/1",
                1
            ),
            &reg
        )
        .is_ok());
        assert!(parse_part(&with("analog_ic", "chip"), &reg).is_err());
        assert!(parse_part(&with("datasheet_checked", "0.9"), &reg).is_err());
        assert!(parse_part(&with("ti/tps22917", "TI/TPS22917"), &reg).is_err());
        assert!(parse_part(&with("0.1.0", "0.1"), &reg).is_err());
        assert!(parse_part(&with("bb-part/0", "bb-part/2"), &reg)
            .unwrap_err()
            .contains("upgrade bb"));
        assert!(parse_part(
            &with("default_for = [\"fast\"]", "default_for = [\"slow\"]"),
            &reg
        )
        .is_err());
    }

    const SWITCH: &str = r#"
schema = "bb-part/1"
[part]
id = "example/button"
version = "0.1.0"
kind = "mechanical"
license = "Apache-2.0"
[pins]
file = "pins.toml"
[[analog]]
name = "contact"
kind = "switch"
fidelity = ["fast"]
default_for = ["fast"]
ports = [{ role = "pin", pin = "1" }, { role = "pin", pin = "2" }]
params = { ron = "1k", bounce_max = "1ms" }
"#;

    #[test]
    fn switch_views() {
        // ADR-010 D2: two typed pin ports, ron/roff (defaults 50 mOhm and
        // 100 MOhm) and bounce_max, no file or subckt; new in bb-part/1.
        let reg = Registry::current();
        let (p, note) = parse_part(SWITCH, &reg).unwrap();
        assert!(note.is_none());
        let v = &p.analog[0];
        assert_eq!(v.kind, "switch");
        assert_eq!(v.subckt, SWITCH_SUBCKT);
        assert_eq!(
            v.ports,
            vec![
                Port::OtherPin { pin: "1".into() },
                Port::OtherPin { pin: "2".into() }
            ]
        );
        assert_eq!(switch_params(v), Ok((1_000_000_000, 100_000_000_000_000)));
        assert_eq!(switch_bounce_max(v), Ok(Some(1_000_000_000_000)));
        let def = switch_definition();
        assert!(def.contains(".subckt bb_switch a b ctl"), "{def}");
        let with = |from: &str, to: &str| SWITCH.replacen(from, to, 1);
        let err = |s: &str| parse_part(s, &reg).unwrap_err();
        assert!(err(&with("bb-part/1", "bb-part/0")).contains("new in bb-part/1"));
        assert!(err(&with(
            "kind = \"switch\"",
            "kind = \"switch\"\nfile = \"x.lib\""
        ))
        .contains("no file or subckt"));
        assert!(err(&with("{ role = \"pin\", pin = \"2\" }", "\"2\"")).contains("typed tables"));
        assert!(err(&with(", { role = \"pin\", pin = \"2\" }", "")).contains("exactly two"));
        assert!(err(&with("ron = \"1k\"", "ron = \"1k\", vth = \"1V\"")).contains("params.vth"));
        assert!(err(&with("ron = \"1k\"", "ron = \"200Meg\"")).contains("ron < roff"));
        assert!(err(&with("role = \"pin\", pin = \"1\"", "role = \"pad\"")).contains("role"));
    }

    #[test]
    fn params() {
        let v = ParamKind::Q {
            dim: Dim::Voltage,
            supply_ok: true,
        };
        assert_eq!(parse_param("3.3V", v), Ok(Param::Quantity(3_300_000)));
        assert_eq!(
            parse_param("VDD", v),
            Ok(Param::Supply {
                factor: Decimal { mant: 1, exp: 0 },
                pin: "VDD".into()
            })
        );
        assert_eq!(
            parse_param("0.7*VDD", v),
            Ok(Param::Supply {
                factor: Decimal { mant: 7, exp: -1 },
                pin: "VDD".into()
            })
        );
        let t = ParamKind::Q {
            dim: Dim::Time,
            supply_ok: false,
        };
        assert!(parse_param("VDD", t).is_err());
        assert!(parse_param(
            "1M",
            ParamKind::Q {
                dim: Dim::Resistance,
                supply_ok: false
            }
        )
        .is_err());
    }
}
