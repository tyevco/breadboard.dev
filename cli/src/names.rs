//! Names (ADR-007 D6, D7; `spec/board-bbm.md` section 4 and
//! `spec/project.md` section 4): SPICE node names for nets, and the
//! resolution of instance, pin and net names given in `bb.toml`, a bench or
//! on the command line.

use std::collections::{BTreeMap, BTreeSet};

/// The SPICE-safe form of a KiCad net name (before system prefixes and
/// collision suffixes): every character other than `[A-Za-z0-9_]` becomes
/// `_`, runs of `_` collapse to one, a leading `_` is dropped, a leading
/// digit gets an `n` prefix, and a name equal to `gnd` (any case) gets `_n`
/// appended. An empty result is `n`.
pub fn spice_base(name: &str, board_prefix: Option<&str>) -> String {
    let mut s = String::new();
    let mut last_us = false;
    for c in name.chars() {
        let c = if c.is_ascii_alphanumeric() || c == '_' {
            c
        } else {
            '_'
        };
        if c == '_' {
            if last_us {
                continue;
            }
            last_us = true;
        } else {
            last_us = false;
        }
        s.push(c);
    }
    let s = s.trim_start_matches('_').to_string();
    let mut s = match board_prefix {
        Some(b) => format!("{b}_{s}"),
        None => s,
    };
    if s.is_empty() {
        s = "n".into();
    }
    if s.starts_with(|c: char| c.is_ascii_digit()) {
        s = format!("n{s}");
    }
    if s.eq_ignore_ascii_case("gnd") {
        s.push_str("_n");
    }
    s
}

/// SPICE names for a set of nets: `(display name, board, KiCad name)` in
/// any order. The simulated board's ground is `0`; names that collide
/// case-insensitively get `_2`, `_3`, ... in display-name order (the first
/// keeps its name). Returns display name -> SPICE name.
pub fn assign_spice(
    nets: &[(String, String, String)],
    simulated: &str,
    ground: &str,
    system: bool,
) -> BTreeMap<String, String> {
    let mut sorted: Vec<&(String, String, String)> = nets.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut used: BTreeSet<String> = BTreeSet::new();
    used.insert("0".into());
    let mut out = BTreeMap::new();
    for (display, board, kicad) in sorted {
        if board == simulated && kicad == ground {
            out.insert(display.clone(), "0".to_string());
            continue;
        }
        let base = spice_base(kicad, system.then_some(board.as_str()));
        let mut name = base.clone();
        let mut k = 2;
        while used.contains(&name.to_ascii_lowercase()) {
            name = format!("{base}_{k}");
            k += 1;
        }
        used.insert(name.to_ascii_lowercase());
        out.insert(display.clone(), name);
    }
    out
}

