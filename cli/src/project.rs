//! The project file `bb.toml` (`bb-project/0`) and the bench file
//! (`bb-bench/0`), ADR-007 D5 and `spec/project.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bbpkg::obj::Obj;
use bbpkg::schema::Registry;
use bbpkg::toml::{self, Value};

/// The project file's name.
pub const PROJECT_FILE: &str = "bb.toml";

/// Where a board's connectivity comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// A committed KiCad netlist (`netlist = "..."`).
    Netlist(String),
    /// A KiCad project, exported with `kicad-cli` (`kicad = "....kicad_pro"`).
    Kicad(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardEntry {
    pub id: String,
    pub source: Source,
    pub ground: String,
}

/// `[instances."<ref>"]`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct InstanceOverride {
    /// A package id (`id` or `id@version`), or `none`.
    pub package: Option<String>,
    pub reason: Option<String>,
    /// The analog view to use (ADR-006 D3 selection step 1).
    pub analog: Option<String>,
}

/// One firmware image of a target (ADR-008 D5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub file: String,
    pub kind: String,
    pub hash: String,
    pub offset: Option<String>,
}

/// `[[bind]]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bind {
    pub pin: String,
    pub model: String,
    pub set: Option<String>,
    pub net: Option<String>,
    /// Explicit parameter overrides (value grammar), in key order.
    pub params: Vec<(String, String)>,
}

/// The pin-model parameter keys a binding may set explicitly (the value
/// grammar; `spec/pin-model.md`).
pub const PIN_PARAM_KEYS: &[&str] = &[
    "voh", "vol", "rise", "fall", "zout", "vih", "vil", "cin", "vref", "bits", "load",
    // bb-pins/1 (ADR-009 D5): used at `pi` only (milestone P2).
    "rail", "rhiz",
];

/// `[registries.<name>]` (`bb-project/1`, ADR-011 D4): a private registry,
/// routed by namespace (`crate::registry::resolver`, `spec/registry.md`
/// section 6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistryEntry {
    pub name: String,
    /// `https://...` or `file://...`.
    pub url: String,
    /// The registry's minisign public key.
    pub key: String,
    /// The namespaces routed to it.
    pub namespaces: Vec<String>,
}

/// `[[device.bind]]` (`bb-project/1`, ADR-013 D1): a device pin on a net.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceBind {
    /// The device package's pin name (`HOME_N`).
    pub pin: String,
    /// The net, by the project's name rules (ADR-007 D7).
    pub net: String,
    /// `digital_in` or `digital_out` (`adc_in` is refused, spec/package.md
    /// section 12).
    pub model: String,
    pub set: Option<String>,
    /// Explicit pin-model parameter overrides, in key order.
    pub params: Vec<(String, String)>,
}

/// `[[device]]` (`bb-project/1`, ADR-013 D1, Phase 1 route B).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceEntry {
    /// `<board>:<ref>` or a bare `<ref>`, as written.
    pub reference: String,
    /// A package id (`id` or `id@version`) whose `[behavioral]` engine is
    /// `device`.
    pub package: String,
    /// The device's parameters (value grammar), in key order.
    pub params: Vec<(String, String)>,
    /// No schematic symbol: the instance exists only in `board.bbm`
    /// (`virtual: true`), and `ref` must not collide with a symbol's.
    pub is_virtual: bool,
    pub binds: Vec<DeviceBind>,
}

/// `[[connection]]` (systems, M11; validated from M4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionEntry {
    pub a: String,
    pub b: String,
    pub model: Option<String>,
    pub pins: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    pub dir: PathBuf,
    pub name: Option<String>,
    /// The simulated board's id.
    pub simulate: String,
    pub boards: Vec<BoardEntry>,
    /// `[packages] paths`, relative to the project.
    pub package_paths: Vec<String>,
    /// Extra D3 field names (`[import] fields`).
    pub extra_fields: Vec<String>,
    /// Keys as written (bare or qualified); resolved by name rules later.
    pub instances: Vec<(String, InstanceOverride)>,
    pub targets: Vec<(String, Vec<Image>)>,
    pub binds: Vec<Bind>,
    pub probes: Vec<String>,
    pub connections: Vec<ConnectionEntry>,
    /// `[[device]]` (`bb-project/1`).
    pub devices: Vec<DeviceEntry>,
    /// `[registries.<name>]` (`bb-project/1`, ADR-011 D4).
    pub registries: Vec<RegistryEntry>,
    /// The migration hint when the file is the previous major
    /// (`bb-project/0`, ADR-006 D11).
    pub schema_hint: Option<String>,
}

/// The D3 fields every import reads.
pub const D3_FIELDS: &[&str] = &["MPN", "Manufacturer", "LCSC", "bb:part"];

fn is_id(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-'))
}

