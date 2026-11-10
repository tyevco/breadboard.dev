//! KiCad schematics: the version gate, the sheet files a root reaches, and
//! the Phase 1 refusals (ADR-007 D2). Connectivity is never computed here:
//! `kicad-cli` exports it (D1).

use std::path::{Path, PathBuf};

use crate::sexp::{self, Sexp};

/// The first KiCad 9 schematic version.
pub const KICAD9: u32 = 20250114;
/// The KiCad 10 schematic version (the newest this bb accepts).
pub const KICAD10: u32 = 20260306;
/// The oldest version accepted with an "untested" warning (KiCad 6.0).
pub const KICAD6: u32 = 20211123;

/// The version gate on one schematic file. `Ok(Some(w))` carries a
/// warning for an untested (KiCad 6-8) file.
pub fn check_version(version: u32, file: &str) -> Result<Option<String>, String> {
    if version > KICAD10 {
        Err(format!(
            "{file}: schematic version {version} is newer than this bb supports (up to {KICAD10}, \
             KiCad 10; ADR-007 D2). Each newer KiCad needs its own ADR amendment"
        ))
    } else if version >= KICAD9 {
        Ok(None)
    } else if version >= KICAD6 {
        Ok(Some(format!(
            "{file}: schematic version {version} (KiCad 6-8) is untested (ADR-007 D2: KiCad 9 and 10 are supported)"
        )))
    } else {
        Err(format!(
            "{file}: schematic version {version} is older than KiCad 6; open and save it in a newer KiCad"
        ))
    }
}

/// One schematic file as read for the gates.
#[derive(Clone, Debug)]
pub struct SheetFile {
    /// Path relative to the project directory, `/`-separated.
    pub rel: String,
    pub version: u32,
}

/// The project's schematic files: the root and every sheet it reaches.
#[derive(Clone, Debug)]
pub struct Project {
    pub dir: PathBuf,
    /// `<name>.kicad_pro`.
    pub pro: String,
    /// The root sheet first, then the others in the order found (each once).
    pub sheets: Vec<SheetFile>,
    pub warnings: Vec<String>,
}

fn rel_join(base: &str, name: &str) -> String {
    let joined = match base.rfind('/') {
        Some(i) => format!("{}/{}", &base[..i], name),
        None => name.to_string(),
    };
    // Normalise "a/./b" and "a/../b".
    let mut out: Vec<&str> = Vec::new();
    for part in joined.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            p => out.push(p),
        }
    }
    out.join("/")
}

