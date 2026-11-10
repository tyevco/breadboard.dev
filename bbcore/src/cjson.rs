//! Breadboard's canonical JSON (`spec/canonical-json.md`, ADR-007 D6).
//!
//! The form shared by `board.bbm`, `run.json` and `.bb/resolution.json`:
//!
//! - UTF-8, object keys sorted by UTF-16 code unit (as RFC 8785), no
//!   insignificant whitespace, one `\n` at the end of a document;
//! - strings escaped as RFC 8785 does: `"` and `\` escaped, the control
//!   characters U+0000-U+001F as `\b \t \n \f \r` or `\u00xx` (lower-case
//!   hex), everything else written as itself;
//! - **numbers are integers only**, arbitrary-precision decimal literals
//!   (optional `-`, no leading zeros, no fraction, no exponent). This reader
//!   takes them as `i128` and refuses anything outside it, and refuses any
//!   fraction or exponent: there are no floats.
//!
//! RFC 8785 is not used because its numbers are IEEE 754 doubles, and the
//! integer sub-units (femtoseconds, attofarads) exceed 2^53.

use std::fmt;

/// A JSON value with integer-only numbers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i128),
    Str(String),
    Arr(Vec<Json>),
    /// Key order as built; the writer sorts, the reader refuses duplicates.
    Obj(Vec<(String, Json)>),
}

/// A canonical-JSON read error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonError {
    pub offset: usize,
    pub msg: String,
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "byte {}: {}", self.offset, self.msg)
    }
}

impl std::error::Error for JsonError {}

