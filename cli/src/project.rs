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
        Registry::current().check("bb-project", &schema)?;
        let mut name = None;
        let mut simulate = None;
        if let Some(mut p) = o.table_opt("project")? {
            name = p.str_opt("name")?;
            simulate = p.str_opt("simulate")?;
            p.done()?;
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
            for k in [
                "voh", "vol", "rise", "fall", "zout", "vih", "vil", "cin", "vref", "bits", "load",
            ] {
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
        o.done()?;
        Ok(Project {
            dir: dir.to_path_buf(),
            name,
            simulate,
            boards,
            package_paths,
            extra_fields,
            instances,
            targets,
            binds,
            probes,
            connections,
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
    /// pA.
    pub amps_pa: i128,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pwl {
    pub net: String,
    /// `(fs, µV)`.
    pub points: Vec<(i128, i128)>,
    pub rs_uohm: i128,
}

/// A bench (`bb-bench/0`).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Bench {
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
}

fn q(text: &str, dim: bbpkg::value::Dim, at: &str) -> Result<i128, String> {
    bbpkg::value::parse_quantity(text, dim).map_err(|e| format!("{at}: {e}"))
}

impl Bench {
    pub fn read(path: &Path) -> Result<Bench, String> {
        let text =
            std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        Bench::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The bench used when `bb run` has no `--bench`: no sources, every
    /// net in scope, the default option set.
    pub fn empty() -> Bench {
        Bench {
            options: bbpkg::optsets::DEFAULT.into(),
            ..Bench::default()
        }
    }

    pub fn parse(text: &str) -> Result<Bench, String> {
        use bbpkg::value::Dim;
        let t = toml::parse(text).map_err(|e| e.to_string())?;
        let mut o = Obj::new("", &t);
        let schema = o.str_req("schema")?;
        Registry::current().check("bb-bench", &schema)?;
        let mut b = Bench {
            text: text.to_string(),
            options: bbpkg::optsets::DEFAULT.into(),
            ..Bench::default()
        };
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
            let amps_pa = q(&l.str_req("amps")?, Dim::Current, &format!("{at}.amps"))?;
            l.done()?;
            b.loads.push(Load { net, amps_pa });
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
        o.done()?;
        Ok(b)
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
        assert!(Bench::parse("schema = \"bb-bench/0\"\n[options]\nset = \"loose\"\n").is_err());
    }
}
