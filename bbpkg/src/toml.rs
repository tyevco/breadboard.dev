//! A TOML 1.0 reader for Breadboard's authored files (`part.toml`,
//! `pins.toml`, test vectors, `bb.lock`, and later `bb.toml` and benches).
//!
//! Written here rather than pulled in as a crate: the workspace builds from
//! its vendored registry, without new downloads. It covers what those files
//! use and what TOML 1.0 requires of them: comments; bare, quoted and dotted
//! keys; `[table]` and `[[array of tables]]` headers; basic, literal and
//! multi-line strings with every escape; decimal, hex, octal and binary
//! integers with `_`; floats (including `inf` and `nan`); booleans; arrays
//! (multi-line, trailing comma); inline tables. Dates and times are refused
//! (no Breadboard schema has one). Redefining a key or a table is an error.
//!
//! Tables are `BTreeMap`s, so iteration is in key order and nothing that
//! reads a file depends on how its keys were ordered.

use std::collections::BTreeMap;
use std::fmt;

/// A TOML table.
pub type Table = BTreeMap<String, Value>;

/// A TOML value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    String(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    Array(Vec<Value>),
    Table(Table),
}

impl Value {
    /// The TOML type name, for error messages.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::String(_) => "string",
            Value::Integer(_) => "integer",
            Value::Float(_) => "float",
            Value::Boolean(_) => "boolean",
            Value::Array(_) => "array",
            Value::Table(_) => "table",
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_table(&self) -> Option<&Table> {
        match self {
            Value::Table(t) => Some(t),
            _ => None,
        }
    }
    pub fn as_array(&self) -> Option<&Vec<Value>> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }
}

/// A parse error with its 1-based line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub line: usize,
    pub msg: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.msg)
    }
}

impl std::error::Error for Error {}

/// Parse a TOML document.
pub fn parse(text: &str) -> Result<Table, Error> {
    let mut p = Parser {
        s: text.as_bytes(),
        src: text,
        i: 0,
        line: 1,
    };
    p.document()
}

/// How a table came to exist, for the redefinition rules.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Origin {
    /// Created implicitly as the parent of a header or dotted key.
    Implicit,
    /// Defined by a `[header]`.
    Header,
    /// Created by a dotted key inside a table body.
    Dotted,
    /// An inline table: closed.
    Inline,
    /// An element of an array of tables.
    ArrayElem,
}

struct Parser<'a> {
    s: &'a [u8],
    src: &'a str,
    i: usize,
    line: usize,
}

/// The document while it is built: every table carries its origin, kept in a
/// side map keyed by its path.
struct Builder {
    root: Table,
    origins: BTreeMap<Vec<String>, Origin>,
    /// Arrays of tables, by path (so `[[a]]` can append and a plain key `a`
    /// cannot be confused with one).
    aot: std::collections::BTreeSet<Vec<String>>,
}