impl Project {
    pub fn read(dir: &Path) -> Result<Project, String> {
        let path = dir.join(PROJECT_FILE);
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
        Project::parse(&text, dir).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn parse(text: &str, dir: &Path) -> Result<Project, String> {
        let t = toml::parse(text).map_err(|e| e.to_string())?;
        let mut o = Obj::new("", &t);
        let schema = o.str_req("schema")?;
        let schema_hint = match Registry::current().check("bb-project", &schema)? {
            bbpkg::schema::Version::Current => None,
            bbpkg::schema::Version::Previous { .. } => Some(format!(
                "bb.toml is {schema}, the previous major: bb reads it; change its schema line to \
                 \"bb-project/1\" (the only difference, ADR-013)"
            )),
        };
        let mut name = None;
        let mut simulate = None;
        if let Some(mut p) = o.table_opt("project")? {
            name = p.str_opt("name")?;
            // bb-project/1 (ADR-013 D2): simulate may be a list. One board
            // is the Phase 1 meaning; several (a single-MCU system run) are
            // milestone R2's.
            simulate = match p.get("simulate") {
                None => None,
                Some(Value::String(s)) => Some(s.clone()),
                Some(Value::Array(a)) => {
                    let ids: Vec<String> = a
                        .iter()
                        .map(|v| v.as_str().map(str::to_string))
                        .collect::<Option<_>>()
                        .ok_or("[project] simulate: expected a board id or a list of them")?;
                    match ids.as_slice() {
                        [one] => Some(one.clone()),
                        [] => return Err("[project] simulate = []: name a board".into()),
                        _ => {
                            return Err(format!(
                                "[project] simulate = {ids:?}: a system run of several boards is \
                                 milestone R2 (ADR-013 D2), not implemented in this build"
                            ))
                        }
                    }
                }
                Some(v) => {
                    return Err(format!(
                        "[project] simulate: expected a board id or a list, got {}",
                        v.type_name()
                    ))
                }
            };
            p.done()?;
        }
        // bb-project/1 (ADR-013 D3): [run] cpu and lattice are milestone
        // R3's (runs without a CPU target; not in this build). Refused, not
        // ignored: a project that names them would not run as written.
        if let Some(r) = o.table_opt("run")? {
            let keys = r.keys();
            return Err(format!(
                "[run] {}: runs without a CPU target and the engine-neutral lattice are \
                 milestone R3 (ADR-013 D3), not implemented in this build",
                keys.join(", ")
            ));
        }
        // bb-project/1 (ADR-011 D4): private registries, routed by namespace.
        let mut registries = Vec::new();
        if let Some(mut rt) = o.table_opt("registries")? {
            for rname in rt.keys() {
                let mut r = rt
                    .table_opt(&rname)?
                    .ok_or_else(|| format!("registries.{rname}: expected a table"))?;
                let e = RegistryEntry {
                    name: rname.clone(),
                    url: r.str_req("url")?,
                    key: r.str_req("key")?,
                    namespaces: r.strs("namespaces")?,
                };
                r.done()?;
                if !is_id(&rname) {
                    return Err(format!(
                        "registries.{rname}: a registry name is lower case [a-z0-9_-]"
                    ));
                }
                if !(e.url.starts_with("https://") || e.url.starts_with("file://")) {
                    return Err(format!(
                        "registries.{rname}.url {:?}: https:// or file:// (ADR-011 D4)",
                        e.url
                    ));
                }
                bbpkg::registry::check_url(&e.url)
                    .map_err(|err| format!("registries.{rname}.url: {err}"))?;
                bbpkg::registry::check_key(&e.key)
                    .map_err(|err| format!("registries.{rname}.key: {err}"))?;
                if e.namespaces.is_empty() {
                    return Err(format!(
                        "registries.{rname}.namespaces: a registry is routed by at least one \
                         namespace (ADR-011 D4)"
                    ));
                }
                for ns in &e.namespaces {
                    let ok = !ns.is_empty()
                        && ns.chars().all(|c| {
                            c.is_ascii_lowercase()
                                || c.is_ascii_digit()
                                || matches!(c, '.' | '_' | '-')
                        });
                    if !ok {
                        return Err(format!(
                            "registries.{rname}.namespaces: {ns:?} is not a namespace ([a-z0-9._-])"
                        ));
                    }
                    if registries
                        .iter()
                        .any(|x: &RegistryEntry| x.namespaces.contains(ns))
                    {
                        return Err(format!(
                            "registries: namespace {ns} is routed to two registries"
                        ));
                    }
                }
                registries.push(e);
            }
            rt.done()?;
        }
        let mut boards = Vec::new();
        for mut b in o.tables("board")? {
            let id = b.str_req("id")?;
            if !is_id(&id) {
                return Err(format!(
                    "{}.id {id:?}: lower case letters, digits, _ and - only",
                    b.path
                ));
            }
            let source = match (b.str_opt("netlist")?, b.str_opt("kicad")?) {
                (Some(n), None) => Source::Netlist(n),
                (None, Some(k)) => {
                    if !k.ends_with(".kicad_pro") {
                        return Err(format!("{}.kicad: expected a .kicad_pro", b.path));
                    }
                    Source::Kicad(k)
                }
                _ => {
                    return Err(format!(
                        "{}: give exactly one of netlist = \"<file>.net\" or kicad = \"<file>.kicad_pro\"",
                        b.path
                    ))
                }
            };
            let ground = b.str_opt("ground")?.unwrap_or_else(|| "GND".into());
            if boards.iter().any(|x: &BoardEntry| x.id == id) {
                return Err(format!("board {id} is declared twice"));
            }
            b.done()?;
            boards.push(BoardEntry { id, source, ground });
        }
        if boards.is_empty() {
            return Err("no [[board]]: a project needs at least one".into());
        }
        let simulate = match simulate {
            Some(s) => {
                if !boards.iter().any(|b| b.id == s) {
                    return Err(format!("[project] simulate = {s:?}: no such [[board]]"));
                }
                s
            }
            None if boards.len() == 1 => boards[0].id.clone(),
            None => return Err(
                "[project] simulate: required with more than one [[board]] (Phase 1 simulates one)"
                    .into(),
            ),
        };
        let mut package_paths = Vec::new();
        if let Some(mut p) = o.table_opt("packages")? {
            package_paths = p.strs("paths")?;
            p.done()?;
        }
        let mut extra_fields = Vec::new();
        if let Some(mut p) = o.table_opt("import")? {
            extra_fields = p.strs("fields")?;
            p.done()?;
        }
        let mut instances = Vec::new();
        if let Some(Value::Table(it)) = o.get("instances") {
            for (k, v) in it {
                let tv = v
                    .as_table()
                    .ok_or_else(|| format!("instances.{k:?}: expected a table"))?;
                let mut io = Obj::new(format!("instances.{k:?}"), tv);
                let ov = InstanceOverride {
                    package: io.str_opt("package")?,
                    reason: io.str_opt("reason")?,
                    analog: io.str_opt("analog")?,
                };
                if ov.package.as_deref() == Some("none") && ov.reason.is_none() {
                    return Err(format!(
                        "instances.{k:?}: package = \"none\" needs a reason (ADR-007 D4)"
                    ));
                }
                io.done()?;
                instances.push((k.clone(), ov));
            }
        } else if o.has("instances") {
            return Err("instances: expected a table".into());
        }
        let mut targets = Vec::new();
        if let Some(Value::Table(tt)) = o.get("targets") {
            for (k, v) in tt {
                let tv = v
                    .as_table()
                    .ok_or_else(|| format!("targets.{k:?}: expected a table"))?;
                let mut to = Obj::new(format!("targets.{k:?}"), tv);
                let mut images = Vec::new();
                match to.get("images") {
                    Some(Value::Array(a)) => {
                        for (i, im) in a.iter().enumerate() {
                            let it = im.as_table().ok_or_else(|| {
                                format!("targets.{k:?}.images[{i}]: expected a table")
                            })?;
                            let mut imo = Obj::new(format!("targets.{k:?}.images[{i}]"), it);
                            let img = Image {
                                file: imo.str_req("file")?,
                                kind: imo.str_req("kind")?,
                                hash: imo.str_req("hash")?,
                                offset: imo.str_opt("offset")?,
                            };
                            imo.done()?;
                            images.push(img);
                        }
                    }
                    None => {}
                    Some(_) => return Err(format!("targets.{k:?}.images: expected an array")),
                }
                to.done()?;
                targets.push((k.clone(), images));
            }
        } else if o.has("targets") {
            return Err("targets: expected a table".into());
        }
        let mut binds = Vec::new();
        for b in o.tables("bind")? {
            let path = b.path.clone();
            let mut b = b;
            let pin = b.str_req("pin")?;
            let model = b.str_req("model")?;
            let set = b.str_opt("set")?;
            let net = b.str_opt("net")?;
            let mut params = Vec::new();
            for k in PIN_PARAM_KEYS {
                if let Some(v) = b.str_opt(k)? {
                    params.push((k.to_string(), v));
                }
            }
            // ADR-008 D7: the SAADC's per-sample records, a binding option
            // (board files: `trace_samples=true`); build_pin checks the model.
            if let Some(t) = b.bool_opt("trace_samples")? {
                params.push(("trace_samples".to_string(), t.to_string()));
            }
            b.done().map_err(|e| {
                format!(
                    "{e} (a [[bind]] takes pin, model, set, net, pin-model parameters and \
                     trace_samples)"
                )
            })?;
            if binds.iter().any(|x: &Bind| x.pin == pin) {
                return Err(format!("{path}: pin {pin} is bound twice"));
            }
            binds.push(Bind {
                pin,
                model,
                set,
                net,
                params,
            });
        }
        let mut probes = Vec::new();
        if let Some(mut p) = o.table_opt("probes")? {
            probes = p.strs("nets")?;
            p.done()?;
        }
        let mut connections = Vec::new();
        for mut c in o.tables("connection")? {
            let mut pins = Vec::new();
            match c.get("pins") {
                None => {}
                Some(Value::Array(a)) if a.is_empty() => {
                    return Err(format!(
                        "{}.pins = []: a connection mates at least one pin (leave pins out to \
                         mate pin n to pin n, or through the harness's wires)",
                        c.path
                    ))
                }
                Some(Value::Array(a)) => {
                    for p in a {
                        match p.as_array().map(Vec::as_slice) {
                            Some([Value::String(x), Value::String(y)]) => {
                                pins.push((x.clone(), y.clone()))
                            }
                            _ => return Err(format!("{}.pins: expected [[a, b], ...]", c.path)),
                        }
                    }
                }
                Some(_) => return Err(format!("{}.pins: expected an array", c.path)),
            }
            let e = ConnectionEntry {
                a: c.str_req("a")?,
                b: c.str_req("b")?,
                model: c.str_opt("model")?,
                pins,
            };
            c.done()?;
            connections.push(e);
        }
        let mut devices: Vec<DeviceEntry> = Vec::new();
        if o.has("device") && schema_hint.is_some() {
            return Err(format!(
                "[[device]] is a bb-project/1 key (ADR-013 D1); this file is {schema}"
            ));
        }
        for mut d in o.tables("device")? {
            let at = d.path.clone();
            let reference = d.str_req("ref")?;
            let package = d.str_req("package")?;
            let params = d.str_map("params")?;
            let is_virtual = d.bool_opt("virtual")?.unwrap_or(false);
            let mut binds: Vec<DeviceBind> = Vec::new();
            for mut b in d.tables("bind")? {
                let bat = b.path.clone();
                let pin = b.str_req("pin")?;
                let net = b.str_req("net")?;
                let model = b.str_req("model")?;
                let set = b.str_opt("set")?;
                let mut params = Vec::new();
                for k in PIN_PARAM_KEYS {
                    if let Some(v) = b.str_opt(k)? {
                        params.push((k.to_string(), v));
                    }
                }
                b.done().map_err(|e| {
                    format!("{e} (a [[device.bind]] takes pin, net, model, set and pin-model parameters)")
                })?;
                if !matches!(model.as_str(), "digital_in" | "digital_out") {
                    return Err(format!(
                        "{bat}: model = {model:?}: a device pin is digital_in or digital_out \
                         (spec/package.md section 12)"
                    ));
                }
                if binds.iter().any(|x| x.pin == pin) {
                    return Err(format!("{bat}: pin {pin} is bound twice"));
                }
                binds.push(DeviceBind {
                    pin,
                    net,
                    model,
                    set,
                    params,
                });
            }
            d.done().map_err(|e| {
                format!(
                    "{e} (a [[device]] takes ref, package, params, virtual and [[device.bind]])"
                )
            })?;
            if binds.is_empty() {
                return Err(format!("{at}: a device needs at least one [[device.bind]]"));
            }
            if devices.iter().any(|x| x.reference == reference) {
                return Err(format!("{at}: device {reference} is declared twice"));
            }
            devices.push(DeviceEntry {
                reference,
                package,
                params,
                is_virtual,
                binds,
            });
        }
        o.done()?;
        if schema_hint.is_some() && !registries.is_empty() {
            return Err(format!(
                "[registries] is a bb-project/1 key (ADR-011 D4); this file is {schema}"
            ));
        }
        Ok(Project {
            dir: dir.to_path_buf(),
            name,
            simulate,
            registries,
            boards,
            package_paths,
            extra_fields,
            instances,
            targets,
            binds,
            probes,
            connections,
            devices,
            schema_hint,
        })
    }

    /// The package search path (ADR-006 D1): `[packages] paths`, then the
    /// first-party directory.
    pub fn search_path(&self) -> bbpkg::SearchPath {
        let mut dirs: Vec<PathBuf> = self
            .package_paths
            .iter()
            .map(|p| self.dir.join(p))
            .collect();
        dirs.push(bbpkg::first_party_dir());
        bbpkg::SearchPath { dirs }
    }

    pub fn multi(&self) -> bool {
        self.boards.len() > 1
    }
}

/// `[targets."<ref>"]` in `bb.toml`, written by `bb flash` with a
/// format-preserving edit: the table's lines are replaced (or the table is
/// appended), every other line, comment and blank line is kept.
pub fn set_target(text: &str, key: &str, images: &[Image]) -> String {
    let header = format!("[targets.{}]", toml::quote(key));
    let mut body = String::from("images = [");
    for (i, im) in images.iter().enumerate() {
        if i > 0 {
            body.push_str(", ");
        }
        body.push_str(&format!(
            "{{ file = {}, kind = {}, hash = {}",
            toml::quote(&im.file),
            toml::quote(&im.kind),
            toml::quote(&im.hash)
        ));
        if let Some(o) = &im.offset {
            body.push_str(&format!(", offset = {}", toml::quote(o)));
        }
        body.push_str(" }");
    }
    body.push_str("]\n");
    set_table(text, &header, &body)
}

/// Replace the body of the table `header` (`[probes]`) with `body`, or
/// append the table; every other line, comment and blank line is kept.
pub fn set_table(text: &str, header: &str, body: &str) -> String {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let is_header = |l: &str| {
        let t = l.trim_start();
        t.starts_with('[')
    };
    let norm = |l: &str| -> String {
        l.split('#')
            .next()
            .unwrap_or("")
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect()
    };
    let want = norm(header);
    if let Some(start) = lines.iter().position(|l| norm(l) == want) {
        let end = lines[start + 1..]
            .iter()
            .position(|l| is_header(l))
            .map(|p| start + 1 + p)
            .unwrap_or(lines.len());
        // Keep trailing blank and comment lines of the old table (they
        // usually belong to the next table).
        let mut keep_from = end;
        while keep_from > start + 1 {
            let t = lines[keep_from - 1].trim();
            if t.is_empty() || t.starts_with('#') {
                keep_from -= 1;
            } else {
                break;
            }
        }
        let mut out: String = lines[..=start].concat();
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(body);
        out.push_str(&lines[keep_from..].concat());
        out
    } else {
        let mut out = text.to_string();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.is_empty() && !out.ends_with("\n\n") {
            out.push('\n');
        }
        out.push_str(header);
        out.push('\n');
        out.push_str(body);
        out
    }
}

/// A bench supply (`[[supply]]`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Supply {
    pub net: String,
    /// µV.
    pub volts_uv: i128,
    /// µΩ of series resistance (0: an ideal source).
    pub rs_uohm: i128,
    /// A tracking supply: `min(volts, V(net) - drop)`.
    pub track: Option<(String, i128)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Load {
    pub net: String,
    /// pA (a DC sink); 0 with a profile.
    pub amps_pa: i128,
    /// `bb-bench/1`: a current profile (`t,amps`, ADR-010 D2), its file
    /// (relative to the bench) and interpolation.
    pub profile: Option<(String, Interp)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pwl {
    pub net: String,
    /// `(fs, µV)`.
    pub points: Vec<(i128, i128)>,
    pub rs_uohm: i128,
}

/// How a playback or a profile interpolates (ADR-010 D2).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Interp {
    Linear,
    Hold,
}

/// `[[playback]]` (ADR-010 D2, D3): an analog net (an `EXTERNAL` source,
/// ideal or behind `rs`), or a bound MCU `digital_in` pin (a digital stream,
/// applied at `ceil_L`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Playback {
    /// A net, or `<instance>.<pin>`.
    pub net: String,
    pub file: String,
    pub rs_uohm: i128,
    pub interp: Interp,
}

