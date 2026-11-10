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
//!
//! Phase 1 M3 (ADR-006): a board file's part may come from a package
//! (`*bb part U1 package=st/stm32f103c8`), its pin models from the
//! package's `pins.toml`; such a board needs a current `bb.lock` next to it
//! (`bb open <board.cir>`), which `bb run` checks and never writes. `bb pkg`
//! lints and tests packages.

mod board;
mod elab;
mod flash;
mod image;
mod import;
mod names;
mod open;
mod pkg;
mod platforms;
mod pool;
mod probe_cmd;
mod project;

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
    find_renode, LookaheadCfg, PinCnfAudit, PinCnfExpect, PinDirection, PinSpec, Platform,
    RenodeConfig, RenodeEngine, Verify,
};
use bbcore::{
    AdcIn, AdcKey, CsvWriter, Direction, Engine, EventSink, Level, Probe, Scheduler,
    SchedulerConfig, Time, VcdKind, VcdWriter,
};
use board::PinKind;

const USAGE: &str = "\
usage: bb run --netlist <board.cir> --firmware <elf> --target <ref> --for <time>
              --probe net:<NET> [--probe net:<NET> ...] --out <dir>
       bb run [--project <dir>] --for <time> [--bench <bench.toml>] [--fidelity fast]
              [--probe net:<name|alias|pin> ...] --out <dir> [--allow-unqualified <pkg:view>]
              [--max-quantum 10ms] [--renode-quantum 10us] [--sample-rate 100kHz] [--seed 0]
              [--renode <path>] [--verify edges] [--no-verify] [--engine-order mcu-first]
              [--lookahead on|off] [--adc-push change|every] [--adc-model bb|stock]
              [--renode-pool env|off] [--uart-log on|off]
       bb pool run|start|stop|status ...   (bb pool --help)
       bb open [<dir>|<.kicad_pro>|<.net>|<board.cir>] [--check] [--update-lock]   (bb open --help)
       bb flash --target <instance> <file.elf> [--kind elf] [--project <dir>]
       bb probe [--list] [net:<name> ...] [--project <dir>]
       bb pkg lint|test|migrate|tree|value ...         (bb pkg --help)

  --netlist    board file: SPICE body plus *bb directives (examples/spike/board.cir).
                 Parts given by package= need a current bb.lock next to it
                 (bb open <board.cir>, ADR-006 D8)
  --project    a bb.toml project (ADR-007; the default when --netlist is not
                 given: the current directory). Reads board.bbm, which must be
                 current (`bb open`), the firmware from [targets] (`bb flash`),
                 [[bind]] pins and [probes]; writes run.json too. Never writes
                 into the project
  --bench      bench file (supplies, loads, temperature, analog scope, option
                 set; ADR-007 D5). Default: no sources, every net in scope,
                 option set phase0
  --fidelity   the analog fidelity rung (Phase 1: fast)
  --elaborate-only
                 a project: elaborate (checks, deck, probes) and write
                 <out>/deck.cir and run.json, then stop without starting any
                 engine
  --allow-unqualified
                 run a view that is not lattice-qualified for (lattice, option
                 set) (ADR-006 D6; recorded in run.json). Repeatable
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
  --adc-model    the STM32F072's ADC (ADR-005): bb (default), Breadboard's
                 model (hardware triggers, conversion timing, the analog
                 watchdog; pushes are timed inputs, a sampled input), or
                 stock, Renode's STM32F0_ADC on the stock platform (ADR-003
                 D6: SetValue at each push, the code-change watch; keep it
                 for comparisons). Changes results: each has its own
                 references
  --renode-pool  env (default): lease a pre-warmed Renode from the pool in
                 $BB_RENODE_POOL when one is idle (`bb pool run`, ADR-004),
                 else start Renode fresh; off: always fresh. Never changes
                 the outputs
  --uart-log     on (default) or off: on the nRF52840, every line the firmware
                 writes to UARTE0/1 becomes an info log record at the time of
                 its last byte (ADR-008 D8)";