impl<'a> Parser<'a> {
    fn err<T>(&self, msg: impl Into<String>) -> Result<T, Error> {
        Err(Error {
            line: self.line,
            msg: msg.into(),
        })
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn peek_at(&self, k: usize) -> Option<u8> {
        self.s.get(self.i + k).copied()
    }

    fn starts_with(&self, p: &str) -> bool {
        self.s[self.i..].starts_with(p.as_bytes())
    }

    fn bump(&mut self) -> Option<u8> {
        let c = self.peek()?;
        self.i += 1;
        if c == b'\n' {
            self.line += 1;
        }
        Some(c)
    }

    /// Spaces and tabs.
    fn ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t')) {
            self.i += 1;
        }
    }

    /// A comment to the end of the line (not the newline).
    fn comment(&mut self) -> Result<(), Error> {
        if self.peek() == Some(b'#') {
            while let Some(c) = self.peek() {
                if c == b'\n' || (c == b'\r' && self.peek_at(1) == Some(b'\n')) {
                    break;
                }
                if c < 0x20 && c != b'\t' || c == 0x7f {
                    return self.err("control character in a comment");
                }
                self.i += 1;
            }
        }
        Ok(())
    }

    /// The end of a line: optional whitespace and comment, then a newline or
    /// the end of the input.
    fn eol(&mut self) -> Result<(), Error> {
        self.ws();
        self.comment()?;
        match self.peek() {
            None => Ok(()),
            Some(b'\n') => {
                self.bump();
                Ok(())
            }
            Some(b'\r') if self.peek_at(1) == Some(b'\n') => {
                self.i += 1;
                self.bump();
                Ok(())
            }
            Some(c) => self.err(format!(
                "expected the end of the line, found {:?}",
                c as char
            )),
        }
    }

    /// Whitespace, comments and newlines (inside arrays).
    fn ws_nl(&mut self) -> Result<(), Error> {
        loop {
            self.ws();
            self.comment()?;
            match self.peek() {
                Some(b'\n') => {
                    self.bump();
                }
                Some(b'\r') if self.peek_at(1) == Some(b'\n') => {
                    self.i += 1;
                    self.bump();
                }
                _ => return Ok(()),
            }
        }
    }

    fn document(&mut self) -> Result<Table, Error> {
        let mut b = Builder {
            root: Table::new(),
            origins: BTreeMap::new(),
            aot: Default::default(),
        };
        // The current table's path; empty = the root.
        let mut current: Vec<String> = Vec::new();
        // Paths of the elements of arrays of tables: each [[a]] element gets
        // its own index in the path ("a", "#3").
        loop {
            self.ws_nl()?;
            match self.peek() {
                None => break,
                Some(b'[') => {
                    let line = self.line;
                    if self.peek_at(1) == Some(b'[') {
                        self.i += 2;
                        self.ws();
                        let key = self.key()?;
                        self.ws();
                        if !self.starts_with("]]") {
                            return self.err("expected ]] after an array-of-tables header");
                        }
                        self.i += 2;
                        self.eol()?;
                        current = b.push_aot(&key).map_err(|msg| Error { line, msg })?;
                    } else {
                        self.i += 1;
                        self.ws();
                        let key = self.key()?;
                        self.ws();
                        if self.peek() != Some(b']') {
                            return self.err("expected ] after a table header");
                        }
                        self.i += 1;
                        self.eol()?;
                        current = b.open_header(&key).map_err(|msg| Error { line, msg })?;
                    }
                }
                Some(_) => {
                    let line = self.line;
                    let key = self.key()?;
                    self.ws();
                    if self.peek() != Some(b'=') {
                        return self.err("expected = after a key");
                    }
                    self.i += 1;
                    self.ws();
                    let v = self.value()?;
                    self.eol()?;
                    b.set(&current, &key, v)
                        .map_err(|msg| Error { line, msg })?;
                }
            }
        }
        Ok(b.root)
    }

    /// A possibly dotted key.
    fn key(&mut self) -> Result<Vec<String>, Error> {
        let mut parts = vec![self.simple_key()?];
        loop {
            self.ws();
            if self.peek() == Some(b'.') {
                self.i += 1;
                self.ws();
                parts.push(self.simple_key()?);
            } else {
                return Ok(parts);
            }
        }
    }

    fn simple_key(&mut self) -> Result<String, Error> {
        match self.peek() {
            Some(b'"') => {
                if self.starts_with("\"\"\"") {
                    return self.err("a multi-line string cannot be a key");
                }
                self.basic_string()
            }
            Some(b'\'') => {
                if self.starts_with("'''") {
                    return self.err("a multi-line string cannot be a key");
                }
                self.literal_string()
            }
            _ => {
                let start = self.i;
                while matches!(self.peek(), Some(c) if c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
                {
                    self.i += 1;
                }
                if self.i == start {
                    return self.err("expected a key");
                }
                Ok(self.src[start..self.i].to_string())
            }
        }
    }

    fn value(&mut self) -> Result<Value, Error> {
        match self.peek() {
            Some(b'"') => {
                if self.starts_with("\"\"\"") {
                    self.ml_basic_string().map(Value::String)
                } else {
                    self.basic_string().map(Value::String)
                }
            }
            Some(b'\'') => {
                if self.starts_with("'''") {
                    self.ml_literal_string().map(Value::String)
                } else {
                    self.literal_string().map(Value::String)
                }
            }
            Some(b'[') => self.array(),
            Some(b'{') => self.inline_table(),
            Some(b't') if self.starts_with("true") => {
                self.i += 4;
                self.after_scalar()?;
                Ok(Value::Boolean(true))
            }
            Some(b'f') if self.starts_with("false") => {
                self.i += 5;
                self.after_scalar()?;
                Ok(Value::Boolean(false))
            }
            Some(_) => self.number(),
            None => self.err("expected a value"),
        }
    }

    /// A scalar must end at whitespace, a comment, a newline, `,`, `]`, `}`.
    fn after_scalar(&self) -> Result<(), Error> {
        match self.peek() {
            None | Some(b' ' | b'\t' | b'\n' | b'\r' | b'#' | b',' | b']' | b'}') => Ok(()),
            Some(c) => self.err(format!("unexpected {:?} after a value", c as char)),
        }
    }

    fn number(&mut self) -> Result<Value, Error> {
        let start = self.i;
        while matches!(self.peek(), Some(c) if c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'+' | b'-' | b':'))
        {
            self.i += 1;
        }
        let tok = &self.src[start..self.i];
        if tok.is_empty() {
            return self.err("expected a value");
        }
        self.after_scalar()?;
        parse_number(tok).ok_or_else(|| Error {
            line: self.line,
            msg: if looks_like_datetime(tok) {
                format!("{tok}: dates and times are not supported in Breadboard files")
            } else {
                format!("{tok:?} is not a valid value (strings need quotes)")
            },
        })
    }

    fn array(&mut self) -> Result<Value, Error> {
        self.i += 1; // [
        let mut out = Vec::new();
        loop {
            self.ws_nl()?;
            if self.peek() == Some(b']') {
                self.i += 1;
                return Ok(Value::Array(out));
            }
            out.push(self.value()?);
            self.ws_nl()?;
            match self.peek() {
                Some(b',') => {
                    self.i += 1;
                }
                Some(b']') => {
                    self.i += 1;
                    return Ok(Value::Array(out));
                }
                _ => return self.err("expected , or ] in an array"),
            }
        }
    }

    fn inline_table(&mut self) -> Result<Value, Error> {
        self.i += 1; // {
        let mut b = Builder {
            root: Table::new(),
            origins: BTreeMap::new(),
            aot: Default::default(),
        };
        self.ws();
        if self.peek() == Some(b'}') {
            self.i += 1;
            return Ok(Value::Table(b.root));
        }
        loop {
            self.ws();
            let line = self.line;
            let key = self.key()?;
            self.ws();
            if self.peek() != Some(b'=') {
                return self.err("expected = in an inline table");
            }
            self.i += 1;
            self.ws();
            let v = self.value()?;
            b.set(&[], &key, v).map_err(|msg| Error { line, msg })?;
            self.ws();
            match self.peek() {
                Some(b',') => {
                    self.i += 1;
                }
                Some(b'}') => {
                    self.i += 1;
                    return Ok(Value::Table(b.root));
                }
                _ => {
                    return self
                        .err("expected , or } in an inline table (one line, no trailing comma)")
                }
            }
        }
    }

    fn escape(&mut self, out: &mut String) -> Result<(), Error> {
        let c = self.bump().ok_or_else(|| Error {
            line: self.line,
            msg: "unterminated escape".into(),
        })?;
        match c {
            b'b' => out.push('\u{8}'),
            b't' => out.push('\t'),
            b'n' => out.push('\n'),
            b'f' => out.push('\u{c}'),
            b'r' => out.push('\r'),
            b'e' => out.push('\u{1b}'),
            b'"' => out.push('"'),
            b'\\' => out.push('\\'),
            b'u' | b'U' => {
                let n = if c == b'u' { 4 } else { 8 };
                let hex = self.src.get(self.i..self.i + n).ok_or_else(|| Error {
                    line: self.line,
                    msg: "short unicode escape".into(),
                })?;
                let cp = u32::from_str_radix(hex, 16).map_err(|_| Error {
                    line: self.line,
                    msg: format!("bad unicode escape {hex:?}"),
                })?;
                let ch = char::from_u32(cp).ok_or_else(|| Error {
                    line: self.line,
                    msg: format!("escape \\{}{hex} is not a Unicode scalar value", c as char),
                })?;
                self.i += n;
                out.push(ch);
            }
            other => return self.err(format!("unknown escape \\{}", other as char)),
        }
        Ok(())
    }

    /// Copy one UTF-8 character (the input is a `&str`, so it is valid).
    fn push_char(&mut self, out: &mut String) -> Result<(), Error> {
        let ch = self.src[self.i..].chars().next().ok_or_else(|| Error {
            line: self.line,
            msg: "unexpected end of input".into(),
        })?;
        if (ch < ' ' && ch != '\t') || ch == '\u{7f}' {
            return self.err(format!("control character {:?} in a string", ch));
        }
        out.push(ch);
        self.i += ch.len_utf8();
        Ok(())
    }

    fn basic_string(&mut self) -> Result<String, Error> {
        self.i += 1;
        let mut out = String::new();
        loop {
            match self.peek() {
                None | Some(b'\n') => return self.err("unterminated string"),
                Some(b'"') => {
                    self.i += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.i += 1;
                    self.escape(&mut out)?;
                }
                Some(_) => self.push_char(&mut out)?,
            }
        }
    }

    fn literal_string(&mut self) -> Result<String, Error> {
        self.i += 1;
        let mut out = String::new();
        loop {
            match self.peek() {
                None | Some(b'\n') => return self.err("unterminated string"),
                Some(b'\'') => {
                    self.i += 1;
                    return Ok(out);
                }
                Some(_) => self.push_char(&mut out)?,
            }
        }
    }

    fn skip_first_newline(&mut self) {
        if self.peek() == Some(b'\n') {
            self.bump();
        } else if self.peek() == Some(b'\r') && self.peek_at(1) == Some(b'\n') {
            self.i += 1;
            self.bump();
        }
    }

    fn ml_basic_string(&mut self) -> Result<String, Error> {
        self.i += 3;
        self.skip_first_newline();
        let mut out = String::new();
        loop {
            if self.starts_with("\"\"\"") {
                // Up to two quotes may sit right before the closing delimiter.
                let mut n = 3;
                while self.peek_at(n) == Some(b'"') && n < 5 {
                    n += 1;
                }
                for _ in 3..n {
                    out.push('"');
                }
                self.i += n;
                return Ok(out);
            }
            match self.peek() {
                None => return self.err("unterminated multi-line string"),
                Some(b'\\') => {
                    // A line-ending backslash trims the newline and the
                    // whitespace after it.
                    let mut j = self.i + 1;
                    while matches!(self.s.get(j), Some(b' ' | b'\t')) {
                        j += 1;
                    }
                    if matches!(self.s.get(j), Some(b'\n')) || self.s[j..].starts_with(b"\r\n") {
                        self.i = j;
                        loop {
                            match self.peek() {
                                Some(b' ' | b'\t') => self.i += 1,
                                Some(b'\n') => {
                                    self.bump();
                                }
                                Some(b'\r') if self.peek_at(1) == Some(b'\n') => {
                                    self.i += 1;
                                    self.bump();
                                }
                                _ => break,
                            }
                        }
                    } else {
                        self.i += 1;
                        self.escape(&mut out)?;
                    }
                }
                Some(b'\n') => {
                    self.bump();
                    out.push('\n');
                }
                Some(b'\r') if self.peek_at(1) == Some(b'\n') => {
                    self.i += 1;
                    self.bump();
                    out.push('\n');
                }
                Some(_) => self.push_char(&mut out)?,
            }
        }
    }

    fn ml_literal_string(&mut self) -> Result<String, Error> {
        self.i += 3;
        self.skip_first_newline();
        let mut out = String::new();
        loop {
            if self.starts_with("'''") {
                let mut n = 3;
                while self.peek_at(n) == Some(b'\'') && n < 5 {
                    n += 1;
                }
                for _ in 3..n {
                    out.push('\'');
                }
                self.i += n;
                return Ok(out);
            }
            match self.peek() {
                None => return self.err("unterminated multi-line string"),
                Some(b'\n') => {
                    self.bump();
                    out.push('\n');
                }
                Some(b'\r') if self.peek_at(1) == Some(b'\n') => {
                    self.i += 1;
                    self.bump();
                    out.push('\n');
                }
                Some(_) => self.push_char(&mut out)?,
            }
        }
    }
}

