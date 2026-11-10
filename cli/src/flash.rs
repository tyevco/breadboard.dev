//! `bb flash` (ADR-007 D7, ADR-008 D5): associate a firmware image with an
//! MCU instance. Breadboard simulates, so "flash" means "record": it checks
//! that the instance's package has a behavioural layer whose engine accepts
//! the image kind (Phase 1: `renode`, `elf`), computes the image hash, and
//! writes `[targets."<board>:<ref>"]` into `bb.toml` with a
//! format-preserving edit. No device is touched.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use bbcore::board::Board;

use crate::import;
use crate::names::Instances;
use crate::project::{self, Image, Project, PROJECT_FILE};

const USAGE: &str = "\
usage: bb flash --target <instance> <file.elf> [--kind elf] [--project <dir>]

  Records the image as the firmware of <instance> (U10, or main:U10) in
  bb.toml [targets], with its sha256 (ADR-007 D7). Reads board.bbm (run
  `bb open` first). bb run checks the hash.";

pub fn main(args: &[String]) -> ExitCode {
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("bb flash: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let mut target = None;
    let mut kind = "elf".to_string();
    let mut dir: Option<PathBuf> = None;
    let mut file: Option<PathBuf> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--target" => target = Some(it.next().ok_or("--target needs a value")?.clone()),
            "--kind" => kind = it.next().ok_or("--kind needs a value")?.clone(),
            "--project" => dir = Some(PathBuf::from(it.next().ok_or("--project needs a value")?)),
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            s if s.starts_with('-') => return Err(format!("unknown option {s}\n{USAGE}")),
            s => {
                if file.replace(PathBuf::from(s)).is_some() {
                    return Err(format!("one image at a time\n{USAGE}"));
                }
            }
        }
    }
    let target = target.ok_or_else(|| format!("--target is required\n{USAGE}"))?;
    let file = file.ok_or_else(|| format!("an image file is required\n{USAGE}"))?;
    if kind != "elf" {
        return Err(format!(
            "--kind {kind}: Phase 1 implements elf (bin, hex, uf2 and flash are reserved, ADR-007 D5)"
        ));
    }
    let dir = import::project_dir(dir.as_deref())?;
    let p = Project::read(&dir)?;
    let bbm = std::fs::read_to_string(dir.join(import::BOARD_FILE))
        .map_err(|e| format!("read {}: {e}: run `bb open` first", import::BOARD_FILE))?;
    let board = Board::parse(&bbm)?;
    let insts = Instances {
        items: board
            .instances
            .iter()
            .map(|i| {
                (
                    i.board.as_str(),
                    i.reference.as_str(),
                    i.value_text.as_str(),
                )
            })
            .collect(),
    };
    let (b, r) = insts.resolve(&target)?;
    let inst = board.instance(&b, &r).expect("resolved");
    let pr = inst.package.as_ref().ok_or_else(|| {
        format!(
            "{b}:{r} has no package ({}); it cannot run firmware",
            inst.status
        )
    })?;
    let pkgs = import::Packages::load(&p.search_path())?;
    let pkg = pkgs.get(&pr.id)?;
    match pkg.part.behavioral.as_ref() {
        Some(beh) if beh.engine == "renode" => {}
        _ => {
            return Err(format!(
                "{b}:{r}: package {} has no behavioural layer whose engine takes an elf image",
                pr.id
            ))
        }
    }
    let bytes = std::fs::read(&file).map_err(|e| format!("read {}: {e}", file.display()))?;
    if !bytes.starts_with(b"\x7fELF") {
        return Err(format!("{}: not an ELF file", file.display()));
    }
    let hash = format!("sha256:{}", bbpkg::sha256::hex_digest(&bytes));
    // The image path relative to the project (no absolute paths in bb.toml
    // when the image is inside it).
    let abs_dir = std::fs::canonicalize(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let abs_file = std::fs::canonicalize(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    let rel = bbpkg::lock::relative_path(&abs_dir, &abs_file);
    let key = format!("{b}:{r}");
    let toml_path = dir.join(PROJECT_FILE);
    let text = std::fs::read_to_string(&toml_path)
        .map_err(|e| format!("read {}: {e}", toml_path.display()))?;
    // Drop an entry written under another spelling of the same instance.
    let mut text = text;
    for (k, _) in &p.targets {
        if *k != key && insts.resolve(k).ok() == Some((b.clone(), r.clone())) {
            return Err(format!(
                "bb.toml already has [targets.{k:?}] for {key}; remove it or use that name"
            ));
        }
    }
    text = project::set_target(
        &text,
        &key,
        &[Image {
            file: rel.clone(),
            kind,
            hash: hash.clone(),
            offset: None,
        }],
    );
    Project::parse(&text, Path::new(&dir))
        .map_err(|e| format!("internal: the edited bb.toml does not parse: {e}"))?;
    std::fs::write(&toml_path, text).map_err(|e| format!("write {}: {e}", toml_path.display()))?;
    println!(
        "bb flash: {key} <- {rel} ({hash}); wrote {}",
        toml_path.display()
    );
    Ok(())
}
