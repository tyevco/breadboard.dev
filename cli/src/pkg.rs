//! `bb pkg`: package tools (ADR-006).
//!
//! - `bb pkg lint [<dir>...]`: load and lint every package under the
//!   directories (default: the first-party `packages/`).
//! - `bb pkg test [<dir>...] [--record] [--report <file>]`: run every test
//!   vector through `NgspiceEngine` at two grant partitions (one cell,
//!   1 ms), at the vector's (lattice, option set) and at every recorded
//!   qualification (D6); `--record` writes the passing pairs as
//!   `[[analog.qualified]]`; `--report` writes one JSON line per vector run
//!   (every measured value and a sha256 of every sample's bits), so two
//!   packages' results can be compared byte for byte (ADR-006 G8).
//! - `bb pkg migrate [<dir>...]`: rewrite packages to the current schema
//!   majors (D11).
//! - `bb pkg tree <dir>`: a package's tree hash (D8).
//! - `bb pkg value [--kicad R|C|L] <text>...`: parse values (D5) and print
//!   them in integer sub-units, one JSON object per line.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use bbcore::Time;
use bbpkg::lint::{self, Level};
use bbpkg::package::{self, Package};
use bbpkg::pkgtest::{self, Vector};
use bbpkg::schema::Registry;
use bbpkg::value::{self, Dim, PassiveKind};

use crate::platforms;

const USAGE: &str = "\
usage: bb pkg lint [<dir>...]                 lint packages (default: first-party packages/)
       bb pkg test [<dir>...] [--record] [--report <file>]
                                              run test vectors through NgspiceEngine (ADR-006 D6)
       bb pkg migrate [<dir>...]              rewrite packages to the current schema majors
       bb pkg tree <package dir>              print a package's tree hash
       bb pkg value [--kicad R|C|L] [--dim voltage|time|resistance|capacitance|inductance|current|temperature|count|percent] <text>...";

pub fn main(args: &[String]) -> ExitCode {
    let r = match args.first().map(String::as_str) {
        Some("lint") => lint_cmd(&args[1..]),
        Some("test") => test_cmd(&args[1..]),
        Some("migrate") => migrate_cmd(&args[1..]),
        Some("tree") => tree_cmd(&args[1..]),
        Some("value") => value_cmd(&args[1..]),
        Some("-h" | "--help" | "help") => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        _ => Err(USAGE.to_string()),
    };
    match r {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("bb pkg: {e}");
            ExitCode::FAILURE
        }
    }
}

fn dirs(args: &[String]) -> Vec<PathBuf> {
    let mut d: Vec<PathBuf> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--report" {
            it.next();
        } else if !a.starts_with('-') {
            d.push(PathBuf::from(a));
        }
    }
    if d.is_empty() {
        vec![bbpkg::first_party_dir()]
    } else {
        d
    }
}

fn packages(args: &[String]) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    for d in dirs(args) {
        let found = package::discover(&d)?;
        if found.is_empty() {
            return Err(format!("{}: no package (no part.toml) found", d.display()));
        }
        out.extend(found);
    }
    Ok(out)
}

/// Is `dir` inside breadboard.dev's first-party `packages/`?
fn is_first_party(dir: &Path) -> bool {
    match (
        std::fs::canonicalize(dir),
        std::fs::canonicalize(bbpkg::first_party_dir()),
    ) {
        (Ok(d), Ok(fp)) => d.starts_with(fp),
        _ => false,
    }
}

