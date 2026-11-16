//! `bb test` (ADR-010 D8-D9, Amendment 5; `spec/test.md`): a `bb-test/0`
//! file of cases, each a `bb run` with its assertions, its expectations and
//! its comparisons, run in parallel (on a warm Renode pool), then
//! `<out>/test.json` (canonical JSON, deterministic) and `<out>/junit.xml`
//! (the JUnit subset of `spec/schema/junit-subset.xsd`; wall times only
//! here).
//!
//! A case is an ordinary `bb run` of this `bb`, in a child process: its
//! outputs are `<out>/<case>/` exactly as `bb run --out` writes them, and
//! nothing about them depends on `bb test` (the pool rule holds: results
//! never depend on leasing). The exit status is 0 if and only if every
//! outcome equals its expectation and every comparison holds.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use bbcore::cjson::Json;
use bbcore::event::AssertOutcome;
use bbcore::{Event, Time};
use bbpkg::obj::Obj;
use bbpkg::toml::{self, Table, Value};

pub const USAGE: &str = "\
usage: bb test <file.toml> --out <dir> [--jobs N] [--pool auto|env|off]

  Runs the cases of a bb-test/0 file (ADR-010 D8; spec/test.md), each as a
  `bb run` into <out>/<case>/ (its log <out>/<case>.log; a case with
  reference = true also <out>/<case>-ref/, its one-cell reference), then
  writes <out>/test.json (canonical JSON: no wall times, no paths; the
  outcomes, values, the run.json and output hashes) and <out>/junit.xml
  (one testsuite per case, one testcase per assertion and per comparison;
  wall times only here). Exits 0 if and only if every outcome equals its
  expectation (an unexpected pass of a negative control is a failure) and
  every golden, reference and same_as comparison holds.

  --out    the output directory (its <case> subdirectories are replaced)
  --jobs   cases run at a time (default: the number of runs, at most 4)
  --pool   auto (default): the warm Renode pool in $BB_RENODE_POOL, else
           one bb test starts with --jobs slots and stops after the runs;
           env: $BB_RENODE_POOL if set, else every Renode fresh; off: every
           Renode fresh (each run with --renode-pool off). The outputs are
           the same either way (ADR-004 Part B)";

/// The schema of a test file.
pub const SCHEMA: &str = "bb-test";
/// The schema of `test.json`.
const RESULT_SCHEMA: &str = "bb-test-result/0";

/// What a case runs: a project (`--project`) or a board file (`--netlist`).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Base {
    Project(String),
    Board(String),
}

/// The `bb run` options a case (or `[defaults]`) sets. Paths are as
/// written, relative to the test file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Opts {
    duration: Option<String>,
    fidelity: Option<String>,
    bench: Option<String>,
    targets: Option<Vec<(String, String)>>,
    probes: Option<Vec<String>>,
    sample_rate: Option<String>,
    assert_files: Option<Vec<String>>,
    max_quantum: Option<String>,
    renode_quantum: Option<String>,
    engine_order: Option<String>,
    lookahead: Option<String>,
    adc_push: Option<String>,
    seed: Option<i64>,
    allow_unqualified: Option<Vec<String>>,
}

impl Opts {
    /// `self` over `d` (a case over `[defaults]`); a list replaces.
    fn over(self, d: &Opts) -> Opts {
        Opts {
            duration: self.duration.or_else(|| d.duration.clone()),
            fidelity: self.fidelity.or_else(|| d.fidelity.clone()),
            bench: self.bench.or_else(|| d.bench.clone()),
            targets: self.targets.or_else(|| d.targets.clone()),
            probes: self.probes.or_else(|| d.probes.clone()),
            sample_rate: self.sample_rate.or_else(|| d.sample_rate.clone()),
            assert_files: self.assert_files.or_else(|| d.assert_files.clone()),
            max_quantum: self.max_quantum.or_else(|| d.max_quantum.clone()),
            renode_quantum: self.renode_quantum.or_else(|| d.renode_quantum.clone()),
            engine_order: self.engine_order.or_else(|| d.engine_order.clone()),
            lookahead: self.lookahead.or_else(|| d.lookahead.clone()),
            adc_push: self.adc_push.or_else(|| d.adc_push.clone()),
            seed: self.seed.or(d.seed),
            allow_unqualified: self
                .allow_unqualified
                .or_else(|| d.allow_unqualified.clone()),
        }
    }

    /// The lattice L of the case's runs (`renode_quantum`, default 10 us).
    fn lattice(&self) -> Result<Time, String> {
        match &self.renode_quantum {
            None => Ok(Time::from_us(10)),
            Some(s) => s
                .parse::<Time>()
                .map_err(|e| format!("renode_quantum: {e}")),
        }
    }
}

/// An expected (or reported) outcome of an assertion.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Outcome {
    Pass,
    Fail,
    NotEvaluable,
}

impl Outcome {
    fn parse(s: &str) -> Option<Outcome> {
        match s {
            "pass" => Some(Outcome::Pass),
            "fail" => Some(Outcome::Fail),
            "not_evaluable" => Some(Outcome::NotEvaluable),
            _ => None,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Outcome::Pass => "pass",
            Outcome::Fail => "fail",
            Outcome::NotEvaluable => "not_evaluable",
        }
    }
}

impl From<AssertOutcome> for Outcome {
    fn from(o: AssertOutcome) -> Outcome {
        match o {
            AssertOutcome::Pass => Outcome::Pass,
            AssertOutcome::Fail => Outcome::Fail,
            AssertOutcome::NotEvaluable => Outcome::NotEvaluable,
        }
    }
}

/// One `[[case]]`, with `[defaults]` applied.
#[derive(Clone, Debug)]
struct Case {
    name: String,
    base: Base,
    o: Opts,
    /// `asserts`: the assertions the case reports (each must give a
    /// record); `None`: every `assert` record of the run, in its order.
    asserts: Option<Vec<String>>,
    /// The `[[case.assert]]` tables as an assertion file's text.
    inline: Option<String>,
    /// The kind of every assertion the run has (bench, files, inline).
    kinds: BTreeMap<String, &'static str>,
    expect: BTreeMap<String, Outcome>,
    reference: bool,
    golden: Option<String>,
    same_as: Option<String>,
}

/// A parsed test file.
#[derive(Clone, Debug)]
struct TestFile {
    /// The directory every relative path is resolved against.
    dir: PathBuf,
    /// The file's name (test.json carries no path).
    label: String,
    sha256: String,
    cases: Vec<Case>,
}

fn name_ok(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))
}

fn strs_opt(o: &mut Obj, key: &str) -> Result<Option<Vec<String>>, String> {
    Ok(if o.has(key) { Some(o.strs(key)?) } else { None })
}