/// Read the root schematic of a `.kicad_pro` and every sheet it reaches,
/// applying the version gates and the Phase 1 refusals (variants in the
/// root, jumper pin groups in any symbol).
pub fn read_project(pro: &Path) -> Result<Project, String> {
    let dir = pro
        .parent()
        .map(|d| {
            if d.as_os_str().is_empty() {
                Path::new(".")
            } else {
                d
            }
        })
        .unwrap_or(Path::new("."))
        .to_path_buf();
    let pro_name = pro
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("{}: not a file name", pro.display()))?
        .to_string();
    let stem = pro_name
        .strip_suffix(".kicad_pro")
        .ok_or_else(|| format!("{}: expected a .kicad_pro", pro.display()))?;
    let root = format!("{stem}.kicad_sch");
    let mut sheets: Vec<SheetFile> = Vec::new();
    let mut warnings = Vec::new();
    let mut todo = vec![root.clone()];
    while let Some(rel) = todo.pop() {
        if sheets.iter().any(|s| s.rel == rel) {
            continue;
        }
        let path = dir.join(&rel);
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let sx = sexp::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        if sx.head() != Some("kicad_sch") {
            return Err(format!("{}: not a KiCad schematic", path.display()));
        }
        let version: u32 = sx
            .value_of("version")
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| format!("{}: no (version N)", path.display()))?;
        if let Some(w) = check_version(version, &rel)? {
            warnings.push(w);
        }
        if rel == root && (sx.contains_head("variants") || sx.contains_head("variant")) {
            return Err(format!(
                "{rel}: the schematic defines variants, which Phase 1 refuses (ADR-007 D2: a \
                 variant changes which parts exist; its meaning in board.bbm needs a decision)"
            ));
        }
        if sx.contains_head("jumper_pin_groups") {
            return Err(format!(
                "{rel}: a symbol has jumper pin groups, which Phase 1 refuses (ADR-007 D2)"
            ));
        }
        let mut subs: Vec<String> = Vec::new();
        for sheet in sx.children("sheet") {
            for p in sheet.children("property") {
                let a = p.args();
                if a.first().and_then(Sexp::text) == Some("Sheetfile") {
                    if let Some(f) = a.get(1).and_then(Sexp::text) {
                        subs.push(rel_join(&rel, f));
                    }
                }
            }
        }
        sheets.push(SheetFile { rel, version });
        // Depth-first in file order.
        for s in subs.into_iter().rev() {
            todo.push(s);
        }
    }
    Ok(Project {
        dir,
        pro: pro_name,
        sheets,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gates() {
        // ADR-007 gate M4e: a schematic stamped 20261106 (KiCad 11) is refused.
        assert!(check_version(20261106, "a").unwrap_err().contains("newer"));
        assert!(check_version(20260306, "a").unwrap().is_none());
        assert!(check_version(20250114, "a").unwrap().is_none());
        assert!(check_version(20230121, "a")
            .unwrap()
            .unwrap()
            .contains("untested"));
        assert!(check_version(20200101, "a").is_err());
        assert_eq!(rel_join("a/b.kicad_sch", "c.kicad_sch"), "a/c.kicad_sch");
        assert_eq!(
            rel_join("b.kicad_sch", "./sub/../c.kicad_sch"),
            "c.kicad_sch"
        );
    }

    /// A project in a fresh temporary directory: `p.kicad_pro`, the root
    /// `p.kicad_sch` (with `root_extra` and a sheet `s.kicad_sch`) and that
    /// sheet (with `sheet_extra`).
    fn project(name: &str, root_extra: &str, sheet_extra: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("bb-kicad-schematic-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("p.kicad_pro"), "{}\n").unwrap();
        std::fs::write(
            d.join("p.kicad_sch"),
            format!(
                "(kicad_sch (version {KICAD10}) (generator \"eeschema\")\n{root_extra}\n\
                 (sheet (at 0 0) (size 10 10) (property \"Sheetname\" \"s\") (property \"Sheetfile\" \"s.kicad_sch\")))\n"
            ),
        )
        .unwrap();
        std::fs::write(
            d.join("s.kicad_sch"),
            format!("(kicad_sch (version {KICAD10}) (generator \"eeschema\")\n{sheet_extra})\n"),
        )
        .unwrap();
        d
    }

    #[test]
    fn variants_and_jumper_pin_groups_are_refused() {
        // ADR-007 D2's two Phase 1 refusals (the post-M7 review found them
        // untested): variants in the root sheet, jumper pin groups anywhere.
        let ok = project("ok", "", "");
        let p = read_project(&ok.join("p.kicad_pro")).unwrap();
        let rels: Vec<&str> = p.sheets.iter().map(|s| s.rel.as_str()).collect();
        assert_eq!(rels, ["p.kicad_sch", "s.kicad_sch"]);

        let v = project("variants", "(variants (variant (name \"lite\")))", "");
        let e = read_project(&v.join("p.kicad_pro")).unwrap_err();
        assert!(
            e.contains("p.kicad_sch: the schematic defines variants"),
            "{e}"
        );

        // Variants are a root-sheet property: a sub-sheet's are not refused.
        let vs = project("variants-sub", "", "(variants (variant (name \"lite\")))");
        assert!(read_project(&vs.join("p.kicad_pro")).is_ok());

        let j = project(
            "jumper",
            "",
            "(lib_symbols (symbol \"Jumper:SolderJumper_3\" (jumper_pin_groups (\"1\" \"2\"))))",
        );
        let e = read_project(&j.join("p.kicad_pro")).unwrap_err();
        assert!(
            e.contains("s.kicad_sch: a symbol has jumper pin groups"),
            "{e}"
        );

        for d in [ok, v, vs, j] {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}
