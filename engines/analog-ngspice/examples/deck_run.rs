//! Run a SPICE body through the real [`NgspiceEngine`] (Phase 1 plan S1).
//!
//! The circuit is driven the way a co-simulation drives it: a grant lattice
//! (TMAX = the lattice unless `--tmax` says otherwise), the engine's step
//! clamp and breakpoint release, and the control inputs delivered as
//! `digital_out` glue (`EXTERNAL` source plus `zout` into the node, 0 V and
//! `--voh`, linear ramps of `--ramp`) at their times, applied before the grant that holds
//! them. Nothing is free-run: the loop grants `--grant` at a time up to
//! `--stop`. The options line (`--options`) is appended to the body, so
//! the run uses exactly the named option set (ADR-006 D6).
//!
//! ```text
//! deck_run --body <file.cir> --stop 120ms [--options '.options RELTOL=1e-5 ...']
//!          [--lattice 10us] [--tmax 10us] [--grant 1ms] [--ramp 100ns] [--voh 1] [--zout 1]
//!          [--edge <node>@<time>=<0|1>]... [--probe <node>]... [--csv <out.csv>] [--every 10us]
//! ```
//!
//! The CSV holds `t_fs` and each probe at every `--every` instant (the
//! engine's interpolation between solver points, a pure function of them).
//! The last stdout line is a JSON summary: `status` (`ok` or the error),
//! the time reached, wall time, solver points and each probe's minimum and
//! maximum over every accepted solver point. Wall time never reaches the
//! CSV, so two runs give identical CSV bytes.
use std::collections::BTreeMap;
use std::io::Write;
use std::time::Instant;

use bb_analog_ngspice::{Drive, InputBinding, NgspiceConfig, NgspiceEngine};
use bbcore::{DigitalOut, Engine, Level, PinSignal, PinSignals, Time, Transition};

/// An exact decimal time with a unit (`fs`, `ps`, `ns`, `us`, `ms`, `s`) to
/// integer femtoseconds.
fn parse_time(s: &str) -> Result<Time, String> {
    let s = s.trim();
    let split = s
        .find(|c: char| c.is_ascii_alphabetic())
        .ok_or_else(|| format!("time {s:?} needs a unit"))?;
    let (num, unit) = s.split_at(split);
    let scale: u32 = match unit {
        "fs" => 0,
        "ps" => 3,
        "ns" => 6,
        "us" => 9,
        "ms" => 12,
        "s" => 15,
        _ => return Err(format!("time {s:?}: unknown unit {unit:?}")),
    };
    let (int, frac) = num.split_once('.').unwrap_or((num, ""));
    if int.is_empty() && frac.is_empty() || frac.len() > scale as usize {
        return Err(format!("time {s:?} is not a whole number of fs"));
    }
    let digits = format!("{int}{frac}");
    let fs: u64 = digits
        .parse::<u64>()
        .map_err(|e| format!("time {s:?}: {e}"))?
        .checked_mul(10u64.pow(scale - frac.len() as u32))
        .ok_or_else(|| format!("time {s:?} overflows"))?;
    Ok(Time::from_fs(fs))
}

struct Args {
    body: String,
    options: String,
    stop: Time,
    lattice: Time,
    tmax: Option<Time>,
    grant: Time,
    ramp: Time,
    voh: f64,
    zout: f64,
    edges: Vec<(String, Time, Level)>,
    probes: Vec<String>,
    csv: Option<String>,
    every: Time,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        body: String::new(),
        options: String::new(),
        stop: Time::ZERO,
        lattice: Time::from_us(10),
        tmax: None,
        grant: Time::from_ms(1),
        ramp: Time::from_ns(100),
        voh: 1.0,
        zout: 1.0,
        edges: Vec::new(),
        probes: Vec::new(),
        csv: None,
        every: Time::from_us(10),
    };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let mut v = || it.next().ok_or_else(|| format!("{k} needs a value"));
        match k.as_str() {
            "--body" => a.body = v()?,
            "--options" => a.options = v()?,
            "--stop" => a.stop = parse_time(&v()?)?,
            "--lattice" => a.lattice = parse_time(&v()?)?,
            "--tmax" => a.tmax = Some(parse_time(&v()?)?),
            "--grant" => a.grant = parse_time(&v()?)?,
            "--ramp" => a.ramp = parse_time(&v()?)?,
            "--zout" => a.zout = v()?.parse().map_err(|e| format!("--zout: {e}"))?,
            "--voh" => a.voh = v()?.parse().map_err(|e| format!("--voh: {e}"))?,
            "--edge" => {
                let e = v()?;
                let (node, rest) = e.split_once('@').ok_or("--edge <node>@<time>=<0|1>")?;
                let (t, lv) = rest.split_once('=').ok_or("--edge <node>@<time>=<0|1>")?;
                let level = match lv {
                    "1" => Level::High,
                    "0" => Level::Low,
                    _ => return Err(format!("--edge {e}: level must be 0 or 1")),
                };
                a.edges.push((node.to_string(), parse_time(t)?, level));
            }
            "--probe" => a.probes.push(v()?),
            "--csv" => a.csv = Some(v()?),
            "--every" => a.every = parse_time(&v()?)?,
            "-h" | "--help" => {
                return Err("usage: deck_run --body <file> --stop <t> [--options <line>] [--lattice 10us] \
                     [--tmax <t>] [--grant 1ms] [--ramp 100ns] [--voh 1] [--zout 1] [--edge node@t=0|1]... \
                     [--probe node]... [--csv out] [--every 10us]"
                    .into())
            }
            _ => return Err(format!("unknown argument {k}")),
        }
    }
    if a.body.is_empty() || a.stop.is_zero() {
        return Err("--body and --stop are required".into());
    }
    if a.every.is_zero() || !a.every.as_fs().is_multiple_of(a.lattice.as_fs()) {
        return Err("--every must be a multiple of --lattice".into());
    }
    a.edges.sort_by_key(|e| e.1);
    Ok(a)
}

fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// Source name of the glue for `node`: `V_` plus the node with every
/// character other than an ASCII letter or digit replaced by `_`.
fn source_of(node: &str) -> String {
    let n: String = node
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("V_{n}")
}

fn run(a: &Args) -> Result<(), String> {
    let body = std::fs::read_to_string(&a.body).map_err(|e| format!("{}: {e}", a.body))?;
    let netlist = format!("{body}\n{}\n", a.options);
    let mut cfg = NgspiceConfig::new(netlist, a.stop, a.tmax.unwrap_or(a.lattice));
    cfg.lattice = Some(a.lattice);
    let mut pins: BTreeMap<String, String> = BTreeMap::new();
    for (node, _, _) in &a.edges {
        if pins.contains_key(node) {
            continue;
        }
        let source = source_of(node);
        pins.insert(node.clone(), source.clone());
        cfg.inputs.push(InputBinding {
            pin: node.clone(),
            source,
            drive: Drive::Digital {
                model: DigitalOut {
                    voh: a.voh,
                    vol: 0.0,
                    rise: a.ramp,
                    fall: a.ramp,
                    zout: a.zout,
                },
                initial: Level::Low,
            },
            glue_node: Some(node.clone()),
        });
    }
    cfg.probes.clone_from(&a.probes);

    let t0 = Instant::now();
    let mut csv = match &a.csv {
        Some(p) => Some(std::io::BufWriter::new(
            std::fs::File::create(p).map_err(|e| format!("{p}: {e}"))?,
        )),
        None => None,
    };
    let mut min = vec![f64::INFINITY; a.probes.len()];
    let mut max = vec![f64::NEG_INFINITY; a.probes.len()];
    let mut status = "ok".to_string();
    let mut now = Time::ZERO;
    let mut points = 0usize;
    match NgspiceEngine::new(cfg) {
        Err(e) => status = format!("engine start: {e}"),
        Ok(mut eng) => {
            if let Some(w) = csv.as_mut() {
                let mut head = String::from("t_fs");
                for p in &a.probes {
                    head.push(',');
                    head.push_str(p);
                }
                writeln!(w, "{head}").map_err(|e| e.to_string())?;
                let mut row = String::from("0");
                for p in &a.probes {
                    let v = eng.voltage(p).map_err(|e| e.to_string())?;
                    row.push_str(&format!(",{v:.9e}"));
                }
                writeln!(w, "{row}").map_err(|e| e.to_string())?;
            }
            // The input snapshot (every pin Low at t = 0, as `initial`).
            let mut base: PinSignals = pins
                .keys()
                .map(|n| (n.clone(), PinSignal::digital(Level::Low, Time::ZERO)))
                .collect();
            while now < a.stop {
                let g = now.saturating_add(a.grant).min(a.stop);
                // Every edge of the grant, in time order, as the kernel
                // delivers a producer's trace before this engine advances
                // (ADR-003 D3; Engine::apply_input_trace).
                let trace: Vec<Transition> = a
                    .edges
                    .iter()
                    .filter(|(_, t, _)| now < *t && *t <= g)
                    .map(|(node, t, level)| Transition {
                        t: *t,
                        pin: node.clone(),
                        signal: PinSignal::digital(*level, *t),
                    })
                    .collect();
                if !trace.is_empty() {
                    eng.apply_input_trace(&base, &trace)
                        .map_err(|e| e.to_string())?;
                    for tr in &trace {
                        base.insert(tr.pin.clone(), tr.signal);
                    }
                }
                if let Err(e) = eng.advance_to(g) {
                    status = format!("{e}");
                    break;
                }
                for (k, p) in a.probes.iter().enumerate() {
                    for (_, v) in eng.solver_window(p).map_err(|e| e.to_string())? {
                        min[k] = min[k].min(v);
                        max[k] = max[k].max(v);
                    }
                }
                if let Some(w) = csv.as_mut() {
                    let e = a.every.as_fs();
                    let mut k = now.as_fs() / e + 1;
                    while k * e <= g.as_fs() {
                        let t = Time::from_fs(k * e);
                        let mut row = format!("{}", t.as_fs());
                        for p in &a.probes {
                            let v = eng.sample(p, t).map_err(|e| e.to_string())?;
                            row.push_str(&format!(",{v:.9e}"));
                        }
                        writeln!(w, "{row}").map_err(|e| e.to_string())?;
                        k += 1;
                    }
                }
                now = g;
            }
            points = eng.solver_points();
        }
    }
    if let Some(w) = csv.as_mut() {
        w.flush().map_err(|e| e.to_string())?;
    }
    let mut probes = String::new();
    for (k, p) in a.probes.iter().enumerate() {
        if k > 0 {
            probes.push(',');
        }
        probes.push_str(&format!(
            "{}:{{\"min\":{:e},\"max\":{:e}}}",
            json_str(p),
            min[k],
            max[k]
        ));
    }
    println!(
        "{{\"status\":{},\"reached_fs\":{},\"stop_fs\":{},\"wall_s\":{:.3},\"solver_points\":{points},\"probes\":{{{probes}}}}}",
        json_str(&status),
        now.as_fs(),
        a.stop.as_fs(),
        t0.elapsed().as_secs_f64()
    );
    if status == "ok" {
        Ok(())
    } else {
        std::process::exit(2)
    }
}

fn main() {
    match parse_args().and_then(|a| run(&a)) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("deck_run: {e}");
            std::process::exit(1);
        }
    }
}
