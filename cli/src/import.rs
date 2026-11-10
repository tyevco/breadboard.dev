//! KiCad import and part resolution (ADR-007 D1-D4, D6): a project's
//! boards, as KiCad netlists, plus its packages, become the elaborated
//! board (`board.bbm`) and the files of `.bb/`.
//!
//! `board.bbm` is a pure function of the netlist content the importer reads
//! (D3), the board-shaping tables of `bb.toml` (`[[board]]` ids and grounds,
//! `[instances]`, `[packages]`, `[[connection]]`) and the packages found on
//! the search path (with their tree hashes) and their platforms. Where the
//! netlist came from (a `.net` or `kicad-cli` on the schematics, KiCad 9
//! or 10, Linux or Windows) goes to `.bb/import.json` only.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use bb_kicad::netlist::{self, Netlist};
use bb_kicad::{cli::KicadCli, schematic};
use bbcore::board::{
    Board, BoardDef, Connection, Instance, InstancePin, Net, Node, PackageRef, PassiveValue,
    Platform, ProbeAlias, Resolution,
};
use bbcore::cjson::Json;
use bbpkg::lock::{self, LockedPackage, LockedPlatform};
use bbpkg::package::{self, Package};
use bbpkg::pattern::Pattern;
use bbpkg::schema::Registry;
use bbpkg::value::{parse_kicad_value, PassiveKind};

use crate::names;
use crate::platforms;
use crate::project::{Project, Source, D3_FIELDS};

/// Where `bb open` keeps its non-canonical outputs.
pub const DOT_BB: &str = ".bb";
pub const BOARD_FILE: &str = "board.bbm";

/// How the netlists are obtained.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// `bb open` (and `--check`): read `.net` files, run `kicad-cli` for
    /// `kicad =` boards.
    Open,
    /// `bb run`: read `.net` files; for `kicad =` boards, the netlist that
    /// `bb open` cached in `.bb/`, after checking that every schematic file
    /// it was exported from is unchanged (no `kicad-cli` per run).
    Run,
}

/// The packages on a search path, each id once (the first directory that
/// provides it wins, ADR-006 D1).
pub struct Packages {
    pub list: Vec<Package>,
    trees: BTreeMap<String, String>,
}

impl Packages {
    pub fn load(sp: &bbpkg::SearchPath) -> Result<Packages, String> {
        let reg = Registry::current();
        let mut list: Vec<Package> = Vec::new();
        for d in &sp.dirs {
            if !d.is_dir() {
                continue;
            }
            let mut here: Vec<Package> = Vec::new();
            for p in package::discover(d)? {
                let pkg = package::load(&p, &reg)?;
                if let Some(o) = here.iter().find(|x| x.part.id == pkg.part.id) {
                    return Err(format!(
                        "package {} is provided twice in {}: {}, {}",
                        pkg.part.id,
                        d.display(),
                        o.dir.display(),
                        pkg.dir.display()
                    ));
                }
                here.push(pkg);
            }
            for pkg in here {
                if !list.iter().any(|x| x.part.id == pkg.part.id) {
                    list.push(pkg);
                }
            }
        }
        list.sort_by(|a, b| a.part.id.cmp(&b.part.id));
        Ok(Packages {
            list,
            trees: BTreeMap::new(),
        })
    }

    pub fn get(&self, spec: &str) -> Result<&Package, String> {
        let (id, want) = match spec.split_once('@') {
            Some((i, v)) => (i, Some(v)),
            None => (spec, None),
        };
        let p = self
            .list
            .iter()
            .find(|p| p.part.id == id)
            .ok_or_else(|| format!("package {id} not found on the search path"))?;
        if let Some(v) = want {
            if p.part.version != v {
                return Err(format!(
                    "package {id}@{v}: {} has version {}",
                    p.dir.display(),
                    p.part.version
                ));
            }
        }
        Ok(p)
    }

    fn tree(&mut self, id: &str) -> Result<String, String> {
        if let Some(t) = self.trees.get(id) {
            return Ok(t.clone());
        }
        let p = self.get(id)?;
        let dir = std::fs::canonicalize(&p.dir).map_err(|e| format!("{}: {e}", p.dir.display()))?;
        let t = lock::tree_hash(&dir)?;
        self.trees.insert(id.to_string(), t.clone());
        Ok(t)
    }
}

