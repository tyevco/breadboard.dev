//! `bb`: Breadboard command-line interface.
//!
//! Phase 0 command:
//!
//! ```text
//! bb run --netlist <board.cir> --firmware <elf> --target <ref> --for <time>
//!        --probe net:<NET> [--probe net:<NET> ...] --out <dir>
//!        [--max-quantum 10ms] [--renode-quantum 10us] [--sample-rate 100kHz] [--seed 0]
//!        [--renode <path>] [--verify edges] [--no-verify]
//! ```
//!
//! Writes `<out>/events.jsonl`, `<out>/nets.csv` and `<out>/nets.vcd`, and
//! prints the wall-clock time to stdout (never into the output files, which
//! must stay byte-identical across runs).
//!
//! The kernel advances engines in the plan order it builds from the bindings
//! (ADR-003 D3): the MCU drives the analog engine's inputs, so it advances
//! first, and every MCU edge of a grant (its trace) reaches ngspice before
//! ngspice advances through it, at its exact Renode timestamp. The
//! registration order does not matter.

mod board;

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use bb_analog_ngspice::{Drive, InputBinding, NgspiceConfig, NgspiceEngine, OutputBinding};
use bb_cpu_renode::{find_renode, PinDirection, PinSpec, RenodeConfig, RenodeEngine, Verify};
use bbcore::{
    CsvWriter, Direction, Engine, EventSink, Level, Probe, Scheduler, SchedulerConfig, Time,
    VcdKind, VcdWriter,
};

const USAGE: &str = "\
usage: bb run --netlist <board.cir> --firmware <elf> --target <ref> --for <time>
              --probe net:<NET> [--probe net:<NET> ...] --out <dir>
              [--max-quantum 10ms] [--renode-quantum 10us] [--sample-rate 100kHz] [--seed 0]
              [--renode <path>] [--verify edges] [--no-verify]

  --netlist      board file: SPICE body plus *bb directives (examples/spike/board.cir)
  --firmware     ELF loaded into the target MCU
  --target       part reference the firmware runs on (e.g. U1)
  --for          simulated duration, e.g. 500ms
  --probe        net to record; a circuit net (analog volts) or an MCU pin net
                 <ref>.<pin> (digital level). Repeatable; order = CSV column order
  --out          output directory (events.jsonl, nets.csv, nets.vcd)
  --max-quantum  cap on the length of one grant (default 10ms); a multiple of
                 the Renode quantum. A pure cap: it bounds the wall time of
                 one Renode RUN_FOR, buffered edges and output latency, and
                 never changes results. It no longer sets Renode's quantum
                 (ADR-003 D2, D4). A board with a feedback loop runs in grants
                 of one Renode quantum whatever this is
  --renode-quantum
                 Renode's global quantum, the grant lattice L (default 10us):
                 every grant boundary is a multiple of it, firmware timing,
                 edge stamps and MCU inputs resolve to it, and ngspice steps at
                 most L and lands on every multiple of it. Run configuration:
                 changing it changes results (ADR-003 D2)
  --sample-rate  nets.csv and nets.vcd sample rate (default 100kHz). Samples
                 never change the grant partition; inside a grant analog nets
                 are interpolated between solver points and digital nets read
                 from the edge trace
  --seed         Renode emulation seed (default 0)
  --renode       path to the renode launcher (default: $BB_RENODE, PATH, ~/.local/bin)
  --verify       when to cross-check Renode's virtual time and pin states against
                 the engine: every (each grant), edges (grants with an edge or
                 input, and the end; default) or final (end only). Pure reads:
                 outputs are identical in every mode
  --no-verify    alias for --verify final";

struct RunArgs {
    netlist: PathBuf,
    firmware: PathBuf,
    target: String,
    duration: Time,
    probes: Vec<String>,
    out: PathBuf,
    max_quantum: Time,
    renode_quantum: Time,
    sample_period: Time,
    seed: i32,
    renode: Option<PathBuf>,
    verify: Verify,
}