fn lint_cmd(args: &[String]) -> Result<bool, String> {
    let reg = Registry::current();
    let pl = platforms::ids();
    let mut errors = 0;
    let mut warnings = 0;
    let mut n = 0;
    for dir in packages(args)? {
        n += 1;
        let pkg = match package::load(&dir, &reg) {
            Ok(p) => p,
            Err(e) => {
                println!("ERROR   {}: {e}", dir.display());
                errors += 1;
                continue;
            }
        };
        // A module's chip package: in the module's own package root
        // (<root>/<namespace>/<name>), else the first-party one.
        let chip = match &pkg.part.module {
            Some(m) => [
                dir.parent().and_then(Path::parent).map(Path::to_path_buf),
                Some(bbpkg::first_party_dir()),
            ]
            .into_iter()
            .flatten()
            .map(|r| r.join(&m.chip))
            .find(|d| d.join("part.toml").is_file())
            .and_then(|d| package::load(&d, &reg).ok()),
            None => None,
        };
        let ctx = lint::Context {
            platforms: &pl,
            devices: &[],
            first_party: is_first_party(&dir),
            chip: chip.as_ref(),
        };
        let f = lint::lint(&pkg, &ctx, &reg);
        for x in &f {
            match x.level {
                Level::Error => {
                    errors += 1;
                    println!("ERROR   {} ({}): {}", pkg.part.id, dir.display(), x.msg);
                }
                Level::Warning => {
                    warnings += 1;
                    println!("warning {} ({}): {}", pkg.part.id, dir.display(), x.msg);
                }
            }
        }
        if f.is_empty() {
            println!(
                "ok      {} {} ({})",
                pkg.part.id,
                pkg.part.version,
                dir.display()
            );
        }
    }
    println!("bb pkg lint: {n} packages, {errors} errors, {warnings} warnings");
    Ok(errors == 0)
}

/// One (lattice, option set) a view is run under.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Pair {
    lattice: Time,
    lattice_text: String,
    options: String,
}

