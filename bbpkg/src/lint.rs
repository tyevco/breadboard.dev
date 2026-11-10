//! `bb pkg lint` (ADR-006 D2-D6, D9; `spec/package.md` "Lint").
//!
//! Errors make the package unusable; warnings are reported (a migration
//! hint, an explicit binding that differs from the package, a conflict
//! between views). Lint never changes a file.

use crate::optsets;
use crate::package::{
    module_pinmap, param_set, pin_model_params, Package, Param, Port, LOCAL_ONLY,
};
use crate::pkgtest;
use crate::schema::Registry;
use crate::spice;
use crate::value::{parse_number, parse_quantity, Dim};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error,
    Warning,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub level: Level,
    pub msg: String,
}

/// What lint needs to know beyond the package.
pub struct Context<'a> {
    /// Platform ids this bb provides (`stm32f103-offline@1`, ...), for
    /// `[behavioral] platform`.
    pub platforms: &'a [String],
    /// Device ids this bb provides (`[behavioral] device`).
    pub devices: &'a [String],
    /// The package lives in breadboard.dev's first-party `packages/`
    /// (where `LicenseRef-local-only` is refused, D9).
    pub first_party: bool,
    /// For a module: its chip package (`[module] chip`), when the caller
    /// found it, so the pinmap's chip pin names are checked too.
    pub chip: Option<&'a Package>,
}

struct Out(Vec<Finding>);

impl Out {
    fn e(&mut self, m: impl Into<String>) {
        self.0.push(Finding {
            level: Level::Error,
            msg: m.into(),
        });
    }
    fn w(&mut self, m: impl Into<String>) {
        self.0.push(Finding {
            level: Level::Warning,
            msg: m.into(),
        });
    }
}

/// Is `s` a plausible SPDX expression (ids joined by AND/OR/WITH)?
fn spdx_ok(s: &str) -> bool {
    !s.is_empty()
        && s.split_whitespace().all(|w| {
            matches!(w, "AND" | "OR" | "WITH" | "(" | ")")
                || w.trim_matches(|c| c == '(' || c == ')')
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
        })
}

/// Lint one loaded package.
pub fn lint(pkg: &Package, ctx: &Context, reg: &Registry) -> Vec<Finding> {
    let mut o = Out(Vec::new());
    let p = &pkg.part;
    for n in &pkg.notes {
        o.w(n.clone());
    }
    // ---- licence (D9).
    if !spdx_ok(&p.license) {
        o.e(format!(
            "part.license {:?}: expected an SPDX expression",
            p.license
        ));
    }
    if p.license.contains(LOCAL_ONLY) && ctx.first_party {
        o.e(format!(
            "part.license is {LOCAL_ONLY}: such a package must live in a local package path outside breadboard.dev, never in its packages/ (ADR-006 D9)"
        ));
    }
    // ---- behavioural layer (D3, D8).
    if let Some(b) = &p.behavioral {
        if let Some(pl) = &b.platform {
            if !ctx.platforms.contains(pl) {
                o.e(format!(
                    "behavioral.platform {pl:?}: this bb provides {}",
                    if ctx.platforms.is_empty() {
                        "none".to_string()
                    } else {
                        ctx.platforms.join(", ")
                    }
                ));
            }
        }
        if let Some(d) = &b.device {
            if !ctx.devices.contains(d) {
                o.e(format!(
                    "behavioral.device {d:?}: no such device in this bb"
                ));
            }
        }
    }
    // ---- module pinmap.
    if let Some(m) = &p.module {
        if !pkg.dir.join(&m.pinmap).is_file() {
            o.e(format!("module.pinmap {}: file not found", m.pinmap));
        } else {
            lint_module(pkg, ctx, &mut o);
        }
    }
    if let Some(pm) = &p.footprint_pinmap {
        if !pkg.dir.join(pm).is_file() {
            o.e(format!("footprints.pinmap {pm}: file not found"));
        }
    }
    for pat in &p.mpn_patterns {
        if pat.is_empty() || pat.starts_with('^') || pat.ends_with('$') {
            o.e(format!(
                "part.mpn_patterns {pat:?}: patterns are anchored to the whole MPN implicitly; write them without ^ and $"
            ));
        }
    }
    lint_pins(pkg, &mut o);
    lint_views(pkg, &mut o);
    lint_tests(pkg, reg, &mut o);
    o.0.sort_by(|a, b| a.level.cmp(&b.level));
    o.0
}