/// Parse `<number>[.<frac>]<unit>` with unit Hz, kHz, MHz or GHz into an exact
/// sample period in femtoseconds.
fn parse_rate(s: &str) -> Result<Time, String> {
    let t = s.trim();
    let split = t
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .ok_or_else(|| format!("sample rate {s:?}: missing unit (Hz, kHz, MHz, GHz)"))?;
    let (num, unit) = t.split_at(split);
    let mult: u128 = match unit {
        "Hz" => 1,
        "kHz" => 1_000,
        "MHz" => 1_000_000,
        "GHz" => 1_000_000_000,
        _ => return Err(format!("sample rate {s:?}: unknown unit {unit:?}")),
    };
    let (i, f) = num.split_once('.').unwrap_or((num, ""));
    if (i.is_empty() && f.is_empty()) || f.contains('.') {
        return Err(format!("sample rate {s:?}: bad number"));
    }
    let digits: u128 = format!("{i}{f}")
        .parse()
        .map_err(|_| format!("sample rate {s:?}: bad number"))?;
    let scale = 10u128
        .checked_pow(u32::try_from(f.len()).map_err(|_| "too many digits".to_string())?)
        .ok_or_else(|| format!("sample rate {s:?}: too many digits"))?;
    // period = 1 / (digits / scale * mult) s = scale * 1e15 / (digits * mult) fs
    let den = digits
        .checked_mul(mult)
        .filter(|d| *d > 0)
        .ok_or_else(|| format!("sample rate {s:?} must be > 0"))?;
    let num = scale * 1_000_000_000_000_000u128;
    if !num.is_multiple_of(den) {
        return Err(format!(
            "sample rate {s:?}: period is not a whole number of femtoseconds"
        ));
    }
    let fs = u64::try_from(num / den).map_err(|_| format!("sample rate {s:?} too low"))?;
    Ok(Time::from_fs(fs))
}

fn need<T>(o: Option<T>, n: &str) -> Result<T, String> {
    o.ok_or_else(|| format!("{n} is required"))
}