struct RunArgs {
    /// `--netlist`: a Phase 0 board file; `None`: a project.
    netlist: Option<PathBuf>,
    firmware: Option<PathBuf>,
    target: Option<String>,
    project: Option<PathBuf>,
    bench: Option<PathBuf>,
    fidelity: String,
    allow_unqualified: Vec<String>,
    /// `--elaborate-only`: write deck.cir and run.json, run nothing.
    elaborate_only: bool,
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
    /// `--adc-model`: Breadboard's ADC model (true) or the stock one.
    adc_bb: bool,
    /// `--renode-pool env` (true, the default) or `off`.
    renode_pool: bool,
    /// `--uart-log on` (true, the default) or `off` (ADR-008 D8).
    uart_log: bool,
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
    let mut project = None;
    let mut bench = None;
    let mut fidelity = "fast".to_string();
    let mut allow_unqualified = Vec::new();
    let mut elaborate_only = false;
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
    let mut adc_bb = ADC_MODEL_DEFAULT_BB;
    let mut uart_log = true;
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
            "--project" => project = Some(PathBuf::from(val(a)?)),
            "--bench" => bench = Some(PathBuf::from(val(a)?)),
            "--fidelity" => fidelity = val(a)?,
            "--allow-unqualified" => allow_unqualified.push(val(a)?),
            "--elaborate-only" => elaborate_only = true,
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
            "--adc-model" => {
                adc_bb = match val(a)?.as_str() {
                    "bb" => true,
                    "stock" => false,
                    v => return Err(format!("--adc-model: {v:?}: expected bb or stock")),
                }
            }
            "--uart-log" => {
                uart_log = match val(a)?.as_str() {
                    "on" => true,
                    "off" => false,
                    v => return Err(format!("--uart-log: {v:?}: expected on or off")),
                }
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
    if netlist.is_some() {
        if probes.is_empty() {
            return Err("at least one --probe is required".into());
        }
        if project.is_some() || bench.is_some() || !allow_unqualified.is_empty() {
            return Err(
                "--project, --bench and --allow-unqualified are for projects, not --netlist".into(),
            );
        }
        need(firmware.as_ref(), "--firmware")?;
        need(target.as_ref(), "--target")?;
    } else if firmware.is_some() || target.is_some() {
        return Err(
            "a project takes its firmware and target from bb.toml [targets] (`bb flash`, ADR-007 D7);              --firmware and --target are for --netlist"
                .into(),
        );
    }
    if fidelity != "fast" {
        return Err(format!(
            "--fidelity {fidelity}: Phase 1 runs fast only (ADR-006 D3)"
        ));
    }
    if lookahead == Some(true) && plugin == PluginMode::Never {
        return Err("--lookahead on needs the lookahead plugin (--lookahead-plugin never)".into());
    }
    Ok(RunArgs {
        netlist,
        firmware,
        target,
        project,
        bench,
        fidelity,
        allow_unqualified,
        elaborate_only,
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
        adc_bb,
        renode_pool,
        uart_log,
    })
}

/// The default of `--adc-model`.
const ADC_MODEL_DEFAULT_BB: bool = true;

/// Output writers buffer 1 MiB: every write(2) to a slow file system
/// (WSL's /mnt/c: up to ~10 ms each under host load) is paid during the
/// simulation, and an 8 KiB buffer made ~200 of them for the spike's
/// nets.csv (ADR-004 Amendment 7). The bytes written are the same.
const OUTPUT_BUFFER: usize = 1 << 20;

fn create(path: &PathBuf) -> Result<BufWriter<File>, String> {
    File::create(path)
        .map(|f| BufWriter::with_capacity(OUTPUT_BUFFER, f))
        .map_err(|e| format!("create {}: {e}", path.display()))
}

/// One loading `adc_in` (ADR-008 D7) and its generated glue.
struct LoadGlue {
    /// The bound pin (`P0.02`) and its circuit net (the pad).
    pin: String,
    net: String,
    /// The SAADC input.
    channel: i32,
    /// The CPU engine's toggle output (`P0.02#smp`), its kernel net
    /// (`U1.P0.02#smp`) and the full name `U1.P0.02#smp`.
    cpu_pin: String,
    kernel_net: String,
    cpu_pin_full: String,
    /// The generated glue net the toggle drives (`__smp_U1_P0_02`) and the
    /// view's observe node (`__hold_U1_P0_02`).
    smp_node: String,
    hold_node: String,
    /// The package and its pin_load view.
    package: String,
    view: String,
}

/// The `#smp` toggle's drive into its glue net (ADR-008 D7): 0 V / 1 V,
/// 1 ns edges, no output resistance (the view's edge detector is the only
/// load).
const SMP_DRIVE: bbcore::DigitalOut = bbcore::DigitalOut {
    voh: 1.0,
    vol: 0.0,
    rise: Time::from_ns(1),
    fall: Time::from_ns(1),
    zout: 0.0,
};

/// A name with every character other than an ASCII letter or digit
/// replaced by `_` (glue rule 3).
fn spice_name(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// The loading `adc_in` bindings of a board (ADR-008 D7).
fn load_glue(board: &board::Board, pins: &[&board::PinBinding]) -> Result<Vec<LoadGlue>, String> {
    let mut out = Vec::new();
    for p in pins {
        let PinKind::Adc {
            net,
            channel,
            load: Some(view),
            ..
        } = &p.kind
        else {
            continue;
        };
        let part = board
            .parts
            .iter()
            .find(|x| x.reference == p.part)
            .ok_or_else(|| format!("{}: no part {}", p.pin, p.part))?;
        let package = part
            .package
            .clone()
            .ok_or_else(|| format!("{}.{}: load={view} needs a package part", p.part, p.pin))?;
        let id = spice_name(&format!("{}_{}", p.part, p.pin));
        out.push(LoadGlue {
            pin: p.pin.clone(),
            net: net.clone(),
            channel: *channel,
            cpu_pin: format!("{}#smp", p.pin),
            kernel_net: format!("{}.{}#smp", p.part, p.pin),
            cpu_pin_full: format!("{}.{}#smp", p.part, p.pin),
            smp_node: format!("__smp_{id}"),
            hold_node: format!("__hold_{id}"),
            package,
            view: view.clone(),
        });
    }
    Ok(out)
}

/// The SPICE text of the loads (glue rule 9, ADR-008 D7): the pin_load
/// views' definitions, then one instance per load, its ports in the view's
/// order (pad: the pin's net; ctrl: the toggle's glue net; observe: the
/// hold node; pin: the net of that package pin's binding, which for the
/// SAADC's `VDD` is the VDD input's `adc_in`), with the view's params.
fn load_deck(board: &board::Board, loads: &[LoadGlue]) -> Result<String, String> {
    use bbpkg::package::Port;
    let mut views: Vec<(String, String)> = Vec::new();
    let mut instances = Vec::new();
    for l in loads {
        let pkg = board
            .packages
            .iter()
            .find(|p| p.part.id == l.package)
            .ok_or_else(|| format!("package {} not loaded", l.package))?;
        let v = pkg
            .part
            .analog
            .iter()
            .find(|v| v.name == l.view && v.kind == "pin_load")
            .ok_or_else(|| format!("{}: no pin_load view {}", l.package, l.view))?;
        let path = pkg.dir.join(&v.file);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("{}: {}: {e}", l.package, path.display()))?;
        let label = format!("{}:{}", l.package, l.view);
        if !views.iter().any(|(n, _)| *n == label) {
            views.push((label, text));
        }
        let mut nodes = Vec::new();
        for port in &v.ports {
            nodes.push(match port {
                Port::Pad => l.net.clone(),
                Port::Ctrl { .. } => l.smp_node.clone(),
                Port::Observe { .. } => l.hold_node.clone(),
                Port::OtherPin { pin } => board
                    .pins
                    .iter()
                    .find(|b| b.pin == *pin && matches!(b.kind, PinKind::Adc { .. }))
                    .and_then(|b| match &b.kind {
                        PinKind::Adc { net, .. } => Some(net.clone()),
                        _ => None,
                    })
                    .ok_or_else(|| {
                        format!(
                            "{}: the load view {} needs the net of pin {pin}: bind it \
                             (`*bb pin <part>.{pin} net=<net> model=adc_in vref=<V>`, the SAADC's \
                             {pin} input)",
                            l.pin, l.view
                        )
                    })?,
                Port::Pin(n) => {
                    return Err(format!("{}: a circuit port {n} in a pin_load view", l.view))
                }
            });
        }
        let params: String = v
            .params
            .iter()
            .map(|(k, val)| format!(" {k}={}", spice_value(val)))
            .collect();
        instances.push(format!(
            "X_bbload_{} {} {}{}{}",
            spice_name(l.cpu_pin_full.trim_end_matches("#smp")),
            nodes.join(" "),
            v.subckt,
            if params.is_empty() { "" } else { " params:" },
            params
        ));
    }
    let defs = bbpkg::spice::assemble(&views)?;
    Ok(format!(
        "* bb: loading adc_in (ADR-008 D7, glue rule 9)\n{defs}\n{}\n",
        instances.join("\n")
    ))
}

/// A view parameter value (the package value grammar) as a SPICE number.
fn spice_value(v: &str) -> String {
    match bbpkg::value::parse_number(v) {
        Ok(d) => format!("{}", d.to_f64()),
        Err(_) => v.to_string(),
    }
}

fn run(a: RunArgs) -> Result<(), String> {
    // Parts may come from packages (ADR-006): resolved on the search path,
    // and checked against the board's bb.lock, which bb run never writes.
    // Two front ends, one run plan (ADR-007 D9): a Phase 0 board file, or a
    // project's board.bbm elaborated with its bench and bindings.
    let fe = front_end(&a)?;
    if a.elaborate_only {
        let rj = fe
            .run_json
            .clone()
            .ok_or("--elaborate-only is for projects (not --netlist)")?;
        fs::create_dir_all(&a.out).map_err(|e| format!("create {}: {e}", a.out.display()))?;
        fs::write(a.out.join("deck.cir"), &fe.board.spice)
            .map_err(|e| format!("write deck.cir: {e}"))?;
        fs::write(
            a.out.join("run.json"),
            bbcore::cjson::Json::Obj(rj).to_canonical(),
        )
        .map_err(|e| format!("write run.json: {e}"))?;
        println!(
            "bb run: elaborated (no engine started): {} probes; deck.cir and run.json in {}",
            fe.probes.len(),
            a.out.display()
        );
        return Ok(());
    }
    let board = fe.board;
    let firmware = fe.firmware;
    let probes = fe.probes;
    let part = board
        .parts
        .iter()
        .find(|p| p.reference == fe.target)
        .ok_or_else(|| format!("--target {}: no such part in the board file", fe.target))?;
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
    let mut rcfg = RenodeConfig::stm32f103(renode, firmware.clone());
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
    // The STM32F072's ADC: Breadboard's model on its overlay (ADR-005 D6:
    // bbAdc, BbStm32Timer TRGO; pushes keyed by code12 and delivered as
    // timed inputs), or Renode 1.17.0's stock platform (ADC =
    // Analog.STM32F0_ADC, instance `adc`; pushes keyed by whole millivolts,
    // ADR-003 Amendments 5 and 10).
    let (adc_name, adc_key) = if a.adc_bb {
        (bb_cpu_renode::BB_ADC_NAME, AdcKey::Code12)
    } else {
        ("adc", AdcKey::Millivolts)
    };
    if part.platform == "stm32f072" {
        rcfg.platform = if a.adc_bb {
            Platform::Stm32f072Bb
        } else {
            Platform::Repl("platforms/cpus/stm32f072.repl".into())
        };
    }
    for p in &mcu_pins {
        if let PinKind::Adc { model, .. } = &p.kind {
            // Breadboard's model converts 12 bits against its 3.3 V
            // reference (the overlay's referenceVoltage, ADR-005 D5.2).
            if part.platform == "stm32f072" && a.adc_bb && (model.vref != 3.3 || model.bits != 12) {
                return Err(format!(
                    "{}.{}: adc_in vref={} bits={}: Breadboard's STM32F072 ADC model needs \
                     vref=3.3 bits=12 (or --adc-model stock)",
                    p.part, p.pin, model.vref, model.bits
                ));
            }
        }
    }
    // Breadboard's nRF52840 (Phase 1 M2, ADR-008 D1/D2): pins P<port>.<nn>,
    // and the end-of-run PIN_CNF audit (D4) of every pin bound to a circuit
    // net. A board file names no drive set, so DRIVE is checked only for
    // open drain against digital_out until package bindings name one.
    let nrf = part.platform == "nrf52840";
    let pin_spec = if nrf { PinSpec::nrf } else { PinSpec::stm32 };
    if nrf {
        rcfg.platform = Platform::Nrf52840Bb;
        rcfg.pin_cnf_audit = match &board.audit {
            // A project (Phase 1 M6): each bound pin with the drive of its
            // parameter set, and the unbound GPIOs on analog-scope nets.
            Some(list) => list
                .iter()
                .map(|(pin, e)| {
                    let expect = match e {
                        board::AuditExpect::Out(d) => PinCnfExpect::DigitalOut {
                            drive: d.as_deref().map(str::parse).transpose()?,
                        },
                        board::AuditExpect::In => PinCnfExpect::DigitalIn,
                        board::AuditExpect::Unbound => PinCnfExpect::Unbound,
                    };
                    Ok(PinCnfAudit {
                        pin: pin.clone(),
                        expect,
                    })
                })
                .collect::<Result<_, String>>()?,
            None => mcu_pins
                .iter()
                .filter_map(|p| {
                    let expect = match p.kind {
                        PinKind::Out { .. } => PinCnfExpect::DigitalOut { drive: None },
                        PinKind::In { .. } => PinCnfExpect::DigitalIn,
                        PinKind::OutUnconnected | PinKind::Adc { .. } => return None,
                    };
                    Some(PinCnfAudit {
                        pin: p.pin.clone(),
                        expect,
                    })
                })
                .collect(),
        };
    }
    // The nRF52840's SAADC (ADR-008 D7): pushes are raw uV timed inputs.
    let adc_name = if nrf {
        bb_cpu_renode::BB_SAADC_NAME
    } else {
        adc_name
    };
    rcfg.pins = mcu_pins
        .iter()
        .map(|p| match p.kind {
            PinKind::In { .. } => pin_spec(&p.pin, PinDirection::Input),
            PinKind::Out { .. } | PinKind::OutUnconnected => pin_spec(&p.pin, PinDirection::Output),
            PinKind::Adc { channel, .. } => Ok(PinSpec::adc(&p.pin, adc_name, channel)),
        })
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    // ADR-008 D7, the loading adc_in: per loaded pin, the CPU engine's
    // `<pin>#smp` toggle (one edge per acquisition) drives a generated glue
    // net through a rule-1 EXTERNAL source; the package's pin_load view sits
    // on the pad, and the pushed value is the view's observe node (V(hold)).
    let loads = load_glue(&board, &mcu_pins)?;
    for l in &loads {
        rcfg.pins
            .push(PinSpec::adc_sample(&l.cpu_pin, adc_name, l.channel));
    }
    rcfg.adc_trace = mcu_pins
        .iter()
        .filter(|p| matches!(p.kind, PinKind::Adc { trace: true, .. }))
        .map(|p| p.pin.clone())
        .collect();
    // ADR-008 D8: the firmware's UARTE lines become log records.
    rcfg.uart_log = nrf && a.uart_log;
    // ADR-003 D5: the binding graph has a feedback loop when the MCU both
    // drives the circuit and reads it (U1 -> ngspice and ngspice -> U1; one
    // MCU and one analog engine in Phase 1). Such boards load the lookahead
    // plugin, with --lookahead off as well, so both modes run the same
    // platform. Checked against the kernel's plan after the run. A loading
    // adc_in is both (its #smp toggle drives the circuit).
    let cyclic = mcu_pins
        .iter()
        .any(|p| matches!(p.kind, PinKind::In { .. } | PinKind::Adc { .. }))
        && (!loads.is_empty()
            || mcu_pins
                .iter()
                .any(|p| matches!(p.kind, PinKind::Out { .. })));
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
    let mut netlist = board.spice.clone();
    if !loads.is_empty() {
        // The board body up to its `.end`, then the loads (glue rule 9).
        let mut body: Vec<&str> = Vec::new();
        for l in board.spice.lines() {
            if l.trim().eq_ignore_ascii_case(".end") {
                break;
            }
            body.push(l);
        }
        netlist = body.join("\n");
        netlist.push('\n');
        netlist.push_str(&load_deck(&board, &loads)?);
    }
    let mut ncfg = NgspiceConfig::new(netlist, a.duration, a.renode_quantum);
    ncfg.lattice = Some(a.renode_quantum);
    for l in &loads {
        ncfg.inputs.push(InputBinding {
            pin: l.kernel_net.clone(),
            source: format!("V_bb_{}", spice_name(&l.cpu_pin_full)),
            drive: Drive::Digital {
                model: SMP_DRIVE,
                initial: Level::Low,
            },
            glue_node: Some(l.smp_node.clone()),
        });
    }
    for p in &mcu_pins {
        match &p.kind {
            // An MCU output into the circuit: EXTERNAL source + zout glue.
            PinKind::Out { net, model } => ncfg.inputs.push(InputBinding {
                pin: p.kernel_net(),
                // nRF pin names carry a '.' (P0.13): V_bb_U1_P0_13.
                source: format!("V_bb_{}_{}", p.part, p.pin.replace('.', "_")),
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
            // A loading adc_in pushes V(hold), the view's observe node
            // (ADR-008 D7); the nRF52840's key is raw uV.
            PinKind::Adc { net, model, .. } => ncfg.adc_outputs.push(AdcOutputBinding {
                pin: p.kernel_net(),
                node: loads
                    .iter()
                    .find(|l| l.pin == p.pin)
                    .map_or_else(|| net.clone(), |l| l.hold_node.clone()),
                model: AdcIn {
                    key: if nrf { model.key } else { adc_key },
                    ..*model
                },
            }),
            PinKind::OutUnconnected => {}
        }
    }
    ncfg.adc_push = a.adc_push;
    // An analog probe: its name in the outputs (a Phase 0 node name, or a
    // project net's display name) and its ngspice node (ADR-007 D6).
    let analog_probes: Vec<(&String, String)> = probes
        .iter()
        .filter(|p| !digital_nets.contains(&p.name) && !adc_nets.contains(&p.name))
        .map(|p| (&p.name, p.node.clone().unwrap_or_else(|| p.name.clone())))
        .collect();
    for (n, node) in &analog_probes {
        ncfg.outputs.push(OutputBinding {
            pin: (*n).clone(),
            node: node.clone(),
        });
    }

    fs::create_dir_all(&a.out).map_err(|e| format!("create {}: {e}", a.out.display()))?;
    if fe.run_json.is_some() {
        // A project's elaborated deck, for review (its sha256 is in run.json).
        fs::write(a.out.join("deck.cir"), &board.spice)
            .map_err(|e| format!("write deck.cir: {e}"))?;
    }
    let wall = Instant::now();
    let cpu = RenodeEngine::start(&rcfg).map_err(|e| format!("start Renode: {e}"))?;
    let pin_cnf = cpu.pin_cnf_findings();
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
    for l in &loads {
        bind(
            &mut sched,
            cpu_id,
            &l.cpu_pin,
            &l.kernel_net,
            Direction::Output,
        )?;
        bind(
            &mut sched,
            an_id,
            &l.kernel_net,
            &l.kernel_net,
            Direction::Input,
        )?;
    }
    for (n, _) in &analog_probes {
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
        probes.iter().map(|p| p.name.clone()).collect(),
    )
    .map_err(|e| e.to_string())?;
    let mut vcd_vars: Vec<(&str, VcdKind)> = Vec::new();
    let mut vcd_refs: Vec<String> = Vec::new();
    for p in &probes {
        let kind = if digital_nets.contains(&p.name) {
            VcdKind::Wire
        } else {
            VcdKind::Real
        };
        vcd_vars.push((p.name.as_str(), kind));
        // A project's analog nets are referenced by their SPICE names
        // (spec/event-stream.md); Phase 0 names are their own.
        vcd_refs.push(p.node.clone().unwrap_or_else(|| p.name.clone()));
    }
    for n in digital_nets
        .iter()
        .chain(loads.iter().map(|l| &l.kernel_net))
    {
        if !probes.iter().any(|p| &p.name == n) {
            vcd_vars.push((n.as_str(), VcdKind::Wire));
            vcd_refs.push(n.clone());
        }
    }
    let mut vcd = VcdWriter::sampled(create(&a.out.join("nets.vcd"))?, &vcd_vars, a.sample_period)
        .and_then(|v| v.with_references(&vcd_refs))
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
    // ADR-008 D4: the end-of-run PIN_CNF audit (nRF52840), read after the
    // last grant; each disagreement is a `log` record at t_end.
    let findings = pin_cnf
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .unwrap_or_default();
    for f in &findings {
        eprintln!("bb run: {}: {:?}: {}", part.reference, f.level, f.msg);
        sink.emit(&bbcore::Event::Log {
            t_fs: summary.now,
            source: part.reference.clone(),
            level: f.level,
            msg: f.msg.clone(),
        })
        .map_err(|e| format!("write events: {e}"))?;
    }
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

    if let Some(mut rj) = fe.run_json {
        rj.push((
            "options".into(),
            bbcore::cjson::Json::obj([
                (
                    "for_fs",
                    bbcore::cjson::Json::Int(a.duration.as_fs() as i128),
                ),
                (
                    "max_quantum_fs",
                    bbcore::cjson::Json::Int(a.max_quantum.as_fs() as i128),
                ),
                (
                    "renode_quantum_fs",
                    bbcore::cjson::Json::Int(a.renode_quantum.as_fs() as i128),
                ),
                (
                    "sample_period_fs",
                    bbcore::cjson::Json::Int(a.sample_period.as_fs() as i128),
                ),
                ("seed", bbcore::cjson::Json::Int(a.seed as i128)),
                ("fidelity", bbcore::cjson::Json::str(&a.fidelity)),
                ("lookahead", bbcore::cjson::Json::Bool(lookahead)),
                ("lookahead_plugin", bbcore::cjson::Json::Bool(plugin)),
                (
                    "adc_push",
                    bbcore::cjson::Json::str(match a.adc_push {
                        AdcPush::OnChange => "change",
                        AdcPush::EveryLattice => "every",
                    }),
                ),
                (
                    "adc_model",
                    bbcore::cjson::Json::str(if a.adc_bb { "bb" } else { "stock" }),
                ),
                (
                    "verify",
                    bbcore::cjson::Json::str(format!("{:?}", a.verify).to_lowercase()),
                ),
                (
                    "engine_order",
                    bbcore::cjson::Json::str(match a.engine_order {
                        EngineOrder::McuFirst => "mcu-first",
                        EngineOrder::AnalogFirst => "analog-first",
                    }),
                ),
                (
                    "allow_unqualified",
                    bbcore::cjson::Json::Arr(
                        a.allow_unqualified
                            .iter()
                            .map(bbcore::cjson::Json::str)
                            .collect(),
                    ),
                ),
                (
                    "probes",
                    bbcore::cjson::Json::Arr(
                        probes
                            .iter()
                            .map(|p| bbcore::cjson::Json::str(&p.name))
                            .collect(),
                    ),
                ),
            ]),
        ));
        rj.push((
            "versions".into(),
            bbcore::cjson::Json::obj([
                ("bb", bbcore::cjson::Json::str(env!("CARGO_PKG_VERSION"))),
                (
                    "renode",
                    bbcore::cjson::Json::str(platforms::RENODE_VERSION),
                ),
            ]),
        ));
        let text = bbcore::cjson::Json::Obj(rj).to_canonical();
        fs::write(a.out.join("run.json"), text).map_err(|e| format!("write run.json: {e}"))?;
    }
    let mut so = std::io::stdout().lock();
    for f in &fe.flagged {
        let _ = writeln!(
            so,
            "bb run: note: view {f}: not checked against a datasheet or a measurement (see run.json)"
        );
    }
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

/// What one run executes, from either front end (ADR-007 D9).
struct FrontEnd {
    board: board::Board,
    target: String,
    firmware: PathBuf,
    probes: Vec<elab::ProbeSpec>,
    /// A project's run.json content (none for a Phase 0 board file).
    run_json: Option<Vec<(String, bbcore::cjson::Json)>>,
    flagged: Vec<String>,
}

fn front_end(a: &RunArgs) -> Result<FrontEnd, String> {
    if let Some(netlist) = &a.netlist {
        // Parts may come from packages (ADR-006): resolved on the search
        // path, and checked against the board's bb.lock, which bb run never
        // writes.
        let board = open::parse_board(netlist, true)?;
        for w in &board.warnings {
            eprintln!("bb run: warning: {}: {w}", netlist.display());
        }
        open::check_lock(netlist, &board)?;
        let firmware = a.firmware.clone().expect("checked");
        if !firmware.is_file() {
            return Err(format!("firmware {} not found", firmware.display()));
        }
        return Ok(FrontEnd {
            board,
            target: a.target.clone().expect("checked"),
            firmware,
            probes: a
                .probes
                .iter()
                .map(|n| elab::ProbeSpec {
                    name: n.clone(),
                    node: None,
                })
                .collect(),
            run_json: None,
            flagged: Vec::new(),
        });
    }
    // A project: read-only (ADR-007 D7). board.bbm and bb.lock must be what
    // `bb open` would write now.
    let dir = import::project_dir(a.project.as_deref())?;
    let p = project::Project::read(&dir)?;
    let imp = import::import(&p, import::Mode::Run)?;
    for w in &imp.warnings {
        eprintln!("bb run: warning: {w}");
    }
    let bbm_path = dir.join(import::BOARD_FILE);
    let on_disk = fs::read(&bbm_path)
        .map_err(|e| format!("read {}: {e}: run `bb open`", bbm_path.display()))?;
    if on_disk != imp.board.to_text().as_bytes() {
        return Err(format!(
            "{} is not current (a source, package or bb.toml changed): run `bb open` (ADR-007 D7)",
            bbm_path.display()
        ));
    }
    let lock = bbpkg::lock::Lock::read(&dir.join("bb.lock"), &bbpkg::schema::Registry::current())?;
    let d = bbpkg::lock::drift(lock.as_ref(), &imp.locked_packages, &imp.locked_platforms);
    if !d.is_empty() {
        return Err(format!(
            "{} does not match the packages and platforms (ADR-006 D8): {}; run `bb open`",
            dir.join("bb.lock").display(),
            d.iter()
                .map(|x| x.describe())
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    let bench = match &a.bench {
        Some(b) => project::Bench::read(b)?,
        None => project::Bench::empty(),
    };
    let plan = elab::plan(&elab::PlanArgs {
        project: &p,
        imp: &imp,
        bench: &bench,
        bench_name: a.bench.as_deref().map(elab::bench_label),
        fidelity: &a.fidelity,
        lattice: a.renode_quantum,
        allow_unqualified: &a.allow_unqualified,
        cli_probes: &a.probes,
    })?;
    for w in &plan.warnings {
        eprintln!("bb run: warning: {w}");
    }
    Ok(FrontEnd {
        board: plan.board,
        target: plan.target,
        firmware: plan.firmware,
        probes: plan.probes,
        run_json: Some(plan.run_json),
        flagged: plan.flagged,
    })
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
        Some("open") => open::main(&args[1..]),
        Some("flash") => flash::main(&args[1..]),
        Some("probe") => probe_cmd::main(&args[1..]),
        Some("pkg") => pkg::main(&args[1..]),
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
        assert_eq!(a.netlist.as_deref(), Some(std::path::Path::new("b.cir")));
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