fn test_cmd(args: &[String]) -> Result<bool, String> {
    let record = args.iter().any(|a| a == "--record");
    if let Some(bad) = args
        .iter()
        .find(|a| a.starts_with('-') && !matches!(a.as_str(), "--record" | "--report"))
    {
        return Err(format!("unknown option {bad}"));
    }
    let mut report: Option<std::fs::File> = match args.iter().position(|a| a == "--report") {
        Some(i) => {
            let f = args.get(i + 1).ok_or("--report needs a file")?;
            Some(std::fs::File::create(f).map_err(|e| format!("--report {f}: {e}"))?)
        }
        None => None,
    };
    let reg = Registry::current();
    let mut ok_all = true;
    let mut nvec = 0;
    for dir in packages(args)? {
        let pkg = package::load(&dir, &reg)?;
        let Some(tdir) = &pkg.part.tests_dir else {
            println!("skip    {}: no [tests]", pkg.part.id);
            continue;
        };
        let vectors = pkgtest::load_vectors(&pkg, &dir.join(tdir), &reg)
            .map_err(|e| format!("{}: {e}", pkg.part.id))?;
        let mut record_views: Vec<(String, Vec<Pair>)> = Vec::new();
        for view in &pkg.part.analog {
            let vs: Vec<&Vector> = vectors.iter().filter(|v| v.view == view.name).collect();
            if vs.is_empty() {
                continue;
            }
            // The pairs: each vector's own, and every recorded one.
            let mut pairs: Vec<Pair> = Vec::new();
            for q in &view.qualified {
                let fs = value::parse_quantity(&q.lattice, Dim::Time).map_err(|e| e.to_string())?;
                let p = Pair {
                    lattice: Time::from_fs(u64::try_from(fs).map_err(|_| "lattice")?),
                    lattice_text: q.lattice.clone(),
                    options: q.options.clone(),
                };
                if !pairs.contains(&p) {
                    pairs.push(p);
                }
            }
            let mut passed_pairs: Vec<Pair> = Vec::new();
            let mut view_ok = true;
            let mut tried: Vec<Pair> = pairs.clone();
            for v in &vs {
                let own = Pair {
                    lattice: v.lattice,
                    lattice_text: v.lattice.to_string(),
                    options: v.options.clone(),
                };
                if !tried
                    .iter()
                    .any(|p| p.lattice == own.lattice && p.options == own.options)
                {
                    tried.push(own);
                }
            }
            for pair in &tried {
                let mut pair_ok = true;
                for v in &vs {
                    nvec += 1;
                    let ok = run_vector(&pkg, view, v, pair, report.as_mut())?;
                    pair_ok &= ok;
                }
                // D6: a set other than phase0 is admissible only if every
                // vector also runs under phase0 and agrees within tolerance.
                if pair.options != bbpkg::optsets::DEFAULT && pair_ok {
                    for v in &vs {
                        let a =
                            pkgtest::run(&pkg, view, v, &pair.options, pair.lattice, pair.lattice)?;
                        let b = pkgtest::run(
                            &pkg,
                            view,
                            v,
                            bbpkg::optsets::DEFAULT,
                            pair.lattice,
                            pair.lattice,
                        )
                        .map_err(|e| {
                            format!(
                                "{}: under phase0: {e} (so {} cannot be qualified, D6)",
                                v.file.display(),
                                pair.options
                            )
                        })?;
                        for ((x, ma), mb) in v.expects.iter().zip(&a.measured).zip(&b.measured) {
                            let agree = matches!((ma.value, mb.value), (Some(p), Some(q)) if (p - q).abs() <= x.tol);
                            if !agree {
                                println!("FAIL    {} view {}: {} under {} does not agree with phase0 within {} (D6)", pkg.part.id, view.name, v.file.display(), pair.options, x.tol);
                                pair_ok = false;
                            }
                        }
                    }
                }
                let recorded = view.qualified.iter().any(|q| {
                    q.options == pair.options
                        && value::parse_quantity(&q.lattice, Dim::Time).ok()
                            == Some(i128::from(pair.lattice.as_fs()))
                });
                if pair_ok {
                    passed_pairs.push(pair.clone());
                    println!(
                        "PASS    {} view {}: qualified at ({}, {}){}",
                        pkg.part.id,
                        view.name,
                        pair.lattice,
                        pair.options,
                        if recorded { "" } else { " (not recorded)" }
                    );
                } else {
                    view_ok = false;
                    println!(
                        "FAIL    {} view {}: not qualified at ({}, {}){}",
                        pkg.part.id,
                        view.name,
                        pair.lattice,
                        pair.options,
                        if recorded {
                            ": a RECORDED qualification fails"
                        } else {
                            ""
                        }
                    );
                }
            }
            ok_all &= view_ok;
            if record && view_ok {
                record_views.push((view.name.clone(), passed_pairs));
            }
        }
        if record && !record_views.is_empty() {
            write_qualified(&pkg, &record_views)?;
        }
    }
    println!(
        "bb pkg test: {nvec} vector runs, {}",
        if ok_all { "all passed" } else { "FAILURES" }
    );
    Ok(ok_all)
}

/// Run one vector at both partitions; print and return pass.
fn run_vector(
    pkg: &Package,
    view: &package::View,
    v: &Vector,
    pair: &Pair,
    report: Option<&mut std::fs::File>,
) -> Result<bool, String> {
    let name = v
        .file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let one = pkgtest::run(pkg, view, v, &pair.options, pair.lattice, pair.lattice)
        .map_err(|e| format!("{} {name}: {e}", pkg.part.id))?;
    let ms = Time::from_ms(1);
    let mq = if ms.as_fs().is_multiple_of(pair.lattice.as_fs()) {
        ms
    } else {
        pair.lattice
    };
    let long = pkgtest::run(pkg, view, v, &pair.options, pair.lattice, mq)
        .map_err(|e| format!("{} {name}: {e}", pkg.part.id))?;
    let mut ok = true;
    if one.samples != long.samples || one.measured != long.measured {
        println!(
            "FAIL    {} {name}: results differ between the one-cell and the {mq} partition",
            pkg.part.id
        );
        ok = false;
    }
    for (x, m) in v.expects.iter().zip(&one.measured) {
        let got = m
            .value
            .map(|g| pkgtest::show(g, &x.meas))
            .unwrap_or_else(|| "no crossing".into());
        let want = pkgtest::show(x.value, &x.meas);
        let tol = pkgtest::show(x.tol, &x.meas);
        println!(
            "{}    {} {name} ({}, {}): {}: {got}, expected {want} +- {tol}",
            if m.pass { "ok  " } else { "FAIL" },
            pkg.part.id,
            pair.lattice,
            pair.options,
            x.meas.describe()
        );
        ok &= m.pass;
    }
    println!(
        "        {} {name}: {} one-cell grants, {} grants at {mq}; partitions identical: {}",
        pkg.part.id,
        one.grants,
        long.grants,
        one.samples == long.samples
    );
    if let Some(f) = report {
        use std::io::Write;
        for (part, r) in [("one_cell", &one), ("1ms", &long)] {
            writeln!(f, "{}", report_line(pkg, view, &name, pair, part, r))
                .map_err(|e| format!("--report: {e}"))?;
        }
    }
    Ok(ok)
}

