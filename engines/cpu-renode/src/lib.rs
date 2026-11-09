//! CPU engine backed by a headless Renode process driven through its
//! External Control API.
//!
//! # How time is controlled
//!
//! Renode is started paused (`--disable-xwt --port -1`, no monitor, no GUI)
//! with a generated script that creates the machine, loads the platform and
//! the ELF, and opens an External Control server
//! (`emulation CreateExternalControlServer`). The engine then only ever sends
//! `RUN_FOR(n ns)` calls that end on the grant lattice: Renode's global
//! quantum [`RenodeConfig::quantum`] (`L`, ADR-003 D2), which the engine
//! declares through [`bbcore::Engine::grant_lattice`]. `RUN_FOR` returns once
//! Renode has paused again, so Renode never runs between grants and its
//! virtual time never passes the grant. [`RenodeConfig::verify`] cross-checks
//! Renode's virtual time (`GET_TIME`) and the output pin states
//! (`GPIO GET_STATE`) against what the engine tracked, and a mismatch is a
//! protocol error. The checks are pure reads, so they never change results;
//! [`Verify`] picks how often they run (ADR-003 D1.2).
//!
//! The emulation always runs with `emulation SetGlobalAdvanceImmediately
//! true` (ADR-003 D1.1). Without it Renode sleeps on the host while the CPU
//! is in WFI and then skips instructions by the host's measured sleep time
//! (`BaseCPU.cs`), which is a host-timing dependence. It is not optional.
//!
//! Known bad ideas: never let Renode's virtual time run ahead of the kernel
//! grant; never `start` the emulation and try to catch up; never end a
//! `RUN_FOR` off the lattice.
//!
//! # Why the lattice (ADR-003 D2)
//!
//! Renode's results depend on its sequence of sync points: one every global
//! quantum, plus wherever a timer limit stops a sleeping CPU. A `RUN_FOR` that
//! ends between two of them shifts every later one. When every `RUN_FOR` ends
//! on a multiple of the quantum, the sync points are the same whatever the
//! grant partition, so the results depend on `L` alone. `L` is run
//! configuration (`bb run --renode-quantum`), like the seed: it is the
//! firmware-timing, edge-stamp and input resolution.
//!
//! # Edges
//!
//! Each output pin has a GPIO state-change callback registered over the
//! External Control API. Renode stamps every change with
//! `MasterTimeSource.ElapsedVirtualTime` (`GPIOPort.cs`), the sync point that
//! opens the slice in which the write executed: the true instruction time
//! lies in `[stamp, next sync point)`, so stamps are slice accurate. The
//! engine reports the stamp `s` (ns, converted to fs) by the lattice +1 fs
//! rule, which depends only on `s` and `L`, never on where grants end:
//!
//! 1. `s` a multiple of `L`: the write happened after the re-sync at `s`, and
//!    the engine reports `s + 1 fs`.
//! 2. otherwise `s` is a limit-driven sync point (a timer expiry that stopped
//!    a sleeping CPU) and the engine reports `s` exactly.
//! 3. a change stamped exactly at the grant end `g` (a lattice point) belongs
//!    to the next grant by rule 1: the engine carries it over, reports the
//!    old level at `g` and the change at `g + 1 fs` in the next grant
//!    (counted in [`RenodeEngine::carried_edges`]).
//! 4. so a write that genuinely happens on a lattice point is reported 1 fs
//!    late (an accepted convention).
//!
//! Every change of a grant is reported as an ordered trace
//! ([`bbcore::Engine::read_output_trace`], ADR-003 D3), with any number of
//! edges per pin, so the engine's `t_safe` is [`Time::MAX`] ("I report
//! everything", D4): the grant length is set by the kernel (`max_quantum`,
//! the loop cap), never by the firmware. Two changes of one pin stamped in
//! the same slice would get the same time and are a protocol error (lower
//! `--renode-quantum`); the same holds in any partition. Stamps before the
//! previous boundary or after the grant are protocol errors.
//!
//! # Inputs
//!
//! Renode can observe an external input only at a sync point. An input
//! transition at time `t` therefore takes effect at `ceil_L(t)`, the first
//! lattice point at or after `t`, and never earlier. Transitions delivered
//! before an advance (`edge_time` inside the coming grant) are queued; the
//! advance is split into `RUN_FOR` segments that end at each distinct
//! `ceil_L(t)`, and `GPIO SET_STATE` is sent, in delivery order, while Renode
//! is paused there. Every segment ends on the lattice at or before the grant,
//! so this is not a free-run. A transition delivered at a boundary
//! (`edge_time <= now`) takes effect at once, at `now`.
//!
//! ADC inputs ([`PinDirection::Adc`], ADR-003 D6) on Renode's stock ADC
//! model work the same way: the producer pushes `Analog` values at lattice
//! points, and the engine sets each one with the External Control `ADC`
//! command (`SetValue`, whole microvolts, on the channel's default sample)
//! while Renode is paused on that lattice point.
//! [`bbcore::Engine::apply_input_trace`] takes each entry's time from the
//! trace, because an `Analog` value carries none.
//!
//! On Breadboard's STM32F072 ([`Platform::Stm32f072Bb`], ADR-005) the ADC
//! model reads its inputs only at its own sample instants, which are clock
//! limits. There the pushes are **timed inputs**: the engine collects them
//! and, before its next `RUN_FOR`, writes them in one batch (one
//! `SYSTEM_BUS` write) to the model's host port ([`BB_ADC_PORT_ADDRESS`]);
//! a sample at `s` reads the latest entry with `t < s`. No `RUN_FOR` is
//! split for them, and the pin is a sampled input
//! ([`bbcore::Engine::input_is_sampled`], ADR-005 D3), so a quiet grant does
//! not stop at a push. The cross-check reads the port's per-channel value
//! and its error word (a timed input that came too late for a sample).
//!
//! # Determinism
//!
//! Renode is deterministic for a single machine given the same ELF, platform,
//! quantum and lattice-aligned `RUN_FOR` calls. The engine fixes everything
//! it can: `emulation SetSeed`, `emulation SetGlobalQuantum` (= `quantum`),
//! `emulation SetGlobalSerialExecution true`,
//! `emulation SetGlobalAdvanceImmediately true`, an offline platform file (no
//! SVD download) and a fixed machine name. GPIO events are acknowledged
//! inline on a single connection, so they are seen in emulation order.
//!
//! # Lookahead (ADR-003 D5)
//!
//! With [`LookaheadCfg::plugin`] the engine loads `platforms/bb_lookahead.cs`,
//! a read-only register block mapped at [`LookaheadCfg::address`], and arms
//! it from the generated script. One `SYSTEM_BUS` read of 8 dwords (a Query)
//! returns what Renode's own WFI fast-forward uses: whether the CPU is in
//! WFI, whether the NVIC has a pending exception or a synced-state action is
//! queued, whether the CPU is halted, the clock source's `NearestLimitIn` and
//! its time. While the CPU sleeps with nothing pending, nothing executes
//! before that limit, so [`bbcore::Engine::quiet_until`] returns
//! `clock + NearestLimitIn` ([`LookaheadCfg::enabled`]); otherwise it returns
//! `now` (no promise). The block is inert: its reads are pure, the WFI hook
//! it adds changes no tlib setting (the NVIC registers one of its own), and
//! firmware accesses to it are ignored and logged.
//!
//! The advance that follows a promise is audited: an output change before
//! the promise, or a WFI exit before it (the plugin latches the time of the
//! first exit since the previous Query), is [`EngineError::PromiseBroken`].
//! A startup self-test (one machine, one `TranslationCPU`, one NVIC, both
//! reflected fields present, no host backends) that fails leaves the engine
//! without promises: the board falls back to the loop cap.
//!
//! Renode 1.17.0's monitor `include` only compiles C# sources (about 1.6 s
//! per run). The engine therefore compiles the block once with Renode's own
//! compiler (`AdHocCompiler`, so the assembly matches the installed Renode),
//! caches the assembly ([`LookaheadCfg::cache_dir`], keyed by a hash of the
//! source and the Renode install) and loads it with `TypeManager.ScanFile`
//! from a generated Python snippet in later runs (ADR-003 Amendment 8).

//!
//! # Startup (ADR-004)
//!
//! A fresh Renode is started with [`renode_command`]: `DOTNET_TieredPGO=0`
//! (Part A) and kill-on-parent-death, plus the cached .NET multi-core JIT
//! startup profile for its Renode and platform (Amendment 6; the first run
//! for a key gathers it). With [`RenodeConfig::pool`] the
//! engine first asks a warm Renode pool for a process ([`pool`], Part B);
//! the rest of the engine does not depend on where its Renode came from.

pub mod console;
pub mod manager;
pub mod pool;
pub mod protocol;

use std::collections::BTreeSet;
use std::io::Read;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use bbcore::time::FS_PER_NS;
use bbcore::{Engine, EngineError, Level, PinSignal, PinSignals, Time, Transition};

use protocol::{Client, ProtocolError};

/// Name this engine reports through [`bbcore::Engine::name`] by default.
pub const ENGINE_NAME: &str = "renode";

/// Platform file written next to the generated script by default. See the
/// file itself for why the stock `platforms/cpus/stm32f103.repl` is wrapped.
pub const STM32F103_OFFLINE_REPL: &str = include_str!("../platforms/stm32f103-offline.repl");

/// Renode's virtual-time resolution.
pub const RENODE_RESOLUTION: Time = Time::from_ns(1);

/// The largest quiet promise, in ns, that [`bbcore::Engine::quiet_until`]
/// returns: the last whole nanosecond [`Time`] can hold (about 18 446 s).
/// A promise further away (an nRF TIMER past its compare never wraps; an
/// RTC with a large prescaler) is clamped to it instead of overflowing in
/// `Time::from_ns` (ADR-008 D6). A shorter promise is always honest, and
/// the kernel caps every grant at `--max-quantum` anyway, so the clamp
/// changes no grant.
pub const MAX_PROMISE_NS: u64 = Time::MAX.as_fs() / FS_PER_NS;

/// A promise of `p_ns` (Renode virtual time) as a [`Time`], clamped to
/// [`MAX_PROMISE_NS`].
fn promise_time(p_ns: u64) -> Time {
    Time::from_ns(p_ns.min(MAX_PROMISE_NS))
}

/// Source of the lookahead register block (ADR-003 D5), mapped when
/// [`LookaheadCfg::plugin`] is set.
pub const BB_LOOKAHEAD_CS: &str = include_str!("../platforms/bb_lookahead.cs");

/// Source of `Timers.BbStm32Timer`, STM32_Timer plus TRGO (ADR-005 D1.2).
pub const BB_STM32_TIMER_CS: &str = include_str!("../platforms/bb_stm32_timer.cs");

/// Source of `Analog.BbStm32F0Adc` and `Miscellaneous.BbAdcHostPort`, the
/// STM32F0 ADC model and its host port (ADR-005 D1-D5).
pub const BB_STM32F0_ADC_CS: &str = include_str!("../platforms/bb_stm32f0_adc.cs");

/// Breadboard's STM32F072 overlay (ADR-005 D6), written next to the
/// generated script for [`Platform::Stm32f072Bb`].
pub const STM32F072_BB_REPL: &str = include_str!("../platforms/stm32f072-bb.repl");

/// Monitor name of the ADC on [`Platform::Stm32f072Bb`].
pub const BB_ADC_NAME: &str = "bbAdc";

/// Bus address of the ADC model's host port on [`Platform::Stm32f072Bb`]
/// (`bbAdcPort`, ADR-005 D5; next to the lookahead block).
pub const BB_ADC_PORT_ADDRESS: u64 = 0x5FFF_0100;

/// Offset of the host port's batch window (header, then `(t_ns low, t_ns
/// high | channel << 16, uV)` per entry).
const PORT_BATCH: u64 = 0x1000;
/// Words per `SYSTEM_BUS` write into the batch window (its size is 0x1000).
const PORT_BATCH_WORDS: usize = 1020;

/// Every C# source of Breadboard's Renode plugins, compiled together into
/// one cached assembly (ADR-005 D6): `(file name, source)`.
pub const PLUGIN_SOURCES: &[(&str, &str)] = &[
    ("bb_lookahead.cs", BB_LOOKAHEAD_CS),
    ("bb_stm32_timer.cs", BB_STM32_TIMER_CS),
    ("bb_stm32f0_adc.cs", BB_STM32F0_ADC_CS),
];