/// A SPICE-safe element-name fragment (glue names: `U10.P0.06` ->
/// `U10_P0_06`). Unlike [`spice_base`] it keeps leading underscores and
/// never prefixes, so a Phase 0 name (`U1`, `PA5`) is unchanged.
pub fn element_fragment(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// What instance names resolve against: `(board, ref, value)`.
pub struct Instances<'a> {
    pub items: Vec<(&'a str, &'a str, &'a str)>,
}

impl Instances<'_> {
    /// `<board>:<ref>` always; a bare `<ref>` only when exactly one board
    /// has it. Returns `(board, ref)`.
    pub fn resolve(&self, name: &str) -> Result<(String, String), String> {
        if let Some((b, r)) = name.split_once(':') {
            return self
                .items
                .iter()
                .find(|(ib, ir, _)| *ib == b && *ir == r)
                .map(|(ib, ir, _)| (ib.to_string(), ir.to_string()))
                .ok_or_else(|| format!("{name}: no instance {r} on board {b}"));
        }
        let c: Vec<_> = self.items.iter().filter(|(_, r, _)| *r == name).collect();
        match c.len() {
            1 => Ok((c[0].0.to_string(), c[0].1.to_string())),
            0 => Err(format!("{name}: no such instance")),
            _ => Err(format!(
                "{name} is ambiguous: {} (qualify it as <board>:{name}, ADR-007 D7)",
                c.iter()
                    .map(|(b, r, v)| format!("{b}:{r} ({v})"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    /// A pin `<instance>.<pin name>` (`main:U10.P0.06`, `U1.PA4`): the
    /// instance part follows [`Instances::resolve`]. Returns `(board, ref,
    /// pin name)`.
    pub fn resolve_pin(&self, name: &str) -> Result<(String, String, String), String> {
        let (board_part, rest) = match name.split_once(':') {
            Some((b, r)) => (Some(b), r),
            None => (None, name),
        };
        let (r, pin) = rest
            .split_once('.')
            .filter(|(r, p)| !r.is_empty() && !p.is_empty())
            .ok_or_else(|| format!("{name}: a pin is <instance>.<pin name> (U10.P0.06)"))?;
        let inst = match board_part {
            Some(b) => format!("{b}:{r}"),
            None => r.to_string(),
        };
        let (b, r) = self.resolve(&inst)?;
        Ok((b, r, pin.to_string()))
    }
}

/// What net names resolve against: `(display name, board, KiCad name)`.
pub struct Nets<'a> {
    pub items: Vec<(&'a str, &'a str, &'a str)>,
    pub simulated: &'a str,
}

fn leaf(kicad: &str) -> &str {
    kicad.rsplit('/').next().unwrap_or(kicad)
}

impl Nets<'_> {
    /// Tried in order, the first unique match wins: the exact display
    /// name; the board-qualified name `<board>:<KiCad name>`; a unique leaf
    /// name (after the last `/`) within the simulated board, then the
    /// project. Ambiguity is an error that lists the candidates.
    pub fn resolve(&self, name: &str) -> Result<String, String> {
        if let Some(d) = self.items.iter().find(|(d, ..)| *d == name) {
            return Ok(d.0.to_string());
        }
        if let Some((b, k)) = name.split_once(':') {
            if let Some(d) = self.items.iter().find(|(_, ib, ik)| *ib == b && *ik == k) {
                return Ok(d.0.to_string());
            }
        }
        let pick = |c: Vec<&(&str, &str, &str)>| -> Result<Option<String>, String> {
            match c.len() {
                0 => Ok(None),
                1 => Ok(Some(c[0].0.to_string())),
                _ => Err(format!(
                    "net {name} is ambiguous: {} (use the full name, ADR-007 D7)",
                    c.iter()
                        .map(|(d, ..)| format!("{d:?}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            }
        };
        let sim: Vec<_> = self
            .items
            .iter()
            .filter(|(_, b, k)| *b == self.simulated && leaf(k) == name)
            .collect();
        if let Some(d) = pick(sim)? {
            return Ok(d);
        }
        let all: Vec<_> = self
            .items
            .iter()
            .filter(|(.., k)| leaf(k) == name)
            .collect();
        if let Some(d) = pick(all)? {
            return Ok(d);
        }
        Err(format!("no net {name}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spice_names() {
        assert_eq!(
            spice_base("/LED ring/LED_DIN_R", None),
            "LED_ring_LED_DIN_R"
        );
        assert_eq!(
            spice_base("/MCU, flash & debug/XL1", None),
            "MCU_flash_debug_XL1"
        );
        assert_eq!(spice_base("+3V3", None), "n3V3");
        assert_eq!(spice_base("GND", Some("base")), "base_GND");
        assert_eq!(spice_base("gnd", None), "gnd_n");
        assert_eq!(spice_base("Gnd", None), "Gnd_n");
        assert_eq!(
            spice_base("LED_A", None),
            "LED_A",
            "Phase 0 names unchanged"
        );
        assert_eq!(
            spice_base("unconnected-(U10-P0.29-Pad10)", None),
            "unconnected_U10_P0_29_Pad10_"
        );
        let nets = vec![
            ("GND".to_string(), "main".to_string(), "GND".to_string()),
            ("+3V3".to_string(), "main".to_string(), "+3V3".to_string()),
            ("3V3".to_string(), "main".to_string(), "3V3".to_string()),
            ("n3v3".to_string(), "main".to_string(), "n3v3".to_string()),
        ];
        let m = assign_spice(&nets, "main", "GND", false);
        assert_eq!(m["GND"], "0");
        // Display order: "+3V3" < "3V3" < "n3v3".
        assert_eq!(
            (m["+3V3"].as_str(), m["3V3"].as_str(), m["n3v3"].as_str()),
            ("n3V3", "n3V3_2", "n3v3_3")
        );
        assert_eq!(element_fragment("U10.P0.06"), "U10_P0_06");
    }

    #[test]
    fn qualified_names() {
        // ADR-007 gate M4b (synthetic two-board system).
        let inst = Instances {
            items: vec![
                ("a", "U1", "RT9080-33GJ5"),
                ("b", "U1", "BQ25180YBGR"),
                ("a", "U2", "TPS22917"),
            ],
        };
        let e = inst.resolve("U1").unwrap_err();
        assert!(
            e.contains("a:U1 (RT9080-33GJ5)") && e.contains("b:U1 (BQ25180YBGR)"),
            "{e}"
        );
        assert_eq!(inst.resolve("a:U1").unwrap(), ("a".into(), "U1".into()));
        assert_eq!(inst.resolve("U2").unwrap(), ("a".into(), "U2".into()));
        assert!(inst
            .resolve_pin("U1.P0.06")
            .unwrap_err()
            .contains("ambiguous"));
        assert_eq!(
            inst.resolve_pin("b:U1.P0.06").unwrap(),
            ("b".into(), "U1".into(), "P0.06".into())
        );
        let nets = Nets {
            items: vec![
                ("a:VSYS", "a", "VSYS"),
                ("b:VSYS", "b", "VSYS"),
                ("a:/LED ring/LED_DIN", "a", "/LED ring/LED_DIN"),
                ("b:/x/Y", "b", "/x/Y"),
                ("b:/z/Y", "b", "/z/Y"),
            ],
            simulated: "a",
        };
        assert_eq!(
            nets.resolve("VSYS").unwrap(),
            "a:VSYS",
            "within the simulated board"
        );
        assert_eq!(nets.resolve("b:VSYS").unwrap(), "b:VSYS");
        assert_eq!(nets.resolve("LED_DIN").unwrap(), "a:/LED ring/LED_DIN");
        assert!(nets.resolve("Y").unwrap_err().contains("ambiguous"));
        assert!(nets.resolve("nope").is_err());
    }
}
