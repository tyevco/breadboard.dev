//! Schema ids and the versioning policy (ADR-006 D11).
//!
//! A schema id is `<name>/<major>`. `bb` reads the current and the previous
//! major of every schema and writes only the current one; reading the
//! previous major prints a migration hint; a newer major is refused with
//! "upgrade bb". `bb pkg migrate` rewrites a package to the current major.
//!
//! Every schema is at major 0 today, so no previous major exists yet. The
//! machinery is exercised by a synthetic `bb-part/1` registry in the tests
//! (gate G9), which is exactly what the first real bump will use.

use std::fmt;

/// The schemas ADR-006 and ADR-007 define.
pub const SCHEMAS: &[&str] = &[
    "bb-part",
    "bb-pins",
    "bb-pkgtest",
    "bb-lock",
    "bb-project",
    "bb-bench",
    "bb-board",
];

/// A parsed schema id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaId {
    pub name: String,
    pub major: u32,
}

impl fmt::Display for SchemaId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.name, self.major)
    }
}

impl SchemaId {
    pub fn parse(s: &str) -> Result<SchemaId, String> {
        let (name, major) = s
            .split_once('/')
            .ok_or_else(|| format!("schema {s:?}: expected <name>/<major>"))?;
        let major: u32 = major
            .parse()
            .ok()
            .filter(|_| !major.starts_with('+') && (major == "0" || !major.starts_with('0')))
            .ok_or_else(|| format!("schema {s:?}: the major version must be a number"))?;
        Ok(SchemaId {
            name: name.to_string(),
            major,
        })
    }
}

/// One mechanical migration from `major` to `major + 1` of a schema: a
/// rewrite of the file text (format-preserving where possible).
pub type Migration = fn(&str) -> Result<String, String>;

/// The versions `bb` knows, per schema: the current major and the
/// migrations into it.
#[derive(Clone)]
pub struct Registry {
    /// `(schema name, current major, migrations[i] = i -> i + 1)`.
    pub entries: Vec<(&'static str, u32, Vec<Migration>)>,
}

/// What reading a file's schema id found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Version {
    /// The current major.
    Current,
    /// The previous major: readable after migration; `hint` says how.
    Previous { hint: String },
}

impl Registry {
    /// The registry of this build: every schema at major 0.
    pub fn current() -> Registry {
        Registry {
            entries: SCHEMAS.iter().map(|s| (*s, 0, Vec::new())).collect(),
        }
    }

    pub fn current_major(&self, name: &str) -> Option<u32> {
        self.entries
            .iter()
            .find(|(n, ..)| *n == name)
            .map(|(_, m, _)| *m)
    }

    /// The schema id this build writes for `name`.
    pub fn current_id(&self, name: &str) -> String {
        format!("{name}/{}", self.current_major(name).unwrap_or(0))
    }

    /// Check a file's `schema` string against the expected schema `name`.
    pub fn check(&self, name: &str, schema: &str) -> Result<Version, String> {
        let id = SchemaId::parse(schema)?;
        if id.name != name {
            return Err(format!("schema {schema:?}: expected a {name}/<major> file"));
        }
        let cur = self
            .current_major(name)
            .ok_or_else(|| format!("unknown schema {name}"))?;
        if id.major == cur {
            Ok(Version::Current)
        } else if id.major > cur {
            Err(format!(
                "schema {schema}: newer than this bb understands ({name}/{cur}); upgrade bb"
            ))
        } else if id.major + 1 == cur {
            Ok(Version::Previous {
                hint: format!(
                    "{schema} is the previous major: bb reads it, but run `bb pkg migrate` to rewrite it as {name}/{cur}"
                ),
            })
        } else {
            Err(format!(
                "schema {schema}: too old for this bb (it reads {name}/{} and {name}/{cur}); migrate with an older bb first",
                cur - 1
            ))
        }
    }

    /// Migrate the text of a `name` file from its major to the current one.
    /// Returns `None` when it is already current.
    pub fn migrate(&self, name: &str, schema: &str, text: &str) -> Result<Option<String>, String> {
        let id = SchemaId::parse(schema)?;
        let (_, cur, migs) = self
            .entries
            .iter()
            .find(|(n, ..)| *n == name)
            .ok_or_else(|| format!("unknown schema {name}"))?;
        if id.major == *cur {
            return Ok(None);
        }
        self.check(name, schema)?;
        let mut t = text.to_string();
        for m in id.major..*cur {
            let f = migs
                .get(m as usize)
                .ok_or_else(|| format!("no migration {name}/{m} -> {name}/{}", m + 1))?;
            t = f(&t)?;
        }
        Ok(Some(t))
    }
}

/// Replace the value of the top-level `schema = "..."` line of a TOML file,
/// keeping everything else byte for byte (for migrations).
pub fn rewrite_schema_line(text: &str, new_id: &str) -> Result<String, String> {
    let mut out = String::with_capacity(text.len() + 4);
    let mut done = false;
    for line in text.split_inclusive('\n') {
        let t = line.trim_start();
        if !done && t.starts_with("schema") && t["schema".len()..].trim_start().starts_with('=') {
            let indent = &line[..line.len() - t.len()];
            let nl = if line.ends_with("\r\n") {
                "\r\n"
            } else if line.ends_with('\n') {
                "\n"
            } else {
                ""
            };
            out.push_str(&format!("{indent}schema = \"{new_id}\"{nl}"));
            done = true;
        } else {
            out.push_str(line);
        }
    }
    if !done {
        return Err("no top-level schema line".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic bump: bb-part/1 renames nothing but changes the id (the
    /// shape of the first real migration, ADR-006 D11, gate G9).
    fn bumped() -> Registry {
        fn part_0_to_1(t: &str) -> Result<String, String> {
            rewrite_schema_line(t, "bb-part/1")
        }
        let mut r = Registry::current();
        for e in &mut r.entries {
            if e.0 == "bb-part" {
                e.1 = 1;
                e.2 = vec![part_0_to_1 as Migration];
            }
        }
        r
    }

    #[test]
    fn current_build_reads_major_0_only() {
        let r = Registry::current();
        assert_eq!(r.check("bb-part", "bb-part/0"), Ok(Version::Current));
        let e = r.check("bb-part", "bb-part/1").unwrap_err();
        assert!(e.contains("upgrade bb"), "{e}");
        assert!(r.check("bb-part", "bb-pins/0").is_err());
        assert!(r.check("bb-part", "bb-part").is_err());
        assert!(r.check("bb-part", "bb-part/01").is_err());
        assert_eq!(r.migrate("bb-part", "bb-part/0", "x"), Ok(None));
    }

    #[test]
    fn synthetic_bump_reads_previous_and_migrates() {
        let r = bumped();
        assert_eq!(r.current_id("bb-part"), "bb-part/1");
        assert!(matches!(
            r.check("bb-part", "bb-part/0"),
            Ok(Version::Previous { .. })
        ));
        assert_eq!(r.check("bb-part", "bb-part/1"), Ok(Version::Current));
        assert!(r
            .check("bb-part", "bb-part/2")
            .unwrap_err()
            .contains("upgrade bb"));
        let text = "# c\nschema = \"bb-part/0\"   # keep?\n[part]\nid = \"x/y\"\n";
        let m = r.migrate("bb-part", "bb-part/0", text).unwrap().unwrap();
        assert_eq!(m, "# c\nschema = \"bb-part/1\"\n[part]\nid = \"x/y\"\n");
        assert_eq!(r.migrate("bb-part", "bb-part/1", &m), Ok(None));
    }
}
