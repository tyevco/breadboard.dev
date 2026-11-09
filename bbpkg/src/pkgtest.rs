//! Package test vectors and lattice qualification through the real
//! `NgspiceEngine` (ADR-006 D6, `spec/package.md` "Tests").
//!
//! A vector instantiates one view as `X1` (its ports mapped by pin name to
//! nodes of a small test circuit), drives control nets with `digital_out`
//! glue at given times, and measures node voltages or crossing times. It
//! runs under the kernel's real `Scheduler`, with the engine's lattice
//! clamp (lattice `L`, TMAX = `L`) and a named option set, at two grant
//! partitions (one cell, and 1 ms), which must give identical results.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use bb_analog_ngspice::{Drive, InputBinding, NgspiceConfig, NgspiceEngine, OutputBinding};
use bbcore::{
    DigitalOut, Direction, Engine, EngineError, EventSink, Level, NetSampler, NetValues, PinSignal,
    PinSignals, Probe, Scheduler, SchedulerConfig, Time, Transition,
};

use crate::obj::Obj;
use crate::optsets;
use crate::package::{Package, Port, View};
use crate::schema::Registry;
use crate::spice;
use crate::value::{parse_number, parse_quantity, spice_number, to_f64, Dim};

/// A control input (`[[edge]]`).
#[derive(Clone, Debug, PartialEq)]
pub struct Edge {
    pub net: String,
    pub model: DigitalOut,
    pub initial: Level,
    pub at: Vec<(Time, Level)>,
}