/// A module (`spec/package.md` section 4, ADR-006 Amendment 3): every pad
/// maps once onto a chip pin; the pads carry no parameter sets and no
/// behavioural layer of their own (the chip's apply); with the chip package
/// at hand, each chip pin exists, its role matches the pad's, the pad's
/// models are the chip pin's, and a pad's supply maps to the chip pin's.
fn lint_module(pkg: &Package, ctx: &Context, o: &mut Out) {
    let p = &pkg.part;
    let m = p.module.as_ref().expect("module");
    if p.behavioral.is_some() {
        o.e("a module has no [behavioral] layer of its own: its chip package's applies");
    }
    if !pkg.pins.defaults.is_empty() {
        o.e("pins.toml: a module's pads carry no parameter sets ([defaults]); bindings take the chip pin's");
    }
    let rows = match module_pinmap(pkg) {
        Ok(r) => r,
        Err(e) => {
            o.e(format!("module.pinmap: {e}"));
            return;
        }
    };
    let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
    for (pad, chip_pin) in &rows {
        if pkg.pins.pins.iter().all(|x| x.number != *pad) {
            o.e(format!(
                "module.pinmap: pad {pad} is not a pin number in pins.toml"
            ));
        }
        if seen.insert(pad, chip_pin).is_some() {
            o.e(format!("module.pinmap: pad {pad} mapped twice"));
        }
    }
    for pin in &pkg.pins.pins {
        if pin.role != "nc" && !seen.contains_key(pin.number.as_str()) {
            o.e(format!(
                "module.pinmap: pad {} ({}) is not mapped to a chip pin",
                pin.number, pin.name
            ));
        }
    }
    let Some(chip) = ctx.chip else {
        o.w(format!(
            "module.chip {}: package not found, so the pinmap's chip pin names are not checked",
            m.chip
        ));
        return;
    };
    if chip.part.id != m.chip {
        o.e(format!(
            "module.chip {}: the chip package given is {}",
            m.chip, chip.part.id
        ));
        return;
    }
    if chip.part.behavioral.is_none() {
        o.w(format!(
            "module.chip {}: the chip has no behavioural layer",
            m.chip
        ));
    }
    let power = |r: &str| matches!(r, "power_in" | "power_out" | "ground");
    for pin in &pkg.pins.pins {
        let Some(cn) = seen.get(pin.number.as_str()) else {
            continue;
        };
        let Some(cp) = chip.pins.by_name(cn) else {
            o.e(format!(
                "module.pinmap: pad {} -> {cn}: no such pin in {}",
                pin.number, m.chip
            ));
            continue;
        };
        if (power(&pin.role) || power(&cp.role)) && pin.role != cp.role {
            o.e(format!(
                "module.pinmap: pad {} ({}, {}) -> {cn} ({}): the roles differ",
                pin.number, pin.name, pin.role, cp.role
            ));
        }
        for model in &pin.models {
            if !cp.models.contains(model) {
                o.e(format!(
                    "module.pinmap: pad {} ({}) offers {model}, which {} pin {cn} does not",
                    pin.number, pin.name, m.chip
                ));
            }
        }
        if let Some(s) = &pin.supply {
            let mapped = pkg
                .pins
                .pins
                .iter()
                .find(|x| x.name == *s)
                .and_then(|x| seen.get(x.number.as_str()).copied());
            if cp.supply.as_deref() != mapped {
                o.e(format!(
                    "module.pinmap: pad {} ({}): its supply {s} maps to {:?}, the chip pin's supply is {:?}",
                    pin.number, pin.name, mapped, cp.supply
                ));
            }
        }
    }
}

