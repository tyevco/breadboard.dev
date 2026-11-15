//! B-expression lint (ADR-015 D3, `spec/package.md` section 11): every
//! function call in a B, E or G source expression and in a `.func` body is
//! checked against ngspice 42's own function table and arity.
//!
//! ngspice 42 does not check arity itself, and some wrong calls crash the
//! process: a three-argument `min()` in a B source segfaults libngspice 42
//! (`PTdifferentiate` and `PTeval`, `src/spicelib/parser/inpptree.c`, read
//! the second argument of a two-argument comma tree), with no message. Inside
//! braces, numparam (`src/frontend/numparam/xpressn.c`) evaluates the
//! expression instead, and a call with the wrong arity is silently wrong
//! there (`min(a, b, c)` gives `min(a, c)`). Both are refused here.
//!
//! The tables are taken from the installed ngspice 42 sources, not guessed:
//!
//! - **B expressions** (`inpptree.c`, `funcs[]` and `PT_mkfnode`): one
//!   argument for every function but `pow`, `pwr`, `min` and `max` (two:
//!   `PTeval` reads `left->left` and `left->right` of a comma),
//!   `ternary_fcn` (three, `PT_mkfnode`), and `pwl` and `pwl_derivative`
//!   (the input, then at least one `x, y` pair: `prepare_PTF_PWL`). `v(...)`
//!   takes one or two nodes, `i(...)` one source.
//! - **numparam** (`xpressn.c`, `fmathS`): `ternary_fcn`, `agauss` and
//!   `gauss` three, `pow`, `pwr`, `min`, `max`, `unif`, `aunif` and `limit`
//!   two, every other one.
//!
//! The same tokeniser ([`calls`]) serves ADR-011 D11's name collisions.

use std::collections::BTreeMap;

/// Where an expression is evaluated.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Ctx {
    /// ngspice's B-source parser (`inpptree.c`).
    B,
    /// numparam, inside `{...}` (`xpressn.c`).
    Numparam,
}

/// The arity rule of a function.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Arity {
    Exactly(usize),
    /// `pwl(x, x1, y1, ...)`: odd and at least three.
    Pwl,
    /// `v(n)` or `v(n1, n2)`.
    OneOrTwo,
}

/// ngspice 42's B-expression function table (`inpptree.c` `funcs[]`, plus
/// `ternary_fcn` from `PT_mkfnode` and the node and branch references).
const B_FUNCS: &[(&str, Arity)] = &[
    ("abs", Arity::Exactly(1)),
    ("acos", Arity::Exactly(1)),
    ("acosh", Arity::Exactly(1)),
    ("asin", Arity::Exactly(1)),
    ("asinh", Arity::Exactly(1)),
    ("atan", Arity::Exactly(1)),
    ("atanh", Arity::Exactly(1)),
    ("cos", Arity::Exactly(1)),
    ("cosh", Arity::Exactly(1)),
    ("exp", Arity::Exactly(1)),
    ("ln", Arity::Exactly(1)),
    ("log", Arity::Exactly(1)),
    ("log10", Arity::Exactly(1)),
    ("sgn", Arity::Exactly(1)),
    ("sin", Arity::Exactly(1)),
    ("sinh", Arity::Exactly(1)),
    ("sqrt", Arity::Exactly(1)),
    ("tan", Arity::Exactly(1)),
    ("tanh", Arity::Exactly(1)),
    ("u", Arity::Exactly(1)),
    ("uramp", Arity::Exactly(1)),
    ("ceil", Arity::Exactly(1)),
    ("floor", Arity::Exactly(1)),
    ("nint", Arity::Exactly(1)),
    ("u2", Arity::Exactly(1)),
    ("pwl", Arity::Pwl),
    ("pwl_derivative", Arity::Pwl),
    ("eq0", Arity::Exactly(1)),
    ("ne0", Arity::Exactly(1)),
    ("gt0", Arity::Exactly(1)),
    ("lt0", Arity::Exactly(1)),
    ("ge0", Arity::Exactly(1)),
    ("le0", Arity::Exactly(1)),
    ("pow", Arity::Exactly(2)),
    ("pwr", Arity::Exactly(2)),
    ("min", Arity::Exactly(2)),
    ("max", Arity::Exactly(2)),
    ("ddt", Arity::Exactly(1)),
    ("ternary_fcn", Arity::Exactly(3)),
    ("v", Arity::OneOrTwo),
    ("i", Arity::Exactly(1)),
];