/// Default address of the lookahead block: unused on the STM32F103.
pub const LOOKAHEAD_ADDRESS: u64 = 0x5FFF_0000;

/// Monitor name of the lookahead block on `sysbus`.
pub(crate) const LOOKAHEAD_NAME: &str = "bbLookahead";

/// CPU lookahead configuration (ADR-003 D5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LookaheadCfg {
    /// Load and arm the lookahead block. `bb run` sets it for boards whose
    /// binding graph has a feedback loop, with `--lookahead off` as well as
    /// `on`, so reference runs load the same platform.
    pub plugin: bool,
    /// Answer [`bbcore::Engine::quiet_until`] with real promises (`bb run
    /// --lookahead on`). Requires `plugin`. Off: `quiet_until` returns `now`
    /// and the block is never read after startup.
    pub enabled: bool,
    /// Bus address of the block (8 dwords); must be unused on the platform.
    pub address: u64,
    /// Where the compiled block is cached. `None`: `$BB_CACHE_DIR`, else
    /// `$XDG_CACHE_HOME/bb`, else `~/.cache/bb`, each with
    /// `renode-plugins/` appended.
    pub cache_dir: Option<PathBuf>,
}

impl Default for LookaheadCfg {
    fn default() -> LookaheadCfg {
        LookaheadCfg {
            plugin: false,
            enabled: false,
            address: LOOKAHEAD_ADDRESS,
            cache_dir: None,
        }
    }
}

/// One Query of the lookahead block: a snapshot of the register block (see
/// `platforms/bb_lookahead.cs`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct LookaheadStatus {
    /// Renode's time (ns, as tracked by the engine) when the snapshot was
    /// taken.
    pub at_ns: u64,
    /// The STATUS register.
    pub status: u32,
    /// `NearestLimitIn` in ns, if a limit exists.
    pub limit_ns: Option<u64>,
    /// Clock-source time in ns.
    pub clock_ns: u64,
    /// WFI exits since arming (wraps).
    pub wfi_exits: u32,
    /// Clock time of the first WFI exit since the previous snapshot.
    pub first_exit_ns: Option<u64>,
}

impl LookaheadStatus {
    pub const ARMED: u32 = 1 << 0;
    pub const SELF_TEST_OK: u32 = 1 << 1;
    pub const IN_WFI: u32 = 1 << 2;
    pub const NVIC_PENDING: u32 = 1 << 3;
    pub const ACTION_QUEUED: u32 = 1 << 4;
    pub const HALTED: u32 = 1 << 5;
    pub const HAS_LIMIT: u32 = 1 << 6;
    pub const FIRMWARE_ACCESS: u32 = 1 << 7;
    pub const EXITED: u32 = 1 << 8;

    fn from_words(at_ns: u64, w: &[u32]) -> Result<LookaheadStatus, EngineError> {
        if w.len() != 8 {
            return Err(EngineError::Protocol(format!(
                "lookahead block: read {} dwords, expected 8",
                w.len()
            )));
        }
        let u64_at = |i: usize| u64::from(w[i]) | (u64::from(w[i + 1]) << 32);
        let opt = |v: u64| (v != u64::MAX).then_some(v);
        Ok(LookaheadStatus {
            at_ns,
            status: w[0],
            limit_ns: opt(u64_at(1)).filter(|_| w[0] & Self::HAS_LIMIT != 0),
            clock_ns: u64_at(3),
            wfi_exits: w[5],
            first_exit_ns: opt(u64_at(6)),
        })
    }

    /// The block was armed by the generated script.
    pub fn armed(&self) -> bool {
        self.status & Self::ARMED != 0
    }

    /// The startup self-test passed.
    pub fn self_test_ok(&self) -> bool {
        self.status & Self::SELF_TEST_OK != 0
    }

    /// Self-test failure reasons (bits 16..23 of STATUS, shifted down; see
    /// `bb_lookahead.cs`), or 0.
    pub fn self_test_failures(&self) -> u32 {
        (self.status >> 16) & 0xFF
    }

    /// The quiet promise in ns: `clock + NearestLimitIn` when the CPU is in
    /// WFI with nothing pending or queued, not halted, a limit exists and
    /// the clock source agrees with Renode's time. Otherwise `None`.
    pub fn promise_ns(&self) -> Option<u64> {
        let need = Self::ARMED | Self::SELF_TEST_OK | Self::IN_WFI | Self::HAS_LIMIT;
        let veto = Self::NVIC_PENDING | Self::ACTION_QUEUED | Self::HALTED;
        if self.status & need != need || self.status & veto != 0 || self.clock_ns != self.at_ns {
            return None;
        }
        self.clock_ns.checked_add(self.limit_ns?)
    }
}

/// Counters of the lookahead machinery (wall-clock-free; for tests and the
/// run summary).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct LookaheadStats {
    /// `quiet_until` calls answered from a Query (fresh or cached).
    pub queries: u64,
    /// SYSTEM_BUS reads of the block (Queries and audits).
    pub reads: u64,
    /// Promises given (`quiet_until > now`).
    pub promises: u64,
    /// Advances audited against a promise.
    pub audits: u64,
}

/// Which platform description to load.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Platform {
    /// [`STM32F103_OFFLINE_REPL`]: stock STM32F103 without the SVD download.
    Stm32f103Offline,
    /// [`STM32F072_BB_REPL`]: Renode's stock STM32F072 with Breadboard's
    /// timers (TRGO) and ADC model (ADR-005). Loads the plugin assembly
    /// before the platform, whatever [`LookaheadCfg::plugin`] says.
    Stm32f072Bb,
    /// A path understood by Renode's `LoadPlatformDescription`, e.g. the stock
    /// `platforms/cpus/stm32f103.repl` (relative to the Renode install) or an
    /// absolute path.
    Repl(String),
}

impl Platform {
    /// The platform names types from Breadboard's plugin assembly, which
    /// must then be loaded before `LoadPlatformDescription`.
    pub fn needs_plugins(&self) -> bool {
        matches!(self, Platform::Stm32f072Bb)
    }
}

/// Direction of an engine pin, seen from the MCU.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PinDirection {
    /// The MCU drives the pin; edges are reported to the kernel.
    Output,
    /// The kernel drives the pin into the MCU (`SET_STATE`).
    Input,
    /// An ADC input (ADR-003 D6): the kernel pushes `Analog` values, which
    /// the engine applies with the External Control `ADC` command
    /// (`SetValue`, whole microvolts) at their lattice points. For such a
    /// pin [`PinSpec::gpio`] names the ADC peripheral (e.g. `adc`) and
    /// [`PinSpec::index`] is the channel.
    Adc,
}

/// One MCU pin exposed to the kernel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinSpec {
    /// Pin name on the engine side, e.g. `PA5`.
    pub name: String,
    /// Renode GPIO controller name, e.g. `gpioPortA` (an ADC input: the ADC
    /// peripheral's name).
    pub gpio: String,
    /// Pin index within the controller (an ADC input: the channel).
    pub index: i32,
    pub direction: PinDirection,
}

impl PinSpec {
    /// STM32 style pin: `stm32("PA5", Output)` -> `gpioPortA` pin 5.
    pub fn stm32(name: &str, direction: PinDirection) -> Result<PinSpec, EngineError> {
        let bad = || EngineError::Other(format!("not an STM32 pin name: {name:?}"));
        let rest = name.strip_prefix('P').ok_or_else(bad)?;
        let mut chars = rest.chars();
        let port = chars
            .next()
            .filter(|c| c.is_ascii_uppercase())
            .ok_or_else(bad)?;
        let index: i32 = chars.as_str().parse().map_err(|_| bad())?;
        if !(0..16).contains(&index) {
            return Err(bad());
        }
        Ok(PinSpec {
            name: name.to_string(),
            gpio: format!("gpioPort{port}"),
            index,
            direction,
        })
    }

    /// An ADC input: pin `name` (e.g. `PA1`) read by channel `channel` of
    /// the ADC peripheral `adc` (e.g. `adc` on `stm32f072.repl`).
    pub fn adc(name: &str, adc: &str, channel: i32) -> PinSpec {
        PinSpec {
            name: name.to_string(),
            gpio: adc.to_string(),
            index: channel,
            direction: PinDirection::Adc,
        }
    }
}

/// How often the engine cross-checks Renode's virtual time and output pin
/// states against its own tracking (ADR-003 D1.2). The checks are pure reads
/// (`GET_TIME`, `GPIO GET_STATE`): every mode gives byte-identical outputs,
/// and they differ only in cost and in how soon a mismatch is reported.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Verify {
    /// After every advance (the Phase 0 behaviour; one CI job runs it).
    Every,
    /// After an advance whose grant carried a GPIO event or an applied
    /// input, and at the end of the run ([`bbcore::Engine::end_of_run`]).
    #[default]
    Edges,
    /// Only at the end of the run.
    Final,
}

impl std::str::FromStr for Verify {
    type Err = String;
    fn from_str(s: &str) -> Result<Verify, String> {
        match s {
            "every" => Ok(Verify::Every),
            "edges" => Ok(Verify::Edges),
            "final" => Ok(Verify::Final),
            _ => Err(format!("{s:?}: expected every, edges or final")),
        }
    }
}

/// A deliberately injected engine fault, for testing that [`Verify`]
/// catches a desynchronised engine. Never used outside tests.
#[doc(hidden)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Fault {
    /// The first `RUN_FOR` of an advance to a grant `>= at` runs `ns`
    /// nanoseconds longer than the engine tracks (a time mismatch).
    RunForExtraNs { at: Time, ns: u64 },
    /// The first GPIO event stamped at or after `at` ns is discarded, so the
    /// tracked pin level goes stale (a GPIO mismatch).
    DropGpioEvent { at_ns: u64 },
    /// The next quiet promise is `ns` nanoseconds longer than the block
    /// says (an over-long promise: the wake audit must catch it).
    StretchPromise { ns: u64 },
}

/// Configuration of a [`RenodeEngine`].
#[derive(Clone, Debug)]
pub struct RenodeConfig {
    /// Engine name (unique within a scheduler).
    pub name: String,
    /// The `renode` launcher. See [`find_renode`].
    pub renode: PathBuf,
    /// Firmware ELF to load with `sysbus LoadELF`.
    pub firmware: PathBuf,
    pub platform: Platform,
    /// Renode machine name.
    pub machine: String,
    pub pins: Vec<PinSpec>,
    /// Renode's global quantum, which is the grant lattice `L` (ADR-003 D2,
    /// `bb run --renode-quantum`). Every `RUN_FOR` ends on a multiple of it.
    /// It is the firmware-timing, edge-stamp and input resolution, and part
    /// of the run configuration: changing it changes results. Must be a
    /// positive whole number of nanoseconds.
    pub quantum: Time,
    /// `emulation SetSeed`.
    pub seed: i32,
    /// When to check Renode's virtual time and the output pin states against
    /// what the engine tracked. Costs round trips; never changes results.
    pub verify: Verify,
    /// How long to wait for Renode to start and open its server (wall clock;
    /// never affects results).
    pub startup_timeout: Duration,
    /// Socket I/O timeout for a single request (wall clock).
    pub io_timeout: Duration,
    /// CPU lookahead (ADR-003 D5): the plugin and `quiet_until`.
    pub lookahead: LookaheadCfg,
    /// Extra monitor lines run after the firmware is loaded and before the
    /// lookahead block is armed (tests: e.g. attach a UART terminal to make
    /// the self-test fail). Never set by `bb`.
    #[doc(hidden)]
    pub extra_monitor: Vec<String>,
    /// A warm Renode pool's socket (ADR-004 Part B): lease a pre-warmed
    /// process from it when one matches and is idle, else start Renode
    /// fresh. Never changes results. Default: [`pool::POOL_ENV`].
    pub pool: Option<PathBuf>,
    /// Messages after which the engine replaces its External Control
    /// connection, between two `RUN_FOR`s while Renode is paused (Renode
    /// 1.17.0 crashes after about 524 000 on one connection, see
    /// [`protocol::EC_MESSAGE_LIMIT`]; ADR-004 Amendment 8). 0: never (a
    /// request then fails at the limit). Never changes results. Default:
    /// [`EC_MESSAGES_ENV`], else [`EC_MESSAGES_DEFAULT`].
    #[doc(hidden)]
    pub ec_messages_per_connection: u64,
}

