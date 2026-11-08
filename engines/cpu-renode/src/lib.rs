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
//! never passes the grant. With [`RenodeConfig::verify`] (the default) every
//! advance is followed by `GET_TIME`, and a mismatch is a protocol error.
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
//! Measured Renode 1.17.0 behaviour: when a timer (here SysTick) expires
//! exactly at the point where `RUN_FOR` paused, the ISR runs at the start of
//! the *next* `RUN_FOR`, and the resulting pin change is stamped with the
//! pause time itself, i.e. the previous boundary. The pin still read the old
//! level at that boundary, so the edge belongs to the new grant; the engine
//! reports it 1 fs after the previous kernel boundary (an error of at most
//! 1 ns, Renode's resolution) and counts it in
//! [`RenodeEngine::boundary_edges`]. Timestamps before the previous boundary
//! or after the grant are protocol errors.
//!
//! # Determinism
//!
//! Renode is deterministic for a single machine given the same ELF, platform,
//! quantum and sequence of `RUN_FOR` calls. The engine fixes everything it
//! can: `emulation SetSeed`, `emulation SetGlobalQuantum` (= `quantum`),
//! `emulation SetGlobalSerialExecution true`, an offline platform file (no SVD
//! download) and a fixed machine name. GPIO events are acknowledged inline on a
//! single connection, so they are seen in emulation order.

pub mod protocol;

use std::io::Read;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
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
    /// After every advance, check Renode's virtual time and the output pin
    /// states against what the engine tracked. Costs round trips; does not
    /// change emulation results.
    pub verify: bool,
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
            verify: true,
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
    verify: bool,
    child: Child,
    workdir: PathBuf,
    client: Client,
    now: Time,
    renode_ns: u64,
    promised: Time,
    pins: Vec<PinState>,
    run_for_calls: u64,
    boundary_edges: u64,
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

fn free_port() -> Result<u16, EngineError> {
    let l = TcpListener::bind("127.0.0.1:0")?;
    Ok(l.local_addr()?.port())
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

        let setup = || -> Result<(Client, Vec<PinState>, u64), ProtocolError> {
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
            Ok((client, pins, t0))
        };
        let (client, pins, t0) = match setup() {
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
            child: engine_child.take().expect("child present"),
            workdir,
            client,
            now: Time::ZERO,
            renode_ns: 0,
            promised: Time::ZERO,
            pins,
            run_for_calls: 0,
            boundary_edges: 0,
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

    fn pin_index(&self, name: &str) -> Option<usize> {
        self.pins.iter().position(|p| p.spec.name == name)
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
        // Phase 0 lookahead: a fixed quantum past Renode's current time. The
        // firmware may change an output at any instruction, so this bounds how
        // late (at most one grant) the analog side learns of an edge; the edge
        // itself carries its exact time.
        self.promised = Time::from_ns(self.renode_ns + self.quantum_ns);
        self.promised
    }

    fn advance_to(&mut self, t: Time) -> Result<(), EngineError> {
        if t <= self.now || t > self.promised {
            return Err(EngineError::Protocol(format!(
                "advance_to({t}) outside ({}, {}]",
                self.now, self.promised
            )));
        }
        let start_ns = self.renode_ns;
        let target_ns = t.as_fs() / FS_PER_NS;
        if target_ns > start_ns {
            self.client.run_for(target_ns - start_ns).map_err(proto)?;
            self.run_for_calls += 1;
        }
        if self.verify {
            let vt = self.client.get_time().map_err(proto)?;
            if vt != target_ns {
                return Err(EngineError::Protocol(format!(
                    "Renode virtual time {vt} ns after RUN_FOR, expected {target_ns} ns (grant {t})"
                )));
            }
        }
        let mut edges = vec![0u32; self.pins.len()];
        for ev in self.client.take_events() {
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
            // Boundary edge: Renode stamps a change made by code that ran
            // because of a timer expiring exactly at the previous pause point
            // with that pause time, although it executed during this grant
            // (the pin read back the old level at that boundary). It is
            // reported 1 fs after the previous kernel boundary, the earliest
            // time inside this grant: an error of at most 1 ns, below Renode's
            // own resolution. Counted in `boundary_edges()`.
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
        if self.verify {
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
                        "{}: Renode reports {s} at {target_ns} ns but events say {:?}",
                        p.spec.name, p.level
                    )));
                }
            }
        }
        self.renode_ns = target_ns;
        self.now = t;
        Ok(())
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
            }
        }
        Ok(())
    }
}

impl Drop for RenodeEngine {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.workdir);
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
