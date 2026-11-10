//! Elaboration (ADR-007 D5-D7; ADR-006 D3, D6): a project's elaborated
//! board, its bench and its bindings become what `bb run` runs. The MCU and
//! its pin models (from `[[bind]]` and the package's `pins.toml`), the
//! analog deck (each in-scope instance's view, inlined in sorted order, the
//! bench's sources and loads, the named option set), the probes, and the
//! static content of `run.json`.
//!
//! A pure function of `board.bbm`, the bench, the locked packages and the
//! options: the deck carries no file paths, instances are emitted in
//! sorted order and shared view definitions once (`bbpkg::spice::assemble`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use bbcore::board::{Board, Instance};
use bbcore::cjson::Json;
use bbcore::Time;
use bbpkg::package::{Package, Port, View};
use bbpkg::value::{parse_number, spice_number, to_decimal, Dim};

use crate::board::{self, build_pin, Part, PinBinding, PinKind, PinReq};
use crate::import::Imported;
use crate::names::{element_fragment, Instances, Nets};
use crate::platforms;
use crate::project::{Bench, Project};

/// The fidelity ladder (ADR-006 D3).
pub const LADDER: &[&str] = &["fast", "pi", "si", "em"];

/// A probe: its name in the outputs, and the ngspice node for an analog
/// one (`None`: an MCU pin's kernel net, a digital level).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeSpec {
    pub name: String,
    pub node: Option<String>,
}

/// What `bb run` runs for a project.
pub struct RunPlan {
    pub board: board::Board,
    pub target: String,
    pub firmware: PathBuf,
    pub probes: Vec<ProbeSpec>,
    /// `run.json` content known before the run.
    pub run_json: Vec<(String, Json)>,
    pub warnings: Vec<String>,
    /// Views whose provenance is `estimated` or `agent_generated` (plan §5
    /// item 4: flagged in the summary).
    pub flagged: Vec<String>,
}

fn instances(board: &Board) -> Instances<'_> {
    Instances {
        items: board
            .instances
            .iter()
            .map(|i| {
                (
                    i.board.as_str(),
                    i.reference.as_str(),
                    i.value_text.as_str(),
                )
            })
            .collect(),
    }
}

fn kicad_name<'a>(board: &'a Board, display: &'a str) -> &'a str {
    let n = board.net(display).expect("net");
    if board.boards.len() > 1 {
        display
            .strip_prefix(&format!("{}:", n.board))
            .unwrap_or(display)
    } else {
        display
    }
}

fn nets<'a>(board: &'a Board, simulated: &'a str) -> Nets<'a> {
    Nets {
        items: board
            .nets
            .iter()
            .map(|n| {
                (
                    n.name.as_str(),
                    n.board.as_str(),
                    kicad_name(board, &n.name),
                )
            })
            .collect(),
        simulated,
    }
}