fn looks_like_datetime(tok: &str) -> bool {
    tok.contains(':') || (tok.len() >= 10 && tok.as_bytes().get(4) == Some(&b'-'))
}

fn digits_ok(s: &str, radix: u32) -> bool {
    // Underscores only between digits.
    !s.is_empty()
        && !s.starts_with('_')
        && !s.ends_with('_')
        && !s.contains("__")
        && s.chars().all(|c| c == '_' || c.is_digit(radix))
}

fn parse_number(tok: &str) -> Option<Value> {
    match tok {
        "inf" | "+inf" => return Some(Value::Float(f64::INFINITY)),
        "-inf" => return Some(Value::Float(f64::NEG_INFINITY)),
        "nan" | "+nan" | "-nan" => return Some(Value::Float(f64::NAN)),
        _ => {}
    }
    for (prefix, radix) in [("0x", 16), ("0o", 8), ("0b", 2)] {
        if let Some(rest) = tok.strip_prefix(prefix) {
            if !digits_ok(rest, radix) {
                return None;
            }
            return i64::from_str_radix(&rest.replace('_', ""), radix)
                .ok()
                .map(Value::Integer);
        }
    }
    let (sign, body) = match tok.as_bytes()[0] {
        b'+' | b'-' => (&tok[..1], &tok[1..]),
        _ => ("", tok),
    };
    if body.is_empty() {
        return None;
    }
    let is_float = body.contains(['.', 'e', 'E']);
    if !is_float {
        if !digits_ok(body, 10) || (body.len() > 1 && body.starts_with('0')) {
            return None;
        }
        return format!("{sign}{}", body.replace('_', ""))
            .parse()
            .ok()
            .map(Value::Integer);
    }
    // Float: int-part [ '.' frac ] [ (e|E) [+-] exp ]; each part digits with
    // single underscores between digits.
    let (mant, exp) = match body.find(['e', 'E']) {
        Some(k) => (&body[..k], Some(&body[k + 1..])),
        None => (body, None),
    };
    let (int, frac) = match mant.split_once('.') {
        Some((a, b)) => (a, Some(b)),
        None => (mant, None),
    };
    if !digits_ok(int, 10) || (int.len() > 1 && int.starts_with('0')) {
        return None;
    }
    if let Some(f) = frac {
        if !digits_ok(f, 10) {
            return None;
        }
    }
    if let Some(e) = exp {
        let e = e.strip_prefix(['+', '-']).unwrap_or(e);
        if !digits_ok(e, 10) {
            return None;
        }
    }
    format!("{sign}{}", body.replace('_', ""))
        .parse()
        .ok()
        .map(Value::Float)
}

