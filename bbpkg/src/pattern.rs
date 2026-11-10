//! `mpn_patterns` (ADR-006 D2, `spec/package.md` section 2): regular
//! expressions matched against the **whole** normalised MPN (anchored
//! implicitly). Written here, not pulled in as a crate (no new crates,
//! ADR-006 Amendment 1 item 8).
//!
//! The syntax is the common subset of Python's `re` (which
//! `scripts/validate_packages.py` compiles the patterns with) and every
//! other regex engine: literals; `.`; character classes `[...]` with
//! ranges and `^` negation; the escapes `\d \w \s \D \W \S` and an escaped
//! punctuation character; groups `( )` and `(?: )` with `|`; and the
//! quantifiers `* + ? {n} {n,} {n,m}` (greedy). Anything else (anchors,
//! backreferences, lookaround, flags, lazy quantifiers) is refused, never
//! guessed. Matching is a backtracking search over characters; MPNs are
//! short.

#[derive(Clone, Debug, PartialEq)]
enum Node {
    Char(char),
    Any,
    Class {
        neg: bool,
        items: Vec<ClassItem>,
    },
    Group(Vec<Vec<Node>>), // alternatives, each a sequence
    Repeat {
        node: Box<Node>,
        min: u32,
        max: Option<u32>,
    },
}

#[derive(Clone, Debug, PartialEq)]
enum ClassItem {
    Range(char, char),
    Digit(bool),
    Word(bool),
    Space(bool),
}

/// A compiled pattern.
#[derive(Clone, Debug, PartialEq)]
pub struct Pattern {
    alts: Vec<Vec<Node>>,
    pub source: String,
}

impl Pattern {
    /// Compile `src`. Errors name the problem.
    pub fn new(src: &str) -> Result<Pattern, String> {
        let chars: Vec<char> = src.chars().collect();
        let mut p = Parser { c: &chars, i: 0 };
        let alts = p.alternation(0)?;
        if p.i != chars.len() {
            return Err(format!("pattern {src:?}: unbalanced ')'"));
        }
        Ok(Pattern {
            alts,
            source: src.to_string(),
        })
    }

    /// Whether the pattern matches all of `s`.
    pub fn full_match(&self, s: &str) -> bool {
        let chars: Vec<char> = s.chars().collect();
        let g = Node::Group(self.alts.clone());
        match_seq(std::slice::from_ref(&g), &chars, 0, &mut |end| {
            end == chars.len()
        })
    }
}

struct Parser<'a> {
    c: &'a [char],
    i: usize,
}