fn parse_args(args: &[String]) -> Result<RunArgs, String> {
    let mut netlist = None;
    let mut firmware = None;
    let mut target = None;
    let mut duration = None;
    let mut probes = Vec::new();
    let mut out = None;
    let mut max_quantum = Time::from_ms(10);
    let mut renode_quantum = Time::from_us(10);
    let mut sample_period = Time::from_us(10);
    let mut seed = 0i32;
    let mut renode = None;
    let mut verify = Verify::Edges;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = |name: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match a.as_str() {
            "--netlist" => netlist = Some(PathBuf::from(val(a)?)),
            "--firmware" => firmware = Some(PathBuf::from(val(a)?)),
            "--target" => target = Some(val(a)?),
            "--for" => duration = Some(val(a)?.parse::<Time>().map_err(|e| format!("--for: {e}"))?),
            "--probe" => {
                let v = val(a)?;
                let net = v
                    .strip_prefix("net:")
                    .filter(|n| !n.is_empty())
                    .ok_or_else(|| format!("--probe {v:?}: expected net:<NET>"))?;
                if probes.iter().any(|p: &String| p == net) {
                    return Err(format!("--probe {v}: given twice"));
                }
                probes.push(net.to_string());
            }
            "--out" => out = Some(PathBuf::from(val(a)?)),
            "--max-quantum" => {
                max_quantum = val(a)?
                    .parse::<Time>()
                    .map_err(|e| format!("--max-quantum: {e}"))?
            }
            "--renode-quantum" => {
                renode_quantum = val(a)?
                    .parse::<Time>()
                    .map_err(|e| format!("--renode-quantum: {e}"))?
            }
            "--sample-rate" => sample_period = parse_rate(&val(a)?)?,
            "--seed" => {
                seed = val(a)?
                    .parse()
                    .map_err(|_| "--seed: expected an i32".to_string())?
            }
            "--renode" => renode = Some(PathBuf::from(val(a)?)),
            "--verify" => verify = val(a)?.parse().map_err(|e| format!("--verify: {e}"))?,
            "--no-verify" => verify = Verify::Final,
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    let duration: Time = need(duration, "--for")?;
    if duration.is_zero() {
        return Err("--for must be > 0".into());
    }
    if renode_quantum.is_zero() || !renode_quantum.as_fs().is_multiple_of(1_000_000) {
        return Err("--renode-quantum must be a positive whole number of ns".into());
    }
    if max_quantum.is_zero() || !max_quantum.as_fs().is_multiple_of(renode_quantum.as_fs()) {
        return Err(format!(
            "--max-quantum {max_quantum} must be a positive multiple of --renode-quantum {renode_quantum}"
        ));
    }
    if !duration.as_fs().is_multiple_of(renode_quantum.as_fs()) {
        return Err(format!(
            "--for {duration} must be a multiple of --renode-quantum {renode_quantum}"
        ));
    }
    if probes.is_empty() {
        return Err("at least one --probe is required".into());
    }
    Ok(RunArgs {
        netlist: need(netlist, "--netlist")?,
        firmware: need(firmware, "--firmware")?,
        target: need(target, "--target")?,
        duration,
        probes,
        out: need(out, "--out")?,
        max_quantum,
        renode_quantum,
        sample_period,
        seed,
        renode,
        verify,
    })
}

fn create(path: &PathBuf) -> Result<BufWriter<File>, String> {
    File::create(path)
        .map(BufWriter::new)
        .map_err(|e| format!("create {}: {e}", path.display()))
}

fn run(a: RunArgs) -> Result<(), String> {
    let text =
        fs::read_to_string(&a.netlist).map_err(|e| format!("read {}: {e}", a.netlist.display()))?;
    let board = board::parse(&text).map_err(|e| format!("{}: {e}", a.netlist.display()))?;
    if !a.firmware.is_file() {
        return Err(format!("firmware {} not found", a.firmware.display()));
    }
    let part = board
        .parts
        .iter()
        .find(|p| p.reference == a.target)
        .ok_or_else(|| format!("--target {}: no such part in the board file", a.target))?;
    if board.parts.len() != 1 {
        return Err("Phase 0 supports exactly one MCU part per board".into());
    }
    let mcu_pins: Vec<&board::PinBinding> = board
        .pins
        .iter()
        .filter(|p| p.part == part.reference)
        .collect();
    if mcu_pins.is_empty() {
        return Err(format!("part {} has no *bb pin bindings", part.reference));
    }
    let digital_nets: Vec<String> = mcu_pins.iter().map(|p| p.kernel_net()).collect();

    // --- CPU engine.
    let renode = match &a.renode {
        Some(p) => p.clone(),
        None => find_renode().ok_or("renode not found (run `make setup` or pass --renode)")?,
    };
    let mut rcfg = RenodeConfig::stm32f103(renode, a.firmware.clone());
    rcfg.name = part.reference.clone();
    rcfg.quantum = a.renode_quantum;
    rcfg.seed = a.seed;
    rcfg.verify = a.verify;
    rcfg.pins = mcu_pins
        .iter()
        .map(|p| PinSpec::stm32(&p.pin, PinDirection::Output))
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;

    // --- Analog engine: the board's SPICE body plus generated pin glue.
    // ADR-003 D2: TMAX is the lattice, not min(max_quantum, sample period),
    // and ngspice lands on every lattice point, so its solver points do not
    // depend on the grant partition or the sample rate.
    let mut ncfg = NgspiceConfig::new(board.spice.clone(), a.duration, a.renode_quantum);
    ncfg.lattice = Some(a.renode_quantum);
    for p in &mcu_pins {
        ncfg.inputs.push(InputBinding {
            pin: p.kernel_net(),
            source: format!("V_bb_{}_{}", p.part, p.pin),
            drive: Drive::Digital {
                model: p.model,
                initial: Level::Low,
            },
            glue_node: Some(p.net.clone()),
        });
    }
    let analog_probes: Vec<&String> = a
        .probes
        .iter()
        .filter(|n| !digital_nets.contains(n))
        .collect();
    for n in &analog_probes {
        ncfg.outputs.push(OutputBinding {
            pin: (*n).clone(),
            node: (*n).clone(),
        });
    }

    fs::create_dir_all(&a.out).map_err(|e| format!("create {}: {e}", a.out.display()))?;
    let wall = Instant::now();
    let cpu = RenodeEngine::start(&rcfg).map_err(|e| format!("start Renode: {e}"))?;
    let analog = NgspiceEngine::new(ncfg).map_err(|e| format!("start ngspice: {e}"))?;
    let analog_name = analog.name().to_string();
    let startup_s = wall.elapsed().as_secs_f64();

    let mut sched = Scheduler::new(SchedulerConfig {
        max_quantum: a.max_quantum,
        loop_quantum: None, // one Renode quantum (ADR-003 D4)
        log_grants: true,
        event_sampling: None,
    })
    .map_err(|e| e.to_string())?;
    let cpu_id = sched.add_engine(Box::new(cpu)).map_err(|e| e.to_string())?;
    let an_id = sched
        .add_engine(Box::new(analog))
        .map_err(|e| e.to_string())?;
    let bind = |s: &mut Scheduler, id, pin: &str, net: &str, d| {
        s.bind(id, pin, net, d).map_err(|e| e.to_string())
    };
    for p in &mcu_pins {
        bind(
            &mut sched,
            cpu_id,
            &p.pin,
            &p.kernel_net(),
            Direction::Output,
        )?;
        bind(
            &mut sched,
            an_id,
            &p.kernel_net(),
            &p.kernel_net(),
            Direction::Input,
        )?;
    }
    for n in &analog_probes {
        bind(&mut sched, an_id, n, n, Direction::Output)?;
    }

    let mut sink = EventSink::new(create(&a.out.join("events.jsonl"))?);
    // Sampled, not aligned: sample instants never become grant boundaries
    // (ADR-002 Amendment 3), and both files are taken at fixed instants
    // inside each grant window, so they do not depend on the grant
    // partition (ADR-003 D3).
    let mut csv = CsvWriter::sampled(
        create(&a.out.join("nets.csv"))?,
        a.sample_period,
        a.probes.clone(),
    )
    .map_err(|e| e.to_string())?;
    let mut vcd_vars: Vec<(&str, VcdKind)> = Vec::new();
    for n in &a.probes {
        let kind = if digital_nets.contains(n) {
            VcdKind::Wire
        } else {
            VcdKind::Real
        };
        vcd_vars.push((n.as_str(), kind));
    }
    for n in &digital_nets {
        if !a.probes.contains(n) {
            vcd_vars.push((n.as_str(), VcdKind::Wire));
        }
    }
    let mut vcd = VcdWriter::sampled(create(&a.out.join("nets.vcd"))?, &vcd_vars, a.sample_period)
        .map_err(|e| e.to_string())?;

    let t_run = Instant::now();
    let summary = {
        let mut probes: [&mut dyn Probe; 2] = [&mut csv, &mut vcd];
        let r = sched.run(a.duration, &mut sink, &mut probes);
        // On an error, the records of every completed grant are written
        // before the error is reported (ADR-003 D4).
        for p in probes.iter_mut() {
            p.finish().map_err(|e| format!("write outputs: {e}"))?;
        }
        sink.flush().map_err(|e| format!("write events: {e}"))?;
        r.map_err(|e| format!("simulation failed: {e}"))?
    };
    sink.flush().map_err(|e| format!("write events: {e}"))?;
    let run_s = t_run.elapsed().as_secs_f64();
    drop(sched); // stops Renode and releases ngspice
    let total_s = wall.elapsed().as_secs_f64();

    let mut so = std::io::stdout().lock();
    let _ = writeln!(
        so,
        "bb run: simulated {} in {} grants (max quantum {}, renode quantum {}, engines {} -> {analog_name})",
        summary.now, summary.grants, a.max_quantum, a.renode_quantum, part.reference
    );
    let _ = writeln!(
        so,
        "bb run: wall time {total_s:.2} s total ({startup_s:.2} s engine startup, {run_s:.2} s simulation); outputs in {}",
        a.out.display()
    );
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("run") => match parse_args(&args[1..]).and_then(run) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("bb: {e}");
                ExitCode::FAILURE
            }
        },
        Some("-h" | "--help" | "help") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_are_exact() {
        assert_eq!(parse_rate("1MHz").unwrap(), Time::from_us(1));
        assert_eq!(parse_rate("100kHz").unwrap(), Time::from_us(10));
        assert_eq!(parse_rate("10kHz").unwrap(), Time::from_us(100));
        assert_eq!(parse_rate("2.5MHz").unwrap(), Time::from_ns(400));
        assert!(parse_rate("3Hz").is_err()); // 333.33.. ms is not whole fs
        assert!(parse_rate("0Hz").is_err());
        assert!(parse_rate("10").is_err());
    }

    #[test]
    fn args_parse() {
        let v: Vec<String> = "--netlist b.cir --firmware f.elf --target U1 --for 500ms \
             --probe net:LED_A --probe net:PA5 --out out --sample-rate 1MHz"
            .split(' ')
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();
        let a = parse_args(&v).unwrap();
        assert_eq!(a.duration, Time::from_ms(500));
        assert_eq!(a.probes, vec!["LED_A", "PA5"]);
        assert_eq!(a.max_quantum, Time::from_ms(10));
        assert_eq!(a.renode_quantum, Time::from_us(10));
        let mut q = v.clone();
        q.extend([
            "--max-quantum".into(),
            "1ms".into(),
            "--renode-quantum".into(),
            "100us".into(),
        ]);
        let qa = parse_args(&q).unwrap();
        assert_eq!(
            (qa.max_quantum, qa.renode_quantum),
            (Time::from_ms(1), Time::from_us(100))
        );
        let mut off = v.clone();
        off.extend(["--max-quantum".into(), "15us".into()]);
        assert!(parse_args(&off).is_err(), "max quantum off the lattice");
        assert_eq!(a.sample_period, Time::from_us(1));
        assert_eq!(a.verify, Verify::Edges);
        let mut v2 = v.clone();
        v2.extend(["--verify".to_string(), "every".to_string()]);
        assert_eq!(parse_args(&v2).unwrap().verify, Verify::Every);
        v2.push("--no-verify".into());
        assert_eq!(parse_args(&v2).unwrap().verify, Verify::Final);
        let mut v3 = v.clone();
        v3.extend(["--verify".to_string(), "sometimes".to_string()]);
        assert!(parse_args(&v3).is_err());
        let mut bad = v.clone();
        bad.push("--bogus".into());
        assert!(parse_args(&bad).is_err());
    }
}