/// What an `[[action]]` does (ADR-010 D2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActionKind {
    /// Close a `switch` instance (while pressed).
    Press(String),
    /// Drive a bench source (a `[[supply]]`'s net, or a bound MCU
    /// `digital_in` pin) to a new value (`to`, as written).
    Set { target: String, to: String },
}

/// `after = { edge, level, delay }`: a relative trigger.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trigger {
    /// An MCU output pin (`<instance>.<pin>`).
    pub edge: String,
    pub level: bool,
    /// fs.
    pub delay: i128,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action {
    /// `[[action]][k]`, for messages.
    pub path: String,
    pub kind: ActionKind,
    /// fs: exactly one of `at` and `after`.
    pub at: Option<i128>,
    pub after: Option<Trigger>,
    /// fs: the press (or the set) ends `for` after it starts.
    pub for_fs: Option<i128>,
    /// `bounce = { count, max }` (a press): `max` in fs, if given.
    pub bounce: Option<(u32, Option<i128>)>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FaultKind {
    /// A pin of an instance leaves its net.
    Open,
    /// The net shorts to the board's ground.
    Short,
}

/// `[[fault]]` (ADR-010 D2): a B conductance switched by an `EXTERNAL`
/// source at `at` (and back after `for`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fault {
    pub path: String,
    pub kind: FaultKind,
    pub net: String,
    /// `open`: the instance pin (`main:C8.1`).
    pub pin: Option<String>,
    /// fs.
    pub at: i128,
    pub for_fs: Option<i128>,
}

