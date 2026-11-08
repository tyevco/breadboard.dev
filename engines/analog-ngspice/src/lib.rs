//! Analog engine backed by the ngspice shared library (`libngspice.so.0`).
//!
//! [`NgspiceEngine`] implements [`bbcore::Engine`] following
//! `docs/adr/002-time-grant-protocol.md` (see "Amendment 1: D5 resolution")
//! and `spec/time-grant.md`.
//!
//! # How a grant is executed
//!
//! The transient analysis runs on ngspice's own background thread
//! (`bg_run`). ngspice calls the `GetSyncData` callback once per accepted
//! time point (`loc == 0`, after the point has been written to the output
//! vectors and before the next step is taken) with a pointer to the next
//! step size. The callback
//!
//! 1. clamps that step so the solver lands exactly on the current grant `g`
//!    (and on the corners of any input ramp), and
//! 2. when the accepted point *is* `g`, copies the new output-vector samples
//!    (`ngSpice_CurPlot` + `ngGet_Vec_Info`), publishes them and **blocks** the
//!    solver thread on a condvar until the kernel grants the next interval.
//!
//! The solver therefore never computes a point past `g`, and since it only
//! ever proceeds while the kernel thread is waiting for it, the sequence of
//! solver steps is a pure function of the netlist and of the call sequence.
//! ngspice is advanced once per kernel grant, never per CPU instruction.
//!
//! # Inputs
//!
//! Inputs are `EXTERNAL` voltage sources (`Vname n+ n- dc 0 external`). Their
//! value comes from the `GetVSRCData` callback, computed from the edge history
//! the kernel provides as a piecewise-linear waveform: each edge is a linear
//! ramp (rise/fall time of the pin model) starting at `max(edge_time, now)`:
//! at its true time when the kernel delivers it before this engine advances
//! through the grant containing it (ADR-002 Amendment 2), otherwise at the
//! boundary where it was delivered (D4.4). [`NgspiceEngine::push_edge`] sets
//! an explicit time.
//!
//! # Process-global state
//!
//! ngspice is a process-global singleton. Only one [`NgspiceEngine`] can exist
//! per process at a time; [`NgspiceEngine::new`] fails while another is alive.
//! Tests that create engines must serialize (e.g. with a global mutex).
//!
//! Known bad idea: never step ngspice per CPU instruction.

mod ffi;

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use bbcore::{DigitalOut, Engine, EngineError, Level, PinSignal, PinSignals, Time};

/// Name this engine reports through [`bbcore::Engine::name`] by default.
pub const ENGINE_NAME: &str = "ngspice";

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// How the kernel drives an input pin of the analog engine.
#[derive(Clone, Debug, PartialEq)]
pub enum Drive {
    /// A digital output of another engine, converted with its pin model:
    /// a level change becomes a linear ramp between `vol` and `voh` over
    /// `rise`/`fall`.
    Digital {
        /// Pin model of the driving output.
        model: DigitalOut,
        /// Level assumed before the first exchange (used for the DC operating
        /// point at t = 0).
        initial: Level,
    },
    /// An analog value in volts. A new value is reached by a linear ramp of
    /// length `slew` (use a few ns; zero gives a step).
    Analog {
        /// Value before the first exchange.
        initial: f64,
        /// Ramp duration for a value change.
        slew: Time,
    },
}

/// Binds a kernel input pin to an `EXTERNAL` voltage source.
#[derive(Clone, Debug, PartialEq)]
pub struct InputBinding {
    /// Engine-side pin name (what the kernel binds nets to).
    pub pin: String,
    /// Name of the voltage source, e.g. `V_PA5` (case-insensitive, like
    /// SPICE).
    pub source: String,
    /// How the pin is driven.
    pub drive: Drive,
    /// `None`: the netlist already contains `<source> n+ n- dc 0 external`.
    /// `Some(node)`: the engine generates the glue itself, i.e.
    /// `<source> <source>_int 0 dc 0 external` plus a series resistor of the
    /// pin model's `zout` from `<source>_int` to `node` (for an analog drive,
    /// or `zout == 0`, the source connects straight to `node`).
    pub glue_node: Option<String>,
}

/// Binds a kernel output pin to a circuit node voltage.
#[derive(Clone, Debug, PartialEq)]
pub struct OutputBinding {
    /// Engine-side pin name.
    pub pin: String,
    /// Circuit node name (voltage relative to ground).
    pub node: String,
}

/// Configuration of an [`NgspiceEngine`].
#[derive(Clone, Debug, PartialEq)]
pub struct NgspiceConfig {
    /// Engine name (default [`ENGINE_NAME`]).
    pub name: String,
    /// Circuit body: element and model lines. No title line, no `.tran`, no
    /// `.control` block; a trailing `.end` is ignored. The engine adds the
    /// title, the input glue, the `.tran` card and `.end`.
    pub netlist: String,
    /// End of the transient analysis. The kernel cannot grant past it.
    pub t_stop: Time,
    /// Maximum internal solver step (`TMAX` of the `.tran` card). Fixed by the
    /// configuration so the step sequence is a pure function of the inputs.
    pub t_max: Time,
    /// Lookahead promised to the kernel: `next_safe_time(now) = now +
    /// lookahead` (bounded by breakpoints and `t_stop`). `None` means the
    /// circuit has no internal events the kernel must observe (ADR-002 D2:
    /// `t_safe = Time::MAX` up to `t_stop`).
    pub lookahead: Option<Time>,
    /// Input pins.
    pub inputs: Vec<InputBinding>,
    /// Output pins (node voltages reported by `read_outputs`).
    pub outputs: Vec<OutputBinding>,
    /// Extra nodes whose voltages are recorded for [`NgspiceEngine::voltage`]
    /// and [`NgspiceEngine::sample`].
    pub probes: Vec<String>,
    /// How long each side of a kernel <-> solver handoff busy-waits on an
    /// atomic sequence number before blocking on the condvar (ADR-003
    /// D1.4). Zero (the default) never spins. Spinning only changes *when*
    /// a thread wakes, never what it computes, so results do not depend on
    /// it; it saves the condvar wake-up latency when grants are short (one-
    /// cell lockstep) at the cost of burning a core while waiting.
    pub handoff_spin: Duration,
    /// The kernel's grant lattice `L` (ADR-003 D2), when the run has one.
    /// Then `t_max` must divide `L`, every solver step is also clamped to the
    /// next lattice point (so there is an accepted point on every lattice
    /// point), and an input ramp's breakpoints are handed to ngspice only at
    /// the lattice point that opens the cell containing the ramp start. The
    /// solver then makes the same calls with the same arguments whatever the
    /// grant partition, so its points do not depend on it. `None`: no
    /// lattice clamp, breakpoints released at once (the Phase 0 behaviour).
    pub lattice: Option<Time>,
}

