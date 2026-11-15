//! Schema ids and the versioning policy (ADR-006 D11).
//!
//! A schema id is `<name>/<major>`. `bb` reads the current and the previous
//! major of every schema and writes only the current one; reading the
//! previous major prints a migration hint; a newer major is refused with
//! "upgrade bb". `bb pkg migrate` rewrites a package to the current major.
//!
//! The Phase 2 schema epoch (milestone S, ADR-009..013) moves every schema
//! to its next major in one commit. Milestones B1 (ADR-010) and R1
//! (ADR-013) develop against the epoch's draft and carry their pieces of it
//! here:
//!
//! - `bb-bench/1` (ADR-010 D1): every `/0` key keeps its meaning, so the
//!   migration rewrites the `schema` line only, and a migrated bench builds
//!   a byte-identical deck (gate G1);
//! - `bb-part/1`, for the `switch` view kind (ADR-010 D2): every `/0` key
//!   keeps its meaning, so the migration rewrites the `schema` line only.
//!   The other `bb-part/1` keys (ADR-009, ADR-012) stay reserved until S;
//! - `bb-project/1` and `bb-board/1` (ADR-013 D1, R1: `[[device]]` and
//!   virtual instances): their `/0` files are read and migrated, the schema
//!   line only.
//!
//! The machinery is also exercised by a synthetic bump in the tests (gate
//! G9 of ADR-006).

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

/// `bb-part/0` -> `bb-part/1`: the `schema` line (every `/0` key keeps its
/// meaning; `/1` adds the `switch` view kind, ADR-010 D2).
fn part_0_to_1(t: &str) -> Result<String, String> {
    rewrite_schema_line(t, "bb-part/1")
}

/// `bb-bench/0` -> `bb-bench/1`: the `schema` line (ADR-010 D1: every `/0`
/// key keeps its meaning).
fn bench_0_to_1(t: &str) -> Result<String, String> {
    rewrite_schema_line(t, "bb-bench/1")
}

impl Registry {
    /// The registry of this build: `bb-part` and `bb-bench` at major 1
    /// (ADR-010, the B1 draft of the schema epoch), `bb-project` and
    /// `bb-board` at major 1 (ADR-013, R1), every other schema at major 0.
    pub fn current() -> Registry {
        fn project_0_to_1(t: &str) -> Result<String, String> {
            rewrite_schema_line(t, "bb-project/1")
        }
        // board.bbm is canonical JSON with one `"schema":"bb-board/0"` member;
        // `bb open` rewrites it whole anyway.
        fn board_0_to_1(t: &str) -> Result<String, String> {
            let from = "\"schema\":\"bb-board/0\"";
            if t.matches(from).count() != 1 {
                return Err("board.bbm: no single schema member".into());
            }
            Ok(t.replacen(from, "\"schema\":\"bb-board/1\"", 1))
        }
        Registry {
            entries: SCHEMAS
                .iter()
                .map(|s| match *s {
                    "bb-part" => (*s, 1, vec![part_0_to_1 as Migration]),
                    "bb-bench" => (*s, 1, vec![bench_0_to_1 as Migration]),
                    "bb-project" => (*s, 1, vec![project_0_to_1 as Migration]),
                    "bb-board" => (*s, 1, vec![board_0_to_1 as Migration]),
                    _ => (*s, 0, Vec::new()),
                })
                .collect(),
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

    /// A synthetic bump: bb-pins/1 renames nothing but changes the id (the
    /// shape of a migration, ADR-006 D11, gate G9).
    fn bumped() -> Registry {
        fn pins_0_to_1(t: &str) -> Result<String, String> {
            rewrite_schema_line(t, "bb-pins/1")
        }
        let mut r = Registry::current();
        for e in &mut r.entries {
            if e.0 == "bb-pins" {
                e.1 = 1;
                e.2 = vec![pins_0_to_1 as Migration];
            }
        }
        r
    }

    #[test]
    fn current_build_reads_its_majors() {
        let r = Registry::current();
        assert_eq!(r.check("bb-pins", "bb-pins/0"), Ok(Version::Current));
        let e = r.check("bb-pins", "bb-pins/1").unwrap_err();
        assert!(e.contains("upgrade bb"), "{e}");
        assert!(r.check("bb-pins", "bb-part/0").is_err());
        assert!(r.check("bb-pins", "bb-pins").is_err());
        assert!(r.check("bb-pins", "bb-pins/01").is_err());
        assert_eq!(r.migrate("bb-pins", "bb-pins/0", "x"), Ok(None));
        // B1's draft of the schema epoch (ADR-010 D1, D2): bb-part/1 and
        // bb-bench/1 are current, their /0 is the previous major and
        // migrates by its schema line.
        for (name, now) in [("bb-part", "bb-part/1"), ("bb-bench", "bb-bench/1")] {
            assert_eq!(r.current_id(name), now);
            assert_eq!(r.check(name, now), Ok(Version::Current));
            let old = now.replace("/1", "/0");
            assert!(matches!(r.check(name, &old), Ok(Version::Previous { .. })));
            let text = format!("# c\nschema = \"{old}\"\n[x]\ny = 1\n");
            assert_eq!(
                r.migrate(name, &old, &text).unwrap().unwrap(),
                text.replace(&old, now)
            );
            assert!(r
                .check(name, &now.replace("/1", "/2"))
                .unwrap_err()
                .contains("upgrade bb"));
        }
    }

    #[test]
    fn project_and_board_are_at_major_1() {
        // ADR-013 (Phase 2 R1): bb-project/1 and bb-board/1; /0 is read and
        // migrated, the schema line only.
        let r = Registry::current();
        assert_eq!(r.current_id("bb-project"), "bb-project/1");
        assert_eq!(r.current_id("bb-board"), "bb-board/1");
        assert!(matches!(
            r.check("bb-project", "bb-project/0"),
            Ok(Version::Previous { .. })
        ));
        assert!(r
            .check("bb-project", "bb-project/2")
            .unwrap_err()
            .contains("upgrade bb"));
        let t = "# p\nschema = \"bb-project/0\"\n[[board]]\nid = \"main\"\n";
        assert_eq!(
            r.migrate("bb-project", "bb-project/0", t).unwrap().unwrap(),
            "# p\nschema = \"bb-project/1\"\n[[board]]\nid = \"main\"\n"
        );
        let b = "{\"boards\":[],\"schema\":\"bb-board/0\"}\n";
        assert_eq!(
            r.migrate("bb-board", "bb-board/0", b).unwrap().unwrap(),
            "{\"boards\":[],\"schema\":\"bb-board/1\"}\n"
        );
    }

    #[test]
    fn synthetic_bump_reads_previous_and_migrates() {
        let r = bumped();
        assert_eq!(r.current_id("bb-pins"), "bb-pins/1");
        assert!(matches!(
            r.check("bb-pins", "bb-pins/0"),
            Ok(Version::Previous { .. })
        ));
        assert_eq!(r.check("bb-pins", "bb-pins/1"), Ok(Version::Current));
        assert!(r
            .check("bb-pins", "bb-pins/2")
            .unwrap_err()
            .contains("upgrade bb"));
        let text = "# c\nschema = \"bb-pins/0\"   # keep?\n[[pin]]\nnumber = \"1\"\n";
        let m = r.migrate("bb-pins", "bb-pins/0", text).unwrap().unwrap();
        assert_eq!(m, "# c\nschema = \"bb-pins/1\"\n[[pin]]\nnumber = \"1\"\n");
        assert_eq!(r.migrate("bb-pins", "bb-pins/1", &m), Ok(None));
    }
}