fn opts(o: &mut Obj) -> Result<Opts, String> {
    let at = o.path.clone();
    let p = |k: &str| {
        if at.is_empty() {
            k.to_string()
        } else {
            format!("{at}.{k}")
        }
    };
    let time = |v: Option<String>, k: &str| -> Result<Option<String>, String> {
        if let Some(s) = &v {
            s.parse::<Time>().map_err(|e| format!("{}: {e}", p(k)))?;
        }
        Ok(v)
    };
    let one_of = |v: Option<String>, k: &str, ok: &[&str]| -> Result<Option<String>, String> {
        match &v {
            Some(s) if !ok.contains(&s.as_str()) => {
                Err(format!("{}: {s:?}: expected {}", p(k), ok.join(" or ")))
            }
            _ => Ok(v),
        }
    };
    let targets = if o.has("targets") {
        let t = o.str_map("targets")?;
        if t.is_empty() {
            return Err(format!("{}: give at least one instance", p("targets")));
        }
        Some(t)
    } else {
        None
    };
    let seed = match o.get("seed") {
        None => None,
        Some(Value::Integer(n)) => Some(*n),
        Some(v) => {
            return Err(format!(
                "{}: expected an integer, got {}",
                p("seed"),
                v.type_name()
            ))
        }
    };
    Ok(Opts {
        duration: time(o.str_opt("for")?, "for")?,
        fidelity: one_of(o.str_opt("fidelity")?, "fidelity", &["fast", "pi"])?,
        bench: o.str_opt("bench")?,
        targets,
        probes: strs_opt(o, "probes")?,
        sample_rate: o.str_opt("sample_rate")?,
        assert_files: strs_opt(o, "assert_files")?,
        max_quantum: time(o.str_opt("max_quantum")?, "max_quantum")?,
        renode_quantum: time(o.str_opt("renode_quantum")?, "renode_quantum")?,
        engine_order: one_of(
            o.str_opt("engine_order")?,
            "engine_order",
            &["mcu-first", "analog-first"],
        )?,
        lookahead: one_of(o.str_opt("lookahead")?, "lookahead", &["on", "off"])?,
        adc_push: one_of(o.str_opt("adc_push")?, "adc_push", &["change", "every"])?,
        seed,
        allow_unqualified: strs_opt(o, "allow_unqualified")?,
    })
}

fn base(o: &mut Obj) -> Result<Option<Base>, String> {
    match (o.str_opt("project")?, o.str_opt("board")?) {
        (Some(_), Some(_)) => Err(format!(
            "{}: give project or board, not both",
            if o.path.is_empty() {
                "the file"
            } else {
                &o.path
            }
        )),
        (Some(p), None) => Ok(Some(Base::Project(p))),
        (None, Some(b)) => Ok(Some(Base::Board(b))),
        (None, None) => Ok(None),
    }
}

/// A TOML value written back as TOML (inline), for the `[[case.assert]]`
/// tables' assertion file. Floats are refused (assertion keys are
/// quantity strings, integers and booleans).
fn toml_value(v: &Value, at: &str) -> Result<String, String> {
    Ok(match v {
        Value::String(s) => toml::quote(s),
        Value::Integer(n) => n.to_string(),
        Value::Boolean(b) => b.to_string(),
        Value::Float(_) => {
            return Err(format!(
                "{at}: a float; write a quantity as a string (\"0.7V\")"
            ))
        }
        Value::Array(a) => format!(
            "[{}]",
            a.iter()
                .map(|x| toml_value(x, at))
                .collect::<Result<Vec<_>, _>>()?
                .join(", ")
        ),
        Value::Table(t) => format!(
            "{{ {} }}",
            t.iter()
                .map(|(k, x)| Ok(format!("{} = {}", toml_key(k), toml_value(x, at)?)))
                .collect::<Result<Vec<_>, String>>()?
                .join(", ")
        ),
    })
}

fn toml_key(k: &str) -> String {
    if !k.is_empty()
        && k.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        k.to_string()
    } else {
        toml::quote(k)
    }
}

/// `[[case.assert]]` tables as a `bb-bench/1` assertion file.
fn inline_file(case: &str, tables: &[Value]) -> Result<String, String> {
    let mut s = format!(
        "# bb test: the inline [[case.assert]] tables of case {case} (ADR-010 D8)\n\
         schema = \"bb-bench/1\"\n"
    );
    for (i, t) in tables.iter().enumerate() {
        let at = format!("case {case}: assert[{i}]");
        let t = t
            .as_table()
            .ok_or_else(|| format!("{at}: expected a table"))?;
        s.push_str("\n[[assert]]\n");
        for (k, v) in t {
            s.push_str(&format!("{} = {}\n", toml_key(k), toml_value(v, &at)?));
        }
    }
    Ok(s)
}

/// The kinds of a list of assertions, by name.
fn kinds_of(specs: &[bbcore::assert::Spec], into: &mut BTreeMap<String, &'static str>) {
    for s in specs {
        into.insert(s.name.clone(), s.kind.name());
    }
}

impl TestFile {
    fn read(path: &Path) -> Result<TestFile, String> {
        let text = fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let dir = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let label = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        TestFile::parse(&text, &dir, &label).map_err(|e| format!("{}: {e}", path.display()))
    }

    fn parse(text: &str, dir: &Path, label: &str) -> Result<TestFile, String> {
        let t: Table = toml::parse(text).map_err(|e| e.to_string())?;
        let mut o = Obj::new("", &t);
        let schema = o.str_req("schema")?;
        let reg = bbpkg::schema::Registry::current();
        reg.check(SCHEMA, &schema)?;
        let file_base = base(&mut o)?;
        let defaults = match o.table_opt("defaults")? {
            Some(mut d) => {
                let x = opts(&mut d)?;
                d.done()?;
                x
            }
            None => Opts::default(),
        };
        let mut cases: Vec<Case> = Vec::new();
        for mut c in o.tables("case")? {
            let at = c.path.clone();
            let name = c.str_req("name")?;
            if !name_ok(&name) || name.ends_with("-ref") {
                return Err(format!(
                    "{at}.name {name:?}: letters, digits and _ . - only, not ending in -ref \
                     (the reference run's directory)"
                ));
            }
            if cases.iter().any(|x| x.name == name) {
                return Err(format!("{at}.name {name:?}: given twice"));
            }
            let at = format!("case {name}");
            let b = base(&mut c)?
                .or_else(|| file_base.clone())
                .ok_or_else(|| format!("{at}: no project or board (in the case or the file)"))?;
            let o = opts(&mut c)?.over(&defaults);
            let asserts = strs_opt(&mut c, "asserts")?;
            let mut expect = BTreeMap::new();
            if let Some(mut e) = c.table_opt("expect")? {
                for k in e.keys() {
                    let v = e.str_req(&k)?;
                    let oc = Outcome::parse(&v).ok_or_else(|| {
                        format!("{at}: expect.{k} {v:?}: pass, fail or not_evaluable")
                    })?;
                    expect.insert(k, oc);
                }
                e.done()?;
            }
            let reference = c.bool_opt("reference")?.unwrap_or(false);
            let golden = c.str_opt("golden")?;
            let same_as = c.str_opt("same_as")?;
            // Inline assertions: checked now, written for the run.
            let mut kinds = BTreeMap::new();
            let inline = match c.get("assert") {
                None => None,
                Some(Value::Array(a)) => {
                    let txt = inline_file(&name, a)?;
                    let specs = crate::asserts::parse_file(&txt)
                        .map_err(|e| format!("{at}: [[case.assert]]: {e}"))?;
                    kinds_of(&specs, &mut kinds);
                    Some(txt)
                }
                Some(v) => {
                    return Err(format!(
                        "{at}.assert: expected an array of tables, got {}",
                        v.type_name()
                    ))
                }
            };
            c.done()?;
            // What the base allows.
            if o.duration.is_none() {
                return Err(format!("{at}: no for (in the case or [defaults])"));
            }
            match &b {
                Base::Board(_) => {
                    if o.bench.is_some() || o.allow_unqualified.is_some() {
                        return Err(format!(
                            "{at}: bench and allow_unqualified are for a project, not a board file"
                        ));
                    }
                    if o.fidelity.as_deref().is_some_and(|f| f != "fast") {
                        return Err(format!("{at}: a board file runs at fast only"));
                    }
                    if o.targets.as_ref().map_or(0, Vec::len) != 1 {
                        return Err(format!(
                            "{at}: a board file needs exactly one targets entry (<ref> = <elf>)"
                        ));
                    }
                    if o.probes.as_ref().is_none_or(Vec::is_empty) {
                        return Err(format!("{at}: a board file needs probes"));
                    }
                }
                Base::Project(_) => {}
            }
            // The kinds of the bench's and the files' assertions (G9 and
            // the names `asserts` and `expect` give).
            if let Some(bf) = &o.bench {
                let bench =
                    crate::project::Bench::read(&dir.join(bf)).map_err(|e| format!("{at}: {e}"))?;
                kinds_of(&bench.asserts, &mut kinds);
            }
            for f in o.assert_files.iter().flatten() {
                let (specs, _) =
                    crate::asserts::read_file(&dir.join(f)).map_err(|e| format!("{at}: {e}"))?;
                kinds_of(&specs, &mut kinds);
            }
            let reported: Vec<String> = match &asserts {
                Some(a) => a.clone(),
                None => kinds.keys().cloned().collect(),
            };
            for n in asserts.iter().flatten() {
                if !kinds.contains_key(n) {
                    return Err(format!(
                        "{at}: asserts names {n:?}, which no assertion of the case defines"
                    ));
                }
            }
            for n in expect.keys() {
                if !reported.contains(n) {
                    return Err(format!(
                        "{at}: expect names {n:?}, which the case does not report"
                    ));
                }
            }
            // ADR-010 G9: a negative control must show that the controlled
            // event occurred, with an event or count assertion expected to
            // pass, so it cannot pass vacuously.
            if expect.values().any(|x| *x == Outcome::Fail)
                && !reported.iter().any(|n| {
                    matches!(kinds.get(n), Some(&("count" | "event")))
                        && expect.get(n).copied().unwrap_or(Outcome::Pass) == Outcome::Pass
                })
            {
                return Err(format!(
                    "{at}: a negative control (an expected fail) needs a count or event assertion \
                     expected to pass, showing the controlled event occurred (ADR-010 G9)"
                ));
            }
            cases.push(Case {
                name,
                base: b,
                o,
                asserts,
                inline,
                kinds,
                expect,
                reference,
                golden,
                same_as,
            });
        }
        o.done()?;
        if cases.is_empty() {
            return Err("no [[case]]".into());
        }
        for c in &cases {
            if let Some(s) = &c.same_as {
                match cases.iter().find(|x| &x.name == s) {
                    None => return Err(format!("case {}: same_as {s:?}: no such case", c.name)),
                    Some(x) if x.name == c.name => {
                        return Err(format!("case {}: same_as names itself", c.name))
                    }
                    Some(x) if x.same_as.is_some() => {
                        return Err(format!(
                            "case {}: same_as {s:?}, which has a same_as itself",
                            c.name
                        ))
                    }
                    Some(_) => {}
                }
            }
        }
        Ok(TestFile {
            dir: dir.to_path_buf(),
            label: label.to_string(),
            sha256: format!("sha256:{}", bbpkg::sha256::hex_digest(text.as_bytes())),
            cases,
        })
    }
}

