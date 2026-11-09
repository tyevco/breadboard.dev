//! The SPICE text of analog views: top-level definitions, the `gnd` rule,
//! and assembly of several views into one deck (ADR-006 D3, D4).
//!
//! A view file defines things; it does not instantiate them. At the top
//! level it may hold only comments, `.subckt ... .ends` blocks, `.model`,
//! `.func` and `.param` lines (with `+` continuations). Each top-level name
//! is collected per (kind, lower-case name): SPICE names are
//! case-insensitive. Byte-identical definitions of one name are emitted
//! once; different definitions are an error naming both views (gate G7).
//! Prefix renaming is deferred to the registry ADR (ADR-006 D3).

use std::collections::BTreeMap;

/// A top-level definition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Def {
    /// `subckt`, `model`, `func` or `param`.
    pub kind: &'static str,
    /// Lower case.
    pub name: String,
    /// The definition's text (its logical lines, continuations kept, each
    /// ending in `\n`; comments inside a `.subckt` kept as written).
    pub text: String,
    /// For a `.subckt`: its port names, in order.
    pub ports: Vec<String>,
    /// For a `.subckt`: the element lines' node-like tokens (for the `gnd`
    /// rule), lower case.
    pub tokens: Vec<String>,
}

/// Physical lines grouped into logical lines (a line starting with `+`
/// continues the previous one). Returns `(first line number, text)` where
/// text keeps every physical line with its `\n`.
fn logical_lines(text: &str) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let l = raw.trim_end_matches('\r');
        if l.trim_start().starts_with('+') {
            if let Some(last) = out.last_mut() {
                last.1.push_str(l);
                last.1.push('\n');
                continue;
            }
        }
        out.push((i + 1, format!("{l}\n")));
    }
    out
}

/// The tokens of a logical line without comments: `*` lines are comments;
/// `;` and `$ ` start an inline comment; `+` continuation marks dropped.
fn tokens(line: &str) -> Vec<String> {
    let mut toks = Vec::new();
    for (k, phys) in line.lines().enumerate() {
        let t = phys.trim_start();
        if t.starts_with('*') {
            continue;
        }
        let mut t = if k > 0 { t.trim_start_matches('+') } else { t };
        if let Some(i) = t.find(';') {
            t = &t[..i];
        }
        if let Some(i) = t.find("$ ") {
            t = &t[..i];
        }
        for w in t.split(|c: char| c.is_whitespace() || c == '(' || c == ')' || c == ',') {
            if !w.is_empty() {
                toks.push(w.to_string());
            }
        }
    }
    toks
}

/// Parse a view file into its top-level definitions.
pub fn definitions(text: &str) -> Result<Vec<Def>, String> {
    let mut defs = Vec::new();
    let mut open: Option<(usize, Def)> = None; // .subckt being read
    let mut depth = 0usize;
    for (lineno, line) in logical_lines(text) {
        let toks = tokens(&line);
        let Some(first) = toks.first() else {
            if let Some((_, d)) = open.as_mut() {
                d.text.push_str(&line);
            }
            continue;
        };
        let kw = first.to_ascii_lowercase();
        if let Some((_, d)) = open.as_mut() {
            d.text.push_str(&line);
            match kw.as_str() {
                ".subckt" => depth += 1,
                ".ends" => {
                    if depth == 0 {
                        let (_, d) = open.take().expect("open");
                        defs.push(d);
                    } else {
                        depth -= 1;
                    }
                }
                k if k.starts_with('.') => {}
                _ => d
                    .tokens
                    .extend(toks.iter().skip(1).map(|t| t.to_ascii_lowercase())),
            }
            continue;
        }
        match kw.as_str() {
            ".subckt" => {
                let name = toks
                    .get(1)
                    .ok_or_else(|| format!("line {lineno}: .subckt without a name"))?;
                let ports = toks[2..]
                    .iter()
                    .take_while(|t| !t.contains('=') && !t.eq_ignore_ascii_case("params:"))
                    .cloned()
                    .collect();
                open = Some((
                    lineno,
                    Def {
                        kind: "subckt",
                        name: name.to_ascii_lowercase(),
                        text: line.clone(),
                        ports,
                        tokens: Vec::new(),
                    },
                ));
                depth = 0;
            }
            ".model" | ".func" => {
                let name = toks
                    .get(1)
                    .ok_or_else(|| format!("line {lineno}: {kw} without a name"))?;
                defs.push(Def {
                    kind: if kw == ".model" { "model" } else { "func" },
                    name: name.to_ascii_lowercase(),
                    text: line.clone(),
                    ports: Vec::new(),
                    tokens: Vec::new(),
                });
            }
            ".param" => {
                // `.param a=1 b=2`: each name is a definition of the line.
                let mut body = line.trim_start()[".param".len()..].to_string();
                while body.contains(" =") || body.contains("= ") {
                    body = body.replace(" =", "=").replace("= ", "=");
                }
                let names: Vec<String> = body
                    .split_whitespace()
                    .filter_map(|w| w.split_once('=').map(|(n, _)| n.trim().to_ascii_lowercase()))
                    .filter(|n| !n.is_empty())
                    .collect();
                if names.is_empty() {
                    return Err(format!("line {lineno}: .param without name=value"));
                }
                for n in names {
                    defs.push(Def {
                        kind: "param",
                        name: n,
                        text: line.clone(),
                        ports: Vec::new(),
                        tokens: Vec::new(),
                    });
                }
            }
            ".ends" => return Err(format!("line {lineno}: .ends without .subckt")),
            ".end" => {}
            k if k.starts_with('.') => {
                return Err(format!(
                    "line {lineno}: {first} at the top level of a view file (only .subckt, .model, .func and .param; a view is self-contained, so no .include or .lib)"
                ))
            }
            _ => {
                return Err(format!(
                    "line {lineno}: element {first:?} at the top level: a view file defines, it does not instantiate"
                ))
            }
        }
    }
    if let Some((l, d)) = open {
        return Err(format!("line {l}: .subckt {} has no .ends", d.name));
    }
    Ok(defs)
}

