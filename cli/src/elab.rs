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
use bbcore::{Level, Time};
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
    /// What the bench drives at run time (ADR-010 D3, D4).
    pub stimulus: Stimulus,
    /// The run's assertions (the bench's, then the `--assert` files'), their
    /// nets the kernel's (ADR-010 D5).
    pub asserts: Vec<bbcore::assert::Spec>,
    /// The analog nets they read: `(kernel net, SPICE node)`.
    pub assert_nodes: Vec<(String, String)>,
}

/// What a `bb-bench/1` bench drives at run time (ADR-010 D3, D4).
#[derive(Clone, Debug, Default)]
pub struct Stimulus {
    /// The analog engine's bench tables: every source whose changes are
    /// known before the run (playback, profiles, timed presses and sets,
    /// faults), handed over once.
    pub tables: Vec<bb_analog_ngspice::TableSource>,
    /// The bench engine's outputs: a bound MCU `digital_in` pin by its kernel
    /// net (`U1.PB0`), or a source of the deck that a triggered action
    /// drives, by its element name (in [`Stimulus::analog`]).
    pub outputs: Vec<bbbench::OutputSpec>,
    /// The outputs that are analog sources (`Drive::Timed` inputs of the
    /// analog engine).
    pub analog: BTreeSet<String>,
    pub triggers: Vec<bbbench::TriggerSpec>,
}

impl Stimulus {
    /// Whether the run has a bench engine.
    pub fn engine(&self) -> bool {
        !self.outputs.is_empty() || !self.triggers.is_empty()
    }

    /// The kernel nets of the MCU pins the bench drives.
    pub fn mcu_pins(&self) -> BTreeSet<String> {
        self.outputs
            .iter()
            .map(|o| o.pin.clone())
            .filter(|p| !self.analog.contains(p))
            .collect()
    }
}

/// A switch instance in the deck (ADR-010 D2).
struct SwitchInst {
    ctl_node: String,
    ctl_source: String,
    bounce_max: Option<i128>,
}

/// What an action or a fault drives.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Target {
    /// A switch's control source.
    Switch(String),
    /// A supply's set source.
    Supply(String),
    /// A bound MCU `digital_in` pin (its kernel net).
    Pin(String),
}

/// One action on a target: its start (or trigger) and its program.
struct Planned {
    path: String,
    at: Option<Time>,
    trigger: Option<(String, Level, Time)>,
    program: Vec<(Time, bbbench::Value)>,
}

fn fs_time(fs: i128) -> Time {
    Time::from_fs(u64::try_from(fs).unwrap_or(u64::MAX))
}

