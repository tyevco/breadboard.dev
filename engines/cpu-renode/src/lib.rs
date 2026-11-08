//! CPU engine backed by a headless Renode process driven through its
//! External Control API.
//!
//! # How time is controlled
//!
//! Renode is started paused (`--disable-xwt --port -1`, no monitor, no GUI)
//! with a generated script that creates the machine, loads the platform and
//! the ELF, and opens an External Control server
//! (`emulation CreateExternalControlServer`). The engine then only ever sends
//! `RUN_FOR(n ns)` with `n = floor(g) - floor(now)` for each kernel grant `g`
//! (times floored to Renode's 1 ns resolution). `RUN_FOR` returns once Renode
//! has paused again, so Renode never runs between grants and its virtual time
//! never passes the grant. [`RenodeConfig::verify`] cross-checks Renode's
//! virtual time (`GET_TIME`) and the output pin states (`GPIO GET_STATE`)
//! against what the engine tracked, and a mismatch is a protocol error. The
//! checks are pure reads, so they never change results; [`Verify`] picks how
//! often they run (ADR-003 D1.2).
//!
//! The emulation always runs with `emulation SetGlobalAdvanceImmediately
//! true` (ADR-003 D1.1). Without it Renode sleeps on the host while the CPU
//! is in WFI and then skips instructions by the host's measured sleep time
//! (`BaseCPU.cs`), which is a host-timing dependence. It is not optional.
//!
//! Known bad idea: never let Renode's virtual time run ahead of the kernel
//! grant; never `start` the emulation and try to catch up.
//!
//! # Edges
//!
//! Each output pin has a GPIO state-change callback registered over the
//! External Control API. Renode reports every change with its virtual
//! timestamp (ns), so an edge is reported to the kernel at the time it
//! happened inside the grant, not quantized to the grant boundary. More than
//! one edge on a pin inside one grant is a protocol error (the configured
//! `quantum`, which bounds every grant through `t_safe`, is too large).
//!
//! Measured Renode 1.17.0 behaviour (ADR-002 Amendment 3): when a timer
//! (here SysTick) expires close to the end of a `RUN_FOR`, Renode can defer
//! the ISR to the start of the *next* `RUN_FOR`; the resulting pin change is
//! then stamped with the pause time itself, i.e. the previous boundary. The
//! pin still read the old level at that boundary, so the edge belongs to the
//! new grant; the engine reports it 1 fs after the previous kernel boundary
//! and counts it in [`RenodeEngine::boundary_edges`]. Timestamps before the
//! previous boundary or after the grant are protocol errors.
//!
//! The firmware-visible time of such an edge is the boundary, not the timer
//! expiry: it is late by the distance from the expiry to the boundary (up to
//! ~50 ns measured, not bounded by Renode's 1 ns resolution). More generally
//! Renode's results depend on the `RUN_FOR` partition: edge times are exact
//! *for a given partition* and deterministic, but a different partition
//! (another quantum) can move them by tens of ns. The kernel therefore keeps
//! the partition a function of the quantum alone: output sampling does not
//! add boundaries (`CsvWriter::sampled`).
//!
//! # Determinism
//!
//! Renode is deterministic for a single machine given the same ELF, platform,
//! quantum and sequence of `RUN_FOR` calls. The engine fixes everything it
//! can: `emulation SetSeed`, `emulation SetGlobalQuantum` (= `quantum`),
//! `emulation SetGlobalSerialExecution true`,
//! `emulation SetGlobalAdvanceImmediately true`, an offline platform file (no SVD
//! download) and a fixed machine name. GPIO events are acknowledged inline on a
//! single connection, so they are seen in emulation order.

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
use bbcore::{Engine, EngineError, Level, PinSignal, PinSignals, Time};

use protocol::{Client, ProtocolError};

/// Name this engine reports through [`bbcore::Engine::name`] by default.
pub const ENGINE_NAME: &str = "renode";

/// Platform file written next to the generated script by default. See the
/// file itself for why the stock `platforms/cpus/stm32f103.repl` is wrapped.
pub const STM32F103_OFFLINE_REPL: &str = include_str!("../platforms/stm32f103-offline.repl");

/// Renode's virtual-time resolution.
pub const RENODE_RESOLUTION: Time = Time::from_ns(1);

/// Which platform description to load.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Platform {
    /// [`STM32F103_OFFLINE_REPL`]: stock STM32F103 without the SVD download.
    Stm32f103Offline,
    /// A path understood by Renode's `LoadPlatformDescription`, e.g. the stock
    /// `platforms/cpus/stm32f103.repl` (relative to the Renode install) or an
    /// absolute path.
    Repl(String),
}

/// Direction of an engine pin, seen from the MCU.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PinDirection {
    /// The MCU drives the pin; edges are reported to the kernel.
    Output,
    /// The kernel drives the pin into the MCU (`SET_STATE`).
    Input,
}