/// The `gnd` rule (ADR-006 D4, `spec/pin-model.md` glue rule 7): ngspice
/// treats a node named `gnd` as global ground even inside a subcircuit, so
/// no subckt port or internal node may be called that. Element lines are
/// checked token by token (conservatively: any token `gnd`).
pub fn gnd_violations(defs: &[Def]) -> Vec<String> {
    let mut out = Vec::new();
    for d in defs.iter().filter(|d| d.kind == "subckt") {
        if d.ports.iter().any(|p| p.eq_ignore_ascii_case("gnd")) {
            out.push(format!(
                ".subckt {}: port named gnd (ngspice global ground)",
                d.name
            ));
        }
        if d.tokens.iter().any(|t| t == "gnd") {
            out.push(format!(
                ".subckt {}: a node named gnd inside it (ngspice global ground)",
                d.name
            ));
        }
    }
    out
}

/// Collect the definitions of several views (`(label, text)`), dedupe
/// byte-identical ones and refuse different definitions of one name.
/// Returns the definitions in sorted (kind, name) order.
pub fn collect(views: &[(String, String)]) -> Result<Vec<(Def, String)>, String> {
    let mut seen: BTreeMap<(&'static str, String), (Def, String)> = BTreeMap::new();
    for (label, text) in views {
        let defs = definitions(text).map_err(|e| format!("{label}: {e}"))?;
        for d in defs {
            let key = (d.kind, d.name.clone());
            match seen.get(&key) {
                None => {
                    seen.insert(key, (d, label.clone()));
                }
                Some((prev, _)) if prev.text == d.text => {}
                Some((_, prev_label)) => {
                    return Err(format!(
                        "{} {} is defined differently by {prev_label} and {label} (ADR-006 D3: identical definitions are emitted once, different ones are an error)",
                        d.kind, d.name
                    ));
                }
            }
        }
    }
    Ok(seen.into_values().collect())
}

/// Assemble views into deck text: every unique definition once, in sorted
/// (kind, name) order, `.param` lines first only by that order. A `.param`
/// line that defines several names is emitted once.
pub fn assemble(views: &[(String, String)]) -> Result<String, String> {
    let defs = collect(views)?;
    let mut out = String::new();
    let mut emitted_params: Vec<&str> = Vec::new();
    for (d, _) in &defs {
        if d.kind == "param" {
            if emitted_params.contains(&d.text.as_str()) {
                continue;
            }
            emitted_params.push(&d.text);
        }
        out.push_str(&d.text);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str =
        "* view a\n.subckt RES 1 2 value=1\nR1 1 2 {value}\n.ends\n.model D1 D(IS=1e-14)\n";
    const B_SAME: &str = "* another header\n.model D1 D(IS=1e-14)\n.subckt cap 1 2 value=1\n+ \nC1 1 2 {value}\n.ends cap\n";
    const B_DIFF: &str = ".model d1 D(IS=2e-14)\n";

    #[test]
    fn definitions_and_ports() {
        let d = definitions(A).unwrap();
        assert_eq!(d.len(), 2);
        assert_eq!((d[0].kind, d[0].name.as_str()), ("subckt", "res"));
        assert_eq!(d[0].ports, vec!["1", "2"]);
        assert_eq!((d[1].kind, d[1].name.as_str()), ("model", "d1"));
        assert!(definitions("R1 a b 1k\n").is_err());
        assert!(definitions(".include x.lib\n").is_err());
        assert!(definitions(".subckt X a\nR1 a 0 1\n").is_err());
        let p = definitions(".param a=1 b = 2\n").unwrap();
        assert_eq!(
            p.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
            vec!["a", "b"]
        );
    }

    #[test]
    fn identical_definitions_once_different_refused() {
        // G7: the shared .model is emitted once; the deck holds res, cap, d1.
        let deck = assemble(&[("a".into(), A.into()), ("b".into(), B_SAME.into())]).unwrap();
        assert_eq!(deck.matches(".model D1").count(), 1);
        assert!(deck.contains(".subckt RES") && deck.contains(".subckt cap"));
        let e = assemble(&[
            ("view a".into(), A.into()),
            ("view b".into(), B_DIFF.into()),
        ])
        .unwrap_err();
        assert!(
            e.contains("model d1") && e.contains("view a") && e.contains("view b"),
            "{e}"
        );
        // Sorted order, independent of the order of the views.
        let d2 = assemble(&[("b".into(), B_SAME.into()), ("a".into(), A.into())]).unwrap();
        assert_eq!(deck, d2);
    }

    #[test]
    fn gnd_rule() {
        let d = definitions(".subckt LSW CT GND ON\nR1 CT ON 1\n.ends\n").unwrap();
        assert_eq!(gnd_violations(&d).len(), 1);
        let d = definitions(".subckt LSW CT GNDP ON\nR1 CT gnd 1 ; comment gnd\n.ends\n").unwrap();
        assert_eq!(gnd_violations(&d).len(), 1);
        let d =
            definitions(".subckt LSW CT GNDP ON\n* gnd in a comment\nR1 CT GNDP 1 ; gnd\n.ends\n")
                .unwrap();
        assert!(gnd_violations(&d).is_empty());
    }
}