impl Builder {
    /// The table at `path` (which must exist).
    fn table_mut(&mut self, path: &[String]) -> &mut Table {
        let mut t = &mut self.root;
        for k in path {
            t = match t.get_mut(k) {
                Some(Value::Table(x)) => x,
                Some(Value::Array(a)) => match a.last_mut() {
                    Some(Value::Table(x)) => x,
                    _ => unreachable!("path through a non-table"),
                },
                _ => unreachable!("missing table on path"),
            };
        }
        t
    }

    /// Walk `path` from the root, creating implicit tables. Through an array
    /// of tables, the path continues in its last element.
    fn ensure(&mut self, path: &[String], how: Origin) -> Result<(), String> {
        for n in 1..=path.len() {
            let prefix = &path[..n];
            let (parent, key) = prefix.split_at(n - 1);
            let key = &key[0];
            let is_aot = self.aot.contains(prefix);
            let t = self.table_mut(parent);
            match t.get(key) {
                None => {
                    t.insert(key.clone(), Value::Table(Table::new()));
                    self.origins.insert(
                        prefix.to_vec(),
                        if n == path.len() {
                            how
                        } else {
                            Origin::Implicit
                        },
                    );
                }
                Some(Value::Table(_)) => {
                    if self.origins.get(prefix) == Some(&Origin::Inline) {
                        return Err(format!(
                            "{} is an inline table and cannot be extended",
                            prefix.join(".")
                        ));
                    }
                }
                Some(Value::Array(_)) if is_aot => {}
                Some(v) => {
                    return Err(format!(
                        "{} is already a {}, not a table",
                        prefix.join("."),
                        v.type_name()
                    ))
                }
            }
        }
        Ok(())
    }