fn lint_pins(pkg: &Package, o: &mut Out) {
    let pins = &pkg.pins;
    // Every pin number once; names unique except supply, ground and nc
    // pins (a part has several VDD/VSS pins).
    let mut numbers = BTreeSet::new();
    let mut names: BTreeMap<&str, &str> = BTreeMap::new();
    for pin in &pins.pins {
        if !numbers.insert(pin.number.as_str()) {
            o.e(format!("pins.toml: pin number {} listed twice", pin.number));
        }
        let shared = matches!(
            pin.role.as_str(),
            "power_in" | "power_out" | "ground" | "nc"
        );
        if let Some(prev) = names.insert(pin.name.as_str(), pin.role.as_str()) {
            let prev_shared = matches!(prev, "power_in" | "power_out" | "ground" | "nc");
            if !(shared && prev_shared && prev == pin.role) {
                o.e(format!(
                    "pins.toml: pin name {} used twice (only supply, ground and nc pins of one role may share a name)",
                    pin.name
                ));
            }
        }
        if pin.name.eq_ignore_ascii_case("gnd") && pin.role != "ground" {
            o.e(format!(
                "pins.toml: pin {} named {} but not role ground",
                pin.number, pin.name
            ));
        }
        // Models allowed per role.
        for m in &pin.models {
            let ok = match pin.role.as_str() {
                "io" => true,
                "input" => m != "digital_out",
                "output" => m == "digital_out",
                "analog" => m == "adc_in",
                _ => false,
            };
            if !ok {
                o.e(format!(
                    "pins.toml: pin {} ({}): role {} cannot take model {m}",
                    pin.number, pin.name, pin.role
                ));
            }
        }
        let has_adc_model = pin.models.iter().any(|m| m == "adc_in");
        if has_adc_model != pin.adc_channel.is_some() {
            o.e(format!(
                "pins.toml: pin {} ({}): adc = {{ channel }} is required with model adc_in, and only with it",
                pin.number, pin.name
            ));
        }
        if let Some(s) = &pin.supply {
            match pins.by_name(s) {
                Some(sp) if matches!(sp.role.as_str(), "power_in" | "power_out") => {}
                _ => o.e(format!(
                    "pins.toml: pin {} ({}): supply {s} is not a power_in or power_out pin",
                    pin.number, pin.name
                )),
            }
        }
    }
    // Parameter sets: known models, complete, every value parses; supply
    // references name supply pins; vil < vih when both are numbers.
    let used_models: BTreeSet<&str> = pins
        .pins
        .iter()
        .flat_map(|p| p.models.iter().map(String::as_str))
        .collect();
    for (model, sets) in &pins.defaults {
        if !used_models.contains(model.as_str()) {
            o.w(format!(
                "pins.toml: defaults.{model}: no pin takes this model"
            ));
        }
        let spec = pin_model_params(model).expect("checked at load");
        for (set, kv) in sets {
            for (pname, _kind, required) in spec {
                if *required && !kv.iter().any(|(k, _)| k == pname) {
                    o.e(format!(
                        "pins.toml: defaults.{model}.{set}: missing {pname}"
                    ));
                }
            }
            match param_set(pkg, model, Some(set)) {
                Err(e) => o.e(format!("pins.toml: {e}")),
                Ok(ps) => {
                    for (k, v) in &ps {
                        if let Param::Supply { pin, .. } = v {
                            match pins.by_name(pin) {
                                Some(sp) if matches!(sp.role.as_str(), "power_in" | "power_out") => {}
                                _ => o.e(format!(
                                    "pins.toml: defaults.{model}.{set}.{k}: {pin} is not a supply pin of this part"
                                )),
                            }
                        }
                    }
                    if let (Some(Param::Quantity(vih)), Some(Param::Quantity(vil))) =
                        (ps.get("vih"), ps.get("vil"))
                    {
                        if vil >= vih {
                            o.e(format!(
                                "pins.toml: defaults.{model}.{set}: needs vil < vih"
                            ));
                        }
                    }
                    if let (
                        Some(Param::Supply {
                            factor: fh,
                            pin: ph,
                        }),
                        Some(Param::Supply {
                            factor: fl,
                            pin: pl,
                        }),
                    ) = (ps.get("vih"), ps.get("vil"))
                    {
                        if ph == pl && fl.to_f64() >= fh.to_f64() {
                            o.e(format!(
                                "pins.toml: defaults.{model}.{set}: needs vil < vih"
                            ));
                        }
                    }
                    if let Some(Param::Quantity(z)) = ps.get("zout") {
                        if *z < 0 {
                            o.e(format!(
                                "pins.toml: defaults.{model}.{set}: zout must be >= 0"
                            ));
                        }
                    }
                    if let Some(Param::Quantity(b)) = ps.get("bits") {
                        if !(1..=24).contains(b) {
                            o.e(format!(
                                "pins.toml: defaults.{model}.{set}: bits must be 1 to 24"
                            ));
                        }
                    }
                }
            }
        }
    }
}