impl Parser<'_> {
    fn err(&self, msg: &str) -> String {
        let s: String = self.c.iter().collect();
        format!("pattern {s:?}: {msg} (at character {})", self.i)
    }
    fn alternation(&mut self, depth: usize) -> Result<Vec<Vec<Node>>, String> {
        if depth > 32 {
            return Err(self.err("groups nested too deep"));
        }
        let mut alts = vec![Vec::new()];
        while self.i < self.c.len() {
            match self.c[self.i] {
                ')' => break,
                '|' => {
                    self.i += 1;
                    alts.push(Vec::new());
                }
                _ => {
                    let atom = self.atom(depth)?;
                    let node = self.quantified(atom)?;
                    alts.last_mut().expect("non-empty").push(node);
                }
            }
        }
        Ok(alts)
    }
    fn quantified(&mut self, atom: Node) -> Result<Node, String> {
        let (min, max) = match self.c.get(self.i) {
            Some('*') => (0, None),
            Some('+') => (1, None),
            Some('?') => (0, Some(1)),
            Some('{') => {
                let close = self.c[self.i..]
                    .iter()
                    .position(|&c| c == '}')
                    .ok_or_else(|| self.err("unterminated {"))?;
                let body: String = self.c[self.i + 1..self.i + close].iter().collect();
                let num = |t: &str| -> Result<u32, String> {
                    t.parse().map_err(|_| self.err("bad {n,m} quantifier"))
                };
                let (a, b) = match body.split_once(',') {
                    None => {
                        let n = num(&body)?;
                        (n, Some(n))
                    }
                    Some((a, "")) => (num(a)?, None),
                    Some((a, b)) => (num(a)?, Some(num(b)?)),
                };
                if b.is_some_and(|b| b < a) || a > 1000 || b.is_some_and(|b| b > 1000) {
                    return Err(self.err("bad {n,m} quantifier"));
                }
                self.i += close; // now on '}', the shared increment below
                (a, b)
            }
            _ => return Ok(atom),
        };
        self.i += 1;
        if matches!(self.c.get(self.i), Some('?' | '+' | '*' | '{')) {
            return Err(self.err("lazy, possessive or stacked quantifiers are not supported"));
        }
        if matches!(atom, Node::Repeat { .. }) {
            return Err(self.err("nothing to repeat"));
        }
        Ok(Node::Repeat {
            node: Box::new(atom),
            min,
            max,
        })
    }
    fn atom(&mut self, depth: usize) -> Result<Node, String> {
        let c = self.c[self.i];
        self.i += 1;
        match c {
            '.' => Ok(Node::Any),
            '(' => {
                if self.c.get(self.i) == Some(&'?') {
                    if self.c.get(self.i + 1) == Some(&':') {
                        self.i += 2;
                    } else {
                        return Err(self.err("only (?: ) groups are supported"));
                    }
                }
                let alts = self.alternation(depth + 1)?;
                if self.c.get(self.i) != Some(&')') {
                    return Err(self.err("unterminated group"));
                }
                self.i += 1;
                Ok(Node::Group(alts))
            }
            '[' => self.class(),
            '\\' => {
                let e = *self.c.get(self.i).ok_or_else(|| self.err("trailing \\"))?;
                self.i += 1;
                match e {
                    'd' => Ok(Node::Class {
                        neg: false,
                        items: vec![ClassItem::Digit(true)],
                    }),
                    'D' => Ok(Node::Class {
                        neg: false,
                        items: vec![ClassItem::Digit(false)],
                    }),
                    'w' => Ok(Node::Class {
                        neg: false,
                        items: vec![ClassItem::Word(true)],
                    }),
                    'W' => Ok(Node::Class {
                        neg: false,
                        items: vec![ClassItem::Word(false)],
                    }),
                    's' => Ok(Node::Class {
                        neg: false,
                        items: vec![ClassItem::Space(true)],
                    }),
                    'S' => Ok(Node::Class {
                        neg: false,
                        items: vec![ClassItem::Space(false)],
                    }),
                    e if e.is_ascii_punctuation() || e == ' ' => Ok(Node::Char(e)),
                    _ => Err(self.err(&format!("unsupported escape \\{e}"))),
                }
            }
            '^' | '$' => Err(self.err(
                "anchors are implicit (the whole MPN is matched); write the pattern without ^ and $",
            )),
            '*' | '+' | '?' | '{' => Err(self.err("nothing to repeat")),
            ']' | '}' => Err(self.err("unbalanced bracket")),
            c => Ok(Node::Char(c)),
        }
    }
    fn class(&mut self) -> Result<Node, String> {
        let mut neg = false;
        if self.c.get(self.i) == Some(&'^') {
            neg = true;
            self.i += 1;
        }
        let mut items = Vec::new();
        let mut first = true;
        loop {
            let c = *self
                .c
                .get(self.i)
                .ok_or_else(|| self.err("unterminated ["))?;
            self.i += 1;
            if c == ']' && !first {
                break;
            }
            first = false;
            let lo = if c == '\\' {
                let e = *self.c.get(self.i).ok_or_else(|| self.err("trailing \\"))?;
                self.i += 1;
                match e {
                    'd' => {
                        items.push(ClassItem::Digit(true));
                        continue;
                    }
                    'w' => {
                        items.push(ClassItem::Word(true));
                        continue;
                    }
                    's' => {
                        items.push(ClassItem::Space(true));
                        continue;
                    }
                    e if e.is_ascii_punctuation() => e,
                    _ => return Err(self.err(&format!("unsupported escape \\{e} in a class"))),
                }
            } else if c == '[' {
                return Err(self.err("nested [ in a class (escape it)"));
            } else {
                c
            };
            if self.c.get(self.i) == Some(&'-') && self.c.get(self.i + 1).is_some_and(|&n| n != ']')
            {
                let hi = self.c[self.i + 1];
                if hi == '\\' || hi < lo {
                    return Err(self.err("bad range in a class"));
                }
                self.i += 2;
                items.push(ClassItem::Range(lo, hi));
            } else {
                items.push(ClassItem::Range(lo, lo));
            }
        }
        Ok(Node::Class { neg, items })
    }
}