/// A bench (`bb-bench/0` or `bb-bench/1`).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Bench {
    /// The schema major the file declares.
    pub major: u32,
    /// m°C.
    pub temp_mc: Option<i128>,
    pub supplies: Vec<Supply>,
    pub loads: Vec<Load>,
    /// Net -> µV.
    pub nominal: BTreeMap<String, i128>,
    pub pwl: Vec<Pwl>,
    /// `None`: every net is in scope (with a warning).
    pub analog_nets: Option<Vec<String>>,
    pub exclude: Vec<String>,
    pub options: String,
    /// The file text (copied into run.json).
    pub text: String,
    /// `bb-bench/1` (ADR-010 D2): the seed of the declared random elements
    /// (contact bounce); 1 when not given.
    pub seed: u64,
    pub actions: Vec<Action>,
    pub playback: Vec<Playback>,
    pub faults: Vec<Fault>,
    /// The CSV files the bench names, by file name as written: their text
    /// (read by [`Bench::read`], relative to the bench file).
    pub data: BTreeMap<String, String>,
    /// Parse warnings (a long `[[pwl]]`).
    pub warnings: Vec<String>,
    /// `bb-bench/1` `[[assert]]` (ADR-010 D5): the nets as written.
    pub asserts: Vec<bbcore::assert::Spec>,
}

fn q(text: &str, dim: bbpkg::value::Dim, at: &str) -> Result<i128, String> {
    bbpkg::value::parse_quantity(text, dim).map_err(|e| format!("{at}: {e}"))
}

/// The `bb-bench/1` keys of ADR-010 D2 that another milestone implements:
/// refused, never ignored (a bench that names one would not run as written).
const LATER_TOP: &[(&str, &str)] = &[
    ("fidelity", "the fidelity dial (ADR-009 D1, milestone P1)"),
    ("corner", "corners (ADR-009 D8, milestone P1)"),
    (
        "battery",
        "the battery element (ADR-009, milestones P1 and R2)",
    ),
    (
        "activity",
        "bench-scheduled internal states (ADR-009 D4, milestone P3)",
    ),
    ("env", "environment signals (a later milestone)"),
    ("probes", "per-probe rates (a later milestone)"),
];

