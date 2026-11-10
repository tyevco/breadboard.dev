//! The elaborated board, `board.bbm` (ADR-007 D6, `spec/board-bbm.md`).
//!
//! Data only: the types `bb open` writes and `bb run` reads, and their
//! canonical JSON form ([`crate::cjson`]). `board.bbm` is a pure function of
//! the canonical netlist content of each board, the board-shaping tables of
//! `bb.toml` and the locked package and platform identities; source paths,
//! hashes, tool strings and dates live in `.bb/import.json` instead, so the
//! `.net` and `.kicad_sch` routes, KiCad 9 and 10, and Linux and Windows
//! `kicad-cli` give the same bytes.
//!
//! Ordering (bytewise on UTF-8): nets by display name, instances by board
//! then reference, nodes by reference then pin number, pins by number.
//! [`Board::to_text`] sorts before writing, so a builder need not.

use std::collections::BTreeMap;

use crate::cjson::{Json, JsonError};

/// The schema id of `board.bbm` (ADR-006 D11: every schema is at major 0).
pub const SCHEMA: &str = "bb-board/0";

/// One KiCad project of the system (`[[board]]` in `bb.toml`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardDef {
    pub id: String,
    /// The display name of the board's ground net (default `GND`).
    pub ground: String,
}

/// A pin on a net: `(reference, pin number)`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Node {
    pub reference: String,
    pub pin: String,
}

/// A net.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Net {
    /// The display name: the KiCad name, qualified `<board>:` in a system.
    pub name: String,
    pub board: String,
    /// The SPICE node name (`spec/board-bbm.md` section 4); `0` for the
    /// simulated board's ground.
    pub spice: String,
    pub nodes: Vec<Node>,
    /// A KiCad `unconnected-(...)` net: probe-only, never bound (D3).
    pub unconnected: bool,
}

/// One pin of an instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstancePin {
    pub number: String,
    /// The pin name from the netlist's `libparts` (KiCad `~` is empty).
    pub name: String,
    /// The display name of its net, if the pin is on one.
    pub net: Option<String>,
}

/// A package as locked: id, version and tree hash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageRef {
    pub id: String,
    pub version: String,
    pub tree: String,
}

/// How an instance was resolved (ADR-007 D4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolution {
    /// `override`, `bb_part`, `mpn`, `lcsc`, `generic`, `no_model` or
    /// `unresolved`.
    pub stage: String,
    /// What matched (the normalised MPN, the footprint library, ...).
    pub key: Option<String>,
}

pub const STAGES: &[&str] = &[
    "override",
    "bb_part",
    "mpn",
    "lcsc",
    "generic",
    "no_model",
    "unresolved",
];

/// A parsed generic-passive value (ADR-006 D5), in integer sub-units.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PassiveValue {
    /// `ohm_u` (µΩ), `farad_a` (aF) or `henry_f` (fH).
    pub unit: String,
    pub value: i128,
    pub tolerance_ppm: Option<i128>,
    pub rating_uv: Option<i128>,
    pub power_uw: Option<i128>,
    pub dielectric: Option<String>,
    pub technology: Option<String>,
}

/// What happens to an instance in a simulation.
pub const STATUSES: &[&str] = &["ok", "no_model", "excluded", "unmodelled"];

/// A component instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instance {
    pub board: String,
    pub reference: String,
    pub value_text: String,
    pub value: Option<PassiveValue>,
    pub footprint: String,
    /// `libsource` lib and part.
    pub lib: String,
    pub part: String,
    /// The sheet path names (`/MCU, flash & debug/`).
    pub sheet: String,
    /// The D3 fields that are present (`MPN`, `Manufacturer`, `LCSC`,
    /// `bb:part`, and those `bb.toml` adds).
    pub fields: BTreeMap<String, String>,
    /// The flags that are set: `dnp`, `exclude_from_bom`,
    /// `exclude_from_sim`, sorted.
    pub flags: Vec<String>,
    pub package: Option<PackageRef>,
    pub resolution: Resolution,
    /// `ok` (has a package), `no_model` (needs none), `excluded` (an
    /// override, DNP or `exclude_from_sim`), `unmodelled` (unresolved).
    pub status: String,
    pub reason: Option<String>,
    pub pins: Vec<InstancePin>,
}