/// The default of [`RenodeConfig::ec_messages_per_connection`]: a quarter
/// of Renode 1.17.0's limit, so one grant's messages never reach it.
pub const EC_MESSAGES_DEFAULT: u64 = 100_000;

/// Environment override of [`RenodeConfig::ec_messages_per_connection`]
/// (gates force frequent reconnects with it). A test knob; never changes
/// results.
#[doc(hidden)]
pub const EC_MESSAGES_ENV: &str = "BB_RENODE_EC_MESSAGES";

fn ec_messages_from_env() -> u64 {
    std::env::var(EC_MESSAGES_ENV)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(EC_MESSAGES_DEFAULT)
}

impl RenodeConfig {
    /// STM32F103 with PA5 as an output and a 10 us quantum (lattice): the
    /// Phase 0 spike setup.
    pub fn stm32f103(renode: PathBuf, firmware: PathBuf) -> RenodeConfig {
        RenodeConfig {
            name: ENGINE_NAME.to_string(),
            renode,
            firmware,
            platform: Platform::Stm32f103Offline,
            machine: "bb".to_string(),
            pins: vec![PinSpec::stm32("PA5", PinDirection::Output).expect("valid pin")],
            quantum: Time::from_us(10),
            seed: 0,
            verify: Verify::Edges,
            startup_timeout: Duration::from_secs(120),
            io_timeout: Duration::from_secs(120),
            lookahead: LookaheadCfg::default(),
            extra_monitor: Vec::new(),
            pool: pool::from_env(),
            ec_messages_per_connection: ec_messages_from_env(),
        }
    }
}

/// Where the compiled plugin assembly for `renode` is cached (ADR-003
/// Amendment 8, ADR-005 D6): `<cache_dir>/bb_plugins-<key>.dll`, the key a
/// hash of every source in [`PLUGIN_SOURCES`] and the canonical Renode
/// launcher.
pub fn plugin_cache_path(renode: &Path, cache_dir: Option<&Path>) -> Result<PathBuf, EngineError> {
    let cache_dir = cache_dir
        .map(Path::to_path_buf)
        .or_else(default_cache_dir)
        .ok_or_else(|| {
            EngineError::Other(
                "no cache directory for the Renode plugins (set BB_CACHE_DIR or HOME)".into(),
            )
        })?;
    let renode_id = pool::canonical_renode(renode);
    let mut parts: Vec<&[u8]> = Vec::new();
    for (name, src) in PLUGIN_SOURCES {
        parts.push(name.as_bytes());
        parts.push(src.as_bytes());
    }
    let id = renode_id.to_string_lossy();
    parts.push(id.as_bytes());
    let key = fnv1a64(&parts);
    Ok(cache_dir.join(format!("bb_plugins-{key:016x}.dll")))
}

/// Name of the plugin loader the engine and the pool write into their work
/// directory.
pub(crate) const PLUGIN_LOADER: &str = "bb_plugins_load.py";

/// Name of the loader's error file in the work directory.
pub(crate) const PLUGIN_ERR: &str = "bb_plugins.err";

/// Write the plugin sources and their loader into `dir`; returns the
/// loader (a Python file the monitor `include`s).
pub fn write_plugin_loader(dir: &Path, cache: &Path, err: &Path) -> Result<PathBuf, EngineError> {
    let mut srcs = Vec::new();
    for (name, src) in PLUGIN_SOURCES {
        let p = dir.join(name);
        std::fs::write(&p, src)?;
        srcs.push(monitor_path(&p)?);
    }
    let loader = dir.join(PLUGIN_LOADER);
    std::fs::write(
        &loader,
        plugin_loader_py(&srcs, &monitor_path(cache)?, &monitor_path(err)?),
    )?;
    Ok(loader)
}

/// Where the engine's Renode came from.
enum Host {
    /// Spawned by this engine; killed on drop.
    Child(Child),
    /// A process leased from a warm pool (ADR-004 Part B); handed back on
    /// drop.
    Lease(pool::Lease),
}

impl Host {
    /// Startup failed: kill the child, or discard the leased process.
    fn abort(self) {
        match self {
            Host::Child(mut c) => {
                let _ = c.kill();
                let _ = c.wait();
            }
            Host::Lease(mut l) => {
                let _ = l.end(false);
            }
        }
    }

    fn pid(&self) -> u32 {
        match self {
            Host::Child(c) => c.id(),
            Host::Lease(l) => l.pid(),
        }
    }

    /// A note for error messages: the child's exit status, or the pool.
    fn status_note(&mut self) -> String {
        match self {
            Host::Child(c) => match c.try_wait() {
                Ok(Some(status)) => format!("renode exited ({status}); "),
                _ => String::new(),
            },
            Host::Lease(l) => format!("pooled renode pid {}; ", l.pid()),
        }
    }
}

/// Default plugin cache directory (see [`LookaheadCfg::cache_dir`]).
fn default_cache_dir() -> Option<PathBuf> {
    let base = std::env::var_os("BB_CACHE_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CACHE_HOME").map(|d| PathBuf::from(d).join("bb")))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache/bb")))?;
    Some(base.join("renode-plugins"))
}

