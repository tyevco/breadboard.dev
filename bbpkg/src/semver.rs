//! Package versions and version requirements (ADR-011 D4): `MAJOR.MINOR.PATCH`
//! and Cargo's caret semantics.
//!
//! - `^1.2.3` is `>=1.2.3, <2.0.0`; `^0.2.3` is `>=0.2.3, <0.3.0`; `^0.0.3` is
//!   `=0.0.3`; `^1.2` is `>=1.2.0, <2.0.0`; `^0.1` is `>=0.1.0, <0.2.0`; `^0`
//!   is `<1.0.0`. A bare `1.2.3` means `^1.2.3`, as in Cargo.
//! - `=1.2.3` is exactly that version (`id@1.2.3` in a board file or
//!   `bb.toml` is an exact request).

use std::cmp::Ordering;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

fn num(s: &str, what: &str) -> Result<u64, String> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) || (s.len() > 1 && s.starts_with('0'))
    {
        return Err(format!(
            "{what}: expected MAJOR.MINOR.PATCH (digits, no leading zeros)"
        ));
    }
    s.parse::<u64>().map_err(|e| format!("{what}: {e}"))
}

impl Version {
    pub fn parse(s: &str) -> Result<Version, String> {
        let what = format!("version {s:?}");
        let p: Vec<&str> = s.split('.').collect();
        if p.len() != 3 {
            return Err(format!("{what}: expected MAJOR.MINOR.PATCH"));
        }
        Ok(Version {
            major: num(p[0], &what)?,
            minor: num(p[1], &what)?,
            patch: num(p[2], &what)?,
        })
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// A version requirement: caret (`^x[.y[.z]]`, or bare) or exact (`=x.y.z`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Req {
    text: String,
    lo: Version,
    /// Exclusive upper bound (`None`: exact, `lo` only).
    hi: Option<Version>,
}

impl Req {
    pub fn parse(s: &str) -> Result<Req, String> {
        let what = format!("version requirement {s:?}");
        if let Some(v) = s.strip_prefix('=') {
            let v = Version::parse(v).map_err(|e| format!("{what}: {e}"))?;
            return Ok(Req {
                text: s.to_string(),
                lo: v,
                hi: None,
            });
        }
        let body = s.strip_prefix('^').unwrap_or(s);
        let p: Vec<&str> = body.split('.').collect();
        if p.is_empty() || p.len() > 3 {
            return Err(format!(
                "{what}: expected ^MAJOR[.MINOR[.PATCH]] or =MAJOR.MINOR.PATCH"
            ));
        }
        let n: Vec<u64> = p.iter().map(|x| num(x, &what)).collect::<Result<_, _>>()?;
        let lo = Version {
            major: n[0],
            minor: *n.get(1).unwrap_or(&0),
            patch: *n.get(2).unwrap_or(&0),
        };
        // Cargo: the bound bumps the left-most non-zero component given (or
        // the last one given when all are zero).
        let hi = match (n.len(), n[0], n.get(1).copied(), n.get(2).copied()) {
            (_, m, _, _) if m > 0 => Version {
                major: m + 1,
                minor: 0,
                patch: 0,
            },
            (1, 0, _, _) => Version {
                major: 1,
                minor: 0,
                patch: 0,
            },
            (_, 0, Some(mi), _) if mi > 0 => Version {
                major: 0,
                minor: mi + 1,
                patch: 0,
            },
            (2, 0, Some(0), _) => Version {
                major: 0,
                minor: 1,
                patch: 0,
            },
            (3, 0, Some(0), Some(pa)) => Version {
                major: 0,
                minor: 0,
                patch: pa + 1,
            },
            _ => unreachable!("covered above"),
        };
        Ok(Req {
            text: s.to_string(),
            lo,
            hi: Some(hi),
        })
    }

    /// The exact requirement `=v`.
    pub fn exact(v: Version) -> Req {
        Req {
            text: format!("={v}"),
            lo: v,
            hi: None,
        }
    }

    pub fn matches(&self, v: &Version) -> bool {
        match self.hi {
            None => *v == self.lo,
            Some(hi) => v.cmp(&self.lo) != Ordering::Less && v.cmp(&hi) == Ordering::Less,
        }
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn caret_semantics_as_cargo() {
        let cases: &[(&str, &[&str], &[&str])] = &[
            ("^1.2.3", &["1.2.3", "1.9.0"], &["1.2.2", "2.0.0"]),
            ("1.2.3", &["1.2.3", "1.3.0"], &["2.0.0"]),
            ("^0.2.3", &["0.2.3", "0.2.9"], &["0.3.0", "0.2.2"]),
            ("^0.0.3", &["0.0.3"], &["0.0.4", "0.0.2"]),
            ("^1.2", &["1.2.0", "1.99.0"], &["1.1.9", "2.0.0"]),
            ("^0.1", &["0.1.0", "0.1.7"], &["0.2.0"]),
            ("^0.0", &["0.0.0", "0.0.9"], &["0.1.0"]),
            ("^0", &["0.0.0", "0.9.9"], &["1.0.0"]),
            ("^1", &["1.0.0", "1.5.0"], &["2.0.0", "0.9.0"]),
            ("=0.2.0", &["0.2.0"], &["0.2.1"]),
        ];
        for (r, yes, no) in cases {
            let q = Req::parse(r).unwrap();
            for y in *yes {
                assert!(q.matches(&v(y)), "{r} should match {y}");
            }
            for n in *no {
                assert!(!q.matches(&v(n)), "{r} should not match {n}");
            }
        }
        for bad in ["", "^", "1.2.3.4", "01.2.3", "^1.x", "=1.2", ">1.0.0"] {
            assert!(Req::parse(bad).is_err(), "{bad}");
        }
        assert!(Version::parse("1.2").is_err());
        assert!(v("0.10.0") > v("0.9.9"));
    }
}
