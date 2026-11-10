//! The KiCad netlist (`kicad-cli sch export netlist --format kicadsexpr`,
//! format "E") and its version gates (ADR-007 D2, D3).
//!
//! Only D3's content is read: per component its reference, value,
//! footprint, `libsource`, sheet path names, the fields (by name) and the
//! flags; per net its name and `(ref, pin number)` nodes; per `libpart` its
//! pins (number, name, type). Everything else (dates, `units` blocks, the
//! datasheet, net classes, `pinfunction`, token order, line endings) is not
//! read, so it cannot reach `board.bbm`.

use std::collections::{BTreeMap, BTreeSet};

use crate::sexp::{self, Sexp};

/// The netlist format this importer reads.
pub const FORMAT: &str = "E";
/// Eeschema majors that are supported (tested).
pub const SUPPORTED_MAJORS: &[u32] = &[9, 10];
/// Older majors accepted with an "untested" warning.
pub const UNTESTED_MAJORS: &[u32] = &[6, 7, 8];

/// One component.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Component {
    pub reference: String,
    pub value: String,
    pub footprint: String,
    pub lib: String,
    pub part: String,
    /// `(sheetpath (names ...))`.
    pub sheet: String,
    /// Every `(fields (field (name N) V))` with a value.
    pub fields: BTreeMap<String, String>,
    /// The flags set by `(property (name "dnp"))` and the like.
    pub flags: BTreeSet<String>,
}

/// A pin of a library part.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibPin {
    pub number: String,
    /// KiCad's `~` (no name) is read as empty.
    pub name: String,
    /// KiCad's electrical type (`passive`, `power_in`, ...): lint only.
    pub kind: String,
}

/// A net and its `(ref, pin number)` nodes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Net {
    pub name: String,
    pub nodes: Vec<(String, String)>,
}

/// A parsed netlist.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Netlist {
    /// `(design (tool ...))`, e.g. `Eeschema 10.0.8-10.0.8~ubuntu24.04.1`.
    pub tool: String,
    pub tool_major: u32,
    /// Sorted by reference.
    pub components: Vec<Component>,
    /// Sorted by name; nodes sorted.
    pub nets: Vec<Net>,
    /// `(lib, part)` -> pins sorted by number.
    pub libparts: BTreeMap<(String, String), Vec<LibPin>>,
    pub warnings: Vec<String>,
}

/// The flags read from component properties (ADR-007 D3).
pub const FLAGS: &[&str] = &["dnp", "exclude_from_bom", "exclude_from_sim"];

/// The Eeschema major of a `(tool ...)` string: `Eeschema 10.0.8-...`,
/// `Eeschema (6.0.11)`, `Eeschema 9.0.9`.
pub fn tool_major(tool: &str) -> Option<u32> {
    let rest = tool.strip_prefix("Eeschema")?.trim_start();
    let rest = rest.strip_prefix('(').unwrap_or(rest);
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() || !rest[digits.len()..].starts_with('.') {
        return None;
    }
    digits.parse().ok()
}

/// The version gate on a netlist's tool string (ADR-007 D2). `Ok(Some(w))`
/// carries a warning for an untested major.
pub fn check_tool(tool: &str, file: &str) -> Result<(u32, Option<String>), String> {
    let major = tool_major(tool).ok_or_else(|| {
        format!("{file}: netlist tool {tool:?} is not an Eeschema version this bb can read")
    })?;
    if SUPPORTED_MAJORS.contains(&major) {
        Ok((major, None))
    } else if UNTESTED_MAJORS.contains(&major) {
        Ok((
            major,
            Some(format!(
                "{file}: netlist from {tool:?}: KiCad {major} is untested (ADR-007 D2: 9 and 10 are supported)"
            )),
        ))
    } else if major > *SUPPORTED_MAJORS.last().expect("non-empty") {
        Err(format!(
            "{file}: netlist from {tool:?}: KiCad {major} is newer than this bb supports \
             (ADR-007 D2: KiCad 9 and 10); each newer KiCad needs its own ADR amendment"
        ))
    } else {
        Err(format!(
            "{file}: netlist from {tool:?}: KiCad {major} is older than this bb reads (6-10)"
        ))
    }
}

fn no_tilde(s: &str) -> String {
    if s == "~" {
        String::new()
    } else {
        s.to_string()
    }
}