/// One `--report` line: the vector's identity (no directory, so two copies
/// of a package compare equal), every measured value (µV or fs, or null)
/// and the sha256 of every sample's `f64` bits.
fn report_line(
    pkg: &Package,
    view: &package::View,
    name: &str,
    pair: &Pair,
    partition: &str,
    r: &pkgtest::RunResult,
) -> String {
    let mut h = bbpkg::sha256::Sha256::new();
    for (net, s) in &r.samples {
        h.update(net.as_bytes());
        h.update(&[0]);
        for (t, bits) in s {
            h.update(&t.to_le_bytes());
            h.update(&bits.to_le_bytes());
        }
    }
    let measured: Vec<String> = r
        .measured
        .iter()
        .map(|m| {
            m.value
                .map(|v| v.to_string())
                .unwrap_or_else(|| "null".into())
        })
        .collect();
    format!(
        "{{\"lattice_fs\":{},\"measured\":[{}],\"options\":{},\"package\":{},\"partition\":{},\"samples_sha256\":{},\"vector\":{},\"view\":{}}}",
        pair.lattice.as_fs(),
        measured.join(","),
        json_str(&pair.options),
        json_str(&pkg.part.id),
        json_str(partition),
        json_str(&bbpkg::sha256::hex(&h.finish())),
        json_str(name),
        json_str(&view.name)
    )
}