/// One `bb run` of a case.
struct Job {
    case: usize,
    reference: bool,
}

/// What a run gave.
#[derive(Clone, Debug)]
struct Ran {
    ok: bool,
    /// The last `bb:` error line of a failed run (for junit.xml only).
    error: String,
    wall: f64,
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// `bb run`'s arguments for a case (or its reference).
fn run_argv(
    tf: &TestFile,
    c: &Case,
    out: &Path,
    reference: bool,
    pool_off: bool,
    inline: Option<&Path>,
) -> Result<Vec<String>, String> {
    let d = &tf.dir;
    let o = &c.o;
    let mut a = vec!["run".to_string()];
    match &c.base {
        Base::Project(p) => {
            a.extend(["--project".into(), s(&d.join(p))]);
            for (inst, f) in o.targets.iter().flatten() {
                a.extend(["--image".into(), format!("{inst}={}", s(&d.join(f)))]);
            }
            if let Some(b) = &o.bench {
                a.extend(["--bench".into(), s(&d.join(b))]);
            }
            if let Some(f) = &o.fidelity {
                a.extend(["--fidelity".into(), f.clone()]);
            }
            for v in o.allow_unqualified.iter().flatten() {
                a.extend(["--allow-unqualified".into(), v.clone()]);
            }
        }
        Base::Board(b) => {
            a.extend(["--netlist".into(), s(&d.join(b))]);
            let (r, f) = &o.targets.as_ref().expect("checked")[0];
            a.extend([
                "--firmware".into(),
                s(&d.join(f)),
                "--target".into(),
                r.clone(),
            ]);
        }
    }
    a.extend(["--for".into(), o.duration.clone().expect("checked")]);
    for p in o.probes.iter().flatten() {
        a.extend(["--probe".into(), format!("net:{p}")]);
    }
    if let Some(r) = &o.sample_rate {
        a.extend(["--sample-rate".into(), r.clone()]);
    }
    for f in o.assert_files.iter().flatten() {
        a.extend(["--assert".into(), s(&d.join(f))]);
    }
    if let Some(f) = inline {
        a.extend(["--assert".into(), s(f)]);
    }
    if let Some(v) = &o.renode_quantum {
        a.extend(["--renode-quantum".into(), v.clone()]);
    }
    if let Some(v) = &o.engine_order {
        a.extend(["--engine-order".into(), v.clone()]);
    }
    if let Some(v) = &o.adc_push {
        a.extend(["--adc-push".into(), v.clone()]);
    }
    if let Some(v) = o.seed {
        a.extend(["--seed".into(), v.to_string()]);
    }
    if reference {
        // The one-cell reference (ADR-010 D8): every grant one lattice
        // cell, no lookahead.
        a.extend([
            "--max-quantum".into(),
            o.lattice()?.to_string(),
            "--lookahead".into(),
            "off".into(),
        ]);
    } else {
        if let Some(v) = &o.max_quantum {
            a.extend(["--max-quantum".into(), v.clone()]);
        }
        if let Some(v) = &o.lookahead {
            a.extend(["--lookahead".into(), v.clone()]);
        }
    }
    if pool_off {
        a.extend(["--renode-pool".into(), "off".into()]);
    }
    a.extend(["--out".into(), s(out)]);
    Ok(a)
}

/// The run directory of a case (`<out>/<case>`, or `<out>/<case>-ref`).
fn run_dir(out: &Path, c: &Case, reference: bool) -> PathBuf {
    out.join(if reference {
        format!("{}-ref", c.name)
    } else {
        c.name.clone()
    })
}

fn run_one(exe: &Path, argv: &[String], dir: &Path) -> Ran {
    // `<dir>.log` (a case name may hold a "."; not `with_extension`).
    let log = PathBuf::from(format!("{}.log", dir.display()));
    let t0 = Instant::now();
    let res = (|| -> Result<bool, String> {
        if dir.exists() {
            fs::remove_dir_all(dir).map_err(|e| format!("remove {}: {e}", dir.display()))?;
        }
        let f = File::create(&log).map_err(|e| format!("create {}: {e}", log.display()))?;
        let f2 = f.try_clone().map_err(|e| e.to_string())?;
        let mut cmd = Command::new(exe);
        cmd.args(argv)
            .stdin(Stdio::null())
            .stdout(Stdio::from(f))
            .stderr(Stdio::from(f2));
        // A killed bb test leaves no run behind (its Renode dies with it,
        // ADR-004 approver decision 7).
        bb_cpu_renode::die_with_parent(&mut cmd);
        let st = cmd
            .status()
            .map_err(|e| format!("start {}: {e}", exe.display()))?;
        Ok(st.success())
    })();
    let wall = t0.elapsed().as_secs_f64();
    match res {
        Ok(true) => Ran {
            ok: true,
            error: String::new(),
            wall,
        },
        Ok(false) => {
            let text = fs::read_to_string(&log).unwrap_or_default();
            let error = text
                .lines()
                .rfind(|l| l.starts_with("bb: "))
                .unwrap_or("bb run failed")
                .to_string();
            Ran {
                ok: false,
                error,
                wall,
            }
        }
        Err(e) => Ran {
            ok: false,
            error: e,
            wall,
        },
    }
}

fn sha(bytes: &[u8]) -> String {
    format!("sha256:{}", bbpkg::sha256::hex_digest(bytes))
}

/// A run's outputs as `bb test` reads them.
struct Outputs {
    events: String,
    nets_csv: Vec<u8>,
    nets_vcd: Vec<u8>,
    run_json: Option<Vec<u8>>,
}

impl Outputs {
    fn read(dir: &Path) -> Result<Outputs, String> {
        let r = |n: &str| fs::read(dir.join(n)).map_err(|e| format!("{n}: {e}"));
        let events = String::from_utf8(r("events.jsonl")?)
            .map_err(|_| "events.jsonl: not UTF-8".to_string())?;
        Ok(Outputs {
            events,
            nets_csv: r("nets.csv")?,
            nets_vcd: r("nets.vcd")?,
            run_json: fs::read(dir.join("run.json")).ok(),
        })
    }

