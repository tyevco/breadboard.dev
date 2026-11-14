//! GDB passthrough (`bb run --gdb <port>`, ADR-008 D10 and Amendment 7).
//!
//! Renode 1.17.0's own GDB stub (`machine StartGdbServer <port> false`)
//! listens on every interface, takes one client, and leaves the engine's
//! time control exposed: a step while no `RUN_FOR` is running calls
//! `emulation.StartAll()` (`BaseCPU.Step`), which starts Renode's
//! free-running time loop; Ctrl-C pauses the CPU in the middle of a slice
//! and can leave a `RUN_FOR` open for ever; `monitor` reaches every Renode
//! command; and its stop replies race the resume packets (a step-wait `S05`
//! can arrive after a `c` and be taken as its reply). So the engine never
//! hands that port to a debugger. It starts the stub on an internal port,
//! connects to it itself right after startup (the stub's only client slot),
//! and runs this proxy between the stub and a debugger, listening on
//! `127.0.0.1` only.
//!
//! # What the proxy knows about Renode
//!
//! While the proxy is connected, Renode keeps the CPU in single-step mode
//! except after a `c`. A CPU in single-step mode executes nothing without a
//! step command: it waits (*parks*) at the top of its loop, usually sending
//! an `S05`. It parks at the start of a `RUN_FOR` (after the mode was set
//! between two `RUN_FOR`s), after a breakpoint (a `T05`, then an `S05`) and
//! after every step. A parked CPU holds the `RUN_FOR` open
//! (`SingleStepBlocking`, the default), so virtual time stops, unless it
//! parked after the last instruction of the `RUN_FOR`'s time: then the
//! `RUN_FOR` ends (an *end park*), and the CPU parks again at the start of
//! the next one, measured without an `S05`. A CPU asleep in WFI is halted:
//! time flows without it until it wakes. Renode's stub writes every packet
//! through one FIFO queue, so its stop replies and command replies arrive in
//! the order they were produced.
//!
//! # Rules
//!
//! - **R1, the resume barrier.** A `c` or `s` (from the debugger, or the
//!   proxy's own continue when no debugger holds the CPU) reaches Renode
//!   only when a state read says the CPU is parked inside a running
//!   `RUN_FOR` that cannot end without it: a Python monitor command reads
//!   the CPU's time handle (`SourceSideActive`: Renode's time source is
//!   running; `TotalElapsedTime`, written by the CPU thread before it
//!   parks) and `IsHalted`, and the resume is forwarded only if the source is
//!   active, the CPU is not halted and its time is before the `RUN_FOR`'s
//!   end. Otherwise the resume is held and the read repeated (after an
//!   `S05`, at the next `RUN_FOR`, or every 2 ms while a `RUN_FOR` is open).
//!   So a step never reaches Renode outside a running `RUN_FOR` (no
//!   `StartAll`, no free-run), and a `c` never resumes a paused CPU between
//!   two `RUN_FOR`s.
//! - **R2, stop replies.** A step's reply is the proxy's own `S05`, sent when
//!   Renode has finished the step (a `qC` sent after the `s` is answered only
//!   then: `Step` blocks Renode's receive thread); Renode's `S05`s are never
//!   forwarded. After a `c`, the debugger gets the first `T` (a breakpoint or
//!   watchpoint), or an `S02` once a requested halt has parked the CPU. The
//!   proxy answers `?` itself, once the CPU is parked.
//! - **R3, Ctrl-C and attach** never reach Renode as a pause. The proxy
//!   switches the CPU to single-step mode (`sysbus.cpu ExecutionMode
//!   SingleStep`) between two `RUN_FOR`s, so the CPU parks at the start of
//!   the next one: a slice boundary, which changes no result.
//! - **R4, detach** (`D`, `k`, or the connection closing) removes the
//!   debugger's breakpoints and watchpoints and continues the CPU; the proxy
//!   keeps Renode's connection, and another debugger may attach later.
//! - **R5, refused packets:** `monitor` (`qRcmd`), reverse execution (`bc`,
//!   `bs`), `vRun`, continue or step with an address or signal, and
//!   Renode's TRACE32 extensions. `vCont` is advertised (with
//!   `vContSupported+` in the `qSupported` reply) and mapped onto `c` and
//!   `s`: without it GDB 15 single-steps in software, a temporary breakpoint
//!   and a continue per step.
//!
//! # Debug marking
//!
//! A stop inside a slice ends Renode's time slice early, and that changes
//! later edge stamps (measured, Amendment 7). The proxy records each
//! operation that can change results: a breakpoint or watchpoint stop, a
//! step, inserting a breakpoint or watchpoint, a register or memory write,
//! and a read of the peripheral or system address range (a register read
//! can have side effects). The engine turns the first one into a `warn` log
//! record; attaching, halting at a slice boundary, reading registers and
//! RAM and detaching change nothing and leave no mark.

use std::collections::{BTreeSet, VecDeque};
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// `bb run --gdb`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GdbConfig {
    /// The port the proxy listens on, on `127.0.0.1` (0: any free port).
    pub port: u16,
    /// Hold the CPU at reset until a debugger has attached (and continued,
    /// or detached), like QEMU's `-S`. Otherwise the run starts at once and
    /// a debugger may attach at any time.
    pub wait: bool,
}

/// Environment variable naming a file the proxy writes every packet to (a
/// diagnostic; never an output of the run).
pub const GDB_TRACE_ENV: &str = "BB_GDB_TRACE";

