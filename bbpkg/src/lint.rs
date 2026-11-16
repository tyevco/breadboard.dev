//! `bb pkg lint` (ADR-006 D2-D6, D9; `spec/package.md` "Lint").
//!
//! Errors make the package unusable; warnings are reported (a migration
//! hint, an explicit binding that differs from the package, a conflict
//! between views). Lint never changes a file.

use crate::bexpr;
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
    /// The package is checked for publication (`bb pkg lint --publish`,
    /// ADR-011 D11): the view-name rule is an error, not a warning.
    pub publish: bool,
}

/// The prefix every top-level name of a package's views must start with
/// before it is published (ADR-011 D11): the package id upper-cased, with
/// `/`, `-` and `.` as `_`, then `_` (`ti/tps22917` -> `TI_TPS22917_`).
pub fn name_prefix(id: &str) -> String {
    let mut s: String = id
        .chars()
        .map(|c| match c {
            '/' | '-' | '.' => '_',
            c => c.to_ascii_uppercase(),
        })
        .collect();
    s.push('_');
    s
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

/// OSI-approved licences (SPDX ids) the publication gate accepts (ADR-011
/// D7). A licence missing here is refused until it is added with its OSI
/// approval checked.
pub const OSI_LICENSES: &[&str] = &[
    "0BSD",
    "AFL-3.0",
    "AGPL-3.0-only",
    "AGPL-3.0-or-later",
    "Apache-2.0",
    "Artistic-2.0",
    "BSD-1-Clause",
    "BSD-2-Clause",
    "BSD-2-Clause-Patent",
    "BSD-3-Clause",
    "BSL-1.0",
    "CDDL-1.0",
    "CERN-OHL-P-2.0",
    "CERN-OHL-S-2.0",
    "CERN-OHL-W-2.0",
    "ECL-2.0",
    "EPL-1.0",
    "EPL-2.0",
    "EUPL-1.2",
    "GPL-2.0-only",
    "GPL-2.0-or-later",
    "GPL-3.0-only",
    "GPL-3.0-or-later",
    "ISC",
    "LGPL-2.1-only",
    "LGPL-2.1-or-later",
    "LGPL-3.0-only",
    "LGPL-3.0-or-later",
    "MIT",
    "MIT-0",
    "MPL-2.0",
    "MS-PL",
    "MS-RL",
    "NCSA",
    "OFL-1.1",
    "PostgreSQL",
    "UPL-1.0",
    "Unlicense",
    "Zlib",
];

/// Licences accepted for data-only packages (no view code; ADR-011 D7).
pub const DATA_LICENSES: &[&str] = &["CC0-1.0", "CC-BY-4.0", "CC-BY-SA-4.0"];

/// SPDX exceptions accepted after `WITH`.
pub const LICENSE_EXCEPTIONS: &[&str] = &[
    "Classpath-exception-2.0",
    "GCC-exception-3.1",
    "KiCad-libraries-exception",
    "LLVM-exception",
];

/// The licence ids and exceptions of an SPDX expression, in order.
fn spdx_ids(s: &str) -> (Vec<String>, Vec<String>) {
    let mut ids = Vec::new();
    let mut exc = Vec::new();
    let mut after_with = false;
    for w in s.replace('(', " ( ").replace(')', " ) ").split_whitespace() {
        match w {
            "AND" | "OR" | "(" | ")" => after_with = false,
            "WITH" => after_with = true,
            id if after_with => {
                exc.push(id.to_string());
                after_with = false;
            }
            id => ids.push(id.to_string()),
        }
    }
    (ids, exc)
}

/// Is `id` a licence a published package may name (OSI, or a data licence)?
pub fn redistributable(id: &str) -> bool {
    OSI_LICENSES.contains(&id) || DATA_LICENSES.contains(&id)
}

/// The publication licence gate (ADR-011 D7): every licence of
/// `part.license` OSI-approved, or CC0-1.0 / CC-BY-4.0 / CC-BY-SA-4.0 for a
/// package without view code; no `LicenseRef-*`; every view with a
/// provenance `source`; a `vendor_derived` view with a redistributable
/// `upstream_license`. A vendor file (`[behavioral] svd`) is a reference
/// and may carry any licence. One message per violation.
pub fn licence_gate(pkg: &Package) -> Vec<String> {
    let p = &pkg.part;
    let mut out = Vec::new();
    let (ids, exc) = spdx_ids(&p.license);
    if ids.is_empty() {
        out.push(format!(
            "part.license {:?}: no licence id (ADR-011 D7)",
            p.license
        ));
    }
    for id in &ids {
        if id.starts_with("LicenseRef-") {
            out.push(format!(
                "part.license {id}: a LicenseRef- licence cannot be published (ADR-011 D7)"
            ));
        } else if DATA_LICENSES.contains(&id.as_str()) {
            if !p.analog.is_empty() {
                out.push(format!(
                    "part.license {id}: allowed only for data-only packages (pins, tables, \
                     vectors), and this package has {} analog view(s) (ADR-011 D7)",
                    p.analog.len()
                ));
            }
        } else if !OSI_LICENSES.contains(&id.as_str()) {
            out.push(format!(
                "part.license {id}: not an OSI-approved licence this bb knows, nor CC0-1.0, \
                 CC-BY-4.0 or CC-BY-SA-4.0 (ADR-011 D7)"
            ));
        }
    }
    for e in &exc {
        if !LICENSE_EXCEPTIONS.contains(&e.as_str()) {
            out.push(format!(
                "part.license: exception {e} is not one this bb knows (ADR-011 D7)"
            ));
        }
    }
    for v in &p.analog {
        match v.provenance.as_ref().or(p.provenance.as_ref()) {
            None => out.push(format!(
                "view {}: no provenance source (every published view needs one, ADR-011 D7)",
                v.name
            )),
            Some(pr) if pr.source == "vendor_derived" => {
                let ok = pr.upstream_license.as_deref().is_some_and(|u| {
                    let (ids, _) = spdx_ids(u);
                    !ids.is_empty() && ids.iter().all(|x| redistributable(x))
                });
                if !ok {
                    out.push(format!(
                        "view {}: vendor_derived with upstream_license {:?}: publishing needs a \
                         known redistributable upstream licence (ADR-011 D7, the TPS22917 rule)",
                        v.name, pr.upstream_license
                    ));
                }
            }
            Some(_) => {}
        }
    }
    out
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
    // ---- the licence gate of publication (ADR-011 D7): errors with
    // --publish only.
    if ctx.publish {
        for m in licence_gate(pkg) {
            o.e(m);
        }
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
    lint_views(pkg, ctx, &mut o);
    lint_epoch(pkg, &mut o);
    lint_tests(pkg, reg, &mut o);
    o.0.sort_by(|a, b| a.level.cmp(&b.level));
    o.0
}

/// Is `name` a power pin (`power_in`/`power_out`) of the package?
fn is_supply_pin(pkg: &Package, name: &str) -> bool {
    pkg.pins
        .by_name(name)
        .is_some_and(|p| matches!(p.role.as_str(), "power_in" | "power_out"))
}

/// The keys of the Phase 2 schema epoch (bb-part/1, bb-pins/1): accepted,
/// checked for consistency, and inert until their milestones use them.
fn lint_epoch(pkg: &Package, o: &mut Out) {
    let p = &pkg.part;
    // [power] (ADR-009 D4, D5).
    if let Some(pw) = &p.power {
        if let Some(r) = &pw.io_rail {
            if !is_supply_pin(pkg, r) {
                o.e(format!(
                    "power.io_rail {r}: not a power_in or power_out pin of this part"
                ));
            }
        }
        for i in &pw.idd {
            if let (Some(max), Ok(typ)) = (&i.max, parse_quantity(&i.typ, Dim::Current)) {
                if parse_quantity(max, Dim::Current).is_ok_and(|m| m < typ) {
                    o.e(format!("power.idd.{}: max is below typ", i.state));
                }
            }
        }
    }
    // [[document]] and [claims] (ADR-012 D3, D4).
    if let Some(f) = &p.claims_file {
        if !pkg.dir.join(f).is_file() {
            o.e(format!("claims.file {f}: file not found"));
        }
    }
    // upstream_license (ADR-011 D7): an SPDX expression.
    let provs = p
        .provenance
        .iter()
        .chain(p.analog.iter().filter_map(|v| v.provenance.as_ref()));
    for pv in provs {
        if let Some(u) = &pv.upstream_license {
            if !spdx_ok(u) {
                o.e(format!(
                    "provenance.upstream_license {u:?}: expected an SPDX expression"
                ));
            }
        }
    }
    // limits (bb-pins/1, ADR-009 D3): the pin and a reference pin exist.
    for l in &pkg.pins.limits {
        if pkg.pins.by_name(&l.pin).is_none() {
            o.e(format!(
                "pins.toml: limits: pin {} is not in pins.toml",
                l.pin
            ));
        }
        for b in [&l.min, &l.max].into_iter().flatten() {
            if let Ok(crate::package::Bound { pin: Some(r), .. }) = crate::package::parse_bound(b) {
                if pkg.pins.by_name(&r).is_none() {
                    o.e(format!(
                        "pins.toml: limits: {} {b:?}: {r} is not a pin of this part",
                        l.pin
                    ));
                }
            }
        }
    }
    // rail (bb-pins/1, ADR-009 D5): a supply pin of the part.
    if let Some(sets) = pkg.pins.defaults.get("digital_out") {
        for (set, kv) in sets {
            for (k, v) in kv {
                if k == "rail" && !is_supply_pin(pkg, v) {
                    o.e(format!(
                        "pins.toml: defaults.digital_out.{set}.rail {v}: not a power_in or \
                         power_out pin of this part"
                    ));
                }
            }
        }
    }
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

fn lint_views(pkg: &Package, ctx: &Context, o: &mut Out) {
    let p = &pkg.part;
    let mut names = BTreeSet::new();
    let mut rung_owner: BTreeMap<&str, &str> = BTreeMap::new();
    let mut texts: Vec<(String, String)> = Vec::new();
    let prefix = name_prefix(&p.id);
    for v in &p.analog {
        let at = format!("[[analog]] {}", v.name);
        if !names.insert(v.name.as_str()) {
            o.e(format!("{at}: view name used twice"));
        }
        // bb-part/1 (ADR-009 D4): a supply_load view enters the deck at
        // `pi` beside the instance's view, so it is never on the fast rung
        // and never a rung's default (the ladder chooses the other views).
        if v.kind == "supply_load" {
            if v.fidelity.iter().any(|r| r == "fast") {
                o.e(format!(
                    "{at}: a supply_load view is not valid at fast (it enters the deck at pi, \
                     ADR-009 D4)"
                ));
            }
            if !v.default_for.is_empty() {
                o.e(format!(
                    "{at}: a supply_load view is no rung's default (it enters the deck beside \
                     the instance's view, ADR-009 D4)"
                ));
            }
            if !v.ports.iter().any(|x| matches!(x, Port::OtherPin { .. })) {
                o.e(format!(
                    "{at}: a supply_load view has at least one supply pin port"
                ));
            }
            let states: Vec<&str> = p
                .power
                .iter()
                .flat_map(|pw| pw.idd.iter().map(|i| i.state.as_str()))
                .collect();
            for port in &v.ports {
                match port {
                    Port::State { state } if !states.contains(&state.as_str()) => o.e(format!(
                        "{at}: state port #{state} has no [power] idd.{state} entry"
                    )),
                    Port::OtherPin { pin }
                        if !pkg.pins.by_name(pin).is_some_and(|x| {
                            matches!(x.role.as_str(), "power_in" | "power_out" | "ground")
                        }) =>
                    {
                        o.e(format!(
                            "{at}: port {pin} is not a supply or ground pin (a supply_load \
                             view's pin ports are the package's supply pins, ADR-009 D4)"
                        ))
                    }
                    _ => {}
                }
            }
        } else if v.ports.iter().any(|x| matches!(x, Port::State { .. })) {
            o.e(format!("{at}: only a supply_load view has state ports"));
        }
        // bb-part/1 (ADR-009): rails name the view's supply ports.
        for r in &v.rails {
            let is_port = v.ports.iter().any(|x| match x {
                Port::Pin(n) | Port::OtherPin { pin: n } => n == r,
                _ => false,
            });
            if !is_port || !is_supply_pin(pkg, r) {
                o.e(format!(
                    "{at}: rails: {r} is not one of the view's ports on a power_in or power_out pin"
                ));
            }
        }
        // bb-part/1 (ADR-009 D8): corners override the view's own params.
        let mut corner_names = BTreeSet::new();
        for c in &v.corners {
            if !corner_names.insert(c.name.as_str()) {
                o.e(format!("{at}: corner {} declared twice", c.name));
            }
            for (k, val) in &c.params {
                if !v.params.iter().any(|(n, _)| n == k) {
                    o.e(format!(
                        "{at}: corner {}: {k} is not one of the view's params",
                        c.name
                    ));
                }
                if let Err(e) = parse_number(val) {
                    o.e(format!("{at}: corner {}: params.{k}: {e}", c.name));
                }
            }
            if c.reference.trim().is_empty() {
                o.e(format!("{at}: corner {}: ref is empty", c.name));
            }
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
        // A switch view (ADR-010 D2): bb generates its element, so there is
        // no file to check; its two contacts must be different pins.
        if v.kind == "switch" {
            if v.ports.first() == v.ports.get(1) {
                o.e(format!("{at}: a switch's two contacts are the same pin"));
            }
            let def = crate::package::switch_definition();
            texts.push((format!("view {}", v.name), def));
            continue;
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
                // ADR-011 D11: before publication every top-level name a
                // view defines starts with the package prefix, and no
                // top-level .param exists (an error with --publish, a
                // warning otherwise).
                for d in &defs {
                    let msg = if d.kind == "param" {
                        Some(format!(
                            "{at}: {}: a top-level .param {} (parameters go inside the \
                             subcircuit or on its line, ADR-011 D11)",
                            v.file, d.name
                        ))
                    } else if !d.name.to_ascii_uppercase().starts_with(&prefix) {
                        Some(format!(
                            "{at}: {}: .{} {} does not start with {prefix} (ADR-011 D11)",
                            v.file, d.kind, d.name
                        ))
                    } else {
                        None
                    };
                    match msg {
                        Some(m) if ctx.publish => o.e(m),
                        Some(m) => o.w(format!("{m}; an error for publication")),
                        None => {}
                    }
                }
                // ADR-015 D3: every B/E/G expression and .func body against
                // ngspice 42's function table and arity.
                for e in bexpr::check(&text) {
                    o.e(format!("{at}: {}: {e}", v.file));
                }
                // ADR-009 D2 (milestone P1): a view valid at pi has no
                // branch-current unknown (rule 1); an inductor is a warning
                // (rule 5). Rule 2 is pkg test's operating point at every
                // corner.
                if v.fidelity.iter().any(|r| r == "pi") {
                    for f in crate::pirules::check(&text) {
                        let m = format!("{at}: {} (valid at pi): {}", v.file, f.msg);
                        if f.error {
                            o.e(m);
                        } else {
                            o.w(m);
                        }
                    }
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