/// numparam's function table (`xpressn.c` `fmathS`).
const NUMPARAM_FUNCS: &[(&str, Arity)] = &[
    ("sqr", Arity::Exactly(1)),
    ("sqrt", Arity::Exactly(1)),
    ("sin", Arity::Exactly(1)),
    ("cos", Arity::Exactly(1)),
    ("exp", Arity::Exactly(1)),
    ("ln", Arity::Exactly(1)),
    ("arctan", Arity::Exactly(1)),
    ("abs", Arity::Exactly(1)),
    ("pow", Arity::Exactly(2)),
    ("pwr", Arity::Exactly(2)),
    ("max", Arity::Exactly(2)),
    ("min", Arity::Exactly(2)),
    ("int", Arity::Exactly(1)),
    ("log", Arity::Exactly(1)),
    ("log10", Arity::Exactly(1)),
    ("sinh", Arity::Exactly(1)),
    ("cosh", Arity::Exactly(1)),
    ("tanh", Arity::Exactly(1)),
    ("ternary_fcn", Arity::Exactly(3)),
    ("agauss", Arity::Exactly(3)),
    ("sgn", Arity::Exactly(1)),
    ("gauss", Arity::Exactly(3)),
    ("unif", Arity::Exactly(2)),
    ("aunif", Arity::Exactly(2)),
    ("limit", Arity::Exactly(2)),
    ("ceil", Arity::Exactly(1)),
    ("floor", Arity::Exactly(1)),
    ("asin", Arity::Exactly(1)),
    ("acos", Arity::Exactly(1)),
    ("atan", Arity::Exactly(1)),
    ("asinh", Arity::Exactly(1)),
    ("acosh", Arity::Exactly(1)),
    ("atanh", Arity::Exactly(1)),
    ("tan", Arity::Exactly(1)),
    ("nint", Arity::Exactly(1)),
];

/// One function call found by [`calls`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    /// Lower case.
    pub name: String,
    /// Number of arguments (`f()` has none).
    pub args: usize,
    pub ctx: Ctx,
}

/// Every function call of `expr`, evaluated in `ctx`, in source order
/// (nested calls after their caller). `{...}` switches to numparam. The
/// arguments of `v(...)` and `i(...)` in a B expression are node and source
/// names, not expressions.
pub fn calls(expr: &str, ctx: Ctx) -> Result<Vec<Call>, String> {
    let mut out = Vec::new();
    scan(expr.as_bytes(), ctx, &mut out)?;
    Ok(out)
}

/// The index of the bracket closing the one at `open` (`(` or `{`).
fn closing(s: &[u8], open: usize) -> Result<usize, String> {
    let mut depth = 0i32;
    for (k, &c) in s.iter().enumerate().skip(open) {
        match c {
            b'(' | b'{' => depth += 1,
            b')' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(k);
                }
            }
            _ => {}
        }
    }
    Err(format!(
        "unbalanced bracket in {:?}",
        String::from_utf8_lossy(s)
    ))
}

