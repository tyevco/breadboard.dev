//! The `kicad-cli` runner (ADR-007 D1): connectivity comes from KiCad's own
//! netlist export, run on a **temporary copy** of the project, never in it
//! (`kicad-cli` writes `<project>.kicad_prl` next to the `.kicad_pro`,
//! Context 3).
//!
//! `kicad-cli` is found through `$BB_KICAD_CLI`, else `PATH`. In WSL it may
//! be the Windows build (`kicad-cli.exe`): then the copy goes to the Windows
//! temporary directory (`$BB_KICAD_WINTMP`, else `%TEMP%`), because the
//! Windows build cannot open `\\wsl.localhost` paths, and paths are passed
//! through `wslpath -w`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::schematic::Project;
use crate::sexp;

/// A `kicad-cli` executable.
#[derive(Clone, Debug)]
pub struct KicadCli {
    pub exe: PathBuf,
    /// The Windows build, run from WSL.
    pub windows: bool,
}

/// The result of one export.
#[derive(Clone, Debug)]
pub struct Export {
    /// The netlist text as `kicad-cli` wrote it.
    pub netlist: String,
    /// The project files that were copied (relative paths, sorted).
    pub copied: Vec<String>,
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

impl KicadCli {
    /// `$BB_KICAD_CLI`, else `kicad-cli` on `PATH`.
    pub fn find() -> Result<KicadCli, String> {
        let exe = match std::env::var_os("BB_KICAD_CLI") {
            Some(p) => PathBuf::from(p),
            None => find_on_path("kicad-cli").ok_or(
                "kicad-cli not found (install KiCad 9 or 10, or set BB_KICAD_CLI); \
                 a committed .net needs no KiCad (netlist = ... in bb.toml)",
            )?,
        };
        if !exe.is_file() {
            return Err(format!("kicad-cli {}: not a file", exe.display()));
        }
        let windows = exe
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("exe"));
        Ok(KicadCli { exe, windows })
    }

    /// `kicad-cli version` (for `.bb/import.json`).
    pub fn version(&self) -> Result<String, String> {
        let out = Command::new(&self.exe)
            .arg("version")
            .output()
            .map_err(|e| format!("run {}: {e}", self.exe.display()))?;
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    fn arg_path(&self, p: &Path) -> Result<String, String> {
        if !self.windows {
            return Ok(p.display().to_string());
        }
        let out = Command::new("wslpath")
            .arg("-w")
            .arg(p)
            .output()
            .map_err(|e| format!("wslpath: {e}"))?;
        if !out.status.success() {
            return Err(format!("wslpath -w {}: failed", p.display()));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    fn temp_base(&self) -> Result<PathBuf, String> {
        if !self.windows {
            return Ok(std::env::temp_dir());
        }
        if let Some(d) = std::env::var_os("BB_KICAD_WINTMP") {
            return Ok(PathBuf::from(d));
        }
        let out = Command::new("cmd.exe")
            .args(["/C", "echo %TEMP%"])
            .current_dir("/mnt/c")
            .output()
            .map_err(|e| format!("cmd.exe (to find %TEMP% for the Windows kicad-cli): {e}"))?;
        let win = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let u = Command::new("wslpath")
            .arg("-u")
            .arg(&win)
            .output()
            .map_err(|e| format!("wslpath: {e}"))?;
        let p = PathBuf::from(String::from_utf8_lossy(&u.stdout).trim());
        if win.is_empty() || !p.is_dir() {
            return Err(format!(
                "cannot find the Windows temporary directory ({win:?}); set BB_KICAD_WINTMP"
            ));
        }
        Ok(p)
    }

    /// Export the netlist of `project` from a temporary copy.
    pub fn export(&self, project: &Project) -> Result<Export, String> {
        let mut files: Vec<String> = project.sheets.iter().map(|s| s.rel.clone()).collect();
        files.push(project.pro.clone());
        if project.dir.join("sym-lib-table").is_file() {
            files.push("sym-lib-table".into());
            files.extend(project_libraries(&project.dir.join("sym-lib-table")));
        }
        files.sort();
        files.dedup();
        let tmp = TempDir::new(&self.temp_base()?)?;
        for f in &files {
            let src = project.dir.join(f);
            if !src.is_file() {
                continue; // a library named in sym-lib-table but absent
            }
            let dst = tmp.path.join(f);
            if let Some(d) = dst.parent() {
                std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
            }
            std::fs::copy(&src, &dst).map_err(|e| format!("copy {}: {e}", src.display()))?;
        }
        let root = tmp.path.join(&project.sheets[0].rel);
        let out = tmp.path.join("bb-export.net");
        let o = Command::new(&self.exe)
            .args(["sch", "export", "netlist", "--format", "kicadsexpr", "-o"])
            .arg(self.arg_path(&out)?)
            .arg(self.arg_path(&root)?)
            .current_dir(&tmp.path)
            .output()
            .map_err(|e| format!("run {}: {e}", self.exe.display()))?;
        if !o.status.success() || !out.is_file() {
            return Err(format!(
                "{} sch export netlist failed ({}): {}{}",
                self.exe.display(),
                o.status,
                String::from_utf8_lossy(&o.stdout).trim(),
                String::from_utf8_lossy(&o.stderr).trim()
            ));
        }
        let netlist =
            std::fs::read_to_string(&out).map_err(|e| format!("read the exported netlist: {e}"))?;
        let copied = files
            .into_iter()
            .filter(|f| project.dir.join(f).is_file())
            .collect();
        Ok(Export { netlist, copied })
    }
}

/// Libraries a project's `sym-lib-table` names inside the project
/// (`${KIPRJMOD}/...`), as relative paths.
fn project_libraries(table: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(table) else {
        return Vec::new();
    };
    let Ok(sx) = sexp::parse(&text) else {
        return Vec::new();
    };
    sx.children("lib")
        .filter_map(|l| l.value_of("uri"))
        .filter_map(|u| u.strip_prefix("${KIPRJMOD}/"))
        .filter(|r| !r.split('/').any(|p| p == ".."))
        .map(str::to_string)
        .collect()
}

static SEQ: AtomicU32 = AtomicU32::new(0);

/// A temporary directory removed on drop.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(base: &Path) -> Result<TempDir, String> {
        for _ in 0..1000 {
            let n = SEQ.fetch_add(1, Ordering::Relaxed);
            let p = base.join(format!("bb-kicad-{}-{n}", std::process::id()));
            match std::fs::create_dir(&p) {
                Ok(()) => return Ok(TempDir { path: p }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("create {}: {e}", p.display())),
            }
        }
        Err(format!(
            "cannot create a temporary directory in {}",
            base.display()
        ))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