    /// The records without the grant records (each line kept with its
    /// newline), as `grep -v '"type":"grant"'`; `asserts` false also drops
    /// the `assert` records (a golden's `non-grant-records`, recorded
    /// without assertions).
    fn non_grant(&self, asserts: bool) -> String {
        self.events
            .split_inclusive('\n')
            .filter(|l| !l.contains("\"type\":\"grant\""))
            .filter(|l| asserts || !l.contains("\"type\":\"assert\""))
            .collect()
    }

    /// The `assert` records, in order.
    fn asserts(&self) -> Result<Vec<Event>, String> {
        self.events
            .lines()
            .filter(|l| l.contains("\"type\":\"assert\""))
            .map(|l| Event::from_line(l).map_err(|e| format!("events.jsonl: {e}")))
            .collect()
    }

    /// The longest grant.
    fn longest_grant(&self) -> Result<u64, String> {
        let mut m = 0;
        for l in self
            .events
            .lines()
            .filter(|l| l.contains("\"type\":\"grant\""))
        {
            if let Event::Grant { t_fs, until_fs, .. } =
                Event::from_line(l).map_err(|e| format!("events.jsonl: {e}"))?
            {
                m = m.max(until_fs.as_fs() - t_fs.as_fs());
            }
        }
        Ok(m)
    }