/// A table's points from an initial value and changes (in time order):
/// the changes at one instant keep the last, a change at 0 replaces the
/// initial value.
fn table_points(initial: f64, changes: &[(Time, f64)]) -> Vec<(Time, f64)> {
    let mut pts: Vec<(Time, f64)> = vec![(Time::ZERO, initial)];
    for &(t, v) in changes {
        match pts.last_mut() {
            Some(last) if last.0 == t => last.1 = v,
            _ => pts.push((t, v)),
        }
    }
    pts
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

fn nets<'a>(board: &'a Board, simulated: &'a [String]) -> Nets<'a> {
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
fn resolve_net_or_alias(board: &Board, simulated: &[String], name: &str) -> Result<String, String> {
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
fn probe_target(board: &Board, simulated: &[String], name: &str) -> Result<ProbeTarget, String> {
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
        match probe_target(board, &p.simulated, pr) {
            Ok(ProbeTarget::Net(_)) => {}
            Ok(ProbeTarget::Pin(b, r, pin)) => {
                let bound = p.binds.iter().any(|x| {
                    insts.resolve_pin(&x.pin).ok() == Some((b.clone(), r.clone(), pin.clone()))
                }) || p.devices.iter().any(|d| {
                    device_ref(p, &d.reference).ok() == Some((b.clone(), r.clone()))
                        && d.binds.iter().any(|x| x.pin == pin)
                });
                if !bound {
                    out.push(format!(
                        "[probes] {pr}: pin {b}:{r}.{pin} has no [[bind]] (only bound MCU and device pins are probed by pin name)"
                    ));
                }
            }
            Err(e) => out.push(format!("[probes] {pr}: {e}")),
        }
    }
    out
}

/// A `[[device]]` reference (ADR-013 D1): `<board>:<ref>`, or a bare `<ref>`
/// in a one-board project. Returns `(board, ref)`.
pub fn device_ref(p: &Project, reference: &str) -> Result<(String, String), String> {
    match reference.split_once(':') {
        Some((b, r)) => Ok((b.to_string(), r.to_string())),
        None if p.boards.len() == 1 => Ok((p.boards[0].id.clone(), reference.to_string())),
        None => Err(format!(
            "[[device]] {reference}: qualify the reference as <board>:{reference} (ADR-007 D7)"
        )),
    }
}

/// One `[[device]]` of a run (ADR-013 D1): its part for the device engine,
/// its pin bindings (built from the device package's `pins.toml`, as a board
/// file's device part's are) and its `run.json` record.
pub struct DevicePlan {
    pub board: String,
    pub part: Part,
    pub pins: Vec<PinBinding>,
    pub package: Package,
    pub json: Json,
}

/// The project's devices: each one's parameters checked against the device
/// (unknown keys, dimensions, ranges), each binding's pin and model against
/// the device's pins (spec/package.md section 12), each pin built with the
/// package's parameter set. `bb open` calls it to report problems; `bb run`
/// to run them. `import` has already checked the packages, the pins' names
/// and the nets.
pub fn plan_devices(
    p: &Project,
    imp: &Imported,
    warnings: &mut Vec<String>,
) -> Result<Vec<DevicePlan>, String> {
    let board = &imp.board;
    let mut out = Vec::new();
    for d in &p.devices {
        let ctx = format!("bb.toml [[device]] {}", d.reference);
        let (db, dr) = device_ref(p, &d.reference)?;
        let inst = board
            .instance(&db, &dr)
            .ok_or_else(|| format!("{ctx}: {db}:{dr} is not in board.bbm (run `bb open`)"))?;
        let pr = inst
            .package
            .as_ref()
            .ok_or_else(|| format!("{ctx}: {db}:{dr} has no package"))?;
        let pkg = imp
            .packages
            .get(&pr.id)
            .ok_or_else(|| format!("{ctx}: package {} is not resolved", pr.id))?;
        let did = pkg
            .part
            .behavioral
            .as_ref()
            .filter(|b| b.engine == "device")
            .and_then(|b| b.device.as_deref())
            .ok_or_else(|| format!("{ctx}: package {} is not a device", pkg.part.id))?;
        let def = bbdev::find(did)
            .ok_or_else(|| format!("{ctx}: device {did} is not one this bb provides"))?;
        let kv: BTreeMap<&str, &str> = d
            .params
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let params = board::device_params(def, &kv).map_err(|e| format!("{ctx}: {e}"))?;
        let part = Part {
            reference: dr.clone(),
            engine: "device".into(),
            platform: def.id.to_string(),
            package: Some(pkg.part.id.clone()),
            params,
        };
        let mut pins = Vec::new();
        for b in &d.binds {
            let bctx = format!("{ctx} [[device.bind]] {}", b.pin);
            board::device_pin(&part, &b.pin, &b.model).map_err(|e| format!("{bctx}: {e}"))?;
            let number = pkg
                .pins
                .by_name(&b.pin)
                .map(|x| x.number.clone())
                .ok_or_else(|| format!("{bctx}: {} has no pin {}", pkg.part.id, b.pin))?;
            let net = inst
                .pins
                .iter()
                .find(|x| x.number == number)
                .and_then(|x| x.net.clone())
                .ok_or_else(|| format!("{bctx}: the pin is on no net (run `bb open`)"))?;
            let n = board.net(&net).expect("net");
            if n.unconnected {
                return Err(format!("{bctx}: {net} is an unconnected net"));
            }
            let explicit: Vec<(&str, &str)> = b
                .params
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            let name = format!("{dr}.{}", b.pin);
            let kind = build_pin(
                &PinReq {
                    ctx: &bctx,
                    name: &name,
                    part: &part,
                    pkg: Some(pkg),
                    pin: &b.pin,
                    model: &b.model,
                    set: b.set.as_deref(),
                    net: Some(n.spice.clone()),
                    explicit: &explicit,
                    supply: None,
                },
                warnings,
            )?;
            pins.push(PinBinding {
                part: dr.clone(),
                pin: b.pin.clone(),
                kind,
            });
        }
        let json = Json::obj([
            ("ref", Json::str(format!("{db}:{dr}"))),
            ("package", Json::str(&pr.id)),
            ("version", Json::str(&pr.version)),
            ("device", Json::str(did)),
            ("virtual", Json::Bool(inst.is_virtual)),
            (
                "params",
                Json::Obj(
                    d.params
                        .iter()
                        .map(|(k, v)| (k.clone(), Json::str(v)))
                        .collect(),
                ),
            ),
        ]);
        out.push(DevicePlan {
            board: db,
            part,
            pins,
            package: pkg.clone(),
            json,
        });
    }
    Ok(out)
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
    /// `--assert` files' assertions (ADR-010 Amendment 3), after the bench's.
    pub extra_asserts: &'a [bbcore::assert::Spec],
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
    // A system run (ADR-013 D2): several simulated boards in one deck. A
    // [[connection]] between two of them without a harness ties its mated
    // nets directly: they become one node (`tied_board`).
    let tied = tied_board(&a.imp.board, &p.simulated, &p.simulate);
    let board = tied.as_ref().unwrap_or(&a.imp.board);
    let sim = p.simulate.as_str();
    let simset: &[String] = &p.simulated;
    let system = simset.len() > 1;
    let simulated = |b: &str| simset.iter().any(|s| s == b);
    let insts = instances(board);
    let mut warnings = a.bench.warnings.clone();
    // Each simulated board keeps its own ground (ADR-013 D2): the primary
    // board's is node 0, another's a named node. A bench element returns to
    // the ground of its net's board.
    let ground_of = |d: &str| -> String {
        let nb = &board.net(d).expect("net").board;
        board
            .boards
            .iter()
            .find(|b| b.id == *nb)
            .and_then(|b| board.net(&b.ground))
            .map(|n| n.spice.clone())
            .unwrap_or_else(|| "0".into())
    };

    // Names first, so a bad --probe fails before anything else is checked.
    for n in a.cli_probes {
        probe_target(board, simset, n).map_err(|e| format!("--probe {n}: {e}"))?;
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
        .filter(|(b, ..)| simulated(b))
        .collect();
    let (tb, tref, images) =
        match sim_targets.as_slice() {
            [one] => one.clone(),
            [] => return Err(
                "no firmware target: run `bb flash --target <instance> <file.elf>` (ADR-007 D7)"
                    .into(),
            ),
            many if system => {
                return Err(format!(
                    "targets {}: multi-MCU simulation is Phase 3 (ADR-013 D2): a system run has \
                 exactly one target across its simulated boards ({})",
                    many.iter()
                        .map(|(b, r, _)| format!("{b}:{r}"))
                        .collect::<Vec<_>>()
                        .join(", "),
                    simset.join(", ")
                ))
            }
            _ => return Err("Phase 1 runs exactly one MCU target on the simulated board".into()),
        };
    if tb != sim {
        return Err(format!(
            "the target {tb}:{tref} is on board {tb}, but the system's primary board (its ground \
             node 0) is {sim}: qualify the [targets] key as \"{tb}:{tref}\" (ADR-013 D2)"
        ));
    }
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
        params: Vec::new(),
    };

    // --- Bench names.
    let net_by = |name: &str, what: &str| -> Result<String, String> {
        let d = resolve_net_or_alias(board, simset, name)
            .map_err(|e| format!("bench {what} {name}: {e}"))?;
        // Only the simulated boards are in the deck (ADR-007 D5, ADR-013 D2).
        match board.net(&d) {
            Some(n) if !simulated(&n.board) => Err(format!(
                "bench {what} {name}: {d} is on board {}, which is not simulated (the run \
                 simulates {} only, ADR-007 D5)",
                n.board,
                simset.join(", ")
            )),
            _ => Ok(d),
        }
    };
    let mut supply_on: BTreeMap<String, i128> = BTreeMap::new();
    for s in &a.bench.supplies {
        let d = net_by(&s.net, "[[supply]]")?;
        if supply_on.insert(d.clone(), s.volts_uv).is_some() {
            return Err(format!("bench: two [[supply]] on net {d}"));
        }
    }
    // [[battery]] (ADR-010 D2, milestone P1): its OCV at the bench's state
    // of charge drives its net, as a supply would.
    let mut battery_ocv: Vec<(String, i128)> = Vec::new();
    for bt in &a.bench.batteries {
        let d = net_by(&bt.net, "[[battery]]")?;
        let what = format!("bench {}.ocv {}", bt.path, bt.ocv);
        let text = a
            .bench
            .data
            .get(&bt.ocv)
            .ok_or_else(|| format!("{what}: not read"))?;
        let rows = crate::project::ocv_table(text, &what)?;
        let v = crate::project::ocv_at(&rows, bt.soc_ppm);
        if supply_on.insert(d.clone(), v).is_some() {
            return Err(format!(
                "bench {}: net {d} has another [[supply]] or [[battery]]",
                bt.path
            ));
        }
        battery_ocv.push((d, v));
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
                resolve_net_or_alias(board, simset, want).map_err(|e| format!("{ctx}: net {e}"))?;
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

    // --- Devices (`[[device]]`, bb-project/1; ADR-013 D1): the device
    // engine's parts and pins, routed as a board file's device parts are
    // (spec/package.md section 12).
    let devs = plan_devices(p, a.imp, &mut warnings)?;
    let mut dev_pins: Vec<PinBinding> = Vec::new();
    // (ref, pin) -> kernel net, for probes by pin name.
    let mut dev_bound: BTreeMap<(String, String), String> = BTreeMap::new();
    let mut dev_refs: BTreeSet<(String, String)> = BTreeSet::new();
    for d in &devs {
        let ctx = format!("bb.toml [[device]] {}:{}", d.board, d.part.reference);
        if d.board != sim {
            // A device's kernel nets are `<ref>.<pin>` without the board, as
            // the target's are; in a system run devices stay on the target's
            // board (ADR-013 Amendment 3).
            return Err(if simulated(&d.board) {
                format!(
                    "{ctx}: board {} is simulated, but a system run's devices are on the \
                     target's board {sim} (ADR-013 Amendment 3)",
                    d.board
                )
            } else {
                format!(
                    "{ctx}: board {} is not simulated (a run simulates the devices of {sim})",
                    d.board
                )
            });
        }
        if d.part.reference == tref {
            return Err(format!("{ctx}: the reference is the target MCU's"));
        }
        dev_refs.insert((d.board.clone(), d.part.reference.clone()));
        for pb in &d.pins {
            if let PinKind::Out { net, .. } = &pb.kind {
                if pins.iter().chain(dev_pins.iter()).any(
                    |x| matches!(&x.kind, PinKind::Out { net: n, .. } if n.eq_ignore_ascii_case(net)),
                ) {
                    return Err(format!(
                        "{ctx}: {}: net {net} is already driven by another pin",
                        pb.pin
                    ));
                }
            }
            dev_bound.insert((pb.part.clone(), pb.pin.clone()), pb.kernel_net());
            dev_pins.push(pb.clone());
        }
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

    // --- Digital-only nets (ADR-014 D4, ADR-013 D1): a bound digital net
    // outside `[analog] nets` gets no glue. Its MCU and device pins are
    // routed through the kernel net only (a device's direct routing,
    // spec/package.md section 12), so every input on it must read a digital
    // output that a device drives or that a device reads. Without `[analog]
    // nets` every net is in scope and nothing changes.
    let display_of: BTreeMap<&str, &str> = board
        .nets
        .iter()
        .filter(|n| n.board == sim)
        .map(|n| (n.spice.as_str(), n.name.as_str()))
        .collect();
    let mut digital_only: BTreeSet<String> = BTreeSet::new();
    let mut digital_only_names: BTreeSet<String> = BTreeSet::new();
    for pb in pins.iter().chain(dev_pins.iter()) {
        let (PinKind::Out { net, .. } | PinKind::In { net, .. }) = &pb.kind else {
            continue;
        };
        let d = display_of
            .get(net.as_str())
            .copied()
            .unwrap_or(net.as_str());
        if !scoped(d) {
            digital_only.insert(net.clone());
            digital_only_names.insert(d.to_string());
        }
    }
    {
        let is_dev = |part: &str| dev_pins.iter().any(|x| x.part == part);
        for r in pins.iter().chain(dev_pins.iter()) {
            let PinKind::In { net, .. } = &r.kind else {
                continue;
            };
            if !digital_only.contains(net) {
                continue;
            }
            let routed = pins.iter().chain(dev_pins.iter()).any(|o| {
                o.part != r.part
                    && (is_dev(&o.part) || is_dev(&r.part))
                    && matches!(&o.kind, PinKind::Out { net: n, .. } if n.eq_ignore_ascii_case(net))
            });
            if !routed {
                return Err(format!(
                    "{}: it reads {}, which is outside the bench's [analog] nets (a digital-only \
                     net, ADR-014 D4), and no device output drives it for a device or the MCU to \
                     read directly; put the net in [analog] nets",
                    r.kernel_net(),
                    display_of
                        .get(net.as_str())
                        .copied()
                        .unwrap_or(net.as_str())
                ));
            }
        }
    }

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
    // ADR-009 D8 (milestone P1): the bench's corners, per instance over the
    // bench's `corner`.
    let mut inst_corner: BTreeMap<(String, String), String> = BTreeMap::new();
    for (k, c) in &a.bench.corners {
        let key = insts
            .resolve(k)
            .map_err(|x| format!("bench [corners] {k:?}: {x}"))?;
        inst_corner.insert(key, c.clone());
    }
    let mut corners_used: BTreeSet<(String, String)> = BTreeSet::new();
    let mut global_corner_used = false;

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
    // ADR-010 D2: the instances with a switch view in the deck, and the
    // instance pins an open fault moves off their net.
    let mut switches: BTreeMap<(String, String), SwitchInst> = BTreeMap::new();
    let mut opened: BTreeMap<(String, String), BTreeMap<String, String>> = BTreeMap::new();
    let mut fault_nodes: Vec<(String, String)> = Vec::new();
    for (k, f) in a.bench.faults.iter().enumerate() {
        let d = net_by(&f.net, "[[fault]]")?;
        let n = board.net(&d).expect("net").spice.clone();
        if n == ground_of(&d) {
            return Err(format!("bench {}: the net {d} is the ground", f.path));
        }
        if !scoped(&d) {
            return Err(format!(
                "bench {}: the net {d} is outside the analog scope",
                f.path
            ));
        }
        match f.kind {
            crate::project::FaultKind::Short => fault_nodes.push((n, ground_of(&d))),
            crate::project::FaultKind::Open => {
                let spec = f.pin.as_deref().expect("parsed");
                let (b, r, pin) = insts
                    .resolve_pin(spec)
                    .map_err(|e| format!("bench {}.pin: {e}", f.path))?;
                let inst = board.instance(&b, &r).expect("resolved");
                let ip = inst
                    .pins
                    .iter()
                    .find(|x| x.number == pin)
                    .or_else(|| {
                        let named: Vec<_> = inst.pins.iter().filter(|x| x.name == pin).collect();
                        (named.len() == 1).then(|| named[0])
                    })
                    .ok_or_else(|| {
                        format!(
                            "bench {}.pin: {b}:{r} has no pin numbered or named {pin}",
                            f.path
                        )
                    })?;
                if ip.net.as_deref() != Some(d.as_str()) {
                    return Err(format!(
                        "bench {}.pin: {b}:{r} pin {} is on {}, not on {d}",
                        f.path,
                        ip.number,
                        ip.net.as_deref().unwrap_or("no net")
                    ));
                }
                let node = format!("bb_fault{k}");
                if opened
                    .entry((b.clone(), r.clone()))
                    .or_default()
                    .insert(ip.number.clone(), node.clone())
                    .is_some()
                {
                    return Err(format!(
                        "bench {}: {b}:{r} pin {} is opened twice",
                        f.path, ip.number
                    ));
                }
                fault_nodes.push((n, node));
            }
        }
    }
    let no_open = BTreeMap::new();
    let mut emitted: BTreeSet<(String, String)> = BTreeSet::new();
    // Every simulated board's instances (ADR-013 D2), sorted by board, then
    // reference (board.bbm's order).
    for i in board.instances.iter().filter(|i| simulated(&i.board)) {
        let q = format!("{}:{}", i.board, i.reference);
        let skip = |why: &str| Json::obj([("ref", Json::str(&q)), ("reason", Json::str(why))]);
        if i.board == tb && i.reference == tref {
            continue;
        }
        // A device (ADR-013 D1) runs in the device engine (run.json
        // `devices`); a virtual instance is never in the deck.
        if i.is_virtual || dev_refs.contains(&(i.board.clone(), i.reference.clone())) {
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
        let key = (i.board.clone(), i.reference.clone());
        let ports = port_nodes(i, pkg, view, board, opened.get(&key).unwrap_or(&no_open))
            .map_err(|e| format!("{q}: {e}"))?;
        let Some(mut ports) = ports else {
            out_of_scope.push(skip("a view port is on an unconnected pin"));
            continue;
        };
        // The view's parameters; a generic passive's value is the parsed
        // KiCad value (never a SPICE suffix, ADR-006 D5).
        let mut params: Vec<(String, String)> = Vec::new();
        let switch = view.kind == "switch";
        if switch {
            // ADR-010 D2: the generated element, its control node last.
            let (ron, roff) =
                bbpkg::package::switch_params(view).map_err(|e| format!("{q}: {e}"))?;
            params.push(("ron".into(), spice_number(to_decimal(ron, Dim::Resistance))));
            params.push((
                "roff".into(),
                spice_number(to_decimal(roff, Dim::Resistance)),
            ));
            let frag = element_fragment(&if board.boards.len() > 1 {
                format!("{}_{}", i.board, i.reference)
            } else {
                i.reference.clone()
            });
            let ctl = format!("bb_ctl_{frag}");
            ports.push(ctl.clone());
            switches.insert(
                key.clone(),
                SwitchInst {
                    ctl_source: format!("V_{ctl}"),
                    ctl_node: ctl,
                    bounce_max: bbpkg::package::switch_bounce_max(view)
                        .map_err(|e| format!("{q}: {e}"))?,
                },
            );
        } else {
            for (k, v) in &view.params {
                let d = parse_number(v).map_err(|e| format!("{q}: view param {k}: {e}"))?;
                params.push((k.clone(), spice_number(d)));
            }
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
        // ADR-009 D8: the instance's corner overrides the view's params. A
        // per-instance corner must exist in the view; the bench's global
        // corner applies where the view declares it.
        let corner = match inst_corner.get(&key) {
            Some(c) => {
                corners_used.insert(key.clone());
                Some(view.corners.iter().find(|x| x.name == *c).ok_or_else(|| {
                    format!(
                        "bench [corners] {q}: view {} of {} has no corner {c:?} (it has: \
                                 {})",
                        view.name,
                        pkg.part.id,
                        if view.corners.is_empty() {
                            "none".to_string()
                        } else {
                            view.corners
                                .iter()
                                .map(|x| x.name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        }
                    )
                })?)
            }
            None => a
                .bench
                .corner
                .as_ref()
                .and_then(|c| view.corners.iter().find(|x| x.name == *c)),
        };
        if let Some(c) = corner {
            if !inst_corner.contains_key(&key) {
                global_corner_used = true;
            }
            for (k, v) in &c.params {
                let d = parse_number(v).map_err(|e| format!("{q}: corner {}: {k}: {e}", c.name))?;
                match params.iter_mut().find(|(n, _)| n == k) {
                    Some(e) => e.1 = spice_number(d),
                    None => params.push((k.clone(), spice_number(d))),
                }
            }
        }
        let label = format!("{}:{}", pkg.part.id, view.name);
        if switch {
            // One generated definition for every switch (ADR-010 D2).
            if seen_views.insert("bb:switch".into()) {
                deck_views.push(("bb:switch".into(), bbpkg::package::switch_definition()));
            }
        } else {
            let text = std::fs::read_to_string(pkg.dir.join(&view.file))
                .map_err(|e| format!("{q}: read view {}: {e}", view.file))?;
            if seen_views.insert(label.clone()) {
                deck_views.push((label.clone(), text));
            }
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
        emitted.insert(key.clone());
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
        let mut ij = Json::obj([
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
        ]);
        // ADR-009 D8: the corner the instance runs at (none: the view's own
        // parameters, so a run without corners keeps its run.json).
        if let (Some(c), Json::Obj(o)) = (corner, &mut ij) {
            o.push(("corner".into(), Json::str(&c.name)));
        }
        inst_json.push(ij);
    }
    // --- Harnesses (ADR-013 D2, ADR-007 Amendment 5): a [[connection]]
    // between two simulated boards is instantiated through its harness
    // view, after the instances, in board.bbm's connection order. Port
    // `A<pin>` is on the net of `a`'s pin, `B<pin>` on `b`'s. A mated pair
    // whose two nets are both outside the analog scope is a dead wire, both
    // ends on node 0 (no current); one net in scope and its mate not is an
    // error, since the wire joins them. A connection without a harness was
    // tied directly (`tied_board`).
    for (k, c) in board.connections.iter().enumerate() {
        let end_board = |e: &str| e.split_once(':').map(|x| x.0.to_string());
        let (Some(ab), Some(bb_)) = (end_board(&c.a), end_board(&c.b)) else {
            continue;
        };
        if !(simulated(&ab) && simulated(&bb_)) {
            continue;
        }
        let Some(model) = &c.model else { continue };
        let ctx = format!("[[connection]] {} - {}", c.a, c.b);
        let (pid, vname) = model
            .split_once(':')
            .ok_or_else(|| format!("{ctx}: model {model:?} names no view (run `bb open`)"))?;
        let pid = pid.split_once('@').map_or(pid, |x| x.0);
        let pkg = a
            .imp
            .packages
            .get(pid)
            .ok_or_else(|| format!("{ctx}: harness package {pid} is not resolved"))?;
        let view = pkg
            .part
            .analog
            .iter()
            .find(|v| v.name == vname)
            .ok_or_else(|| format!("{ctx}: {pid} has no analog view {vname}"))?;
        let start = LADDER
            .iter()
            .position(|r| *r == a.fidelity)
            .expect("checked");
        let rung = LADDER[..=start]
            .iter()
            .rev()
            .find(|r| view.fidelity.iter().any(|f| f == *r))
            .ok_or_else(|| {
                format!(
                    "{ctx}: harness view {pid}:{vname} is not valid at {} or below (it is valid \
                     at {})",
                    a.fidelity,
                    view.fidelity.join(", ")
                )
            })?;
        let inst_of = |e: &str| {
            let (b, r) = e.split_once(':').expect("qualified");
            board.instance(b, r)
        };
        let (ia, ib) = (inst_of(&c.a), inst_of(&c.b));
        let net_at = |inst: Option<&Instance>, pin: &str| -> Option<String> {
            inst?
                .pins
                .iter()
                .find(|x| x.number == pin)
                .and_then(|x| x.net.clone())
                .filter(|n| !board.net(n).expect("net").unconnected)
        };
        let frag = element_fragment(&format!("conn{}_{}_{}", k + 1, c.a, c.b));
        let mut port_node: BTreeMap<String, String> = BTreeMap::new();
        let mut dead: Vec<String> = Vec::new();
        for (pa, pb) in &c.pins {
            let (na, nb) = (net_at(ia, pa), net_at(ib, pb));
            let ins = |n: &Option<String>| n.as_deref().is_some_and(scoped);
            let (pa_port, pb_port) = (format!("A{pa}"), format!("B{pb}"));
            match (ins(&na), ins(&nb)) {
                (true, true) => {
                    port_node.insert(pa_port, node_of_net(board, na.as_deref().expect("in")));
                    port_node.insert(pb_port, node_of_net(board, nb.as_deref().expect("in")));
                }
                (false, false) => {
                    // A dead wire: both ends on node 0 carry no current. A
                    // wire end on no net is its own node.
                    if na.is_none() || nb.is_none() {
                        for (port, n) in [(&pa_port, &na), (&pb_port, &nb)] {
                            port_node.insert(
                                port.clone(),
                                match n {
                                    None => format!("bb_{frag}_{port}"),
                                    Some(_) => "0".into(),
                                },
                            );
                        }
                    } else {
                        port_node.insert(pa_port, "0".into());
                        port_node.insert(pb_port, "0".into());
                    }
                    dead.push(format!(
                        "{}/{}",
                        na.as_deref().unwrap_or("-"),
                        nb.as_deref().unwrap_or("-")
                    ));
                }
                (ina, _) => {
                    let (i, o) = if ina { (&na, &nb) } else { (&nb, &na) };
                    match o {
                        // A mated pin on no net: the wire's far end is its
                        // own node.
                        None => {
                            let (inp, outp) = if ina {
                                (pa_port, pb_port)
                            } else {
                                (pb_port, pa_port)
                            };
                            port_node.insert(inp, node_of_net(board, i.as_deref().expect("in")));
                            port_node.insert(outp.clone(), format!("bb_{frag}_{outp}"));
                        }
                        Some(o) => {
                            return Err(format!(
                                "{ctx}: pins {pa}/{pb}: {} is in the analog scope and its mate {o} \
                                 is not; a harness wire joins them, so list both or neither in \
                                 the bench's [analog] nets (ADR-013 D2)",
                                i.as_deref().expect("in")
                            ))
                        }
                    }
                }
            }
        }
        let mut line = format!("X_{frag}");
        for port in &view.ports {
            let Port::Pin(name) = port else {
                return Err(format!("{ctx}: harness view {vname} has a typed port"));
            };
            let n = port_node.get(name).ok_or_else(|| {
                format!("{ctx}: harness port {name} is not one of the mated pins' ends")
            })?;
            line.push(' ');
            line.push_str(n);
            nodes_used.insert(n.clone());
        }
        line.push(' ');
        line.push_str(&view.subckt);
        for (pk, v) in &view.params {
            let d = parse_number(v).map_err(|e| format!("{ctx}: view param {pk}: {e}"))?;
            line.push_str(&format!(" {pk}={}", spice_number(d)));
        }
        let label = format!("{}:{}", pkg.part.id, view.name);
        let text = std::fs::read_to_string(pkg.dir.join(&view.file))
            .map_err(|e| format!("{ctx}: read view {}: {e}", view.file))?;
        if seen_views.insert(label.clone()) {
            deck_views.push((label.clone(), text));
        }
        lines.push(line);
        if !dead.is_empty() {
            warnings.push(format!(
                "{ctx}: these mated pairs are outside the analog scope at both ends, so their \
                 wires carry nothing: {}",
                dead.join(", ")
            ));
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
            ("ref", Json::str(format!("{}={}", c.a, c.b))),
            ("kind", Json::str("harness")),
            ("package", Json::str(&pkg.part.id)),
            ("version", Json::str(&pkg.part.version)),
            ("view", Json::str(&view.name)),
            ("fidelity", Json::str(a.fidelity)),
            ("rung", Json::str(*rung)),
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
    // ADR-009 D8: a per-instance corner for an instance that is not in the
    // deck, or a global corner no view in the deck declares, does nothing:
    // said, not silent.
    for (k, c) in &inst_corner {
        if !corners_used.contains(k) {
            warnings.push(format!(
                "bench [corners] {}:{} = {c:?}: the instance has no view in the deck, so the \
                 corner applies to nothing",
                k.0, k.1
            ));
        }
    }
    if let Some(c) = a.bench.corner.as_ref().filter(|_| !global_corner_used) {
        warnings.push(format!(
            "bench corner = {c:?}: no view in the deck declares it (every instance runs at its \
             view's own parameters)"
        ));
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

    for (b, r) in opened.keys() {
        if !emitted.contains(&(b.clone(), r.clone())) {
            return Err(format!(
                "bench [[fault]] open on {b}:{r}: the instance is not in the analog deck"
            ));
        }
    }

    // --- The bench's stimulus (bb-bench/1, ADR-010 D2-D4): actions,
    // playback, profiles and faults, as tables of the analog engine or as
    // the bench engine's outputs.
    let node_of = |d: &str| board.net(d).expect("net").spice.clone();
    let mut stim = Stimulus::default();
    // A bound pin of the target MCU, by its instance pin name.
    let binding = |pin: &str| {
        bound
            .get(pin)
            .and_then(|k| pins.iter().find(|x| x.kernel_net() == *k))
    };
    // A bench stimulus into the MCU names a bound digital_in pin; its kernel
    // net, or None for a net name.
    let mcu_in = |name: &str, what: &str| -> Result<Option<String>, String> {
        match probe_target(board, simset, name) {
            Ok(ProbeTarget::Pin(b, r, pin)) => {
                if b != tb || r != tref {
                    return Err(format!(
                        "bench {what} {name}: {b}:{r} is not the target MCU {tb}:{tref}"
                    ));
                }
                match binding(&pin) {
                    Some(x) if matches!(x.kind, PinKind::In { .. }) => Ok(Some(x.kernel_net())),
                    Some(_) => Err(format!(
                        "bench {what} {name}: the pin is not bound as digital_in (the bench drives \
                         an MCU input)"
                    )),
                    None => Err(format!(
                        "bench {what} {name}: pin {b}:{r}.{pin} has no [[bind]] (bind it as \
                         digital_in)"
                    )),
                }
            }
            Ok(ProbeTarget::Net(_)) => Ok(None),
            Err(e) => Err(format!("bench {what} {name}: {e}")),
        }
    };
    let mcu_out = |name: &str, what: &str| -> Result<String, String> {
        match probe_target(board, simset, name) {
            Ok(ProbeTarget::Pin(b, r, pin)) if b == tb && r == tref => match binding(&pin) {
                Some(x) if matches!(x.kind, PinKind::Out { .. } | PinKind::OutUnconnected) => {
                    Ok(x.kernel_net())
                }
                _ => Err(format!(
                    "bench {what} {name}: a trigger edge is an MCU pin bound as digital_out"
                )),
            },
            Ok(_) => Err(format!(
                "bench {what} {name}: a trigger edge is a pin of the target MCU {tb}:{tref} \
                 (<instance>.<pin>), bound as digital_out"
            )),
            Err(e) => Err(format!("bench {what} {name}: {e}")),
        }
    };
    let volts = |uv: i128| bbpkg::value::to_f64(uv, Dim::Voltage);
    let mut by_target: BTreeMap<Target, Vec<Planned>> = BTreeMap::new();
    let mut target_initial: BTreeMap<Target, bbbench::Value> = BTreeMap::new();
    let mut bounce_json: Vec<Json> = Vec::new();
    for (k, act) in a.bench.actions.iter().enumerate() {
        let at = &act.path;
        let (target, program) = match &act.kind {
            crate::project::ActionKind::Press(name) => {
                let (b, r) = insts
                    .resolve(name)
                    .map_err(|e| format!("bench {at}.press: {e}"))?;
                let Some(sw) = switches.get(&(b.clone(), r.clone())) else {
                    let kinds: Vec<String> = board
                        .instance(&b, &r)
                        .and_then(|i| i.package.as_ref())
                        .and_then(|p| a.imp.packages.get(&p.id))
                        .map(|p| {
                            p.part
                                .analog
                                .iter()
                                .map(|v| format!("{} ({})", v.name, v.kind))
                                .collect()
                        })
                        .unwrap_or_default();
                    return Err(format!(
                        "bench {at}.press {name}: {b}:{r} has no switch view in the analog deck \
                         (its views: {}; ADR-010 D2)",
                        if kinds.is_empty() {
                            "none".to_string()
                        } else {
                            kinds.join(", ")
                        }
                    ));
                };
                let t = Target::Switch(sw.ctl_source.clone());
                target_initial.insert(t.clone(), bbbench::Value::Volts(0.0));
                let on = bbbench::Value::Volts(1.0);
                let off = bbbench::Value::Volts(0.0);
                let mut prog = vec![(Time::ZERO, on)];
                let bounce = |phase: &str,
                              start: Time,
                              first: bbbench::Value,
                              prog: &mut Vec<(Time, bbbench::Value)>,
                              json: &mut Vec<Json>|
                 -> Result<(), String> {
                    let Some((count, max)) = act.bounce else {
                        return Ok(());
                    };
                    let max = match (max, sw.bounce_max) {
                        (Some(m), Some(v)) if m > v => {
                            return Err(format!(
                                "bench {at}.bounce.max: longer than the switch view's bounce_max"
                            ))
                        }
                        (Some(m), _) => m,
                        (None, Some(v)) => v,
                        (None, None) => {
                            return Err(format!(
                                "bench {at}.bounce: give max (the switch view declares no \
                                 bounce_max)"
                            ))
                        }
                    };
                    if let Some(f) = act.for_fs {
                        if max >= f {
                            return Err(format!("bench {at}.bounce.max: must be shorter than for"));
                        }
                    }
                    let key = format!("{name}/{k}/{phase}");
                    let offs = bbbench::bounce_offsets(a.bench.seed, &key, count, fs_time(max));
                    let mut v = first;
                    for o in &offs {
                        v = if v == on { off } else { on };
                        prog.push((start.saturating_add(*o), v));
                    }
                    json.push(Json::obj([
                        ("action", Json::str(at)),
                        ("phase", Json::str(phase)),
                        (
                            "offsets_fs",
                            Json::Arr(offs.iter().map(|o| Json::Int(o.as_fs() as i128)).collect()),
                        ),
                    ]));
                    Ok(())
                };
                bounce("press", Time::ZERO, on, &mut prog, &mut bounce_json)?;
                if let Some(f) = act.for_fs {
                    let f = fs_time(f);
                    prog.push((f, off));
                    bounce("release", f, off, &mut prog, &mut bounce_json)?;
                }
                (t, prog)
            }
            crate::project::ActionKind::Set { target, to } => {
                if let Some(k) = mcu_in(target, &format!("{at}.set"))? {
                    let lv = match to.as_str() {
                        "0" | "low" => Level::Low,
                        "1" | "high" => Level::High,
                        x => {
                            return Err(format!(
                                "bench {at}.to {x:?}: a level on an MCU pin is 0 or 1"
                            ))
                        }
                    };
                    let t = Target::Pin(k);
                    target_initial
                        .entry(t.clone())
                        .or_insert(bbbench::Value::Level(Level::Low));
                    let base = target_initial[&t];
                    let mut prog = vec![(Time::ZERO, bbbench::Value::Level(lv))];
                    if let Some(f) = act.for_fs {
                        prog.push((fs_time(f), base));
                    }
                    (t, prog)
                } else {
                    let d = net_by(target, &format!("{at}.set"))?;
                    let Some(&uv) = supply_on.get(&d) else {
                        return Err(format!(
                            "bench {at}.set {target}: {d} has no [[supply]] (set drives a bench \
                             source)"
                        ));
                    };
                    let to_uv = bbpkg::value::parse_quantity(to, Dim::Voltage)
                        .map_err(|e| format!("bench {at}.to: {e}"))?;
                    let t = Target::Supply(d);
                    target_initial.insert(t.clone(), bbbench::Value::Volts(volts(uv)));
                    let mut prog = vec![(Time::ZERO, bbbench::Value::Volts(volts(to_uv)))];
                    if let Some(f) = act.for_fs {
                        prog.push((fs_time(f), bbbench::Value::Volts(volts(uv))));
                    }
                    (t, prog)
                }
            }
        };
        let mut sorted = program;
        sorted.sort_by_key(|(t, _)| *t);
        let trigger = match &act.after {
            Some(tr) => Some((
                mcu_out(&tr.edge, &format!("{at}.after.edge"))?,
                if tr.level { Level::High } else { Level::Low },
                fs_time(tr.delay),
            )),
            None => None,
        };
        by_target.entry(target).or_default().push(Planned {
            path: at.clone(),
            at: act.at.map(fs_time),
            trigger,
            program: sorted,
        });
    }
    // Digital playback into MCU pins, and analog playback (tables).
    let mut csv_json: Vec<Json> = Vec::new();
    let mut seen_files: BTreeSet<String> = BTreeSet::new();
    let csv = |file: &str, col: bbbench::csv::Column| -> Result<bbbench::csv::Table, String> {
        let text = a
            .bench
            .data
            .get(file)
            .ok_or_else(|| format!("bench: {file}: not read"))?;
        bbbench::csv::parse(text, col).map_err(|e| format!("bench {file}: {e}"))
    };
    let mut note_file = |file: &str, json: &mut Vec<Json>| {
        if seen_files.insert(file.to_string()) {
            json.push(Json::obj([
                ("file", Json::str(file)),
                ("sha256", Json::str(sha(a.bench.data[file].as_bytes()))),
            ]));
        }
    };
    let mut playback_lines = Vec::new();
    let mut digital_playback: BTreeMap<String, bbbench::OutputSpec> = BTreeMap::new();
    let mut play_nets: BTreeSet<String> = BTreeSet::new();
    for pb in &a.bench.playback {
        note_file(&pb.file, &mut csv_json);
        if let Some(k) = mcu_in(&pb.net, "[[playback]]")? {
            if pb.rs_uohm != 0 || pb.interp != crate::project::Interp::Linear {
                return Err(format!(
                    "bench [[playback]] {}: a digital stream into an MCU pin takes no rs or \
                     interp",
                    pb.net
                ));
            }
            let t = csv(&pb.file, bbbench::csv::Column::Level)?;
            let lv = t.levels();
            let (initial, rest) = match lv.first() {
                Some((t0, l)) if t0.is_zero() => (*l, &lv[1..]),
                _ => (Level::Low, &lv[..]),
            };
            if digital_playback.contains_key(&k) || by_target.contains_key(&Target::Pin(k.clone()))
            {
                return Err(format!(
                    "bench [[playback]] {}: the pin has another bench stimulus (one per pin)",
                    pb.net
                ));
            }
            digital_playback.insert(
                k.clone(),
                bbbench::OutputSpec {
                    pin: k,
                    initial: bbbench::Value::Level(initial),
                    schedule: rest
                        .iter()
                        .map(|(t, l)| (*t, bbbench::Value::Level(*l)))
                        .collect(),
                },
            );
            continue;
        }
        let d = net_by(&pb.net, "[[playback]]")?;
        let n = node_of(&d);
        let g = ground_of(&d);
        if n == g {
            return Err(format!("bench [[playback]] on the ground net {d}"));
        }
        if !scoped(&d) {
            return Err(format!(
                "bench [[playback]] on {d}: the net is outside the analog scope"
            ));
        }
        let on_pwl = a
            .bench
            .pwl
            .iter()
            .any(|p| net_by(&p.net, "[[pwl]]").ok().as_deref() == Some(d.as_str()));
        if supply_on.contains_key(&d) || on_pwl || !play_nets.insert(d.clone()) {
            return Err(format!(
                "bench [[playback]] on {d}: another bench source drives it (one per net)"
            ));
        }
        if let Some(drv) = power_out_nets.get(&d) {
            return Err(format!(
                "bench [[playback]] on {d}: {drv} also drives it (a power_out pin): two drivers"
            ));
        }
        let t = csv(&pb.file, bbbench::csv::Column::Volts)?;
        let f = element_fragment(&n);
        let src = format!("V_play_{f}");
        if pb.rs_uohm > 0 {
            playback_lines.push(format!("{src} bb_play_{f} {g} dc 0 external"));
            playback_lines.push(format!(
                "R_play_{f} bb_play_{f} {n} {}",
                spice_number(to_decimal(pb.rs_uohm, Dim::Resistance))
            ));
        } else {
            playback_lines.push(format!("{src} {n} {g} dc 0 external"));
        }
        stim.tables.push(bb_analog_ngspice::TableSource {
            source: src,
            points: t.points,
            interp: match pb.interp {
                crate::project::Interp::Linear => bb_analog_ngspice::Interp::Linear,
                crate::project::Interp::Hold => bb_analog_ngspice::Interp::Hold,
            },
        });
        nodes_used.insert(n);
    }
    // The targets: a table when every change is known before the run, else
    // the bench engine's output (any triggered action).
    let mut set_sources: BTreeMap<String, String> = BTreeMap::new();
    let mut engine_outputs: Vec<(Target, bbbench::OutputSpec)> = Vec::new();
    for (target, planned) in &by_target {
        let initial = target_initial[target];
        // Timed actions in start order; one ends before the next starts.
        let mut timed: Vec<&Planned> = planned.iter().filter(|p| p.at.is_some()).collect();
        timed.sort_by_key(|p| p.at);
        for w in timed.windows(2) {
            let end = w[0].at.expect("timed") + w[0].program.last().expect("non-empty").0;
            if w[1].at.expect("timed") <= end {
                return Err(format!(
                    "bench {} starts at or before {} ends (actions on one target do not overlap)",
                    w[1].path, w[0].path
                ));
            }
        }
        let schedule: Vec<(Time, bbbench::Value)> = timed
            .iter()
            .flat_map(|p| {
                let s = p.at.expect("timed");
                p.program.iter().map(move |(o, v)| (s + *o, *v))
            })
            .collect();
        let triggered: Vec<&Planned> = planned.iter().filter(|p| p.trigger.is_some()).collect();
        let pin = match target {
            Target::Switch(s) => s.clone(),
            Target::Supply(d) => {
                let s = format!("V_bb_set_{}", element_fragment(&node_of(d)));
                set_sources.insert(d.clone(), s.clone());
                s
            }
            Target::Pin(k) => k.clone(),
        };
        let analog = !matches!(target, Target::Pin(_));
        if analog && triggered.is_empty() {
            let bbbench::Value::Volts(v0) = initial else {
                unreachable!("an analog target")
            };
            let changes: Vec<(Time, f64)> = schedule
                .iter()
                .map(|(t, v)| match v {
                    bbbench::Value::Volts(x) => (*t, *x),
                    bbbench::Value::Level(_) => unreachable!("an analog target"),
                })
                .collect();
            stim.tables.push(bb_analog_ngspice::TableSource {
                source: pin,
                points: table_points(v0, &changes),
                interp: bb_analog_ngspice::Interp::Hold,
            });
            continue;
        }
        // The bench engine's output: changes at 0 set the initial value.
        let mut init = initial;
        let mut rest = Vec::new();
        for (t, v) in schedule {
            if t.is_zero() {
                init = v;
            } else {
                rest.push((t, v));
            }
        }
        if analog {
            stim.analog.insert(pin.clone());
        }
        let idx = engine_outputs.len();
        engine_outputs.push((
            target.clone(),
            bbbench::OutputSpec {
                pin,
                initial: init,
                schedule: rest,
            },
        ));
        for p in triggered {
            let (input, level, delay) = p.trigger.clone().expect("triggered");
            stim.triggers.push(bbbench::TriggerSpec {
                input,
                level,
                delay,
                output: idx,
                program: p.program.clone(),
            });
        }
    }
    stim.outputs = engine_outputs.into_iter().map(|(_, o)| o).collect();
    stim.outputs.extend(digital_playback.into_values());
    // Faults (ADR-010 D2): a switched conductance per fault, its control a
    // table.
    let mut fault_lines = Vec::new();
    if !fault_nodes.is_empty() && seen_views.insert("bb:switch".into()) {
        deck_views.push(("bb:switch".into(), bbpkg::package::switch_definition()));
    }
    for (k, (f, (na, nb))) in a.bench.faults.iter().zip(&fault_nodes).enumerate() {
        let open = f.kind == crate::project::FaultKind::Open;
        let (before, during) = if open { (1.0, 0.0) } else { (0.0, 1.0) };
        let mut changes = vec![(fs_time(f.at), during)];
        if let Some(d) = f.for_fs {
            changes.push((fs_time(f.at + d), before));
        }
        let ctl = format!("bb_fault{k}_ctl");
        fault_lines.push(format!(
            "X_bb_fault{k} {na} {nb} {ctl} {} ron={} roff={}",
            bbpkg::package::SWITCH_SUBCKT,
            spice_number(to_decimal(
                bbpkg::value::parse_quantity(bbpkg::package::SWITCH_RON, Dim::Resistance)
                    .expect("constant"),
                Dim::Resistance
            )),
            spice_number(to_decimal(
                bbpkg::value::parse_quantity(bbpkg::package::SWITCH_ROFF, Dim::Resistance)
                    .expect("constant"),
                Dim::Resistance
            ))
        ));
        fault_lines.push(format!("V_{ctl} {ctl} 0 dc 0 external"));
        stim.tables.push(bb_analog_ngspice::TableSource {
            source: format!("V_{ctl}"),
            points: table_points(before, &changes),
            interp: bb_analog_ngspice::Interp::Hold,
        });
        nodes_used.insert(na.clone());
        nodes_used.insert(nb.clone());
    }
    // The switches' control sources: driven when an action presses it,
    // else open (0).
    let mut switch_lines = Vec::new();
    for sw in switches.values() {
        let driven = by_target.contains_key(&Target::Switch(sw.ctl_source.clone()));
        switch_lines.push(if driven {
            format!("{} {} 0 dc 0 external", sw.ctl_source, sw.ctl_node)
        } else {
            format!("{} {} 0 DC 0", sw.ctl_source, sw.ctl_node)
        });
    }

    // --- Bench sources and loads.
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
        // The ground of the net's board (ADR-013 D2; node 0 on one board).
        let g = ground_of(&d);
        if n == g {
            return Err(format!("bench [[supply]] on the ground net {d}"));
        }
        let f = element_fragment(&n);
        // A supply a `set` action drives (ADR-010 D2): its voltage is the
        // set source's node (an EXTERNAL table, or the bench engine's).
        let set_src = set_sources.get(&d);
        let v = match set_src {
            Some(src) if s.rs_uohm > 0 || s.track.is_some() => {
                let node = src.trim_start_matches("V_").to_string();
                bench_lines.push(format!("{src} {node} 0 dc 0 external"));
                format!("V({node})")
            }
            _ => spice_number(to_decimal(s.volts_uv, Dim::Voltage)),
        };
        if let Some(src) = set_src.filter(|_| s.rs_uohm == 0 && s.track.is_none()) {
            // An ideal fixed supply: the set source is the supply itself.
            bench_lines.push(format!("{src} {n} {g} dc 0 external"));
            nodes_used.insert(n);
            continue;
        }
        // The source voltage: fixed, or tracking another net.
        let target = match &s.track {
            None => v.clone(),
            Some((tn, drop)) => {
                let td = net_by(tn, "[[supply]] track")?;
                let tnode = node_of(&td);
                nodes_used.insert(tnode.clone());
                format!(
                    "min({v}, {} - {})",
                    volts_of(&tnode, &ground_of(&td)),
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
                "B_bench_{f} {g} {n} I = ({target} - {}) / {}",
                volts_of(&n, &g),
                spice_number(to_decimal(s.rs_uohm, Dim::Resistance))
            ));
        } else if s.track.is_some() {
            bench_lines.push(format!("B_bench_{f} {n} {g} V = {target}"));
        } else {
            bench_lines.push(format!("V_bench_{f} {n} {g} DC {target}"));
        }
        nodes_used.insert(n);
    }
    // [[battery]] (ADR-010 D2, milestone P1): the Norton form of the OCV
    // behind R0 (a constant current OCV/R0 in parallel with R0), then each
    // RC pair in series (R and C in parallel): no branch-current unknown
    // (ADR-009 D2), and a plain resistor on every node, so ngspice's source
    // stepping never sees a node held only by a B source (ADR-009
    // Amendment 4).
    for (k, (bt, (d, ocv))) in a.bench.batteries.iter().zip(&battery_ocv).enumerate() {
        if !scoped(d) {
            return Err(format!(
                "bench {} on {d}: the net is outside the analog scope",
                bt.path
            ));
        }
        if let Some(drv) = power_out_nets.get(d) {
            return Err(format!(
                "bench {} on {d}: {drv} also drives it (a power_out pin): two drivers; exclude \
                 {drv} in the bench ([analog] exclude, ADR-007 D5)",
                bt.path
            ));
        }
        let n = node_of(d);
        let g = ground_of(d);
        if n == g {
            return Err(format!("bench {} on the ground net {d}", bt.path));
        }
        note_file(&bt.ocv, &mut csv_json);
        let inner = |j: usize| format!("bb_bat{k}_{j}");
        let n0 = if bt.rc.is_empty() {
            n.clone()
        } else {
            inner(0)
        };
        let r0 = spice_number(to_decimal(bt.r0_uohm, Dim::Resistance));
        bench_lines.push(format!(
            "I_bench_bat{k} {g} {n0} DC {{{} / {r0}}}",
            spice_number(to_decimal(*ocv, Dim::Voltage)),
        ));
        bench_lines.push(format!("R_bench_bat{k}_0 {n0} {g} {r0}"));
        for (j, (r, c)) in bt.rc.iter().enumerate() {
            let from = inner(j);
            let to = if j + 1 == bt.rc.len() {
                n.clone()
            } else {
                inner(j + 1)
            };
            bench_lines.push(format!(
                "R_bench_bat{k}_{} {from} {to} {}",
                j + 1,
                spice_number(to_decimal(*r, Dim::Resistance))
            ));
            bench_lines.push(format!(
                "C_bench_bat{k}_{} {from} {to} {}",
                j + 1,
                spice_number(to_decimal(*c, Dim::Capacitance))
            ));
        }
        nodes_used.insert(n);
    }
    for (k, l) in a.bench.loads.iter().enumerate() {
        let d = net_by(&l.net, "[[load]]")?;
        let n = node_of(&d);
        let g = ground_of(&d);
        let name = format!("I_bench_load{k}_{}", element_fragment(&n));
        match &l.profile {
            None => bench_lines.push(format!(
                "{name} {n} {g} DC {}",
                spice_number(to_decimal(l.amps_pa, Dim::Current))
            )),
            // A current profile (ADR-010 D2): an EXTERNAL current source
            // served from its table.
            Some((file, interp)) => {
                if !scoped(&d) {
                    return Err(format!(
                        "bench [[load]] profile on {d}: the net is outside the analog scope"
                    ));
                }
                note_file(file, &mut csv_json);
                let t = csv(file, bbbench::csv::Column::Amps)?;
                bench_lines.push(format!("{name} {n} {g} dc 0 external"));
                stim.tables.push(bb_analog_ngspice::TableSource {
                    source: name,
                    points: t.points,
                    interp: match interp {
                        crate::project::Interp::Linear => bb_analog_ngspice::Interp::Linear,
                        crate::project::Interp::Hold => bb_analog_ngspice::Interp::Hold,
                    },
                });
            }
        }
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
        bench_lines.push(format!(
            "V_pwl_{f} {src} {} PWL({})",
            ground_of(&d),
            pts.join(" ")
        ));
        if pw.rs_uohm > 0 {
            bench_lines.push(format!(
                "R_pwl_{f} {src} {n} {}",
                spice_number(to_decimal(pw.rs_uohm, Dim::Resistance))
            ));
        }
        nodes_used.insert(n);
    }
    // --- Off-board nets at the simulated board's connectors (ADR-007 D5,
    // Amendment 5): the other end of a [[connection]] is not simulated, so
    // each net it would join is a bench port, driven by a [[supply]] or a
    // [[pwl]], or left open with this warning. The simulated ground is the
    // deck's node 0 and needs no port.
    bench_lines.extend(playback_lines);
    bench_lines.extend(switch_lines);
    bench_lines.extend(fault_lines);
    let mut bench_driven: BTreeSet<String> = supply_on.keys().cloned().collect();
    for pw in &a.bench.pwl {
        bench_driven.insert(net_by(&pw.net, "[[pwl]]")?);
    }
    bench_driven.extend(play_nets.iter().cloned());
    for c in &board.connections {
        let on_sim = |end: &str| end.split_once(':').is_some_and(|x| simulated(x.0));
        let a_side = on_sim(&c.a);
        if a_side == on_sim(&c.b) {
            // Neither end simulated, or both (a system's harness, above).
            continue;
        }
        let (mine, other) = if a_side { (&c.a, &c.b) } else { (&c.b, &c.a) };
        let (mb, mr) = mine.split_once(':').expect("qualified");
        let Some(inst) = board.instance(mb, mr) else {
            continue;
        };
        let mut open: BTreeSet<String> = BTreeSet::new();
        for (pa, pb) in &c.pins {
            let pin = if a_side { pa } else { pb };
            let Some(n) = inst
                .pins
                .iter()
                .find(|x| &x.number == pin)
                .and_then(|x| x.net.as_deref())
                .and_then(|d| board.net(d))
            else {
                continue;
            };
            if n.unconnected || n.spice == ground_of(&n.name) || bench_driven.contains(&n.name) {
                continue;
            }
            open.insert(n.name.clone());
        }
        if !open.is_empty() {
            let who = if system {
                format!("the run simulates {} only, ADR-013 D2", simset.join(", "))
            } else {
                format!("Phase 1 simulates {sim} only, ADR-007 D5")
            };
            warnings.push(format!(
                "[[connection]] {mine} - {other}: {other} is not simulated ({who}), so these nets \
                 at {mine} are left open: {} (a bench [[supply]] or [[pwl]] on one makes it a \
                 bench port)",
                open.into_iter().collect::<Vec<_>>().join(", ")
            ));
        }
    }
    for pb in pins.iter().chain(dev_pins.iter()) {
        match &pb.kind {
            PinKind::Out { net, .. } | PinKind::In { net, .. } if digital_only.contains(net) => {}
            PinKind::Out { net, .. } | PinKind::In { net, .. } | PinKind::Adc { net, .. } => {
                nodes_used.insert(net.clone());
            }
            PinKind::OutUnconnected => {}
        }
    }

    // --- The deck.
    let mut deck = if system {
        format!(
            "* Breadboard elaborated deck (ADR-007 D7, ADR-013 D2): boards {} (ground {sim}), \
             fidelity {}, option set {}\n",
            simset.join(", "),
            a.fidelity,
            a.bench.options
        )
    } else {
        format!(
            "* Breadboard elaborated deck (ADR-007 D7): board {sim}, fidelity {}, option set {}\n",
            a.fidelity, a.bench.options
        )
    };
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
        let spec = match probe_target(board, simset, n).map_err(|e| format!("probe {n}: {e}"))? {
            ProbeTarget::Pin(b, r, pin) => {
                let k = (b == tb && r == tref)
                    .then(|| bound.get(&pin).cloned())
                    .flatten()
                    .or_else(|| {
                        (b == sim)
                            .then(|| dev_bound.get(&(r.clone(), pin.clone())).cloned())
                            .flatten()
                    })
                    .ok_or_else(|| {
                        format!(
                            "probe {n}: pin {b}:{r}.{pin} has no [[bind]] on the target MCU or \
                             [[device.bind]]"
                        )
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
                if digital_only_names.contains(&d) {
                    let drv = pins
                        .iter()
                        .chain(dev_pins.iter())
                        .find(|x| matches!(&x.kind, PinKind::Out { net, .. } if *net == node))
                        .map(|x| x.kernel_net())
                        .unwrap_or_default();
                    return Err(format!(
                        "probe {n}: {d} is outside the bench's [analog] nets, a digital-only net \
                         (ADR-014 D4); probe its driver's pin {drv}"
                    ));
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

    // --- Assertions (ADR-010 D5, D6): their nets as the kernel names them.
    // A digital net is a bound MCU or device pin (its kernel net, or
    // `<instance>.<pin>`); an analog net is a net of the deck.
    let kernel_digital: BTreeSet<String> =
        bound.values().chain(dev_bound.values()).cloned().collect();
    let all_asserts: Vec<bbcore::assert::Spec> = a
        .bench
        .asserts
        .iter()
        .chain(a.extra_asserts.iter())
        .cloned()
        .collect();
    let (asserts, assert_nodes) = crate::asserts::resolve(all_asserts, |n, u| {
        use crate::asserts::Use;
        if u == Use::Digital && kernel_digital.contains(n) {
            return Ok((n.to_string(), None));
        }
        match (u, probe_target(board, simset, n)?) {
            (Use::Digital, ProbeTarget::Pin(b, r, pin)) => (b == tb && r == tref)
                .then(|| bound.get(&pin).cloned())
                .flatten()
                .or_else(|| {
                    (b == sim)
                        .then(|| dev_bound.get(&(r.clone(), pin.clone())).cloned())
                        .flatten()
                })
                .map(|k| (k, None))
                .ok_or_else(|| {
                    format!(
                        "pin {b}:{r}.{pin} has no [[bind]] on the target MCU or [[device.bind]]"
                    )
                }),
            (Use::Digital, ProbeTarget::Net(d)) => Err(format!(
                "{d} is an analog net; an edge, a level or a count reads a bound MCU or device pin"
            )),
            (Use::Analog, ProbeTarget::Pin(b, r, pin)) => Err(format!(
                "{b}:{r}.{pin} is a pin (a digital level); an analog check reads a net"
            )),
            (Use::Analog, ProbeTarget::Net(d)) => {
                let node = node_of(&d);
                if node == "0" {
                    return Err(format!("{d} is the ground (node 0)"));
                }
                if digital_only_names.contains(&d) || !nodes_used.contains(&node) {
                    return Err(format!(
                        "net {d} has nothing in the analog deck (outside the bench's [analog] \
                         nets, or only unmodelled parts on it)"
                    ));
                }
                Ok((d, Some(node)))
            }
        }
    })?;
    // D6: a net an ideal bench source forces is a tautology to check.
    let mut forced: Vec<(String, String)> = Vec::new();
    for s in a.bench.supplies.iter().filter(|s| s.rs_uohm == 0) {
        forced.push((
            net_by(&s.net, "[[supply]]")?,
            format!("[[supply]] on {} (rs = 0)", s.net),
        ));
    }
    for pw in a.bench.pwl.iter().filter(|p| p.rs_uohm == 0) {
        forced.push((
            net_by(&pw.net, "[[pwl]]")?,
            format!("[[pwl]] on {} (rs = 0)", pw.net),
        ));
    }
    for pb in a.bench.playback.iter().filter(|p| p.rs_uohm == 0) {
        if let Ok(d) = net_by(&pb.net, "[[playback]]") {
            forced.push((d, format!("[[playback]] on {} (rs = 0)", pb.net)));
        }
    }
    crate::asserts::check_forced(&asserts, &forced)?;

    // --- run.json (static part).
    let not_simulated: Vec<Json> = board
        .instances
        .iter()
        .filter(|i| simulated(&i.board) && i.status != "ok")
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
    let bbm_text = a.imp.board.to_text();
    let import_boards = a
        .imp
        .import_json
        .get("boards")
        .cloned()
        .unwrap_or(Json::Arr(vec![]));
    let mut bench_obj: Vec<(String, Json)> = vec![
        (
            "file".into(),
            a.bench_name.as_ref().map(Json::str).unwrap_or(Json::Null),
        ),
        ("text".into(), Json::str(&a.bench.text)),
        ("sha256".into(), Json::str(sha(a.bench.text.as_bytes()))),
        ("options".into(), Json::str(&a.bench.options)),
        (
            "options_card".into(),
            Json::str(bbpkg::optsets::get(&a.bench.options).expect("checked")),
        ),
    ];
    // bb-bench/1 (ADR-010, "Determinism impact"): every new input is data
    // hashed into run.json; keys appear only when the bench uses them.
    if !csv_json.is_empty() {
        bench_obj.push(("data".into(), Json::Arr(csv_json)));
    }
    if !bounce_json.is_empty() {
        bench_obj.push(("seed".into(), Json::Int(i128::from(a.bench.seed))));
        bench_obj.push(("bounce".into(), Json::Arr(bounce_json)));
    }
    if stim.engine() {
        bench_obj.push((
            "engine".into(),
            Json::obj([
                ("id", Json::str(bbbench::ENGINE_ID)),
                ("content", Json::str(bbbench::identity())),
            ]),
        ));
    }
    let run_json = vec![
        ("schema".to_string(), Json::str("bb-run/0")),
        ("board_bbm".to_string(), Json::str(sha(bbm_text.as_bytes()))),
        ("import".to_string(), import_boards),
        ("bench".to_string(), Json::Obj(bench_obj)),
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
                        // ADR-011 (Determinism impact): the source is an
                        // input recorded: "path" for a local package (its
                        // path would differ between copies of a project),
                        // else the registry source.
                        Json::obj([
                            ("id", Json::str(&l.id)),
                            ("version", Json::str(&l.version)),
                            ("source", Json::str(bbpkg::lock::source_kind(&l.source))),
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
                            // run.json (bb-run/0) keeps its key: the Renode
                            // version, null for a device (the lock's engine-
                            // neutral engine_version, ADR-011 D5).
                            (
                                "renode",
                                l.engine_version
                                    .as_ref()
                                    .filter(|_| l.engine.as_deref() == Some("renode"))
                                    .map(Json::str)
                                    .unwrap_or(Json::Null),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("deck_sha256".to_string(), Json::str(sha(deck.as_bytes()))),
    ];
    let mut run_json = run_json;
    // ADR-013 D1: the devices and the digital-only nets, recorded only when a
    // project has them (so no Phase 1 run.json changes).
    if !devs.is_empty() {
        run_json.push((
            "devices".to_string(),
            Json::Arr(devs.iter().map(|d| d.json.clone()).collect()),
        ));
    }
    if !digital_only_names.is_empty() {
        run_json.push((
            "digital_only".to_string(),
            Json::Arr(digital_only_names.iter().map(Json::str).collect()),
        ));
    }
    // ADR-013 D2: a system run records its simulated boards, as bb.toml
    // lists them (the target's board's ground is node 0); only then, so no
    // one-board run.json changes.
    if system {
        run_json.push((
            "simulated".to_string(),
            Json::Arr(simset.iter().map(Json::str).collect()),
        ));
    }
    let mut parts = vec![part];
    let mut packages = vec![chip.clone()];
    for d in &devs {
        parts.push(d.part.clone());
        if !packages.iter().any(|x| x.part.id == d.package.part.id) {
            packages.push(d.package.clone());
        }
    }
    let mut all_pins = pins;
    all_pins.extend(dev_pins);

    Ok(RunPlan {
        board: board::Board {
            parts,
            pins: all_pins,
            spice: deck,
            packages,
            warnings: Vec::new(),
            audit: Some(audit),
            digital_only,
        },
        target: tref,
        firmware,
        probes,
        run_json,
        warnings,
        flagged,
        stimulus: stim,
        asserts,
        assert_nodes,
    })
}

/// The SPICE nodes of a view's ports for one instance, in port order.
/// `None` when a port's pin is not connected. `opened` maps an instance pin
/// number to the node an open fault moves it to (ADR-010 D2).
fn port_nodes(
    i: &Instance,
    pkg: &Package,
    view: &View,
    board: &Board,
    opened: &BTreeMap<String, String>,
) -> Result<Option<Vec<String>>, String> {
    let mut out = Vec::new();
    for port in &view.ports {
        let pname = match (view.kind.as_str(), port) {
            (_, Port::Pin(p)) => p,
            // A switch's contacts (ADR-010 D2).
            ("switch", Port::OtherPin { pin }) => pin,
            _ => {
                return Err(format!(
                    "view {} has typed ports (a pin_load view); it cannot be a circuit instance",
                    view.name
                ))
            }
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
        let faulted = numbers.iter().filter(|n| opened.contains_key(**n)).count();
        if faulted > 0 && faulted != numbers.len() {
            return Err(format!(
                "port {pname}: an open fault names one of its several pins ({}); fault all of \
                 them or none",
                numbers.join(", ")
            ));
        }
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
            let s = match opened.get(num) {
                Some(f) => f.clone(),
                None => board.net(net).expect("net").spice.clone(),
            };
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

/// The SPICE node of a net (its display name).
fn node_of_net(board: &Board, display: &str) -> String {
    board.net(display).expect("net").spice.clone()
}

/// A node's voltage against a ground node, as a B-source term: `V(n)` on
/// node 0 (every one-board deck), `V(n,g)` against another board's ground.
fn volts_of(node: &str, ground: &str) -> String {
    if ground == "0" {
        format!("V({node})")
    } else {
        format!("V({node},{ground})")
    }
}

/// A system run's direct ties (ADR-013 D2): a `[[connection]]` between two
/// simulated boards without a harness view ties each mated pair's nets,
/// which become one node. Returns the board with the tied nets' SPICE names
/// made one (node 0 if a group holds it, else the bytewise first name among
/// the group's nets on the primary board, else among all of them), or
/// `None` when nothing is tied (every one-board run).
fn tied_board(board: &Board, simulated: &[String], primary: &str) -> Option<Board> {
    let on = |end: &str| {
        end.split_once(':')
            .is_some_and(|(b, _)| simulated.iter().any(|s| s == b))
    };
    // Union-find over net display names.
    let mut parent: BTreeMap<String, String> = BTreeMap::new();
    fn find(p: &mut BTreeMap<String, String>, x: &str) -> String {
        let mut r = x.to_string();
        while let Some(n) = p.get(&r).filter(|n| **n != r) {
            r = n.clone();
        }
        p.insert(x.to_string(), r.clone());
        r
    }
    for c in &board.connections {
        if c.model.is_some() || !on(&c.a) || !on(&c.b) {
            continue;
        }
        let net_at = |end: &str, pin: &str| -> Option<String> {
            let (b, r) = end.split_once(':')?;
            board
                .instance(b, r)?
                .pins
                .iter()
                .find(|x| x.number == pin)
                .and_then(|x| x.net.clone())
                .filter(|n| !board.net(n).is_some_and(|x| x.unconnected))
        };
        for (pa, pb) in &c.pins {
            let (Some(na), Some(nb)) = (net_at(&c.a, pa), net_at(&c.b, pb)) else {
                continue;
            };
            let (ra, rb) = (find(&mut parent, &na), find(&mut parent, &nb));
            if ra != rb {
                let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
                parent.insert(hi, lo);
            }
        }
    }
    if parent.is_empty() {
        return None;
    }
    let names: Vec<String> = parent.keys().cloned().collect();
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for n in names {
        let r = find(&mut parent, &n);
        groups.entry(r).or_default().push(n);
    }
    let mut out = board.clone();
    for members in groups.values() {
        let spices: BTreeSet<String> = members
            .iter()
            .map(|m| board.net(m).expect("net").spice.clone())
            .collect();
        let on_primary: BTreeSet<String> = members
            .iter()
            .map(|m| board.net(m).expect("net"))
            .filter(|n| n.board == primary)
            .map(|n| n.spice.clone())
            .collect();
        let node = if spices.contains("0") {
            "0".to_string()
        } else {
            on_primary
                .iter()
                .next()
                .or_else(|| spices.iter().next())
                .expect("non-empty")
                .clone()
        };
        for n in out.nets.iter_mut().filter(|n| members.contains(&n.name)) {
            n.spice = node.clone();
        }
    }
    Some(out)
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
pub fn describe_probe(board: &Board, simulated: &[String], name: &str) -> Result<String, String> {
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
        // lower rung that has one is chosen (the view is the default at
        // fast and pi since milestone P1, ADR-009 D1).
        let (v, rung) = select_view(&pkg, "pi", None).unwrap().unwrap();
        assert_eq!((v.name.as_str(), rung.as_str()), ("lsw", "pi"));
        let (v, rung) = select_view(&pkg, "si", None).unwrap().unwrap();
        assert_eq!((v.name.as_str(), rung.as_str()), ("lsw", "pi"));
        let mut fast_only = pkg.clone();
        fast_only.part.analog[0].default_for = vec!["fast".into()];
        let (v, rung) = select_view(&fast_only, "pi", None).unwrap().unwrap();
        assert_eq!((v.name.as_str(), rung.as_str()), ("lsw", "fast"));
        assert!(select_view(&pkg, "spice", None).is_err());
        // A pin_load view is never selected as the part's circuit.
        let mut p2 = pkg.clone();
        p2.part.analog[0].kind = "pin_load".into();
        assert!(select_view(&p2, "fast", None).unwrap().is_none());
    }
}