impl Bench {
    pub fn read(path: &Path) -> Result<Bench, String> {
        let text =
            std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let mut b = Bench::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        // The CSV files (ADR-010 D3), relative to the bench file; only CSV
        // reaches a run.
        let dir = path.parent().unwrap_or(Path::new("."));
        let files: Vec<String> = b
            .playback
            .iter()
            .map(|p| p.file.clone())
            .chain(
                b.loads
                    .iter()
                    .filter_map(|l| l.profile.as_ref().map(|p| p.0.clone())),
            )
            .collect();
        for f in files {
            if b.data.contains_key(&f) {
                continue;
            }
            let p = dir.join(&f);
            let t = std::fs::read_to_string(&p)
                .map_err(|e| format!("{}: read {}: {e}", path.display(), p.display()))?;
            b.data.insert(f, t);
        }
        Ok(b)
    }

    /// The bench used when `bb run` has no `--bench`: no sources, every
    /// net in scope, the default option set.
    pub fn empty() -> Bench {
        Bench {
            major: 1,
            seed: 1,
            options: bbpkg::optsets::DEFAULT.into(),
            ..Bench::default()
        }
    }

    pub fn parse(text: &str) -> Result<Bench, String> {
        use bbpkg::value::Dim;
        let t = toml::parse(text).map_err(|e| e.to_string())?;
        let mut o = Obj::new("", &t);
        let schema = o.str_req("schema")?;
        let version = Registry::current().check("bb-bench", &schema)?;
        let major = bbpkg::schema::SchemaId::parse(&schema)?.major;
        let mut b = Bench {
            major,
            seed: 1,
            text: text.to_string(),
            options: bbpkg::optsets::DEFAULT.into(),
            ..Bench::default()
        };
        // Reading the previous major prints the migration hint (spec/package.md
        // section 10).
        if let bbpkg::schema::Version::Previous { .. } = version {
            b.warnings.push(format!(
                "{schema} is the previous major: bb reads it, but run `bb pkg migrate --bench \
                 <file>` to rewrite it as {}",
                Registry::current().current_id("bb-bench")
            ));
        }
        if major >= 1 {
            for (k, what) in LATER_TOP {
                if o.has(k) {
                    return Err(format!(
                        "{k}: a bb-bench/1 key for {what} (ADR-010 D2), not implemented in this \
                         build; a bench that names it would not run as written"
                    ));
                }
            }
            if let Some(s) = o.get("seed") {
                b.seed = match s {
                    Value::Integer(n) if *n >= 0 => *n as u64,
                    _ => return Err("seed: expected a non-negative integer".into()),
                };
            }
        }
        if let Some(tc) = o.str_opt("temp_c")? {
            let mc = bbpkg::value::parse_number(&tc)
                .map_err(|e| format!("temp_c: {e}"))?
                .to_int(3)
                .ok_or("temp_c: finer than 1 m°C")?;
            b.temp_mc = Some(mc);
        }
        for mut s in o.tables("supply")? {
            let at = s.path.clone();
            let net = s.str_req("net")?;
            let volts_uv = q(&s.str_req("volts")?, Dim::Voltage, &format!("{at}.volts"))?;
            let rs_uohm = match s.str_opt("rs")? {
                Some(r) => q(&r, Dim::Resistance, &format!("{at}.rs"))?,
                None => 0,
            };
            let track = match s.table_opt("track")? {
                Some(mut tr) => {
                    let n = tr.str_req("net")?;
                    let d = q(
                        &tr.str_req("drop")?,
                        Dim::Voltage,
                        &format!("{at}.track.drop"),
                    )?;
                    tr.done()?;
                    Some((n, d))
                }
                None => None,
            };
            if rs_uohm < 0 {
                return Err(format!("{at}.rs: must be >= 0"));
            }
            s.done()?;
            b.supplies.push(Supply {
                net,
                volts_uv,
                rs_uohm,
                track,
            });
        }
        for mut l in o.tables("load")? {
            let at = l.path.clone();
            let net = l.str_req("net")?;
            if major >= 1 && l.has("replace") {
                return Err(format!(
                    "{at}.replace: static-load mode (ADR-009 D4, milestone P3), not implemented \
                     in this build"
                ));
            }
            let (amps_pa, profile) = if major >= 1 {
                match (l.str_opt("amps")?, l.str_opt("profile")?) {
                    (Some(a), None) => {
                        if l.has("interp") {
                            return Err(format!("{at}.interp: only with a profile"));
                        }
                        (q(&a, Dim::Current, &format!("{at}.amps"))?, None)
                    }
                    (None, Some(f)) => (0, Some((f, interp(&mut l, &at)?))),
                    _ => {
                        return Err(format!(
                            "{at}: give exactly one of amps = \"<I>\" or profile = \"<file.csv>\""
                        ))
                    }
                }
            } else {
                (
                    q(&l.str_req("amps")?, Dim::Current, &format!("{at}.amps"))?,
                    None,
                )
            };
            l.done()?;
            b.loads.push(Load {
                net,
                amps_pa,
                profile,
            });
        }
        if let Some(Value::Table(nt)) = o.get("nominal") {
            for (k, v) in nt {
                let s = v
                    .as_str()
                    .ok_or_else(|| format!("nominal.{k}: expected a string"))?;
                b.nominal
                    .insert(k.clone(), q(s, Dim::Voltage, &format!("nominal.{k}"))?);
            }
        }
        for mut p in o.tables("pwl")? {
            let at = p.path.clone();
            let net = p.str_req("net")?;
            let rs_uohm = match p.str_opt("rs")? {
                Some(r) => q(&r, Dim::Resistance, &format!("{at}.rs"))?,
                None => 0,
            };
            let mut points = Vec::new();
            match p.get("points") {
                Some(Value::Array(a)) => {
                    for pt in a {
                        match pt.as_array().map(Vec::as_slice) {
                            Some([Value::String(t), Value::String(v)]) => points.push((
                                q(t, Dim::Time, &format!("{at}.points"))?,
                                q(v, Dim::Voltage, &format!("{at}.points"))?,
                            )),
                            _ => {
                                return Err(format!(
                                    "{at}.points: expected [[\"<t>\", \"<v>\"], ...]"
                                ))
                            }
                        }
                    }
                }
                _ => return Err(format!("{at}.points: required")),
            }
            if points.is_empty() || points.windows(2).any(|w| w[1].0 <= w[0].0) || points[0].0 < 0 {
                return Err(format!(
                    "{at}.points: times must start at >= 0 and increase"
                ));
            }
            p.done()?;
            // ADR-010 D2: [[pwl]] is for short tables (an inline PWL card is
            // O(n^2) in ngspice); [[playback]] is the route for long ones.
            if points.len() > PWL_REFUSE {
                return Err(format!(
                    "{at}.points: {} points; [[pwl]] takes at most {PWL_REFUSE} (an inline PWL \
                     is O(n^2) in ngspice): use [[playback]] with a CSV (ADR-010 D2)",
                    points.len()
                ));
            }
            if points.len() > PWL_WARN {
                b.warnings.push(format!(
                    "{at}.points: {} points; [[pwl]] is for short tables (above {PWL_WARN}, use \
                     [[playback]], ADR-010 D2)",
                    points.len()
                ));
            }
            b.pwl.push(Pwl {
                net,
                points,
                rs_uohm,
            });
        }
        if let Some(mut a) = o.table_opt("analog")? {
            if a.has("nets") {
                b.analog_nets = Some(a.strs("nets")?);
            }
            b.exclude = a.strs("exclude")?;
            a.done()?;
        }
        if let Some(mut op) = o.table_opt("options")? {
            b.options = op.str_req("set")?;
            op.done()?;
        }
        if bbpkg::optsets::get(&b.options).is_none() {
            return Err(format!(
                "[options] set = {:?}: not a named option set ({}; ADR-006 D6)",
                b.options,
                bbpkg::optsets::names().join(", ")
            ));
        }
        if major >= 1 {
            b.parse_v1(&mut o)?;
        }
        o.done()?;
        Ok(b)
    }