    fn hashes(&self) -> Vec<(String, Json)> {
        let mut h = vec![
            (
                "events.jsonl".to_string(),
                Json::str(sha(self.events.as_bytes())),
            ),
            ("nets.csv".to_string(), Json::str(sha(&self.nets_csv))),
            ("nets.vcd".to_string(), Json::str(sha(&self.nets_vcd))),
            (
                "non-grant-records".to_string(),
                Json::str(sha(self.non_grant(false).as_bytes())),
            ),
        ];
        if let Some(r) = &self.run_json {
            h.push(("run.json".to_string(), Json::str(sha(r))));
        }
        h
    }
}

/// One reported assertion.
#[derive(Clone, Debug)]
struct Verdict {
    name: String,
    kind: String,
    expected: Outcome,
    /// `None`: the run gave no record for it.
    rec: Option<Rec>,
}

#[derive(Clone, Debug)]
struct Rec {
    outcome: Outcome,
    t_fs: u64,
    value: Option<i64>,
    limit: Option<i64>,
    unit: Option<String>,
    rung: String,
    msg: String,
}

impl Verdict {
    fn ok(&self) -> bool {
        self.rec
            .as_ref()
            .is_some_and(|r| r.outcome == self.expected)
    }
}

/// A comparison (golden, reference, same_as).
#[derive(Clone, Debug)]
struct Cmp {
    name: &'static str,
    ok: bool,
    /// A description for junit.xml.
    what: String,
    json: Json,
}

/// What a case gave.
struct CaseResult {
    run: Ran,
    verdicts: Vec<Verdict>,
    cmps: Vec<Cmp>,
    /// A problem reading the outputs (the run failed, or a file is
    /// missing).
    error: Option<String>,
    hashes: Vec<(String, Json)>,
    rung: Option<String>,
    props: Vec<(String, String)>,
}

impl CaseResult {
    fn ok(&self) -> bool {
        self.run.ok
            && self.error.is_none()
            && self.verdicts.iter().all(Verdict::ok)
            && self.cmps.iter().all(|c| c.ok)
    }
}

/// Parse a `sha256sum` file: `(name, hex)`.
fn golden_lines(text: &str) -> Result<Vec<(String, String)>, String> {
    let mut v = Vec::new();
    for (i, l) in text.lines().enumerate() {
        if l.trim().is_empty() {
            continue;
        }
        let (h, n) = l
            .split_once(' ')
            .ok_or_else(|| format!("line {}: expected <sha256>  <file>", i + 1))?;
        let n = n.trim_start_matches(' ').trim_start_matches('*');
        if h.len() != 64 || !h.chars().all(|c| c.is_ascii_hexdigit()) || n.is_empty() {
            return Err(format!("line {}: expected <sha256>  <file>", i + 1));
        }
        v.push((n.to_string(), h.to_ascii_lowercase()));
    }
    if v.is_empty() {
        return Err("no line".into());
    }
    Ok(v)
}

/// The golden comparison: `nets.csv`, `nets.vcd` and `non-grant-records`
/// (the records without the grant and assert records). `events.jsonl`'s
/// line is not checked: its grant records depend on the partition.
fn golden_cmp(tf: &TestFile, file: &str, o: &Outputs) -> Cmp {
    let mut checked = Vec::new();
    let mut differs = Vec::new();
    let mut not_checked = Vec::new();
    let res = (|| -> Result<(), String> {
        let text = fs::read_to_string(tf.dir.join(file)).map_err(|e| format!("{file}: {e}"))?;
        for (n, h) in golden_lines(&text).map_err(|e| format!("{file}: {e}"))? {
            let got = match n.as_str() {
                "nets.csv" => sha(&o.nets_csv),
                "nets.vcd" => sha(&o.nets_vcd),
                "non-grant-records" => sha(o.non_grant(false).as_bytes()),
                "events.jsonl" => {
                    not_checked.push(n);
                    continue;
                }
                _ => return Err(format!("{file}: {n}: not an output bb test compares")),
            };
            if got != format!("sha256:{h}") {
                differs.push(n.clone());
            }
            checked.push(n);
        }
        if checked.is_empty() {
            return Err(format!("{file}: no line bb test compares"));
        }
        Ok(())
    })();
    let (ok, err) = match res {
        Ok(()) => (differs.is_empty(), None),
        Err(e) => (false, Some(e)),
    };
    let strs = |v: &[String]| Json::Arr(v.iter().map(Json::str).collect());
    let mut j = vec![
        ("file".to_string(), Json::str(file)),
        ("ok".to_string(), Json::Bool(ok)),
        ("checked".to_string(), strs(&checked)),
        ("differs".to_string(), strs(&differs)),
        ("not_checked".to_string(), strs(&not_checked)),
    ];
    if let Some(e) = &err {
        j.push(("error".to_string(), Json::str(e)));
    }
    Cmp {
        name: "golden",
        ok,
        what: match (&err, ok) {
            (Some(e), _) => e.clone(),
            (None, true) => format!("{} equal {file}", checked.join(", ")),
            (None, false) => format!("{} differ from {file}", differs.join(", ")),
        },
        json: Json::Obj(j),
    }
}

/// Two runs' outputs: which of `files` differ (`non-grant` compares the
/// records without the grant records, the assert records included).
fn differ(a: &Outputs, b: &Outputs, files: &[&str]) -> Vec<String> {
    files
        .iter()
        .filter(|f| match **f {
            "nets.csv" => a.nets_csv != b.nets_csv,
            "nets.vcd" => a.nets_vcd != b.nets_vcd,
            "events.jsonl" => a.events != b.events,
            _ => a.non_grant(true) != b.non_grant(true),
        })
        .map(|f| f.to_string())
        .collect()
}

/// The JUnit properties of a project run (ADR-010 D9), from its run.json.
fn props_of(run_json: &[u8], rung: Option<&str>) -> Vec<(String, String)> {
    let mut p = Vec::new();
    if let Some(r) = rung {
        p.push(("rung".to_string(), r.to_string()));
    }
    let Ok(j) = Json::parse(&String::from_utf8_lossy(run_json)) else {
        return p;
    };
    let st = |v: Option<&Json>| v.and_then(Json::as_str).map(str::to_string);
    if let Some(f) = st(j.get("options").and_then(|o| o.get("fidelity"))) {
        p.push(("fidelity".into(), f));
    }
    if let Some(d) = st(j.get("deck_sha256")) {
        p.push(("deck".into(), d));
    }
    if let Some(d) = st(j.get("board_bbm")) {
        p.push(("board_bbm".into(), d));
    }
    let fw: Vec<String> = j
        .get("targets")
        .and_then(Json::as_arr)
        .unwrap_or_default()
        .iter()
        .flat_map(|t| t.get("images").and_then(Json::as_arr).unwrap_or_default())
        .filter_map(|i| st(i.get("hash")))
        .collect();
    if !fw.is_empty() {
        p.push(("firmware".into(), fw.join(" ")));
    }
    p.push(("run_json".into(), sha(run_json)));
    // The provenance flags (spec/package.md section 2): the views an agent
    // wrote, and those bb run flags until a person reviews them
    // (agent_generated or estimated).
    let mut agent = BTreeSet::new();
    let mut flagged = BTreeSet::new();
    for a in j.get("analog").and_then(Json::as_arr).unwrap_or_default() {
        let src = st(a.get("provenance").and_then(|p| p.get("source")));
        let id = format!(
            "{}:{}",
            st(a.get("package")).unwrap_or_default(),
            st(a.get("view")).unwrap_or_default()
        );
        match src.as_deref() {
            Some("agent_generated") => {
                agent.insert(id.clone());
                flagged.insert(id);
            }
            Some("estimated") => {
                flagged.insert(id);
            }
            _ => {}
        }
    }
    p.push((
        "agent_generated".into(),
        agent.into_iter().collect::<Vec<_>>().join(" "),
    ));
    p.push((
        "unreviewed_views".into(),
        flagged.into_iter().collect::<Vec<_>>().join(" "),
    ));
    p
}

fn evaluate(tf: &TestFile, out: &Path, i: usize, ran: &[Option<Ran>]) -> CaseResult {
    let c = &tf.cases[i];
    let run = ran[2 * i].clone().expect("every case ran");
    let mut r = CaseResult {
        run: run.clone(),
        verdicts: Vec::new(),
        cmps: Vec::new(),
        error: None,
        hashes: Vec::new(),
        rung: None,
        props: Vec::new(),
    };
    if !run.ok {
        r.error = Some(run.error.clone());
        return r;
    }
    let dir = run_dir(out, c, false);
    let o = match Outputs::read(&dir) {
        Ok(o) => o,
        Err(e) => {
            r.error = Some(e);
            return r;
        }
    };
    r.hashes = o.hashes();
    // The assertions.
    let recs = match o.asserts() {
        Ok(x) => x,
        Err(e) => {
            r.error = Some(e);
            return r;
        }
    };
    let mut by_name: Vec<(String, Rec)> = Vec::new();
    for e in recs {
        if let Event::Assert {
            t_fs,
            name,
            outcome,
            value,
            limit,
            unit,
            rung,
            msg,
            ..
        } = e
        {
            r.rung.get_or_insert_with(|| rung.clone());
            by_name.push((
                name,
                Rec {
                    outcome: outcome.into(),
                    t_fs: t_fs.as_fs(),
                    value,
                    limit,
                    unit,
                    rung,
                    msg,
                },
            ));
        }
    }
    let names: Vec<String> = match &c.asserts {
        Some(a) => a.clone(),
        None => by_name.iter().map(|(n, _)| n.clone()).collect(),
    };
    for n in names {
        let rec = by_name
            .iter()
            .find(|(x, _)| *x == n)
            .map(|(_, r)| r.clone());
        r.verdicts.push(Verdict {
            kind: c.kinds.get(&n).copied().unwrap_or("").to_string(),
            expected: c.expect.get(&n).copied().unwrap_or(Outcome::Pass),
            name: n,
            rec,
        });
    }
    // An expectation the run gave no record for is a failure too (checked
    // by Verdict::ok); an expect key the run never reported likewise.
    for n in c.expect.keys() {
        if !r.verdicts.iter().any(|v| &v.name == n) {
            r.verdicts.push(Verdict {
                name: n.clone(),
                kind: c.kinds.get(n).copied().unwrap_or("").to_string(),
                expected: c.expect[n],
                rec: None,
            });
        }
    }
    if let Some(rj) = &o.run_json {
        r.props = props_of(rj, r.rung.as_deref());
    } else {
        // A board file writes no run.json: the rung of its records, and the
        // image hash of its firmware (ADR-008 D5).
        if let Some(rg) = &r.rung {
            r.props.push(("rung".into(), rg.clone()));
        }
        if let (Base::Board(_), Some(t)) = (&c.base, &c.o.targets) {
            match fs::read(tf.dir.join(&t[0].1))
                .map_err(|e| e.to_string())
                .and_then(|b| crate::image::elf_image_hash(&b))
            {
                Ok(h) => r.props.push(("firmware".into(), h)),
                Err(e) => r.error = Some(format!("firmware {}: {e}", t[0].1)),
            }
        }
    }
    // The golden.
    if let Some(g) = &c.golden {
        r.cmps.push(golden_cmp(tf, g, &o));
    }
    // The reference (ADR-010 D8): identical non-grant records, nets.csv
    // and nets.vcd; "consistency" when the case's partition is the
    // one-cell one already.
    if c.reference {
        let rr = ran[2 * i + 1].clone().expect("the reference ran");
        let (ok, what, json) = if !rr.ok {
            (
                false,
                format!("the reference run failed: {}", rr.error),
                Json::obj([("ok", Json::Bool(false)), ("run", Json::str("failed"))]),
            )
        } else {
            match (
                Outputs::read(&run_dir(out, c, true)),
                o.longest_grant(),
                c.o.lattice(),
            ) {
                (Ok(ro), Ok(longest), Ok(l)) => {
                    let kind = if longest <= l.as_fs() {
                        "consistency"
                    } else {
                        "equivalence"
                    };
                    let d = differ(&o, &ro, &["nets.csv", "nets.vcd", "non-grant-records"]);
                    (
                        d.is_empty(),
                        if d.is_empty() {
                            format!("{kind}: equal to the one-cell reference except grant records")
                        } else {
                            format!("{kind}: {} differ from the reference", d.join(", "))
                        },
                        Json::obj([
                            ("kind", Json::str(kind)),
                            ("ok", Json::Bool(d.is_empty())),
                            ("differs", Json::Arr(d.iter().map(Json::str).collect())),
                            ("hashes", Json::Obj(ro.hashes())),
                        ]),
                    )
                }
                (Err(e), ..) | (_, Err(e), _) | (.., Err(e)) => (
                    false,
                    e.clone(),
                    Json::obj([("ok", Json::Bool(false)), ("error", Json::str(e))]),
                ),
            }
        };
        r.cmps.push(Cmp {
            name: "reference",
            ok,
            what,
            json,
        });
    }
    // same_as: byte-identical to another case (an engine order, a pool).
    if let Some(other) = &c.same_as {
        let j = tf
            .cases
            .iter()
            .position(|x| &x.name == other)
            .expect("checked");
        let files = ["nets.csv", "nets.vcd", "events.jsonl"];
        let (ok, what) = if !ran[2 * j].as_ref().is_some_and(|x| x.ok) {
            (false, format!("case {other} failed"))
        } else {
            match Outputs::read(&run_dir(out, &tf.cases[j], false)) {
                Ok(b) => {
                    let d = differ(&o, &b, &files);
                    if d.is_empty() {
                        (
                            true,
                            format!("{} byte-identical to {other}", files.join(", ")),
                        )
                    } else {
                        (false, format!("{} differ from {other}", d.join(", ")))
                    }
                }
                Err(e) => (false, e),
            }
        };
        r.cmps.push(Cmp {
            name: "same_as",
            ok,
            what: what.clone(),
            json: Json::obj([
                ("case", Json::str(other)),
                ("ok", Json::Bool(ok)),
                (
                    "files",
                    Json::Arr(files.iter().map(|f| Json::str(*f)).collect()),
                ),
            ]),
        });
    }
    r
}

fn int_or_null(v: Option<i64>) -> Json {
    v.map_or(Json::Null, |x| Json::Int(x.into()))
}

/// `test.json` (canonical JSON; no wall time, no path).
fn test_json(tf: &TestFile, results: &[CaseResult]) -> Json {
    let cases: Vec<Json> = tf
        .cases
        .iter()
        .zip(results)
        .map(|(c, r)| {
            let mut o = vec![
                ("name".to_string(), Json::str(&c.name)),
                ("ok".to_string(), Json::Bool(r.ok())),
                (
                    "run".to_string(),
                    Json::str(if r.run.ok { "ok" } else { "failed" }),
                ),
                (
                    "asserts".to_string(),
                    Json::Arr(
                        r.verdicts
                            .iter()
                            .map(|v| {
                                let mut a = vec![
                                    ("name".to_string(), Json::str(&v.name)),
                                    ("kind".to_string(), Json::str(&v.kind)),
                                    ("expected".to_string(), Json::str(v.expected.name())),
                                    ("ok".to_string(), Json::Bool(v.ok())),
                                ];
                                match &v.rec {
                                    None => a.push(("outcome".into(), Json::str("missing"))),
                                    Some(x) => a.extend([
                                        ("outcome".to_string(), Json::str(x.outcome.name())),
                                        ("t_fs".to_string(), Json::Int(x.t_fs.into())),
                                        ("value".to_string(), int_or_null(x.value)),
                                        ("limit".to_string(), int_or_null(x.limit)),
                                        (
                                            "unit".to_string(),
                                            x.unit.clone().map_or(Json::Null, Json::Str),
                                        ),
                                        ("rung".to_string(), Json::str(&x.rung)),
                                        ("msg".to_string(), Json::str(&x.msg)),
                                    ]),
                                }
                                Json::Obj(a)
                            })
                            .collect(),
                    ),
                ),
                ("hashes".to_string(), Json::Obj(r.hashes.clone())),
            ];
            for cmp in &r.cmps {
                o.push((cmp.name.to_string(), cmp.json.clone()));
            }
            if let Some(f) = r.props.iter().find(|(k, _)| k == "firmware") {
                o.push(("firmware".to_string(), Json::str(&f.1)));
            }
            Json::Obj(o)
        })
        .collect();
    let failed: Vec<Json> = tf
        .cases
        .iter()
        .zip(results)
        .filter(|(_, r)| !r.ok())
        .map(|(c, _)| Json::str(&c.name))
        .collect();
    Json::obj([
        ("schema", Json::str(RESULT_SCHEMA)),
        ("file", Json::str(&tf.label)),
        ("sha256", Json::str(&tf.sha256)),
        ("ok", Json::Bool(failed.is_empty())),
        ("failed", Json::Arr(failed)),
        ("cases", Json::Arr(cases)),
    ])
}

/// XML text or attribute escaping.
fn xml(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            // XML 1.0 allows tab, LF and CR among the control characters.
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {
                o.push_str(&format!("\\u{:04x}", c as u32))
            }
            c => o.push(c),
        }
    }
    o
}