/// Rewrite the `[[analog.qualified]]` entries of the given views in
/// `part.toml`: each view's old entries are removed and the passing pairs
/// written at the end of its `[[analog]]` block. Other text is kept.
fn write_qualified(pkg: &Package, views: &[(String, Vec<Pair>)]) -> Result<(), String> {
    let path = pkg.dir.join("part.toml");
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let header = |l: &str| l.trim_start().starts_with('[');
    // The [[analog]] blocks in file order.
    let mut out = String::new();
    let mut i = 0;
    let mut view_idx = 0;
    while i < lines.len() {
        let l = lines[i];
        if l.trim() == "[[analog]]" {
            let view = &pkg.part.analog[view_idx];
            view_idx += 1;
            // The block: until the next header that is not [analog.*] or
            // [[analog.*]].
            let mut j = i + 1;
            while j < lines.len() {
                let t = lines[j].trim();
                if header(t) && !(t.starts_with("[analog.") || t.starts_with("[[analog.")) {
                    break;
                }
                j += 1;
            }
            let block = &lines[i..j];
            let new_pairs = views.iter().find(|(n, _)| *n == view.name);
            // Copy the block without its qualified entries when rewriting.
            let mut k = 0;
            let mut body = String::new();
            while k < block.len() {
                let t = block[k].trim();
                if new_pairs.is_some() && t == "[[analog.qualified]]" {
                    k += 1;
                    while k < block.len() && !header(block[k].trim()) && !block[k].trim().is_empty()
                    {
                        k += 1;
                    }
                    while k < block.len() && block[k].trim().is_empty() {
                        k += 1;
                    }
                    continue;
                }
                body.push_str(block[k]);
                k += 1;
            }
            if let Some((_, pairs)) = new_pairs {
                let trimmed = body.trim_end_matches('\n').to_string();
                body = format!("{trimmed}\n");
                for p in pairs {
                    body.push_str(&format!(
                        "\n[[analog.qualified]]   # bb pkg test --record\nlattice = \"{}\"\noptions = \"{}\"\n",
                        canonical_time(p),
                        p.options
                    ));
                }
                if j < lines.len() {
                    body.push('\n');
                }
            }
            out.push_str(&body);
            i = j;
        } else {
            out.push_str(l);
            i += 1;
        }
    }
    // The result must still parse to the same package plus the records.
    let (part, _) = package::parse_part(&out, &Registry::current())
        .map_err(|e| format!("--record produced an invalid part.toml: {e}"))?;
    for (n, pairs) in views {
        let v = part
            .analog
            .iter()
            .find(|v| v.name == *n)
            .ok_or("--record lost a view")?;
        if v.qualified.len() != pairs.len() {
            return Err("--record: internal error rewriting part.toml".into());
        }
    }
    if out != text {
        std::fs::write(&path, out).map_err(|e| format!("write {}: {e}", path.display()))?;
        println!(
            "record  {}: wrote [[analog.qualified]] for {}",
            pkg.part.id,
            views
                .iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    Ok(())
}

/// The lattice as written in a record: the recorded text when there is
/// one, else `<n>us`/`<n>ns`.
fn canonical_time(p: &Pair) -> String {
    if !p.lattice_text.is_empty() && value::parse_quantity(&p.lattice_text, Dim::Time).is_ok() {
        return p.lattice_text.clone();
    }
    let fs = p.lattice.as_fs();
    if fs.is_multiple_of(1_000_000_000) {
        format!("{}us", fs / 1_000_000_000)
    } else {
        format!("{}ns", fs / 1_000_000)
    }
}

fn migrate_cmd(args: &[String]) -> Result<bool, String> {
    let reg = Registry::current();
    let mut changed = 0;
    for dir in packages(args)? {
        let mut files = vec![(dir.join("part.toml"), "bb-part")];
        let part_text =
            std::fs::read_to_string(dir.join("part.toml")).map_err(|e| e.to_string())?;
        if let Ok(t) = bbpkg::toml::parse(&part_text) {
            if let Some(f) = t
                .get("pins")
                .and_then(|p| p.as_table())
                .and_then(|p| p.get("file"))
                .and_then(|f| f.as_str())
            {
                files.push((dir.join(f), "bb-pins"));
            }
            if let Some(d) = t
                .get("tests")
                .and_then(|p| p.as_table())
                .and_then(|p| p.get("dir"))
                .and_then(|f| f.as_str())
            {
                if let Ok(rd) = std::fs::read_dir(dir.join(d)) {
                    let mut v: Vec<PathBuf> = rd
                        .filter_map(|e| e.ok().map(|e| e.path()))
                        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
                        .collect();
                    v.sort();
                    files.extend(v.into_iter().map(|p| (p, "bb-pkgtest")));
                }
            }
        }
        for (f, schema) in files {
            let text =
                std::fs::read_to_string(&f).map_err(|e| format!("read {}: {e}", f.display()))?;
            let t = bbpkg::toml::parse(&text).map_err(|e| format!("{}: {e}", f.display()))?;
            let id = t
                .get("schema")
                .and_then(|s| s.as_str())
                .ok_or_else(|| format!("{}: no schema", f.display()))?;
            match reg
                .migrate(schema, id, &text)
                .map_err(|e| format!("{}: {e}", f.display()))?
            {
                None => {}
                Some(new) => {
                    println!(
                        "migrate {}: {id} -> {}",
                        f.display(),
                        reg.current_id(schema)
                    );
                    std::fs::write(&f, new).map_err(|e| e.to_string())?;
                    changed += 1;
                }
            }
        }
    }
    println!("bb pkg migrate: {changed} files rewritten");
    Ok(true)
}

fn tree_cmd(args: &[String]) -> Result<bool, String> {
    let [dir] = args else {
        return Err("usage: bb pkg tree <package dir>".into());
    };
    println!("{}", bbpkg::lock::tree_hash(Path::new(dir))?);
    Ok(true)
}

fn json_str(s: &str) -> String {
    serde_json::to_string(s).expect("string")
}

fn value_cmd(args: &[String]) -> Result<bool, String> {
    let mut kicad: Option<PassiveKind> = None;
    let mut dim: Option<Dim> = None;
    let mut it = args.iter();
    let mut texts = Vec::new();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--kicad" => {
                kicad = Some(match it.next().map(String::as_str) {
                    Some("R") => PassiveKind::Resistor,
                    Some("C") => PassiveKind::Capacitor,
                    Some("L") => PassiveKind::Inductor,
                    k => return Err(format!("--kicad {k:?}: expected R, C or L")),
                })
            }
            "--dim" => {
                dim = Some(match it.next().map(String::as_str) {
                    Some("voltage") => Dim::Voltage,
                    Some("time") => Dim::Time,
                    Some("resistance") => Dim::Resistance,
                    Some("capacitance") => Dim::Capacitance,
                    Some("inductance") => Dim::Inductance,
                    Some("current") => Dim::Current,
                    Some("temperature") => Dim::Temperature,
                    Some("count") => Dim::Count,
                    Some("percent") => Dim::Percent,
                    d => return Err(format!("--dim {d:?}: unknown dimension")),
                })
            }
            t => texts.push(t.to_string()),
        }
    }
    let mut ok = true;
    for t in &texts {
        let line = match (kicad, dim) {
            (Some(k), _) => match value::parse_kicad_value(t, k) {
                Ok(v) => {
                    let opt =
                        |x: Option<i128>| x.map(|n| n.to_string()).unwrap_or_else(|| "null".into());
                    let opts = |x: &Option<String>| {
                        x.as_deref().map(json_str).unwrap_or_else(|| "null".into())
                    };
                    format!(
                        "{{\"dielectric\":{},\"power_uW\":{},\"rating_uV\":{},\"spice\":{},\"technology\":{},\"text\":{},\"tolerance_ppm\":{},\"unit\":{},\"value\":{}}}",
                        opts(&v.dielectric),
                        opt(v.power_uw),
                        opt(v.rating_uv),
                        json_str(&value::spice_number(value::to_decimal(v.value, k.dim()))),
                        opts(&v.technology),
                        json_str(t),
                        opt(v.tolerance_ppm),
                        json_str(k.dim().subunit()),
                        v.value
                    )
                }
                Err(e) => {
                    ok = false;
                    format!("{{\"error\":{},\"text\":{}}}", json_str(&e.0), json_str(t))
                }
            },
            (None, Some(d)) => match value::parse_quantity(t, d) {
                Ok(v) => format!(
                    "{{\"text\":{},\"unit\":{},\"value\":{v}}}",
                    json_str(t),
                    json_str(d.subunit())
                ),
                Err(e) => {
                    ok = false;
                    format!("{{\"error\":{},\"text\":{}}}", json_str(&e.0), json_str(t))
                }
            },
            (None, None) => match value::parse_number(t) {
                Ok(v) => format!(
                    "{{\"spice\":{},\"text\":{}}}",
                    json_str(&value::spice_number(v)),
                    json_str(t)
                ),
                Err(e) => {
                    ok = false;
                    format!("{{\"error\":{},\"text\":{}}}", json_str(&e.0), json_str(t))
                }
            },
        };
        println!("{line}");
    }
    Ok(ok)
}
