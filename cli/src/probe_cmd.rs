//! `bb probe` (ADR-007 D7): list the probe-able nets and aliases of a
//! project, or add probes to `bb.toml` `[probes]` (format-preserving).

use std::path::PathBuf;
use std::process::ExitCode;

use bbcore::board::Board;

use crate::import;
use crate::project::{self, Project, PROJECT_FILE};

const USAGE: &str = "\
usage: bb probe [--list] [net:<name> ...] [--project <dir>]

  --list        the probe-able nets (display name, SPICE name, leaf name) and
                the test-point aliases, from board.bbm
  net:<name>    add a probe to bb.toml [probes] nets (a net name, a leaf name,
                an alias or a bound MCU pin, resolved by ADR-007 D7's rules)";

pub fn main(args: &[String]) -> ExitCode {
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("bb probe: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let mut list = false;
    let mut dir: Option<PathBuf> = None;
    let mut add: Vec<String> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--list" => list = true,
            "--project" => dir = Some(PathBuf::from(it.next().ok_or("--project needs a value")?)),
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            s => match s.strip_prefix("net:") {
                Some(n) if !n.is_empty() => add.push(n.to_string()),
                _ => return Err(format!("{s}: expected net:<name>\n{USAGE}")),
            },
        }
    }
    let dir = import::project_dir(dir.as_deref())?;
    let p = Project::read(&dir)?;
    let bbm = std::fs::read_to_string(dir.join(import::BOARD_FILE))
        .map_err(|e| format!("read {}: {e}: run `bb open` first", import::BOARD_FILE))?;
    let board = Board::parse(&bbm)?;
    if list || add.is_empty() {
        for n in &board.nets {
            let kicad = if board.boards.len() > 1 {
                n.name.split_once(':').map(|x| x.1).unwrap_or(&n.name)
            } else {
                &n.name
            };
            let leaf = kicad.rsplit('/').next().unwrap_or(kicad);
            println!(
                "net\t{}\t{}\tspice {}\tleaf {}{}",
                n.name,
                n.board,
                n.spice,
                leaf,
                if n.unconnected { "\t(unconnected)" } else { "" }
            );
        }
        for a in &board.probes {
            println!("alias\t{}\t-> {}", a.alias, a.net);
        }
    }
    if add.is_empty() {
        return Ok(());
    }
    let problems = {
        let mut q = p.clone();
        q.probes = add.clone();
        crate::elab::check_names(&q, &board)
    };
    if !problems.is_empty() {
        return Err(problems.join("; "));
    }
    let mut nets = p.probes.clone();
    for n in add {
        println!(
            "bb probe: {n} -> {}",
            crate::elab::describe_probe(&board, &p.simulate, &n)?
        );
        if !nets.contains(&n) {
            nets.push(n);
        }
    }
    let toml_path = dir.join(PROJECT_FILE);
    let text = std::fs::read_to_string(&toml_path)
        .map_err(|e| format!("read {}: {e}", toml_path.display()))?;
    let body = format!(
        "nets = [{}]\n",
        nets.iter()
            .map(|n| bbpkg::toml::quote(n))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let text = project::set_table(&text, "[probes]", &body);
    Project::parse(&text, &dir)
        .map_err(|e| format!("internal: the edited bb.toml does not parse: {e}"))?;
    std::fs::write(&toml_path, text).map_err(|e| format!("write {}: {e}", toml_path.display()))?;
    println!(
        "bb probe: [probes] nets = {nets:?}; wrote {}",
        toml_path.display()
    );
    Ok(())
}