    fn open_header(&mut self, path: &[String]) -> Result<Vec<String>, String> {
        if self.aot.contains(path) {
            return Err(format!(
                "[{}] is already an array of tables",
                path.join(".")
            ));
        }
        let existed = {
            let (parent, key) = path.split_at(path.len() - 1);
            self.ensure(parent, Origin::Implicit)?;
            self.table_mut(parent).contains_key(&key[0])
        };
        if existed {
            match self.origins.get(path) {
                Some(Origin::Implicit) => {
                    self.origins.insert(path.to_vec(), Origin::Header);
                }
                Some(Origin::Header) => {
                    return Err(format!("table [{}] defined twice", path.join(".")))
                }
                _ => return Err(format!("[{}] redefines an existing key", path.join("."))),
            }
        } else {
            self.ensure(path, Origin::Header)?;
        }
        Ok(path.to_vec())
    }

    fn push_aot(&mut self, path: &[String]) -> Result<Vec<String>, String> {
        let (parent, key) = path.split_at(path.len() - 1);
        self.ensure(parent, Origin::Implicit)?;
        let is_aot = self.aot.contains(path);
        let t = self.table_mut(parent);
        match t.get_mut(&key[0]) {
            None => {
                t.insert(
                    key[0].clone(),
                    Value::Array(vec![Value::Table(Table::new())]),
                );
                self.aot.insert(path.to_vec());
            }
            Some(Value::Array(a)) if is_aot => a.push(Value::Table(Table::new())),
            Some(_) => {
                return Err(format!(
                    "[[{}]]: {} is already defined and is not an array of tables",
                    path.join("."),
                    path.join(".")
                ))
            }
        }
        // A new element: forget the origins recorded under the previous one.
        let prefix = path.to_vec();
        self.origins
            .retain(|k, _| !(k.len() > prefix.len() && k.starts_with(&prefix)));
        self.aot
            .retain(|k| !(k.len() > prefix.len() && k.starts_with(&prefix)));
        self.origins.insert(prefix.clone(), Origin::ArrayElem);
        Ok(prefix)
    }