/// Normalise an MPN field (ADR-007 D4 stage 3): trim, and drop one
/// trailing parenthesised comment. Nothing else is removed.
pub fn normalize_mpn(mpn: &str) -> String {
    let t = mpn.trim();
    if t.ends_with(')') {
        if let Some(open) = t.rfind('(') {
            let head = t[..open].trim_end();
            if !head.is_empty() {
                return head.to_string();
            }
        }
    }
    t.to_string()
}

/// A footprint glob (`Resistor_SMD:*`): `*` matches any run of characters.
fn glob(pat: &str, s: &str) -> bool {
    fn go(p: &[char], s: &[char]) -> bool {
        match p.split_first() {
            None => s.is_empty(),
            Some(('*', rest)) => (0..=s.len()).any(|i| go(rest, &s[i..])),
            Some((c, rest)) => s.first() == Some(c) && go(rest, &s[1..]),
        }
    }
    let p: Vec<char> = pat.chars().collect();
    let s: Vec<char> = s.chars().collect();
    go(&p, &s)
}

const GENERIC: &[(&str, PassiveKind, char)] = &[
    ("generic/resistor", PassiveKind::Resistor, 'R'),
    ("generic/capacitor", PassiveKind::Capacitor, 'C'),
    ("generic/inductor", PassiveKind::Inductor, 'L'),
];

/// Footprint libraries whose parts need no model (ADR-007 D4 stage 6).
const NO_MODEL: &[&str] = &["MountingHole", "Fiducial", "TestPoint"];

fn unit_key(k: PassiveKind) -> &'static str {
    match k {
        PassiveKind::Resistor => "ohm_u",
        PassiveKind::Capacitor => "farad_a",
        PassiveKind::Inductor => "henry_f",
    }
}

/// Everything one import produces.
pub struct Imported {
    pub board: Board,
    /// The resolved packages by id.
    pub packages: BTreeMap<String, Package>,
    pub locked_packages: Vec<LockedPackage>,
    pub locked_platforms: Vec<LockedPlatform>,
    pub resolution: Json,
    pub unresolved_toml: String,
    pub import_json: Json,
    /// `kicad =` boards: `(board id, exported netlist)` for `.bb/`.
    pub exported: Vec<(String, String)>,
    pub warnings: Vec<String>,
    /// `(stage, count)` and `(status, count)`.
    pub stage_counts: BTreeMap<String, usize>,
    pub status_counts: BTreeMap<String, usize>,
    /// Unresolved (unmodelled) instances, qualified.
    pub unresolved: Vec<String>,
}

fn rel(p: &str) -> String {
    p.replace('\\', "/")
}

fn sha(bytes: &[u8]) -> String {
    format!("sha256:{}", bbpkg::sha256::hex_digest(bytes))
}

/// The netlist of one board and where it came from.
struct Got {
    text: String,
    file: String,
    import: Json,
    exported: bool,
}