/// A net name, a test point alias or a probe alias -> the net's display name.
fn resolve_net_or_alias(board: &Board, simulated: &str, name: &str) -> Result<String, String> {
    match nets(board, simulated).resolve(name) {
        Ok(d) => Ok(d),
        Err(e) => {
            // Aliases follow the instance rule: `main:TP1` always, a bare
            // `TP1` only when one board has it.
            let c: Vec<_> = board
                .probes
                .iter()
                .filter(|a| {
                    a.alias == name
                        || (!name.contains(':')
                            && a.alias.split_once(':').is_some_and(|(_, r)| r == name))
                })
                .collect();
            match c.len() {
                0 => Err(e),
                1 => Ok(c[0].net.clone()),
                _ => Err(format!(
                    "{name} is ambiguous: {} (qualify it, ADR-007 D7)",
                    c.iter()
                        .map(|a| a.alias.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            }
        }
    }
}

/// What a probe name names.
enum ProbeTarget {
    Net(String),
    /// `(board, ref, pin name)`.
    Pin(String, String, String),
}

/// A probe name: a net or an alias (D7's net rules), else a pin
/// `<instance>.<pin>`. An error names the candidates when it is ambiguous.
fn probe_target(board: &Board, simulated: &str, name: &str) -> Result<ProbeTarget, String> {
    match resolve_net_or_alias(board, simulated, name) {
        Ok(d) => Ok(ProbeTarget::Net(d)),
        Err(e1) => match instances(board).resolve_pin(name) {
            Ok((b, r, p)) => Ok(ProbeTarget::Pin(b, r, p)),
            Err(e2) => Err(if name.contains('.') { e2 } else { e1 }),
        },
    }
}

/// `bb open`'s check (ADR-007 D7): every `[[bind]]`, probe and target names
/// a resolved instance (or a net). Returns the problems.
pub fn check_names(p: &Project, board: &Board) -> Vec<String> {
    let insts = instances(board);
    let mut out = Vec::new();
    let status = |b: &str, r: &str| board.instance(b, r).map(|i| i.status.clone());
    for b in &p.binds {
        match insts.resolve_pin(&b.pin) {
            Ok((bd, r, pin)) => {
                let st = status(&bd, &r).unwrap_or_default();
                if st != "ok" {
                    out.push(format!(
                        "[[bind]] {}: instance {bd}:{r} is {st} (it needs a package)",
                        b.pin
                    ));
                } else if !board
                    .instance(&bd, &r)
                    .is_some_and(|i| i.pins.iter().any(|x| x.name == pin))
                {
                    out.push(format!(
                        "[[bind]] {}: {bd}:{r} has no pin named {pin}",
                        b.pin
                    ));
                }
            }
            Err(e) => out.push(format!("[[bind]] {}: {e}", b.pin)),
        }
    }
    for (k, _) in &p.targets {
        match insts.resolve(k) {
            Ok((bd, r)) => {
                if status(&bd, &r).as_deref() != Some("ok") {
                    out.push(format!("[targets.{k:?}]: {bd}:{r} has no package"));
                }
            }
            Err(e) => out.push(format!("[targets.{k:?}]: {e}")),
        }
    }
    for pr in &p.probes {
        match probe_target(board, &p.simulate, pr) {
            Ok(ProbeTarget::Net(_)) => {}
            Ok(ProbeTarget::Pin(b, r, pin)) => {
                let bound = p.binds.iter().any(|x| {
                    insts.resolve_pin(&x.pin).ok() == Some((b.clone(), r.clone(), pin.clone()))
                });
                if !bound {
                    out.push(format!(
                        "[probes] {pr}: pin {b}:{r}.{pin} has no [[bind]] (only bound MCU pins are probed by pin name)"
                    ));
                }
            }
            Err(e) => out.push(format!("[probes] {pr}: {e}")),
        }
    }
    out
}

/// Select an instance's analog view at fidelity `f` (ADR-006 D3): the
/// project override, else the view whose `default_for` has `f`, else the
/// first lower rung that has one. Returns the view and the rung chosen at.
pub fn select_view<'a>(
    pkg: &'a Package,
    fidelity: &str,
    override_view: Option<&str>,
) -> Result<Option<(&'a View, String)>, String> {
    if let Some(name) = override_view {
        let v = pkg
            .part
            .analog
            .iter()
            .find(|v| v.name == name)
            .ok_or_else(|| format!("package {} has no analog view {name:?}", pkg.part.id))?;
        return Ok(Some((v, "override".into())));
    }
    let start = LADDER.iter().position(|r| *r == fidelity).ok_or_else(|| {
        format!(
            "fidelity {fidelity:?}: expected one of {}",
            LADDER.join(", ")
        )
    })?;
    for rung in LADDER[..=start].iter().rev() {
        if let Some(v) = pkg
            .part
            .analog
            .iter()
            .find(|v| v.kind != "pin_load" && v.default_for.iter().any(|d| d == rung))
        {
            return Ok(Some((v, rung.to_string())));
        }
    }
    Ok(None)
}

/// Inputs of [`plan`].
pub struct PlanArgs<'a> {
    pub project: &'a Project,
    pub imp: &'a Imported,
    pub bench: &'a Bench,
    /// The bench file as given (for run.json: its file name only).
    pub bench_name: Option<String>,
    pub fidelity: &'a str,
    pub lattice: Time,
    pub allow_unqualified: &'a [String],
    pub cli_probes: &'a [String],
}

fn sha(bytes: &[u8]) -> String {
    format!("sha256:{}", bbpkg::sha256::hex_digest(bytes))
}

fn prov_json(p: Option<&bbpkg::package::Provenance>) -> Json {
    match p {
        None => Json::Null,
        Some(p) => {
            let mut o = vec![
                ("source".to_string(), Json::str(&p.source)),
                ("confidence".to_string(), Json::str(&p.confidence)),
            ];
            if let Some(b) = &p.base {
                o.push(("base".into(), Json::str(b)));
            }
            if let Some(b) = &p.origin {
                o.push(("origin".into(), Json::str(b)));
            }
            Json::Obj(o)
        }
    }
}

/// Elaborate a project for `bb run`.
pub fn plan(a: &PlanArgs) -> Result<RunPlan, String> {
    let p = a.project;
    let board = &a.imp.board;
    let sim = p.simulate.as_str();
    let insts = instances(board);
    let mut warnings = Vec::new();

    // Names first, so a bad --probe fails before anything else is checked.
    for n in a.cli_probes {
        probe_target(board, sim, n).map_err(|e| format!("--probe {n}: {e}"))?;
    }

    // --- The target MCU and its firmware.
    let sim_targets: Vec<(String, String, &Vec<crate::project::Image>)> = p
        .targets
        .iter()
        .map(|(k, imgs)| {
            insts
                .resolve(k)
                .map(|(b, r)| (b, r, imgs))
                .map_err(|e| format!("bb.toml [targets.{k:?}]: {e}"))
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|(b, ..)| b == sim)
        .collect();
    let (tb, tref, images) =
        match sim_targets.as_slice() {
            [one] => one.clone(),
            [] => return Err(
                "no firmware target: run `bb flash --target <instance> <file.elf>` (ADR-007 D7)"
                    .into(),
            ),
            _ => return Err("Phase 1 runs exactly one MCU target on the simulated board".into()),
        };
    let tinst = board.instance(&tb, &tref).expect("resolved");
    let tpkg_ref = tinst
        .package
        .as_ref()
        .ok_or_else(|| format!("target {tb}:{tref} has no package ({})", tinst.status))?;
    let tpkg = &a.imp.packages[&tpkg_ref.id];
    // A module target (spec/package.md section 4, ADR-006 Amendment 3): the
    // behavioural layer and the pin models are its chip's, and a pad reaches
    // its chip pin through the module's pinmap (Phase 1 M6).
    let (chip, pad_to_chip): (&Package, Option<BTreeMap<String, String>>) = match &tpkg.part.module
    {
        Some(m) => {
            let chip = a.imp.packages.get(&m.chip).ok_or_else(|| {
                format!(
                    "target {tb}:{tref}: the module's chip package {} is not resolved",
                    m.chip
                )
            })?;
            let map = bbpkg::package::module_pinmap(tpkg)?.into_iter().collect();
            (chip, Some(map))
        }
        None => (tpkg, None),
    };
    // The chip pin name of a target instance pin (number, name as imported).
    let chip_pin = |number: &str, name: &str| -> Result<String, String> {
        match &pad_to_chip {
            Some(m) => m.get(number).cloned().ok_or_else(|| {
                format!(
                    "target {tb}:{tref}: pad {number} ({name}) is not in {}'s pinmap",
                    tpkg.part.id
                )
            }),
            None => Ok(name.to_string()),
        }
    };
    let beh = chip.part.behavioral.as_ref().ok_or_else(|| {
        format!(
            "target {tb}:{tref}: package {} has no behavioural layer",
            chip.part.id
        )
    })?;
    if beh.engine != "renode" {
        return Err(format!(
            "target {tb}:{tref}: engine {} (bb run supports renode)",
            beh.engine
        ));
    }
    let pdef = platforms::find(beh.platform.as_deref().unwrap_or_default())
        .ok_or_else(|| format!("target {tb}:{tref}: unknown platform"))?;
    let elfs: Vec<&crate::project::Image> = images.iter().filter(|i| i.kind == "elf").collect();
    if elfs.len() != 1 || images.len() != 1 {
        return Err(format!(
            "target {tb}:{tref}: Phase 1 implements exactly one image of kind elf (ADR-007 D5)"
        ));
    }
    let firmware = p.dir.join(&elfs[0].file);
    let fw_bytes =
        std::fs::read(&firmware).map_err(|e| format!("firmware {}: {e}", firmware.display()))?;
    // The image hash (ADR-008 D5): the loadable content, not the file.
    let fw_hash = crate::image::elf_image_hash(&fw_bytes)
        .map_err(|e| format!("firmware {}: {e}", elfs[0].file))?;
    if fw_hash != elfs[0].hash {
        return Err(format!(
            "firmware {} changed since `bb flash` (image hash {} in bb.toml, {fw_hash} now): \
             run `bb flash` again",
            elfs[0].file, elfs[0].hash
        ));
    }
    let part = Part {
        reference: tref.clone(),
        engine: "renode".into(),
        platform: pdef.board_name.to_string(),
        package: Some(chip.part.id.clone()),
    };

    // --- Bench names.
    let net_by = |name: &str, what: &str| -> Result<String, String> {
        resolve_net_or_alias(board, sim, name).map_err(|e| format!("bench {what} {name}: {e}"))
    };
    let mut supply_on: BTreeMap<String, i128> = BTreeMap::new();
    for s in &a.bench.supplies {
        let d = net_by(&s.net, "[[supply]]")?;
        if supply_on.insert(d.clone(), s.volts_uv).is_some() {
            return Err(format!("bench: two [[supply]] on net {d}"));
        }
    }
    let mut nominal: BTreeMap<String, i128> = BTreeMap::new();
    for (k, v) in &a.bench.nominal {
        nominal.insert(net_by(k, "[nominal]")?, *v);
    }

    // --- Bindings.
    let mut pins: Vec<PinBinding> = Vec::new();
    // Pin name -> kernel net; chip pin name -> the drive its binding's set
    // names (the PIN_CNF audit).
    let mut bound: BTreeMap<String, String> = BTreeMap::new();
    let mut drives: BTreeMap<String, Option<String>> = BTreeMap::new();
    // Chip pins whose adc_in binding says `load = "none"` explicitly
    // (ADR-008 D7: run.json records it).
    let mut load_none: Vec<String> = Vec::new();
    for b in &p.binds {
        let ctx = format!("bb.toml [[bind]] {}", b.pin);
        let (bb, r, pin) = insts
            .resolve_pin(&b.pin)
            .map_err(|e| format!("{ctx}: {e}"))?;
        if bb != tb || r != tref {
            return Err(format!(
                "{ctx}: Phase 1 binds pins of the target MCU {tb}:{tref} only"
            ));
        }
        let ip: Vec<_> = tinst.pins.iter().filter(|x| x.name == pin).collect();
        let ip = match ip.as_slice() {
            [one] => *one,
            [] => return Err(format!("{ctx}: {tref} has no pin named {pin}")),
            _ => return Err(format!("{ctx}: {tref} has several pins named {pin}")),
        };
        let netd = ip.net.clone();
        if let Some(want) = &b.net {
            let w =
                resolve_net_or_alias(board, sim, want).map_err(|e| format!("{ctx}: net {e}"))?;
            if netd.as_deref() != Some(w.as_str()) {
                return Err(format!(
                    "{ctx}: net = {want:?} ({w}), but the pin is on {}",
                    netd.as_deref().unwrap_or("no net")
                ));
            }
        }
        let node = match &netd {
            Some(n) if !board.net(n).expect("net").unconnected => {
                Some(board.net(n).expect("net").spice.clone())
            }
            _ => None,
        };
        // A supply pin of the chip, on the net of the target's pins that
        // carry it (a module: every pad its pinmap maps to that chip pin).
        let supply = |sp: &str| -> Result<i128, String> {
            let nets: BTreeSet<String> = tinst
                .pins
                .iter()
                .filter(|x| match &pad_to_chip {
                    Some(m) => m.get(&x.number).map(String::as_str) == Some(sp),
                    None => x.name == sp,
                })
                .filter_map(|x| x.net.clone())
                .collect();
            let sn = match nets.len() {
                1 => nets.into_iter().next().expect("one"),
                0 => return Err(format!("supply pin {sp} is on no net")),
                _ => {
                    return Err(format!(
                        "supply pin {sp} is on several nets: {}",
                        nets.into_iter().collect::<Vec<_>>().join(", ")
                    ))
                }
            };
            supply_on
                .get(&sn)
                .or_else(|| nominal.get(&sn))
                .copied()
                .ok_or_else(|| {
                    format!(
                        "supply pin {sp} is on net {sn}, which no bench [[supply]] drives and \
                         [nominal] does not list (ADR-006 D4)"
                    )
                })
        };
        let explicit: Vec<(&str, &str)> = b
            .params
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let cpin = chip_pin(&ip.number, &pin).map_err(|e| format!("{ctx}: {e}"))?;
        // A module's pads list the models they offer (a subset of the chip
        // pin's, `bb pkg lint`); the one exception is the nRF52840's VDD
        // input, an adc_in on a supply pad (ADR-008 D7), which build_pin
        // checks.
        if pad_to_chip.is_some() {
            let pad = tpkg.pins.pins.iter().find(|x| x.number == ip.number);
            let vdd_input = cpin == "VDD" && b.model == "adc_in";
            if let Some(pad) = pad.filter(|_| !vdd_input) {
                if !pad.models.contains(&b.model) {
                    return Err(format!(
                        "{ctx}: pad {} ({}) of {} does not offer model {} (it offers: {})",
                        pad.number,
                        pad.name,
                        tpkg.part.id,
                        b.model,
                        if pad.models.is_empty() {
                            "none".to_string()
                        } else {
                            pad.models.join(", ")
                        }
                    ));
                }
            }
        }
        let kind = build_pin(
            &PinReq {
                ctx: &ctx,
                name: &b.pin,
                part: &part,
                pkg: Some(chip),
                pin: &cpin,
                model: &b.model,
                set: b.set.as_deref(),
                net: node,
                explicit: &explicit,
                supply: Some(&supply),
            },
            &mut warnings,
        )?;
        if matches!(&kind, PinKind::Adc { load: None, .. })
            && explicit.iter().any(|(k, v)| *k == "load" && *v == "none")
        {
            load_none.push(cpin.clone());
        }
        if let PinKind::Out { net, .. } = &kind {
            if pins.iter().any(
                |x| matches!(&x.kind, PinKind::Out { net: n, .. } if n.eq_ignore_ascii_case(net)),
            ) {
                return Err(format!("{ctx}: net {net} is already driven by another pin"));
            }
        }
        if drives
            .insert(
                cpin.clone(),
                board::nrf_set_drive(b.set.as_deref()).filter(|_| b.model == "digital_out"),
            )
            .is_some()
        {
            return Err(format!("{ctx}: chip pin {cpin} is bound twice"));
        }
        let pb = PinBinding {
            part: tref.clone(),
            pin: cpin,
            kind,
        };
        bound.insert(pin.clone(), pb.kernel_net());
        pins.push(pb);
    }

    // --- Analog scope.
    let in_scope: Option<BTreeSet<String>> = match &a.bench.analog_nets {
        Some(list) => Some(
            list.iter()
                .map(|n| net_by(n, "[analog] nets"))
                .collect::<Result<_, _>>()?,
        ),
        None => {
            warnings.push(
                "the bench has no [analog] nets: every net is in the analog scope (ADR-007 D5)"
                    .into(),
            );
            None
        }
    };
    let scoped = |d: &str| in_scope.as_ref().is_none_or(|s| s.contains(d));

    // --- The PIN_CNF audit list (ADR-008 D4): every bound pin, and every
    // unbound GPIO (a chip pin of role `io`) whose net is in the analog scope.
    let mut audit: Vec<(String, board::AuditExpect)> = Vec::new();
    for pb in &pins {
        let e = match &pb.kind {
            PinKind::Out { .. } => board::AuditExpect::Out(drives.get(&pb.pin).cloned().flatten()),
            PinKind::In { .. } => board::AuditExpect::In,
            PinKind::OutUnconnected | PinKind::Adc { .. } => continue,
        };
        audit.push((pb.pin.clone(), e));
    }
    let mut unbound: BTreeSet<String> = BTreeSet::new();
    for x in &tinst.pins {
        let Some(n) = x.net.as_deref() else { continue };
        if board.net(n).expect("net").unconnected || !scoped(n) {
            continue;
        }
        let cp = chip_pin(&x.number, &x.name)?;
        let io = chip.pins.by_name(&cp).is_some_and(|d| d.role == "io");
        if io && !drives.contains_key(&cp) {
            unbound.insert(cp);
        }
    }
    audit.extend(
        unbound
            .into_iter()
            .map(|cp| (cp, board::AuditExpect::Unbound)),
    );
    let mut excluded: BTreeSet<(String, String)> = BTreeSet::new();
    for e in &a.bench.exclude {
        excluded.insert(
            insts
                .resolve(e)
                .map_err(|x| format!("bench [analog] exclude: {x}"))?,
        );
    }
    let overrides: BTreeMap<(String, String), Option<String>> = p
        .instances
        .iter()
        .filter_map(|(k, ov)| insts.resolve(k).ok().map(|key| (key, ov.analog.clone())))
        .collect();

    let mut deck_views: Vec<(String, String)> = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    let mut nodes_used: BTreeSet<String> = BTreeSet::new();
    let mut inst_json: Vec<Json> = Vec::new();
    let mut out_of_scope: Vec<Json> = Vec::new();
    let mut flagged = Vec::new();
    let mut seen_views: BTreeSet<String> = BTreeSet::new();
    let mut power_out_nets: BTreeMap<String, String> = BTreeMap::new();
    let mut unqualified = Vec::new();
    let lat_text = |t: &str| t.parse::<Time>().ok();
    for i in board.instances.iter().filter(|i| i.board == sim) {
        let q = format!("{}:{}", i.board, i.reference);
        let skip = |why: &str| Json::obj([("ref", Json::str(&q)), ("reason", Json::str(why))]);
        if i.reference == tref {
            continue;
        }
        if i.status != "ok" {
            continue; // listed from board.bbm in run.json
        }
        if excluded.contains(&(i.board.clone(), i.reference.clone())) {
            out_of_scope.push(skip("excluded by the bench ([analog] exclude)"));
            continue;
        }
        let pr = i.package.as_ref().expect("ok has a package");
        let pkg = &a.imp.packages[&pr.id];
        let ov = overrides
            .get(&(i.board.clone(), i.reference.clone()))
            .cloned()
            .flatten();
        let Some((view, rung)) =
            select_view(pkg, a.fidelity, ov.as_deref()).map_err(|e| format!("{q}: {e}"))?
        else {
            out_of_scope.push(skip(&format!("no analog view at {} or below", a.fidelity)));
            continue;
        };
        let connected: Vec<&bbcore::board::InstancePin> = i
            .pins
            .iter()
            .filter(|x| {
                x.net
                    .as_ref()
                    .is_some_and(|n| !board.net(n).expect("net").unconnected)
            })
            .collect();
        if let Some(x) = connected
            .iter()
            .find(|x| !scoped(x.net.as_deref().expect("net")))
        {
            out_of_scope.push(skip(&format!(
                "pin {} is on {}, outside the analog scope",
                x.number,
                x.net.as_deref().unwrap_or("")
            )));
            continue;
        }
        let ports = port_nodes(i, pkg, view, board).map_err(|e| format!("{q}: {e}"))?;
        let Some(ports) = ports else {
            out_of_scope.push(skip("a view port is on an unconnected pin"));
            continue;
        };
        // The view's parameters; a generic passive's value is the parsed
        // KiCad value (never a SPICE suffix, ADR-006 D5).
        let mut params: Vec<(String, String)> = Vec::new();
        for (k, v) in &view.params {
            let d = parse_number(v).map_err(|e| format!("{q}: view param {k}: {e}"))?;
            params.push((k.clone(), spice_number(d)));
        }
        if let Some(v) = &i.value {
            let dim = match v.unit.as_str() {
                "ohm_u" => Dim::Resistance,
                "farad_a" => Dim::Capacitance,
                _ => Dim::Inductance,
            };
            let s = spice_number(to_decimal(v.value, dim));
            match params.iter_mut().find(|(k, _)| k == "value") {
                Some(e) => e.1 = s,
                None => params.push(("value".into(), s)),
            }
        }
        let text = std::fs::read_to_string(pkg.dir.join(&view.file))
            .map_err(|e| format!("{q}: read view {}: {e}", view.file))?;
        let label = format!("{}:{}", pkg.part.id, view.name);
        if seen_views.insert(label.clone()) {
            deck_views.push((label.clone(), text));
        }
        let mut line = format!(
            "X{}",
            element_fragment(&if board.boards.len() > 1 {
                format!("{}_{}", i.board, i.reference)
            } else {
                i.reference.clone()
            })
        );
        for n in &ports {
            line.push(' ');
            line.push_str(n);
            nodes_used.insert(n.clone());
        }
        line.push(' ');
        line.push_str(&view.subckt);
        for (k, v) in &params {
            line.push_str(&format!(" {k}={v}"));
        }
        lines.push(line);
        // power_out pins drive their net (the two-drivers rule).
        for x in &connected {
            if pkg
                .pins
                .pins
                .iter()
                .any(|pp| pp.number == x.number && pp.role == "power_out")
            {
                power_out_nets.insert(x.net.clone().expect("net"), q.clone());
            }
        }
        let prov = view.provenance.as_ref().or(pkg.part.provenance.as_ref());
        if let Some(pv) = prov {
            if pv.source == "estimated" || pv.source == "agent_generated" {
                let f = format!("{label} ({})", pv.source);
                if !flagged.contains(&f) {
                    flagged.push(f);
                }
            }
        }
        let qualified = view
            .qualified
            .iter()
            .any(|qq| lat_text(&qq.lattice) == Some(a.lattice) && qq.options == a.bench.options);
        if !qualified
            && !a
                .allow_unqualified
                .iter()
                .any(|x| *x == label || *x == view.name)
            && !unqualified.contains(&label)
        {
            unqualified.push(label.clone());
        }
        inst_json.push(Json::obj([
            ("ref", Json::str(&q)),
            ("package", Json::str(&pr.id)),
            ("version", Json::str(&pr.version)),
            ("view", Json::str(&view.name)),
            ("fidelity", Json::str(a.fidelity)),
            ("rung", Json::str(&rung)),
            ("provenance", prov_json(prov)),
            (
                "qualified",
                Json::Arr(
                    view.qualified
                        .iter()
                        .map(|qq| {
                            Json::obj([
                                ("lattice", Json::str(&qq.lattice)),
                                ("options", Json::str(&qq.options)),
                            ])
                        })
                        .collect(),
                ),
            ),
            ("qualified_for_run", Json::Bool(qualified)),
        ]));
    }
    // The pin_load view of every loading adc_in (ADR-008 D7, glue rule 9) is
    // in the deck too: the same qualification rule (ADR-006 D6), recorded in
    // `analog` under the bound pin (ADR-008 Amendment 5).
    for pb in &pins {
        let PinKind::Adc {
            load: Some(vname), ..
        } = &pb.kind
        else {
            continue;
        };
        let view = chip
            .part
            .analog
            .iter()
            .find(|v| v.name == *vname && v.kind == "pin_load")
            .ok_or_else(|| format!("{}: no pin_load view {vname}", chip.part.id))?;
        let label = format!("{}:{}", chip.part.id, view.name);
        let prov = view.provenance.as_ref().or(chip.part.provenance.as_ref());
        if let Some(pv) = prov {
            if pv.source == "estimated" || pv.source == "agent_generated" {
                let f = format!("{label} ({})", pv.source);
                if !flagged.contains(&f) {
                    flagged.push(f);
                }
            }
        }
        let qualified = view
            .qualified
            .iter()
            .any(|qq| lat_text(&qq.lattice) == Some(a.lattice) && qq.options == a.bench.options);
        if !qualified
            && !a
                .allow_unqualified
                .iter()
                .any(|x| *x == label || *x == view.name)
            && !unqualified.contains(&label)
        {
            unqualified.push(label.clone());
        }
        inst_json.push(Json::obj([
            ("ref", Json::str(format!("{sim}:{tref}.{}", pb.pin))),
            ("package", Json::str(&chip.part.id)),
            ("version", Json::str(&chip.part.version)),
            ("view", Json::str(&view.name)),
            ("kind", Json::str("pin_load")),
            ("provenance", prov_json(prov)),
            (
                "qualified",
                Json::Arr(
                    view.qualified
                        .iter()
                        .map(|qq| {
                            Json::obj([
                                ("lattice", Json::str(&qq.lattice)),
                                ("options", Json::str(&qq.options)),
                            ])
                        })
                        .collect(),
                ),
            ),
            ("qualified_for_run", Json::Bool(qualified)),
        ]));
    }
    // An explicit `load = "none"` (ADR-008 D7, Amendment 6): the unloaded,
    // ideal input, recorded under the bound pin with `view = "none"`.
    for pin in &load_none {
        inst_json.push(Json::obj([
            ("ref", Json::str(format!("{sim}:{tref}.{pin}"))),
            ("package", Json::str(&chip.part.id)),
            ("version", Json::str(&chip.part.version)),
            ("view", Json::str("none")),
            ("kind", Json::str("pin_load")),
        ]));
    }
    if !unqualified.is_empty() {
        return Err(format!(
            "views not lattice-qualified for (lattice {}, option set {}) (ADR-006 D6): {}; \
             qualify them (`bb pkg test --record`) or pass --allow-unqualified <package:view>",
            a.lattice,
            a.bench.options,
            unqualified.join(", ")
        ));
    }

    // --- Bench sources and loads.
    let node_of = |d: &str| board.net(d).expect("net").spice.clone();
    let mut bench_lines = Vec::new();
    for s in &a.bench.supplies {
        let d = net_by(&s.net, "[[supply]]")?;
        if !scoped(&d) {
            return Err(format!(
                "bench [[supply]] on {d}: the net is outside the analog scope"
            ));
        }
        if let Some(drv) = power_out_nets.get(&d) {
            return Err(format!(
                "bench [[supply]] on {d}: {drv} also drives it (a power_out pin): two drivers; \
                 exclude {drv} in the bench ([analog] exclude, ADR-007 D5)"
            ));
        }
        let n = node_of(&d);
        if n == "0" {
            return Err(format!("bench [[supply]] on the ground net {d}"));
        }
        let f = element_fragment(&n);
        let v = spice_number(to_decimal(s.volts_uv, Dim::Voltage));
        // The source voltage: fixed, or tracking another net.
        let target = match &s.track {
            None => v.clone(),
            Some((tn, drop)) => {
                let td = net_by(tn, "[[supply]] track")?;
                let tnode = node_of(&td);
                nodes_used.insert(tnode.clone());
                format!(
                    "min({v}, V({tnode}) - {})",
                    spice_number(to_decimal(*drop, Dim::Voltage))
                )
            }
        };
        if s.rs_uohm > 0 {
            // With a series resistance the supply is its Norton equivalent,
            // one B conductance into the net: no branch-current unknown, so
            // a milliohm rs in front of a rail's capacitors does not ask
            // ngspice for node voltages below double resolution at phase0
            // (ADR-007 Amendment 3; AGENTS.md, ADR-006 Amendment 2).
            bench_lines.push(format!(
                "B_bench_{f} 0 {n} I = ({target} - V({n})) / {}",
                spice_number(to_decimal(s.rs_uohm, Dim::Resistance))
            ));
        } else if s.track.is_some() {
            bench_lines.push(format!("B_bench_{f} {n} 0 V = {target}"));
        } else {
            bench_lines.push(format!("V_bench_{f} {n} 0 DC {target}"));
        }
        nodes_used.insert(n);
    }
    for (k, l) in a.bench.loads.iter().enumerate() {
        let d = net_by(&l.net, "[[load]]")?;
        let n = node_of(&d);
        bench_lines.push(format!(
            "I_bench_load{k}_{} {n} 0 DC {}",
            element_fragment(&n),
            spice_number(to_decimal(l.amps_pa, Dim::Current))
        ));
        nodes_used.insert(n);
    }
    for pw in &a.bench.pwl {
        let d = net_by(&pw.net, "[[pwl]]")?;
        let n = node_of(&d);
        let f = element_fragment(&n);
        let src = if pw.rs_uohm > 0 {
            format!("bb_pwl_{f}")
        } else {
            n.clone()
        };
        let pts: Vec<String> = pw
            .points
            .iter()
            .map(|(t, v)| {
                format!(
                    "{} {}",
                    spice_number(to_decimal(*t, Dim::Time)),
                    spice_number(to_decimal(*v, Dim::Voltage))
                )
            })
            .collect();
        bench_lines.push(format!("V_pwl_{f} {src} 0 PWL({})", pts.join(" ")));
        if pw.rs_uohm > 0 {
            bench_lines.push(format!(
                "R_pwl_{f} {src} {n} {}",
                spice_number(to_decimal(pw.rs_uohm, Dim::Resistance))
            ));
        }
        nodes_used.insert(n);
    }
    for pb in &pins {
        match &pb.kind {
            PinKind::Out { net, .. } | PinKind::In { net, .. } | PinKind::Adc { net, .. } => {
                nodes_used.insert(net.clone());
            }
            PinKind::OutUnconnected => {}
        }
    }

    // --- The deck.
    let mut deck = format!(
        "* Breadboard elaborated deck (ADR-007 D7): board {sim}, fidelity {}, option set {}\n",
        a.fidelity, a.bench.options
    );
    deck.push_str(&bbpkg::spice::assemble(&deck_views)?);
    for l in &lines {
        deck.push_str(l);
        deck.push('\n');
    }
    for l in &bench_lines {
        deck.push_str(l);
        deck.push('\n');
    }
    if let Some(t) = a.bench.temp_mc {
        deck.push_str(&format!(
            ".temp {}\n",
            spice_number(to_decimal(t, Dim::Temperature))
        ));
    }
    deck.push_str(bbpkg::optsets::get(&a.bench.options).expect("checked"));
    deck.push('\n');

    // --- Probes: bb.toml [probes], then --probe.
    let mut names_in: Vec<String> = p.probes.clone();
    for c in a.cli_probes {
        if !names_in.contains(c) {
            names_in.push(c.clone());
        }
    }
    if names_in.is_empty() {
        return Err("no probes: give --probe net:<name> or [probes] nets in bb.toml".into());
    }
    let mut probes: Vec<ProbeSpec> = Vec::new();
    for n in &names_in {
        let spec = match probe_target(board, sim, n).map_err(|e| format!("probe {n}: {e}"))? {
            ProbeTarget::Pin(b, r, pin) => {
                let k = (b == tb && r == tref)
                    .then(|| bound.get(&pin).cloned())
                    .flatten()
                    .ok_or_else(|| {
                        format!("probe {n}: pin {b}:{r}.{pin} has no [[bind]] on the target MCU")
                    })?;
                ProbeSpec {
                    name: k,
                    node: None,
                }
            }
            ProbeTarget::Net(d) => {
                let node = node_of(&d);
                if node == "0" {
                    return Err(format!("probe {n}: {d} is the ground (node 0)"));
                }
                if !nodes_used.contains(&node) {
                    return Err(format!(
                    "probe {n}: net {d} has nothing in the analog deck (outside the scope, or only unmodelled parts on it)"
                ));
                }
                ProbeSpec {
                    name: d,
                    node: Some(node),
                }
            }
        };
        if probes.iter().any(|x| x.name == spec.name) {
            return Err(format!("probe {n}: {} is given twice", spec.name));
        }
        probes.push(spec);
    }

    // --- run.json (static part).
    let not_simulated: Vec<Json> = board
        .instances
        .iter()
        .filter(|i| i.board == sim && i.status != "ok")
        .map(|i| {
            let mut o = vec![
                (
                    "ref".to_string(),
                    Json::str(format!("{}:{}", i.board, i.reference)),
                ),
                ("status".to_string(), Json::str(&i.status)),
            ];
            if let Some(r) = &i.reason {
                o.push(("reason".into(), Json::str(r)));
            }
            Json::Obj(o)
        })
        .collect();
    let bbm_text = board.to_text();
    let import_boards = a
        .imp
        .import_json
        .get("boards")
        .cloned()
        .unwrap_or(Json::Arr(vec![]));
    let run_json = vec![
        ("schema".to_string(), Json::str("bb-run/0")),
        ("board_bbm".to_string(), Json::str(sha(bbm_text.as_bytes()))),
        ("import".to_string(), import_boards),
        (
            "bench".to_string(),
            Json::obj([
                (
                    "file",
                    a.bench_name.as_ref().map(Json::str).unwrap_or(Json::Null),
                ),
                ("text", Json::str(&a.bench.text)),
                ("sha256", Json::str(sha(a.bench.text.as_bytes()))),
                ("options", Json::str(&a.bench.options)),
                (
                    "options_card",
                    Json::str(bbpkg::optsets::get(&a.bench.options).expect("checked")),
                ),
            ]),
        ),
        (
            "targets".to_string(),
            Json::Arr(vec![Json::obj([
                ("instance", Json::str(format!("{tb}:{tref}"))),
                (
                    "images",
                    Json::Arr(
                        images
                            .iter()
                            .map(|im| {
                                Json::obj([
                                    ("kind", Json::str(&im.kind)),
                                    ("hash", Json::str(&im.hash)),
                                ])
                            })
                            .collect(),
                    ),
                ),
            ])]),
        ),
        ("analog".to_string(), Json::Arr(inst_json)),
        ("out_of_scope".to_string(), Json::Arr(out_of_scope)),
        ("not_simulated".to_string(), Json::Arr(not_simulated)),
        (
            "packages".to_string(),
            Json::Arr(
                a.imp
                    .locked_packages
                    .iter()
                    .map(|l| {
                        Json::obj([
                            ("id", Json::str(&l.id)),
                            ("version", Json::str(&l.version)),
                            ("tree", Json::str(&l.tree)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "platforms".to_string(),
            Json::Arr(
                a.imp
                    .locked_platforms
                    .iter()
                    .map(|l| {
                        Json::obj([
                            ("id", Json::str(&l.id)),
                            ("content", Json::str(&l.content)),
                            (
                                "renode",
                                l.renode.as_ref().map(Json::str).unwrap_or(Json::Null),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("deck_sha256".to_string(), Json::str(sha(deck.as_bytes()))),
    ];

    Ok(RunPlan {
        board: board::Board {
            parts: vec![part],
            pins,
            spice: deck,
            packages: vec![chip.clone()],
            warnings: Vec::new(),
            audit: Some(audit),
        },
        target: tref,
        firmware,
        probes,
        run_json,
        warnings,
        flagged,
    })
}

/// The SPICE nodes of a view's ports for one instance, in port order.
/// `None` when a port's pin is not connected.
fn port_nodes(
    i: &Instance,
    pkg: &Package,
    view: &View,
    board: &Board,
) -> Result<Option<Vec<String>>, String> {
    let mut out = Vec::new();
    for port in &view.ports {
        let Port::Pin(pname) = port else {
            return Err(format!(
                "view {} has typed ports (a pin_load view); it cannot be a circuit instance",
                view.name
            ));
        };
        let numbers: Vec<&str> = pkg
            .pins
            .pins
            .iter()
            .filter(|pp| &pp.name == pname)
            .map(|pp| pp.number.as_str())
            .collect();
        if numbers.is_empty() {
            return Err(format!(
                "view {} port {pname}: no pin of that name in {}'s pins.toml",
                view.name, pkg.part.id
            ));
        }
        let mut node: Option<String> = None;
        for num in numbers {
            let net = i
                .pins
                .iter()
                .find(|x| x.number == num)
                .and_then(|x| x.net.as_deref())
                .filter(|n| !board.net(n).expect("net").unconnected);
            let Some(net) = net else {
                return Ok(None);
            };
            let s = board.net(net).expect("net").spice.clone();
            match &node {
                None => node = Some(s),
                Some(prev) if *prev == s => {}
                Some(prev) => {
                    return Err(format!(
                        "port {pname}: its pins are on different nets ({prev}, {s})"
                    ))
                }
            }
        }
        out.push(node.expect("at least one"));
    }
    Ok(Some(out))
}

/// A project directory's bench path as recorded (file name only, no
/// absolute paths in run.json).
pub fn bench_label(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// What a probe name resolves to, for `bb probe`: a net's display name, or
/// `pin <board>:<ref>.<pin>`.
pub fn describe_probe(board: &Board, simulated: &str, name: &str) -> Result<String, String> {
    Ok(match probe_target(board, simulated, name)? {
        ProbeTarget::Net(d) => d,
        ProbeTarget::Pin(b, r, p) => format!("pin {b}:{r}.{p}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bbpkg::schema::Registry;

    fn tps22917() -> Package {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../packages/ti/tps22917");
        bbpkg::package::load(&dir, &Registry::current()).unwrap()
    }

    #[test]
    fn select_view_override_default_and_ladder() {
        // ADR-006 D3 step 1 (the post-M7 review found the override untested).
        let pkg = tps22917();
        let (v, rung) = select_view(&pkg, "fast", None).unwrap().unwrap();
        assert_eq!((v.name.as_str(), rung.as_str()), ("lsw", "fast"));
        // The project override wins and is recorded as such.
        let (v, rung) = select_view(&pkg, "fast", Some("lsw")).unwrap().unwrap();
        assert_eq!((v.name.as_str(), rung.as_str()), ("lsw", "override"));
        // An unknown view name is refused, naming the package.
        let e = select_view(&pkg, "fast", Some("vendor")).unwrap_err();
        assert!(
            e.contains("package ti/tps22917 has no analog view \"vendor\""),
            "{e}"
        );
        // The ladder: at a higher rung with no default there, the first
        // lower rung that has one is chosen.
        let (v, rung) = select_view(&pkg, "si", None).unwrap().unwrap();
        assert_eq!((v.name.as_str(), rung.as_str()), ("lsw", "fast"));
        assert!(select_view(&pkg, "spice", None).is_err());
        // A pin_load view is never selected as the part's circuit.
        let mut p2 = pkg.clone();
        p2.part.analog[0].kind = "pin_load".into();
        assert!(select_view(&p2, "fast", None).unwrap().is_none());
    }
}
