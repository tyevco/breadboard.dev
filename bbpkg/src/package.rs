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

/// Reserved keys of bb-part/1 (D11).
pub const RESERVED_TOP: &[&str] = &["layers", "thermal", "rtl", "power"];

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
}

#[derive(Clone, Debug, PartialEq)]
pub struct Qualified {
    pub lattice: String,
    pub options: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub name: String,
    /// `circuit` or `pin_load`.
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
}

#[derive(Clone, Debug, PartialEq)]
pub struct Behavioral {
    pub engine: String,
    pub platform: Option<String>,
    pub device: Option<String>,
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
];

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

fn provenance(mut o: Obj) -> Result<Provenance, String> {
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
    };
    let path = o.path.clone();
    o.done()?;
    one_of(&format!("{path}.source"), &p.source, SOURCES)?;
    one_of(&format!("{path}.confidence"), &p.confidence, CONFIDENCES)?;
    Ok(p)
}

fn rungs(what: &str, v: &[String]) -> Result<(), String> {
    for r in v {
        one_of(what, r, RUNGS)?;
    }
    Ok(())
}

fn view(mut o: Obj) -> Result<View, String> {
    o.reserved(&["rails"], "bb-part/1")?;
    let path = o.path.clone();
    let kind = o.str_opt("kind")?.unwrap_or_else(|| "circuit".into());
    one_of(&format!("{path}.kind"), &kind, &["circuit", "pin_load"])?;
    let mut ports = Vec::new();
    match o.get("ports") {
        None => return Err(format!("{path}.ports: required")),
        Some(toml::Value::Array(a)) => {
            for (i, v) in a.iter().enumerate() {
                let at = format!("{path}.ports[{i}]");
                match (kind.as_str(), v) {
                    ("circuit", toml::Value::String(s)) => ports.push(Port::Pin(s.clone())),
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
    let v = View {
        name: o.str_req("name")?,
        kind,
        purpose: o.str_opt("purpose")?,
        fidelity: o.strs("fidelity")?,
        default_for: o.strs("default_for")?,
        file: o.str_req("file")?,
        subckt: o.str_req("subckt")?,
        ports,
        params: o.str_map("params")?,
        qualified,
        provenance: o.table_opt("provenance")?.map(provenance).transpose()?,
    };
    o.done()?;
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
    o.reserved(RESERVED_TOP, "bb-part/1")?;
    let schema = o.str_req("schema")?;
    let note = match reg.check("bb-part", &schema)? {
        Version::Current => None,
        Version::Previous { hint } => Some(hint),
    };
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
    pins.reserved(&["ibis"], "bb-part/1")?;
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
            b.reserved(&["cpu", "svd"], "bb-part/1")?;
            let r = Behavioral {
                engine: b.str_req("engine")?,
                platform: b.str_opt("platform")?,
                device: b.str_opt("device")?,
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
        .map(view)
        .collect::<Result<Vec<_>, String>>()?;
    let provenance = o.table_opt("provenance")?.map(provenance).transpose()?;
    let tests_dir = match o.table_opt("tests")? {
        Some(mut t) => {
            let d = t.str_req("dir")?;
            t.done()?;
            Some(d)
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
            if pin_model_params(m).is_none() {
                return Err(format!(
                    "{path}.models: {m:?} is not a pin model (digital_out, digital_in, adc_in)"
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
        },
        note,
    ))
}

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
        let (p, note) = parse_part(PART, &Registry::current()).unwrap();
        assert!(note.is_none());
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
        assert!(e.contains("reserved for bb-part/1"), "{e}");
        let e = parse_part(
            &with(
                "file = \"pins.toml\"",
                "file = \"pins.toml\"\nibis = \"x.ibs\"",
            ),
            &reg,
        )
        .unwrap_err();
        assert!(e.contains("pins.ibis: reserved"), "{e}");
        let e =
            parse_part(&with("name = \"lsw\"", "name = \"lsw\"\nrails = []"), &reg).unwrap_err();
        assert!(e.contains("rails: reserved"), "{e}");
        assert!(parse_part(&with("analog_ic", "chip"), &reg).is_err());
        assert!(parse_part(&with("datasheet_checked", "0.9"), &reg).is_err());
        assert!(parse_part(&with("ti/tps22917", "TI/TPS22917"), &reg).is_err());
        assert!(parse_part(&with("0.1.0", "0.1"), &reg).is_err());
        assert!(parse_part(&with("bb-part/0", "bb-part/1"), &reg)
            .unwrap_err()
            .contains("upgrade bb"));
        assert!(parse_part(
            &with("default_for = [\"fast\"]", "default_for = [\"slow\"]"),
            &reg
        )
        .is_err());
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