fn value_text(r: &Rec) -> String {
    let u = r.unit.as_deref().unwrap_or("");
    let v = |x: Option<i64>| x.map_or("-".to_string(), |n| format!("{n} {u}").trim().to_string());
    format!("value={} limit={} t={} fs", v(r.value), v(r.limit), r.t_fs)
}

/// `junit.xml` (ADR-010 D9; the subset of `spec/schema/junit-subset.xsd`).
fn junit(tf: &TestFile, results: &[CaseResult]) -> String {
    let mut body = String::new();
    let (mut tt, mut tf_, mut te, mut ts, mut wall) = (0, 0, 0, 0, 0.0);
    for (c, r) in tf.cases.iter().zip(results) {
        let mut cases = String::new();
        let (mut t, mut f, mut e, mut sk) = (0, 0, 0, 0);
        // The run itself, always (a reporter drops a suite without a
        // testcase): an error when it failed or its outputs are unreadable.
        t += 1;
        if !r.run.ok || r.error.is_some() {
            e += 1;
            let msg = r.error.clone().unwrap_or_else(|| r.run.error.clone());
            cases.push_str(&format!(
                "    <testcase classname=\"{}\" name=\"run\" time=\"{:.3}\">\n      <error message=\"{}\" type=\"run\"/>\n    </testcase>\n",
                xml(&c.name),
                r.run.wall,
                xml(&msg)
            ));
        } else {
            cases.push_str(&format!(
                "    <testcase classname=\"{}\" name=\"run\" time=\"{:.3}\">\n      <system-out>bb run completed</system-out>\n    </testcase>\n",
                xml(&c.name),
                r.run.wall
            ));
        }
        for v in &r.verdicts {
            t += 1;
            let head = format!(
                "    <testcase classname=\"{}\" name=\"{}\" time=\"0\">",
                xml(&c.name),
                xml(&v.name)
            );
            let exp = v.expected.name();
            match &v.rec {
                None => {
                    f += 1;
                    cases.push_str(&format!(
                        "{head}\n      <failure message=\"no assert record (expected {exp})\" type=\"missing\"/>\n    </testcase>\n"
                    ));
                }
                Some(x) if x.outcome != v.expected => {
                    f += 1;
                    cases.push_str(&format!(
                        "{head}\n      <failure message=\"{} (expected {exp}): {}\" type=\"{}\">{}</failure>\n    </testcase>\n",
                        x.outcome.name(),
                        xml(&value_text(x)),
                        x.outcome.name(),
                        xml(&x.msg)
                    ));
                }
                Some(x) if x.outcome == Outcome::NotEvaluable => {
                    sk += 1;
                    cases.push_str(&format!(
                        "{head}\n      <skipped message=\"not_evaluable (expected): {}\"/>\n    </testcase>\n",
                        xml(&x.msg)
                    ));
                }
                Some(x) => {
                    cases.push_str(&format!(
                        "{head}\n      <system-out>{} (expected {exp}): {}; {}</system-out>\n    </testcase>\n",
                        x.outcome.name(),
                        xml(&value_text(x)),
                        xml(&x.msg)
                    ));
                }
            }
        }
        for cmp in &r.cmps {
            t += 1;
            let head = format!(
                "    <testcase classname=\"{}\" name=\"{}\" time=\"0\">",
                xml(&c.name),
                cmp.name
            );
            if cmp.ok {
                cases.push_str(&format!(
                    "{head}\n      <system-out>{}</system-out>\n    </testcase>\n",
                    xml(&cmp.what)
                ));
            } else {
                f += 1;
                cases.push_str(&format!(
                    "{head}\n      <failure message=\"{}\" type=\"{}\"/>\n    </testcase>\n",
                    xml(&cmp.what),
                    cmp.name
                ));
            }
        }
        let mut props = String::new();
        for (k, v) in &r.props {
            props.push_str(&format!(
                "      <property name=\"{}\" value=\"{}\"/>\n",
                xml(k),
                xml(v)
            ));
        }
        body.push_str(&format!(
            "  <testsuite name=\"{}\" tests=\"{t}\" failures=\"{f}\" errors=\"{e}\" skipped=\"{sk}\" time=\"{:.3}\">\n",
            xml(&c.name),
            r.run.wall
        ));
        if !props.is_empty() {
            body.push_str(&format!("    <properties>\n{props}    </properties>\n"));
        }
        body.push_str(&cases);
        body.push_str("  </testsuite>\n");
        tt += t;
        tf_ += f;
        te += e;
        ts += sk;
        wall += r.run.wall;
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuites name=\"{}\" tests=\"{tt}\" failures=\"{tf_}\" errors=\"{te}\" skipped=\"{ts}\" time=\"{wall:.3}\">\n{body}</testsuites>\n",
        xml(&tf.label)
    )
}