    fn set(&mut self, current: &[String], key: &[String], v: Value) -> Result<(), String> {
        let mut full = current.to_vec();
        full.extend_from_slice(&key[..key.len() - 1]);
        // Dotted keys create (or extend) tables defined by dotted keys only.
        for n in current.len() + 1..=full.len() {
            let prefix = &full[..n];
            let (parent, k) = prefix.split_at(n - 1);
            let t = self.table_mut(parent);
            match t.get(&k[0]) {
                None => {
                    t.insert(k[0].clone(), Value::Table(Table::new()));
                    self.origins.insert(prefix.to_vec(), Origin::Dotted);
                }
                Some(Value::Table(_)) => match self.origins.get(prefix) {
                    Some(Origin::Dotted) => {}
                    _ => {
                        return Err(format!(
                            "dotted key {} extends a table defined elsewhere",
                            key.join(".")
                        ))
                    }
                },
                Some(other) => {
                    return Err(format!(
                        "{} is already a {}, not a table",
                        prefix.join("."),
                        other.type_name()
                    ))
                }
            }
        }
        let last = key.last().expect("non-empty key").clone();
        let mut path_of_value = full.clone();
        path_of_value.push(last.clone());
        let inline = matches!(v, Value::Table(_));
        let t = self.table_mut(&full);
        if t.contains_key(&last) {
            return Err(format!("key {} defined twice", path_of_value.join(".")));
        }
        t.insert(last, v);
        if inline {
            self.origins.insert(path_of_value, Origin::Inline);
        }
        Ok(())
    }
}