/// 64-bit FNV-1a, for the plugin cache key (not a security boundary: the
/// cache holds what this engine compiled with the same Renode).
fn fnv1a64(parts: &[&[u8]]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in parts {
        for b in p.iter().chain(std::iter::once(&0xFFu8)) {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

/// Python run from the generated script: load the cached plugin assembly,
/// or compile `srcs` (together, into one assembly) with Renode's compiler,
/// load it and cache it. Any failure writes `err` and exits Renode (a
/// failed monitor command would leave it waiting without a server until the
/// startup timeout).
fn plugin_loader_py(srcs: &[String], cache: &str, err: &str) -> String {
    let list = srcs
        .iter()
        .map(|s| format!("r\"{s}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"# Generated by bb-cpu-renode: load Breadboard's Renode plugins (ADR-003 D5, ADR-005 D6).
import System
from Antmicro.Renode.Utilities import TypeManager, AdHocCompiler
BB_SRCS = [{list}]
BB_CACHE = r"{cache}"
BB_ERR = r"{err}"
def bb_fail(msg):
    f = open(BB_ERR, "w")
    f.write(msg)
    f.close()
    System.Environment.Exit(3)
def bb_load_cached():
    try:
        return System.IO.File.Exists(BB_CACHE) and TypeManager.Instance.ScanFile(BB_CACHE)
    except Exception as e:
        return False
try:
    if not bb_load_cached():
        dll = AdHocCompiler().Compile(System.Array[System.String](BB_SRCS))
        System.Reflection.Assembly.LoadFrom(dll)
        if not TypeManager.Instance.ScanFile(dll):
            bb_fail("TypeManager.ScanFile(" + dll + ") failed")
        try:
            System.IO.Directory.CreateDirectory(System.IO.Path.GetDirectoryName(BB_CACHE))
            tmp = BB_CACHE + "." + str(System.Environment.ProcessId) + ".tmp"
            System.IO.File.Copy(dll, tmp, True)
            System.IO.File.Move(tmp, BB_CACHE, True)
        except Exception as e:
            f = open(BB_ERR + ".cache", "w")
            f.write("caching the compiled plugins failed: " + str(e))
            f.close()
except Exception as e:
    bb_fail("loading the Breadboard plugins failed: " + str(e))
"#
    )
}

/// Environment switch for the ADR-004 G1 baseline: with
/// `BB_RENODE_STOCK_RUNTIME=1` the engine starts Renode with the stock .NET
/// runtime settings, no startup JIT profile (Amendment 6) and the 100 ms
/// connect poll it used before ADR-004 Part A. A measurement knob; it never
/// changes results.
#[doc(hidden)]
pub const STOCK_RUNTIME_ENV: &str = "BB_RENODE_STOCK_RUNTIME";

fn stock_runtime() -> bool {
    std::env::var(STOCK_RUNTIME_ENV).is_ok_and(|v| v == "1")
}

/// How long the engine sleeps between External Control connect attempts
/// (ADR-004 A2: 5 ms; the stock baseline: 100 ms). Wall clock only.
fn connect_poll() -> Duration {
    Duration::from_millis(if stock_runtime() { 100 } else { 5 })
}

/// A [`Command`] for the `renode` launcher with ADR-004 Part A applied and
/// kill-on-parent-death set.
///
/// - `DOTNET_TieredPGO=0` (A1), for this child only: .NET 8's tier-0
///   instrumentation is pure startup cost for Renode, whose hot loop is
///   native tlib code. Codegen tiers never reach results (ADR-004,
///   "Determinism argument" 4). Not set with [`STOCK_RUNTIME_ENV`].
/// - The child gets `SIGKILL` when the thread that spawned it exits
///   (`PR_SET_PDEATHSIG`), so a killed `bb` or pool never leaves a Renode
///   behind (ADR-004 approver decision 7). The spawning thread must
///   therefore live as long as the process is used: `bb`'s main thread, a
///   test's thread, a pool slot's thread.
pub fn renode_command(renode: &Path) -> Command {
    let mut c = Command::new(renode);
    if !stock_runtime() {
        c.env("DOTNET_TieredPGO", "0");
    }
    die_with_parent(&mut c);
    c
}

#[cfg(target_os = "linux")]
fn die_with_parent(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    extern "C" {
        fn prctl(option: std::os::raw::c_int, ...) -> std::os::raw::c_int;
        fn getppid() -> std::os::raw::c_int;
    }
    const PR_SET_PDEATHSIG: std::os::raw::c_int = 1;
    const SIGKILL: std::os::raw::c_ulong = 9;
    const ESRCH: i32 = 3;
    let parent = std::process::id() as std::os::raw::c_int;
    // SAFETY: the closure runs in the forked child before exec and only
    // makes two async-signal-safe system calls; it does not allocate.
    unsafe {
        cmd.pre_exec(move || {
            if prctl(PR_SET_PDEATHSIG, SIGKILL) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            // The parent may have died between fork and prctl.
            if getppid() != parent {
                return Err(std::io::Error::from_raw_os_error(ESRCH));
            }
            Ok(())
        });
    }
}

#[cfg(not(target_os = "linux"))]
fn die_with_parent(_cmd: &mut Command) {}

/// Environment switch for the startup JIT profile (ADR-004 Amendment 6):
/// `BB_RENODE_JIT_PROFILE=off` starts fresh Renode processes without it.
/// A measurement knob and escape hatch; it never changes results.
#[doc(hidden)]
pub const JIT_PROFILE_ENV: &str = "BB_RENODE_JIT_PROFILE";

/// The .NET multi-core JIT startup profile of a fresh Renode (ADR-004
/// Amendment 6, A3). .NET 8 reads `DOTNET_MultiCoreJitProfile=<name>` at
/// startup: with a profile file `<name><suffix>` present, a background
/// thread JIT-compiles the methods the profile lists while the main thread
/// runs; the runtime appends `<suffix>` (`_clrhost_clrhost_1.prof` for the
/// apphost) and writes a new profile at a graceful shutdown. Only codegen
/// timing changes, never results (ADR-004 "Determinism argument" 4).
///
/// - **Playback** (a profile for this key exists): `MultiCoreJitProfile` =
///   `<dir>/<key>` and `MultiCoreJitNoProfileGather=1`, so the run never
///   writes the cache.
/// - **Gather** (none yet): `MultiCoreJitProfile` = a name unique to this
///   engine; at a clean teardown the engine stops Renode with `SIGTERM`
///   (graceful: the runtime writes the profile, about 0.1 s) instead of
///   `SIGKILL`, then renames the file into place. Parallel first runs each
///   write their own file and the last rename wins; every one is valid.
///
/// The key hashes the canonical Renode launcher, the platform description
/// and whether the lookahead plugin is loaded. A stale or unreadable profile
/// only costs JIT work: the runtime skips what it cannot match.
struct JitProfile {
    dir: PathBuf,
    key: String,
    /// `Some(name)`: gather into `<dir>/<name><suffix>`.
    gather: Option<String>,
}

const JIT_PROFILE_VERSION: &[u8] = b"bb-mcj-1";

fn jit_profile_dir() -> Option<PathBuf> {
    let base = std::env::var_os("BB_CACHE_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CACHE_HOME").map(|d| PathBuf::from(d).join("bb")))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache/bb")))?;
    Some(base.join("renode-jit"))
}

/// The profile file for `key` in `dir`, if one is there (any runtime suffix).
fn jit_profile_file(dir: &Path, key: &str) -> Option<PathBuf> {
    let prefix = format!("{key}_");
    std::fs::read_dir(dir).ok()?.flatten().find_map(|e| {
        let n = e.file_name().into_string().ok()?;
        (n.starts_with(&prefix) && n.ends_with(".prof")).then(|| e.path())
    })
}

impl JitProfile {
    /// `None` with the stock baseline, `BB_RENODE_JIT_PROFILE=off`, no cache
    /// directory, or an unreadable platform file.
    fn for_run(cfg: &RenodeConfig, instance: u64) -> Option<JitProfile> {
        if stock_runtime() || std::env::var(JIT_PROFILE_ENV).is_ok_and(|v| v == "off") {
            return None;
        }
        let platform = match &cfg.platform {
            Platform::Stm32f103Offline => STM32F103_OFFLINE_REPL.as_bytes().to_vec(),
            Platform::Stm32f072Bb => STM32F072_BB_REPL.as_bytes().to_vec(),
            // A path inside the Renode install (e.g. `platforms/cpus/...`)
            // is keyed by its name; the install is in the key already.
            Platform::Repl(p) => std::fs::read(p).unwrap_or_else(|_| p.as_bytes().to_vec()),
        };
        let renode_id = pool::canonical_renode(&cfg.renode);
        let key = format!(
            "renode-{:016x}",
            fnv1a64(&[
                JIT_PROFILE_VERSION,
                renode_id.to_string_lossy().as_bytes(),
                &platform,
                &[u8::from(cfg.lookahead.plugin)],
            ])
        );
        let dir = jit_profile_dir()?;
        std::fs::create_dir_all(&dir).ok()?;
        let gather = jit_profile_file(&dir, &key)
            .is_none()
            .then(|| format!("{key}.tmp-{}-{instance}", std::process::id()));
        Some(JitProfile { dir, key, gather })
    }

    fn apply(&self, cmd: &mut Command) {
        match &self.gather {
            None => {
                cmd.env("DOTNET_MultiCoreJitProfile", self.dir.join(&self.key));
                cmd.env("DOTNET_MultiCoreJitNoProfileGather", "1");
            }
            Some(name) => {
                cmd.env("DOTNET_MultiCoreJitProfile", self.dir.join(name));
                cmd.env_remove("DOTNET_MultiCoreJitNoProfileGather");
            }
        }
    }

    /// After a gathering Renode has exited: move its profile into place.
    /// Best effort; a missing file costs the next run its gather.
    fn publish(&self) {
        let Some(name) = &self.gather else { return };
        let Ok(rd) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for e in rd.flatten() {
            let Ok(n) = e.file_name().into_string() else {
                continue;
            };
            if let Some(suffix) = n.strip_prefix(name.as_str()) {
                let dst = self.dir.join(format!("{}{suffix}", self.key));
                if !suffix.ends_with(".prof") || std::fs::rename(e.path(), &dst).is_err() {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    }
}

/// Stop a gathering child gracefully (`SIGTERM`, the runtime writes its JIT
/// profile), falling back to `SIGKILL` after `grace`.
fn terminate_gracefully(c: &mut Child, grace: Duration) {
    #[cfg(unix)]
    {
        extern "C" {
            fn kill(pid: std::os::raw::c_int, sig: std::os::raw::c_int) -> std::os::raw::c_int;
        }
        const SIGTERM: std::os::raw::c_int = 15;
        if let Ok(Some(_)) = c.try_wait() {
            return;
        }
        // SAFETY: plain kill(2) on our own, not yet reaped, child.
        if unsafe { kill(c.id() as std::os::raw::c_int, SIGTERM) } == 0 {
            let deadline = Instant::now() + grace;
            while Instant::now() < deadline {
                if let Ok(Some(_)) = c.try_wait() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
    let _ = grace;
    let _ = c.kill();
    let _ = c.wait();
}

/// Locate the `renode` launcher: `$BB_RENODE`, then `renode` on `PATH`, then
/// `~/.local/bin/renode` (where `make setup` installs it).
pub fn find_renode() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("BB_RENODE") {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let p = dir.join("renode");
            if p.is_file() {
                return Some(p);
            }
        }
    }
    let home = std::env::var_os("HOME")?;
    let p = Path::new(&home).join(".local/bin/renode");
    p.is_file().then_some(p)
}

struct PinState {
    spec: PinSpec,
    gpio: i32,
    level: Level,
    edge_time: Time,
    /// Input pin: the last level driven or queued to be driven.
    scheduled: Option<Level>,
    /// ADC input: the last value (µV) set or queued to be set.
    scheduled_uv: Option<u32>,
    /// ADC input: the last value (µV) set (for the cross-check).
    applied_uv: Option<u32>,
    /// ADC input on Breadboard's model: pushes are timed inputs through the
    /// host port (a sampled input, ADR-005 D3), not `SetValue` splits.
    timed: bool,
}

/// What an input transition drives.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum InputValue {
    Level(Level),
    Microvolts(u32),
}

/// An input transition waiting for its lattice point `ceil_L(t)`.
struct PendingInput {
    at_ns: u64,
    pin: usize,
    value: InputValue,
}

/// A [`bbcore::Engine`] backed by one headless Renode process.
pub struct RenodeEngine {
    name: String,
    quantum_ns: u64,
    verify: Verify,
    /// An input was driven (`SET_STATE`) since the last advance.
    input_applied: bool,
    /// Number of cross-checks run (time + GPIO counted as one).
    verifications: u64,
    fault: Option<Fault>,
    machine: i32,
    /// `"sysbus"` bus-context descriptor, fetched on first use.
    sysbus: Option<i32>,
    host: Host,
    workdir: PathBuf,
    client: Client,
    now: Time,
    renode_ns: u64,
    promised: Time,
    pins: Vec<PinState>,
    /// Every output change of the last advance, in emulation order (ADR-003
    /// D3).
    trace: Vec<Transition>,
    /// Input transitions queued for a lattice point inside the next advance,
    /// sorted by `at_ns` (stable: delivery order within a point).
    pending: Vec<PendingInput>,
    /// Timed ADC inputs `(t_ns, pin, uV)` not yet written to the host port
    /// (ADR-005 D5), in delivery order.
    adc_batch: Vec<(u64, usize, u32)>,
    /// GPIO events stamped exactly at the last grant end: they belong to the
    /// next grant (lattice rule 3).
    carried: Vec<protocol::GpioEvent>,
    run_for_calls: u64,
    boundary_edges: u64,
    carried_edges: u64,
    /// An advance or input failed: Drop keeps `workdir` (with `renode.log`)
    /// for diagnosis.
    failed: bool,
    /// Lookahead (ADR-003 D5).
    la: Lookahead,
    /// The fresh child's startup JIT profile (ADR-004 Amendment 6).
    jit: Option<JitProfile>,
    /// What [`RenodeEngine::reconnect`] needs (ADR-004 Amendment 8).
    ec: EcConnection,
}

/// The External Control connection's parameters, for reconnecting.
struct EcConnection {
    port: u16,
    machine: String,
    io_timeout: Duration,
    /// [`RenodeConfig::ec_messages_per_connection`].
    messages_per_connection: u64,
    /// Connections replaced so far.
    reconnects: u64,
}

/// Lookahead state of a [`RenodeEngine`].
struct Lookahead {
    cfg: LookaheadCfg,
    /// Startup snapshot (plugin loaded): self-test result.
    startup: Option<LookaheadStatus>,
    /// The snapshot of the last read, while it still describes Renode's
    /// state (no advance or `SET_STATE` since): a Query at the same time
    /// reuses it.
    cached: Option<LookaheadStatus>,
    /// The promise of the last `quiet_until`, `(from, until)`, valid for the
    /// one advance that starts at `from`.
    promise: Option<(Time, Time)>,
    /// A Query failed inside `quiet_until` (which cannot return an error):
    /// reported by the next advance.
    deferred: Option<EngineError>,
    stats: LookaheadStats,
}

static INSTANCE: AtomicU64 = AtomicU64::new(0);

fn monitor_path(p: &Path) -> Result<String, EngineError> {
    let s = p
        .to_str()
        .ok_or_else(|| EngineError::Other(format!("path is not UTF-8: {}", p.display())))?;
    if s.chars().any(|c| c.is_whitespace() || c == ';' || c == '"') {
        return Err(EngineError::Other(format!(
            "path {s:?} contains whitespace, ';' or '\"', which the Renode monitor cannot take"
        )));
    }
    Ok(s.to_string())
}

/// Exact decimal seconds for a whole number of nanoseconds, e.g. `0.000010000`.
fn ns_to_seconds_str(ns: u64) -> String {
    format!("{}.{:09}", ns / 1_000_000_000, ns % 1_000_000_000)
}

/// Ports already handed to a Renode instance of this process. The OS may
/// hand the same ephemeral port to two `bind(0)` calls once the first probe
/// socket is closed; two engines started concurrently would then race for
/// one server port and one would connect to the other's Renode.
static PORTS: Mutex<BTreeSet<u16>> = Mutex::new(BTreeSet::new());

pub(crate) fn free_port() -> Result<u16, EngineError> {
    for _ in 0..64 {
        let l = TcpListener::bind("127.0.0.1:0")?;
        let port = l.local_addr()?.port();
        if PORTS.lock().unwrap_or_else(|e| e.into_inner()).insert(port) {
            return Ok(port);
        }
    }
    Err(EngineError::Other(
        "no unused local port for the External Control server".into(),
    ))
}

fn log_tail(path: &Path) -> String {
    let mut s = String::new();
    if let Ok(mut f) = std::fs::File::open(path) {
        let _ = f.read_to_string(&mut s);
    }
    let lines: Vec<&str> = s.lines().collect();
    lines[lines.len().saturating_sub(30)..].join("\n")
}

impl RenodeEngine {
    /// Start Renode, load the platform and firmware, connect, and register the
    /// pins. Renode's virtual time is 0 and it is paused when this returns.
    pub fn start(cfg: &RenodeConfig) -> Result<RenodeEngine, EngineError> {
        let quantum_ns = cfg.quantum.as_fs() / FS_PER_NS;
        if quantum_ns == 0 || !cfg.quantum.as_fs().is_multiple_of(FS_PER_NS) {
            return Err(EngineError::Other(format!(
                "quantum {} must be a positive whole number of ns",
                cfg.quantum
            )));
        }
        // Renode's LoadELF opens the firmware about a dozen times. On a slow
        // file system (WSL's /mnt/c: 10-130 ms an open or stat under host
        // load) that cost 0.1-1.1 s of startup, so bb reads it once and
        // Renode loads a copy in the workdir: the same bytes under the same
        // file name (ADR-004 Amendment 7).
        let fw_err = |e: &dyn std::fmt::Display| {
            EngineError::Other(format!("firmware {}: {e}", cfg.firmware.display()))
        };
        let fw_name = cfg
            .firmware
            .file_name()
            .ok_or_else(|| fw_err(&"no file name"))?;
        let fw_bytes = std::fs::read(&cfg.firmware).map_err(|e| fw_err(&e))?;

        let instance = INSTANCE.fetch_add(1, Ordering::Relaxed);
        let workdir =
            std::env::temp_dir().join(format!("bb-renode-{}-{instance}", std::process::id()));
        let _ = std::fs::remove_dir_all(&workdir);
        std::fs::create_dir_all(&workdir)?;
        let firmware = workdir.join("fw").join(fw_name);
        std::fs::create_dir_all(workdir.join("fw"))?;
        std::fs::write(&firmware, &fw_bytes)?;
        drop(fw_bytes);
        let platform = match &cfg.platform {
            Platform::Stm32f103Offline => {
                let p = workdir.join("platform.repl");
                std::fs::write(&p, STM32F103_OFFLINE_REPL)?;
                monitor_path(&p)?
            }
            Platform::Stm32f072Bb => {
                let p = workdir.join("platform.repl");
                std::fs::write(&p, STM32F072_BB_REPL)?;
                monitor_path(&p)?
            }
            Platform::Repl(p) => monitor_path(Path::new(p))?,
        };
        let port = free_port()?;
        let la = &cfg.lookahead;
        if la.enabled && !la.plugin {
            return Err(EngineError::Other(
                "lookahead enabled without the lookahead plugin".into(),
            ));
        }
        let plugin_err = workdir.join(PLUGIN_ERR);
        // The plugin assembly holds the lookahead block and the platform's
        // Breadboard peripherals (ADR-005 D6): loaded for either.
        let plugin_cache = if la.plugin || cfg.platform.needs_plugins() {
            Some(plugin_cache_path(&cfg.renode, la.cache_dir.as_deref())?)
        } else {
            None
        };
        // ADR-004 Part B: a warm process from the pool, if one is idle and
        // matches this Renode and plugin; otherwise a fresh Renode.
        let lease = match &cfg.pool {
            Some(sock) => {
                let key = pool::PoolKey {
                    renode: pool::canonical_renode(&cfg.renode)
                        .to_string_lossy()
                        .into_owned(),
                    plugin: plugin_cache
                        .as_ref()
                        .map(|p| p.to_string_lossy().into_owned()),
                };
                match pool::Lease::acquire(sock, &key, Duration::from_secs(10)) {
                    Ok(l) => l,
                    Err(e) => {
                        eprintln!("{}: {e}; starting Renode fresh", cfg.name);
                        None
                    }
                }
            }
            None => None,
        };
        // The loader runs before the platform, which may name the plugin's
        // types (ADR-005 D6). A pool process loaded the cached assembly at
        // pre-warm, so a lease leaves the loader out (ADR-004 "Lease" 2).
        let plugin_load = match &plugin_cache {
            Some(cache) if lease.is_none() => {
                let loader = write_plugin_loader(&workdir, cache, &plugin_err)?;
                format!("include @{}\n", monitor_path(&loader)?)
            }
            _ => String::new(),
        };
        let (lookahead_map, plugin_arm) = if la.plugin {
            (
                format!(
                    "machine LoadPlatformDescriptionFromString \"{LOOKAHEAD_NAME}: Miscellaneous.BbLookahead @ sysbus {:#x}\"\n",
                    la.address
                ),
                format!("sysbus.{LOOKAHEAD_NAME} Arm\n"),
            )
        } else {
            (String::new(), String::new())
        };
        let extra: String = cfg.extra_monitor.iter().map(|l| format!("{l}\n")).collect();
        let script = format!(
            "{plugin_load}\
             mach create \"{machine}\"\n\
             machine LoadPlatformDescription @{platform}\n\
             {lookahead_map}\
             sysbus LoadELF @{elf}\n\
             emulation SetSeed {seed}\n\
             emulation SetGlobalQuantum \"{quantum}\"\n\
             emulation SetGlobalSerialExecution true\n\
             emulation SetGlobalAdvanceImmediately true\n\
             {extra}\
             {plugin_arm}\
             logLevel 2\n\
             emulation CreateExternalControlServer \"bb\" {port}\n",
            machine = cfg.machine,
            elf = monitor_path(&firmware)?,
            seed = cfg.seed,
            quantum = ns_to_seconds_str(quantum_ns),
        );
        let script_path = workdir.join("session.resc");
        std::fs::write(&script_path, script)?;
        let log_path = workdir.join("renode.log");

        let fail = |host: &mut Option<Host>, msg: String| -> EngineError {
            if let Some(h) = host.take() {
                h.abort();
            }
            let tail = log_tail(&log_path);
            let plugin = std::fs::read_to_string(&plugin_err)
                .map(|e| format!("\n--- lookahead plugin ---\n{e}"))
                .unwrap_or_default();
            let _ = std::fs::remove_dir_all(&workdir);
            EngineError::Other(format!("{msg}{plugin}\n--- renode log (tail) ---\n{tail}"))
        };
        let mut jit = None;
        let mut host = Some(match lease {
            Some(l) => Host::Lease(l),
            None => {
                let log = std::fs::File::create(&log_path)?;
                let mut cmd = renode_command(&cfg.renode);
                jit = JitProfile::for_run(cfg, instance);
                if let Some(j) = &jit {
                    j.apply(&mut cmd);
                }
                let child = cmd
                    .args(["--disable-xwt", "--plain", "--port", "-1"])
                    .arg(&script_path)
                    .current_dir(&workdir)
                    .stdin(Stdio::null())
                    .stdout(log.try_clone()?)
                    .stderr(log)
                    .spawn()
                    .map_err(|e| {
                        EngineError::Other(format!("spawn {}: {e}", cfg.renode.display()))
                    })?;
                Host::Child(child)
            }
        });
        // A leased process runs the same script from its console; the
        // manager copies its output into this run's renode.log.
        if let Some(Host::Lease(l)) = host.as_mut() {
            if let Err(e) = l.run(
                &monitor_path(&workdir)?,
                &monitor_path(&script_path)?,
                port,
                cfg.startup_timeout,
            ) {
                return Err(fail(&mut host, format!("pooled renode: {e}")));
            }
        }

        // Wait for the External Control server.
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let deadline = Instant::now() + cfg.startup_timeout;
        let stream = loop {
            if let Some(Host::Child(c)) = host.as_mut() {
                if let Ok(Some(status)) = c.try_wait() {
                    return Err(fail(
                        &mut host,
                        format!("renode exited during startup ({status})"),
                    ));
                }
            }
            match TcpStream::connect_timeout(&addr, Duration::from_millis(500)) {
                // TCP self-connect: while nothing listens on the port yet, the
                // kernel can pick the port itself as this socket's ephemeral
                // source port, and the connection then reaches itself. Seen
                // with many engines starting at once (ADR-003 Amendment 3).
                Ok(s) if s.local_addr().ok() == s.peer_addr().ok() && Instant::now() < deadline => {
                    drop(s);
                    std::thread::sleep(connect_poll());
                }
                Ok(s) => break s,
                Err(_) if Instant::now() < deadline => std::thread::sleep(connect_poll()),
                Err(e) => {
                    return Err(fail(
                        &mut host,
                        format!("no External Control server on port {port}: {e}"),
                    ))
                }
            }
        };

        type Setup = (Client, i32, Vec<PinState>, u64, Option<(i32, Vec<u32>)>);
        let setup = || -> Result<Setup, ProtocolError> {
            let mut client = Client::handshake(stream, cfg.io_timeout)?;
            let md = client.get_machine(&cfg.machine)?;
            let mut pins = Vec::new();
            for (i, spec) in cfg.pins.iter().enumerate() {
                if spec.direction == PinDirection::Adc {
                    let adc = client.get_adc(md, &spec.gpio)?;
                    let n = client.adc_channel_count(adc)?;
                    if !(0..n).contains(&spec.index) {
                        return Err(ProtocolError::Server(format!(
                            "{}: ADC {} has {n} channels, no channel {}",
                            spec.name, spec.gpio, spec.index
                        )));
                    }
                    pins.push(PinState {
                        spec: spec.clone(),
                        gpio: adc,
                        level: Level::Low,
                        edge_time: Time::ZERO,
                        scheduled: None,
                        scheduled_uv: None,
                        applied_uv: None,
                        timed: cfg.platform == Platform::Stm32f072Bb,
                    });
                    continue;
                }
                let gpio = client.get_gpio(md, &spec.gpio)?;
                let level = Level::from_bool(client.gpio_get_state(gpio, spec.index)?);
                if spec.direction == PinDirection::Output {
                    client.gpio_register_event(gpio, spec.index, i as i32)?;
                }
                pins.push(PinState {
                    spec: spec.clone(),
                    gpio,
                    level,
                    edge_time: Time::ZERO,
                    scheduled: None,
                    scheduled_uv: None,
                    applied_uv: None,
                    timed: false,
                });
            }
            let t0 = client.get_time()?;
            // The startup snapshot of the lookahead block: armed, self-test.
            let la = if cfg.lookahead.plugin {
                let sb = client.get_sysbus(md, "sysbus")?;
                let w = client.sysbus_read_u32s(sb, cfg.lookahead.address, 8)?;
                Some((sb, w))
            } else {
                None
            };
            Ok((client, md, pins, t0, la))
        };
        let (client, machine, pins, t0, la_words) = match setup() {
            Ok(v) => v,
            Err(e) => return Err(fail(&mut host, format!("setup failed: {e}"))),
        };
        if t0 != 0 {
            return Err(fail(
                &mut host,
                format!("Renode virtual time is {t0} ns at start, expected 0"),
            ));
        }
        let mut sysbus = None;
        let mut startup = None;
        if let Some((sb, w)) = la_words {
            let st = match LookaheadStatus::from_words(0, &w) {
                Ok(st) if st.armed() => st,
                Ok(st) => {
                    return Err(fail(
                        &mut host,
                        format!(
                            "lookahead block at {:#x} is not armed (STATUS {:#x})",
                            cfg.lookahead.address, st.status
                        ),
                    ))
                }
                Err(e) => return Err(fail(&mut host, e.to_string())),
            };
            if !st.self_test_ok() {
                eprintln!(
                    "{}: lookahead self-test failed (reasons {:#04x}, see bb_lookahead.cs): \
                     no quiet promises, the board keeps the loop cap",
                    cfg.name,
                    st.self_test_failures()
                );
            }
            // Caching is best effort: a failure costs the next run the compile.
            let mut cache_err = plugin_err.clone().into_os_string();
            cache_err.push(".cache");
            if let Ok(e) = std::fs::read_to_string(&cache_err) {
                eprintln!("{}: lookahead plugin: {e}", cfg.name);
            }
            sysbus = Some(sb);
            startup = Some(st);
        }
        Ok(RenodeEngine {
            name: cfg.name.clone(),
            quantum_ns,
            verify: cfg.verify,
            input_applied: false,
            verifications: 0,
            fault: None,
            machine,
            sysbus,
            host: host.take().expect("host present"),
            workdir,
            client,
            now: Time::ZERO,
            renode_ns: 0,
            promised: Time::ZERO,
            pins,
            trace: Vec::new(),
            pending: Vec::new(),
            adc_batch: Vec::new(),
            carried: Vec::new(),
            run_for_calls: 0,
            boundary_edges: 0,
            carried_edges: 0,
            failed: false,
            jit,
            ec: EcConnection {
                port,
                machine: cfg.machine.clone(),
                io_timeout: cfg.io_timeout,
                messages_per_connection: cfg.ec_messages_per_connection,
                reconnects: 0,
            },
            la: Lookahead {
                cfg: cfg.lookahead.clone(),
                startup,
                cached: startup,
                promise: None,
                deferred: None,
                stats: LookaheadStats::default(),
            },
        })
    }

    /// The lookahead block's startup snapshot (self-test result), if the
    /// plugin is loaded.
    pub fn lookahead_startup(&self) -> Option<LookaheadStatus> {
        self.la.startup
    }

    /// Lookahead counters.
    pub fn lookahead_stats(&self) -> LookaheadStats {
        self.la.stats
    }

    /// Read the lookahead block now (a Query: one `SYSTEM_BUS` read of 8
    /// dwords). A pure read for the emulation; it resets the block's
    /// first-exit latch. Errors if the plugin is not loaded.
    pub fn lookahead_query(&mut self) -> Result<LookaheadStatus, EngineError> {
        if !self.la.cfg.plugin {
            return Err(EngineError::Other("lookahead plugin not loaded".into()));
        }
        let w = self.sysbus_read_u32s(self.la.cfg.address, 8)?;
        self.la.stats.reads += 1;
        let st = LookaheadStatus::from_words(self.renode_ns, &w)?;
        self.la.cached = Some(st);
        Ok(st)
    }

    /// `quiet_until` (ADR-003 D5) with errors.
    fn quiet_inner(&mut self, now: Time) -> Result<Time, EngineError> {
        // Outputs change right after `now`: an event stamped at the last
        // grant end is reported at `now + 1 fs` (lattice rule 3).
        if !self.carried.is_empty() {
            return Ok(now);
        }
        let st = match self.la.cached {
            Some(st) if st.at_ns == self.renode_ns => st,
            _ => self.lookahead_query()?,
        };
        self.la.stats.queries += 1;
        let Some(mut p_ns) = st.promise_ns() else {
            return Ok(now);
        };
        if let Some(Fault::StretchPromise { ns }) = self.fault {
            p_ns = p_ns.saturating_add(ns);
            self.fault = None;
        }
        // The promise assumes constant inputs: an input already queued for
        // a lattice point inside the window ends it there.
        if let Some(first) = self.pending.first() {
            p_ns = p_ns.min(first.at_ns);
        }
        let until = promise_time(p_ns);
        if until <= now {
            return Ok(now);
        }
        self.la.promise = Some((now, until));
        self.la.stats.promises += 1;
        Ok(until)
    }

    /// The audit after an advance from `from` that followed the promise
    /// `until` (ADR-003 D5.4): no output change and no WFI exit before
    /// `until`. Leaves the post-advance snapshot cached for the next Query.
    fn audit(&mut self, from: Time, until: Time) -> Result<(), EngineError> {
        self.la.stats.audits += 1;
        let broken = |at: Time| EngineError::PromiseBroken {
            promised: until,
            at,
        };
        if let Some(tr) = self.trace.iter().find(|tr| tr.t > from && tr.t < until) {
            return Err(broken(tr.t));
        }
        // A change stamped at the grant end executed right after it.
        let carried_at = Time::from_ns(self.renode_ns) + Time::from_fs(1);
        if !self.carried.is_empty() && carried_at < until {
            return Err(broken(carried_at));
        }
        let st = self.lookahead_query()?;
        if let Some(e) = st.first_exit_ns.map(Time::from_ns) {
            if e < until {
                return Err(broken(e));
            }
        }
        Ok(())
    }

    /// Renode's virtual time as tracked by the engine, in ns. Always
    /// `floor(now / 1 ns)`.
    pub fn renode_time_ns(&self) -> u64 {
        self.renode_ns
    }

    /// Query Renode's virtual time (ns) over the API.
    pub fn query_renode_time_ns(&mut self) -> Result<u64, EngineError> {
        self.client.get_time().map_err(proto)
    }

    /// External Control connections replaced so far (ADR-004 Amendment 8).
    pub fn ec_reconnects(&self) -> u64 {
        self.ec.reconnects
    }

    /// Messages sent on the current External Control connection.
    pub fn ec_messages_sent(&self) -> u64 {
        self.client.messages_sent()
    }

    /// Number of `RUN_FOR` requests sent so far.
    pub fn run_for_calls(&self) -> u64 {
        self.run_for_calls
    }

    /// Edges stamped on a lattice point and therefore reported 1 fs after
    /// it (lattice rule 1).
    pub fn boundary_edges(&self) -> u64 {
        self.boundary_edges
    }

    /// GPIO events stamped exactly at a grant end and carried over to the
    /// next grant (lattice rule 3).
    pub fn carried_edges(&self) -> u64 {
        self.carried_edges
    }

    /// Number of time/GPIO cross-checks run so far (see [`Verify`]).
    pub fn verifications(&self) -> u64 {
        self.verifications
    }

    /// Arm a test fault (see [`Fault`]). Replaces any armed fault.
    #[doc(hidden)]
    pub fn inject_fault(&mut self, fault: Fault) {
        self.fault = Some(fault);
    }

    fn sysbus_id(&mut self) -> Result<i32, EngineError> {
        if let Some(id) = self.sysbus {
            return Ok(id);
        }
        let id = self
            .client
            .get_sysbus(self.machine, "sysbus")
            .map_err(proto)?;
        self.sysbus = Some(id);
        Ok(id)
    }

    /// Read `count` 32-bit words at absolute address `address` over the
    /// External Control `SYSTEM_BUS` command (global bus context). A pure
    /// read for memory and for peripherals whose reads have no side effects.
    pub fn sysbus_read_u32s(&mut self, address: u64, count: u32) -> Result<Vec<u32>, EngineError> {
        let id = self.sysbus_id()?;
        self.client
            .sysbus_read_u32s(id, address, count)
            .map_err(proto)
    }

    /// Write one 32-bit word at absolute address `address` (global bus
    /// context). Changes emulation state: never called by the kernel path.
    pub fn sysbus_write_u32(&mut self, address: u64, value: u32) -> Result<(), EngineError> {
        let id = self.sysbus_id()?;
        self.client
            .sysbus_write_u32(id, address, value)
            .map_err(proto)
    }

    fn pin_index(&self, name: &str) -> Option<usize> {
        self.pins.iter().position(|p| p.spec.name == name)
    }

    /// Directory with the generated script and `renode.log`. Removed on drop
    /// unless the engine failed.
    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    /// The process id of this engine's Renode.
    pub fn renode_pid(&self) -> u32 {
        self.host.pid()
    }

    /// The Renode process was leased from a warm pool (ADR-004 Part B)
    /// rather than spawned for this engine.
    pub fn leased(&self) -> bool {
        matches!(self.host, Host::Lease(_))
    }

    /// Mark the engine failed and attach the Renode log tail (and where the
    /// full log is kept) to an error from the running phase.
    fn failure(&mut self, e: EngineError) -> EngineError {
        self.failed = true;
        let exited = self.host.status_note();
        let log = self.workdir.join("renode.log");
        let msg = format!(
            "{e}\n--- {exited}renode log kept at {} (tail) ---\n{}",
            log.display(),
            log_tail(&log)
        );
        match e {
            EngineError::Protocol(_) => EngineError::Protocol(msg),
            _ => EngineError::Other(msg),
        }
    }
}

fn proto(e: ProtocolError) -> EngineError {
    match e {
        ProtocolError::Io(e) => EngineError::Io(e),
        ProtocolError::Server(m) => EngineError::Protocol(m),
    }
}

impl Engine for RenodeEngine {
    fn name(&self) -> &str {
        &self.name
    }

    fn next_safe_time(&mut self, _now: Time) -> Time {
        // ADR-003 D4: every change of a grant is reported in the trace with
        // its lattice-rule time, so any grant is safe. This is not a promise
        // that outputs stay constant (the firmware may change an output at
        // any instruction); consumers later in the plan order apply each
        // change at its time.
        self.promised = Time::MAX;
        self.promised
    }

    fn read_output_trace(&mut self) -> Result<Option<Vec<Transition>>, EngineError> {
        Ok(Some(self.trace.clone()))
    }

    fn grant_lattice(&self) -> Option<Time> {
        Some(Time::from_ns(self.quantum_ns))
    }

    /// ADR-005 D3: an ADC input on Breadboard's model is sampled. The model
    /// reads it only at its sample instants, each a clock limit at or after
    /// the lookahead promise, and a push changes nothing else in Renode.
    fn input_is_sampled(&self, pin: &str) -> bool {
        self.pins
            .iter()
            .any(|p| p.spec.name == pin && p.spec.direction == PinDirection::Adc && p.timed)
    }

    fn advance_to(&mut self, t: Time) -> Result<(), EngineError> {
        // A grant outside the promise or off the lattice is the caller's
        // error; Renode was not touched, so there is no log worth keeping.
        if t <= self.now || t > self.promised {
            return Err(EngineError::Protocol(format!(
                "advance_to({t}) outside ({}, {}]",
                self.now, self.promised
            )));
        }
        if !t.as_fs().is_multiple_of(self.quantum_ns * FS_PER_NS) {
            return Err(EngineError::Protocol(format!(
                "advance_to({t}) is not on the grant lattice {} (a RUN_FOR must never \
                 end off the lattice, ADR-003 D2)",
                Time::from_ns(self.quantum_ns)
            )));
        }
        if let Some(e) = self.la.deferred.take() {
            return Err(self.failure(e));
        }
        // A promise holds for the one advance that starts where it was given.
        let promise = self.la.promise.take().filter(|(from, _)| *from == self.now);
        let from = self.now;
        self.la.cached = None;
        self.advance_inner(t).map_err(|e| self.failure(e))?;
        if let Some((_, until)) = promise {
            if let Err(e) = self.audit(from, until) {
                return Err(match e {
                    EngineError::PromiseBroken { .. } => {
                        self.failed = true;
                        e
                    }
                    e => self.failure(e),
                });
            }
        }
        Ok(())
    }

    /// ADR-003 D5: while the CPU sleeps in WFI with nothing pending or
    /// queued, it executes nothing (so no output changes) before the clock
    /// source's nearest limit. Returns `now` (no promise) without the
    /// plugin, with lookahead off, after a failed self-test, when the
    /// engine is not at `now`, or when an output change carried over from
    /// the last grant end is due right after `now`.
    fn quiet_until(&mut self, now: Time) -> Time {
        self.la.promise = None;
        let usable = self.la.cfg.enabled
            && self.la.startup.is_some_and(|s| s.self_test_ok())
            && self.la.deferred.is_none()
            && now == self.now;
        if !usable {
            return now;
        }
        match self.quiet_inner(now) {
            Ok(t) => t,
            Err(e) => {
                self.la.deferred = Some(e);
                now
            }
        }
    }

    fn read_outputs(&mut self) -> Result<PinSignals, EngineError> {
        Ok(self
            .pins
            .iter()
            .filter(|p| p.spec.direction == PinDirection::Output)
            .map(|p| {
                (
                    p.spec.name.clone(),
                    PinSignal::digital(p.level, p.edge_time),
                )
            })
            .collect())
    }

    fn apply_inputs(&mut self, inputs: &PinSignals) -> Result<(), EngineError> {
        self.apply_inner(inputs).map_err(|e| self.failure(e))
    }

    fn apply_input_trace(
        &mut self,
        base: &PinSignals,
        trace: &[Transition],
    ) -> Result<(), EngineError> {
        self.apply_trace_inner(base, trace)
            .map_err(|e| self.failure(e))
    }

    /// The end-of-run cross-check of [`Verify::Edges`] and [`Verify::Final`]
    /// (with [`Verify::Every`] the last advance has just been checked).
    fn end_of_run(&mut self) -> Result<(), EngineError> {
        if self.ec.reconnects > 0 {
            // stderr only: never part of the outputs.
            eprintln!(
                "{}: External Control reconnected {} times (every {} messages, ADR-004 \
                 Amendment 8)",
                self.name, self.ec.reconnects, self.ec.messages_per_connection
            );
        }
        if self.verify == Verify::Every {
            return Ok(());
        }
        let r = self.verify_time().and_then(|()| self.verify_pins());
        self.verifications += 1;
        r.map_err(|e| self.failure(e))
    }
}

impl RenodeEngine {
    /// `GET_TIME` must equal the tracked time.
    fn verify_time(&mut self) -> Result<(), EngineError> {
        let vt = self.client.get_time().map_err(proto)?;
        if vt != self.renode_ns {
            return Err(EngineError::Protocol(format!(
                "verify: Renode virtual time {vt} ns, expected {} ns",
                self.renode_ns
            )));
        }
        Ok(())
    }

    /// `GPIO GET_STATE` of every output pin must equal the tracked level,
    /// or, for a pin with an event carried over to the next grant (lattice
    /// rule 3), the level of its last carried event: Renode already shows it.
    fn verify_pins(&mut self) -> Result<(), EngineError> {
        // The bus descriptor, fetched once (not once per cross-check).
        let bus = if self.pins.iter().any(|p| p.timed) {
            Some(self.sysbus_id()?)
        } else {
            None
        };
        for (i, p) in self.pins.iter().enumerate() {
            if p.spec.direction == PinDirection::Adc && p.timed {
                // The host port: the channel's latest timed input, and no
                // timed input that came too late for a sample (ADR-005 D5.3).
                let ch = u64::try_from(p.spec.index).unwrap_or(0);
                let id = bus.expect("fetched above for a timed pin");
                let got = self
                    .client
                    .sysbus_read_u32s(id, BB_ADC_PORT_ADDRESS + 0x100 + 4 * ch, 1)
                    .map_err(proto)?[0];
                let err = self
                    .client
                    .sysbus_read_u32s(id, BB_ADC_PORT_ADDRESS + 4, 4)
                    .map_err(proto)?;
                if err[0] != 0 {
                    return Err(EngineError::Protocol(format!(
                        "verify: {}: the ADC host port reports error {:#x} (first at {} ns, \
                         channel {}): a timed input arrived after a sample it would have changed",
                        p.spec.name,
                        err[0],
                        u64::from(err[1]) | (u64::from(err[2]) << 32),
                        err[3]
                    )));
                }
                if let Some(want) = p.applied_uv {
                    if got != want {
                        return Err(EngineError::Protocol(format!(
                            "verify: {}: ADC channel {} holds {got} uV at {} ns, pushed {want} uV",
                            p.spec.name, p.spec.index, self.renode_ns
                        )));
                    }
                }
                continue;
            }
            if p.spec.direction == PinDirection::Adc {
                // ADC GetValue must read back the last value set.
                if let Some(want) = p.applied_uv {
                    let got = self
                        .client
                        .adc_get_value(p.gpio, p.spec.index)
                        .map_err(proto)?;
                    if got != want {
                        return Err(EngineError::Protocol(format!(
                            "verify: {}: ADC channel {} reads {got} uV at {} ns, set {want} uV",
                            p.spec.name, p.spec.index, self.renode_ns
                        )));
                    }
                }
                continue;
            }
            if p.spec.direction != PinDirection::Output {
                continue;
            }
            let want = self
                .carried
                .iter()
                .rev()
                .find(|e| e.descriptor == i as i32)
                .map_or(p.level, |e| Level::from_bool(e.state));
            let s = self
                .client
                .gpio_get_state(p.gpio, p.spec.index)
                .map_err(proto)?;
            if Level::from_bool(s) != want {
                return Err(EngineError::Protocol(format!(
                    "verify: {}: Renode reports {s} at {} ns but events say {want:?}",
                    p.spec.name, self.renode_ns
                )));
            }
        }
        Ok(())
    }

    /// One `RUN_FOR` from the tracked time to `to_ns` (a lattice point);
    /// collects the GPIO events it produced.
    fn run_to(
        &mut self,
        to_ns: u64,
        events: &mut Vec<protocol::GpioEvent>,
    ) -> Result<(), EngineError> {
        if to_ns <= self.renode_ns {
            return Ok(());
        }
        let mut ns = to_ns - self.renode_ns;
        if let Some(Fault::RunForExtraNs { at, ns: extra }) = self.fault {
            if Time::from_ns(to_ns) >= at {
                ns += extra;
                self.fault = None;
            }
        }
        self.reconnect_if_due()?;
        self.client.run_for(ns).map_err(proto)?;
        self.run_for_calls += 1;
        self.renode_ns = to_ns;
        events.extend(self.client.take_events());
        Ok(())
    }

    /// Replace the External Control connection once it has carried
    /// [`RenodeConfig::ec_messages_per_connection`] messages. Called right
    /// before a `RUN_FOR`, so Renode is paused.
    fn reconnect_if_due(&mut self) -> Result<(), EngineError> {
        let n = self.ec.messages_per_connection;
        if n == 0 || self.client.messages_sent() < n {
            return Ok(());
        }
        self.reconnect()
    }

    /// Close the External Control connection and open a new one while
    /// Renode is paused (ADR-004 Amendment 8). Renode 1.17.0's server
    /// leaks receive-thread stack per message ([`protocol::EC_MESSAGE_LIMIT`]);
    /// a new connection gets a new receive thread.
    ///
    /// After the client disconnects, Renode disposes the connection's
    /// command handlers and listens on the same port again
    /// (`ExternalControlSocket.Disconnect(Unconnected)` →
    /// `RestartConnection`), so the connect is retried until the server is
    /// back. Descriptors (machine, GPIO, ADC, bus) belong to the handlers and
    /// are fetched again. The GPIO event hooks registered on the first
    /// connection stay on the pins and send through the socket object, so
    /// they are not registered again (that would report every edge twice).
    /// Nothing here touches emulation state: no time passes and no
    /// peripheral is accessed, so results never depend on when (or whether)
    /// the engine reconnects.
    fn reconnect(&mut self) -> Result<(), EngineError> {
        let pending_events = self.client.take_events();
        self.client.close();
        let addr = SocketAddr::from(([127, 0, 0, 1], self.ec.port));
        let deadline = Instant::now() + self.ec.io_timeout.min(Duration::from_secs(30));
        let stream = loop {
            if let Host::Child(c) = &mut self.host {
                if let Ok(Some(status)) = c.try_wait() {
                    return Err(EngineError::Protocol(format!(
                        "renode exited while reconnecting External Control ({status})"
                    )));
                }
            }
            match TcpStream::connect_timeout(&addr, Duration::from_millis(500)) {
                // TCP self-connect while nothing listens (see `start`).
                Ok(s) if s.local_addr().ok() == s.peer_addr().ok() && Instant::now() < deadline => {
                    drop(s);
                    std::thread::sleep(Duration::from_millis(1));
                }
                Ok(s) => break s,
                Err(_) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(e) => {
                    return Err(EngineError::Protocol(format!(
                        "External Control reconnect: no server on port {} again: {e}",
                        self.ec.port
                    )))
                }
            }
        };
        let mut client = Client::handshake(stream, self.ec.io_timeout).map_err(proto)?;
        let md = client.get_machine(&self.ec.machine).map_err(proto)?;
        for p in &mut self.pins {
            p.gpio = if p.spec.direction == PinDirection::Adc {
                client.get_adc(md, &p.spec.gpio).map_err(proto)?
            } else {
                client.get_gpio(md, &p.spec.gpio).map_err(proto)?
            };
        }
        if self.sysbus.is_some() {
            self.sysbus = Some(client.get_sysbus(md, "sysbus").map_err(proto)?);
        }
        client.restore_events(pending_events);
        self.machine = md;
        self.client = client;
        self.ec.reconnects += 1;
        Ok(())
    }

    fn set_input(&mut self, i: usize, value: InputValue) -> Result<(), EngineError> {
        let p = &mut self.pins[i];
        match value {
            InputValue::Level(level) => self
                .client
                .gpio_set_state(p.gpio, p.spec.index, level == Level::High)
                .map_err(proto)?,
            InputValue::Microvolts(uv) => {
                self.client
                    .adc_set_value(p.gpio, p.spec.index, uv)
                    .map_err(proto)?;
                p.applied_uv = Some(uv);
            }
        }
        self.input_applied = true;
        // The input may wake the CPU (EXTI -> NVIC): a cached lookahead
        // snapshot no longer describes Renode.
        self.la.cached = None;
        Ok(())
    }

    fn advance_inner(&mut self, t: Time) -> Result<(), EngineError> {
        let start_ns = self.renode_ns;
        let target_ns = t.as_fs() / FS_PER_NS;
        // Events carried over from the previous grant end come first: they
        // are stamped `start_ns` (lattice rule 3).
        let mut events = std::mem::take(&mut self.carried);
        // Timed ADC inputs go to the model before Renode runs (ADR-005 D5).
        self.flush_adc_batch()?;
        // Split the advance at each queued input's lattice point and drive
        // the input while Renode is paused there (ADR-003 D2: never early).
        // Every segment ends on the lattice at or before `t`.
        while let Some(at_ns) = self.pending.first().map(|p| p.at_ns) {
            if at_ns > target_ns {
                break;
            }
            self.run_to(at_ns, &mut events)?;
            while self.pending.first().is_some_and(|p| p.at_ns == at_ns) {
                let p = self.pending.remove(0);
                self.set_input(p.pin, p.value)?;
            }
        }
        self.run_to(target_ns, &mut events)?;
        if let Some(Fault::DropGpioEvent { at_ns }) = self.fault {
            if let Some(k) = events.iter().position(|e| e.time_ns >= at_ns) {
                events.remove(k);
                self.fault = None;
            }
        }
        let check = match self.verify {
            Verify::Every => true,
            Verify::Edges => !events.is_empty() || self.input_applied,
            Verify::Final => false,
        };
        self.input_applied = false;
        self.renode_ns = target_ns;
        if check {
            self.verify_time()?;
        }
        self.trace.clear();
        // Per pin: the time of its last change in this grant.
        let mut last_change: Vec<Option<Time>> = vec![None; self.pins.len()];
        for ev in events {
            let i = usize::try_from(ev.descriptor)
                .ok()
                .filter(|i| *i < self.pins.len())
                .ok_or_else(|| {
                    EngineError::Protocol(format!("GPIO event with unknown descriptor {ev:?}"))
                })?;
            if ev.time_ns < start_ns || ev.time_ns > target_ns {
                return Err(EngineError::Protocol(format!(
                    "{}: edge at {} ns reported outside the grant [{start_ns}, {target_ns}] ns",
                    self.pins[i].spec.name, ev.time_ns
                )));
            }
            // Lattice rule 3: a change stamped exactly at the grant end
            // happened after the re-sync there, so it belongs to the next
            // grant. Renode already shows it; the kernel sees it next grant.
            if ev.time_ns == target_ns {
                self.carried.push(ev);
                self.carried_edges += 1;
                continue;
            }
            let pin = &mut self.pins[i];
            let level = Level::from_bool(ev.state);
            if level == pin.level {
                continue; // not a change (e.g. re-write of the same value)
            }
            // Lattice rules 1 and 2. A stamp at `start_ns` (the previous
            // boundary, a lattice point) becomes `now + 1 fs`, inside this
            // grant.
            let edge_time = lattice_edge_time(ev.time_ns, self.quantum_ns);
            if edge_time.as_fs() % (self.quantum_ns * FS_PER_NS) == 1 {
                self.boundary_edges += 1;
            }
            // Two changes stamped in one slice would share a time: Renode's
            // stamps cannot order them (ADR-003 D2/D3).
            if last_change[i].is_some_and(|t| edge_time <= t) {
                return Err(EngineError::Protocol(format!(
                    "{}: two changes stamped in the same Renode slice ({} ns); the edge-stamp \
                     resolution is the quantum, lower --renode-quantum",
                    pin.spec.name, ev.time_ns
                )));
            }
            last_change[i] = Some(edge_time);
            pin.level = level;
            pin.edge_time = edge_time;
            self.trace.push(Transition {
                t: edge_time,
                pin: pin.spec.name.clone(),
                signal: PinSignal::digital(level, edge_time),
            });
        }
        self.now = t;
        if check {
            self.verify_pins()?;
            self.verifications += 1;
        }
        Ok(())
    }

    fn apply_inner(&mut self, inputs: &PinSignals) -> Result<(), EngineError> {
        for (name, sig) in inputs {
            // A snapshot: a digital value takes effect at its edge_time, an
            // ADC value (which carries no time) at once.
            let t = match sig {
                PinSignal::Digital { edge_time, .. } => *edge_time,
                PinSignal::Analog { .. } => self.now,
            };
            self.apply_one(name, sig, t)?;
        }
        Ok(())
    }

    /// Apply one input value that holds from `t` on.
    fn apply_one(&mut self, name: &str, sig: &PinSignal, t: Time) -> Result<(), EngineError> {
        let i = self
            .pin_index(name)
            .filter(|i| {
                matches!(
                    self.pins[*i].spec.direction,
                    PinDirection::Input | PinDirection::Adc
                )
            })
            .ok_or_else(|| {
                EngineError::Protocol(format!("{} has no input pin {name:?}", self.name))
            })?;
        let value = match (self.pins[i].spec.direction, sig) {
            (PinDirection::Input, PinSignal::Digital { level, .. }) => {
                if self.pins[i].scheduled == Some(*level) {
                    return Ok(()); // a snapshot already applied or queued: no-op
                }
                self.pins[i].scheduled = Some(*level);
                InputValue::Level(*level)
            }
            (PinDirection::Adc, PinSignal::Analog { volts }) => {
                // The producer delivers whole microvolts in [0, vref]
                // (ADR-003 D6); SetValue takes them as a u32.
                if !volts.is_finite() || *volts < 0.0 || *volts > f64::from(u32::MAX) / 1e6 {
                    return Err(EngineError::Protocol(format!(
                        "{}.{name}: ADC value {volts} V is out of range",
                        self.name
                    )));
                }
                let uv = (volts * 1e6).round() as u32;
                if self.pins[i].scheduled_uv == Some(uv) {
                    return Ok(());
                }
                self.pins[i].scheduled_uv = Some(uv);
                if self.pins[i].timed {
                    // A timed input from `t` on (a lattice point, or `now`
                    // for a snapshot): samples strictly after it read it.
                    let at = t.max(self.now);
                    let t_ns = at.as_fs().div_ceil(FS_PER_NS);
                    self.adc_batch.push((t_ns, i, uv));
                    self.input_applied = true;
                    return Ok(());
                }
                InputValue::Microvolts(uv)
            }
            (PinDirection::Input, PinSignal::Analog { .. }) => {
                return Err(EngineError::Protocol(format!(
                    "{}.{name}: an analog value on a digital input needs a DigitalIn \
                     threshold in the producer",
                    self.name
                )))
            }
            _ => {
                return Err(EngineError::Protocol(format!(
                    "{}.{name}: an ADC input takes analog values (adc_in), got {sig:?}",
                    self.name
                )))
            }
        };
        // ADR-003 D2: Renode observes an input at its next sync point, so a
        // transition at `t` takes effect at ceil_L(t), never earlier. One
        // already in the past (delivered at a boundary) takes effect now.
        let at_ns = ceil_lattice_ns(t.max(self.now), self.quantum_ns);
        if at_ns <= self.renode_ns {
            self.set_input(i, value)?;
        } else {
            let k = self.pending.partition_point(|p| p.at_ns <= at_ns);
            self.pending.insert(
                k,
                PendingInput {
                    at_ns,
                    pin: i,
                    value,
                },
            );
        }
        Ok(())
    }

    /// Write the queued timed ADC inputs to the model's host port: one
    /// batch (a header and three words per entry), in `SYSTEM_BUS` writes of
    /// at most [`PORT_BATCH_WORDS`] words. Changes nothing in Renode but the
    /// model's input lists (ADR-005 D5.1).
    fn flush_adc_batch(&mut self) -> Result<(), EngineError> {
        if self.adc_batch.is_empty() {
            return Ok(());
        }
        let batch = std::mem::take(&mut self.adc_batch);
        for chunk in batch.chunks(0xFFFF) {
            let mut words = Vec::with_capacity(1 + 3 * chunk.len());
            words.push(0xBA7C_0000 | chunk.len() as u32);
            for &(t_ns, i, uv) in chunk {
                let ch = u32::try_from(self.pins[i].spec.index).unwrap_or(u32::MAX) & 0xFF;
                if t_ns >> 48 != 0 {
                    return Err(EngineError::Protocol(format!(
                        "timed ADC input at {t_ns} ns: beyond the host port's 48-bit time"
                    )));
                }
                words.push(t_ns as u32);
                words.push(((t_ns >> 32) as u32) | (ch << 16));
                words.push(uv);
            }
            let id = self.sysbus_id()?;
            for w in words.chunks(PORT_BATCH_WORDS) {
                self.client
                    .sysbus_write_u32s(id, BB_ADC_PORT_ADDRESS + PORT_BATCH, w)
                    .map_err(proto)?;
            }
        }
        for &(_, i, uv) in &batch {
            self.pins[i].applied_uv = Some(uv);
        }
        Ok(())
    }

    /// ADR-003 D3: every transition of the trace at its own time (digital:
    /// its `edge_time`, which equals `t`; ADC: `t`, a lattice point), so an
    /// ADC push inside the coming grant is set while Renode is paused on its
    /// lattice point (D6). The default would apply a snapshot, and an
    /// `Analog` value in a snapshot carries no time.
    fn apply_trace_inner(
        &mut self,
        base: &PinSignals,
        trace: &[Transition],
    ) -> Result<(), EngineError> {
        self.apply_inner(base)?;
        for tr in trace {
            self.apply_one(&tr.pin, &tr.signal, tr.t)?;
        }
        Ok(())
    }
}

/// Lattice +1 fs rule (ADR-003 D2) for a Renode stamp `stamp_ns` and lattice
/// `quantum_ns`: a stamp on a lattice point is reported 1 fs after it, any
/// other stamp (a timer-driven sync point) exactly.
fn lattice_edge_time(stamp_ns: u64, quantum_ns: u64) -> Time {
    let t = Time::from_ns(stamp_ns);
    if stamp_ns.is_multiple_of(quantum_ns) {
        t + Time::from_fs(1)
    } else {
        t
    }
}

/// `ceil_L(t)` in ns: the first lattice point at or after `t`.
fn ceil_lattice_ns(t: Time, quantum_ns: u64) -> u64 {
    let l = quantum_ns * FS_PER_NS;
    t.as_fs().div_ceil(l) * quantum_ns
}

impl Drop for RenodeEngine {
    fn drop(&mut self) {
        match &mut self.host {
            Host::Child(c) => {
                // A clean run that gathers the startup JIT profile stops
                // Renode gracefully so the runtime writes it (ADR-004
                // Amendment 6); every other child is killed.
                let gather = self.jit.as_ref().is_some_and(|j| j.gather.is_some());
                if gather && !self.failed && !std::thread::panicking() {
                    self.client.close();
                    terminate_gracefully(c, Duration::from_secs(5));
                    if let Some(j) = &self.jit {
                        j.publish();
                    }
                } else {
                    let _ = c.kill();
                    let _ = c.wait();
                }
            }
            Host::Lease(l) => {
                // Close External Control first: the manager clears the
                // process only after Renode has seen the disconnect
                // (ADR-004 Amendment 2). A failed engine or a panic
                // discards the process.
                self.client.close();
                let ok = !self.failed && !std::thread::panicking();
                if let Err(e) = l.end(ok) {
                    eprintln!("{}: {e}", self.name);
                }
            }
        }
        if self.failed {
            eprintln!(
                "{}: Renode workdir kept for diagnosis: {}",
                self.name,
                self.workdir.display()
            );
        } else {
            let _ = std::fs::remove_dir_all(&self.workdir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stm32_pin_names() {
        let p = PinSpec::stm32("PA5", PinDirection::Output).unwrap();
        assert_eq!((p.gpio.as_str(), p.index), ("gpioPortA", 5));
        let p = PinSpec::stm32("PC13", PinDirection::Input).unwrap();
        assert_eq!((p.gpio.as_str(), p.index), ("gpioPortC", 13));
        assert!(PinSpec::stm32("PA16", PinDirection::Output).is_err());
        assert!(PinSpec::stm32("A5", PinDirection::Output).is_err());
    }

    #[test]
    fn seconds_string_is_exact() {
        assert_eq!(ns_to_seconds_str(10_000), "0.000010000");
        assert_eq!(ns_to_seconds_str(1_500_000_001), "1.500000001");
    }

    #[test]
    fn lattice_rule_and_ceil() {
        // Lattice 10 us: a stamp on a lattice point is reported +1 fs.
        assert_eq!(
            lattice_edge_time(50_000_000, 10_000),
            Time::from_ms(50) + Time::from_fs(1)
        );
        // A timer-driven sync off the lattice is exact.
        assert_eq!(
            lattice_edge_time(299_999_930, 10_000),
            Time::from_ns(299_999_930)
        );
        assert_eq!(ceil_lattice_ns(Time::from_ps(123_456_789), 10_000), 130_000);
        assert_eq!(ceil_lattice_ns(Time::from_us(120), 10_000), 120_000);
        assert_eq!(
            ceil_lattice_ns(Time::from_us(120) + Time::from_fs(1), 10_000),
            130_000
        );
        assert_eq!(ceil_lattice_ns(Time::ZERO, 10_000), 0);
    }

    #[test]
    fn lookahead_promise_needs_every_condition() {
        let quiet = LookaheadStatus::ARMED
            | LookaheadStatus::SELF_TEST_OK
            | LookaheadStatus::IN_WFI
            | LookaheadStatus::HAS_LIMIT;
        let words = |status: u32, limit: u64, clock: u64| {
            vec![
                status,
                limit as u32,
                (limit >> 32) as u32,
                clock as u32,
                (clock >> 32) as u32,
                7,
                u32::MAX,
                u32::MAX,
            ]
        };
        let st = LookaheadStatus::from_words(50_000, &words(quiet, 49_950_000, 50_000)).unwrap();
        assert_eq!(st.promise_ns(), Some(50_000_000));
        assert_eq!((st.wfi_exits, st.first_exit_ns), (7, None));
        for veto in [
            LookaheadStatus::NVIC_PENDING,
            LookaheadStatus::ACTION_QUEUED,
            LookaheadStatus::HALTED,
        ] {
            let st = LookaheadStatus::from_words(50_000, &words(quiet | veto, 10, 50_000)).unwrap();
            assert_eq!(st.promise_ns(), None, "veto {veto:#x}");
        }
        for need in [
            LookaheadStatus::ARMED,
            LookaheadStatus::SELF_TEST_OK,
            LookaheadStatus::IN_WFI,
            LookaheadStatus::HAS_LIMIT,
        ] {
            let st =
                LookaheadStatus::from_words(50_000, &words(quiet & !need, 10, 50_000)).unwrap();
            assert_eq!(st.promise_ns(), None, "missing {need:#x}");
        }
        // The clock source must agree with Renode's time.
        let st = LookaheadStatus::from_words(50_000, &words(quiet, 10, 49_999)).unwrap();
        assert_eq!(st.promise_ns(), None);
        // No limit.
        let st = LookaheadStatus::from_words(0, &words(quiet, u64::MAX, 0)).unwrap();
        assert_eq!(st.promise_ns(), None);
        // A latched exit and the self-test reasons.
        let mut w = words(LookaheadStatus::ARMED | (0x20 << 16), 0, 0);
        w[6] = 123;
        w[7] = 0;
        let st = LookaheadStatus::from_words(0, &w).unwrap();
        assert_eq!(
            (st.self_test_failures(), st.first_exit_ns),
            (0x20, Some(123))
        );
        assert!(LookaheadStatus::from_words(0, &w[..7]).is_err());
    }

    /// ADR-008 D6 (M1): a promise past `Time`'s range is clamped, never a
    /// "Time overflow" panic; promises in range are exact.
    #[test]
    fn promise_is_clamped_to_time_range() {
        assert_eq!(MAX_PROMISE_NS, 18_446_744_073_709);
        let clamped = Time::from_ns(MAX_PROMISE_NS);
        assert_eq!(promise_time(u64::MAX), clamped);
        assert_eq!(promise_time(MAX_PROMISE_NS + 1), clamped);
        assert_eq!(promise_time(MAX_PROMISE_NS), clamped);
        assert!(Time::MAX.as_fs() - clamped.as_fs() < FS_PER_NS);
        assert_eq!(promise_time(50_000_000), Time::from_ms(50));
        assert_eq!(promise_time(0), Time::ZERO);
        // A snapshot whose clock limit is past Time's range (an nRF TIMER
        // after its compare counts on without wrapping): the promise is
        // valid in ns and clamped as a Time.
        let quiet = LookaheadStatus::ARMED
            | LookaheadStatus::SELF_TEST_OK
            | LookaheadStatus::IN_WFI
            | LookaheadStatus::HAS_LIMIT;
        let limit: u64 = 20_000_000_000_000; // 20 000 s
        let clock: u64 = 1_000_000;
        let w = [
            quiet,
            limit as u32,
            (limit >> 32) as u32,
            clock as u32,
            (clock >> 32) as u32,
            0,
            u32::MAX,
            u32::MAX,
        ];
        let st = LookaheadStatus::from_words(clock, &w).unwrap();
        assert_eq!(st.promise_ns(), Some(limit + clock));
        assert_eq!(promise_time(st.promise_ns().unwrap()), clamped);
    }

    #[test]
    fn plugin_cache_key_depends_on_source_and_renode() {
        let a = fnv1a64(&[b"src", b"/opt/renode-1.17.0/renode"]);
        assert_eq!(a, fnv1a64(&[b"src", b"/opt/renode-1.17.0/renode"]));
        assert_ne!(a, fnv1a64(&[b"src2", b"/opt/renode-1.17.0/renode"]));
        assert_ne!(a, fnv1a64(&[b"src", b"/opt/renode-1.18.0/renode"]));
        assert_ne!(fnv1a64(&[b"ab", b"c"]), fnv1a64(&[b"a", b"bc"]));
    }

    #[test]
    fn monitor_paths_reject_separators() {
        assert!(monitor_path(Path::new("/a b/c.elf")).is_err());
        assert!(monitor_path(Path::new("/a;b")).is_err());
        assert_eq!(monitor_path(Path::new("/a/b.elf")).unwrap(), "/a/b.elf");
    }
}