struct Args {
    file: PathBuf,
    out: PathBuf,
    jobs: Option<usize>,
    pool: String,
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut file = None;
    let mut out = None;
    let mut jobs = None;
    let mut pool = "auto".to_string();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{a} needs a value"))
        };
        match a.as_str() {
            "--out" => out = Some(PathBuf::from(val()?)),
            "--jobs" => {
                jobs = Some(
                    val()?
                        .parse::<usize>()
                        .ok()
                        .filter(|n| *n > 0)
                        .ok_or("--jobs: expected a positive number")?,
                )
            }
            "--pool" => {
                pool = val()?;
                if !["auto", "env", "off"].contains(&pool.as_str()) {
                    return Err(format!("--pool {pool:?}: expected auto, env or off"));
                }
            }
            s if !s.starts_with("--") && file.is_none() => file = Some(PathBuf::from(s)),
            s => return Err(format!("unknown argument {s:?}")),
        }
    }
    Ok(Args {
        file: file.ok_or("give the test file")?,
        out: out.ok_or("--out is required")?,
        jobs,
        pool,
    })
}

fn run_test(a: &Args) -> Result<bool, String> {
    let tf = TestFile::read(&a.file)?;
    let exe = std::env::current_exe().map_err(|e| format!("bb's own path: {e}"))?;
    fs::create_dir_all(&a.out).map_err(|e| format!("create {}: {e}", a.out.display()))?;
    let pool_off = a.pool == "off";
    // The inline assertion files.
    let inline_dir = a.out.join(".bb-test");
    let mut inline = vec![None; tf.cases.len()];
    for (i, c) in tf.cases.iter().enumerate() {
        if let Some(txt) = &c.inline {
            fs::create_dir_all(&inline_dir)
                .map_err(|e| format!("create {}: {e}", inline_dir.display()))?;
            let p = inline_dir.join(format!("{}.asserts.toml", c.name));
            fs::write(&p, txt).map_err(|e| format!("write {}: {e}", p.display()))?;
            inline[i] = Some(p);
        }
    }
    // Every case first (in the file's order), then the references.
    let mut jobs: Vec<Job> = (0..tf.cases.len())
        .map(|case| Job {
            case,
            reference: false,
        })
        .collect();
    for (i, c) in tf.cases.iter().enumerate() {
        if c.reference {
            jobs.push(Job {
                case: i,
                reference: true,
            });
        }
    }
    let argvs: Vec<(Vec<String>, PathBuf)> = jobs
        .iter()
        .map(|j| {
            let c = &tf.cases[j.case];
            let dir = run_dir(&a.out, c, j.reference);
            run_argv(
                &tf,
                c,
                &dir,
                j.reference,
                pool_off,
                inline[j.case].as_deref(),
            )
            .map(|v| (v, dir))
        })
        .collect::<Result<_, _>>()?;
    let n = a.jobs.unwrap_or(jobs.len().min(4)).max(1);
    println!(
        "bb test: {}: {} cases, {} runs, {n} at a time{}",
        tf.label,
        tf.cases.len(),
        jobs.len(),
        match (
            a.pool.as_str(),
            std::env::var_os(bb_cpu_renode::pool::POOL_ENV)
        ) {
            ("off", _) => ", every Renode fresh".to_string(),
            (_, Some(_)) => ", on the pool in $BB_RENODE_POOL".to_string(),
            _ => ", every Renode fresh (no $BB_RENODE_POOL)".to_string(),
        }
    );
    // ran[2i] the case, ran[2i + 1] its reference.
    let ran: Mutex<Vec<Option<Ran>>> = Mutex::new(vec![None; 2 * tf.cases.len()]);
    let next = AtomicUsize::new(0);
    std::thread::scope(|sc| {
        for _ in 0..n {
            sc.spawn(|| loop {
                let k = next.fetch_add(1, Ordering::SeqCst);
                let Some(j) = jobs.get(k) else { break };
                let (argv, dir) = &argvs[k];
                let r = run_one(&exe, argv, dir);
                let c = &tf.cases[j.case];
                println!(
                    "bb test:   {}{}: bb run {} ({:.1} s)",
                    c.name,
                    if j.reference { " (reference)" } else { "" },
                    if r.ok { "done" } else { "FAILED" },
                    r.wall
                );
                if !r.ok {
                    println!("bb test:     {}", r.error);
                }
                ran.lock().expect("no panic")[2 * j.case + usize::from(j.reference)] = Some(r);
            });
        }
    });
    let ran = ran.into_inner().expect("no panic");
    let results: Vec<CaseResult> = (0..tf.cases.len())
        .map(|i| evaluate(&tf, &a.out, i, &ran))
        .collect();
    let tj = test_json(&tf, &results).to_canonical();
    let p = a.out.join("test.json");
    fs::write(&p, tj).map_err(|e| format!("write {}: {e}", p.display()))?;
    let p = a.out.join("junit.xml");
    fs::write(&p, junit(&tf, &results)).map_err(|e| format!("write {}: {e}", p.display()))?;
    let mut all = true;
    for (c, r) in tf.cases.iter().zip(&results) {
        let ok = r.ok();
        all &= ok;
        let n_ok = r.verdicts.iter().filter(|v| v.ok()).count();
        let mut line = format!(
            "bb test: {} {}: {n_ok}/{} assertions as expected",
            if ok { "PASS" } else { "FAIL" },
            c.name,
            r.verdicts.len()
        );
        for v in r.verdicts.iter().filter(|v| v.expected != Outcome::Pass) {
            line.push_str(&format!(
                "; {} {} (expected {})",
                v.name,
                v.rec.as_ref().map_or("missing", |x| x.outcome.name()),
                v.expected.name()
            ));
        }
        for cmp in &r.cmps {
            line.push_str(&format!(
                "; {} {}",
                cmp.name,
                if cmp.ok { "ok" } else { "FAILED" }
            ));
        }
        println!("{line}");
        if let Some(e) = &r.error {
            println!("bb test:   {e}");
        }
        for v in r.verdicts.iter().filter(|v| !v.ok()) {
            println!(
                "bb test:   {}: {} (expected {}){}",
                v.name,
                v.rec.as_ref().map_or("no record", |x| x.outcome.name()),
                v.expected.name(),
                v.rec
                    .as_ref()
                    .map(|x| format!(": {}; {}", value_text(x), x.msg))
                    .unwrap_or_default()
            );
        }
        for cmp in r.cmps.iter().filter(|c| !c.ok) {
            println!("bb test:   {}: {}", cmp.name, cmp.what);
        }
    }
    println!(
        "bb test: {}: {} ({} cases; {} and {})",
        tf.label,
        if all { "PASS" } else { "FAIL" },
        tf.cases.len(),
        a.out.join("test.json").display(),
        a.out.join("junit.xml").display()
    );
    Ok(all)
}