/// Quote `s` as a TOML basic string.
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                out.push_str(&format!("\\u{:04X}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> Value {
        Value::String(v.into())
    }

    #[test]
    fn tables_arrays_and_scalars() {
        let doc = parse(
            r#"
# comment
schema = "bb-part/0"   # trailing
[part]
id = "ti/tps22917"
mpn = ["A", 'B',
  "C",   # multi-line, trailing comma
]
n = 1_000
neg = -17
f = 3.5e-3
yes = true
[footprints]
kicad = []
[[analog]]
name = "lsw"
params = { ron_add = "0", nested.x = 1 }
[[analog.qualified]]
lattice = "10us"
[analog.provenance]
source = "datasheet"
[[analog]]
name = "two"
a.b.c = 'lit\eral'
"quoted key" = """
first
second \
   joined"""
"#,
        )
        .unwrap();
        assert_eq!(doc["schema"], s("bb-part/0"));
        let part = doc["part"].as_table().unwrap();
        assert_eq!(part["mpn"], Value::Array(vec![s("A"), s("B"), s("C")]));
        assert_eq!(part["n"], Value::Integer(1000));
        assert_eq!(part["neg"], Value::Integer(-17));
        assert_eq!(part["f"], Value::Float(3.5e-3));
        assert_eq!(part["yes"], Value::Boolean(true));
        let analog = doc["analog"].as_array().unwrap();
        assert_eq!(analog.len(), 2);
        let a0 = analog[0].as_table().unwrap();
        assert_eq!(a0["name"], s("lsw"));
        let params = a0["params"].as_table().unwrap();
        assert_eq!(params["ron_add"], s("0"));
        assert_eq!(params["nested"].as_table().unwrap()["x"], Value::Integer(1));
        assert_eq!(a0["qualified"].as_array().unwrap().len(), 1);
        assert_eq!(
            a0["provenance"].as_table().unwrap()["source"],
            s("datasheet")
        );
        let a1 = analog[1].as_table().unwrap();
        assert_eq!(
            a1["a"].as_table().unwrap()["b"].as_table().unwrap()["c"],
            s("lit\\eral")
        );
        assert_eq!(a1["quoted key"], s("first\nsecond joined"));
    }

    #[test]
    fn escapes_and_unicode() {
        let doc = parse("a = \"\\u00b5 \\t \\\" \\\\ µ\"\nb = '''\nx''y'''\n").unwrap();
        assert_eq!(doc["a"], s("\u{b5} \t \" \\ µ"));
        assert_eq!(doc["b"], s("x''y"));
    }

    #[test]
    fn crlf_is_accepted() {
        let doc = parse("[a]\r\nb = 1\r\n").unwrap();
        assert_eq!(doc["a"].as_table().unwrap()["b"], Value::Integer(1));
    }

    #[test]
    fn errors() {
        for bad in [
            "a = 1\na = 2\n",
            "[a]\n[a]\n",
            "a = {x = 1}\n[a]\n",
            "a = {x = 1}\na.y = 2\n",
            "a = 1\n[a]\n",
            "[[a]]\n[a]\n",
            "a = \"unterminated\n",
            "a = bare\n",
            "a = 1979-05-27\n",
            "a = 07\n",
            "a = 1__0\n",
            "a = \"\\q\"\n",
            "a = [1 2]\n",
            "a = 1 b = 2\n",
            "= 1\n",
            "a = {x = 1,}\n",
        ] {
            assert!(parse(bad).is_err(), "should fail: {bad:?}");
        }
        let e = parse("x = 1\ny = oops\n").unwrap_err();
        assert_eq!(e.line, 2);
    }

    #[test]
    fn header_after_implicit_parent_and_aot_subtables() {
        let doc =
            parse("[a.b]\nx = 1\n[a]\ny = 2\n[[t]]\n[t.s]\nk = 1\n[[t]]\n[t.s]\nk = 2\n").unwrap();
        let a = doc["a"].as_table().unwrap();
        assert_eq!(a["y"], Value::Integer(2));
        let t = doc["t"].as_array().unwrap();
        assert_eq!(
            t[1].as_table().unwrap()["s"].as_table().unwrap()["k"],
            Value::Integer(2)
        );
    }

    #[test]
    fn quote_roundtrips() {
        for x in ["plain", "with \"q\" and \\", "tab\tnl\n", "µ"] {
            let doc = parse(&format!("k = {}\n", quote(x))).unwrap();
            assert_eq!(doc["k"], s(x));
        }
    }
}