/// Split `s` at its top-level commas.
fn split_args(s: &[u8]) -> Vec<&[u8]> {
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0i32, 0usize);
    for (k, &c) in s.iter().enumerate() {
        match c {
            b'(' | b'{' => depth += 1,
            b')' | b'}' => depth -= 1,
            b',' if depth == 0 => {
                parts.push(&s[start..k]);
                start = k + 1;
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

fn scan(s: &[u8], ctx: Ctx, out: &mut Vec<Call>) -> Result<(), String> {
    let mut k = 0;
    while k < s.len() {
        let c = s[k];
        if c == b'{' {
            let e = closing(s, k)?;
            scan(&s[k + 1..e], Ctx::Numparam, out)?;
            k = e + 1;
        } else if c == b'}' || (c == b')') {
            return Err(format!(
                "unbalanced bracket in {:?}",
                String::from_utf8_lossy(s)
            ));
        } else if c.is_ascii_digit() || (c == b'.' && s.get(k + 1).is_some_and(u8::is_ascii_digit))
        {
            // A number: digits and dots, an exponent, a scale suffix.
            while k < s.len() && (s[k].is_ascii_digit() || s[k] == b'.') {
                k += 1;
            }
            if k < s.len() && (s[k] == b'e' || s[k] == b'E') {
                let mut j = k + 1;
                if j < s.len() && (s[j] == b'+' || s[j] == b'-') {
                    j += 1;
                }
                if j < s.len() && s[j].is_ascii_digit() {
                    k = j;
                    while k < s.len() && s[k].is_ascii_digit() {
                        k += 1;
                    }
                }
            }
            while k < s.len() && (s[k].is_ascii_alphanumeric() || s[k] == b'_') {
                k += 1;
            }
        } else if c.is_ascii_alphabetic() || c == b'_' {
            let start = k;
            while k < s.len() && (s[k].is_ascii_alphanumeric() || s[k] == b'_') {
                k += 1;
            }
            let name = String::from_utf8_lossy(&s[start..k]).to_ascii_lowercase();
            let mut j = k;
            while j < s.len() && s[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < s.len() && s[j] == b'(' {
                let e = closing(s, j)?;
                let inner = &s[j + 1..e];
                let parts = split_args(inner);
                let args = if parts.len() == 1 && parts[0].iter().all(u8::is_ascii_whitespace) {
                    0
                } else {
                    parts.len()
                };
                out.push(Call {
                    name: name.clone(),
                    args,
                    ctx,
                });
                let refs = ctx == Ctx::B && (name == "v" || name == "i");
                if !refs {
                    for p in parts {
                        scan(p, ctx, out)?;
                    }
                }
                k = e + 1;
            }
        } else if c == b'(' {
            let e = closing(s, k)?;
            scan(&s[k + 1..e], ctx, out)?;
            k = e + 1;
        } else {
            k += 1;
        }
    }
    Ok(())
}

fn arity_ok(a: Arity, n: usize) -> bool {
    match a {
        Arity::Exactly(m) => n == m,
        Arity::Pwl => n >= 3 && n % 2 == 1,
        Arity::OneOrTwo => n == 1 || n == 2,
    }
}

fn arity_text(a: Arity) -> String {
    match a {
        Arity::Exactly(1) => "exactly 1 argument".into(),
        Arity::Exactly(m) => format!("exactly {m} arguments"),
        Arity::Pwl => "an input and at least one x, y pair (an odd number, >= 3)".into(),
        Arity::OneOrTwo => "1 or 2 nodes".into(),
    }
}

/// Check the calls of `expr` (in `ctx`) against the tables and the user
/// functions `funcs` (name, arity; a `.func` serves both contexts).
pub fn check_expr(
    expr: &str,
    ctx: Ctx,
    funcs: &BTreeMap<String, usize>,
) -> Result<(), Vec<String>> {
    let cs = calls(expr, ctx).map_err(|e| vec![e])?;
    let mut errs = Vec::new();
    for c in cs {
        let table = match c.ctx {
            Ctx::B => B_FUNCS,
            Ctx::Numparam => NUMPARAM_FUNCS,
        };
        let where_ = match c.ctx {
            Ctx::B => "ngspice 42's B-expression parser",
            Ctx::Numparam => "ngspice 42's numparam (inside braces)",
        };
        if let Some(&n) = funcs.get(&c.name) {
            if c.args != n {
                errs.push(format!(
                    "{}() is a .func of {n} argument(s), called with {}",
                    c.name, c.args
                ));
            }
            continue;
        }
        match table.iter().find(|(n, _)| *n == c.name) {
            None => errs.push(format!(
                "unknown function {}() for {where_} (and no .func of that name)",
                c.name
            )),
            Some(&(_, a)) if !arity_ok(a, c.args) => {
                let crash = c.ctx == Ctx::B && matches!(a, Arity::Exactly(2)) && c.args > 2;
                errs.push(format!(
                    "{}() takes {} in {where_}, not {}{}",
                    c.name,
                    arity_text(a),
                    c.args,
                    if crash {
                        " (libngspice 42 segfaults on it)"
                    } else {
                        ""
                    }
                ))
            }
            Some(_) => {}
        }
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs)
    }
}

/// Logical lines (`+` continuations joined with a space) with their first
/// line number, comments removed: `*` lines, and inline `;` and `$ `
/// comments.
fn logical_lines(text: &str) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let t = raw.trim_end_matches('\r').trim_start();
        if t.starts_with('*') {
            continue;
        }
        let mut body = t;
        if let Some(j) = body.find(';') {
            body = &body[..j];
        }
        if let Some(j) = body.find("$ ") {
            body = &body[..j];
        }
        if let Some(rest) = body.strip_prefix('+') {
            if let Some(last) = out.last_mut() {
                last.1.push(' ');
                last.1.push_str(rest);
                continue;
            }
        }
        out.push((i + 1, body.to_string()));
    }
    out
}

/// The expression of an E or G element: after `value=`, `vol=` or `cur=`
/// (any case, spaces around `=`), without its outer `{}` or `''`; `None` for
/// a linear, `poly` or `table` source.
fn eg_expression(line: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    for kw in ["value", "vol", "cur"] {
        let mut from = 0;
        while let Some(p) = lower[from..].find(kw) {
            let at = from + p;
            from = at + kw.len();
            let before_ok = at == 0 || lower.as_bytes()[at - 1].is_ascii_whitespace();
            let rest = lower[at + kw.len()..].trim_start();
            if before_ok && rest.starts_with('=') {
                let off = line.len() - rest.len() + 1;
                return Some(strip_delims(line[off..].trim()));
            }
        }
    }
    None
}

/// `{expr}` or `'expr'` as written around a whole E/G expression: the
/// inside (a B-type expression).
fn strip_delims(e: &str) -> String {
    let b = e.as_bytes();
    if b.len() >= 2
        && ((b[0] == b'{' && b[b.len() - 1] == b'}') || (b[0] == b'\'' && b[b.len() - 1] == b'\''))
    {
        if b[0] == b'{' && closing(b, 0).ok() != Some(b.len() - 1) {
            return e.to_string();
        }
        return e[1..e.len() - 1].to_string();
    }
    e.to_string()
}

/// `l` after its first `n` whitespace-separated tokens, trimmed at the
/// start.
fn after_tokens(l: &str, n: usize) -> &str {
    let mut rest = l.trim_start();
    for _ in 0..n {
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        rest = rest[end..].trim_start();
    }
    rest
}

/// A `.func` line: its name, parameters and body.
fn func_def(line: &str) -> Option<(String, usize, String)> {
    let rest = line.trim_start()[".func".len()..].trim_start();
    let open = rest.find('(')?;
    let name = rest[..open].trim().to_ascii_lowercase();
    let close = closing(rest.as_bytes(), open).ok()?;
    let params = rest[open + 1..close].trim();
    let n = if params.is_empty() {
        0
    } else {
        params.split(',').count()
    };
    let body = rest[close + 1..].trim_start();
    let body = body.strip_prefix('=').unwrap_or(body).trim();
    Some((name, n, strip_delims(body)))
}

/// Lint every B, E and G expression and every `.func` body of a SPICE text
/// (a view file, a test circuit or a whole deck). One message per problem,
/// `line <n>: <element>: <problem>`, in line order.
pub fn check(text: &str) -> Vec<String> {
    let lines = logical_lines(text);
    let mut funcs: BTreeMap<String, usize> = BTreeMap::new();
    for (_, l) in &lines {
        if l.to_ascii_lowercase().starts_with(".func") {
            if let Some((name, n, _)) = func_def(l) {
                funcs.insert(name, n);
            }
        }
    }
    let mut errs = Vec::new();
    for (no, l) in &lines {
        let Some(first) = l.split_whitespace().next() else {
            continue;
        };
        let lower = first.to_ascii_lowercase();
        let (what, expr) = if lower == ".func" {
            match func_def(l) {
                Some((name, _, body)) => (format!(".func {name}"), Some(body)),
                None => {
                    errs.push(format!(
                        "line {no}: .func: expected .func name(params) body"
                    ));
                    continue;
                }
            }
        } else if lower.starts_with('b') {
            // B<name> n+ n- V=<expr> | I=<expr>
            let tail = after_tokens(l, 3);
            let t = tail
                .strip_prefix(['v', 'V', 'i', 'I'])
                .unwrap_or(tail)
                .trim_start();
            if tail.len() == t.len() || !t.starts_with('=') {
                errs.push(format!(
                    "line {no}: {first}: a B source needs V=<expr> or I=<expr> after its two nodes"
                ));
                continue;
            }
            (first.to_string(), Some(t[1..].to_string()))
        } else if lower.starts_with('e') || lower.starts_with('g') {
            (first.to_string(), eg_expression(l))
        } else {
            continue;
        };
        let Some(expr) = expr else {
            continue;
        };
        if let Err(es) = check_expr(&expr, Ctx::B, &funcs) {
            for e in es {
                errs.push(format!("line {no}: {what}: {e}"));
            }
        }
    }
    errs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_argument_min_is_refused() {
        // The libngspice 42 segfault (ADR-015 D3; measured: exit 139).
        let e = check("B1 b 0 I = min(V(a), 2, 3)*1m\n");
        assert_eq!(e.len(), 1, "{e:?}");
        assert!(e[0].contains("min() takes exactly 2 arguments"), "{e:?}");
        assert!(e[0].contains("segfaults"), "{e:?}");
        assert!(e[0].starts_with("line 1: B1:"), "{e:?}");
        // Two arguments, nested, in a subckt and a continuation: fine.
        assert!(check(
            ".subckt X a b\nB1 a b I = min(1, max(0, (V(a)-V(b))/0.01))\n+ * 2\n.ends\n"
        )
        .is_empty());
    }

    #[test]
    fn unknown_functions_and_wrong_arities() {
        let e = check("B1 a 0 V = atan2(V(a), 1)\n");
        assert!(e[0].contains("unknown function atan2()"), "{e:?}");
        let e = check("B1 a 0 V = tanh(V(a), 1)\n");
        assert!(e[0].contains("tanh() takes exactly 1 argument"), "{e:?}");
        let e = check("B1 a 0 V = V(a, b, c)\n");
        assert!(e[0].contains("v() takes 1 or 2 nodes"), "{e:?}");
        // pwl: an input and pairs.
        assert!(check("B1 a 0 V = pwl(V(x), 0,1, 2,3)\n").is_empty());
        assert!(!check("B1 a 0 V = pwl(V(x), 0,1, 2)\n").is_empty());
        // Inside braces, numparam's table: a 3-argument min is silently
        // wrong there, and refused.
        let e = check("B1 a 0 I = {min(ra, rb, rc)}*V(a)\n");
        assert!(e[0].contains("numparam"), "{e:?}");
        assert!(check("B1 a 0 I = {limit(1, 2)}*V(a)\n").is_empty());
        // `limit` is not a B function.
        assert!(!check("B1 a 0 I = limit(1, 2)\n").is_empty());
        // A malformed B line.
        assert!(check("B1 a 0 5\n")[0].contains("needs V=<expr> or I=<expr>"));
    }

    #[test]
    fn e_g_sources_and_funcs() {
        assert!(check("E1 out 0 in 0 2\nG1 out 0 in 0 1m\n").is_empty());
        let e = check("E1 out 0 value={max(V(a), 1, 2)}\nG1 out 0 cur='pow(V(a))'\n");
        assert_eq!(e.len(), 2, "{e:?}");
        assert!(e[0].starts_with("line 1: E1: max()"), "{e:?}");
        assert!(e[1].starts_with("line 2: G1: pow()"), "{e:?}");
        // A .func: its body is checked, and calls must match its arity.
        let t = ".func soft(x, w) {w*ln(1 + exp(x/w))}\nB1 a 0 V = soft(V(a), 0.05)\n";
        assert!(check(t).is_empty(), "{:?}", check(t));
        let e = check(".func soft(x, w) {w*ln(1 + exp(x/w))}\nB1 a 0 V = soft(V(a))\n");
        assert!(
            e[0].contains(".func of 2 argument(s), called with 1"),
            "{e:?}"
        );
        let e = check(".func bad(x) = {min(x, 1, 2)}\n");
        assert!(e[0].starts_with("line 1: .func bad: min()"), "{e:?}");
        // Comments are not expressions.
        assert!(check("* B1 a 0 V = min(1,2,3)\nR1 a 0 1k ; B min(1,2,3)\n").is_empty());
    }

    /// G3: every committed view, test circuit and board file passes.
    #[test]
    fn every_committed_spice_file_passes() {
        fn walk(d: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(rd) = std::fs::read_dir(d) else { return };
            let mut es: Vec<_> = rd.flatten().map(|e| e.path()).collect();
            es.sort();
            for p in es {
                if p.is_dir() {
                    walk(&p, out);
                } else if matches!(
                    p.extension().and_then(|e| e.to_str()),
                    Some("lib" | "cir" | "sub" | "mod")
                ) {
                    out.push(p);
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut files = Vec::new();
        for d in ["packages", "examples"] {
            walk(&root.join(d), &mut files);
        }
        assert!(files.len() > 40, "{} files", files.len());
        let mut bad = Vec::new();
        let mut exprs = 0;
        for f in &files {
            let t = std::fs::read_to_string(f).unwrap();
            exprs += t
                .lines()
                .filter(|l| l.trim_start().starts_with(['B', 'b']))
                .count();
            for e in check(&t) {
                bad.push(format!("{}: {e}", f.display()));
            }
        }
        assert!(exprs > 30, "{exprs} B lines: the scan found the views");
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[test]
    fn calls_are_tokenised_with_numbers_and_nodes() {
        let c = calls(
            "0.5*(1 + tanh((V(IN1,GNDP) - vth_k*V(VCC,GNDP))/0.01)) + 1e-3 + 2.5meg",
            Ctx::B,
        )
        .unwrap();
        let names: Vec<(&str, usize)> = c.iter().map(|c| (c.name.as_str(), c.args)).collect();
        assert_eq!(names, vec![("tanh", 1), ("v", 2), ("v", 2)]);
        assert!(calls("min(1, (2)", Ctx::B).is_err());
    }
}