/// An engine platform identity and its content hash (ADR-006 D8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Platform {
    pub id: String,
    pub content: String,
}

/// A probe alias: a test point and the net it touches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeAlias {
    pub alias: String,
    pub net: String,
}

/// A mated connector pair between two boards (`[[connection]]`, M11).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Connection {
    pub a: String,
    pub b: String,
    pub model: Option<String>,
    pub pins: Vec<(String, String)>,
}

/// The elaborated board.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Board {
    pub boards: Vec<BoardDef>,
    pub nets: Vec<Net>,
    pub instances: Vec<Instance>,
    pub platforms: Vec<Platform>,
    pub probes: Vec<ProbeAlias>,
    pub connections: Vec<Connection>,
    /// The verbatim SPICE body of a hand-written `board.cir` (ADR-007 D9);
    /// an imported board has none.
    pub spice_body: Option<String>,
}

fn s(x: &str) -> Json {
    Json::str(x)
}

fn opt_str(v: &mut Vec<(String, Json)>, k: &str, x: &Option<String>) {
    if let Some(x) = x {
        v.push((k.into(), s(x)));
    }
}

fn opt_int(v: &mut Vec<(String, Json)>, k: &str, x: &Option<i128>) {
    if let Some(x) = x {
        v.push((k.into(), Json::Int(*x)));
    }
}

impl Board {
    /// Sort every array by its key (the canonical order).
    pub fn sort(&mut self) {
        self.boards.sort_by(|a, b| a.id.cmp(&b.id));
        for n in &mut self.nets {
            n.nodes.sort();
            n.nodes.dedup();
        }
        self.nets.sort_by(|a, b| a.name.cmp(&b.name));
        for i in &mut self.instances {
            i.pins.sort_by(|a, b| a.number.cmp(&b.number));
            i.flags.sort();
        }
        self.instances.sort_by(|a, b| {
            (a.board.as_str(), a.reference.as_str()).cmp(&(b.board.as_str(), b.reference.as_str()))
        });
        self.platforms.sort_by(|a, b| a.id.cmp(&b.id));
        self.probes.sort_by(|a, b| a.alias.cmp(&b.alias));
        self.connections
            .sort_by(|a, b| (a.a.as_str(), a.b.as_str()).cmp(&(b.a.as_str(), b.b.as_str())));
    }