impl NgspiceConfig {
    /// A configuration with no pins: `netlist`, `t_stop` and `t_max`.
    pub fn new(netlist: impl Into<String>, t_stop: Time, t_max: Time) -> NgspiceConfig {
        NgspiceConfig {
            name: ENGINE_NAME.to_string(),
            netlist: netlist.into(),
            t_stop,
            t_max,
            lookahead: None,
            inputs: Vec::new(),
            outputs: Vec::new(),
            probes: Vec::new(),
            handoff_spin: Duration::ZERO,
            lattice: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Piecewise-linear source waveform
// ---------------------------------------------------------------------------

/// One linear ramp: `v0` at `t0`, `v1` from `t0 + dur` on.
#[derive(Clone, Debug)]
struct Seg {
    t0_fs: u64,
    t0_s: f64,
    dur_s: f64,
    v0: f64,
    v1: f64,
}

impl Seg {
    fn value_at(&self, t: f64) -> f64 {
        if t <= self.t0_s {
            return self.v0;
        }
        if self.dur_s <= 0.0 || t >= self.t0_s + self.dur_s {
            return self.v1;
        }
        self.v0 + (self.v1 - self.v0) * ((t - self.t0_s) / self.dur_s)
    }
}

/// The value of an `EXTERNAL` source as a function of time. Deterministic: a
/// pure function of the pushed edges.
#[derive(Clone, Debug)]
struct Wave {
    /// Sorted by `t0_fs`; never empty (the first is the initial value).
    segs: Vec<Seg>,
}

impl Wave {
    fn constant(v: f64) -> Wave {
        Wave {
            segs: vec![Seg {
                t0_fs: 0,
                t0_s: 0.0,
                dur_s: 0.0,
                v0: v,
                v1: v,
            }],
        }
    }

    fn value_at(&self, t: f64) -> f64 {
        // Last segment starting at or before t (the first one otherwise).
        let i = self.segs.iter().rposition(|s| s.t0_s <= t).unwrap_or(0);
        self.segs[i].value_at(t)
    }

    /// Ramp from the current value at `t0` to `target` over `dur`.
    fn push(&mut self, t0: Time, dur: Time, target: f64) {
        let t0_s = t0.as_secs_f64();
        let v0 = self.value_at(t0_s);
        // A new edge at the same instant as an older one replaces it.
        while self.segs.len() > 1 && self.segs.last().is_some_and(|s| s.t0_fs >= t0.as_fs()) {
            self.segs.pop();
        }
        self.segs.push(Seg {
            t0_fs: t0.as_fs(),
            t0_s,
            dur_s: dur.as_secs_f64(),
            v0,
            v1: target,
        });
    }

    /// Drop segments that can no longer be evaluated (the solver never goes
    /// back before the last accepted boundary).
    fn prune(&mut self, now: Time) {
        if let Some(i) = self.segs.iter().rposition(|s| s.t0_fs <= now.as_fs()) {
            self.segs.drain(..i);
        }
    }
}

// ---------------------------------------------------------------------------
// State shared with the ngspice callbacks
// ---------------------------------------------------------------------------

/// Samples copied from the output vectors at a boundary.
struct Reading {
    /// Value of the `time` vector of each new point.
    times: Vec<f64>,
    /// Per watched node, the values of the new points.
    values: Vec<Vec<f64>>,
}

struct State {
    /// An engine currently owns ngspice.
    active: bool,
    /// The background thread has started at least once for this engine.
    bg_started: bool,
    /// The background thread is running.
    bg_running: bool,
    /// Teardown: callbacks return immediately and never block.
    abort: bool,
    /// Current grant, in fs and in seconds.
    target_fs: u64,
    target_s: f64,
    /// Grant sequence number; incremented by every `advance_to`.
    seq: u64,
    /// `Some(seq)` while the solver is parked at the boundary of grant `seq`.
    parked: Option<u64>,
    /// Time (s) of the point the solver is parked at.
    parked_time_s: f64,
    /// New output samples taken at the last boundary.
    reading: Option<Reading>,
    /// Vector index up to which samples were already copied.
    read_from: usize,
    /// Node vectors to copy (lower case).
    watch: Vec<String>,
    /// `EXTERNAL` sources by lower-case name.
    sources: BTreeMap<String, Wave>,
    /// Upcoming ramp corners (s, sorted): solver steps are clamped to land on
    /// them.
    corners: Vec<f64>,
    /// Breakpoints to hand to `ngSpice_SetBkpt` on the solver thread, as
    /// `(release_s, bkpt_s)` in push order: each is set at the first
    /// accepted point at or after `release_s` (ADR-003 D2).
    new_bkpts: Vec<(f64, f64)>,
    /// Grant lattice in fs (0: none).
    lattice_fs: u64,
    /// First fatal problem seen by a callback.
    error: Option<String>,
    /// [`NgspiceConfig::handoff_spin`].
    handoff_spin: Duration,
}

impl State {
    const fn new() -> State {
        State {
            active: false,
            bg_started: false,
            bg_running: false,
            abort: false,
            target_fs: 0,
            target_s: 0.0,
            seq: 0,
            parked: None,
            parked_time_s: 0.0,
            reading: None,
            read_from: 0,
            watch: Vec::new(),
            sources: BTreeMap::new(),
            corners: Vec::new(),
            new_bkpts: Vec::new(),
            lattice_fs: 0,
            error: None,
            handoff_spin: Duration::ZERO,
        }
    }

    fn fail(&mut self, msg: String) {
        if self.error.is_none() {
            self.error = Some(msg);
        }
    }

    /// Limit the next solver step so it lands exactly on the grant, on the
    /// next lattice point (ADR-003 D2) and on the next ramp corner.
    fn clamp(&self, time: f64, delta: &mut f64) {
        let mut limit = self.target_s;
        if let Some(l) = next_lattice_s(time, self.lattice_fs) {
            limit = limit.min(l);
        }
        if let Some(&c) = self.corners.iter().find(|&&c| c > time + CORNER_MIN_S) {
            limit = limit.min(c);
        }
        if time + *delta > limit && limit > time {
            *delta = limit - time;
        }
    }
}

/// The first lattice point after the solver time `time` (s), for a lattice
/// of `lattice_fs` (0: none). A time within the grant tolerance of a lattice
/// point counts as on it, so a point that landed a few ulps short of `kL`
/// is not followed by a micro-step to `kL`. A pure function of `time`.
fn next_lattice_s(time: f64, lattice_fs: u64) -> Option<f64> {
    if lattice_fs == 0 {
        return None;
    }
    let t_fs = secs_to_fs(time);
    let near = (t_fs + lattice_fs / 2) / lattice_fs;
    let near_s = Time::from_fs(near * lattice_fs).as_secs_f64();
    let k = if (time - near_s).abs() <= grant_tolerance_s(near_s) {
        near + 1
    } else {
        t_fs / lattice_fs + 1
    };
    Some(Time::from_fs(k * lattice_fs).as_secs_f64())
}

/// Corners closer than this to the current solver time are not clamped to
/// (ngspice merges breakpoints closer than its own `CKTminBreak` anyway).
const CORNER_MIN_S: f64 = 1e-12;

static STATE: Mutex<State> = Mutex::new(State::new());
static WAKE: Condvar = Condvar::new();
/// Mirrors of `State::seq` and of the grant the solver is parked at
/// (`NOT_PARKED` if none), written under the `STATE` lock, read lock-free by
/// the optional handoff spin (ADR-003 D1.4). Every decision is still taken
/// under the lock; the mirrors only tell a spinning thread when to stop.
static GRANT_SEQ: AtomicU64 = AtomicU64::new(0);
static PARKED_SEQ: AtomicU64 = AtomicU64::new(NOT_PARKED);
const NOT_PARKED: u64 = u64::MAX;

/// Reset the shared state (and its atomic mirrors) for a new engine or
/// after teardown.
fn reset_state(st: &mut State) {
    *st = State::new();
    GRANT_SEQ.store(0, Ordering::Release);
    PARKED_SEQ.store(NOT_PARKED, Ordering::Release);
}

/// Busy-wait until `done()` or until `limit` has passed (wall clock; it only
/// decides when a thread goes to sleep, never what is computed).
fn spin_until(limit: Duration, done: impl Fn() -> bool) {
    if limit.is_zero() {
        return;
    }
    let start = Instant::now();
    let mut n = 0u32;
    while !done() {
        std::hint::spin_loop();
        n = n.wrapping_add(1);
        if n.is_multiple_of(64) && start.elapsed() >= limit {
            return;
        }
    }
}
/// ngspice text output (stdout/stderr lines), kept separately from `STATE`
/// because ngspice prints from inside calls the engine makes.
static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());
const LOG_CAP: usize = 4000;

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

fn log() -> MutexGuard<'static, Vec<String>> {
    LOG.lock().unwrap_or_else(|e| e.into_inner())
}

/// Distance to the next larger `f64` (unit in the last place) of `|x|`.
fn ulp(x: f64) -> f64 {
    let x = x.abs();
    f64::from_bits(x.to_bits() + 1) - x
}

/// How close (s) an accepted solver time must be to the grant time to count
/// as "at the grant" (ADR-002 Amendment 3). The decision is made in the f64
/// domain the solver works in: `time + (target - time)` need not equal
/// `target` exactly, and above ~2.3 s one ulp exceeds 0.5 fs, so rounding to
/// integer fs cannot be used to recognise the boundary. Half a femtosecond
/// (the kernel resolution) or four ulps of the target, whichever is larger.
fn grant_tolerance_s(target_s: f64) -> f64 {
    (4.0 * ulp(target_s)).max(0.5e-15)
}

/// Seconds (ngspice) to integer femtoseconds (kernel), rounding to nearest.
/// Only for points strictly inside a grant (diagnostics, sample lookup); a
/// boundary is always stored as the exact kernel time.
fn secs_to_fs(t: f64) -> u64 {
    if t <= 0.0 {
        0
    } else {
        (t * 1e15).round() as u64
    }
}

/// Exact decimal spelling of a time for a netlist card (integer fs).
fn spice_time(t: Time) -> String {
    format!("{}e-15", t.as_fs())
}

fn cstr_lossy(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: ngspice passes NUL-terminated strings.
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

// --- callbacks (all `extern "C"`; a panic here aborts, it never unwinds) ---

unsafe extern "C" fn cb_send_char(s: *mut c_char, _id: c_int, _u: *mut c_void) -> c_int {
    let line = cstr_lossy(s);
    if std::env::var_os("BB_NGSPICE_ECHO").is_some() {
        eprintln!("[ngspice] {line}");
    }
    let mut l = log();
    if l.len() >= LOG_CAP {
        l.drain(..LOG_CAP / 2);
    }
    l.push(line);
    0
}

unsafe extern "C" fn cb_controlled_exit(
    status: c_int,
    _immediate: bool,
    quit: bool,
    _id: c_int,
    _u: *mut c_void,
) -> c_int {
    let mut st = state();
    st.fail(format!(
        "ngspice requested exit (status {status}, quit={quit})"
    ));
    WAKE.notify_all();
    0
}

/// ngspice passes its `fl_exited` flag (sharedspice.c `_thread_run`):
/// `false` when the background thread starts, `true` when it ends.
unsafe extern "C" fn cb_bg_thread(exited: bool, _id: c_int, _u: *mut c_void) -> c_int {
    let mut st = state();
    st.bg_running = !exited;
    if !exited {
        st.bg_started = true;
    }
    WAKE.notify_all();
    0
}

unsafe extern "C" fn cb_get_vsrc(
    value: *mut f64,
    time: f64,
    name: *mut c_char,
    _id: c_int,
    _u: *mut c_void,
) -> c_int {
    let name = cstr_lossy(name).to_ascii_lowercase();
    let mut st = state();
    let v = match st.sources.get(&name) {
        Some(w) => w.value_at(time),
        None => {
            st.fail(format!(
                "EXTERNAL source {name} is not bound to an input pin"
            ));
            0.0
        }
    };
    if !value.is_null() {
        // SAFETY: ngspice passes a valid out-pointer.
        unsafe { *value = v };
    }
    0
}

unsafe extern "C" fn cb_get_sync(
    time: f64,
    delta: *mut f64,
    _olddelta: f64,
    redostep: c_int,
    _id: c_int,
    loc: c_int,
    _u: *mut c_void,
) -> c_int {
    if delta.is_null() {
        return 0;
    }
    // SAFETY: ngspice passes a pointer to ckt->CKTdelta.
    let delta = unsafe { &mut *delta };
    let mut st = state();
    if st.abort {
        return 0;
    }
    if loc == 0 {
        // `time` is the point just accepted and dumped to the vectors.
        let tol = grant_tolerance_s(st.target_s);
        if st.parked != Some(st.seq) && time >= st.target_s - tol {
            if time > st.target_s + tol {
                let msg = format!(
                    "solver overshot the grant: accepted t = {time:e} s, grant = {} fs",
                    st.target_fs
                );
                st.fail(msg);
            }
            let watch = st.watch.clone();
            let from = st.read_from;
            drop(st);
            let reading = read_vectors(&watch, from);
            st = state();
            match reading {
                Ok(r) => {
                    st.read_from = from + r.times.len();
                    st.reading = Some(r);
                }
                Err(e) => st.fail(e),
            }
            st.parked_time_s = time;
            st.parked = Some(st.seq);
            PARKED_SEQ.store(st.seq, Ordering::Release);
            WAKE.notify_all();
            let spin = st.handoff_spin;
            if !spin.is_zero() && !st.abort {
                let parked = st.seq;
                drop(st);
                spin_until(spin, || GRANT_SEQ.load(Ordering::Acquire) != parked);
                st = state();
            }
            while st.parked == Some(st.seq) && !st.abort {
                st = WAKE.wait(st).unwrap_or_else(|e| e.into_inner());
            }
            if st.abort {
                return 0;
            }
        }
        // Breakpoint release (ADR-003 D2): a ramp's breakpoints reach
        // ngspice at the accepted point that opens the lattice cell holding
        // the ramp start, which is where a one-cell run is parked when it
        // learns of the ramp. Without a lattice they are due at once.
        let rtol = grant_tolerance_s(time);
        if st.new_bkpts.iter().any(|&(r, _)| r <= time + rtol) {
            let mut due = Vec::new();
            st.new_bkpts.retain(|&(r, b)| {
                let now_due = r <= time + rtol;
                if now_due {
                    due.push(b);
                }
                !now_due
            });
            drop(st);
            if let Ok(lib) = ffi::lib(|_| Ok(())) {
                for b in due {
                    // SAFETY: called on the solver thread, between steps.
                    unsafe { (lib.set_bkpt)(b) };
                }
            }
            st = state();
        }
        st.clamp(time, delta);
    } else if redostep != 0 {
        // Step rejected; `time` is the last accepted point again.
        st.clamp(time, delta);
    }
    0
}

/// Copy the samples `[from..]` of the `time` vector and of every watched node
/// of the current plot. Runs on the solver thread while it is between steps.
fn read_vectors(watch: &[String], from: usize) -> Result<Reading, String> {
    let lib = ffi::lib(|_| Ok(()))?;
    // SAFETY: returns a pointer to the current plot's type name.
    let plot = cstr_lossy(unsafe { (lib.cur_plot)() });
    let fetch = |vec: &str| -> Result<Vec<f64>, String> {
        let full = CString::new(format!("{plot}.{vec}")).map_err(|e| e.to_string())?;
        // SAFETY: ngGet_Vec_Info only reads the name; the returned struct is
        // static inside ngspice and is copied out before the next call.
        let info = unsafe { (lib.get_vec_info)(full.as_ptr().cast_mut()) };
        if info.is_null() {
            return Err(format!("ngspice vector {plot}.{vec} not found"));
        }
        // SAFETY: non-null pointer to ngspice's vector_info.
        let info = unsafe { &*info };
        if info.v_realdata.is_null() {
            return Err(format!("ngspice vector {plot}.{vec} is not real"));
        }
        let len = usize::try_from(info.v_length).unwrap_or(0);
        // SAFETY: v_realdata holds v_length doubles; the solver is parked, so
        // the buffer is not reallocated while we copy it.
        let data = unsafe { std::slice::from_raw_parts(info.v_realdata, len) };
        Ok(data.get(from..).unwrap_or(&[]).to_vec())
    };
    let times = fetch("time")?;
    let mut values = Vec::with_capacity(watch.len());
    for w in watch {
        let v = fetch(w)?;
        if v.len() != times.len() {
            return Err(format!(
                "vector {w} has {} new points, time has {}",
                v.len(),
                times.len()
            ));
        }
        values.push(v);
    }
    Ok(Reading { times, values })
}

fn init_lib() -> Result<&'static ffi::Lib, String> {
    ffi::lib(|l| {
        // SAFETY: prototypes from sharedspice.h; callbacks are 'static fns.
        let rc = unsafe {
            (l.init)(
                Some(cb_send_char),
                None,
                Some(cb_controlled_exit),
                None,
                None,
                Some(cb_bg_thread),
                std::ptr::null_mut(),
            )
        };
        if rc != 0 {
            return Err(format!("ngSpice_Init returned {rc}"));
        }
        let mut ident: c_int = 0;
        // SAFETY: as above.
        let rc = unsafe {
            (l.init_sync)(
                Some(cb_get_vsrc),
                None,
                Some(cb_get_sync),
                &mut ident,
                std::ptr::null_mut(),
            )
        };
        if rc != 0 {
            return Err(format!("ngSpice_Init_Sync returned {rc}"));
        }
        Ok(())
    })
}

fn command(lib: &ffi::Lib, cmd: &str) -> Result<(), String> {
    let c = CString::new(cmd).map_err(|e| e.to_string())?;
    // SAFETY: ngspice copies the command string.
    let rc = unsafe { (lib.command)(c.as_ptr().cast_mut()) };
    if rc != 0 {
        return Err(format!("ngSpice_Command({cmd}) returned {rc}"));
    }
    Ok(())
}

/// Error lines ngspice printed since the log was last cleared.
fn logged_errors() -> Vec<String> {
    log()
        .iter()
        .filter(|l| l.starts_with("stderr Error") || l.starts_with("stderr error"))
        .cloned()
        .collect()
}

fn other(msg: impl Into<String>) -> EngineError {
    EngineError::Other(msg.into())
}

/// How long to wait for ngspice to start or stop its thread (failure paths
/// only; normal grants wait without a timeout).
const THREAD_TIMEOUT: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// The engine
// ---------------------------------------------------------------------------

struct InputPin {
    source: String,
    drive: Drive,
    /// Last level / value applied.
    level: Level,
    volts: f64,
}

/// Solver points of the watched nodes since the previous grant boundary.
struct History {
    times_fs: Vec<u64>,
    times_s: Vec<f64>,
    /// Indexed like `NgspiceEngine::watch`.
    values: Vec<Vec<f64>>,
}

/// The ngspice analog engine. See the crate docs.
pub struct NgspiceEngine {
    name: String,
    lib: &'static ffi::Lib,
    now: Time,
    t_stop: Time,
    lookahead: Option<Time>,
    inputs: BTreeMap<String, InputPin>,
    outputs: BTreeMap<String, String>,
    /// Watched node names (lower case), in vector-copy order.
    watch: Vec<String>,
    hist: History,
    /// Kernel-visible breakpoints (`add_breakpoint`).
    breakpoints: BTreeSet<Time>,
    /// [`NgspiceConfig::lattice`].
    lattice: Option<Time>,
    /// Number of solver points accepted so far (including t = 0).
    solver_points: usize,
    failed: bool,
}

impl NgspiceEngine {
    /// Load the circuit, start the transient, and park the solver at t = 0
    /// (after the DC operating point).
    pub fn new(cfg: NgspiceConfig) -> Result<NgspiceEngine, EngineError> {
        if cfg.t_stop.is_zero() || cfg.t_max.is_zero() {
            return Err(other("t_stop and t_max must be > 0"));
        }
        if let Some(l) = cfg.lattice {
            if l.is_zero() || !l.as_fs().is_multiple_of(cfg.t_max.as_fs()) {
                return Err(other(format!(
                    "TMAX {} must divide the grant lattice {l} (ADR-003 D2)",
                    cfg.t_max
                )));
            }
        }
        let lib = init_lib().map_err(other)?;

        // Inputs, sources and glue.
        let mut inputs = BTreeMap::new();
        let mut sources = BTreeMap::new();
        let mut glue = Vec::new();
        for b in &cfg.inputs {
            let (level, volts) = match &b.drive {
                Drive::Digital { model, initial } => (*initial, model.level_volts(*initial)),
                Drive::Analog { initial, .. } => (Level::Low, *initial),
            };
            let key = b.source.to_ascii_lowercase();
            if !key.starts_with('v') {
                return Err(other(format!("source {} must be a V element", b.source)));
            }
            if sources.insert(key, Wave::constant(volts)).is_some() {
                return Err(other(format!("source {} bound twice", b.source)));
            }
            if let Some(node) = &b.glue_node {
                let zout = match &b.drive {
                    Drive::Digital { model, .. } => model.zout,
                    Drive::Analog { .. } => 0.0,
                };
                if zout > 0.0 {
                    let int = format!("{}_int", b.source);
                    glue.push(format!("{} {int} 0 dc 0 external", b.source));
                    glue.push(format!("R_bbz_{} {int} {node} {zout:e}", b.source));
                } else {
                    glue.push(format!("{} {node} 0 dc 0 external", b.source));
                }
            }
            let pin = InputPin {
                source: b.source.to_ascii_lowercase(),
                drive: b.drive.clone(),
                level,
                volts,
            };
            if inputs.insert(b.pin.clone(), pin).is_some() {
                return Err(other(format!("input pin {} bound twice", b.pin)));
            }
        }
        let mut outputs = BTreeMap::new();
        let mut watch: Vec<String> = Vec::new();
        let mut add_watch = |n: &str| {
            let n = n.to_ascii_lowercase();
            if !watch.contains(&n) {
                watch.push(n);
            }
        };
        for o in &cfg.outputs {
            add_watch(&o.node);
            if outputs
                .insert(o.pin.clone(), o.node.to_ascii_lowercase())
                .is_some()
            {
                return Err(other(format!("output pin {} bound twice", o.pin)));
            }
        }
        for p in &cfg.probes {
            add_watch(p);
        }

        // Netlist.
        let mut lines = vec![format!("* breadboard engine {}", cfg.name)];
        for raw in cfg.netlist.lines() {
            let l = raw.trim();
            let lower = l.to_ascii_lowercase();
            if lower == ".end" {
                break;
            }
            if lower.starts_with(".tran")
                || lower.starts_with(".control")
                || lower.starts_with(".endc")
            {
                return Err(other(format!(
                    "netlist must not contain `{l}`: the engine owns the analysis"
                )));
            }
            lines.push(raw.to_string());
        }
        lines.extend(glue);
        // The analysis runs one TMAX past t_stop: ngspice finishes (without a
        // sync callback) on the point at its final time, so a grant that ends
        // exactly at t_stop must still be a point where the solver can park.
        lines.push(format!(
            ".tran {} {} 0 {}",
            spice_time(cfg.t_max),
            spice_time(cfg.t_stop.saturating_add(cfg.t_max)),
            spice_time(cfg.t_max)
        ));
        lines.push(".end".to_string());

        // Claim the singleton and reset the shared state.
        {
            let mut st = state();
            if st.active {
                return Err(other(
                    "another NgspiceEngine is alive in this process (ngspice is a singleton)",
                ));
            }
            reset_state(&mut st);
            st.active = true;
            st.watch = watch.clone();
            st.sources = sources;
            st.handoff_spin = cfg.handoff_spin;
            st.lattice_fs = cfg.lattice.map_or(0, Time::as_fs);
        }
        let mut engine = NgspiceEngine {
            name: cfg.name.clone(),
            lib,
            now: Time::ZERO,
            t_stop: cfg.t_stop,
            lookahead: cfg.lookahead,
            inputs,
            outputs,
            hist: History {
                times_fs: Vec::new(),
                times_s: Vec::new(),
                values: vec![Vec::new(); watch.len()],
            },
            watch,
            breakpoints: BTreeSet::new(),
            lattice: cfg.lattice,
            solver_points: 0,
            failed: false,
        };
        // From here on, Drop releases the singleton.
        engine.start(&lines)?;
        Ok(engine)
    }