    /// The `bb-bench/1` sections of milestone B1 (ADR-010 D2): `[[action]]`,
    /// `[[playback]]` and `[[fault]]` (`seed` and `[[load]] profile` are
    /// read with the `/0` keys).
    fn parse_v1(&mut self, o: &mut Obj) -> Result<(), String> {
        use bbpkg::value::Dim;
        let time = |text: &str, at: &str| -> Result<i128, String> {
            let t = q(text, Dim::Time, at)?;
            if t < 0 {
                return Err(format!("{at}: a negative time"));
            }
            Ok(t)
        };
        for mut a in o.tables("action")? {
            let at = a.path.clone();
            let kind = match (a.str_opt("press")?, a.str_opt("set")?) {
                (Some(p), None) => {
                    if a.has("to") {
                        return Err(format!("{at}.to: a press takes no value"));
                    }
                    ActionKind::Press(p)
                }
                (None, Some(s)) => ActionKind::Set {
                    target: s,
                    to: a
                        .str_opt("to")?
                        .ok_or_else(|| format!("{at}.to: required with set"))?,
                },
                _ => {
                    return Err(format!(
                        "{at}: give exactly one of press = \"<instance>\" or set = \"<net or pin>\""
                    ))
                }
            };
            let at_t = a
                .str_opt("at")?
                .map(|s| time(&s, &format!("{at}.at")))
                .transpose()?;
            let after = match a.table_opt("after")? {
                Some(mut t) => {
                    let edge = t.str_req("edge")?;
                    let level = match t.get("level") {
                        Some(Value::Integer(0)) => false,
                        Some(Value::Integer(1)) => true,
                        _ => return Err(format!("{at}.after.level: 0 or 1 (an integer)")),
                    };
                    let delay = match t.str_opt("delay")? {
                        Some(d) => time(&d, &format!("{at}.after.delay"))?,
                        None => 0,
                    };
                    if t.has("log") {
                        return Err(format!(
                            "{at}.after.log: a log trigger is not implemented in this build \
                             (milestone B1 triggers on edges; ADR-010 Amendment 1)"
                        ));
                    }
                    t.done()?;
                    Some(Trigger { edge, level, delay })
                }
                None => None,
            };
            if at_t.is_some() == after.is_some() {
                return Err(format!(
                    "{at}: give exactly one of at = \"<t>\" or after = {{ edge, level, delay }}"
                ));
            }
            let for_fs = a
                .str_opt("for")?
                .map(|s| time(&s, &format!("{at}.for")))
                .transpose()?;
            if for_fs == Some(0) {
                return Err(format!("{at}.for: must be > 0"));
            }
            let bounce = match a.table_opt("bounce")? {
                Some(mut t) => {
                    if !matches!(kind, ActionKind::Press(_)) {
                        return Err(format!("{at}.bounce: only a press bounces"));
                    }
                    let count = match t.get("count") {
                        Some(Value::Integer(n)) if (1..=64).contains(n) => *n as u32,
                        _ => return Err(format!("{at}.bounce.count: an integer from 1 to 64")),
                    };
                    let max = t
                        .str_opt("max")?
                        .map(|s| time(&s, &format!("{at}.bounce.max")))
                        .transpose()?;
                    t.done()?;
                    Some((count, max))
                }
                None => None,
            };
            a.done()?;
            self.actions.push(Action {
                path: at,
                kind,
                at: at_t,
                after,
                for_fs,
                bounce,
            });
        }
        for mut p in o.tables("playback")? {
            let at = p.path.clone();
            let net = p.str_req("net")?;
            let file = p.str_req("file")?;
            let rs_uohm = match p.str_opt("rs")? {
                Some(r) => q(&r, Dim::Resistance, &format!("{at}.rs"))?,
                None => 0,
            };
            if rs_uohm < 0 {
                return Err(format!("{at}.rs: must be >= 0"));
            }
            let interp = interp(&mut p, &at)?;
            p.done()?;
            self.playback.push(Playback {
                net,
                file,
                rs_uohm,
                interp,
            });
        }
        for mut f in o.tables("fault")? {
            let at = f.path.clone();
            let kind = match f.str_req("kind")?.as_str() {
                "open" => FaultKind::Open,
                "short" => FaultKind::Short,
                k => return Err(format!("{at}.kind {k:?}: open or short")),
            };
            let net = f.str_req("net")?;
            let pin = f.str_opt("pin")?;
            match (kind, &pin) {
                (FaultKind::Open, None) => {
                    return Err(format!(
                        "{at}.pin: an open fault names the instance pin that leaves the net \
                         (\"main:C8.1\")"
                    ))
                }
                (FaultKind::Short, Some(_)) => {
                    return Err(format!(
                        "{at}.pin: a short is between the net and the ground; it names no pin"
                    ))
                }
                _ => {}
            }
            let t = time(&f.str_req("at")?, &format!("{at}.at"))?;
            let for_fs = f
                .str_opt("for")?
                .map(|s| time(&s, &format!("{at}.for")))
                .transpose()?;
            if for_fs == Some(0) {
                return Err(format!("{at}.for: must be > 0"));
            }
            f.done()?;
            self.faults.push(Fault {
                path: at,
                kind,
                net,
                pin,
                at: t,
                for_fs,
            });
        }
        // The checks (ADR-010 D5, milestone B2).
        self.asserts = crate::asserts::parse_tables(o)?;
        Ok(())
    }
}