    /// The canonical JSON value (arrays must already be sorted).
    pub fn to_json(&self) -> Json {
        let boards = self
            .boards
            .iter()
            .map(|b| Json::obj([("id", s(&b.id)), ("ground", s(&b.ground))]))
            .collect();
        let nets = self
            .nets
            .iter()
            .map(|n| {
                let mut o = vec![
                    ("name".to_string(), s(&n.name)),
                    ("board".to_string(), s(&n.board)),
                    ("spice".to_string(), s(&n.spice)),
                    (
                        "nodes".to_string(),
                        Json::Arr(
                            n.nodes
                                .iter()
                                .map(|x| Json::obj([("ref", s(&x.reference)), ("pin", s(&x.pin))]))
                                .collect(),
                        ),
                    ),
                ];
                if n.unconnected {
                    o.push(("unconnected".into(), Json::Bool(true)));
                }
                Json::Obj(o)
            })
            .collect();
        let instances = self.instances.iter().map(instance_json).collect();
        let mut top = vec![
            ("schema".to_string(), s(SCHEMA)),
            ("boards".to_string(), Json::Arr(boards)),
            ("nets".to_string(), Json::Arr(nets)),
            ("instances".to_string(), Json::Arr(instances)),
            (
                "platforms".to_string(),
                Json::Arr(
                    self.platforms
                        .iter()
                        .map(|p| Json::obj([("id", s(&p.id)), ("content", s(&p.content))]))
                        .collect(),
                ),
            ),
            (
                "probes".to_string(),
                Json::Arr(
                    self.probes
                        .iter()
                        .map(|p| Json::obj([("alias", s(&p.alias)), ("net", s(&p.net))]))
                        .collect(),
                ),
            ),
        ];
        if !self.connections.is_empty() {
            top.push((
                "connections".into(),
                Json::Arr(
                    self.connections
                        .iter()
                        .map(|c| {
                            let mut o =
                                vec![("a".to_string(), s(&c.a)), ("b".to_string(), s(&c.b))];
                            opt_str(&mut o, "model", &c.model);
                            if !c.pins.is_empty() {
                                o.push((
                                    "pins".into(),
                                    Json::Arr(
                                        c.pins
                                            .iter()
                                            .map(|(x, y)| Json::Arr(vec![s(x), s(y)]))
                                            .collect(),
                                    ),
                                ));
                            }
                            Json::Obj(o)
                        })
                        .collect(),
                ),
            ));
        }
        if let Some(b) = &self.spice_body {
            top.push(("spice_body".into(), s(b)));
        }
        Json::Obj(top)
    }

    /// The canonical text of `board.bbm` (sorted first).
    pub fn to_text(&self) -> String {
        let mut b = self.clone();
        b.sort();
        b.to_json().to_canonical()
    }

    /// Read `board.bbm`.
    pub fn parse(text: &str) -> Result<Board, String> {
        let j = Json::parse(text).map_err(|e: JsonError| format!("board.bbm: {e}"))?;
        from_json(&j).map_err(|e| format!("board.bbm: {e}"))
    }

    pub fn instance(&self, board: &str, reference: &str) -> Option<&Instance> {
        self.instances
            .iter()
            .find(|i| i.board == board && i.reference == reference)
    }

    pub fn net(&self, name: &str) -> Option<&Net> {
        self.nets.iter().find(|n| n.name == name)
    }
}

fn instance_json(i: &Instance) -> Json {
    let mut o = vec![
        ("board".to_string(), s(&i.board)),
        ("ref".to_string(), s(&i.reference)),
        ("value_text".to_string(), s(&i.value_text)),
        ("footprint".to_string(), s(&i.footprint)),
        ("lib".to_string(), s(&i.lib)),
        ("part".to_string(), s(&i.part)),
        ("sheet".to_string(), s(&i.sheet)),
        ("status".to_string(), s(&i.status)),
    ];
    if let Some(v) = &i.value {
        let mut vo = vec![(v.unit.clone(), Json::Int(v.value))];
        opt_int(&mut vo, "tol_ppm", &v.tolerance_ppm);
        opt_int(&mut vo, "rating_uv", &v.rating_uv);
        opt_int(&mut vo, "power_uw", &v.power_uw);
        opt_str(&mut vo, "dielectric", &v.dielectric);
        opt_str(&mut vo, "technology", &v.technology);
        o.push(("value".into(), Json::Obj(vo)));
    }
    if !i.fields.is_empty() {
        o.push((
            "fields".into(),
            Json::Obj(i.fields.iter().map(|(k, v)| (k.clone(), s(v))).collect()),
        ));
    }
    if !i.flags.is_empty() {
        o.push((
            "flags".into(),
            Json::Arr(i.flags.iter().map(|f| s(f)).collect()),
        ));
    }
    if let Some(p) = &i.package {
        o.push((
            "package".into(),
            Json::obj([
                ("id", s(&p.id)),
                ("version", s(&p.version)),
                ("tree", s(&p.tree)),
            ]),
        ));
    }
    let mut r = vec![("stage".to_string(), s(&i.resolution.stage))];
    opt_str(&mut r, "key", &i.resolution.key);
    o.push(("resolution".into(), Json::Obj(r)));
    opt_str(&mut o, "reason", &i.reason);
    o.push((
        "pins".into(),
        Json::Arr(
            i.pins
                .iter()
                .map(|p| {
                    let mut po = vec![
                        ("number".to_string(), s(&p.number)),
                        ("name".to_string(), s(&p.name)),
                    ];
                    opt_str(&mut po, "net", &p.net);
                    Json::Obj(po)
                })
                .collect(),
        ),
    ));
    Json::Obj(o)
}