    fn start(&mut self, lines: &[String]) -> Result<(), EngineError> {
        log().clear();
        let owned: Vec<CString> = lines
            .iter()
            .map(|l| CString::new(l.as_str()))
            .collect::<Result<_, _>>()
            .map_err(|e| other(e.to_string()))?;
        let mut ptrs: Vec<*mut c_char> = owned.iter().map(|c| c.as_ptr().cast_mut()).collect();
        ptrs.push(std::ptr::null_mut());
        // SAFETY: NULL-terminated array of NUL-terminated lines; ngspice copies.
        let rc = unsafe { (self.lib.circ)(ptrs.as_mut_ptr()) };
        let errs = logged_errors();
        if rc != 0 || !errs.is_empty() {
            return Err(other(format!(
                "ngspice rejected the netlist (rc {rc}): {}",
                errs.join(" | ")
            )));
        }
        command(self.lib, "bg_run").map_err(other)?;
        // Wait for the solver to park at t = 0.
        let deadline = Instant::now() + THREAD_TIMEOUT;
        let mut st = state();
        loop {
            if st.parked == Some(0) || st.error.is_some() || (st.bg_started && !st.bg_running) {
                break;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                drop(st);
                return Err(other(format!(
                    "ngspice did not reach t = 0: {}",
                    log().join(" | ")
                )));
            }
            st = WAKE
                .wait_timeout(st, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        drop(st);
        self.take_boundary(Time::ZERO)
    }

    /// After the solver parked at `t`: check errors, take the new samples.
    fn take_boundary(&mut self, t: Time) -> Result<(), EngineError> {
        let mut st = state();
        if let Some(e) = st.error.clone() {
            drop(st);
            self.failed = true;
            return Err(other(format!(
                "{e}; ngspice: {}",
                logged_errors().join(" | ")
            )));
        }
        if st.parked != Some(st.seq) {
            drop(st);
            self.failed = true;
            return Err(other(format!(
                "ngspice stopped before reaching {t}: {}",
                logged_errors().join(" | ")
            )));
        }
        let reading = st.reading.take();
        let parked_s = st.parked_time_s;
        for w in st.sources.values_mut() {
            w.prune(t);
        }
        let tf = t.as_secs_f64();
        st.corners.retain(|&c| c > tf);
        drop(st);

        let r = reading.ok_or_else(|| other("no samples at boundary"))?;
        if r.times.is_empty() {
            self.failed = true;
            return Err(other(format!("no new solver point at {t}")));
        }
        let t_s = t.as_secs_f64();
        if (parked_s - t_s).abs() > grant_tolerance_s(t_s) {
            self.failed = true;
            return Err(other(format!(
                "solver parked at {parked_s:e} s, expected {t}"
            )));
        }
        self.solver_points += r.times.len();
        // Keep the previous boundary point plus the new points.
        let h = &mut self.hist;
        let keep_from = h.times_fs.len().saturating_sub(1);
        h.times_fs.drain(..keep_from);
        h.times_s.drain(..keep_from);
        for v in &mut h.values {
            v.drain(..keep_from);
        }
        // Interior points are rounded to fs and kept inside [previous
        // boundary, t] (rounding can push a point within one ulp of a
        // boundary past it once ulp > 1 fs).
        let lo = self.now.as_fs();
        let hi = t.as_fs();
        h.times_fs
            .extend(r.times.iter().map(|&s| secs_to_fs(s).clamp(lo, hi)));
        h.times_s.extend_from_slice(&r.times);
        for (dst, src) in h.values.iter_mut().zip(r.values) {
            dst.extend(src);
        }
        // The last point is the boundary itself, at exactly `t`.
        if let Some(last) = h.times_fs.last_mut() {
            *last = t.as_fs();
        }
        self.now = t;
        Ok(())
    }

    fn watch_index(&self, node: &str) -> Result<usize, EngineError> {
        let n = node.to_ascii_lowercase();
        self.watch
            .iter()
            .position(|w| *w == n)
            .ok_or_else(|| other(format!("node {node} is not an output or probe")))
    }

    /// Current simulation time (the last grant boundary).
    pub fn now(&self) -> Time {
        self.now
    }

    /// Voltage of a watched node (output or probe) at the current boundary.
    pub fn voltage(&self, node: &str) -> Result<f64, EngineError> {
        let i = self.watch_index(node)?;
        self.hist.values[i]
            .last()
            .copied()
            .ok_or_else(|| other("no samples yet"))
    }

    /// Voltage of a watched node at `t`, for `t` between the previous grant
    /// boundary and the current one (inclusive). Linear interpolation between
    /// the two solver points around `t`; a pure function of the solver points,
    /// so it is deterministic. Exactly at a solver point (e.g. a boundary) it
    /// returns that point's value.
    pub fn sample(&self, node: &str, t: Time) -> Result<f64, EngineError> {
        let i = self.watch_index(node)?;
        let h = &self.hist;
        let (first, last) = match (h.times_fs.first(), h.times_fs.last()) {
            (Some(&a), Some(&b)) => (a, b),
            _ => return Err(other("no samples yet")),
        };
        let tf = t.as_fs();
        if tf < first || tf > last {
            return Err(other(format!(
                "sample time {t} outside the retained window [{}, {}]",
                Time::from_fs(first),
                Time::from_fs(last)
            )));
        }
        let vals = &h.values[i];
        match h.times_fs.binary_search(&tf) {
            Ok(k) => {
                // Several points can round to the same fs; take the last.
                let mut k = k;
                while k + 1 < h.times_fs.len() && h.times_fs[k + 1] == tf {
                    k += 1;
                }
                Ok(vals[k])
            }
            Err(k) => {
                // first < tf < last, so 1 <= k < len.
                let (t0, t1) = (h.times_s[k - 1], h.times_s[k]);
                let (v0, v1) = (vals[k - 1], vals[k]);
                if t1 <= t0 {
                    return Ok(v1);
                }
                let x = (t.as_secs_f64() - t0) / (t1 - t0);
                Ok(v0 + (v1 - v0) * x.clamp(0.0, 1.0))
            }
        }
    }

    /// Solver points (time, value) of a watched node retained since the
    /// previous boundary, for diagnostics.
    pub fn solver_window(&self, node: &str) -> Result<Vec<(Time, f64)>, EngineError> {
        let i = self.watch_index(node)?;
        Ok(self
            .hist
            .times_fs
            .iter()
            .zip(&self.hist.values[i])
            .map(|(&t, &v)| (Time::from_fs(t), v))
            .collect())
    }

    /// Like [`Self::solver_window`], but with ngspice's raw `f64` solver
    /// times (s) instead of times rounded to fs: for bit-exact comparisons of
    /// solver point sequences (the ADR-003 D2 partition gate).
    pub fn solver_window_raw(&self, node: &str) -> Result<Vec<(f64, f64)>, EngineError> {
        let i = self.watch_index(node)?;
        Ok(self
            .hist
            .times_s
            .iter()
            .zip(&self.hist.values[i])
            .map(|(&t, &v)| (t, v))
            .collect())
    }

    /// Total solver points accepted so far (including t = 0).
    pub fn solver_points(&self) -> usize {
        self.solver_points
    }

    /// Register a kernel-visible breakpoint: `next_safe_time` will not promise
    /// past it, so the kernel ends a grant there.
    pub fn add_breakpoint(&mut self, t: Time) {
        if t > self.now {
            self.breakpoints.insert(t);
        }
    }

    /// Push a level change of a digital input pin at absolute time `at >=
    /// now`. The source ramps from its value at `at` to the level's voltage
    /// over the model's rise/fall time.
    pub fn push_edge(&mut self, pin: &str, level: Level, at: Time) -> Result<(), EngineError> {
        if at < self.now {
            return Err(EngineError::Protocol(format!(
                "edge on {pin} at {at} is before now = {}",
                self.now
            )));
        }
        let p = self
            .inputs
            .get_mut(pin)
            .ok_or_else(|| other(format!("unknown input pin {pin}")))?;
        let Drive::Digital { model, .. } = p.drive else {
            return Err(other(format!("pin {pin} is analog; use push_value")));
        };
        if p.level == level {
            return Ok(());
        }
        p.level = level;
        let dur = if level == Level::High {
            model.rise
        } else {
            model.fall
        };
        let target = model.level_volts(level);
        p.volts = target;
        let source = p.source.clone();
        self.push_ramp(&source, at, dur, target);
        Ok(())
    }

    /// Push a new value of an analog input pin at absolute time `at >= now`
    /// (ramp of the pin's `slew`).
    pub fn push_value(&mut self, pin: &str, volts: f64, at: Time) -> Result<(), EngineError> {
        if at < self.now {
            return Err(EngineError::Protocol(format!(
                "value on {pin} at {at} is before now = {}",
                self.now
            )));
        }
        let p = self
            .inputs
            .get_mut(pin)
            .ok_or_else(|| other(format!("unknown input pin {pin}")))?;
        let Drive::Analog { slew, .. } = p.drive else {
            return Err(other(format!("pin {pin} is digital; use push_edge")));
        };
        if p.volts.to_bits() == volts.to_bits() {
            return Ok(());
        }
        p.volts = volts;
        let source = p.source.clone();
        self.push_ramp(&source, at, slew, volts);
        Ok(())
    }

    fn push_ramp(&mut self, source: &str, at: Time, dur: Time, target: f64) {
        let mut st = state();
        if let Some(w) = st.sources.get_mut(source) {
            w.push(at, dur, target);
        }
        let mut corners = Vec::new();
        if at > self.now {
            corners.push(at.as_secs_f64());
        }
        if !dur.is_zero() {
            corners.push(at.saturating_add(dur).as_secs_f64());
        }
        // ADR-003 D2: release the breakpoints at the lattice point that
        // opens the cell (kL, (k+1)L] holding `at` (never before now).
        let release = match self.lattice {
            Some(l) => {
                let open = at.as_fs().div_ceil(l.as_fs()).saturating_sub(1) * l.as_fs();
                Time::from_fs(open).max(self.now)
            }
            None => self.now,
        };
        let release_s = release.as_secs_f64();
        for c in corners {
            st.corners.push(c);
            st.new_bkpts.push((release_s, c));
        }
        st.corners.sort_by(f64::total_cmp);
        st.corners.dedup();
    }
}

impl Engine for NgspiceEngine {
    fn name(&self) -> &str {
        &self.name
    }

    fn next_safe_time(&mut self, now: Time) -> Time {
        let mut t = self.t_stop;
        if let Some(l) = self.lookahead {
            t = t.min(now.saturating_add(l));
        }
        if let Some(&b) = self
            .breakpoints
            .range(Time::from_fs(now.as_fs() + 1)..)
            .next()
        {
            t = t.min(b);
        }
        // Must be > now even at the horizon; advance_to then reports the error.
        t.max(now.saturating_add(Time::from_fs(1)))
    }

    fn advance_to(&mut self, t: Time) -> Result<(), EngineError> {
        if self.failed {
            return Err(other("engine failed earlier"));
        }
        if t <= self.now {
            return Err(EngineError::Protocol(format!(
                "advance_to({t}) not after now = {}",
                self.now
            )));
        }
        if t > self.t_stop {
            return Err(EngineError::Protocol(format!(
                "advance_to({t}) past the transient end t_stop = {}",
                self.t_stop
            )));
        }
        let mut st = state();
        st.seq += 1;
        st.target_fs = t.as_fs();
        st.target_s = t.as_secs_f64();
        let seq = st.seq;
        GRANT_SEQ.store(seq, Ordering::Release);
        WAKE.notify_all();
        let spin = st.handoff_spin;
        if !spin.is_zero() {
            drop(st);
            spin_until(spin, || PARKED_SEQ.load(Ordering::Acquire) == seq);
            st = state();
        }
        while st.parked != Some(seq) && st.error.is_none() && st.bg_running {
            st = WAKE.wait(st).unwrap_or_else(|e| e.into_inner());
        }
        drop(st);
        self.breakpoints = self.breakpoints.split_off(&Time::from_fs(t.as_fs() + 1));
        self.take_boundary(t)
    }

    fn read_outputs(&mut self) -> Result<PinSignals, EngineError> {
        let mut out = PinSignals::new();
        for (pin, node) in &self.outputs {
            out.insert(pin.clone(), PinSignal::analog(self.voltage(node)?));
        }
        Ok(out)
    }

    fn sample_output(&self, pin: &str, t: Time) -> Option<PinSignal> {
        let node = self.outputs.get(pin)?;
        self.sample(node, t).ok().map(PinSignal::analog)
    }

    fn apply_inputs(&mut self, inputs: &PinSignals) -> Result<(), EngineError> {
        let now = self.now;
        for (pin, sig) in inputs {
            if !self.inputs.contains_key(pin) {
                return Err(other(format!("unknown input pin {pin}")));
            }
            match *sig {
                // ADR-002 Amendment 2: an edge delivered before this engine
                // advances (Gauss-Seidel, edge_time in (now, g]) starts at its
                // true edge_time. An edge delivered at a boundary after the
                // engine already simulated past it (edge_time <= now) starts
                // at now (D4.4). A repeated snapshot with an unchanged level
                // is a no-op.
                PinSignal::Digital { level, edge_time } => {
                    self.push_edge(pin, level, edge_time.max(now))?
                }
                PinSignal::Analog { volts } => self.push_value(pin, volts, now)?,
            }
        }
        Ok(())
    }
}

impl Drop for NgspiceEngine {
    fn drop(&mut self) {
        {
            let mut st = state();
            st.abort = true;
            WAKE.notify_all();
        }
        // With `abort` set the callbacks no longer block, so `bg_halt`
        // (ft_intrpt -> E_PAUSE after the next accepted point) returns.
        // SAFETY: plain FFI call.
        if unsafe { (self.lib.running)() } {
            let _ = command(self.lib, "bg_halt");
        }
        let deadline = Instant::now() + THREAD_TIMEOUT;
        let mut st = state();
        while st.bg_running && Instant::now() < deadline {
            st = WAKE
                .wait_timeout(st, Duration::from_millis(50))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        drop(st);
        let _ = command(self.lib, "remcirc");
        let _ = command(self.lib, "destroy all");
        let mut st = state();
        reset_state(&mut st);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wave_ramps_and_replaces() {
        let mut w = Wave::constant(0.0);
        w.push(Time::from_us(1), Time::from_ns(10), 2.0);
        assert_eq!(w.value_at(0.5e-6), 0.0);
        assert_eq!(w.value_at(1e-6), 0.0);
        assert!((w.value_at(1e-6 + 5e-9) - 1.0).abs() < 1e-9);
        assert_eq!(w.value_at(2e-6), 2.0);
        // Interrupt mid-ramp: starts from the current value.
        let mut w2 = w.clone();
        w2.push(Time::from_us(1) + Time::from_ns(5), Time::from_ns(10), 0.0);
        let v = w2.value_at(1e-6 + 5e-9);
        assert!((v - 1.0).abs() < 1e-9);
        assert_eq!(w2.value_at(1e-6 + 15e-9), 0.0);
        w2.prune(Time::from_us(3));
        assert_eq!(w2.segs.len(), 1);
        assert_eq!(w2.value_at(4e-6), 0.0);
    }

    #[test]
    fn time_conversions() {
        assert_eq!(secs_to_fs(0.5e-3), 500_000_000_000);
        assert_eq!(secs_to_fs(3e-3), 3_000_000_000_000);
        assert_eq!(secs_to_fs(-1.0), 0);
        assert_eq!(spice_time(Time::from_ns(1)), "1000000e-15");
    }

    #[test]
    fn grant_tolerance_covers_one_ulp_at_large_times() {
        // The verifier's failing case: 2.304 s lands one ulp high and rounds
        // to grant + 1 fs; it must still count as "at the grant".
        let t = Time::from_ms(2304).as_secs_f64();
        let landed = f64::from_bits(t.to_bits() + 1);
        assert!((landed - t).abs() <= grant_tolerance_s(t));
        // Small times keep the half-femtosecond resolution.
        assert_eq!(grant_tolerance_s(1e-3), 0.5e-15);
        // A real overshoot (a whole step) is still caught.
        assert!(1e-6 > grant_tolerance_s(10_000.0));
    }
}