/// What the proxy observed, for the run summary and the debug mark.
#[derive(Clone, Debug, Default)]
pub struct GdbStatus {
    /// Debugger connections accepted.
    pub sessions: u64,
    /// Operations that can change results ([`GdbProxy`], "Debug marking").
    pub perturbations: u64,
    /// The first of them.
    pub first: Option<String>,
}

/// The proxy, owned by the Renode engine.
pub struct GdbProxy {
    tx: Sender<Ev>,
    port: u16,
    status: Arc<Mutex<GdbStatus>>,
    brain: Option<JoinHandle<()>>,
    listener: Option<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
}

/// The monitor command that reads the CPU's state for R1:
/// `<elapsed ns> <source active> <halted>`.
const STATE_QUERY: &str = "python \"c = monitor.Machine.SystemBus.GetCPUs()[0]; \
     print('%d %s %s' % (c.TimeHandle.TotalElapsedTime.Ticks, c.TimeHandle.SourceSideActive, c.IsHalted))\"";
/// The monitor command that makes the CPU park at its next slice boundary.
const HALT_COMMAND: &str = "sysbus.cpu ExecutionMode SingleStep";
/// The ping: a read with a one-packet reply and no effect.
const PING: &[u8] = b"qC";
/// How often R1 reads the CPU's state again while a resume is held.
const RETRY: Duration = Duration::from_millis(2);

impl GdbProxy {
    /// Connect to Renode's stub on `127.0.0.1:stub_port` and listen for a
    /// debugger. Renode switches the CPU to single-step mode on this
    /// connection, so the CPU parks at the start of the first `RUN_FOR`.
    pub fn start(stub_port: u16, cfg: &GdbConfig, deadline: Instant) -> Result<GdbProxy, String> {
        let addr = SocketAddr::from(([127, 0, 0, 1], stub_port));
        let renode = loop {
            match TcpStream::connect_timeout(&addr, Duration::from_millis(500)) {
                Ok(s) if s.local_addr().ok() != s.peer_addr().ok() => break s,
                Ok(_) | Err(_) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Ok(_) => return Err(format!("GDB stub on port {stub_port}: self-connect")),
                Err(e) => return Err(format!("no GDB stub on port {stub_port}: {e}")),
            }
        };
        renode.set_nodelay(true).map_err(|e| e.to_string())?;
        let listener = TcpListener::bind(("127.0.0.1", cfg.port)).map_err(|e| {
            format!(
                "--gdb {}: cannot listen on 127.0.0.1:{}: {e}",
                cfg.port, cfg.port
            )
        })?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let trace = match std::env::var_os(GDB_TRACE_ENV) {
            Some(p) => Some(std::io::BufWriter::new(std::fs::File::create(&p).map_err(
                |e| {
                    format!(
                        "{GDB_TRACE_ENV}={}: {e}",
                        std::path::Path::new(&p).display()
                    )
                },
            )?)),
            None => None,
        };
        let (tx, rx) = mpsc::channel();
        let status = Arc::new(Mutex::new(GdbStatus::default()));
        let stop = Arc::new(AtomicBool::new(false));

        let r = renode.try_clone().map_err(|e| e.to_string())?;
        let txr = tx.clone();
        std::thread::Builder::new()
            .name("bb-gdb-renode".into())
            .spawn(move || read_packets(r, |e| txr.send(Ev::Renode(e)).is_ok()))
            .map_err(|e| e.to_string())?;

        let txl = tx.clone();
        let stopl = stop.clone();
        let listener = std::thread::Builder::new()
            .name("bb-gdb-listen".into())
            .spawn(move || {
                for s in listener.incoming() {
                    if stopl.load(Ordering::SeqCst) {
                        break;
                    }
                    if let Ok(s) = s {
                        if txl.send(Ev::Accept(s)).is_err() {
                            break;
                        }
                    }
                }
            })
            .map_err(|e| e.to_string())?;

        let brain = Brain {
            renode,
            renode_ok: true,
            tx: tx.clone(),
            expect: VecDeque::new(),
            client: None,
            next_client: 0,
            wait_for_first: cfg.wait,
            run_for: None,
            gen: 0,
            stepping: true,
            parked: false,
            held: None,
            checking: false,
            retry_at: None,
            halt: Halt::None,
            engine: None,
            perturbed: None,
            status: status.clone(),
            trace,
            t0: Instant::now(),
        };
        let brain = std::thread::Builder::new()
            .name("bb-gdb".into())
            .spawn(move || brain.run(rx))
            .map_err(|e| e.to_string())?;
        Ok(GdbProxy {
            tx,
            port,
            status,
            brain: Some(brain),
            listener: Some(listener),
            stop,
        })
    }

    /// The port the proxy listens on (`127.0.0.1`).
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The shared session summary.
    pub fn status(&self) -> Arc<Mutex<GdbStatus>> {
        self.status.clone()
    }

    /// Called right before a `RUN_FOR` that ends at `g_ns`. Applies a pending
    /// halt (Ctrl-C, attach) while Renode is paused.
    pub fn before_run_for(&self, g_ns: u64) -> Result<(), String> {
        let (reply, rx) = mpsc::sync_channel(1);
        self.tx
            .send(Ev::Before { g_ns, reply })
            .map_err(|_| "GDB proxy stopped".to_string())?;
        rx.recv().map_err(|_| "GDB proxy stopped".to_string())?
    }

    /// Called right after a `RUN_FOR` returned. Returns the first operation
    /// that can change results since the last call, if any.
    pub fn after_run_for(&self) -> Result<Option<String>, String> {
        let (reply, rx) = mpsc::sync_channel(1);
        self.tx
            .send(Ev::After { reply })
            .map_err(|_| "GDB proxy stopped".to_string())?;
        rx.recv().map_err(|_| "GDB proxy stopped".to_string())?
    }