impl Json {
    /// An object from pairs (any order; the writer sorts).
    pub fn obj<K: Into<String>>(pairs: impl IntoIterator<Item = (K, Json)>) -> Json {
        Json::Obj(pairs.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }
    pub fn str(s: impl Into<String>) -> Json {
        Json::Str(s.into())
    }
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(v) => v.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_int(&self) -> Option<i128> {
        match self {
            Json::Int(i) => Some(*i),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn as_arr(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(a) => Some(a),
            _ => None,
        }
    }
    pub fn as_obj(&self) -> Option<&[(String, Json)]> {
        match self {
            Json::Obj(o) => Some(o),
            _ => None,
        }
    }

    /// The canonical text of a document: the value plus one `\n`.
    pub fn to_canonical(&self) -> String {
        let mut s = String::new();
        write_value(&mut s, self);
        s.push('\n');
        s
    }

    /// The canonical text of the value alone (no trailing newline).
    pub fn to_canonical_inline(&self) -> String {
        let mut s = String::new();
        write_value(&mut s, self);
        s
    }

    /// Parse a JSON document. Integers only; a float, an exponent, a
    /// leading zero, a duplicate key or a number outside i128 is an error.
    /// Whitespace between tokens is accepted (canonical input has none).
    pub fn parse(text: &str) -> Result<Json, JsonError> {
        let mut p = Reader {
            b: text.as_bytes(),
            i: 0,
        };
        p.ws();
        let v = p.value(0)?;
        p.ws();
        if p.i != p.b.len() {
            return Err(p.err("trailing data after the document"));
        }
        Ok(v)
    }
}

/// Sort key of an object key: its UTF-16 code units (RFC 8785 §3.2.3).
fn utf16_key(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn write_value(out: &mut String, v: &Json) {
    match v {
        Json::Null => out.push_str("null"),
        Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Json::Int(i) => out.push_str(&i.to_string()),
        Json::Str(s) => write_str(out, s),
        Json::Arr(a) => {
            out.push('[');
            for (k, x) in a.iter().enumerate() {
                if k > 0 {
                    out.push(',');
                }
                write_value(out, x);
            }
            out.push(']');
        }
        Json::Obj(o) => {
            let mut keys: Vec<(Vec<u16>, &String, &Json)> =
                o.iter().map(|(k, v)| (utf16_key(k), k, v)).collect();
            keys.sort_by(|a, b| a.0.cmp(&b.0));
            out.push('{');
            for (k, (_, key, val)) in keys.iter().enumerate() {
                if k > 0 {
                    out.push(',');
                }
                write_str(out, key);
                out.push(':');
                write_value(out, val);
            }
            out.push('}');
        }
    }
}

fn write_str(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}

const MAX_DEPTH: usize = 128;

impl Reader<'_> {
    fn err(&self, msg: impl Into<String>) -> JsonError {
        JsonError {
            offset: self.i,
            msg: msg.into(),
        }
    }
    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }
    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }
    fn lit(&mut self, word: &str, v: Json) -> Result<Json, JsonError> {
        if self.b[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            Ok(v)
        } else {
            Err(self.err("unexpected token"))
        }
    }
    fn value(&mut self, depth: usize) -> Result<Json, JsonError> {
        if depth > MAX_DEPTH {
            return Err(self.err("nesting too deep"));
        }
        match self.peek() {
            None => Err(self.err("unexpected end of input")),
            Some(b'n') => self.lit("null", Json::Null),
            Some(b't') => self.lit("true", Json::Bool(true)),
            Some(b'f') => self.lit("false", Json::Bool(false)),
            Some(b'"') => self.string().map(Json::Str),
            Some(b'[') => {
                self.i += 1;
                let mut a = Vec::new();
                self.ws();
                if self.peek() == Some(b']') {
                    self.i += 1;
                    return Ok(Json::Arr(a));
                }
                loop {
                    self.ws();
                    a.push(self.value(depth + 1)?);
                    self.ws();
                    match self.peek() {
                        Some(b',') => self.i += 1,
                        Some(b']') => {
                            self.i += 1;
                            return Ok(Json::Arr(a));
                        }
                        _ => return Err(self.err("expected , or ]")),
                    }
                }
            }
            Some(b'{') => {
                self.i += 1;
                let mut o: Vec<(String, Json)> = Vec::new();
                self.ws();
                if self.peek() == Some(b'}') {
                    self.i += 1;
                    return Ok(Json::Obj(o));
                }
                loop {
                    self.ws();
                    if self.peek() != Some(b'"') {
                        return Err(self.err("expected a key"));
                    }
                    let k = self.string()?;
                    if o.iter().any(|(x, _)| *x == k) {
                        return Err(self.err(format!("duplicate key {k:?}")));
                    }
                    self.ws();
                    if self.peek() != Some(b':') {
                        return Err(self.err("expected :"));
                    }
                    self.i += 1;
                    self.ws();
                    let v = self.value(depth + 1)?;
                    o.push((k, v));
                    self.ws();
                    match self.peek() {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(Json::Obj(o));
                        }
                        _ => return Err(self.err("expected , or }")),
                    }
                }
            }
            Some(c) if c == b'-' || c.is_ascii_digit() => self.number(),
            Some(_) => Err(self.err("unexpected character")),
        }
    }
    fn number(&mut self) -> Result<Json, JsonError> {
        let start = self.i;
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        let ds = self.i;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.i += 1;
        }
        let digits = &self.b[ds..self.i];
        if digits.is_empty() {
            return Err(self.err("a number needs digits"));
        }
        if digits.len() > 1 && digits[0] == b'0' {
            return Err(self.err("a number with a leading zero"));
        }
        if matches!(self.peek(), Some(b'.' | b'e' | b'E')) {
            return Err(self.err(
                "a fraction or exponent: canonical JSON has integers only (spec/canonical-json.md)",
            ));
        }
        let text = std::str::from_utf8(&self.b[start..self.i]).expect("ASCII");
        if text == "-0" {
            return Err(self.err("-0 is not canonical"));
        }
        text.parse::<i128>().map(Json::Int).map_err(|_| JsonError {
            offset: start,
            msg: format!("{text}: outside the i128 range"),
        })
    }
    fn hex4(&mut self) -> Result<u32, JsonError> {
        if self.i + 4 > self.b.len() {
            return Err(self.err("short \\u escape"));
        }
        let t =
            std::str::from_utf8(&self.b[self.i..self.i + 4]).map_err(|_| self.err("bad \\u"))?;
        let v = u32::from_str_radix(t, 16).map_err(|_| self.err("bad \\u escape"))?;
        self.i += 4;
        Ok(v)
    }
    fn string(&mut self) -> Result<String, JsonError> {
        self.i += 1; // opening quote
        let mut s = String::new();
        loop {
            let start = self.i;
            while self
                .peek()
                .is_some_and(|c| c != b'"' && c != b'\\' && c >= 0x20)
            {
                self.i += 1;
            }
            s.push_str(
                std::str::from_utf8(&self.b[start..self.i])
                    .map_err(|_| self.err("invalid UTF-8"))?,
            );
            match self.peek() {
                None => return Err(self.err("unterminated string")),
                Some(b'"') => {
                    self.i += 1;
                    return Ok(s);
                }
                Some(b'\\') => {
                    self.i += 1;
                    let e = self.peek().ok_or_else(|| self.err("bad escape"))?;
                    self.i += 1;
                    match e {
                        b'"' => s.push('"'),
                        b'\\' => s.push('\\'),
                        b'/' => s.push('/'),
                        b'b' => s.push('\u{8}'),
                        b'f' => s.push('\u{c}'),
                        b'n' => s.push('\n'),
                        b'r' => s.push('\r'),
                        b't' => s.push('\t'),
                        b'u' => {
                            let hi = self.hex4()?;
                            let cp = if (0xD800..0xDC00).contains(&hi) {
                                if !self.b[self.i..].starts_with(b"\\u") {
                                    return Err(self.err("unpaired surrogate"));
                                }
                                self.i += 2;
                                let lo = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&lo) {
                                    return Err(self.err("unpaired surrogate"));
                                }
                                0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                            } else {
                                hi
                            };
                            s.push(char::from_u32(cp).ok_or_else(|| self.err("bad code point"))?);
                        }
                        _ => return Err(self.err("bad escape")),
                    }
                }
                Some(_) => return Err(self.err("a control character in a string")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_round_trip_beyond_doubles() {
        // ADR-007 gate M4g: 2^63 fs and a negative value, exactly.
        let t = 1i128 << 63;
        let v = Json::obj([("t_fs", Json::Int(t)), ("neg", Json::Int(-42))]);
        let text = v.to_canonical();
        assert_eq!(text, "{\"neg\":-42,\"t_fs\":9223372036854775808}\n");
        let back = Json::parse(&text).unwrap();
        assert_eq!(back.get("t_fs").unwrap().as_int(), Some(t));
        assert_eq!(back.get("neg").unwrap().as_int(), Some(-42));
        assert_eq!(back.to_canonical(), text);
        let max = Json::Int(i128::MAX).to_canonical();
        assert_eq!(Json::parse(&max).unwrap(), Json::Int(i128::MAX));
        assert_eq!(
            Json::parse(&Json::Int(i128::MIN).to_canonical()).unwrap(),
            Json::Int(i128::MIN)
        );
    }

    #[test]
    fn floats_and_non_canonical_numbers_are_refused() {
        for bad in [
            "1.5",
            "1e3",
            "2E1",
            "-0.0",
            "01",
            "-0",
            "{\"a\":3.0}",
            "170141183460469231731687303715884105728",
        ] {
            assert!(Json::parse(bad).is_err(), "{bad}");
        }
        assert!(Json::parse("{\"a\":1,\"a\":2}").is_err(), "duplicate key");
        assert!(Json::parse("[1,]").is_err());
        assert!(Json::parse("1 2").is_err());
    }

    #[test]
    fn keys_sort_by_utf16_and_strings_escape_as_jcs() {
        // U+FB01 (one UTF-16 unit, 0xFB01) sorts after U+1F600 (surrogates
        // 0xD83D...) in UTF-16 order, the reverse of code-point order.
        let v = Json::obj([
            ("\u{fb01}", Json::Int(1)),
            ("\u{1f600}", Json::Int(2)),
            ("b", Json::Int(3)),
            ("a", Json::Int(4)),
        ]);
        assert_eq!(
            v.to_canonical_inline(),
            "{\"a\":4,\"b\":3,\"\u{1f600}\":2,\"\u{fb01}\":1}"
        );
        let s = Json::str("a\"b\\c\n\u{1}/\u{7f}é");
        assert_eq!(s.to_canonical_inline(), "\"a\\\"b\\\\c\\n\\u0001/\u{7f}é\"");
        assert_eq!(Json::parse(&s.to_canonical_inline()).unwrap(), s);
        assert_eq!(
            Json::parse("\"\\ud83d\\ude00\\/\"").unwrap(),
            Json::str("\u{1f600}/")
        );
        assert!(Json::parse("\"\\ud83d\"").is_err());
    }
}