/// One MCU pin exposed to the kernel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinSpec {
    /// Pin name on the engine side, e.g. `PA5`.
    pub name: String,
    /// Renode GPIO controller name, e.g. `gpioPortA`.
    pub gpio: String,
    /// Pin index within the controller.
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
    /// Lookahead: `t_safe = now + quantum`, and Renode's global quantum.
    /// Must be a positive whole number of nanoseconds.
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
}

impl RenodeConfig {
    /// STM32F103 with PA5 as an output, quantum 10 us: the Phase 0 spike setup.
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
        }
    }
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
    /// Last level driven into an input pin.
    applied: Option<Level>,
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
    child: Child,
    workdir: PathBuf,
    client: Client,
    now: Time,
    renode_ns: u64,
    promised: Time,
    pins: Vec<PinState>,
    run_for_calls: u64,
    boundary_edges: u64,
    /// An advance or input failed: Drop keeps `workdir` (with `renode.log`)
    /// for diagnosis.
    failed: bool,
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

fn free_port() -> Result<u16, EngineError> {
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
        let firmware = std::fs::canonicalize(&cfg.firmware)
            .map_err(|e| EngineError::Other(format!("firmware {}: {e}", cfg.firmware.display())))?;

        let workdir = std::env::temp_dir().join(format!(
            "bb-renode-{}-{}",
            std::process::id(),
            INSTANCE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&workdir);
        std::fs::create_dir_all(&workdir)?;
        let platform = match &cfg.platform {
            Platform::Stm32f103Offline => {
                let p = workdir.join("platform.repl");
                std::fs::write(&p, STM32F103_OFFLINE_REPL)?;
                monitor_path(&p)?
            }
            Platform::Repl(p) => monitor_path(Path::new(p))?,
        };
        let port = free_port()?;
        let script = format!(
            "mach create \"{machine}\"\n\
             machine LoadPlatformDescription @{platform}\n\
             sysbus LoadELF @{elf}\n\
             emulation SetSeed {seed}\n\
             emulation SetGlobalQuantum \"{quantum}\"\n\
             emulation SetGlobalSerialExecution true\n\
             emulation SetGlobalAdvanceImmediately true\n\
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
        let log = std::fs::File::create(&log_path)?;

        let child = Command::new(&cfg.renode)
            .args(["--disable-xwt", "--plain", "--port", "-1"])
            .arg(&script_path)
            .current_dir(&workdir)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .spawn()
            .map_err(|e| EngineError::Other(format!("spawn {}: {e}", cfg.renode.display())))?;

        let mut engine_child = Some(child);
        let fail = |child: &mut Option<Child>, msg: String| -> EngineError {
            if let Some(mut c) = child.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
            let tail = log_tail(&log_path);
            let _ = std::fs::remove_dir_all(&workdir);
            EngineError::Other(format!("{msg}\n--- renode log (tail) ---\n{tail}"))
        };

        // Wait for the External Control server.
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let deadline = Instant::now() + cfg.startup_timeout;
        let stream = loop {
            if let Some(c) = engine_child.as_mut() {
                if let Ok(Some(status)) = c.try_wait() {
                    return Err(fail(
                        &mut engine_child,
                        format!("renode exited during startup ({status})"),
                    ));
                }
            }
            match TcpStream::connect_timeout(&addr, Duration::from_millis(500)) {
                Ok(s) => break s,
                Err(_) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(100))
                }
                Err(e) => {
                    return Err(fail(
                        &mut engine_child,
                        format!("no External Control server on port {port}: {e}"),
                    ))
                }
            }
        };

        let setup = || -> Result<(Client, i32, Vec<PinState>, u64), ProtocolError> {
            let mut client = Client::handshake(stream, cfg.io_timeout)?;
            let md = client.get_machine(&cfg.machine)?;
            let mut pins = Vec::new();
            for (i, spec) in cfg.pins.iter().enumerate() {
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
                    applied: None,
                });
            }
            let t0 = client.get_time()?;
            Ok((client, md, pins, t0))
        };
        let (client, machine, pins, t0) = match setup() {
            Ok(v) => v,
            Err(e) => return Err(fail(&mut engine_child, format!("setup failed: {e}"))),
        };
        if t0 != 0 {
            return Err(fail(
                &mut engine_child,
                format!("Renode virtual time is {t0} ns at start, expected 0"),
            ));
        }
        Ok(RenodeEngine {
            name: cfg.name.clone(),
            quantum_ns,
            verify: cfg.verify,
            input_applied: false,
            verifications: 0,
            fault: None,
            machine,
            sysbus: None,
            child: engine_child.take().expect("child present"),
            workdir,
            client,
            now: Time::ZERO,
            renode_ns: 0,
            promised: Time::ZERO,
            pins,
            run_for_calls: 0,
            boundary_edges: 0,
            failed: false,
        })
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

    /// Number of `RUN_FOR` requests sent so far.
    pub fn run_for_calls(&self) -> u64 {
        self.run_for_calls
    }

    /// Edges Renode stamped exactly at the previous pause point, reported
    /// 1 fs after that boundary instead (see `advance_to`).
    pub fn boundary_edges(&self) -> u64 {
        self.boundary_edges
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

    /// Mark the engine failed and attach the Renode log tail (and where the
    /// full log is kept) to an error from the running phase.
    fn failure(&mut self, e: EngineError) -> EngineError {
        self.failed = true;
        let exited = match self.child.try_wait() {
            Ok(Some(status)) => format!("renode exited ({status}); "),
            _ => String::new(),
        };
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
        // Phase 0 lookahead: a fixed quantum past Renode's current time. This
        // is not a promise that outputs stay constant: the firmware may change
        // an output at any instruction. It bounds the grant so at most one
        // edge per pin falls inside it; the edge carries its time within this
        // partition, and consumers registered after this engine apply it at
        // that time (ADR-002 Amendment 3).
        self.promised = Time::from_ns(self.renode_ns + self.quantum_ns);
        self.promised
    }

    fn advance_to(&mut self, t: Time) -> Result<(), EngineError> {
        // A grant outside the promise is the caller's error; Renode was not
        // touched, so there is no log worth keeping.
        if t <= self.now || t > self.promised {
            return Err(EngineError::Protocol(format!(
                "advance_to({t}) outside ({}, {}]",
                self.now, self.promised
            )));
        }
        self.advance_inner(t).map_err(|e| self.failure(e))
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

    /// The end-of-run cross-check of [`Verify::Edges`] and [`Verify::Final`]
    /// (with [`Verify::Every`] the last advance has just been checked).
    fn end_of_run(&mut self) -> Result<(), EngineError> {
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

    /// `GPIO GET_STATE` of every output pin must equal the tracked level.
    fn verify_pins(&mut self) -> Result<(), EngineError> {
        for p in self
            .pins
            .iter()
            .filter(|p| p.spec.direction == PinDirection::Output)
        {
            let s = self
                .client
                .gpio_get_state(p.gpio, p.spec.index)
                .map_err(proto)?;
            if Level::from_bool(s) != p.level {
                return Err(EngineError::Protocol(format!(
                    "verify: {}: Renode reports {s} at {} ns but events say {:?}",
                    p.spec.name, self.renode_ns, p.level
                )));
            }
        }
        Ok(())
    }

    fn advance_inner(&mut self, t: Time) -> Result<(), EngineError> {
        let start_ns = self.renode_ns;
        let target_ns = t.as_fs() / FS_PER_NS;
        if target_ns > start_ns {
            let mut ns = target_ns - start_ns;
            if let Some(Fault::RunForExtraNs { at, ns: extra }) = self.fault {
                if t >= at {
                    ns += extra;
                    self.fault = None;
                }
            }
            self.client.run_for(ns).map_err(proto)?;
            self.run_for_calls += 1;
        }
        let mut events = self.client.take_events();
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
        let mut edges = vec![0u32; self.pins.len()];
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
            let pin = &mut self.pins[i];
            let level = Level::from_bool(ev.state);
            if level == pin.level {
                continue; // not a change (e.g. re-write of the same value)
            }
            // Boundary edge: Renode stamps a change made by an ISR it deferred
            // past the previous pause point with that pause time, although it
            // executed during this grant (the pin read back the old level at
            // that boundary). It is reported 1 fs after the previous kernel
            // boundary, the earliest time inside this grant. That is when the
            // firmware really changed the pin in this partition; the timer
            // expiry it reacts to may have been tens of ns earlier (see the
            // crate docs). Counted in `boundary_edges()`.
            let mut edge_time = Time::from_ns(ev.time_ns);
            if edge_time <= self.now {
                edge_time = self.now + Time::from_fs(1);
                self.boundary_edges += 1;
            }
            edges[i] += 1;
            if edges[i] > 1 {
                return Err(EngineError::Protocol(format!(
                    "{}: more than one edge inside grant ({start_ns}, {target_ns}] ns; \
                     reduce the engine quantum",
                    pin.spec.name
                )));
            }
            pin.level = level;
            pin.edge_time = edge_time;
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
            let i = self
                .pin_index(name)
                .filter(|i| self.pins[*i].spec.direction == PinDirection::Input)
                .ok_or_else(|| {
                    EngineError::Protocol(format!("{} has no input pin {name:?}", self.name))
                })?;
            let level = sig.level().ok_or_else(|| {
                EngineError::Protocol(format!(
                    "{}.{name}: analog input needs a DigitalIn threshold, not supported yet",
                    self.name
                ))
            })?;
            let p = &mut self.pins[i];
            if p.applied != Some(level) {
                self.client
                    .gpio_set_state(p.gpio, p.spec.index, level == Level::High)
                    .map_err(proto)?;
                p.applied = Some(level);
                self.input_applied = true;
            }
        }
        Ok(())
    }
}

impl Drop for RenodeEngine {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
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
    fn monitor_paths_reject_separators() {
        assert!(monitor_path(Path::new("/a b/c.elf")).is_err());
        assert!(monitor_path(Path::new("/a;b")).is_err());
        assert_eq!(monitor_path(Path::new("/a/b.elf")).unwrap(), "/a/b.elf");
    }
}