/// What an expectation measures.
#[derive(Clone, Debug, PartialEq)]
pub enum Meas {
    /// V(node) at `at`.
    At { node: String, at: Time },
    /// The time of the `n`-th crossing of `level` (µV) in the given
    /// direction after `after`.
    Cross {
        node: String,
        level_uv: i128,
        rise: bool,
        n: u32,
        after: Time,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Expect {
    pub meas: Meas,
    /// µV for `At`, fs for `Cross`.
    pub value: i128,
    pub tol: i128,
    pub source: String,
}

/// A test vector file.
#[derive(Clone, Debug, PartialEq)]
pub struct Vector {
    pub file: PathBuf,
    pub view: String,
    pub circuit: PathBuf,
    pub stop: Time,
    pub lattice: Time,
    pub options: String,
    pub sample: Time,
    pub ports: BTreeMap<String, String>,
    pub params: Vec<(String, String)>,
    pub edges: Vec<Edge>,
    pub expects: Vec<Expect>,
}

fn time(text: &str, what: &str) -> Result<Time, String> {
    let fs = parse_quantity(text, Dim::Time).map_err(|e| format!("{what}: {e}"))?;
    u64::try_from(fs)
        .map(Time::from_fs)
        .map_err(|_| format!("{what}: {text:?} out of range"))
}

fn level(text: &str, what: &str) -> Result<Level, String> {
    match text {
        "high" => Ok(Level::High),
        "low" => Ok(Level::Low),
        _ => Err(format!("{what}: {text:?}: expected high or low")),
    }
}

/// Parse one vector file.
pub fn parse_vector(path: &Path, text: &str, reg: &Registry) -> Result<Vector, String> {
    let t = crate::toml::parse(text).map_err(|e| e.to_string())?;
    let mut o = Obj::new("", &t);
    reg.check("bb-pkgtest", &o.str_req("schema")?)?;
    let dir = path.parent().unwrap_or(Path::new("."));
    let lattice = time(
        &o.str_opt("lattice")?.unwrap_or_else(|| "10us".into()),
        "lattice",
    )?;
    let options = o
        .str_opt("options")?
        .unwrap_or_else(|| optsets::DEFAULT.into());
    if optsets::get(&options).is_none() {
        return Err(format!(
            "options {options:?}: not a named option set ({})",
            optsets::names().join(", ")
        ));
    }
    let sample = match o.str_opt("sample")? {
        Some(s) => time(&s, "sample")?,
        None => lattice,
    };
    let mut ports = BTreeMap::new();
    for (k, v) in o.str_map("ports")? {
        ports.insert(k, v);
    }
    let mut edges = Vec::new();
    for mut e in o.tables("edge")? {
        let path = e.path.clone();
        let net = e.str_req("net")?;
        let mut m = e
            .table_opt("model")?
            .ok_or_else(|| format!("{path}.model: required"))?;
        let q = |m: &mut Obj, k: &str, d: Dim| -> Result<i128, String> {
            parse_quantity(&m.str_req(k)?, d).map_err(|x| format!("{path}.model.{k}: {x}"))
        };
        let model = DigitalOut {
            voh: to_f64(q(&mut m, "voh", Dim::Voltage)?, Dim::Voltage),
            vol: to_f64(q(&mut m, "vol", Dim::Voltage)?, Dim::Voltage),
            rise: Time::from_fs(
                u64::try_from(q(&mut m, "rise", Dim::Time)?)
                    .map_err(|_| format!("{path}: rise"))?,
            ),
            fall: Time::from_fs(
                u64::try_from(q(&mut m, "fall", Dim::Time)?)
                    .map_err(|_| format!("{path}: fall"))?,
            ),
            zout: to_f64(q(&mut m, "zout", Dim::Resistance)?, Dim::Resistance),
        };
        m.done()?;
        let initial = level(
            &e.str_opt("initial")?.unwrap_or_else(|| "low".into()),
            &format!("{path}.initial"),
        )?;
        let mut at = Vec::new();
        match e.get("at") {
            Some(crate::toml::Value::Array(a)) => {
                for (i, pair) in a.iter().enumerate() {
                    let w = format!("{path}.at[{i}]");
                    let pair = pair
                        .as_array()
                        .filter(|p| p.len() == 2)
                        .ok_or_else(|| format!("{w}: expected [time, level]"))?;
                    let (Some(ts), Some(ls)) = (pair[0].as_str(), pair[1].as_str()) else {
                        return Err(format!("{w}: expected two strings"));
                    };
                    at.push((time(ts, &w)?, level(ls, &w)?));
                }
            }
            _ => return Err(format!("{path}.at: required, an array of [time, level]")),
        }
        let mut prev = (Time::ZERO, initial);
        for (t, l) in &at {
            if *t <= prev.0 || *l == prev.1 {
                return Err(format!("{path}.at: times must increase from > 0 and levels alternate from the initial level"));
            }
            prev = (*t, *l);
        }
        e.done()?;
        edges.push(Edge {
            net,
            model,
            initial,
            at,
        });
    }
    let mut expects = Vec::new();
    for mut x in o.tables("expect")? {
        let path = x.path.clone();
        let mut m = x
            .table_opt("meas")?
            .ok_or_else(|| format!("{path}.meas: required"))?;
        let node = m.str_req("node")?;
        let (meas, dim) = if let Some(at) = m.str_opt("at")? {
            (
                Meas::At {
                    node,
                    at: time(&at, &format!("{path}.meas.at"))?,
                },
                Dim::Voltage,
            )
        } else {
            let level_uv = parse_quantity(&m.str_req("when")?, Dim::Voltage)
                .map_err(|e| format!("{path}.meas.when: {e}"))?;
            let (rise, n) = match (m.str_opt("rise")?, m.str_opt("fall")?) {
                (Some(n), None) => (true, n),
                (None, Some(n)) => (false, n),
                _ => {
                    return Err(format!(
                    "{path}.meas: give exactly one of rise or fall (the crossing number, \"1\")"
                ))
                }
            };
            let n: u32 = n
                .parse()
                .ok()
                .filter(|n| *n >= 1)
                .ok_or_else(|| format!("{path}.meas: crossing number {n:?}"))?;
            let after = match m.str_opt("after")? {
                Some(a) => time(&a, &format!("{path}.meas.after"))?,
                None => Time::ZERO,
            };
            (
                Meas::Cross {
                    node,
                    level_uv,
                    rise,
                    n,
                    after,
                },
                Dim::Time,
            )
        };
        m.done()?;
        let value =
            parse_quantity(&x.str_req("value")?, dim).map_err(|e| format!("{path}.value: {e}"))?;
        let tol =
            parse_quantity(&x.str_req("tol")?, dim).map_err(|e| format!("{path}.tol: {e}"))?;
        if tol < 0 {
            return Err(format!("{path}.tol: must be >= 0"));
        }
        let source = x.str_req("source")?;
        x.done()?;
        expects.push(Expect {
            meas,
            value,
            tol,
            source,
        });
    }
    let v = Vector {
        file: path.to_path_buf(),
        view: o.str_req("view")?,
        circuit: dir.join(o.str_req("circuit")?),
        stop: time(&o.str_req("stop")?, "stop")?,
        lattice,
        options,
        sample,
        ports,
        params: o.str_map("params")?,
        edges,
        expects,
    };
    o.done()?;
    if v.expects.is_empty() {
        return Err("no [[expect]]".into());
    }
    if v.stop.is_zero() || !v.stop.as_fs().is_multiple_of(v.lattice.as_fs()) {
        return Err(format!(
            "stop {} must be a positive multiple of the lattice {}",
            v.stop, v.lattice
        ));
    }
    Ok(v)
}

/// Every vector (`*.toml`) in `dir`, sorted by file name, checked against
/// the package's views.
pub fn load_vectors(pkg: &Package, dir: &Path, reg: &Registry) -> Result<Vec<Vector>, String> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("read {}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "toml") && p.is_file())
        .collect();
    files.sort();
    let mut out = Vec::new();
    for f in files {
        let name = f.file_name().unwrap().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(&f).map_err(|e| format!("read {}: {e}", f.display()))?;
        let v = parse_vector(&f, &text, reg).map_err(|e| format!("{name}: {e}"))?;
        let view = pkg
            .part
            .analog
            .iter()
            .find(|x| x.name == v.view)
            .ok_or_else(|| format!("{name}: view {:?} is not in part.toml", v.view))?;
        if view.kind != "circuit" {
            return Err(format!(
                "{name}: view {} is a {} view; Phase 1 vectors test circuit views",
                view.name, view.kind
            ));
        }
        for p in &view.ports {
            if let Port::Pin(n) = p {
                if !v.ports.contains_key(n) {
                    return Err(format!(
                        "{name}: ports.{n}: the view's port {n} is not mapped to a node"
                    ));
                }
            }
        }
        for k in v.ports.keys() {
            if !view.ports.contains(&Port::Pin(k.clone())) {
                return Err(format!(
                    "{name}: ports.{k}: not a port of view {}",
                    view.name
                ));
            }
        }
        if !v.circuit.is_file() {
            return Err(format!("{name}: circuit {} not found", v.circuit.display()));
        }
        out.push(v);
    }
    Ok(out)
}

/// The deck body for a vector: the view's definitions, `X1`, the test
/// circuit and the option set's card.
pub fn deck(pkg: &Package, view: &View, v: &Vector, options: &str) -> Result<String, String> {
    let file = pkg.dir.join(&view.file);
    let text =
        std::fs::read_to_string(&file).map_err(|e| format!("read {}: {e}", file.display()))?;
    let defs = spice::assemble(&[(format!("view {}", view.name), text)])?;
    let mut params: BTreeMap<String, String> = view.params.iter().cloned().collect();
    for (k, val) in &v.params {
        params.insert(k.clone(), val.clone());
    }
    let mut x1 = String::from("X1");
    for p in &view.ports {
        if let Port::Pin(n) = p {
            x1.push(' ');
            x1.push_str(&v.ports[n]);
        }
    }
    x1.push(' ');
    x1.push_str(&view.subckt);
    for (k, val) in &params {
        let d = parse_number(val).map_err(|e| format!("params.{k}: {e}"))?;
        x1.push_str(&format!(" {k}={}", spice_number(d)));
    }
    let circuit = std::fs::read_to_string(&v.circuit)
        .map_err(|e| format!("read {}: {e}", v.circuit.display()))?;
    for l in circuit.lines() {
        let t = l.trim_start().to_ascii_lowercase();
        if t.starts_with(".tran")
            || t.starts_with(".options")
            || t.starts_with(".option ")
            || t.starts_with(".control")
        {
            return Err(format!(
                "{}: a test circuit has no .tran, .options or .control (the runner adds them)",
                v.circuit.display()
            ));
        }
    }
    let card = optsets::get(options).ok_or_else(|| format!("options {options:?}: unknown"))?;
    Ok(format!(
        "* bb pkg test: {} view {}\n{defs}{x1}\n{circuit}\n{card}\n",
        pkg.part.id, view.name
    ))
}

/// Drives the vector's control nets: every change is reported in its trace
/// (`t_safe` = `Time::MAX`), at its exact time.
struct EdgeSource {
    edges: Vec<(Time, String, Level)>,
    levels: BTreeMap<String, (Level, Time)>,
    now: Time,
    trace: Vec<Transition>,
}

impl Engine for EdgeSource {
    fn name(&self) -> &str {
        "edges"
    }
    fn next_safe_time(&mut self, _now: Time) -> Time {
        Time::MAX
    }
    fn advance_to(&mut self, t: Time) -> Result<(), EngineError> {
        self.trace.clear();
        for (et, pin, l) in &self.edges {
            if *et > self.now && *et <= t {
                self.levels.insert(pin.clone(), (*l, *et));
                self.trace.push(Transition {
                    t: *et,
                    pin: pin.clone(),
                    signal: PinSignal::digital(*l, *et),
                });
            }
        }
        self.now = t;
        Ok(())
    }
    fn read_outputs(&mut self) -> Result<PinSignals, EngineError> {
        Ok(self
            .levels
            .iter()
            .map(|(p, (l, t))| (p.clone(), PinSignal::digital(*l, *t)))
            .collect())
    }
    fn read_output_trace(&mut self) -> Result<Option<Vec<Transition>>, EngineError> {
        Ok(Some(self.trace.clone()))
    }
    fn apply_inputs(&mut self, _: &PinSignals) -> Result<(), EngineError> {
        Ok(())
    }
}

/// Samples nets at fixed instants (never a grant boundary).
struct Sampler {
    instants: Vec<Time>,
    next: usize,
    nets: Vec<String>,
    /// net -> [(t, volts)]
    out: BTreeMap<String, Vec<(Time, f64)>>,
}

impl Probe for Sampler {
    fn next_sample_time(&self, _now: Time) -> Option<Time> {
        None
    }
    fn on_boundary(&mut self, _prev: Time, _now: Time, _nets: &NetValues) -> std::io::Result<()> {
        Ok(())
    }
    fn on_window(
        &mut self,
        _prev: Time,
        now: Time,
        nets: &NetValues,
        sampler: &dyn NetSampler,
    ) -> std::io::Result<()> {
        while self.next < self.instants.len() && self.instants[self.next] <= now {
            let t = self.instants[self.next];
            for n in &self.nets {
                let v = sampler
                    .sample(n, t)
                    .or_else(|| nets.get(n).copied())
                    .and_then(|s| s.volts())
                    .unwrap_or(f64::NAN);
                self.out.entry(n.clone()).or_default().push((t, v));
            }
            self.next += 1;
        }
        Ok(())
    }
    fn finish(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// One measured result.
#[derive(Clone, Debug, PartialEq)]
pub struct Measured {
    /// µV or fs; `None` if a crossing never happened.
    pub value: Option<i128>,
    pub pass: bool,
}

/// One run of a vector at one grant partition.
#[derive(Clone, Debug, PartialEq)]
pub struct RunResult {
    pub measured: Vec<Measured>,
    /// Every sample, as `f64` bits, for the partition identity check.
    pub samples: Vec<(String, Vec<(u64, u64)>)>,
    pub grants: u64,
}

fn crossing(samples: &[(Time, f64)], level: f64, rise: bool, n: u32, after: Time) -> Option<i128> {
    let mut k = 0;
    for w in samples.windows(2) {
        let ((t0, v0), (t1, v1)) = (w[0], w[1]);
        if t1 <= after {
            continue;
        }
        let hit = if rise {
            v0 < level && v1 >= level
        } else {
            v0 > level && v1 <= level
        };
        if hit {
            k += 1;
            if k == n {
                let f = (level - v0) / (v1 - v0);
                let dt = (t1.as_fs() - t0.as_fs()) as f64;
                return Some(t0.as_fs() as i128 + (f * dt).round() as i128);
            }
        }
    }
    None
}

/// Run a vector at grant partition `max_quantum` under `options` and
/// `lattice`.
pub fn run(
    pkg: &Package,
    view: &View,
    v: &Vector,
    options: &str,
    lattice: Time,
    max_quantum: Time,
) -> Result<RunResult, String> {
    let body = deck(pkg, view, v, options)?;
    if !v.stop.as_fs().is_multiple_of(lattice.as_fs()) {
        return Err(format!(
            "stop {} is not a multiple of the lattice {lattice}",
            v.stop
        ));
    }
    let mut cfg = NgspiceConfig::new(body, v.stop, lattice);
    cfg.lattice = Some(lattice);
    for e in &v.edges {
        cfg.inputs.push(InputBinding {
            pin: format!("edge_{}", e.net),
            source: format!("V_bbt_{}", e.net),
            drive: Drive::Digital {
                model: e.model,
                initial: e.initial,
            },
            glue_node: Some(e.net.clone()),
        });
    }
    let nodes: BTreeSet<String> = v
        .expects
        .iter()
        .map(|x| match &x.meas {
            Meas::At { node, .. } | Meas::Cross { node, .. } => node.clone(),
        })
        .collect();
    for n in &nodes {
        cfg.outputs.push(OutputBinding {
            pin: format!("node_{n}"),
            node: n.clone(),
        });
    }
    let mut edges = Vec::new();
    let mut levels = BTreeMap::new();
    for e in &v.edges {
        levels.insert(format!("edge_{}", e.net), (e.initial, Time::ZERO));
        for (t, l) in &e.at {
            edges.push((*t, format!("edge_{}", e.net), *l));
        }
    }
    edges.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    let src = EdgeSource {
        edges,
        levels,
        now: Time::ZERO,
        trace: Vec::new(),
    };
    let engine = NgspiceEngine::new(cfg).map_err(|e| format!("start ngspice: {e}"))?;
    let mut s = Scheduler::new(SchedulerConfig {
        max_quantum,
        loop_quantum: None,
        lookahead: false,
        log_grants: false,
        event_sampling: None,
    })
    .map_err(|e| e.to_string())?;
    let sid = s.add_engine(Box::new(src)).map_err(|e| e.to_string())?;
    let nid = s.add_engine(Box::new(engine)).map_err(|e| e.to_string())?;
    for e in &v.edges {
        let p = format!("edge_{}", e.net);
        s.bind(sid, &p, &p, Direction::Output)
            .map_err(|e| e.to_string())?;
        s.bind(nid, &p, &p, Direction::Input)
            .map_err(|e| e.to_string())?;
    }
    for n in &nodes {
        let p = format!("node_{n}");
        s.bind(nid, &p, &p, Direction::Output)
            .map_err(|e| e.to_string())?;
    }
    // Sample instants: the grid (every `sample`) and every `at`.
    let mut inst: BTreeSet<Time> = BTreeSet::new();
    let mut t = Time::ZERO;
    while t <= v.stop {
        inst.insert(t);
        t = Time::from_fs(t.as_fs() + v.sample.as_fs());
    }
    for x in &v.expects {
        if let Meas::At { at, .. } = &x.meas {
            if *at > v.stop {
                return Err(format!("meas at {at} is after stop {}", v.stop));
            }
            inst.insert(*at);
        }
    }
    let mut probe = Sampler {
        instants: inst.into_iter().collect(),
        next: 0,
        nets: nodes.iter().map(|n| format!("node_{n}")).collect(),
        out: BTreeMap::new(),
    };
    let mut sink_buf = Vec::new();
    let summary = {
        let mut sink = EventSink::new(&mut sink_buf);
        let mut probes: [&mut dyn Probe; 1] = [&mut probe];
        s.run(v.stop, &mut sink, &mut probes)
            .map_err(|e| format!("simulation failed: {e}"))?
    };
    drop(s); // releases the ngspice singleton
    let mut measured = Vec::new();
    for x in &v.expects {
        let value = match &x.meas {
            Meas::At { node, at } => probe.out[&format!("node_{node}")]
                .iter()
                .find(|(t, _)| t == at)
                .map(|(_, v)| bbcore::volts_to_microvolts(*v) as i128),
            Meas::Cross {
                node,
                level_uv,
                rise,
                n,
                after,
            } => crossing(
                &probe.out[&format!("node_{node}")],
                to_f64(*level_uv, Dim::Voltage),
                *rise,
                *n,
                *after,
            ),
        };
        let pass = value.is_some_and(|m| (m - x.value).abs() <= x.tol);
        measured.push(Measured { value, pass });
    }
    let samples = probe
        .out
        .iter()
        .map(|(n, s)| {
            (
                n.clone(),
                s.iter().map(|(t, v)| (t.as_fs(), v.to_bits())).collect(),
            )
        })
        .collect();
    Ok(RunResult {
        measured,
        samples,
        grants: summary.grants,
    })
}

impl Meas {
    /// For reports: `V(out) at 2ms`, `V(out) rise 1 through 0.5 V after 0s`.
    pub fn describe(&self) -> String {
        match self {
            Meas::At { node, at } => format!("V({node}) at {at}"),
            Meas::Cross {
                node,
                level_uv,
                rise,
                n,
                after,
            } => format!(
                "V({node}) {} {n} through {} V after {after}",
                if *rise { "rise" } else { "fall" },
                spice_number(crate::value::to_decimal(*level_uv, Dim::Voltage))
            ),
        }
    }
}

/// Format a measured value for reports.
pub fn show(value: i128, meas: &Meas) -> String {
    match meas {
        Meas::At { .. } => format!(
            "{} V",
            spice_number(crate::value::to_decimal(value, Dim::Voltage))
        ),
        Meas::Cross { .. } => format!(
            "{} s",
            spice_number(crate::value::to_decimal(value, Dim::Time))
        ),
    }
}