fn get_netlist(p: &Project, b: &crate::project::BoardEntry, mode: Mode) -> Result<Got, String> {
    match &b.source {
        Source::Netlist(f) => {
            let path = p.dir.join(f);
            let bytes =
                std::fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
            let text = String::from_utf8(bytes.clone())
                .map_err(|_| format!("{}: not UTF-8", path.display()))?;
            Ok(Got {
                import: Json::obj([
                    ("id", Json::str(&b.id)),
                    ("route", Json::str("netlist")),
                    (
                        "sources",
                        Json::Arr(vec![Json::obj([
                            ("path", Json::str(rel(f))),
                            ("sha256", Json::str(sha(&bytes))),
                        ])]),
                    ),
                ]),
                text,
                file: rel(f),
                exported: false,
            })
        }
        Source::Kicad(pro) => {
            let pro_path = p.dir.join(pro);
            let proj = schematic::read_project(&pro_path)?;
            let pro_dir_rel = Path::new(pro)
                .parent()
                .map(|d| rel(&d.to_string_lossy()))
                .unwrap_or_default();
            let prefix = |f: &str| {
                if pro_dir_rel.is_empty() {
                    f.to_string()
                } else {
                    format!("{pro_dir_rel}/{f}")
                }
            };
            let schematics = Json::Arr(
                proj.sheets
                    .iter()
                    .map(|s| {
                        Json::obj([
                            ("path", Json::str(prefix(&s.rel))),
                            ("version", Json::Int(s.version as i128)),
                        ])
                    })
                    .collect(),
            );
            let cache = p.dir.join(DOT_BB).join(format!("{}.net", b.id));
            let (text, files, tool) = match mode {
                Mode::Open => {
                    let cli = KicadCli::find()?;
                    let ver = cli.version().unwrap_or_default();
                    let ex = cli.export(&proj)?;
                    (ex.netlist, ex.copied, Some(ver))
                }
                Mode::Run => {
                    let ij = read_import_json(&p.dir)?;
                    let entry = ij
                        .get("boards")
                        .and_then(Json::as_arr)
                        .and_then(|a| {
                            a.iter()
                                .find(|x| x.get("id").and_then(Json::as_str) == Some(&b.id))
                        })
                        .ok_or_else(|| {
                            format!("board {}: not in .bb/import.json: run `bb open`", b.id)
                        })?;
                    for s in entry.get("sources").and_then(Json::as_arr).unwrap_or(&[]) {
                        let f = s.get("path").and_then(Json::as_str).unwrap_or_default();
                        let want = s.get("sha256").and_then(Json::as_str).unwrap_or_default();
                        let now = std::fs::read(p.dir.join(f))
                            .map(|b| sha(&b))
                            .unwrap_or_default();
                        if now != want {
                            return Err(format!(
                                "{f} changed since `bb open` (ADR-007 D7): run `bb open`"
                            ));
                        }
                    }
                    let text = std::fs::read_to_string(&cache)
                        .map_err(|e| format!("read {}: {e}: run `bb open`", cache.display()))?;
                    let want = entry
                        .get("netlist_sha256")
                        .and_then(Json::as_str)
                        .unwrap_or_default();
                    if sha(text.as_bytes()) != want {
                        return Err(format!(
                            "{} changed since `bb open`: run `bb open`",
                            cache.display()
                        ));
                    }
                    return Ok(Got {
                        text,
                        file: format!("{DOT_BB}/{}.net", b.id),
                        import: entry.clone(),
                        exported: false,
                    });
                }
            };
            let mut sources = Vec::new();
            for f in &files {
                let bytes =
                    std::fs::read(proj.dir.join(f)).map_err(|e| format!("read {f}: {e}"))?;
                sources.push(Json::obj([
                    ("path", Json::str(prefix(f))),
                    ("sha256", Json::str(sha(&bytes))),
                ]));
            }
            let mut o = vec![
                ("id".to_string(), Json::str(&b.id)),
                ("route".to_string(), Json::str("kicad")),
                ("sources".to_string(), Json::Arr(sources)),
                ("schematics".to_string(), schematics),
                (
                    "netlist_sha256".to_string(),
                    Json::str(sha(text.as_bytes())),
                ),
            ];
            if let Some(t) = tool {
                o.push(("kicad_cli".into(), Json::str(t)));
            }
            Ok(Got {
                text,
                file: format!("{pro} (kicad-cli)"),
                import: Json::Obj(o),
                exported: true,
            })
        }
    }
}

fn read_import_json(dir: &Path) -> Result<Json, String> {
    let p = dir.join(DOT_BB).join("import.json");
    let t = std::fs::read_to_string(&p)
        .map_err(|e| format!("read {}: {e}: run `bb open`", p.display()))?;
    Json::parse(&t).map_err(|e| format!("{}: {e}", p.display()))
}

/// One stage the resolver tried, for `.bb/resolution.json`.
struct Tried {
    stage: &'static str,
    key: String,
    result: String,
}

