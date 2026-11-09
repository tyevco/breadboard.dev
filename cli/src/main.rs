//! `bb`: Breadboard command-line interface.
//!
//! Phase 0 command:
//!
//! ```text
//! bb run --netlist <board.cir> --firmware <elf> --target <ref> --for <time>
//!        --probe net:<NET> [--probe net:<NET> ...] --out <dir>
//!        [--max-quantum 10ms] [--renode-quantum 10us] [--sample-rate 100kHz] [--seed 0]
//!        [--renode <path>] [--verify edges] [--no-verify] [--engine-order mcu-first]
//!        [--lookahead on|off]
//! ```
//!
//! Writes `<out>/events.jsonl`, `<out>/nets.csv` and `<out>/nets.vcd`, and
//! prints the wall-clock time to stdout (never into the output files, which
//! must stay byte-identical across runs).
//!
//! The kernel advances engines in the plan order it builds from the bindings
//! (ADR-003 D3). When the MCU only drives the analog engine (the spike), it
//! advances first, and every MCU edge of a grant (its trace) reaches ngspice
//! before ngspice advances through it, at its exact Renode timestamp. When
//! ngspice only drives MCU inputs (`digital_in`, `examples/feedback-open`),
//! ngspice goes first and Renode applies each thresholded transition at
//! `ceil_L(t)`. With both (a feedback loop, `examples/feedback`) the grants
//! are one Renode quantum. The registration order does not matter
//! (`--engine-order`).
//!
//! A board with a feedback loop loads Renode's lookahead plugin
//! (ADR-003 D5) whether `--lookahead` is on or off, so both runs have the
//! same platform. Lookahead is on by default for such boards (ADR-003 plan
//! step 8): the kernel lifts the loop cap while the sleeping MCU promises to
//! stay quiet (`quiet_until`); lookahead changes only the grant partition,
//! never the results. `--lookahead off` gives the one-cell reference.
//!
//! An `adc_in` pin (ADR-003 D6; `platform=stm32f072`, `examples/adc`) is an
//! MCU input too: ngspice pushes the node at the lattice points where the
//! delivered whole millivolts change (`--adc-push every`: at every lattice
//! point, the reference) and Renode sets each value on the ADC channel
//! while paused there. With lookahead, a push stops ngspice early.
//!
//! With `BB_RENODE_POOL` set (`bb pool run`, ADR-004 Part B) the Renode
//! engine leases a pre-warmed Renode from that pool when one is idle and
//! starts one fresh otherwise; `--renode-pool off` ignores the pool. The
//! outputs are the same either way.

mod board;
mod pool;

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use bb_analog_ngspice::{
    AdcOutputBinding, AdcPush, DigitalOutputBinding, Drive, InputBinding, NgspiceConfig,
    NgspiceEngine, OutputBinding,
};
use bb_cpu_renode::{
    find_renode, LookaheadCfg, PinDirection, PinSpec, Platform, RenodeConfig, RenodeEngine, Verify,
};
use bbcore::{
    CsvWriter, Direction, Engine, EventSink, Level, Probe, Scheduler, SchedulerConfig, Time,
    VcdKind, VcdWriter,
};
use board::PinKind;