    /// End the session: a debugger waiting for a stop reply gets `W00`
    /// (the run is over), and every connection is closed.
    pub fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.tx.send(Ev::Shutdown);
        if let Some(b) = self.brain.take() {
            let _ = b.join();
        }
        // Unblock the accept loop.
        let _ = TcpStream::connect_timeout(
            &SocketAddr::from(([127, 0, 0, 1], self.port)),
            Duration::from_millis(200),
        );
        if let Some(l) = self.listener.take() {
            let _ = l.join();
        }
    }
}

impl Drop for GdbProxy {
    fn drop(&mut self) {
        self.shutdown();
    }
}

enum Ev {
    Renode(Wire),
    Accept(TcpStream),
    Client(u64, Wire),
    Before {
        g_ns: u64,
        reply: SyncSender<Result<(), String>>,
    },
    After {
        reply: SyncSender<Result<Option<String>, String>>,
    },
    Shutdown,
}

/// One item read from a GDB remote-protocol stream.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Wire {
    Packet(Vec<u8>),
    /// The interrupt byte (0x03, Ctrl-C).
    Interrupt,
    Eof,
}

/// Read packets until EOF; `send` returns false to stop.
fn read_packets(mut s: TcpStream, mut send: impl FnMut(Wire) -> bool) {
    let mut parser = Parser::default();
    let mut buf = [0u8; 4096];
    loop {
        let n = match s.read(&mut buf) {
            Ok(0) | Err(_) => {
                send(Wire::Eof);
                return;
            }
            Ok(n) => n,
        };
        for w in parser.feed(&buf[..n]) {
            if !send(w) {
                return;
            }
        }
    }
}

