//! ADR-009 D2's rules for a view that declares the `pi` rung (milestone P1,
//! `spec/package.md` section 11), checked on the view file's element lines.
//!
//! - **Rule 1, no branch-current unknown.** A `pi` view's currents are
//!   functions of node voltages: B I-type sources, R, C, G (a VCCS), D, and
//!   transistor primitives. A V source (a 0 V ammeter too), an E (VCVS), an
//!   F (CCCS), an H (CCVS), a V-type B source, a W (current-controlled
//!   switch, which reads a V source's current) and transmission lines (T, O,
//!   U) each add a branch-current unknown, and an XSPICE code model (A) is
//!   refused because its currents cannot be checked here. An internal signal
//!   is an I-type B source into a 1 Ohm conductance (ADR-009 D2). There is no
//!   exemption for "signal nodes": lint cannot tell one from a current loop.
//! - **Rule 4, the function table at its arity**, is ADR-015 D3's check
//!   ([`crate::bexpr`]), applied to every view already.
//! - **Rule 5, an inductor** (L, and a K coupling, which needs one) is a
//!   warning: ngspice gives it a branch current, and the prototype measured a
//!   30 nH series inductor at +68 % cost.
//!
//! Rule 2 (a DC conductance at every operating point) is a property of the
//! solved circuit, not of the text: `bb pkg test` solves the operating point
//! of every vector at every corner of the view (ADR-015 D4's fail-fast).

/// A finding of the `pi` rules.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PiFinding {
    /// `true`: an error (rule 1); `false`: a warning (rule 5).
    pub error: bool,
    pub msg: String,
}

/// The logical lines of a view file (continuations joined), without
/// comment lines and inline comments.
fn logical_lines(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in text.lines() {
        let mut l = raw.trim_end_matches('\r').trim_start();
        if l.starts_with('*') || l.is_empty() {
            continue;
        }
        if let Some(i) = l.find(';') {
            l = &l[..i];
        }
        if let Some(i) = l.find("$ ") {
            l = &l[..i];
        }
        if let Some(rest) = l.strip_prefix('+') {
            if let Some(last) = out.last_mut() {
                last.push(' ');
                last.push_str(rest.trim());
                continue;
            }
        }
        out.push(l.trim().to_string());
    }
    out
}

/// Is a B source line `B<name> <n+> <n-> V = ...` (a V-type source)? The
/// type is the first non-node token: `V=`, `V =`, `v=...`.
fn b_type(line: &str) -> Option<char> {
    let mut it = line.split_whitespace();
    it.next()?; // name
    it.next()?; // n+
    it.next()?; // n-
    let rest: String = it.collect::<Vec<_>>().join(" ");
    let mut ch = rest.chars();
    let c = ch.next()?.to_ascii_lowercase();
    let after = ch.as_str().trim_start();
    if (c == 'v' || c == 'i') && after.starts_with('=') {
        Some(c)
    } else {
        None
    }
}

/// Check a `pi` view file's text against ADR-009 D2 rules 1 and 5.
pub fn check(text: &str) -> Vec<PiFinding> {
    let mut out = Vec::new();
    let err = |m: String| PiFinding {
        error: true,
        msg: m,
    };
    for l in logical_lines(text) {
        if l.starts_with('.') {
            continue;
        }
        let name = l.split_whitespace().next().unwrap_or("");
        let Some(k) = name.chars().next().map(|c| c.to_ascii_lowercase()) else {
            continue;
        };
        match k {
            'v' => out.push(err(format!(
                "{name}: a voltage source is a branch-current unknown (ADR-009 D2 rule 1; write an \
                 internal signal as an I-type B source into a 1 Ohm conductance)"
            ))),
            'e' | 'h' => out.push(err(format!(
                "{name}: a {} is a branch-current unknown (ADR-009 D2 rule 1)",
                if k == 'e' { "VCVS" } else { "CCVS" }
            ))),
            'f' | 'w' => out.push(err(format!(
                "{name}: a {} reads a voltage source's current, a branch-current unknown (ADR-009 \
                 D2 rule 1)",
                if k == 'f' {
                    "CCCS"
                } else {
                    "current-controlled switch"
                }
            ))),
            't' | 'o' | 'u' => out.push(err(format!(
                "{name}: a transmission line is a branch-current unknown (ADR-009 D2 rule 1)"
            ))),
            'a' => out.push(err(format!(
                "{name}: an XSPICE code model (ADR-009 D2 rule 1: pass elements are B conductances \
                 whose currents lint can read)"
            ))),
            'b' => match b_type(&l) {
                Some('i') => {}
                Some(_) => out.push(err(format!(
                    "{name}: a V-type B source is a branch-current unknown (ADR-009 D2 rule 1; \
                     write it as an I-type B source into a 1 Ohm conductance)"
                ))),
                None => out.push(err(format!(
                    "{name}: a B source whose type (I = or V =) cannot be read"
                ))),
            },
            'l' | 'k' => out.push(PiFinding {
                error: false,
                msg: format!(
                    "{name}: an inductor{} adds a branch current; a series L with milliohms cost \
                     +68 % in the prototype (ADR-009 D2 rule 5)",
                    if k == 'k' { " coupling" } else { "" }
                ),
            }),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_1_and_5() {
        let ok = ".subckt X A B params: g=1\n* V1 a comment\nB_EN B s I = 0.5 + 0.5*tanh(V(A))\n\
                  + * 2\nR_EN s B 1\nC1 A B 1n\nG1 A B s B {g}\nD1 A B dd\n.model dd d\n.ends X\n";
        assert!(check(ok).is_empty(), "{:?}", check(ok));
        let bad = ".subckt X A B\nV1 A n1 0\nE1 n2 0 A B 1\nF1 A B V1 1\nH1 A B V1 1\n\
                   B1 n3 0 V = min(V(A), 1)\nB2 n3 0 v=1\nA1 A B m\nW1 A B V1 sw\nT1 A 0 B 0 z0=50\n\
                   L1 A B 1u\n.ends\n";
        let f = check(bad);
        assert_eq!(f.iter().filter(|x| x.error).count(), 9, "{f:?}");
        assert_eq!(f.iter().filter(|x| !x.error).count(), 1, "{f:?}");
        assert!(f.iter().any(|x| x.msg.starts_with("B2: a V-type")));
        // Spaces around the type: `I =`, `i=`, `I= `.
        assert!(check("B1 a b I = 1\nB2 a b i=1\nB3 a b I= 1\n").is_empty());
    }
}