fn class_has(items: &[ClassItem], c: char) -> bool {
    items.iter().any(|it| match *it {
        ClassItem::Range(a, b) => a <= c && c <= b,
        ClassItem::Digit(p) => c.is_ascii_digit() == p,
        ClassItem::Word(p) => (c.is_alphanumeric() || c == '_') == p,
        ClassItem::Space(p) => c.is_whitespace() == p,
    })
}

/// Match `seq` at `pos`, calling `k` with each end position (backtracking
/// in greedy order); returns true as soon as `k` accepts.
fn match_seq(seq: &[Node], s: &[char], pos: usize, k: &mut dyn FnMut(usize) -> bool) -> bool {
    let Some((first, rest)) = seq.split_first() else {
        return k(pos);
    };
    match first {
        Node::Char(c) => s.get(pos) == Some(c) && match_seq(rest, s, pos + 1, k),
        Node::Any => s.get(pos).is_some_and(|&c| c != '\n') && match_seq(rest, s, pos + 1, k),
        Node::Class { neg, items } => {
            s.get(pos).is_some_and(|&c| class_has(items, c) != *neg)
                && match_seq(rest, s, pos + 1, k)
        }
        Node::Group(alts) => alts
            .iter()
            .any(|alt| match_seq(alt, s, pos, &mut |e| match_seq(rest, s, e, k))),
        Node::Repeat { node, min, max } => repeat(node, *min, *max, 0, rest, s, pos, k),
    }
}

#[allow(clippy::too_many_arguments)]
fn repeat(
    node: &Node,
    min: u32,
    max: Option<u32>,
    count: u32,
    rest: &[Node],
    s: &[char],
    pos: usize,
    k: &mut dyn FnMut(usize) -> bool,
) -> bool {
    // Greedy: try one more repetition first (if allowed and it consumes
    // input), then the rest.
    if max.is_none_or(|m| count < m)
        && match_seq(std::slice::from_ref(node), s, pos, &mut |e| {
            e > pos && repeat(node, min, max, count + 1, rest, s, e, k)
        })
    {
        return true;
    }
    count >= min && match_seq(rest, s, pos, k)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(p: &str, s: &str) -> bool {
        Pattern::new(p).unwrap().full_match(s)
    }

    #[test]
    fn full_match_semantics() {
        assert!(m("TPS22917DBV[RT]", "TPS22917DBVR"));
        assert!(
            !m("TPS22917DBV[RT]", "TPS22917DBVRX"),
            "anchored at the end"
        );
        assert!(
            !m("TPS22917DBV[RT]", "XTPS22917DBVR"),
            "anchored at the start"
        );
        assert!(m("STM32F072RBT[67](TR)?", "STM32F072RBT6"));
        assert!(m("STM32F072RBT[67](TR)?", "STM32F072RBT7TR"));
        assert!(!m("STM32F072RBT[67](TR)?", "STM32F072RBT6T"));
        // ADR-007 gate M4f: an ordering code is never stripped.
        assert!(!m("RT9080", "RT9080-33GJ5"));
        assert!(m("RT9080-\\d{2}GJ5", "RT9080-33GJ5"));
        assert!(m("a(b|cd)*e", "acdbbe"));
        assert!(m("x.{2,3}y", "x12y") && m("x.{2,3}y", "x123y") && !m("x.{2,3}y", "x1234y"));
        assert!(m("[^ ]+ \\(x\\)", "abc (x)"));
        assert!(m("(?:ab)+", "ababab") && !m("(?:ab)+", "aba"));
        assert!(m("a*a*a*b", "aaaab"));
        assert!(m("[A-Z0-9-]+", "BAV99-215"));
    }

    #[test]
    fn refuses_unsupported_syntax() {
        for bad in [
            "^ab", "ab$", "a**", "(a", "a)", "[a", "(?i)a", "a*?", "\\1", "a{3,2}", "*a",
        ] {
            assert!(Pattern::new(bad).is_err(), "{bad}");
        }
    }
}