/// GDB remote serial protocol framing (acks are dropped; checksums are not
/// checked: TCP is reliable).
#[derive(Default)]
struct Parser {
    state: PState,
    payload: Vec<u8>,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum PState {
    #[default]
    Idle,
    Body,
    Sum(u8),
}

impl Parser {
    fn feed(&mut self, bytes: &[u8]) -> Vec<Wire> {
        let mut out = Vec::new();
        for &b in bytes {
            match self.state {
                PState::Idle => match b {
                    b'$' => {
                        self.payload.clear();
                        self.state = PState::Body;
                    }
                    0x03 => out.push(Wire::Interrupt),
                    _ => {} // '+', '-', noise
                },
                PState::Body => {
                    if b == b'#' {
                        self.state = PState::Sum(0);
                    } else {
                        self.payload.push(b);
                    }
                }
                PState::Sum(k) => {
                    if k == 0 {
                        self.state = PState::Sum(1);
                    } else {
                        self.state = PState::Idle;
                        out.push(Wire::Packet(std::mem::take(&mut self.payload)));
                    }
                }
            }
        }
        out
    }
}

/// `$payload#cs`.
fn frame(payload: &[u8]) -> Vec<u8> {
    let sum = payload.iter().fold(0u8, |a, b| a.wrapping_add(*b));
    let mut v = Vec::with_capacity(payload.len() + 4);
    v.push(b'$');
    v.extend_from_slice(payload);
    v.extend_from_slice(format!("#{sum:02x}").as_bytes());
    v
}

/// Expand run-length encoding (`x*n`: `n - 29` more copies of `x`).
fn unrle(p: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(p.len());
    let mut i = 0;
    while i < p.len() {
        if p[i] == b'*' && i + 1 < p.len() {
            if let Some(&last) = out.last() {
                for _ in 0..(p[i + 1] as usize).saturating_sub(29) {
                    out.push(last);
                }
            }
            i += 2;
        } else {
            out.push(p[i]);
            i += 1;
        }
    }
    out
}

fn hex_decode(s: &[u8]) -> Vec<u8> {
    s.chunks(2)
        .filter_map(|c| std::str::from_utf8(c).ok())
        .filter_map(|c| u8::from_str_radix(c, 16).ok())
        .collect()
}

fn hex_encode(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

/// A stop reply from the stub (`S`, `T`, `W`, `X`): asynchronous. No reply
/// to a packet the proxy forwards starts with one of these (`?` is answered
/// by the proxy itself).
fn is_stop(p: &[u8]) -> bool {
    matches!(p.first(), Some(b'S' | b'T' | b'W' | b'X'))
}

/// The address ranges whose reads go to peripheral or system registers on
/// the Cortex-M targets (peripherals, Breadboard's host ports, the PPB).
fn touches_registers(addr: u64, len: u64) -> bool {
    let end = addr.saturating_add(len.max(1));
    let ranges = [
        (0x4000_0000u64, 0x6000_0000u64),
        (0xE000_0000, 0x1_0000_0000),
    ];
    ranges.iter().any(|&(a, b)| addr < b && end > a)
}

/// Renode's `qSupported` reply as the proxy offers it: without the reverse
/// execution it refuses (R5), and with `vContSupported+`. Without that
/// feature GDB must assume the target cannot single-step, and GDB 15 then
/// steps in software (a temporary breakpoint and a continue per step).
fn supported_reply(p: &[u8]) -> Vec<u8> {
    let s = String::from_utf8_lossy(p);
    let mut f: Vec<&str> = s
        .split(';')
        .filter(|x| !x.is_empty() && !x.starts_with("Reverse") && !x.starts_with("vContSupported"))
        .collect();
    f.push("vContSupported+");
    f.join(";").into_bytes()
}

/// The CPU state R1 reads: elapsed ns, time source running, halted (WFI).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CpuState {
    t_ns: u64,
    active: bool,
    halted: bool,
}

fn parse_state(out: &[u8]) -> Option<CpuState> {
    let s = String::from_utf8_lossy(out);
    let mut w = s.split_whitespace();
    let t_ns = w.next()?.parse().ok()?;
    let b = |x: Option<&str>| match x? {
        "True" => Some(true),
        "False" => Some(false),
        _ => None,
    };
    Some(CpuState {
        t_ns,
        active: b(w.next())?,
        halted: b(w.next())?,
    })
}

/// R1: may a resume reach Renode now? The CPU must be parked in a running
/// `RUN_FOR` that ends at `g_ns`, and not at its end.
fn may_resume(st: CpuState, g_ns: u64) -> bool {
    st.active && !st.halted && st.t_ns < g_ns
}

/// How a packet from the debugger is handled.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Class {
    /// Forward; the reply goes back to the debugger.
    Forward,
    /// Forward and record a possible change of results.
    ForwardPerturbing(&'static str),
    /// Answer with this payload without forwarding.
    Answer(Vec<u8>),
    /// `monitor`: refused with a message.
    RefuseMonitor,
    Continue,
    Step,
    Question,
    Detach,
    Kill,
    InsertPoint,
    RemovePoint,
}

fn classify(p: &[u8]) -> Class {
    let s = String::from_utf8_lossy(p);
    let s = s.as_ref();
    match s {
        "?" => return Class::Question,
        "c" => return Class::Continue,
        "s" => return Class::Step,
        "k" => return Class::Kill,
        _ => {}
    }
    if s == "D" || s.starts_with("D;") {
        return Class::Detach;
    }
    if s.starts_with("qRcmd,") {
        return Class::RefuseMonitor;
    }
    // vCont: advertised (see `supported_reply`) and mapped onto `s` and `c`. One
    // CPU, all-stop: any step action is a step, else a continue; signals
    // are ignored, as Renode ignores them.
    if s == "vCont?" {
        return Class::Answer(b"vCont;c;C;s;S".to_vec());
    }
    if let Some(actions) = s.strip_prefix("vCont;") {
        let kinds: Vec<u8> = actions
            .split(';')
            .filter_map(|a| a.bytes().next())
            .collect();
        if kinds.iter().any(|k| matches!(k, b's' | b'S')) {
            return Class::Step;
        }
        if !kinds.is_empty() && kinds.iter().all(|k| matches!(k, b'c' | b'C')) {
            return Class::Continue;
        }
        return Class::Answer(b"E01".to_vec());
    }
    // Continue/step at an address or with a signal, reverse execution,
    // restarts, Renode's TRACE32 packets.
    let refused = [
        "bc", "bs", "vRun", "Qtrace32", "qtrace32", "vAttach", "vKill",
    ];
    if refused.iter().any(|r| s.starts_with(r))
        || matches!(
            s.as_bytes().first(),
            Some(b'c' | b's' | b'C' | b'S' | b'R' | b'!')
        )
    {
        return Class::Answer(Vec::new());
    }
    match s.as_bytes().first() {
        Some(b'Z') => Class::InsertPoint,
        Some(b'z') => Class::RemovePoint,
        Some(b'M' | b'X' | b'P' | b'G') => Class::ForwardPerturbing("a register or memory write"),
        Some(b'm') => {
            let r = s[1..].split_once(',').and_then(|(a, l)| {
                Some((
                    u64::from_str_radix(a, 16).ok()?,
                    u64::from_str_radix(l, 16).ok()?,
                ))
            });
            match r {
                Some((a, l)) if touches_registers(a, l) => {
                    Class::ForwardPerturbing("a read of a peripheral or system register")
                }
                _ => Class::Forward,
            }
        }
        _ => Class::Forward,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Resume {
    /// The debugger's `c` (`auto`: the proxy's own, no debugger attached).
    Continue {
        auto: bool,
    },
    Step,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Halt {
    None,
    /// Wanted (Ctrl-C, attach): applied before the next `RUN_FOR`.
    Requested,
    /// The CPU is in single-step mode and parks at the next slice boundary.
    Applied,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CState {
    Stopped,
    Running(Resume),
}

struct ClientConn {
    id: u64,
    out: TcpStream,
    state: CState,
    /// `?` was asked before the CPU parked: answered at the park.
    question: bool,
    /// A stop reply was sent to this debugger.
    told: bool,
    last_stop: Vec<u8>,
    /// Breakpoints and watchpoints this debugger inserted (`type,addr,kind`).
    points: BTreeSet<String>,
}

enum Expect {
    /// The reply goes to the debugger with this id.
    Client(u64),
    /// Renode's `qSupported` reply, edited by [`supported_reply`].
    Supported(u64),
    /// The reply to an insertion or removal: on `OK` the point set changes.
    Point(u64, bool, String),
    /// The reply is dropped.
    Drop,
    StepDone(u64),
    Rcmd(RcmdFor, Vec<u8>),
}

#[derive(Clone, Copy)]
enum RcmdFor {
    Halt,
    State(u64),
}

struct Brain {
    renode: TcpStream,
    renode_ok: bool,
    tx: Sender<Ev>,
    expect: VecDeque<Expect>,
    client: Option<ClientConn>,
    next_client: u64,
    /// `--gdb-wait on`, until the first debugger attaches.
    wait_for_first: bool,
    /// The end (ns) of the open `RUN_FOR`.
    run_for: Option<u64>,
    /// Incremented at each `RUN_FOR` start and end.
    gen: u64,
    /// The CPU is in single-step mode.
    stepping: bool,
    /// A state read found the CPU parked since the last resume or `RUN_FOR`
    /// boundary.
    parked: bool,
    held: Option<Resume>,
    /// A state read is in flight.
    checking: bool,
    /// When to read the state again.
    retry_at: Option<Instant>,
    halt: Halt,
    /// The engine waits for the halt command's reply.
    engine: Option<SyncSender<Result<(), String>>>,
    /// The first change-of-results operation since the last `RUN_FOR` end.
    perturbed: Option<String>,
    status: Arc<Mutex<GdbStatus>>,
    trace: Option<std::io::BufWriter<std::fs::File>>,
    t0: Instant,
}

impl Brain {
    fn run(mut self, rx: Receiver<Ev>) {
        loop {
            let ev = match self.retry_at {
                Some(at) => {
                    let wait = at.saturating_duration_since(Instant::now());
                    match rx.recv_timeout(wait) {
                        Ok(ev) => ev,
                        Err(RecvTimeoutError::Timeout) => {
                            self.retry_at = None;
                            self.try_settle();
                            continue;
                        }
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
                None => match rx.recv() {
                    Ok(ev) => ev,
                    Err(_) => break,
                },
            };
            match ev {
                Ev::Renode(Wire::Packet(p)) => self.on_renode(p),
                Ev::Renode(Wire::Eof) => {
                    self.log("renode", b"EOF");
                    self.renode_ok = false;
                    if let Some(r) = self.engine.take() {
                        let _ = r.send(Err("Renode closed the GDB stub connection".into()));
                    }
                }
                Ev::Renode(Wire::Interrupt) => {}
                Ev::Accept(s) => self.on_accept(s),
                Ev::Client(id, w) => {
                    if self.client.as_ref().is_some_and(|c| c.id == id) {
                        match w {
                            Wire::Packet(p) => self.on_client(p),
                            Wire::Interrupt => self.on_interrupt(),
                            Wire::Eof => self.release("connection closed"),
                        }
                    }
                }
                Ev::Before { g_ns, reply } => self.on_before(g_ns, reply),
                Ev::After { reply } => self.on_after(reply),
                Ev::Shutdown => break,
            }
        }
        if let Some(mut c) = self.client.take() {
            if matches!(c.state, CState::Running(_)) {
                let _ = c.out.write_all(&frame(b"W00"));
            }
            let _ = c.out.flush();
            let _ = c.out.shutdown(Shutdown::Both);
        }
        let _ = self.renode.shutdown(Shutdown::Both);
    }

    fn log(&mut self, dir: &str, p: &[u8]) {
        if let Some(f) = self.trace.as_mut() {
            let _ = writeln!(
                f,
                "{:10.6} {dir} {} [run_for={:?} stepping={} parked={} held={:?} halt={:?}]",
                self.t0.elapsed().as_secs_f64(),
                String::from_utf8_lossy(&p[..p.len().min(200)]).escape_debug(),
                self.run_for,
                self.stepping,
                self.parked,
                self.held,
                self.halt
            );
        }
    }

    fn send_renode(&mut self, p: &[u8], expect: Option<Expect>) {
        self.log("->renode", p);
        if !self.renode_ok {
            return;
        }
        if self.renode.write_all(&frame(p)).is_err() {
            self.renode_ok = false;
            if let Some(r) = self.engine.take() {
                let _ = r.send(Err("cannot write to Renode's GDB stub".into()));
            }
            return;
        }
        if let Some(e) = expect {
            self.expect.push_back(e);
        }
    }

    fn send_client(&mut self, p: &[u8]) {
        self.log("->gdb", p);
        let ok = match self.client.as_mut() {
            Some(c) => c.out.write_all(&frame(p)).is_ok(),
            None => true,
        };
        if !ok {
            self.release("write failed");
        }
    }

    /// Tell the debugger the CPU stopped (`reply`).
    fn tell_stop(&mut self, reply: &[u8]) {
        if let Some(c) = self.client.as_mut() {
            c.state = CState::Stopped;
            c.last_stop = reply.to_vec();
            c.told = true;
            c.question = false;
        }
        self.send_client(reply);
    }

    fn note(&mut self, what: &str) {
        if self.perturbed.is_none() {
            self.perturbed = Some(what.to_string());
        }
        let mut s = self.status.lock().unwrap_or_else(|e| e.into_inner());
        s.perturbations += 1;
        if s.first.is_none() {
            s.first = Some(what.to_string());
        }
    }

    fn on_before(&mut self, g_ns: u64, reply: SyncSender<Result<(), String>>) {
        self.log("engine", format!("before RUN_FOR to {g_ns} ns").as_bytes());
        if !self.renode_ok {
            let _ = reply.send(Err("Renode's GDB stub connection is closed".into()));
            return;
        }
        self.gen += 1;
        self.run_for = Some(g_ns);
        self.parked = false;
        if self.halt == Halt::Requested {
            // R3: single-step mode while Renode is paused; the CPU parks at
            // the start of this RUN_FOR.
            self.engine = Some(reply);
            let cmd = format!("qRcmd,{}", hex_encode(HALT_COMMAND));
            self.send_renode(
                cmd.as_bytes(),
                Some(Expect::Rcmd(RcmdFor::Halt, Vec::new())),
            );
        } else {
            let _ = reply.send(Ok(()));
            self.try_settle();
        }
    }

    fn on_after(&mut self, reply: SyncSender<Result<Option<String>, String>>) {
        self.log("engine", b"after RUN_FOR");
        self.gen += 1;
        self.run_for = None;
        self.parked = false;
        self.retry_at = None;
        let _ = reply.send(if self.renode_ok {
            Ok(self.perturbed.take())
        } else {
            Err("Renode's GDB stub connection is closed".into())
        });
    }

    fn on_accept(&mut self, s: TcpStream) {
        if self.client.is_some() {
            eprintln!("bb run: GDB: a second debugger connection was refused (one at a time)");
            let _ = s.shutdown(Shutdown::Both);
            return;
        }
        let Ok(r) = s.try_clone() else { return };
        let _ = s.set_nodelay(true);
        self.next_client += 1;
        let id = self.next_client;
        let tx = self.tx.clone();
        let spawned = std::thread::Builder::new()
            .name("bb-gdb-client".into())
            .spawn(move || read_packets(r, |w| tx.send(Ev::Client(id, w)).is_ok()));
        if spawned.is_err() {
            return;
        }
        self.log("accept", id.to_string().as_bytes());
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .sessions += 1;
        self.client = Some(ClientConn {
            id,
            out: s,
            state: CState::Stopped,
            question: false,
            told: false,
            last_stop: b"S05".to_vec(),
            points: BTreeSet::new(),
        });
        self.wait_for_first = false;
        // The debugger takes the CPU: no automatic continue, and a running
        // CPU is halted at the next slice boundary.
        if self.held == Some(Resume::Continue { auto: true }) {
            self.held = None;
        }
        if !self.stepping && self.halt == Halt::None {
            self.halt = Halt::Requested;
        }
    }

    fn on_client(&mut self, p: Vec<u8>) {
        self.log("gdb->", &p);
        if let Some(c) = self.client.as_mut() {
            let _ = c.out.write_all(b"+");
        }
        let id = self.client.as_ref().map(|c| c.id).unwrap_or(0);
        let class = classify(&p);
        match class {
            Class::Question => {
                let told = self
                    .client
                    .as_ref()
                    .is_some_and(|c| c.told && c.state == CState::Stopped);
                if told || self.parked {
                    let s = self
                        .client
                        .as_ref()
                        .map(|c| c.last_stop.clone())
                        .unwrap_or_default();
                    self.tell_stop(&s);
                } else {
                    // Answered once the CPU is parked (it may still run to
                    // its next slice boundary after an attach).
                    if let Some(c) = self.client.as_mut() {
                        c.question = true;
                    }
                    self.try_settle();
                }
            }
            Class::Continue | Class::Step => {
                let r = if class == Class::Step {
                    Resume::Step
                } else {
                    Resume::Continue { auto: false }
                };
                if let Some(c) = self.client.as_mut() {
                    c.state = CState::Running(r);
                }
                self.held = Some(r);
                self.try_settle();
            }
            Class::Detach => {
                self.send_client(b"OK");
                self.release("detach");
            }
            Class::Kill => self.release("kill"),
            Class::RefuseMonitor => {
                let msg = "bb: monitor commands are refused under bb run --gdb: they reach \
                           Renode's time control (ADR-008 D10)\n";
                self.send_client(format!("O{}", hex_encode(msg)).as_bytes());
                self.send_client(b"E01");
            }
            Class::Answer(a) => self.send_client(&a),
            Class::InsertPoint | Class::RemovePoint => {
                let insert = class == Class::InsertPoint;
                if insert {
                    self.note("a breakpoint or watchpoint insertion");
                }
                let spec = String::from_utf8_lossy(&p[1..]).into_owned();
                self.send_renode(&p, Some(Expect::Point(id, insert, spec)));
            }
            Class::ForwardPerturbing(what) => {
                self.note(what);
                self.send_renode(&p, Some(Expect::Client(id)));
            }
            Class::Forward if p.starts_with(b"qSupported") => {
                self.send_renode(&p, Some(Expect::Supported(id)))
            }
            Class::Forward => self.send_renode(&p, Some(Expect::Client(id))),
        }
    }

    fn on_interrupt(&mut self) {
        self.log("gdb->", b"^C");
        let running = self
            .client
            .as_ref()
            .is_some_and(|c| c.state == CState::Running(Resume::Continue { auto: false }));
        if !running {
            return;
        }
        if self.held.is_some() {
            // The CPU never left its park.
            self.held = None;
            self.tell_stop(b"S02");
            return;
        }
        if self.halt == Halt::None {
            self.halt = Halt::Requested;
        }
    }

    /// R4: remove the debugger's points and let the CPU run.
    fn release(&mut self, why: &str) {
        let Some(c) = self.client.take() else { return };
        self.log("release", why.as_bytes());
        let _ = c.out.shutdown(Shutdown::Both);
        for spec in &c.points {
            let z = format!("z{spec}");
            self.send_renode(z.as_bytes(), Some(Expect::Drop));
        }
        self.held = None;
        if self.halt == Halt::Requested {
            self.halt = Halt::None;
        }
        if self.stepping {
            self.held = Some(Resume::Continue { auto: true });
            self.try_settle();
        }
    }

    fn on_renode(&mut self, p: Vec<u8>) {
        self.log("renode->", &p);
        if self.renode_ok {
            let _ = self.renode.write_all(b"+");
        }
        if is_stop(&p) {
            self.on_stop(p);
            return;
        }
        let Some(front) = self.expect.front_mut() else {
            return; // unexpected; dropped
        };
        if let Expect::Rcmd(_, out) = front {
            let p = unrle(&p);
            if p.first() == Some(&b'O') && p != b"OK" {
                out.extend(hex_decode(&p[1..]));
                return;
            }
        }
        let e = self.expect.pop_front().expect("front");
        match e {
            Expect::Client(id) => {
                if self.client.as_ref().is_some_and(|c| c.id == id) {
                    self.send_client(&p);
                }
            }
            Expect::Supported(id) => {
                if self.client.as_ref().is_some_and(|c| c.id == id) {
                    let reply = supported_reply(&p);
                    self.send_client(&reply);
                }
            }
            Expect::Point(id, insert, spec) => {
                if let Some(c) = self.client.as_mut().filter(|c| c.id == id) {
                    if p == b"OK" {
                        if insert {
                            c.points.insert(spec);
                        } else {
                            c.points.remove(&spec);
                        }
                    }
                    self.send_client(&p);
                } else if insert && p == b"OK" {
                    // Its debugger left before the reply: R4 removes it.
                    let z = format!("z{spec}");
                    self.send_renode(z.as_bytes(), Some(Expect::Drop));
                }
            }
            Expect::Drop => {}
            Expect::StepDone(id) => {
                // Renode's `Step` has returned: the instruction executed.
                let stepping_client = self
                    .client
                    .as_ref()
                    .is_some_and(|c| c.id == id && c.state == CState::Running(Resume::Step));
                if stepping_client {
                    self.tell_stop(b"S05");
                }
                self.try_settle();
            }
            Expect::Rcmd(RcmdFor::Halt, _) => {
                self.halt = Halt::Applied;
                self.stepping = true;
                if let Some(r) = self.engine.take() {
                    let _ = r.send(if p == b"OK" {
                        Ok(())
                    } else {
                        Err(format!("GDB halt: {}", String::from_utf8_lossy(&p)))
                    });
                }
                self.try_settle();
            }
            Expect::Rcmd(RcmdFor::State(gen), out) => {
                self.checking = false;
                let st = parse_state(&out);
                self.log(
                    "cpu-state",
                    format!("{st:?} (gen {gen}, now {})", self.gen).as_bytes(),
                );
                if gen != self.gen {
                    self.try_settle();
                    return;
                }
                let (Some(st), Some(g)) = (st, self.run_for) else {
                    if st.is_none() {
                        eprintln!(
                            "bb run: GDB: unexpected CPU state reply {:?}",
                            String::from_utf8_lossy(&out)
                        );
                    }
                    return;
                };
                if may_resume(st, g) {
                    self.settled();
                } else if st.active && st.t_ns >= g {
                    // An end park: the CPU parks again at the start of the
                    // next RUN_FOR (`on_before` reads again).
                } else {
                    // Not started yet, or asleep in WFI: read again soon.
                    self.retry_at = Some(Instant::now() + RETRY);
                }
            }
        }
    }

    /// The CPU is parked in the open RUN_FOR: answer `?`, finish a halt,
    /// forward the held resume.
    fn settled(&mut self) {
        self.parked = true;
        if self.halt == Halt::Applied {
            self.halt = Halt::None;
            let ctrl_c = self
                .client
                .as_ref()
                .is_some_and(|c| c.state == CState::Running(Resume::Continue { auto: false }));
            if ctrl_c {
                self.tell_stop(b"S02");
            }
        }
        if self.client.as_ref().is_some_and(|c| c.question) {
            let s = self
                .client
                .as_ref()
                .map(|c| c.last_stop.clone())
                .unwrap_or_default();
            self.tell_stop(&s);
        }
        if self.client.is_none() && !self.wait_for_first && self.held.is_none() {
            self.held = Some(Resume::Continue { auto: true });
        }
        if let Some(r) = self.held.take() {
            self.forward(r);
        }
    }

    fn on_stop(&mut self, p: Vec<u8>) {
        match p[0] {
            b'T' => {
                // A breakpoint or watchpoint: the CPU is in single-step mode
                // and parks next.
                self.stepping = true;
                self.parked = false;
                self.note("a breakpoint or watchpoint stop");
                if self.halt == Halt::Requested {
                    self.halt = Halt::None;
                }
                let running = self
                    .client
                    .as_ref()
                    .is_some_and(|c| c.state == CState::Running(Resume::Continue { auto: false }));
                if running {
                    self.tell_stop(&p);
                }
                self.try_settle();
            }
            b'S' => {
                // A park (S05) is only a hint to read the CPU's state; a
                // pause (S02) never comes from a debugger request.
                self.try_settle();
            }
            _ => {
                // W/X: the target ended (Renode sends these only on an abort).
                let running = self
                    .client
                    .as_ref()
                    .is_some_and(|c| c.state != CState::Stopped);
                if running {
                    self.send_client(&p);
                }
            }
        }
    }

    /// Something waits for the CPU to be parked: a held resume, a `?`, a
    /// halt (or, with no debugger, the automatic continue).
    fn waiting(&self) -> bool {
        self.held.is_some()
            || self.halt == Halt::Applied
            || self.client.as_ref().is_some_and(|c| c.question)
            || (self.client.is_none() && !self.wait_for_first)
    }

    /// R1: read the CPU's state if something waits for a park.
    fn try_settle(&mut self) {
        if !self.waiting()
            || self.checking
            || self.run_for.is_none()
            || !self.stepping
            || !self.renode_ok
        {
            return;
        }
        self.checking = true;
        self.retry_at = None;
        let cmd = format!("qRcmd,{}", hex_encode(STATE_QUERY));
        let gen = self.gen;
        self.send_renode(
            cmd.as_bytes(),
            Some(Expect::Rcmd(RcmdFor::State(gen), Vec::new())),
        );
    }

    fn forward(&mut self, r: Resume) {
        self.parked = false;
        match r {
            Resume::Continue { .. } => {
                self.stepping = false;
                self.send_renode(b"c", None);
            }
            Resume::Step => {
                self.note("a single-step");
                self.stepping = true;
                let id = self.client.as_ref().map(|c| c.id).unwrap_or(0);
                self.send_renode(b"s", None);
                self.send_renode(PING, Some(Expect::StepDone(id)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framing_round_trips() {
        assert_eq!(frame(b"OK"), b"$OK#9a".to_vec());
        let mut p = Parser::default();
        let mut w = p.feed(b"+$qSupported:multiprocess+#c6$?#3");
        w.extend(p.feed(b"f\x03-$c#63"));
        assert_eq!(
            w,
            vec![
                Wire::Packet(b"qSupported:multiprocess+".to_vec()),
                Wire::Packet(b"?".to_vec()),
                Wire::Interrupt,
                Wire::Packet(b"c".to_vec()),
            ]
        );
    }

    #[test]
    fn rle_and_hex() {
        assert_eq!(unrle(b"0* "), b"0000".to_vec());
        assert_eq!(hex_decode(b"35300d0a"), b"50\r\n".to_vec());
        assert_eq!(hex_encode("ab"), "6162");
    }

    #[test]
    fn packets_are_classified() {
        assert_eq!(classify(b"?"), Class::Question);
        assert_eq!(classify(b"c"), Class::Continue);
        assert_eq!(classify(b"s"), Class::Step);
        assert_eq!(classify(b"D"), Class::Detach);
        assert_eq!(classify(b"D;1"), Class::Detach);
        assert_eq!(classify(b"k"), Class::Kill);
        assert_eq!(classify(b"qRcmd,7374617274"), Class::RefuseMonitor);
        for refused in [
            "bc", "bs", "vRun;", "c8000094", "s8000094", "C05", "S05", "R00", "!",
        ] {
            assert_eq!(
                classify(refused.as_bytes()),
                Class::Answer(Vec::new()),
                "{refused}"
            );
        }
        assert_eq!(
            classify(b"vCont?"),
            Class::Answer(b"vCont;c;C;s;S".to_vec())
        );
        assert_eq!(classify(b"vCont;c"), Class::Continue);
        assert_eq!(classify(b"vCont;c:1"), Class::Continue);
        assert_eq!(classify(b"vCont;s:1;c"), Class::Step);
        assert_eq!(classify(b"vCont;S05:1"), Class::Step);
        assert_eq!(classify(b"vCont;t"), Class::Answer(b"E01".to_vec()));
        assert_eq!(classify(b"qSupported:swbreak+"), Class::Forward);
        assert_eq!(classify(b"qSymbol::"), Class::Forward);
        assert_eq!(classify(b"Z0,8000094,2"), Class::InsertPoint);
        assert_eq!(classify(b"z0,8000094,2"), Class::RemovePoint);
        assert!(matches!(
            classify(b"M20000000,4:00000000"),
            Class::ForwardPerturbing(_)
        ));
        assert!(matches!(
            classify(b"P0=00000000"),
            Class::ForwardPerturbing(_)
        ));
        assert_eq!(classify(b"m20000000,10"), Class::Forward);
        assert_eq!(classify(b"m8000094,4"), Class::Forward);
        assert!(matches!(
            classify(b"m40010800,10"),
            Class::ForwardPerturbing(_)
        ));
        assert!(matches!(
            classify(b"me000e010,4"),
            Class::ForwardPerturbing(_)
        ));
        assert!(matches!(
            classify(b"m3ffffffc,8"),
            Class::ForwardPerturbing(_)
        ));
        assert_eq!(classify(b"g"), Class::Forward);
        assert_eq!(classify(b"p f"), Class::Forward);
    }

    #[test]
    fn supported_reply_offers_vcont_steps() {
        let r = supported_reply(
            b"PacketSize=1024;qXfer:features:read+;swbreak+;hwbreak+;ReverseStep+;ReverseContinue+",
        );
        assert_eq!(
            String::from_utf8(r).unwrap(),
            "PacketSize=1024;qXfer:features:read+;swbreak+;hwbreak+;vContSupported+"
        );
    }

    #[test]
    fn stop_replies_are_recognised() {
        for s in ["S05", "T05swbreak:;", "W00", "X06"] {
            assert!(is_stop(s.as_bytes()), "{s}");
        }
        for r in ["OK", "QC1", "E01", "", "l<?xml", "00ff", "PacketSize=400"] {
            assert!(!is_stop(r.as_bytes()), "{r}");
        }
    }

    /// R1: a resume reaches Renode only with the time source running, the
    /// CPU awake and its time before the RUN_FOR's end.
    #[test]
    fn resume_barrier() {
        let st = parse_state(b"139875 True False\r\r\n").unwrap();
        assert_eq!(
            st,
            CpuState {
                t_ns: 139_875,
                active: true,
                halted: false
            }
        );
        assert!(may_resume(st, 140_000));
        assert!(!may_resume(st, 139_875), "an end park");
        let idle = parse_state(b"140000 False False").unwrap();
        assert!(!may_resume(idle, 150_000), "RUN_FOR not running yet");
        let wfi = parse_state(b"140000 True True").unwrap();
        assert!(!may_resume(wfi, 150_000), "asleep in WFI");
        assert!(parse_state(b"Traceback").is_none());
    }
}