// --- reading ----------------------------------------------------------------

struct O<'a> {
    what: String,
    pairs: &'a [(String, Json)],
    used: Vec<&'a str>,
}

impl<'a> O<'a> {
    fn new(what: impl Into<String>, j: &'a Json) -> Result<O<'a>, String> {
        let what = what.into();
        let pairs = j
            .as_obj()
            .ok_or_else(|| format!("{what}: expected an object"))?;
        Ok(O {
            what,
            pairs,
            used: Vec::new(),
        })
    }
    fn get(&mut self, k: &'a str) -> Option<&'a Json> {
        self.used.push(k);
        self.pairs.iter().find(|(x, _)| x == k).map(|(_, v)| v)
    }
    fn str(&mut self, k: &'a str) -> Result<String, String> {
        let w = self.what.clone();
        self.get(k)
            .and_then(Json::as_str)
            .map(str::to_string)
            .ok_or_else(|| format!("{w}: {k} must be a string"))
    }
    fn opt_str(&mut self, k: &'a str) -> Result<Option<String>, String> {
        let w = self.what.clone();
        match self.get(k) {
            None => Ok(None),
            Some(Json::Str(x)) => Ok(Some(x.clone())),
            Some(_) => Err(format!("{w}: {k} must be a string")),
        }
    }
    fn opt_int(&mut self, k: &'a str) -> Result<Option<i128>, String> {
        let w = self.what.clone();
        match self.get(k) {
            None => Ok(None),
            Some(Json::Int(x)) => Ok(Some(*x)),
            Some(_) => Err(format!("{w}: {k} must be an integer")),
        }
    }
    fn arr(&mut self, k: &'a str) -> Result<&'a [Json], String> {
        let w = self.what.clone();
        match self.get(k) {
            None => Ok(&[]),
            Some(Json::Arr(a)) => Ok(a),
            Some(_) => Err(format!("{w}: {k} must be an array")),
        }
    }
    fn done(self) -> Result<(), String> {
        for (k, _) in self.pairs {
            if !self.used.contains(&k.as_str()) {
                return Err(format!("{}: unknown key {k:?}", self.what));
            }
        }
        Ok(())
    }
}

fn from_json(j: &Json) -> Result<Board, String> {
    let mut top = O::new("board", j)?;
    let schema = top.str("schema")?;
    if schema != SCHEMA {
        return Err(format!(
            "schema {schema:?}: this bb reads {SCHEMA} (ADR-006 D11; run `bb open` again)"
        ));
    }
    let mut b = Board::default();
    for x in top.arr("boards")? {
        let mut o = O::new("boards[]", x)?;
        b.boards.push(BoardDef {
            id: o.str("id")?,
            ground: o.str("ground")?,
        });
        o.done()?;
    }
    for x in top.arr("nets")? {
        let mut o = O::new("nets[]", x)?;
        let mut nodes = Vec::new();
        for n in o.arr("nodes")? {
            let mut no = O::new("nodes[]", n)?;
            nodes.push(Node {
                reference: no.str("ref")?,
                pin: no.str("pin")?,
            });
            no.done()?;
        }
        let unconnected = match o.get("unconnected") {
            None => false,
            Some(Json::Bool(v)) => *v,
            Some(_) => return Err("nets[]: unconnected must be a boolean".into()),
        };
        b.nets.push(Net {
            name: o.str("name")?,
            board: o.str("board")?,
            spice: o.str("spice")?,
            nodes,
            unconnected,
        });
        o.done()?;
    }
    for x in top.arr("instances")? {
        b.instances.push(instance_from(x)?);
    }
    for x in top.arr("platforms")? {
        let mut o = O::new("platforms[]", x)?;
        b.platforms.push(Platform {
            id: o.str("id")?,
            content: o.str("content")?,
        });
        o.done()?;
    }
    for x in top.arr("probes")? {
        let mut o = O::new("probes[]", x)?;
        b.probes.push(ProbeAlias {
            alias: o.str("alias")?,
            net: o.str("net")?,
        });
        o.done()?;
    }
    for x in top.arr("connections")? {
        let mut o = O::new("connections[]", x)?;
        let mut pins = Vec::new();
        for p in o.arr("pins")? {
            match p.as_arr() {
                Some([Json::Str(a), Json::Str(c)]) => pins.push((a.clone(), c.clone())),
                _ => return Err("connections[].pins: expected [a, b] pairs".into()),
            }
        }
        b.connections.push(Connection {
            a: o.str("a")?,
            b: o.str("b")?,
            model: o.opt_str("model")?,
            pins,
        });
        o.done()?;
    }
    b.spice_body = top.opt_str("spice_body")?;
    top.done()?;
    Ok(b)
}

fn instance_from(x: &Json) -> Result<Instance, String> {
    let mut o = O::new("instances[]", x)?;
    let value = match o.get("value") {
        None => None,
        Some(v) => {
            let pairs = v.as_obj().ok_or("instances[].value: expected an object")?;
            let unit = pairs
                .iter()
                .find(|(k, _)| matches!(k.as_str(), "ohm_u" | "farad_a" | "henry_f"))
                .ok_or("instances[].value: no ohm_u, farad_a or henry_f")?;
            let mut vo = O::new("instances[].value", v)?;
            let value = vo
                .opt_int(match unit.0.as_str() {
                    "ohm_u" => "ohm_u",
                    "farad_a" => "farad_a",
                    _ => "henry_f",
                })?
                .ok_or("instances[].value: the value must be an integer")?;
            let pv = PassiveValue {
                unit: unit.0.clone(),
                value,
                tolerance_ppm: vo.opt_int("tol_ppm")?,
                rating_uv: vo.opt_int("rating_uv")?,
                power_uw: vo.opt_int("power_uw")?,
                dielectric: vo.opt_str("dielectric")?,
                technology: vo.opt_str("technology")?,
            };
            vo.done()?;
            Some(pv)
        }
    };
    let mut fields = BTreeMap::new();
    if let Some(f) = o.get("fields") {
        for (k, v) in f.as_obj().ok_or("instances[].fields: expected an object")? {
            fields.insert(
                k.clone(),
                v.as_str()
                    .ok_or("instances[].fields: strings only")?
                    .to_string(),
            );
        }
    }
    let mut flags = Vec::new();
    for f in o.arr("flags")? {
        flags.push(
            f.as_str()
                .ok_or("instances[].flags: strings only")?
                .to_string(),
        );
    }
    let package = match o.get("package") {
        None => None,
        Some(p) => {
            let mut po = O::new("instances[].package", p)?;
            let r = PackageRef {
                id: po.str("id")?,
                version: po.str("version")?,
                tree: po.str("tree")?,
            };
            po.done()?;
            Some(r)
        }
    };
    let resolution = {
        let r = o.get("resolution").ok_or("instances[]: no resolution")?;
        let mut ro = O::new("instances[].resolution", r)?;
        let stage = ro.str("stage")?;
        if !STAGES.contains(&stage.as_str()) {
            return Err(format!("instances[].resolution: unknown stage {stage:?}"));
        }
        let key = ro.opt_str("key")?;
        ro.done()?;
        Resolution { stage, key }
    };
    let mut pins = Vec::new();
    for p in o.arr("pins")? {
        let mut po = O::new("instances[].pins[]", p)?;
        pins.push(InstancePin {
            number: po.str("number")?,
            name: po.str("name")?,
            net: po.opt_str("net")?,
        });
        po.done()?;
    }
    let status = o.str("status")?;
    if !STATUSES.contains(&status.as_str()) {
        return Err(format!("instances[]: unknown status {status:?}"));
    }
    let i = Instance {
        board: o.str("board")?,
        reference: o.str("ref")?,
        value_text: o.str("value_text")?,
        value,
        footprint: o.str("footprint")?,
        lib: o.str("lib")?,
        part: o.str("part")?,
        sheet: o.str("sheet")?,
        fields,
        flags,
        package,
        resolution,
        status,
        reason: o.opt_str("reason")?,
        pins,
    };
    o.done()?;
    Ok(i)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Board {
        Board {
            boards: vec![BoardDef {
                id: "main".into(),
                ground: "GND".into(),
            }],
            nets: vec![
                Net {
                    name: "VSYS".into(),
                    board: "main".into(),
                    spice: "VSYS".into(),
                    nodes: vec![
                        Node {
                            reference: "R2".into(),
                            pin: "1".into(),
                        },
                        Node {
                            reference: "R10".into(),
                            pin: "2".into(),
                        },
                    ],
                    unconnected: false,
                },
                Net {
                    name: "GND".into(),
                    board: "main".into(),
                    spice: "0".into(),
                    nodes: vec![],
                    unconnected: false,
                },
            ],
            instances: vec![Instance {
                board: "main".into(),
                reference: "R2".into(),
                value_text: "2.2M".into(),
                value: Some(PassiveValue {
                    unit: "ohm_u".into(),
                    value: 2_200_000_000_000,
                    tolerance_ppm: Some(10_000),
                    rating_uv: None,
                    power_uw: None,
                    dielectric: None,
                    technology: None,
                }),
                footprint: "Resistor_SMD:R_0402_1005Metric".into(),
                lib: "Device".into(),
                part: "R".into(),
                sheet: "/".into(),
                fields: [("MPN".to_string(), "X".to_string())].into(),
                flags: vec!["dnp".into()],
                package: Some(PackageRef {
                    id: "generic/resistor".into(),
                    version: "0.1.0".into(),
                    tree: "sha256:00".into(),
                }),
                resolution: Resolution {
                    stage: "generic".into(),
                    key: Some("Resistor_SMD".into()),
                },
                status: "excluded".into(),
                reason: Some("dnp".into()),
                pins: vec![InstancePin {
                    number: "1".into(),
                    name: "".into(),
                    net: Some("VSYS".into()),
                }],
            }],
            platforms: vec![],
            probes: vec![ProbeAlias {
                alias: "TP1".into(),
                net: "VSYS".into(),
            }],
            connections: vec![],
            spice_body: None,
        }
    }

    #[test]
    fn round_trips_and_sorts_bytewise() {
        let b = sample();
        let text = b.to_text();
        // Nets by display name, nodes by ref bytewise ("R10" < "R2").
        assert!(text.find("\"name\":\"GND\"").unwrap() < text.find("\"name\":\"VSYS\"").unwrap());
        let nets = &text[text.find("\"nets\"").unwrap()..];
        assert!(nets.find("\"ref\":\"R10\"").unwrap() < nets.find("\"ref\":\"R2\"").unwrap());
        let back = Board::parse(&text).unwrap();
        assert_eq!(back, {
            let mut s = b.clone();
            s.sort();
            s
        });
        assert_eq!(back.to_text(), text);
        assert!(text.ends_with("}\n") && text.matches('\n').count() == 1);
        assert!(Board::parse(&text.replace("bb-board/0", "bb-board/9")).is_err());
        assert!(
            Board::parse(&text.replace("\"status\":\"excluded\"", "\"status\":\"maybe\"")).is_err()
        );
    }
}