/// Parse a netlist file's text. `file` names it in errors.
pub fn parse(text: &str, file: &str) -> Result<Netlist, String> {
    let root = sexp::parse(text).map_err(|e| format!("{file}: {e}"))?;
    if root.head() != Some("export") {
        return Err(format!(
            "{file}: not a KiCad netlist (no (export ...)); export one with \
             `kicad-cli sch export netlist --format kicadsexpr`"
        ));
    }
    match root.value_of("version") {
        Some(FORMAT) => {}
        v => {
            return Err(format!(
                "{file}: netlist format {}: this bb reads format \"{FORMAT}\" (ADR-007 D2)",
                v.unwrap_or("(none)")
            ))
        }
    }
    let tool = root
        .child("design")
        .and_then(|d| d.value_of("tool"))
        .ok_or_else(|| format!("{file}: no (design (tool ...)): cannot check the KiCad version"))?
        .to_string();
    let (tool_major, w) = check_tool(&tool, file)?;
    let mut warnings: Vec<String> = w.into_iter().collect();

    let mut components = Vec::new();
    if let Some(comps) = root.child("components") {
        for c in comps.children("comp") {
            components.push(component(c, file)?);
        }
    }
    components.sort_by(|a: &Component, b| a.reference.cmp(&b.reference));
    for w in components.windows(2) {
        if w[0].reference == w[1].reference {
            return Err(format!(
                "{file}: reference {} appears twice (annotate the schematic)",
                w[0].reference
            ));
        }
    }

    let mut libparts: BTreeMap<(String, String), Vec<LibPin>> = BTreeMap::new();
    if let Some(lps) = root.child("libparts") {
        for lp in lps.children("libpart") {
            let lib = lp.value_of("lib").unwrap_or_default().to_string();
            let part = lp.value_of("part").unwrap_or_default().to_string();
            let mut pins = Vec::new();
            if let Some(ps) = lp.child("pins") {
                for p in ps.children("pin") {
                    let number = p
                        .value_of("num")
                        .ok_or_else(|| format!("{file}: libpart {lib}:{part}: a pin without num"))?
                        .to_string();
                    stacked(&number, &format!("libpart {lib}:{part}"), file)?;
                    pins.push(LibPin {
                        number,
                        name: no_tilde(p.value_of("name").unwrap_or_default()),
                        kind: p.value_of("type").unwrap_or_default().to_string(),
                    });
                }
            }
            pins.sort_by(|a, b| a.number.cmp(&b.number));
            libparts.insert((lib, part), pins);
        }
    }

    let mut nets = Vec::new();
    if let Some(ns) = root.child("nets") {
        for n in ns.children("net") {
            let name = n
                .value_of("name")
                .ok_or_else(|| format!("{file}: a net without a name"))?
                .to_string();
            let mut nodes = Vec::new();
            for node in n.children("node") {
                let r = node
                    .value_of("ref")
                    .ok_or_else(|| format!("{file}: net {name}: a node without ref"))?;
                let p = node
                    .value_of("pin")
                    .ok_or_else(|| format!("{file}: net {name}: a node without pin"))?;
                stacked(p, &format!("net {name:?}, {r}"), file)?;
                nodes.push((r.to_string(), p.to_string()));
            }
            nodes.sort();
            nodes.dedup();
            nets.push(Net { name, nodes });
        }
    }
    nets.sort_by(|a, b| a.name.cmp(&b.name));
    for w in nets.windows(2) {
        if w[0].name == w[1].name {
            return Err(format!("{file}: net {:?} appears twice", w[0].name));
        }
    }
    for c in &components {
        if !libparts.contains_key(&(c.lib.clone(), c.part.clone())) {
            warnings.push(format!(
                "{file}: {}: no libpart {}:{} in the netlist; its pin names are unknown",
                c.reference, c.lib, c.part
            ));
        }
    }
    Ok(Netlist {
        tool,
        tool_major,
        components,
        nets,
        libparts,
        warnings,
    })
}

/// Stacked pins (`[1-4]`, KiCad 10) are refused in Phase 1 (ADR-007 D2).
fn stacked(number: &str, at: &str, file: &str) -> Result<(), String> {
    if number.contains('[') {
        return Err(format!(
            "{file}: {at}: pin number {number:?} is a stacked pin, which Phase 1 refuses \
             (ADR-007 D2: its meaning needs a decision first)"
        ));
    }
    Ok(())
}

