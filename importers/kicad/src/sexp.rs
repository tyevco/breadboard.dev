//! An S-expression reader for KiCad files (ADR-007 D1; AGENTS.md: never
//! regex). KiCad's files are lists of atoms and double-quoted strings; a
//! string escapes `"` and `\` with a backslash (and writes `\n` for a line
//! break). Nothing else is needed for `.net`, `.kicad_sch` or `.kicad_pro`
//! headers.

use std::fmt;

/// One S-expression.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sexp {
    List(Vec<Sexp>),
    /// A bare token (`export`, `20250114`, `yes`).
    Atom(String),
    /// A double-quoted string, unescaped.
    Str(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub msg: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.msg)
    }
}

impl std::error::Error for ParseError {}

impl Sexp {
    /// The head atom of a list (`(net ...)` -> `net`).
    pub fn head(&self) -> Option<&str> {
        match self {
            Sexp::List(v) => match v.first() {
                Some(Sexp::Atom(a)) => Some(a),
                _ => None,
            },
            _ => None,
        }
    }
    /// The items after the head.
    pub fn args(&self) -> &[Sexp] {
        match self {
            Sexp::List(v) if !v.is_empty() => &v[1..],
            _ => &[],
        }
    }
    /// The text of an atom or a string.
    pub fn text(&self) -> Option<&str> {
        match self {
            Sexp::Atom(a) | Sexp::Str(a) => Some(a),
            Sexp::List(_) => None,
        }
    }
    /// The first child list with head `name`.
    pub fn child(&self, name: &str) -> Option<&Sexp> {
        self.args().iter().find(|c| c.head() == Some(name))
    }
    /// Every child list with head `name`.
    pub fn children<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Sexp> + 'a {
        self.args().iter().filter(move |c| c.head() == Some(name))
    }
    /// `(name "value")` -> `value` (the first argument of child `name`).
    pub fn value_of(&self, name: &str) -> Option<&str> {
        self.child(name)?.args().first()?.text()
    }
    /// Whether any list anywhere below (or this one) has head `name`.
    pub fn contains_head(&self, name: &str) -> bool {
        if self.head() == Some(name) {
            return true;
        }
        match self {
            Sexp::List(v) => v.iter().any(|c| c.contains_head(name)),
            _ => false,
        }
    }
}

/// Parse one top-level S-expression (KiCad files hold exactly one).
pub fn parse(text: &str) -> Result<Sexp, ParseError> {
    let mut p = P {
        b: text.as_bytes(),
        i: 0,
        line: 1,
    };
    p.ws();
    if p.peek() != Some(b'(') {
        return Err(p.err("expected '(' at the start of the file"));
    }
    let v = p.list(0)?;
    p.ws();
    if p.i != p.b.len() {
        return Err(p.err("trailing data after the top-level list"));
    }
    Ok(v)
}

struct P<'a> {
    b: &'a [u8],
    i: usize,
    line: usize,
}

const MAX_DEPTH: usize = 512;

impl P<'_> {
    fn err(&self, msg: &str) -> ParseError {
        ParseError {
            line: self.line,
            msg: msg.to_string(),
        }
    }
    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }
    fn ws(&mut self) {
        while let Some(c) = self.peek() {
            match c {
                b'\n' => {
                    self.line += 1;
                    self.i += 1;
                }
                b' ' | b'\t' | b'\r' => self.i += 1,
                _ => break,
            }
        }
    }
    fn list(&mut self, depth: usize) -> Result<Sexp, ParseError> {
        if depth > MAX_DEPTH {
            return Err(self.err("nesting too deep"));
        }
        self.i += 1; // '('
        let mut items = Vec::new();
        loop {
            self.ws();
            match self.peek() {
                None => return Err(self.err("unterminated list")),
                Some(b')') => {
                    self.i += 1;
                    return Ok(Sexp::List(items));
                }
                Some(b'(') => items.push(self.list(depth + 1)?),
                Some(b'"') => items.push(self.string()?),
                Some(_) => items.push(self.atom()?),
            }
        }
    }
    fn atom(&mut self) -> Result<Sexp, ParseError> {
        let start = self.i;
        while let Some(c) = self.peek() {
            if matches!(c, b' ' | b'\t' | b'\r' | b'\n' | b'(' | b')' | b'"') {
                break;
            }
            self.i += 1;
        }
        let s =
            std::str::from_utf8(&self.b[start..self.i]).map_err(|_| self.err("invalid UTF-8"))?;
        Ok(Sexp::Atom(s.to_string()))
    }
    fn string(&mut self) -> Result<Sexp, ParseError> {
        self.i += 1; // '"'
        let mut out: Vec<u8> = Vec::new();
        loop {
            match self.peek() {
                None => return Err(self.err("unterminated string")),
                Some(b'"') => {
                    self.i += 1;
                    break;
                }
                Some(b'\\') => {
                    self.i += 1;
                    match self.peek() {
                        Some(b'n') => out.push(b'\n'),
                        Some(b't') => out.push(b'\t'),
                        Some(b'r') => out.push(b'\r'),
                        Some(c) => out.push(c),
                        None => return Err(self.err("unterminated string")),
                    }
                    self.i += 1;
                }
                Some(c) => {
                    if c == b'\n' {
                        self.line += 1;
                    }
                    out.push(c);
                    self.i += 1;
                }
            }
        }
        String::from_utf8(out)
            .map(Sexp::Str)
            .map_err(|_| self.err("invalid UTF-8 in a string"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_kicad_shapes() {
        let s = parse(
            "(export (version \"E\")\n  (design (tool \"Eeschema 10.0.8\"))\n  (x a \"q\\\"uote\\\\ \" ()))",
        )
        .unwrap();
        assert_eq!(s.head(), Some("export"));
        assert_eq!(s.value_of("version"), Some("E"));
        assert_eq!(
            s.child("design").unwrap().value_of("tool"),
            Some("Eeschema 10.0.8")
        );
        let x = s.child("x").unwrap();
        assert_eq!(x.args()[1], Sexp::Str("q\"uote\\ ".into()));
        assert!(s.contains_head("tool"));
        assert!(parse("(a (b)").is_err());
        assert!(parse("(a) (b)").is_err());
        assert!(parse("a").is_err());
        assert!(parse("(a \"open)").is_err());
    }
}