pub fn main(args: &[String]) -> ExitCode {
    if args.first().is_some_and(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let a = match parse_args(args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("bb test: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    // --pool auto without a pool: run this command again under one (bb pool
    // run), with --pool env; the pool stops after it (ADR-010 D8).
    if a.pool == "auto" && std::env::var_os(bb_cpu_renode::pool::POOL_ENV).is_none() {
        let runs = match TestFile::read(&a.file) {
            Ok(tf) => tf.cases.len() + tf.cases.iter().filter(|c| c.reference).count(),
            Err(e) => {
                eprintln!("bb test: {e}");
                return ExitCode::FAILURE;
            }
        };
        let slots = a.jobs.unwrap_or(runs.min(4)).max(1);
        let exe = match std::env::current_exe() {
            Ok(e) => e,
            Err(e) => {
                eprintln!("bb test: bb's own path: {e}");
                return ExitCode::FAILURE;
            }
        };
        let mut cmd = Command::new(&exe);
        cmd.args(["pool", "run", "--slots", &slots.to_string(), "--"])
            .arg(&exe)
            .arg("test")
            .arg(&a.file)
            .arg("--out")
            .arg(&a.out)
            .args(["--jobs", &slots.to_string(), "--pool", "env"]);
        bb_cpu_renode::die_with_parent(&mut cmd);
        return match cmd.status() {
            Ok(st) if st.success() => ExitCode::SUCCESS,
            Ok(_) => ExitCode::FAILURE,
            Err(e) => {
                eprintln!("bb test: bb pool run: {e}");
                ExitCode::FAILURE
            }
        };
    }
    match run_test(&a) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("bb test: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = r#"
schema = "bb-test/0"
project = "."

[defaults]
for = "120ms"
fidelity = "fast"
probes = ["VSYS"]

[[case]]
name = "ok"
bench = "bench.toml"
targets = { "main:U10" = "fw/ok.elf" }
golden = "golden/ok.sha256"
reference = true

[[case.assert]]
name = "din-count"
kind = "count"
net = "U10.P0.06"
level = 1
min = 1

[[case.assert]]
name = "lsw3"
kind = "range"
net = "LED_DIN"
ref = "VLED"
max = "0.7V"
after = { edge = "U10.P1.04", level = 1, delay = "1ms" }

[[case]]
name = "wrong"
engine_order = "analog-first"
expect = { lsw3 = "fail" }
same_as = "ok"

[[case.assert]]
name = "din-count"
kind = "count"
net = "U10.P0.06"
min = 1

[[case.assert]]
name = "lsw3"
kind = "range"
net = "LED_DIN"
max = "0.7V"
"#;

    fn parse(t: &str) -> Result<TestFile, String> {
        // No bench file exists here: the cases that name one are only
        // parsed up to it.
        TestFile::parse(t, Path::new("/nonexistent"), "t.toml")
    }

    #[test]
    fn inline_asserts_round_trip() {
        let t = toml::parse(FILE).unwrap();
        let cases = t["case"].as_array().unwrap();
        let a = cases[0].as_table().unwrap()["assert"].as_array().unwrap();
        let txt = inline_file("ok", a).unwrap();
        let specs = crate::asserts::parse_file(&txt).unwrap();
        assert_eq!(
            specs.iter().map(|s| s.kind.name()).collect::<Vec<_>>(),
            ["count", "range"]
        );
        // The same tables written again give the same file.
        let t2 = toml::parse(&txt).unwrap();
        assert_eq!(
            inline_file("ok", t2["assert"].as_array().unwrap()).unwrap(),
            txt
        );
        assert!(toml_value(&Value::Float(0.5), "x").is_err());
    }

    #[test]
    fn file_rules() {
        // The bench is read for its assertions: a missing one is an error.
        let e = parse(FILE).unwrap_err();
        assert!(e.contains("bench.toml"), "{e}");
        let no_bench = FILE.replace("bench = \"bench.toml\"\n", "");
        let tf = parse(&no_bench).unwrap();
        assert_eq!(tf.cases.len(), 2);
        let w = &tf.cases[1];
        assert_eq!(w.o.duration.as_deref(), Some("120ms"));
        assert_eq!(w.o.engine_order.as_deref(), Some("analog-first"));
        assert_eq!(w.expect["lsw3"], Outcome::Fail);
        assert_eq!(w.kinds["din-count"], "count");
        // The argv: a project with an image, the inline file, the reference.
        let argv = run_argv(
            &tf,
            &tf.cases[0],
            Path::new("/o/ok-ref"),
            true,
            true,
            Some(Path::new("/o/.bb-test/ok.asserts.toml")),
        )
        .unwrap();
        let j = argv.join(" ");
        assert!(
            j.starts_with("run --project /nonexistent/. --image main:U10=/nonexistent/fw/ok.elf"),
            "{j}"
        );
        assert!(
            j.contains("--max-quantum 10us --lookahead off --renode-pool off --out /o/ok-ref"),
            "{j}"
        );
        // G9: a negative control without a count or event assertion.
        let g9 = no_bench.replace(
            "[[case.assert]]\nname = \"din-count\"\nkind = \"count\"\nnet = \"U10.P0.06\"\nmin = 1\n\n[[case.assert]]\nname = \"lsw3\"\nkind = \"range\"\nnet = \"LED_DIN\"\nmax = \"0.7V\"\n",
            "[[case.assert]]\nname = \"lsw3\"\nkind = \"range\"\nnet = \"LED_DIN\"\nmax = \"0.7V\"\n",
        );
        let e = parse(&g9).unwrap_err();
        assert!(e.contains("G9"), "{e}");
        // Unknown keys, a same_as chain, a board file without targets.
        assert!(parse(&no_bench.replace("golden =", "goldn =")).is_err());
        let e = parse(&no_bench.replace("same_as = \"ok\"", "same_as = \"wrong\"")).unwrap_err();
        assert!(e.contains("itself"), "{e}");
        let e = parse(&no_bench.replace("project = \".\"", "board = \"b.cir\"")).unwrap_err();
        assert!(e.contains("targets") || e.contains("probes"), "{e}");
        let e = parse(&no_bench.replace("name = \"wrong\"", "name = \"ok-ref\"")).unwrap_err();
        assert!(e.contains("-ref"), "{e}");
        let e = parse(&no_bench.replace("schema = \"bb-test/0\"", "schema = \"bb-test/1\""))
            .unwrap_err();
        assert!(e.contains("upgrade bb"), "{e}");
    }

    #[test]
    fn golden_and_xml() {
        let g = format!(
            "{}  nets.csv\n{} *events.jsonl\n",
            "a".repeat(64),
            "B".repeat(64)
        );
        assert_eq!(
            golden_lines(&g).unwrap(),
            [
                ("nets.csv".to_string(), "a".repeat(64)),
                ("events.jsonl".to_string(), "b".repeat(64))
            ]
        );
        assert!(golden_lines("xyz nets.csv\n").is_err());
        assert_eq!(xml("a<b>&\"'\u{1}"), "a&lt;b&gt;&amp;&quot;&apos;\\u0001");
        let o = Outputs {
            events: "{\"type\":\"grant\",\"t_fs\":0,\"until_fs\":20,\"limited_by\":\"t_end\"}\n{\"type\":\"assert\",\"t_fs\":5}\n{\"type\":\"pin_edge\"}\n".into(),
            nets_csv: Vec::new(),
            nets_vcd: Vec::new(),
            run_json: None,
        };
        assert_eq!(o.non_grant(false), "{\"type\":\"pin_edge\"}\n");
        assert_eq!(
            o.non_grant(true),
            "{\"type\":\"assert\",\"t_fs\":5}\n{\"type\":\"pin_edge\"}\n"
        );
        assert_eq!(o.longest_grant().unwrap(), 20);
    }
}