fn lint_views(pkg: &Package, o: &mut Out) {
    let p = &pkg.part;
    let mut names = BTreeSet::new();
    let mut rung_owner: BTreeMap<&str, &str> = BTreeMap::new();
    let mut texts: Vec<(String, String)> = Vec::new();
    for v in &p.analog {
        let at = format!("[[analog]] {}", v.name);
        if !names.insert(v.name.as_str()) {
            o.e(format!("{at}: view name used twice"));
        }
        for r in &v.default_for {
            if !v.fidelity.contains(r) {
                o.e(format!(
                    "{at}: default_for {r} is not in fidelity (default_for must be a subset)"
                ));
            }
            if let Some(prev) = rung_owner.insert(r.as_str(), v.name.as_str()) {
                o.e(format!(
                    "{at}: rung {r} is already the default of view {prev} (one default per rung)"
                ));
            }
        }
        for (k, val) in &v.params {
            if let Err(e) = parse_number(val) {
                o.e(format!("{at}: params.{k}: {e}"));
            }
        }
        for q in &v.qualified {
            if let Err(e) = parse_quantity(&q.lattice, Dim::Time) {
                o.e(format!("{at}: qualified.lattice: {e}"));
            }
            if optsets::get(&q.options).is_none() {
                o.e(format!(
                    "{at}: qualified.options {:?}: not a named option set ({})",
                    q.options,
                    optsets::names().join(", ")
                ));
            }
        }
        if v.provenance.is_none() && p.provenance.is_none() {
            o.e(format!(
                "{at}: no provenance (per view, or a package-level [provenance])"
            ));
        }
        // Ports name pins; pin_load pin ports too.
        for port in &v.ports {
            let pin = match port {
                Port::Pin(n) | Port::OtherPin { pin: n } => Some(n),
                _ => None,
            };
            if let Some(n) = pin {
                if pkg.pins.by_name(n).is_none() {
                    o.e(format!("{at}: port {n} is not a pin in pins.toml"));
                }
            }
        }
        if v.kind == "pin_load" && v.ports.iter().filter(|p| matches!(p, Port::Pad)).count() != 1 {
            o.e(format!("{at}: a pin_load view has exactly one pad port"));
        }
        if v.kind == "circuit" {
            let mut seen = BTreeSet::new();
            for port in &v.ports {
                if let Port::Pin(n) = port {
                    if !seen.insert(n) {
                        o.e(format!("{at}: port {n} listed twice"));
                    }
                }
            }
        }
        // The file: definitions, the subckt and its port count, the gnd rule.
        let path = pkg.dir.join(&v.file);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                o.e(format!("{at}: file {}: {e}", v.file));
                continue;
            }
        };
        match spice::definitions(&text) {
            Err(e) => o.e(format!("{at}: {}: {e}", v.file)),
            Ok(defs) => {
                match defs
                    .iter()
                    .find(|d| d.kind == "subckt" && d.name == v.subckt.to_ascii_lowercase())
                {
                    None => o.e(format!("{at}: {} defines no .subckt {}", v.file, v.subckt)),
                    Some(d) => {
                        if d.ports.len() != v.ports.len() {
                            o.e(format!(
                                "{at}: .subckt {} has {} ports, the view lists {}",
                                v.subckt,
                                d.ports.len(),
                                v.ports.len()
                            ));
                        }
                    }
                }
                for g in spice::gnd_violations(&defs) {
                    o.e(format!("{at}: {}: {g}", v.file));
                }
                // A bare M in any number of the file is not checked here:
                // view files are SPICE, where M is milli by definition; the
                // D5 grammar applies to the package's TOML strings.
            }
        }
        texts.push((format!("view {}", v.name), text));
    }
    // D3: the package's own views must not define one name differently.
    if let Err(e) = spice::collect(&texts) {
        o.e(e);
    }
    // D9 rule 2: a parameter that several views carry with different
    // defaults is a conflict to report, never to average (a warning: each
    // view is right for its own purpose).
    let mut by_param: BTreeMap<&str, Vec<(&str, &str)>> = BTreeMap::new();
    for v in &p.analog {
        for (k, val) in &v.params {
            by_param
                .entry(k.as_str())
                .or_default()
                .push((v.name.as_str(), val.as_str()));
        }
    }
    for (k, vs) in by_param {
        let distinct: BTreeSet<Option<u64>> = vs
            .iter()
            .map(|(_, val)| parse_number(val).ok().map(|d| d.to_f64().to_bits()))
            .collect();
        if vs.len() > 1 && distinct.len() > 1 {
            o.w(format!(
                "conflict (D9 rule 2): parameter {k} differs between views: {}",
                vs.iter()
                    .map(|(n, val)| format!("{n} {k}={val}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
}

fn lint_tests(pkg: &Package, reg: &Registry, o: &mut Out) {
    let Some(dir) = &pkg.part.tests_dir else {
        if !pkg.part.analog.is_empty() {
            o.w("no [tests] dir: the analog views have no vectors, so they cannot be lattice-qualified (D6)".to_string());
        }
        return;
    };
    let tdir = pkg.dir.join(dir);
    match pkgtest::load_vectors(pkg, &tdir, reg) {
        Err(e) => o.e(format!("tests: {e}")),
        Ok(vs) => {
            for v in &pkg.part.analog {
                if !vs.iter().any(|t| t.view == v.name) {
                    o.w(format!("[[analog]] {}: no test vector", v.name));
                }
            }
            if vs.is_empty() {
                o.w(format!("tests: no vectors in {dir}"));
            }
        }
    }
}