const USAGE: &str = "\
usage: bb run --netlist <board.cir> --firmware <elf> --target <ref> --for <time>
              --probe net:<NET> [--probe net:<NET> ...] --out <dir>
              [--max-quantum 10ms] [--renode-quantum 10us] [--sample-rate 100kHz] [--seed 0]
              [--renode <path>] [--verify edges] [--no-verify] [--engine-order mcu-first]
              [--lookahead on|off] [--adc-push change|every] [--renode-pool env|off]
       bb pool run|start|stop|status ...   (bb pool --help)

  --netlist    board file: SPICE body plus *bb directives (examples/spike/board.cir)
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
                 of one Renode quantum, except where --lookahead lifts that
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
  --no-verify    alias for --verify final
  --engine-order registration order: mcu-first (default) or analog-first. The
                 advance order comes from the bindings, so outputs are
                 identical either way (ADR-003 D3; a test knob)
  --lookahead    CPU lookahead for boards with a feedback loop (ADR-003 D5):
                 on (the default whenever the lookahead plugin is loaded) or
                 off. With on, grants grow past one Renode quantum while the
                 MCU sleeps in WFI with nothing pending and promises to stay
                 quiet until its next timer event; the promise is audited and
                 a broken one fails the run. Changes only the grant records,
                 never the results: off is the one-cell reference. Such
                 boards load Renode's lookahead plugin in both modes
  --lookahead-plugin
                 auto (default: boards with a feedback loop), always or never.
                 The plugin is inert, so outputs are identical (a test knob)
  --adc-push     when an adc_in pin's value reaches the MCU's ADC (ADR-003
                 D6): change (default), at the lattice points where its whole
                 millivolts change, which is all Renode's ADC converts; or
                 every lattice point, with the value in whole microvolts
                 (the reference of the equivalence gate). The MCU sees the
                 same conversions either way (a test knob)
  --renode-pool  env (default): lease a pre-warmed Renode from the pool in
                 $BB_RENODE_POOL when one is idle (`bb pool run`, ADR-004),
                 else start Renode fresh; off: always fresh. Never changes
                 the outputs";

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
    engine_order: EngineOrder,
    /// `--lookahead`; `None` = the default, on whenever the plugin is
    /// loaded (ADR-003 plan step 8).
    lookahead: Option<bool>,
    plugin: PluginMode,
    adc_push: AdcPush,
    /// `--renode-pool env` (true, the default) or `off`.
    renode_pool: bool,
}

fn parse_adc_push(s: &str) -> Result<AdcPush, String> {
    match s {
        "change" => Ok(AdcPush::OnChange),
        "every" => Ok(AdcPush::EveryLattice),
        _ => Err(format!("{s:?}: expected change or every")),
    }
}

/// When to load Renode's lookahead plugin (`--lookahead-plugin`). The plugin
/// is inert (ADR-003 step 5), so the choice must not change any output.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum PluginMode {
    /// Boards whose binding graph has a feedback loop.
    Auto,
    Always,
    Never,
}

impl std::str::FromStr for PluginMode {
    type Err = String;
    fn from_str(s: &str) -> Result<PluginMode, String> {
        match s {
            "auto" => Ok(PluginMode::Auto),
            "always" => Ok(PluginMode::Always),
            "never" => Ok(PluginMode::Never),
            _ => Err(format!("{s:?}: expected auto, always or never")),
        }
    }
}

fn parse_on_off(s: &str) -> Result<bool, String> {
    match s {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err(format!("{s:?}: expected on or off")),
    }
}

/// Registration order of the two engines (`--engine-order`). It must not
/// change any output (ADR-003 D3); the option exists for that gate.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum EngineOrder {
    McuFirst,
    AnalogFirst,
}