/// `[[pwl]]` (ADR-010 D2): a warning above this many points, refused above
/// [`PWL_REFUSE`].
pub const PWL_WARN: usize = 1_000;
pub const PWL_REFUSE: usize = 10_000;

fn interp(o: &mut Obj, at: &str) -> Result<Interp, String> {
    match o.str_opt("interp")?.as_deref() {
        None | Some("linear") => Ok(Interp::Linear),
        Some("hold") => Ok(Interp::Hold),
        Some(x) => Err(format!("{at}.interp {x:?}: linear or hold")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: &str = r#"
schema = "bb-project/0"
[project]
name = "t"
[[board]]
id = "main"
netlist = "kicad/main.net"
[instances."main:J11"]
package = "none"
reason = "NFC coil"
[[bind]]
pin = "U1.PA4"
model = "digital_out"
zout = "50"
[probes]
nets = ["VLED"]
# a comment that must survive
[targets."main:U1"]
images = [{ file = "old.elf", kind = "elf", hash = "sha256:00" }]

# next
[[connection]]
a = "main:J1"
b = "base:J3"
"#;

    #[test]
    fn parses_and_edits_projects() {
        let p = Project::parse(P, Path::new(".")).unwrap();
        assert_eq!(p.simulate, "main");
        assert_eq!(p.boards[0].ground, "GND");
        assert_eq!(
            p.binds[0].params,
            vec![("zout".to_string(), "50".to_string())]
        );
        assert_eq!(p.instances[0].1.reason.as_deref(), Some("NFC coil"));
        assert_eq!(p.targets[0].1[0].file, "old.elf");
        let img = Image {
            file: "fw/new.elf".into(),
            kind: "elf".into(),
            hash: "sha256:11".into(),
            offset: None,
        };
        let edited = set_target(P, "main:U1", std::slice::from_ref(&img));
        assert!(edited.contains("# a comment that must survive"));
        assert!(edited.contains("# next"));
        assert!(!edited.contains("old.elf"));
        let p2 = Project::parse(&edited, Path::new(".")).unwrap();
        assert_eq!(p2.targets[0].1[0], img);
        let added = set_target(&edited, "main:U2", std::slice::from_ref(&img));
        assert_eq!(
            Project::parse(&added, Path::new("."))
                .unwrap()
                .targets
                .len(),
            2
        );
        // The SAADC's binding keys (ADR-008 D7): load is a parameter,
        // trace_samples a boolean binding option.
        let adc = P.replace(
            "model = \"digital_out\"\nzout = \"50\"",
            "model = \"adc_in\"\nload = \"none\"\ntrace_samples = true",
        );
        assert_eq!(
            Project::parse(&adc, Path::new(".")).unwrap().binds[0].params,
            vec![
                ("load".to_string(), "none".to_string()),
                ("trace_samples".to_string(), "true".to_string())
            ]
        );
        assert!(Project::parse(
            &adc.replace("trace_samples = true", "trace_samples = \"true\""),
            Path::new(".")
        )
        .is_err());
        // Errors: unknown keys, package none without a reason, two sources.
        assert!(Project::parse(&P.replace("zout", "colour"), Path::new(".")).is_err());
        assert!(Project::parse(&P.replace("reason = \"NFC coil\"\n", ""), Path::new(".")).is_err());
        assert!(Project::parse(
            &P.replace(
                "netlist = \"kicad/main.net\"",
                "netlist = \"a.net\"\nkicad = \"a.kicad_pro\""
            ),
            Path::new(".")
        )
        .is_err());
    }

    #[test]
    fn bb_project_1_keys_of_the_schema_epoch() {
        // Milestone S: simulate as a list (one board; several are R2's), [run]
        // (R3's) refused with its milestone, [registries] (E1's) read and
        // checked, the bb-pins/1 binding keys rail and rhiz accepted.
        let v1 = P.replacen("bb-project/0", "bb-project/1", 1);
        let with = |from: &str, to: &str| v1.replacen(from, to, 1);
        let p = Project::parse(
            &with("name = \"t\"", "name = \"t\"\nsimulate = [\"main\"]"),
            Path::new("."),
        )
        .unwrap();
        assert_eq!(p.simulate, "main");
        let e = Project::parse(
            &with(
                "name = \"t\"",
                "name = \"t\"\nsimulate = [\"main\", \"base\"]",
            ),
            Path::new("."),
        )
        .unwrap_err();
        assert!(e.contains("milestone R2"), "{e}");
        let e =
            Project::parse(&format!("{v1}\n[run]\ncpu = \"none\"\n"), Path::new(".")).unwrap_err();
        assert!(e.contains("[run] cpu") && e.contains("milestone R3"), "{e}");
        let reg = "\n[registries.acme]\nurl = \"file:///srv/reg\"\nkey = \"RWR3ioZYEKjIEvJyXiAwNL6czjJWdX3+bxrhw4QWChjFKUCBHjYT6EGI\"\nnamespaces = [\"acme\"]\n";
        let p = Project::parse(&format!("{v1}{reg}"), Path::new(".")).unwrap();
        assert_eq!(p.registries[0].namespaces, vec!["acme"]);
        for (from, to, want) in [
            ("file:///srv/reg", "http://x", "https:// or file://"),
            (
                "namespaces = [\"acme\"]",
                "namespaces = []",
                "at least one namespace",
            ),
            (
                "namespaces = [\"acme\"]",
                "namespaces = [\"Acme\"]",
                "not a namespace",
            ),
            (
                "key = \"RWR3ioZYEKjIEvJyXiAwNL6czjJWdX3+bxrhw4QWChjFKUCBHjYT6EGI\"\n",
                "",
                "key: required",
            ),
            (
                "RWR3ioZYEKjIEvJyXiAwNL6czjJWdX3+bxrhw4QWChjFKUCBHjYT6EGI",
                "RWQ",
                "minisign public key",
            ),
            (
                "file:///srv/reg",
                "file://relative",
                "file:///<absolute path>",
            ),
        ] {
            let e = Project::parse(
                &format!("{v1}{}", reg.replacen(from, to, 1)),
                Path::new("."),
            )
            .unwrap_err();
            assert!(e.contains(want), "{want}: {e}");
        }
        let two = format!("{v1}{reg}[registries.other]\nurl = \"https://o\"\nkey = \"RWR3ioZYEKjIEvJyXiAwNL6czjJWdX3+bxrhw4QWChjFKUCBHjYT6EGI\"\nnamespaces = [\"acme\"]\n");
        assert!(Project::parse(&two, Path::new("."))
            .unwrap_err()
            .contains("routed to two registries"));
        assert!(Project::parse(&format!("{P}{reg}"), Path::new("."))
            .unwrap_err()
            .contains("bb-project/1 key"));
        let p = Project::parse(
            &with(
                "zout = \"50\"",
                "zout = \"50\"\nrail = \"VDD\"\nrhiz = \"100Meg\"",
            ),
            Path::new("."),
        )
        .unwrap();
        assert_eq!(p.binds[0].params.len(), 3);
    }

    #[test]
    fn parses_benches() {
        let b = Bench::parse(
            r#"
schema = "bb-bench/0"
temp_c = "-10"
[[supply]]
net = "VSYS"
volts = "3.0V"
rs = "0.569"
[[supply]]
net = "+3V3"
volts = "3.3V"
track = { net = "VSYS", drop = "44.2mV" }
[[load]]
net = "VSYS"
amps = "50mA"
[nominal]
"+3V3" = "3.3V"
[[pwl]]
net = "X"
points = [["0", "0V"], ["1ms", "3.3V"]]
[analog]
nets = ["VSYS"]
exclude = ["U1"]
[options]
set = "phase0"
"#,
        )
        .unwrap();
        assert_eq!(b.temp_mc, Some(-10_000));
        assert_eq!(b.supplies[0].rs_uohm, 569_000);
        assert_eq!(b.supplies[1].track, Some(("VSYS".into(), 44_200)));
        assert_eq!(b.loads[0].amps_pa, 50_000_000_000);
        assert_eq!(b.pwl[0].points[1], (1_000_000_000_000, 3_300_000));
        assert_eq!(b.major, 0);
        assert!(Bench::parse("schema = \"bb-bench/0\"\n[options]\nset = \"loose\"\n").is_err());
    }

    const V1: &str = r#"
schema = "bb-bench/1"
seed = 3
[[supply]]
net = "VSW"
volts = "3.3V"
[[load]]
net = "VSW"
profile = "i.csv"
interp = "hold"
[[playback]]
net = "AIN"
file = "a.csv"
rs = "100"
[[playback]]
net = "U1.PB0"
file = "b.csv"
[[action]]
press = "SW1"
at = "2ms"
for = "50ms"
bounce = { count = 3, max = "1ms" }
[[action]]
set = "VSW"
to = "2.5V"
after = { edge = "U1.PA5", level = 1, delay = "1ms" }
[[fault]]
kind = "open"
net = "BTN"
pin = "main:C1.1"
at = "50ms"
[[fault]]
kind = "short"
net = "AFILT"
at = "60ms"
for = "1ms"
"#;

    const BENCH0: &str =
        "schema = \"bb-bench/0\"\n[[supply]]\nnet = \"VSYS\"\nvolts = \"3.0V\"\nrs = \"0.569\"\n";

    #[test]
    fn parses_bench_v1() {
        // ADR-010 D1-D2 (milestone B1): the /0 keys keep their meaning, and
        // seed, profiles, playback, actions and faults are read.
        let b = Bench::parse(V1).unwrap();
        assert_eq!((b.major, b.seed), (1, 3));
        assert_eq!(b.loads[0].profile, Some(("i.csv".into(), Interp::Hold)));
        assert_eq!(b.playback[0].rs_uohm, 100_000_000);
        assert_eq!(b.playback[1].interp, Interp::Linear);
        assert_eq!(b.actions[0].kind, ActionKind::Press("SW1".into()));
        assert_eq!(b.actions[0].bounce, Some((3, Some(1_000_000_000_000))));
        assert_eq!(b.actions[0].for_fs, Some(50_000_000_000_000));
        assert_eq!(
            b.actions[1].after,
            Some(Trigger {
                edge: "U1.PA5".into(),
                level: true,
                delay: 1_000_000_000_000
            })
        );
        assert_eq!(b.faults[0].kind, FaultKind::Open);
        assert_eq!(b.faults[1].for_fs, Some(1_000_000_000_000));
        // A /0 bench migrated is the same bench.
        let v1 = Bench::parse(&BENCH0.replace("bb-bench/0", "bb-bench/1")).unwrap();
        assert_eq!(v1.supplies, Bench::parse(BENCH0).unwrap().supplies);
        // Refusals.
        let err = |s: String| Bench::parse(&s).unwrap_err();
        assert!(err(V1.replace("bb-bench/1", "bb-bench/0")).contains("amps: required"));
        assert!(err(format!(
            "{BENCH0}[[action]]\npress = \"SW1\"\nat = \"1ms\"\n"
        ))
        .contains("action: unknown key"));
        assert!(err(format!("{BENCH0}seed = 1\n")).contains("seed: unknown key"));
        assert!(err(format!("{V1}[[battery]]\nnet = \"VSW\"\n")).contains("battery"));
        assert!(err(V1.replace(
            "at = \"2ms\"",
            "at = \"2ms\"\nafter = { edge = \"X\", level = 1 }"
        ))
        .contains("exactly one of at"));
        assert!(
            err(V1.replace("press = \"SW1\"", "press = \"SW1\"\nset = \"X\""))
                .contains("exactly one of press")
        );
        assert!(err(V1.replace("level = 1", "level = 2")).contains("0 or 1"));
        assert!(
            err(V1.replace("kind = \"short\"", "kind = \"short\"\npin = \"C1.1\""))
                .contains("names no pin")
        );
        assert!(err(V1.replace("pin = \"main:C1.1\"\n", "")).contains("instance pin"));
        assert!(
            err(V1.replace("interp = \"hold\"", "interp = \"cubic\"")).contains("linear or hold")
        );
        assert!(
            err(V1.replace("profile = \"i.csv\"", "profile = \"i.csv\"\namps = \"1A\""))
                .contains("exactly one of amps")
        );
        assert!(err(V1.replace("count = 3", "count = 0")).contains("1 to 64"));
        assert!(err(V1.replace("for = \"50ms\"", "for = \"0s\"")).contains("> 0"));
        assert!(err(V1.replace("seed = 3", "seed = -1")).contains("seed"));
        let long: String = (0..=PWL_REFUSE)
            .map(|k| format!("[\"{k}us\", \"1V\"], "))
            .collect();
        assert!(
            err(format!("{V1}[[pwl]]\nnet = \"X\"\npoints = [{long}]\n")).contains("at most 10000")
        );
        let mid: String = (0..=PWL_WARN)
            .map(|k| format!("[\"{k}us\", \"1V\"], "))
            .collect();
        let w = Bench::parse(&format!("{V1}[[pwl]]\nnet = \"X\"\npoints = [{mid}]\n")).unwrap();
        assert!(w.warnings[0].contains("short tables"), "{:?}", w.warnings);
    }
}