fn component(c: &Sexp, file: &str) -> Result<Component, String> {
    let reference = c
        .value_of("ref")
        .ok_or_else(|| format!("{file}: a component without ref"))?
        .to_string();
    let mut fields = BTreeMap::new();
    if let Some(fs) = c.child("fields") {
        for f in fs.children("field") {
            let Some(name) = f.value_of("name") else {
                continue;
            };
            // (field (name "MPN") "value"): the value is the first
            // non-list argument.
            if let Some(v) = f.args().iter().find_map(|a| match a {
                Sexp::Str(s) | Sexp::Atom(s) => Some(s),
                _ => None,
            }) {
                fields.insert(name.to_string(), v.clone());
            }
        }
    }
    let mut flags = BTreeSet::new();
    for p in c.children("property") {
        if let Some(n) = p.value_of("name") {
            if FLAGS.contains(&n) {
                // Present means set; an explicit "no"/"false" value clears it.
                let off = matches!(p.value_of("value"), Some("no" | "false" | "0"));
                if !off {
                    flags.insert(n.to_string());
                }
            }
        }
    }
    let (lib, part) = match c.child("libsource") {
        Some(ls) => (
            ls.value_of("lib").unwrap_or_default().to_string(),
            ls.value_of("part").unwrap_or_default().to_string(),
        ),
        None => (String::new(), String::new()),
    };
    Ok(Component {
        value: c.value_of("value").unwrap_or_default().to_string(),
        footprint: c.value_of("footprint").unwrap_or_default().to_string(),
        lib,
        part,
        sheet: c
            .child("sheetpath")
            .and_then(|s| s.value_of("names"))
            .unwrap_or("/")
            .to_string(),
        fields,
        flags,
        reference,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NET: &str = r#"(export (version "E")
  (design (source "/x/a.kicad_sch") (date "2026-11-09T07:16:18") (tool "Eeschema 10.0.8-10.0.8~ubuntu24.04.1"))
  (components
    (comp (ref "R1") (value "10k") (footprint "Resistor_SMD:R_0402_1005Metric")
      (fields (field (name "MPN") "RC0402") (field (name "Datasheet")))
      (libsource (lib "Device") (part "R") (description "Resistor"))
      (property (name "dnp"))
      (sheetpath (names "/") (tstamps "/")))
    (comp (ref "C1") (value "100n") (footprint "Capacitor_SMD:C_0402")
      (libsource (lib "Device") (part "C"))
      (sheetpath (names "/Sub/") (tstamps "/x/"))))
  (libparts
    (libpart (lib "Device") (part "R") (pins (pin (num "1") (name "~") (type "passive")) (pin (num "2") (name "~") (type "passive"))))
    (libpart (lib "Device") (part "C") (pins (pin (num "2") (name "~") (type "passive")) (pin (num "1") (name "~") (type "passive")))))
  (nets
    (net (code "2") (name "VIN") (class "Default") (node (ref "R1") (pin "1") (pinfunction "1_1") (pintype "passive")))
    (net (code "1") (name "GND") (class "Default") (node (ref "R1") (pin "2")) (node (ref "C1") (pin "2")) (node (ref "C1") (pin "1")))))"#;

    #[test]
    fn reads_d3_content() {
        let n = parse(NET, "t.net").unwrap();
        assert_eq!(n.tool_major, 10);
        assert_eq!(
            n.components
                .iter()
                .map(|c| c.reference.as_str())
                .collect::<Vec<_>>(),
            vec!["C1", "R1"]
        );
        let r1 = &n.components[1];
        assert_eq!(r1.fields.get("MPN").map(String::as_str), Some("RC0402"));
        assert!(!r1.fields.contains_key("Datasheet"));
        assert!(r1.flags.contains("dnp"));
        assert_eq!(n.components[0].sheet, "/Sub/");
        assert_eq!(n.nets[0].name, "GND");
        assert_eq!(
            n.nets[0].nodes,
            vec![
                ("C1".to_string(), "1".to_string()),
                ("C1".to_string(), "2".to_string()),
                ("R1".to_string(), "2".to_string())
            ]
        );
        assert_eq!(n.libparts[&("Device".into(), "C".into())][0].number, "1");
        assert_eq!(n.libparts[&("Device".into(), "R".into())][0].name, "");
    }

    #[test]
    fn version_gates() {
        // ADR-007 gate M4e: KiCad 11 refused; 9 and 10 accepted; 6-8 warn.
        let k11 = NET.replace("Eeschema 10.0.8-10.0.8~ubuntu24.04.1", "Eeschema 11.0.0");
        let e = parse(&k11, "t.net").unwrap_err();
        assert!(e.contains("KiCad 11") && e.contains("newer"), "{e}");
        assert!(parse(
            &NET.replace("Eeschema 10.0.8-10.0.8~ubuntu24.04.1", "Eeschema 9.0.9"),
            "t"
        )
        .is_ok());
        let k8 = parse(
            &NET.replace("Eeschema 10.0.8-10.0.8~ubuntu24.04.1", "Eeschema (8.0.1)"),
            "t",
        )
        .unwrap();
        assert!(k8.warnings[0].contains("untested"));
        assert!(parse(
            &NET.replace("Eeschema 10.0.8-10.0.8~ubuntu24.04.1", "Eeschema 5.1.9"),
            "t"
        )
        .is_err());
        assert!(parse(&NET.replace("(version \"E\")", "(version \"D\")"), "t").is_err());
        assert!(parse(
            &NET.replace("Eeschema 10.0.8-10.0.8~ubuntu24.04.1", "pcbnew 10.0.8"),
            "t"
        )
        .is_err());
        // Stacked pins are refused.
        let st = NET.replace("(pin \"1\") (pinfunction", "(pin \"[1-4]\") (pinfunction");
        assert!(parse(&st, "t").unwrap_err().contains("stacked"));
        assert_eq!(tool_major("Eeschema 10.0.8-10.0.8~ubuntu24.04.1"), Some(10));
        assert_eq!(tool_major("Eeschema (6.0.11)"), Some(6));
        assert_eq!(tool_major("Eeschema"), None);
    }

    #[test]
    fn line_endings_and_dates_do_not_matter() {
        let a = parse(NET, "a").unwrap();
        let b = parse(
            &NET.replace('\n', "\r\n")
                .replace("2026-11-09T07:16:18", "2030-01-01T00:00:00"),
            "b",
        )
        .unwrap();
        assert_eq!(a, b);
    }
}