impl std::str::FromStr for EngineOrder {
    type Err = String;
    fn from_str(s: &str) -> Result<EngineOrder, String> {
        match s {
            "mcu-first" => Ok(EngineOrder::McuFirst),
            "analog-first" => Ok(EngineOrder::AnalogFirst),
            _ => Err(format!("{s:?}: expected mcu-first or analog-first")),
        }
    }
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
    let mut engine_order = EngineOrder::McuFirst;
    let mut lookahead = None;
    let mut plugin = PluginMode::Auto;
    let mut adc_push = AdcPush::OnChange;
    let mut renode_pool = true;
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
            "--engine-order" => {
                engine_order = val(a)?
                    .parse()
                    .map_err(|e| format!("--engine-order: {e}"))?
            }
            "--lookahead" => {
                lookahead = Some(parse_on_off(&val(a)?).map_err(|e| format!("--lookahead: {e}"))?)
            }
            "--lookahead-plugin" => {
                plugin = val(a)?
                    .parse()
                    .map_err(|e| format!("--lookahead-plugin: {e}"))?
            }
            "--adc-push" => {
                adc_push = parse_adc_push(&val(a)?).map_err(|e| format!("--adc-push: {e}"))?
            }
            "--renode-pool" => {
                renode_pool = match val(a)?.as_str() {
                    "env" => true,
                    "off" => false,
                    v => return Err(format!("--renode-pool: {v:?}: expected env or off")),
                }
            }
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
    if lookahead == Some(true) && plugin == PluginMode::Never {
        return Err("--lookahead on needs the lookahead plugin (--lookahead-plugin never)".into());
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
        engine_order,
        lookahead,
        plugin,
        adc_push,
        renode_pool,
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
    // MCU pin nets carrying a digital level, and those carrying an ADC
    // input's pushed value (ADR-003 D6).
    let digital_nets: Vec<String> = mcu_pins
        .iter()
        .filter(|p| !matches!(p.kind, PinKind::Adc { .. }))
        .map(|p| p.kernel_net())
        .collect();
    let adc_nets: Vec<String> = mcu_pins
        .iter()
        .filter(|p| matches!(p.kind, PinKind::Adc { .. }))
        .map(|p| p.kernel_net())
        .collect();

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
    // ADR-004 Part B: only $BB_RENODE_POOL names a pool (approver decision 4).
    rcfg.pool = if a.renode_pool {
        bb_cpu_renode::pool::from_env()
    } else {
        None
    };
    if part.platform == "stm32f072" {
        // Renode 1.17.0's stock STM32F072 (ADC = Analog.STM32F0_ADC,
        // instance `adc`; it loads offline). ADR-003 Amendment 5.
        rcfg.platform = Platform::Repl("platforms/cpus/stm32f072.repl".into());
    }
    rcfg.pins = mcu_pins
        .iter()
        .map(|p| match p.kind {
            PinKind::In { .. } => PinSpec::stm32(&p.pin, PinDirection::Input),
            PinKind::Out { .. } | PinKind::OutUnconnected => {
                PinSpec::stm32(&p.pin, PinDirection::Output)
            }
            PinKind::Adc { channel, .. } => Ok(PinSpec::adc(&p.pin, "adc", channel)),
        })
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    // ADR-003 D5: the binding graph has a feedback loop when the MCU both
    // drives the circuit and reads it (U1 -> ngspice and ngspice -> U1; one
    // MCU and one analog engine in Phase 1). Such boards load the lookahead
    // plugin, with --lookahead off as well, so both modes run the same
    // platform. Checked against the kernel's plan after the run.
    let cyclic = mcu_pins
        .iter()
        .any(|p| matches!(p.kind, PinKind::In { .. } | PinKind::Adc { .. }))
        && mcu_pins
            .iter()
            .any(|p| matches!(p.kind, PinKind::Out { .. }));
    let plugin = match a.plugin {
        PluginMode::Auto => cyclic,
        PluginMode::Always => true,
        PluginMode::Never => false,
    };
    // ADR-003 plan step 8: lookahead is on by default wherever the plugin
    // is loaded (boards with a feedback loop); `--lookahead off` gives the
    // one-cell reference. Without the plugin there is no promise to give.
    let lookahead = a.lookahead.unwrap_or(plugin);
    rcfg.lookahead = LookaheadCfg {
        plugin,
        enabled: lookahead && plugin,
        ..LookaheadCfg::default()
    };

    // --- Analog engine: the board's SPICE body plus generated pin glue.
    // ADR-003 D2: TMAX is the lattice, not min(max_quantum, sample period),
    // and ngspice lands on every lattice point, so its solver points do not
    // depend on the grant partition or the sample rate.
    let mut ncfg = NgspiceConfig::new(board.spice.clone(), a.duration, a.renode_quantum);
    ncfg.lattice = Some(a.renode_quantum);
    for p in &mcu_pins {
        match &p.kind {
            // An MCU output into the circuit: EXTERNAL source + zout glue.
            PinKind::Out { net, model } => ncfg.inputs.push(InputBinding {
                pin: p.kernel_net(),
                source: format!("V_bb_{}_{}", p.part, p.pin),
                drive: Drive::Digital {
                    model: *model,
                    initial: Level::Low,
                },
                glue_node: Some(net.clone()),
            }),
            // An MCU input: ngspice thresholds the node (ADR-003 D3) and
            // adds cin.
            PinKind::In { net, model } => ncfg.digital_outputs.push(DigitalOutputBinding {
                pin: p.kernel_net(),
                node: net.clone(),
                model: *model,
            }),
            // An MCU ADC input: ngspice pushes the node's value at lattice
            // points (ADR-003 D6).
            PinKind::Adc { net, model, .. } => ncfg.adc_outputs.push(AdcOutputBinding {
                pin: p.kernel_net(),
                node: net.clone(),
                model: *model,
            }),
            PinKind::OutUnconnected => {}
        }
    }
    ncfg.adc_push = a.adc_push;
    let analog_probes: Vec<&String> = a
        .probes
        .iter()
        .filter(|n| !digital_nets.contains(n) && !adc_nets.contains(n))
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
    let renode_how = if cpu.leased() {
        format!("leased from the pool (pid {})", cpu.renode_pid())
    } else {
        format!("started fresh (pid {})", cpu.renode_pid())
    };
    let analog = NgspiceEngine::new(ncfg).map_err(|e| format!("start ngspice: {e}"))?;
    let analog_name = analog.name().to_string();
    let startup_s = wall.elapsed().as_secs_f64();

    let mut sched = Scheduler::new(SchedulerConfig {
        max_quantum: a.max_quantum,
        loop_quantum: None, // one Renode quantum (ADR-003 D4)
        lookahead,
        log_grants: true,
        event_sampling: None,
    })
    .map_err(|e| e.to_string())?;
    // Per-grant plans, for the lookahead line of the run summary only
    // (diagnostics; they never reach the output files).
    sched.record_plans(lookahead);
    // Registration order only breaks ties (plan order, record order); the
    // advance order comes from the bindings (ADR-003 D3).
    let (cpu_id, an_id) = match a.engine_order {
        EngineOrder::McuFirst => {
            let c = sched.add_engine(Box::new(cpu)).map_err(|e| e.to_string())?;
            let n = sched
                .add_engine(Box::new(analog))
                .map_err(|e| e.to_string())?;
            (c, n)
        }
        EngineOrder::AnalogFirst => {
            let n = sched
                .add_engine(Box::new(analog))
                .map_err(|e| e.to_string())?;
            let c = sched.add_engine(Box::new(cpu)).map_err(|e| e.to_string())?;
            (c, n)
        }
    };
    let bind = |s: &mut Scheduler, id, pin: &str, net: &str, d| {
        s.bind(id, pin, net, d).map_err(|e| e.to_string())
    };
    for p in &mcu_pins {
        let k = p.kernel_net();
        match p.kind {
            PinKind::Out { .. } => {
                bind(&mut sched, cpu_id, &p.pin, &k, Direction::Output)?;
                bind(&mut sched, an_id, &k, &k, Direction::Input)?;
            }
            PinKind::OutUnconnected => bind(&mut sched, cpu_id, &p.pin, &k, Direction::Output)?,
            PinKind::In { .. } | PinKind::Adc { .. } => {
                bind(&mut sched, an_id, &k, &k, Direction::Output)?;
                bind(&mut sched, cpu_id, &p.pin, &k, Direction::Input)?;
            }
        }
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
    if let Some(p) = sched.plan() {
        if p.cyclic != cyclic {
            return Err(format!(
                "internal: the board was classified as {} but the kernel's plan is {}",
                if cyclic { "cyclic" } else { "acyclic" },
                if p.cyclic { "cyclic" } else { "acyclic" }
            ));
        }
    }
    let plan = match sched.plan() {
        Some(p) => {
            let names = p
                .order
                .iter()
                .map(|&i| {
                    if i == cpu_id.index() {
                        part.reference.as_str()
                    } else {
                        analog_name.as_str()
                    }
                })
                .collect::<Vec<_>>()
                .join(" -> ");
            format!("{names}{}", if p.cyclic { ", feedback loop" } else { "" })
        }
        None => String::new(),
    };
    // Lookahead diagnostics (ADR-003 D5): grants with a Query and quiet
    // grants. Every quiet grant was audited; a broken promise would have
    // failed the run with PromiseBroken above.
    let plans = sched.plan_log();
    let queries = plans.iter().filter(|p| !p.queried.is_empty()).count();
    let quiet = plans.iter().filter(|p| !p.quiet.is_empty()).count();
    drop(sched); // stops Renode and releases ngspice
    let total_s = wall.elapsed().as_secs_f64();

    let mut so = std::io::stdout().lock();
    let _ = writeln!(
        so,
        "bb run: simulated {} in {} grants (max quantum {}, renode quantum {}, plan {plan}, \
         lookahead {}{})",
        summary.now,
        summary.grants,
        a.max_quantum,
        a.renode_quantum,
        if lookahead { "on" } else { "off" },
        if plugin { ", plugin loaded" } else { "" }
    );
    if lookahead {
        let _ = writeln!(
            so,
            "bb run: lookahead: {queries} grants with a Query, {quiet} quiet grants, \
             0 promises broken"
        );
    }
    let _ = writeln!(so, "bb run: renode: {renode_how}");
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
        Some("pool") => pool::main(&args[1..]),
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
        assert_eq!(a.engine_order, EngineOrder::McuFirst);
        let mut o = v.clone();
        o.extend(["--engine-order".into(), "analog-first".into()]);
        assert_eq!(
            parse_args(&o).unwrap().engine_order,
            EngineOrder::AnalogFirst
        );
        o.extend(["--engine-order".into(), "random".into()]);
        assert!(parse_args(&o).is_err());
        // Default: on wherever the plugin is loaded (decided in run()).
        assert!(a.lookahead.is_none() && a.plugin == PluginMode::Auto);
        let mut la = v.clone();
        la.extend(["--lookahead".into(), "on".into()]);
        assert_eq!(parse_args(&la).unwrap().lookahead, Some(true));
        la.extend(["--lookahead-plugin".into(), "always".into()]);
        assert_eq!(parse_args(&la).unwrap().plugin, PluginMode::Always);
        la.extend(["--lookahead-plugin".into(), "never".into()]);
        assert!(parse_args(&la).is_err(), "on without the plugin");
        let mut lo = v.clone();
        lo.extend(["--lookahead".into(), "off".into()]);
        assert_eq!(parse_args(&lo).unwrap().lookahead, Some(false));
        let mut ln = v.clone();
        ln.extend(["--lookahead-plugin".into(), "never".into()]);
        assert!(
            parse_args(&ln).unwrap().lookahead.is_none(),
            "default without the plugin"
        );
        let mut lb = v.clone();
        lb.extend(["--lookahead".into(), "maybe".into()]);
        assert!(parse_args(&lb).is_err());
        assert!(a.renode_pool, "default: the pool in $BB_RENODE_POOL");
        let mut po = v.clone();
        po.extend(["--renode-pool".into(), "off".into()]);
        assert!(!parse_args(&po).unwrap().renode_pool);
        po.extend(["--renode-pool".into(), "always".into()]);
        assert!(parse_args(&po).is_err());
    }
}