/// Import a project.
pub fn import(p: &Project, mode: Mode) -> Result<Imported, String> {
    let mut warnings = Vec::new();
    let mut pk = Packages::load(&p.search_path())?;
    let multi = p.multi();

    // Netlists.
    let mut lists: Vec<(String, Netlist)> = Vec::new();
    let mut import_boards = Vec::new();
    let mut exported = Vec::new();
    for b in &p.boards {
        let got = get_netlist(p, b, mode)?;
        let n = netlist::parse(&got.text, &got.file)?;
        warnings.extend(n.warnings.iter().cloned());
        let mut ij = got.import;
        if let Json::Obj(o) = &mut ij {
            if !o.iter().any(|(k, _)| k == "tool") {
                o.push(("tool".into(), Json::str(&n.tool)));
            }
        }
        import_boards.push(ij);
        if got.exported {
            exported.push((b.id.clone(), got.text));
        }
        lists.push((b.id.clone(), n));
    }

    // Instance names, for [instances] keys.
    let inst_items: Vec<(&str, &str, &str)> = lists
        .iter()
        .flat_map(|(b, n)| {
            n.components
                .iter()
                .map(move |c| (b.as_str(), c.reference.as_str(), c.value.as_str()))
        })
        .collect();
    let insts = names::Instances {
        items: inst_items.clone(),
    };
    let mut overrides: BTreeMap<(String, String), &crate::project::InstanceOverride> =
        BTreeMap::new();
    for (k, ov) in &p.instances {
        let key = insts
            .resolve(k)
            .map_err(|e| format!("bb.toml [instances.{k:?}]: {e}"))?;
        if overrides.insert(key.clone(), ov).is_some() {
            return Err(format!(
                "bb.toml [instances]: {}:{} is named twice",
                key.0, key.1
            ));
        }
    }
    let mut fields_wanted: Vec<String> = D3_FIELDS.iter().map(|s| s.to_string()).collect();
    fields_wanted.extend(p.extra_fields.iter().cloned());

    // Nets.
    let mut board = Board::default();
    let mut kicad_names: Vec<(String, String, String)> = Vec::new();
    let mut net_of: BTreeMap<(String, String, String), String> = BTreeMap::new();
    for (bid, n) in &lists {
        let be = p.boards.iter().find(|b| &b.id == bid).expect("board");
        if !n.nets.iter().any(|x| x.name == be.ground) {
            return Err(format!(
                "board {bid}: no ground net {:?} (ADR-007 D3; set [[board]] ground = \"<net>\")",
                be.ground
            ));
        }
        board.boards.push(BoardDef {
            id: bid.clone(),
            ground: if multi {
                format!("{bid}:{}", be.ground)
            } else {
                be.ground.clone()
            },
        });
        for net in &n.nets {
            let display = if multi {
                format!("{bid}:{}", net.name)
            } else {
                net.name.clone()
            };
            for (r, pin) in &net.nodes {
                net_of.insert((bid.clone(), r.clone(), pin.clone()), display.clone());
            }
            kicad_names.push((display, bid.clone(), net.name.clone()));
        }
    }
    let sim_ground = p
        .boards
        .iter()
        .find(|b| b.id == p.simulate)
        .map(|b| b.ground.clone())
        .expect("simulate");
    let spice = names::assign_spice(&kicad_names, &p.simulate, &sim_ground, multi);
    for (bid, n) in &lists {
        for net in &n.nets {
            let display = if multi {
                format!("{bid}:{}", net.name)
            } else {
                net.name.clone()
            };
            board.nets.push(Net {
                spice: spice[&display].clone(),
                name: display,
                board: bid.clone(),
                nodes: net
                    .nodes
                    .iter()
                    .map(|(r, pin)| Node {
                        reference: r.clone(),
                        pin: pin.clone(),
                    })
                    .collect(),
                unconnected: net.name.starts_with("unconnected-("),
            });
        }
    }

    // Instances and resolution.
    let patterns: Vec<(String, Vec<Pattern>)> = pk
        .list
        .iter()
        .map(|x| {
            let ps = x
                .part
                .mpn_patterns
                .iter()
                .map(|s| Pattern::new(s).map_err(|e| format!("package {}: {e}", x.part.id)))
                .collect::<Result<Vec<_>, _>>()?;
            Ok((x.part.id.clone(), ps))
        })
        .collect::<Result<_, String>>()?;
    let generic_globs: Vec<String> = GENERIC
        .iter()
        .filter_map(|(id, ..)| pk.get(id).ok())
        .flat_map(|x| x.part.footprints_kicad.iter().cloned())
        .collect();
    let mut used: BTreeMap<String, Package> = BTreeMap::new();
    let mut res_items = Vec::new();
    let mut unresolved_stubs = Vec::new();
    let mut unresolved = Vec::new();
    for (bid, n) in &lists {
        for c in &n.components {
            let q = format!("{bid}:{}", c.reference);
            let mut tried: Vec<Tried> = Vec::new();
            let mut fields: BTreeMap<String, String> = BTreeMap::new();
            for f in &fields_wanted {
                if let Some(v) = c.fields.get(f) {
                    if !v.is_empty() && v != "~" {
                        fields.insert(f.clone(), v.clone());
                    }
                }
            }
            let mut chosen: Option<(String, &'static str, String)> = None; // (id, stage, key)
            let mut status = "unmodelled";
            let mut reason: Option<String> = None;
            let mut value: Option<PassiveValue> = None;
            let mut excluded_by_override = false;
            let mut no_model = false;
            // 1. Project override.
            if let Some(ov) = overrides.get(&(bid.clone(), c.reference.clone())) {
                if let Some(spec) = &ov.package {
                    if spec == "none" {
                        excluded_by_override = true;
                        reason = ov.reason.clone();
                        tried.push(Tried {
                            stage: "override",
                            key: "none".into(),
                            result: "excluded".into(),
                        });
                    } else {
                        let pkg = pk
                            .get(spec)
                            .map_err(|e| format!("bb.toml [instances.{q:?}]: {e}"))?;
                        chosen = Some((pkg.part.id.clone(), "override", spec.clone()));
                        tried.push(Tried {
                            stage: "override",
                            key: spec.clone(),
                            result: pkg.part.id.clone(),
                        });
                    }
                }
            }
            // 2. The bb:part field.
            if chosen.is_none() && !excluded_by_override {
                if let Some(spec) = fields.get("bb:part") {
                    let pkg = pk
                        .get(spec)
                        .map_err(|e| format!("{q}: bb:part = {spec:?}: {e}"))?;
                    chosen = Some((pkg.part.id.clone(), "bb_part", spec.clone()));
                    tried.push(Tried {
                        stage: "bb_part",
                        key: spec.clone(),
                        result: pkg.part.id.clone(),
                    });
                }
            }
            // 3. MPN, exactly (or a package's pattern).
            let mpn_norm = fields.get("MPN").map(|m| normalize_mpn(m));
            let mut candidates: Vec<String> = Vec::new();
            if chosen.is_none() && !excluded_by_override {
                if let Some(m) = mpn_norm.as_deref().filter(|m| !m.is_empty()) {
                    let ids: BTreeSet<String> = pk
                        .list
                        .iter()
                        .zip(&patterns)
                        .filter(|(x, (_, ps))| {
                            x.part.mpn.iter().any(|v| v == m) || ps.iter().any(|p| p.full_match(m))
                        })
                        .map(|(x, _)| x.part.id.clone())
                        .collect();
                    match ids.len() {
                        0 => tried.push(Tried {
                            stage: "mpn",
                            key: m.to_string(),
                            result: "no package".into(),
                        }),
                        1 => {
                            let id = ids.into_iter().next().expect("one");
                            tried.push(Tried {
                                stage: "mpn",
                                key: m.to_string(),
                                result: id.clone(),
                            });
                            chosen = Some((id, "mpn", m.to_string()));
                        }
                        _ => {
                            return Err(format!(
                                "{q}: MPN {m:?} matches several packages ({}); ADR-007 D4 never \
                                 guesses: narrow a package's mpn/mpn_patterns or add an \
                                 [instances.{q:?}] override",
                                ids.into_iter().collect::<Vec<_>>().join(", ")
                            ))
                        }
                    }
                }
            }
            // 4. LCSC.
            if chosen.is_none() && !excluded_by_override {
                if let Some(l) = fields.get("LCSC") {
                    let ids: Vec<String> = pk
                        .list
                        .iter()
                        .filter(|x| x.part.lcsc.iter().any(|v| v == l))
                        .map(|x| x.part.id.clone())
                        .collect();
                    match ids.len() {
                        0 => tried.push(Tried {
                            stage: "lcsc",
                            key: l.clone(),
                            result: "no package".into(),
                        }),
                        1 => {
                            tried.push(Tried {
                                stage: "lcsc",
                                key: l.clone(),
                                result: ids[0].clone(),
                            });
                            chosen = Some((ids[0].clone(), "lcsc", l.clone()));
                        }
                        _ => {
                            return Err(format!(
                                "{q}: LCSC {l} matches several packages ({})",
                                ids.join(", ")
                            ))
                        }
                    }
                }
            }
            // 5. Generic passives by footprint plus value.
            let fp_lib = c.footprint.split(':').next().unwrap_or("").to_string();
            if chosen.is_none() && !excluded_by_override {
                for (gid, kind, prefix) in GENERIC {
                    let Ok(g) = pk.get(gid) else { continue };
                    let by_fp = g
                        .part
                        .footprints_kicad
                        .iter()
                        .any(|pat| glob(pat, &c.footprint));
                    let custom = !generic_globs.iter().any(|pat| glob(pat, &c.footprint))
                        && !NO_MODEL.contains(&fp_lib.as_str());
                    let by_ref = custom && {
                        let mut ch = c.reference.chars();
                        ch.next() == Some(*prefix)
                            && !ch.as_str().is_empty()
                            && ch.as_str().chars().all(|d| d.is_ascii_digit())
                    };
                    if !(by_fp || by_ref) {
                        continue;
                    }
                    let key = if by_fp {
                        fp_lib.clone()
                    } else {
                        format!("ref prefix {prefix}")
                    };
                    match parse_kicad_value(&c.value, *kind) {
                        Ok(v) => {
                            value = Some(PassiveValue {
                                unit: unit_key(*kind).into(),
                                value: v.value,
                                tolerance_ppm: v.tolerance_ppm,
                                rating_uv: v.rating_uv,
                                power_uw: v.power_uw,
                                dielectric: v.dielectric,
                                technology: v.technology,
                            });
                            tried.push(Tried {
                                stage: "generic",
                                key: key.clone(),
                                result: gid.to_string(),
                            });
                            chosen = Some((gid.to_string(), "generic", key));
                        }
                        Err(e) => tried.push(Tried {
                            stage: "generic",
                            key,
                            result: format!("value: {e}"),
                        }),
                    }
                    break;
                }
            }
            // 6. No model needed.
            if chosen.is_none() && !excluded_by_override && NO_MODEL.contains(&fp_lib.as_str()) {
                no_model = true;
                tried.push(Tried {
                    stage: "no_model",
                    key: fp_lib.clone(),
                    result: "no model needed".into(),
                });
            }
            // Candidates for an agent to look at (informative): packages
            // whose MPNs share a prefix with this one. Never chosen.
            if let Some(m) = mpn_norm.as_deref().filter(|m| !m.is_empty()) {
                for x in &pk.list {
                    if chosen.as_ref().map(|c| c.0.as_str()) != Some(x.part.id.as_str())
                        && x.part
                            .mpn
                            .iter()
                            .any(|v| v.starts_with(m) || m.starts_with(v.as_str()))
                    {
                        candidates.push(x.part.id.clone());
                    }
                }
            }
            let (pkg_ref, stage, key) = match &chosen {
                Some((id, stage, key)) => {
                    let pkg = pk.get(id)?.clone();
                    let tree = pk.tree(id)?;
                    let r = PackageRef {
                        id: id.clone(),
                        version: pkg.part.version.clone(),
                        tree,
                    };
                    used.entry(id.clone()).or_insert(pkg);
                    status = "ok";
                    (Some(r), stage.to_string(), Some(key.clone()))
                }
                None if excluded_by_override => {
                    status = "excluded";
                    (None, "override".to_string(), Some("none".into()))
                }
                None if no_model => {
                    status = "no_model";
                    (None, "no_model".to_string(), Some(fp_lib.clone()))
                }
                None => (None, "unresolved".to_string(), None),
            };
            // DNP and exclude_from_sim keep the resolution, with a reason.
            let flags: Vec<String> = c.flags.iter().cloned().collect();
            if status == "ok" || status == "unmodelled" {
                if c.flags.contains("dnp") {
                    status = "excluded";
                    reason = Some("do not populate (dnp)".into());
                } else if c.flags.contains("exclude_from_sim") {
                    status = "excluded";
                    reason = Some("excluded from simulation (exclude_from_sim)".into());
                }
            }
            if status == "unmodelled" {
                unresolved.push(q.clone());
                let mut keys = Vec::new();
                if let Some(m) = &mpn_norm {
                    keys.push(format!("MPN {m}"));
                }
                if let Some(l) = fields.get("LCSC") {
                    keys.push(format!("LCSC {l}"));
                }
                unresolved_stubs.push(format!(
                    "# [instances.{}]   # {} {} ({}; footprint {})\n# package = \"<namespace>/<name>\"   # or: package = \"none\" with reason = \"...\"\n",
                    bbpkg::toml::quote(&q),
                    c.reference,
                    c.value,
                    if keys.is_empty() { "no MPN or LCSC".to_string() } else { keys.join(", ") },
                    if c.footprint.is_empty() { "none" } else { &c.footprint }
                ));
            }
            // Pins: from the libpart, nets from the netlist nodes.
            let lp = n.libparts.get(&(c.lib.clone(), c.part.clone()));
            let mut pins: Vec<InstancePin> = lp
                .map(|v| {
                    v.iter()
                        .map(|lpin| InstancePin {
                            number: lpin.number.clone(),
                            name: lpin.name.clone(),
                            net: net_of
                                .get(&(bid.clone(), c.reference.clone(), lpin.number.clone()))
                                .cloned(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            for ((b2, r2, pin), net) in
                net_of.range((bid.clone(), c.reference.clone(), String::new())..)
            {
                if b2 != bid || r2 != &c.reference {
                    break;
                }
                if !pins.iter().any(|x| &x.number == pin) {
                    pins.push(InstancePin {
                        number: pin.clone(),
                        name: String::new(),
                        net: Some(net.clone()),
                    });
                }
            }
            // ADR-007 D3: the symbol's pin names must equal the package's.
            if let Some((id, ..)) = &chosen {
                let pkg = &used[id];
                if pkg.part.kind != "passive" {
                    for ip in &pins {
                        match pkg.pins.pins.iter().find(|x| x.number == ip.number) {
                            None => {
                                return Err(format!(
                                    "{q}: pin {} ({}) is not a pin of package {id} (ADR-007 D3)",
                                    ip.number, ip.name
                                ))
                            }
                            Some(pp) if !ip.name.is_empty() && pp.name != ip.name => {
                                return Err(format!(
                                    "{q}: pin {} is {:?} in the schematic but {:?} in package {id} \
                                     (ADR-007 D3: the symbol's pin names must equal the package's)",
                                    ip.number, ip.name, pp.name
                                ))
                            }
                            _ => {}
                        }
                    }
                }
            }
            // Test points become probe aliases.
            if fp_lib == "TestPoint" {
                if let Some(net) = pins.iter().find_map(|x| x.net.clone()) {
                    board.probes.push(ProbeAlias {
                        alias: if multi {
                            q.clone()
                        } else {
                            c.reference.clone()
                        },
                        net,
                    });
                }
            }
            res_items.push(Json::Obj({
                let mut o = vec![
                    ("board".to_string(), Json::str(bid)),
                    ("ref".to_string(), Json::str(&c.reference)),
                    ("stage".to_string(), Json::str(&stage)),
                    ("status".to_string(), Json::str(status)),
                    (
                        "tried".to_string(),
                        Json::Arr(
                            tried
                                .iter()
                                .map(|t| {
                                    Json::obj([
                                        ("stage", Json::str(t.stage)),
                                        ("key", Json::str(&t.key)),
                                        ("result", Json::str(&t.result)),
                                    ])
                                })
                                .collect(),
                        ),
                    ),
                ];
                if let Some(m) = &mpn_norm {
                    o.push(("mpn_normalised".into(), Json::str(m)));
                }
                if let Some((id, ..)) = &chosen {
                    o.push(("package".into(), Json::str(id)));
                }
                if !candidates.is_empty() {
                    o.push((
                        "candidates".into(),
                        Json::Arr(candidates.iter().map(Json::str).collect()),
                    ));
                }
                o
            }));
            board.instances.push(Instance {
                board: bid.clone(),
                reference: c.reference.clone(),
                value_text: c.value.clone(),
                value: if chosen.as_ref().is_some_and(|x| x.1 == "generic") {
                    value
                } else {
                    None
                },
                footprint: c.footprint.clone(),
                lib: c.lib.clone(),
                part: c.part.clone(),
                sheet: c.sheet.clone(),
                fields,
                flags,
                package: pkg_ref,
                resolution: Resolution { stage, key },
                status: status.into(),
                reason,
                pins,
            });
        }
    }

    // A module's chip package is part of the resolved set (ADR-006
    // Amendment 3): it supplies the module's behavioural layer and pin
    // models, so the lock pins it (and its platform) like any other package.
    let chips: Vec<String> = used
        .values()
        .filter_map(|p| p.part.module.as_ref().map(|m| m.chip.clone()))
        .filter(|c| !used.contains_key(c))
        .collect();
    for chip in chips {
        let pkg = pk.get(&chip).map_err(|e| format!("a module's chip: {e}"))?;
        used.insert(chip, pkg.clone());
    }

    // Platforms of the resolved packages.
    let mut locked_platforms: Vec<LockedPlatform> = Vec::new();
    for pkg in used.values() {
        if let Some(pid) = pkg
            .part
            .behavioral
            .as_ref()
            .and_then(|b| b.platform.as_deref())
        {
            let def = platforms::find(pid).ok_or_else(|| {
                format!(
                    "package {}: platform {pid} is not one this bb provides ({})",
                    pkg.part.id,
                    platforms::ids().join(", ")
                )
            })?;
            let l = def.locked();
            if !board.platforms.iter().any(|x| x.id == l.id) {
                board.platforms.push(Platform {
                    id: l.id.clone(),
                    content: l.content.clone(),
                });
                locked_platforms.push(l);
            }
        }
    }
    let proj_abs =
        std::fs::canonicalize(&p.dir).map_err(|e| format!("{}: {e}", p.dir.display()))?;
    let mut locked_packages = Vec::new();
    for (id, pkg) in &used {
        let dir =
            std::fs::canonicalize(&pkg.dir).map_err(|e| format!("{}: {e}", pkg.dir.display()))?;
        locked_packages.push(LockedPackage {
            id: id.clone(),
            version: pkg.part.version.clone(),
            path: lock::relative_path(&proj_abs, &dir),
            tree: pk.tree(id)?,
        });
    }

    // Connections (systems): names are checked, the pins resolved at M11.
    for c in &p.connections {
        for side in [&c.a, &c.b] {
            insts
                .resolve(side)
                .map_err(|e| format!("bb.toml [[connection]] {side}: {e}"))?;
        }
        board.connections.push(Connection {
            a: c.a.clone(),
            b: c.b.clone(),
            model: c.model.clone(),
            pins: c.pins.clone(),
        });
    }

    board.sort();

    let mut stage_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut status_counts: BTreeMap<String, usize> = BTreeMap::new();
    for i in &board.instances {
        *stage_counts.entry(i.resolution.stage.clone()).or_default() += 1;
        *status_counts.entry(i.status.clone()).or_default() += 1;
    }
    let resolution = Json::obj([
        ("schema", Json::str("bb-resolution/0")),
        ("instances", Json::Arr(res_items)),
        (
            "stages",
            Json::Obj(
                stage_counts
                    .iter()
                    .map(|(k, v)| (k.clone(), Json::Int(*v as i128)))
                    .collect(),
            ),
        ),
        (
            "statuses",
            Json::Obj(
                status_counts
                    .iter()
                    .map(|(k, v)| (k.clone(), Json::Int(*v as i128)))
                    .collect(),
            ),
        ),
    ]);
    let mut unresolved_toml = String::from(
        "# .bb/unresolved.toml: written by `bb open` on every run (ADR-007 D4); do not edit.\n\
         # One commented stub per unresolved instance. To resolve one, copy its stub into\n\
         # bb.toml (bb open never edits bb.toml) and name a package, or package = \"none\".\n",
    );
    if unresolved_stubs.is_empty() {
        unresolved_toml.push_str("# (none)\n");
    }
    for s in &unresolved_stubs {
        unresolved_toml.push('\n');
        unresolved_toml.push_str(s);
    }
    let import_json = Json::obj([
        ("schema", Json::str("bb-import/0")),
        ("boards", Json::Arr(import_boards)),
        ("bb", Json::str(env!("CARGO_PKG_VERSION"))),
    ]);
    Ok(Imported {
        board,
        packages: used,
        locked_packages,
        locked_platforms,
        resolution,
        unresolved_toml,
        import_json,
        exported,
        warnings,
        stage_counts,
        status_counts,
        unresolved,
    })
}

/// `import.json` with the host (not canonical; never diffed).
pub fn import_json_with_host(j: &Json) -> String {
    let host = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|h| h.trim().to_string())
        .unwrap_or_default();
    let mut j = j.clone();
    if let Json::Obj(o) = &mut j {
        o.push(("host".into(), Json::str(host)));
    }
    j.to_canonical()
}

/// The project directory of a `bb open`/`bb run` argument: a directory with
/// `bb.toml`, or a `bb.toml` file.
pub fn project_dir(arg: Option<&Path>) -> Result<PathBuf, String> {
    let p = arg
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    if p.is_dir() {
        if p.join(crate::project::PROJECT_FILE).is_file() {
            return Ok(p);
        }
        return Err(format!("{}: no bb.toml", p.display()));
    }
    if p.file_name().and_then(|n| n.to_str()) == Some(crate::project::PROJECT_FILE) {
        return Ok(p
            .parent()
            .map(|d| {
                if d.as_os_str().is_empty() {
                    Path::new(".")
                } else {
                    d
                }
            })
            .unwrap_or(Path::new("."))
            .to_path_buf());
    }
    Err(format!(
        "{}: not a project directory or bb.toml",
        p.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mpn_normalisation_and_globs() {
        assert_eq!(
            normalize_mpn(" TC2030-IDC-NL (footprint only) "),
            "TC2030-IDC-NL"
        );
        assert_eq!(
            normalize_mpn("SC-20S 32.768kHz 20PPM 9pF"),
            "SC-20S 32.768kHz 20PPM 9pF"
        );
        assert_eq!(normalize_mpn("BAV99,215"), "BAV99,215");
        assert_eq!(normalize_mpn("(only)"), "(only)");
        assert!(glob("Resistor_SMD:*", "Resistor_SMD:R_0402_1005Metric"));
        assert!(!glob("Resistor_SMD:*", "Resistor_THT:R_Axial"));
        assert!(glob("*", ""));
    }
}
