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
//! # Outputs
//!
//! [`OutputBinding`]s report a node voltage (`Analog`) at each boundary and,
//! through `sample_output`, at any instant of the last grant (interpolated
//! between solver points). [`DigitalOutputBinding`]s threshold a node with a
//! consumer's `digital_in` model at every accepted solver point and report
//! each level change in the output trace (ADR-003 D3): the analog → digital
//! crossing of a feedback loop or of an analog-driven MCU input.
//! [`AdcOutputBinding`]s push a node to a consumer's ADC (`adc_in`, ADR-003
//! D6): at the lattice points where the delivered whole millivolts change
//! (or, [`AdcPush::EveryLattice`], at every lattice point), as `Analog`
//! transitions at those lattice points; `advance_until` stops at the first
//! push of a watched ADC pin.
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

use bbcore::{
    AdcIn, DigitalIn, DigitalOut, Engine, EngineError, Level, PinSignal, PinSignals, Time,
    Transition,
};

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
    /// A bench output (ADR-010 D4): an analog value (volts, or the 0/1 of a
    /// switch's control) that changes at the time of its trace entry (never
    /// before the engine's `now`), by a ramp of [`TIMED_RAMP`] with a corner
    /// and a breakpoint at its end. Unlike [`Drive::Analog`], a value
    /// delivered in a trace before this engine advances takes effect at its
    /// own time, not at the boundary. (A trigger's time is often an MCU edge
    /// at a lattice point plus 1 fs, closer to the lattice point than the
    /// solver resolves a corner; the ramp's end is resolved.)
    Timed {
        /// Value before the first exchange.
        initial: f64,
    },
}

/// The ramp of a [`Drive::Timed`] change (ADR-010 D4), as the 1 ns edges of
/// a package test vector's control.
pub const TIMED_RAMP: Time = Time::from_ns(1);

/// How a [`TableSource`] interpolates between its points (ADR-010 D3).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Interp {
    /// A straight line between neighbouring points (as a PWL source).
    Linear,
    /// Each point's value holds until the next point (a step there).
    Hold,
}

/// The longest linear [`TableSource`] whose points are all ngspice
/// breakpoints (ADR-010 Amendment 1): the `[[pwl]]` limit below which a
/// bench warns, so a playback of at most this many points gives the result
/// of the same points as an inline PWL card (which sets a breakpoint at
/// every point, ADR-010 G2). A longer linear table's points are corners the
/// steps are clamped to, without breakpoints: a breakpoint restarts the
/// integration with steps of a tenth of the gap, which for data on every
/// lattice point multiplies the solver points by about four (measured on
/// examples/bench: 1 s at 100 ksps added 3.2 s with them, 0.4 s without).
pub const TABLE_BREAKPOINTS_MAX: usize = 1_000;

/// A bench table (ADR-010 D3, D4): the value of an `EXTERNAL` voltage or
/// current source of the netlist (`V<name> n+ n- dc 0 external`, `I<name>
/// n+ n- dc 0 external`) as a function of time, served by the engine's
/// callbacks from the table with a cursor (O(1) per solver point), never
/// by a PWL card. Before the first point the first value holds, after the
/// last the last. Every point after `t = 0` is a corner: the solver steps
/// are clamped to land on it. The points of a hold table and of a table of
/// at most [`TABLE_BREAKPOINTS_MAX`] points also become ngspice breakpoints,
/// each at the accepted point that opens the lattice cell before the one
/// holding it (a pure function of the solver points, so of no grant
/// partition).
#[derive(Clone, Debug, PartialEq)]
pub struct TableSource {
    /// The element name (`V...` or `I...`; case-insensitive).
    pub source: String,
    /// `(t, value)`, strictly increasing in `t`; value in V or A.
    pub points: Vec<(Time, f64)>,
    pub interp: Interp,
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

/// Binds a kernel output pin to a circuit node through the `digital_in` pin
/// model of the consuming (digital) engine (ADR-003 D3, `spec/pin-model.md`).
///
/// The engine thresholds the node voltage with [`DigitalIn::threshold`] at
/// every accepted solver point and reports each level change as a `Digital`
/// transition of its output trace ([`bbcore::Engine::read_output_trace`]).
/// The crossing time is linearly interpolated between the two straddling
/// solver points and rounded up to whole femtoseconds: a pure function of
/// the solver points, which a grant lattice makes partition independent
/// (ADR-003 D2). The initial level (t = 0) is `threshold(Low, v(0))`. The
/// engine also adds the model's `cin` as a capacitor from `node` to ground
/// (glue `C_bbin_<pin>`, the pin name with every character other than an
/// ASCII letter or digit replaced by `_`).
#[derive(Clone, Debug, PartialEq)]
pub struct DigitalOutputBinding {
    /// Engine-side pin name (what the kernel binds the digital net to).
    pub pin: String,
    /// Circuit node whose voltage is thresholded.
    pub node: String,
    /// The consumer's input model: `vih`, `vil` (hysteresis, `vil < vih`)
    /// and `cin` (F, `>= 0`).
    pub model: DigitalIn,
}

/// Binds a kernel output pin to a circuit node through the `adc_in` pin
/// model of the consuming (CPU) engine's ADC (ADR-003 D6, Amendment 10,
/// `spec/pin-model.md`).
///
/// A push model: at every lattice point `kL` (the engine needs a grant
/// lattice, and the lattice clamp puts a solver point on each) the engine
/// reads the node, delivers [`AdcIn::delivered_microvolts`] of it and, by
/// [`NgspiceConfig::adc_push`], emits an `Analog` transition at `kL` of its
/// output trace. The pin's boundary value is the last delivered value.
/// Adds no glue: the ADC input is ideal (no load).
#[derive(Clone, Debug, PartialEq)]
pub struct AdcOutputBinding {
    /// Engine-side pin name (what the kernel binds the ADC input net to).
    pub pin: String,
    /// Circuit node the ADC reads.
    pub node: String,
    /// The consumer's ADC input model.
    pub model: AdcIn,
}

/// When an `adc_in` output emits a transition (ADR-003 Amendment 10).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum AdcPush {
    /// At the lattice points where the push key ([`AdcIn::push_key`]: the
    /// 12-bit code, or whole millivolts for the stock model) changes,
    /// delivering [`AdcIn::key_microvolts`]. The default.
    #[default]
    OnChange,
    /// At every lattice point, delivering the clamped value in whole
    /// microvolts: the reference the step 9 equivalence gate compares with
    /// (`bb run --adc-push every`). A trace entry per pin per lattice point,
    /// so an early-stop watch on such a pin stops at the first lattice point.
    EveryLattice,
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
    /// Thresholded digital output pins (ADR-003 D3): a node voltage seen
    /// through a consumer's `digital_in` model, reported as an ordered trace
    /// of level changes.
    pub digital_outputs: Vec<DigitalOutputBinding>,
    /// ADC output pins (ADR-003 D6): a node voltage pushed to a consumer's
    /// ADC at lattice points, as `Analog` transitions of the output trace.
    /// Needs [`NgspiceConfig::lattice`].
    pub adc_outputs: Vec<AdcOutputBinding>,
    /// When the ADC outputs emit transitions (default: on a change of the
    /// delivered whole millivolts).
    pub adc_push: AdcPush,
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
    /// The progress watchdog (ADR-015 D1).
    pub watchdog: Watchdog,
    /// The lattice snap (ADR-015 Amendment 4, the H2 fix of the SAADC
    /// stall; default `true`): a solver step that would land more than the
    /// grant tolerance but less than [`snap_s`] short of the grant or of the
    /// next lattice point lands on it, so ngspice's TMAX steps never drift
    /// off the lattice into a sub-femtosecond step to `kL`. `false` is the
    /// clamp before H2, kept only to reproduce the stall (`bb run
    /// --ngspice-lattice-snap off`, `make ngspice-stall`).
    pub lattice_snap: bool,
    /// Bench tables (ADR-010 D3, D4): `EXTERNAL` sources whose values come
    /// from a table handed over once, so no analog value crosses the kernel
    /// boundary per solver point.
    pub tables: Vec<TableSource>,
}

/// The engine's deterministic progress watchdog (ADR-015 D1,
/// `spec/time-grant.md` "What engines MUST NOT do"). Counts of accepted
/// solver points, never wall time: a run either completes identically or
/// fails identically, with [`EngineError::NoProgress`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Watchdog {
    /// At most this many accepted points in one lattice cell `((k-1)L, kL]`
    /// (needs [`NgspiceConfig::lattice`]; without a lattice only
    /// `max_points_without_progress` applies). E3's PWM case takes about 60
    /// per edge and at most a few thousand per cell (ADR-015 D1).
    pub max_points_per_cell: u64,
    /// At most this many consecutive accepted points whose time, rounded to
    /// whole fs, does not advance.
    pub max_points_without_progress: u64,
}

impl Default for Watchdog {
    /// ADR-015's defaults: 200 000 points per cell, 10 000 points without
    /// progress.
    fn default() -> Watchdog {
        Watchdog {
            max_points_per_cell: 200_000,
            max_points_without_progress: 10_000,
        }
    }
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
            digital_outputs: Vec::new(),
            adc_outputs: Vec::new(),
            adc_push: AdcPush::OnChange,
            probes: Vec::new(),
            handoff_spin: Duration::ZERO,
            lattice: None,
            watchdog: Watchdog::default(),
            lattice_snap: true,
            tables: Vec::new(),
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

/// A [`TableSource`] as the callbacks serve it (ADR-010 D3, D4).
#[derive(Clone, Debug)]
struct Table {
    t_s: Vec<f64>,
    v: Vec<f64>,
    hold: bool,
    /// Per point, when its breakpoint is released: the first accepted
    /// point at or after this time (s).
    release_s: Vec<f64>,
    /// The last point at or before the last accepted solver time.
    cur: usize,
    /// The next point whose breakpoint is not yet released.
    next_bk: usize,
}

impl Table {
    fn new(t: &TableSource, lattice_fs: u64) -> Table {
        let t_s: Vec<f64> = t.points.iter().map(|(x, _)| x.as_secs_f64()).collect();
        let release_s = t
            .points
            .iter()
            .enumerate()
            .map(|(k, (x, _))| {
                let fs = x.as_fs();
                if lattice_fs > 0 {
                    // The lattice point opening the cell before the one
                    // holding the point (so the breakpoint is in ngspice's
                    // table before the step decision at the cell's start).
                    let open = fs.div_ceil(lattice_fs).saturating_sub(2) * lattice_fs;
                    Time::from_fs(open).as_secs_f64()
                } else if k >= 2 {
                    t.points[k - 2].0.as_secs_f64()
                } else {
                    0.0
                }
            })
            .collect();
        // Which points become breakpoints (ADR-010 Amendment 1): every one of
        // a hold table (a step) or of a short table (as an inline PWL sets
        // them, so a short table gives the PWL's result); none of a long
        // linear table, whose corners the clamp lands on anyway.
        let next_bk = if t.interp == Interp::Hold || t.points.len() <= TABLE_BREAKPOINTS_MAX {
            t.points
                .iter()
                .position(|(x, _)| !x.is_zero())
                .unwrap_or(t.points.len())
        } else {
            t.points.len()
        };
        Table {
            t_s,
            v: t.points.iter().map(|(_, v)| *v).collect(),
            hold: t.interp == Interp::Hold,
            release_s,
            cur: 0,
            next_bk,
        }
    }

    /// The value at solver time `t` (s): a pure function of `t`; the cursor
    /// only makes the search O(1) for the solver's forward times. A `hold`
    /// step is left-continuous, as an input ramp is (`Seg::value_at`): at
    /// its point the old value, after it the new one, so the step falls in
    /// the solver's first (short) step after the breakpoint.
    fn value_at(&self, t: f64) -> f64 {
        let n = self.t_s.len();
        if t < self.t_s[0] {
            return self.v[0];
        }
        let mut i = if t >= self.t_s[self.cur] {
            self.cur
        } else {
            self.t_s.partition_point(|&x| x <= t).saturating_sub(1)
        };
        while i + 1 < n && self.t_s[i + 1] <= t {
            i += 1;
        }
        if self.hold {
            // Left-continuous: at a later point (within the tolerance the
            // lattice clamp and the park test use), the value before it.
            if i > 0 && t - self.t_s[i] <= grant_tolerance_s(self.t_s[i]) {
                return self.v[i - 1];
            }
            return self.v[i];
        }
        if i + 1 == n {
            return self.v[i];
        }
        let x = (t - self.t_s[i]) / (self.t_s[i + 1] - self.t_s[i]);
        self.v[i] + x * (self.v[i + 1] - self.v[i])
    }

    /// The solver accepted a point at `time`: move the cursor, and collect
    /// the breakpoints due (released at or before `time`).
    fn accept(&mut self, time: f64, tol: f64, due: &mut Vec<f64>) {
        let n = self.t_s.len();
        while self.cur + 1 < n && self.t_s[self.cur + 1] <= time {
            self.cur += 1;
        }
        while self.next_bk < n && self.release_s[self.next_bk] <= time + tol {
            due.push(self.t_s[self.next_bk]);
            self.next_bk += 1;
        }
    }

    /// The first corner after `time` (plus the minimum corner distance).
    fn next_corner(&self, time: f64) -> Option<f64> {
        let mut i = self.cur;
        while i < self.t_s.len() && self.t_s[i] <= time + CORNER_MIN_S {
            i += 1;
        }
        self.t_s.get(i).copied()
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
    /// Bench tables (ADR-010 D3) by lower-case source name (`v...`, `i...`).
    tables: BTreeMap<String, Table>,
    /// Upcoming ramp corners (s, sorted): solver steps are clamped to land on
    /// them.
    corners: Vec<f64>,
    /// Breakpoints to hand to `ngSpice_SetBkpt` on the solver thread, as
    /// `(release_s, bkpt_s)` in push order: each is set at the first
    /// accepted point at or after `release_s` (ADR-003 D2).
    new_bkpts: Vec<(f64, f64)>,
    /// Grant lattice in fs (0: none).
    lattice_fs: u64,
    /// [`NgspiceConfig::lattice_snap`] (ADR-015 Amendment 4).
    lattice_snap: bool,
    /// The early-stop watch of the current `advance_until` grant (ADR-003
    /// D5); `None` for `advance_to`.
    until: Option<Until>,
    /// First fatal problem seen by a callback.
    error: Option<String>,
    /// [`NgspiceConfig::handoff_spin`].
    handoff_spin: Duration,
    /// The progress watchdog's counters (ADR-015 D1).
    wd: WatchCount,
    /// The watchdog tripped: the accepted point's time (fs) and the detail.
    stall: Option<(u64, String)>,
}

/// The progress watchdog's limits and counters (ADR-015 D1). Updated at
/// every accepted solver point on the solver thread; a pure function of the
/// sequence of accepted points.
struct WatchCount {
    max_cell: u64,
    max_flat: u64,
    /// The lattice cell of the last point (`ceil(point / L)`; 0 without a
    /// lattice).
    cell: u64,
    /// Accepted points in that cell so far.
    cell_points: u64,
    /// The last point's time rounded to fs, and how many consecutive points
    /// since did not advance it.
    last_fs: u64,
    flat: u64,
}

impl WatchCount {
    const fn new(w: Watchdog) -> WatchCount {
        WatchCount {
            max_cell: w.max_points_per_cell,
            max_flat: w.max_points_without_progress,
            cell: u64::MAX,
            cell_points: 0,
            last_fs: 0,
            flat: 0,
        }
    }

    /// Count the point just accepted at `time` (s). `Some(detail)` when a
    /// limit is exceeded.
    fn accept(&mut self, time: f64, lattice_fs: u64) -> Option<String> {
        let fs = point_fs(time, lattice_fs);
        let rounded = secs_to_fs(time);
        if self.cell == u64::MAX || rounded > self.last_fs {
            self.flat = 0;
        } else {
            self.flat += 1;
        }
        self.last_fs = self.last_fs.max(rounded);
        if self.flat >= self.max_flat {
            return Some(format!(
                "{} consecutive accepted solver points did not advance past {} (the watchdog \
                 allows {})",
                self.flat,
                Time::from_fs(self.last_fs),
                self.max_flat
            ));
        }
        if lattice_fs == 0 {
            self.cell = 0;
            return None;
        }
        let cell = fs.div_ceil(lattice_fs);
        if cell != self.cell {
            self.cell = cell;
            self.cell_points = 0;
        }
        self.cell_points += 1;
        if self.cell_points > self.max_cell {
            let hi = cell * lattice_fs;
            return Some(format!(
                "{} accepted solver points in the lattice cell ({}, {}] (the watchdog allows {} \
                 per cell)",
                self.cell_points,
                Time::from_fs(hi.saturating_sub(lattice_fs)),
                Time::from_fs(hi),
                self.max_cell
            ));
        }
        None
    }
}

/// `x` as C's `%.5e` prints it (`7.74300e-02`): ngspice's "Reference value"
/// line, computed from the solver time rather than read from ngspice's log
/// (ngspice prints that line on a CPU-clock timer, so its presence is not
/// deterministic).
fn c_exp5(x: f64) -> String {
    let s = format!("{x:.5e}");
    match s.split_once('e') {
        Some((m, e)) => {
            let (sign, digits) = match e.strip_prefix('-') {
                Some(d) => ('-', d),
                None => ('+', e),
            };
            format!("{m}e{sign}{digits:0>2}")
        }
        None => s,
    }
}

/// The early-stop watch of an `advance_until` grant (ADR-003 D5). The solver
/// thread thresholds the watched nodes at every accepted point, exactly as
/// `NgspiceEngine::threshold_window` later does over the same points; at
/// the first level change, at `t_x`, the grant end becomes `ceil_L(t_x)`.
/// The lattice clamp has put a point on every lattice point, so the solver
/// is at or before that point and parks there, after the same steps as a
/// one-cell run.
struct Until {
    lattice_fs: u64,
    /// The grant start (the previous boundary), exact.
    from_fs: u64,
    pins: Vec<UntilPin>,
    /// Watched ADC outputs (ADR-003 D6): a push at a lattice point is a
    /// change, so the stop is that lattice point.
    adcs: Vec<UntilAdc>,
    /// `<plot>.time`, then `<plot>.<node>` per pin and per ADC, built at
    /// the first point.
    vecs: Option<(CString, Vec<CString>)>,
    /// The previous accepted point: raw time (s) and time in fs as the
    /// window history stores it.
    prev_s: f64,
    prev_fs: u64,
    /// The stop (fs), once found.
    stop_fs: Option<u64>,
}

struct UntilPin {
    node: String,
    model: DigitalIn,
    level: Level,
    prev_v: f64,
}

struct UntilAdc {
    node: String,
    model: AdcIn,
    /// The push key at the grant start.
    key: u32,
    push: AdcPush,
}

/// The lattice point (fs) that the solver time `time` (s) is on, if any:
/// the same predicate as the lattice clamp (`next_lattice_s`), so the
/// accepted point the clamp put on `kL` is recognised as `kL`. A pure
/// function of `time`.
fn lattice_point_of(time: f64, lattice_fs: u64) -> Option<u64> {
    if lattice_fs == 0 {
        return None;
    }
    let near = (secs_to_fs(time) + lattice_fs / 2) / lattice_fs * lattice_fs;
    let near_s = Time::from_fs(near).as_secs_f64();
    ((time - near_s).abs() <= grant_tolerance_s(near_s)).then_some(near)
}

/// The fs time of the accepted solver point at `time` (s): exactly `kL` if
/// the point is on a lattice point ([`lattice_point_of`]), otherwise
/// `time` rounded to fs. A pure function of the raw time, never of whether
/// a grant ends at the point, so the window history, the crossing clamp and
/// the early-stop watch give the same fs in every grant partition (ADR-003
/// D2, Amendment 12). Past ~2.3 s the point the clamp put on `kL` can sit
/// more than 0.5 fs from it while still within the grant tolerance, so
/// rounding alone would give `kL + 1` inside a long grant but `kL` where a
/// grant ends.
fn point_fs(time: f64, lattice_fs: u64) -> u64 {
    lattice_point_of(time, lattice_fs).unwrap_or_else(|| secs_to_fs(time))
}

impl Until {
    /// Threshold the point just accepted at `time` (the grant currently
    /// ends at `target_fs`). Returns the stop once a watched pin changes
    /// level. Runs on the solver thread, between steps, without the state
    /// lock.
    fn step(&mut self, time: f64, target_fs: u64, target_s: f64) -> Result<Option<u64>, String> {
        let lib = ffi::lib(|_| Ok(()))?;
        if self.vecs.is_none() {
            // SAFETY: returns a pointer to the current plot's type name.
            let plot = cstr_lossy(unsafe { (lib.cur_plot)() });
            let name = |v: &str| CString::new(format!("{plot}.{v}")).map_err(|e| e.to_string());
            let nodes = self
                .pins
                .iter()
                .map(|p| name(&p.node))
                .chain(self.adcs.iter().map(|a| name(&a.node)))
                .collect::<Result<_, _>>()?;
            self.vecs = Some((name("time")?, nodes));
        }
        let (tv, nodes) = self.vecs.as_ref().expect("built above");
        let last = |v: &CString| -> Result<f64, String> {
            // SAFETY: as in `read_vectors`; the solver is between steps.
            let info = unsafe { (lib.get_vec_info)(v.as_ptr().cast_mut()) };
            if info.is_null() {
                return Err(format!("ngspice vector {v:?} not found"));
            }
            // SAFETY: non-null pointer to ngspice's vector_info.
            let info = unsafe { &*info };
            let len = usize::try_from(info.v_length).unwrap_or(0);
            if info.v_realdata.is_null() || len == 0 {
                return Err(format!("ngspice vector {v:?} is empty or not real"));
            }
            // SAFETY: v_realdata holds v_length doubles.
            Ok(unsafe { *info.v_realdata.add(len - 1) })
        };
        let t1 = last(tv)?;
        if t1.to_bits() != time.to_bits() {
            return Err(format!(
                "early-stop watch: last time point {t1:e} s is not the accepted point {time:e} s"
            ));
        }
        // The point's fs as the window history will store it: the grant
        // end exactly if it is at the grant, otherwise `point_fs` (a lattice
        // point exactly) kept inside the grant.
        let t1_fs = if target_s - t1 <= grant_tolerance_s(target_s) {
            target_fs
        } else {
            point_fs(t1, self.lattice_fs).clamp(self.from_fs, target_fs)
        };
        let mut first: Option<u64> = None;
        // ADC pushes happen at lattice points only (ADR-003 D6), exactly as
        // `NgspiceEngine::adc_window` later reads the same points.
        let mut adc_stop: Option<u64> = None;
        if let Some(lp) =
            lattice_point_of(t1, self.lattice_fs).filter(|&lp| lp > self.from_fs && lp <= target_fs)
        {
            for (a, v) in self.adcs.iter_mut().zip(&nodes[self.pins.len()..]) {
                let uv = a.model.delivered_microvolts(last(v)?);
                let key = a.model.push_key(uv);
                if a.push == AdcPush::EveryLattice || key != a.key {
                    adc_stop = Some(lp);
                }
                a.key = key;
            }
        }
        for (p, v) in self.pins.iter_mut().zip(nodes) {
            let v1 = last(v)?;
            let new = p.model.threshold(p.level, v1);
            if new != p.level {
                let thr = if new == Level::High {
                    p.model.vih
                } else {
                    p.model.vil
                };
                let lo = self.prev_fs + 1;
                let at = crossing_fs((self.prev_s, p.prev_v), (t1, v1), thr)
                    .max(lo)
                    .min(t1_fs.max(lo))
                    .min(target_fs);
                first = Some(first.map_or(at, |f| f.min(at)));
                p.level = new;
            }
            p.prev_v = v1;
        }
        self.prev_s = t1;
        self.prev_fs = t1_fs;
        let digital_stop =
            first.map(|at| (at.div_ceil(self.lattice_fs) * self.lattice_fs).min(target_fs));
        if let Some(stop) = [digital_stop, adc_stop].into_iter().flatten().min() {
            self.stop_fs = Some(stop);
        }
        Ok(self.stop_fs)
    }
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
            tables: BTreeMap::new(),
            corners: Vec::new(),
            new_bkpts: Vec::new(),
            lattice_fs: 0,
            lattice_snap: true,
            until: None,
            error: None,
            handoff_spin: Duration::ZERO,
            wd: WatchCount::new(Watchdog {
                max_points_per_cell: 200_000,
                max_points_without_progress: 10_000,
            }),
            stall: None,
        }
    }

    fn fail(&mut self, msg: String) {
        if self.error.is_none() {
            self.error = Some(msg);
        }
    }

    /// Limit the next solver step so it lands exactly on the grant, on the
    /// next lattice point (ADR-003 D2) and on the next ramp corner, and
    /// stretch a step that would land just short of the grant or lattice
    /// point onto it (ADR-015 Amendment 4).
    fn clamp(&self, time: f64, delta: &mut f64) {
        let mut boundary = self.target_s;
        if let Some(l) = next_lattice_s(time, self.lattice_fs) {
            boundary = boundary.min(l);
        }
        let mut limit = boundary;
        if let Some(&c) = self.corners.iter().find(|&&c| c > time + CORNER_MIN_S) {
            limit = limit.min(c);
        }
        for t in self.tables.values() {
            if let Some(c) = t.next_corner(time) {
                limit = limit.min(c);
            }
        }
        if time + *delta > limit && limit > time {
            *delta = limit - time;
        } else if self.lattice_snap && limit == boundary && boundary > time {
            // ADR-015 Amendment 4 (H2): a step that would land just short of
            // the grant or lattice point, more than the grant tolerance below
            // it but within the snap distance, lands on it instead. Without
            // this, ngspice's own steps of exactly TMAX = L drift by rounding
            // (`time + TMAX` is rounded, and the lattice points are not TMAX
            // apart in f64) until a point lands just outside the tolerance,
            // and the clamp then adds a sub-femtosecond step to `kL`. Such a
            // micro-step restarts the integration at about 1e-16 s, where a
            // board's Newton iteration can fail to converge at every step
            // (the SAADC stall). A landing within the tolerance is left as
            // it is, so a run that never drifts that far keeps its solver
            // points bit for bit.
            let gap = boundary - (time + *delta);
            if gap > grant_tolerance_s(boundary) && gap <= snap_s(*delta) {
                *delta = boundary - time;
            }
        }
    }
}

/// The largest gap (s) between a step's landing and the grant or lattice
/// point after it that [`State::clamp`] closes by stretching the step:
/// [`CORNER_MIN_S`], and at most 1 % of the step, so a stretch never changes
/// a step by more than ngspice's own truncation-error control would.
fn snap_s(delta: f64) -> f64 {
    CORNER_MIN_S.min(0.01 * delta)
}

/// The first lattice point after the solver time `time` (s), for a lattice
/// of `lattice_fs` (0: none). A time within the grant tolerance of a lattice
/// point counts as on it, so a point that landed a few ulps short of `kL`
/// is not followed by a micro-step to `kL`. A pure function of `time`.
///
/// Every decision compares `time` with the nearest lattice point in `f64`
/// (`time - kL` is exact for close values), never through `time` rounded to
/// fs: a point just outside the tolerance below `kL` that rounds to `kL`
/// must still step to `kL`, as the park test at a grant ending at `kL`
/// (`cb_get_sync`) sees it (ADR-003 Amendment 4).
fn next_lattice_s(time: f64, lattice_fs: u64) -> Option<f64> {
    if lattice_fs == 0 {
        return None;
    }
    let t_fs = secs_to_fs(time);
    let near = (t_fs + lattice_fs / 2) / lattice_fs;
    let near_s = Time::from_fs(near * lattice_fs).as_secs_f64();
    let k = if (time - near_s).abs() <= grant_tolerance_s(near_s) || time > near_s {
        near + 1
    } else {
        near
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
/// Only for points strictly inside a grant and off the lattice
/// ([`point_fs`]); a boundary is always stored as the exact kernel time.
fn secs_to_fs(t: f64) -> u64 {
    if t <= 0.0 {
        0
    } else {
        (t * 1e15).round() as u64
    }
}

/// The time (integer fs, rounded up) at which the straight line through the
/// solver points `(t0, v0)` and `(t1, v1)` (s, V) reaches `threshold`
/// (ADR-003 D3). A pure function of its arguments; the fraction is clamped to
/// the segment.
fn crossing_fs((t0, v0): (f64, f64), (t1, v1): (f64, f64), threshold: f64) -> u64 {
    let frac = if v1 == v0 {
        1.0
    } else {
        ((threshold - v0) / (v1 - v0)).clamp(0.0, 1.0)
    };
    let tx = t0 + frac * (t1 - t0);
    if tx <= 0.0 {
        0
    } else {
        (tx * 1e15).ceil() as u64
    }
}

/// Glue element name of the `cin` capacitor of a digital output pin.
fn cin_glue_name(pin: &str) -> String {
    let p: String = pin
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("C_bbin_{p}")
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

/// An `EXTERNAL` source's name, lower case, for the source maps: on the stack
/// for an ASCII name of up to 64 bytes (every generated name), so the
/// callbacks, which ngspice makes for every source at every load of every
/// Newton iteration, allocate nothing (milestone P2: the `pi` glue's `oe`
/// sources made this path measurable). The same key as before, so no result
/// changes.
fn source_key(p: *const c_char, buf: &mut [u8; 64]) -> std::borrow::Cow<'_, str> {
    if p.is_null() {
        return std::borrow::Cow::Borrowed("");
    }
    // SAFETY: ngspice passes NUL-terminated strings.
    let b = unsafe { CStr::from_ptr(p) }.to_bytes();
    if b.len() <= buf.len() && b.is_ascii() {
        let s = &mut buf[..b.len()];
        s.copy_from_slice(b);
        s.make_ascii_lowercase();
        std::borrow::Cow::Borrowed(std::str::from_utf8(s).expect("ASCII"))
    } else {
        std::borrow::Cow::Owned(cstr_lossy(p).to_ascii_lowercase())
    }
}

unsafe extern "C" fn cb_get_vsrc(
    value: *mut f64,
    time: f64,
    name: *mut c_char,
    _id: c_int,
    _u: *mut c_void,
) -> c_int {
    let mut buf = [0u8; 64];
    let name = source_key(name, &mut buf);
    let name: &str = name.as_ref();
    let mut st = state();
    let v = match (st.sources.get(name), st.tables.get(name)) {
        (Some(w), _) => w.value_at(time),
        (None, Some(t)) => t.value_at(time),
        (None, None) => {
            st.fail(format!(
                "EXTERNAL source {name} is not bound to an input pin or a bench table"
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

/// `EXTERNAL` current sources: bench tables only (ADR-010 D3, a current
/// profile).
unsafe extern "C" fn cb_get_isrc(
    value: *mut f64,
    time: f64,
    name: *mut c_char,
    _id: c_int,
    _u: *mut c_void,
) -> c_int {
    let mut buf = [0u8; 64];
    let name = source_key(name, &mut buf);
    let name: &str = name.as_ref();
    let mut st = state();
    let v = match st.tables.get(name) {
        Some(t) => t.value_at(time),
        None => {
            st.fail(format!("EXTERNAL current source {name} has no bench table"));
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
    if st.parked.is_none() && st.seq == 0 {
        // Before the first park: ngspice 42's transient-op fallback
        // (`optran`, after gmin and source stepping fail) runs a transient of
        // its own and calls this callback with its own time, while the
        // transient's vectors are still empty. Those are not points of the
        // run: let them pass untouched. If the fallback converges, the
        // transient's first point follows; if not, the transient fails at
        // its initial point and the operating-point error names the nodes
        // (ADR-015 D4). A converged operating point never gets here with
        // empty vectors.
        drop(st);
        let empty = time_vector_len().is_none_or(|n| n == 0);
        st = state();
        if empty {
            return 0;
        }
    }
    if loc == 0 {
        // `time` is the point just accepted and dumped to the vectors.
        // The progress watchdog (ADR-015 D1): a count of accepted points,
        // so whether and where it trips is a pure function of the run. On a
        // trip the solver parks here for good (no more points, no more
        // memory) until the engine is dropped.
        let lattice_fs = st.lattice_fs;
        if let Some(why) = st.wd.accept(time, lattice_fs) {
            let at = point_fs(time, lattice_fs);
            let near = lattice_point_of(time, lattice_fs).map_or_else(
                || {
                    if lattice_fs == 0 {
                        String::new()
                    } else {
                        format!(
                            ", in the cell ending at the lattice point {}",
                            Time::from_fs(at.div_ceil(lattice_fs) * lattice_fs)
                        )
                    }
                },
                |lp| format!(", on the lattice point {}", Time::from_fs(lp)),
            );
            let detail = format!(
                "{why}; the solver is at {time:.17e} s{near} (ngspice's \"Reference value : {}\"), \
                 in the grant to {}",
                c_exp5(time),
                Time::from_fs(st.target_fs)
            );
            st.stall = Some((at, detail.clone()));
            st.fail(format!("NoProgress: {detail}"));
            WAKE.notify_all();
            while !st.abort {
                st = WAKE.wait(st).unwrap_or_else(|e| e.into_inner());
            }
            return 0;
        }
        // Early stop (ADR-003 D5): threshold the watched nodes at this point
        // and, at the first level change, end the grant at ceil_L(t_x).
        if st.parked != Some(st.seq) && st.until.as_ref().is_some_and(|u| u.stop_fs.is_none()) {
            let mut u = st.until.take().expect("checked above");
            let (target_fs, target_s) = (st.target_fs, st.target_s);
            drop(st);
            let r = u.step(time, target_fs, target_s);
            st = state();
            match r {
                Ok(Some(stop)) if stop < st.target_fs => {
                    st.target_fs = stop;
                    st.target_s = Time::from_fs(stop).as_secs_f64();
                }
                Ok(_) => {}
                Err(e) => st.fail(e),
            }
            st.until = Some(u);
        }
        let tol = grant_tolerance_s(st.target_s);
        // `target - time` (exact for close values, Sterbenz), not `time >=
        // target - tol` (rounded): the same predicate as the lattice clamp's
        // `|time - kL| <= tol` (`next_lattice_s`), so a point the clamp
        // treats as on a lattice point is also at a grant ending there, and
        // no partition takes a micro-step another does not (ADR-003
        // Amendment 4).
        if st.parked != Some(st.seq) && st.target_s - time <= tol {
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
        // Bench tables (ADR-010 D3): the cursor, and the corners due.
        let mut due = Vec::new();
        for t in st.tables.values_mut() {
            t.accept(time, rtol, &mut due);
        }
        if !due.is_empty() || st.new_bkpts.iter().any(|&(r, _)| r <= time + rtol) {
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

/// The length of the current plot's `time` vector (`None` if it has none).
/// Runs on the solver thread between steps.
fn time_vector_len() -> Option<usize> {
    let lib = ffi::lib(|_| Ok(())).ok()?;
    // SAFETY: returns a pointer to the current plot's type name.
    let plot = cstr_lossy(unsafe { (lib.cur_plot)() });
    let full = CString::new(format!("{plot}.time")).ok()?;
    // SAFETY: as in `read_vectors`.
    let info = unsafe { (lib.get_vec_info)(full.as_ptr().cast_mut()) };
    if info.is_null() {
        return None;
    }
    // SAFETY: non-null pointer to ngspice's vector_info.
    let info = unsafe { &*info };
    if info.v_realdata.is_null() {
        return Some(0);
    }
    Some(usize::try_from(info.v_length).unwrap_or(0))
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
                Some(cb_get_isrc),
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

/// What a failed advance reports from the log: the error lines, or when
/// ngspice printed none (it stops a transient on a singular matrix or a
/// collapsed step with warnings only), the last three distinct warnings,
/// oldest first. "singular matrix: check node qf" names the cause. ngspice
/// spells its texts in more than one case ("Timestep too small" from
/// `doAnalyses`, "timestep too small" elsewhere), so every match ignores
/// ASCII case.
fn failure_lines(log: &[String]) -> Vec<String> {
    let errs: Vec<String> = log
        .iter()
        .filter(|l| l.to_ascii_lowercase().starts_with("stderr error"))
        .cloned()
        .collect();
    if !errs.is_empty() {
        return errs;
    }
    let mut w: Vec<String> = Vec::new();
    for l in log.iter().rev() {
        let lc = l.to_ascii_lowercase();
        let warn = lc.starts_with("stderr warning")
            || lc.contains("singular matrix")
            || lc.contains("timestep too small");
        if warn && !w.contains(l) {
            w.push(l.clone());
            if w.len() == 3 {
                break;
            }
        }
    }
    w.reverse();
    w
}

/// [`failure_lines`] of the current log.
fn failure_detail() -> String {
    let lines = failure_lines(&log());
    if lines.is_empty() {
        "(ngspice printed no error or warning)".into()
    } else {
        lines.join(" | ")
    }
}

/// The nodes an operating-point failure names (ADR-015 D4), in the order
/// ngspice prints them, each once, lower case: `trouble with node "x"`,
/// `singular matrix:  check node x` / `check nodes x and y`, and the rows of
/// the "Last Node Voltages" table marked `*` (not converged). Branch
/// unknowns (`v1#branch`) are not nodes and are left out.
pub fn op_failure_nodes(log: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |n: &str| {
        let n = n.trim().trim_matches('"').to_ascii_lowercase();
        if !n.is_empty() && !n.contains('#') && !out.contains(&n) {
            out.push(n);
        }
    };
    let mut table = false;
    for l in log {
        let body = l
            .strip_prefix("stdout ")
            .or_else(|| l.strip_prefix("stderr "))
            .unwrap_or(l);
        if let Some(i) = body.find("trouble with node \"") {
            let rest = &body[i + "trouble with node \"".len()..];
            if let Some(j) = rest.find('"') {
                add(&rest[..j]);
            }
        }
        if let Some(i) = body.find("check nodes ") {
            let rest = &body[i + "check nodes ".len()..];
            if let Some((a, b)) = rest.split_once(" and ") {
                add(a);
                add(b.split_whitespace().next().unwrap_or_default());
            }
        } else if let Some(i) = body.find("check node ") {
            add(body[i + "check node ".len()..]
                .split_whitespace()
                .next()
                .unwrap_or_default());
        }
        if body.trim_start().starts_with("Last Node Voltages") {
            table = true;
            continue;
        }
        if table {
            let toks: Vec<&str> = body.split_whitespace().collect();
            if toks.is_empty() || toks[0].starts_with("--") || toks[0] == "Node" {
                continue;
            }
            if toks.len() < 3 || toks[1].parse::<f64>().is_err() {
                table = false;
                continue;
            }
            if toks.last() == Some(&"*") {
                add(toks[0]);
            }
        }
    }
    out
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

/// A thresholded digital output (ADR-003 D3).
struct DigitalOutput {
    /// Index into `NgspiceEngine::watch`.
    watch: usize,
    model: DigitalIn,
    level: Level,
    edge_time: Time,
}

/// An ADC output (ADR-003 D6).
struct AdcOutput {
    /// Index into `NgspiceEngine::watch`.
    watch: usize,
    model: AdcIn,
    /// Push key of the last delivered value.
    key: u32,
    /// Last delivered value (µV): the pin's boundary value.
    uv: u32,
    /// The delivered value at the previous boundary (for `sample_output`).
    start_uv: u32,
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
    /// Thresholded digital outputs by pin name.
    digital: BTreeMap<String, DigitalOutput>,
    /// ADC outputs by pin name.
    adc: BTreeMap<String, AdcOutput>,
    /// [`NgspiceConfig::adc_push`].
    adc_push: AdcPush,
    /// Level changes of the digital outputs and ADC pushes in the last
    /// grant, by time (ties: digital pins, then ADC pins, each in pin
    /// order).
    trace: Vec<Transition>,
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
    /// [`NgspiceEngine::deck`].
    deck: String,
}

/// A configuration checked and turned into its pins and its deck (without
/// the analysis card): what [`NgspiceEngine::new`] and [`operating_point`]
/// share.
struct Prepared {
    inputs: BTreeMap<String, InputPin>,
    outputs: BTreeMap<String, String>,
    digital: BTreeMap<String, DigitalOutput>,
    adc: BTreeMap<String, AdcOutput>,
    watch: Vec<String>,
    sources: BTreeMap<String, Wave>,
    /// Bench tables (ADR-010 D3) by lower-case source name.
    tables: BTreeMap<String, Table>,
    /// Title, circuit body, glue and the `.save` lines.
    lines: Vec<String>,
}

/// The vectors the engine reads (ADR-015 D2): every watched node (the
/// analog outputs, the probes, the thresholded `digital_in` nodes and the
/// `adc_in` nodes, which `read_vectors` and the early-stop watch copy) and
/// every input glue node. Lower case, in first-use order; `time`, the scale,
/// is saved by ngspice without being named.
fn save_list(watch: &[String], inputs: &[InputBinding]) -> Vec<String> {
    let mut save: Vec<String> = watch.to_vec();
    for b in inputs {
        if let Some(n) = &b.glue_node {
            let n = n.to_ascii_lowercase();
            if !save.contains(&n) {
                save.push(n);
            }
        }
    }
    #[cfg(test)]
    if let Some(omit) = tests::SAVE_OMIT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_deref()
    {
        save.retain(|s| s != omit);
    }
    save
}

/// The `.save` cards of `save` (eight vectors per card).
fn save_cards(save: &[String]) -> Vec<String> {
    save.chunks(8)
        .map(|c| format!(".save {}", c.join(" ")))
        .collect()
}

fn prepare(cfg: &NgspiceConfig) -> Result<Prepared, EngineError> {
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
    {
        // Inputs, sources and glue.
        let mut inputs = BTreeMap::new();
        let mut sources = BTreeMap::new();
        let mut glue = Vec::new();
        for b in &cfg.inputs {
            let (level, volts) = match &b.drive {
                Drive::Digital { model, initial } => (*initial, model.level_volts(*initial)),
                Drive::Analog { initial, .. } | Drive::Timed { initial } => (Level::Low, *initial),
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
                    Drive::Analog { .. } | Drive::Timed { .. } => 0.0,
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
        // Bench tables (ADR-010 D3, D4).
        let lattice_fs = cfg.lattice.map_or(0, Time::as_fs);
        let mut tables = BTreeMap::new();
        for t in &cfg.tables {
            let key = t.source.to_ascii_lowercase();
            if !(key.starts_with('v') || key.starts_with('i')) {
                return Err(other(format!(
                    "table source {} must be a V or I element",
                    t.source
                )));
            }
            if t.points.is_empty() || t.points.windows(2).any(|w| w[1].0 <= w[0].0) {
                return Err(other(format!(
                    "table source {}: needs points, strictly increasing in time",
                    t.source
                )));
            }
            if t.points.iter().any(|(_, v)| !v.is_finite()) {
                return Err(other(format!(
                    "table source {}: a value is not finite",
                    t.source
                )));
            }
            if sources.contains_key(&key) || tables.insert(key, Table::new(t, lattice_fs)).is_some()
            {
                return Err(other(format!("source {} bound twice", t.source)));
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
        let mut digital_watch = Vec::new();
        for d in &cfg.digital_outputs {
            let m = d.model;
            if !(m.vil.is_finite() && m.vih.is_finite() && m.vil < m.vih) {
                return Err(other(format!(
                    "digital output {}: need finite vil < vih (got vil {}, vih {})",
                    d.pin, m.vil, m.vih
                )));
            }
            if !(m.cin.is_finite() && m.cin >= 0.0) {
                return Err(other(format!(
                    "digital output {}: cin must be >= 0 (got {})",
                    d.pin, m.cin
                )));
            }
            if outputs.contains_key(&d.pin) || digital_watch.iter().any(|(p, _)| p == &d.pin) {
                return Err(other(format!("output pin {} bound twice", d.pin)));
            }
            add_watch(&d.node);
            digital_watch.push((d.pin.clone(), d.node.to_ascii_lowercase()));
            if m.cin > 0.0 {
                glue.push(format!(
                    "{} {} 0 {:e}",
                    cin_glue_name(&d.pin),
                    d.node,
                    m.cin
                ));
            }
        }
        let digital: BTreeMap<String, DigitalOutput> = cfg
            .digital_outputs
            .iter()
            .zip(&digital_watch)
            .map(|(d, (_, node))| {
                let w = watch.iter().position(|n| n == node).expect("node watched");
                (
                    d.pin.clone(),
                    DigitalOutput {
                        watch: w,
                        model: d.model,
                        level: Level::Low,
                        edge_time: Time::ZERO,
                    },
                )
            })
            .collect();
        let mut adc = BTreeMap::new();
        for a in &cfg.adc_outputs {
            let m = a.model;
            if !(m.vref.is_finite() && m.vref > 0.0) || !(1..=16).contains(&m.bits) {
                return Err(other(format!(
                    "ADC output {}: need vref > 0 and 1 <= bits <= 16 (got vref {}, bits {})",
                    a.pin, m.vref, m.bits
                )));
            }
            if cfg.lattice.is_none() {
                return Err(other(format!(
                    "ADC output {}: needs a grant lattice (pushes are at lattice points, \
                     ADR-003 D6)",
                    a.pin
                )));
            }
            if outputs.contains_key(&a.pin)
                || digital.contains_key(&a.pin)
                || adc.contains_key(&a.pin)
            {
                return Err(other(format!("output pin {} bound twice", a.pin)));
            }
            let node = a.node.to_ascii_lowercase();
            let w = match watch.iter().position(|n| *n == node) {
                Some(w) => w,
                None => {
                    watch.push(node);
                    watch.len() - 1
                }
            };
            adc.insert(
                a.pin.clone(),
                AdcOutput {
                    watch: w,
                    model: m,
                    key: 0,
                    uv: 0,
                    start_uv: 0,
                },
            );
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
                || lower == ".op"
                || lower.starts_with(".op ")
                || lower.starts_with(".control")
                || lower.starts_with(".endc")
            {
                return Err(other(format!(
                    "netlist must not contain `{l}`: the engine owns the analysis"
                )));
            }
            // ADR-015 D2: the engine saves exactly the vectors it reads.
            if lower.starts_with(".save") {
                return Err(other(format!(
                    "netlist must not contain `{l}`: the engine owns the deck's .save \
                     (ADR-015 D2)"
                )));
            }
            lines.push(raw.to_string());
        }
        lines.extend(glue);
        lines.extend(save_cards(&save_list(&watch, &cfg.inputs)));
        Ok(Prepared {
            inputs,
            outputs,
            digital,
            adc,
            watch,
            sources,
            tables,
            lines,
        })
    }
}

/// Claim the ngspice singleton for a new engine (or an operating-point
/// check) and reset the shared state.
fn claim(
    cfg: &NgspiceConfig,
    watch: &[String],
    sources: BTreeMap<String, Wave>,
    tables: BTreeMap<String, Table>,
) -> Result<(), EngineError> {
    let mut st = state();
    if st.active {
        return Err(other(
            "another NgspiceEngine is alive in this process (ngspice is a singleton)",
        ));
    }
    reset_state(&mut st);
    st.active = true;
    st.watch = watch.to_vec();
    st.sources = sources;
    st.tables = tables;
    st.handoff_spin = cfg.handoff_spin;
    st.lattice_fs = cfg.lattice.map_or(0, Time::as_fs);
    st.lattice_snap = cfg.lattice_snap;
    st.wd = WatchCount::new(cfg.watchdog);
    Ok(())
}

/// Load `lines` into ngspice (a NULL-terminated array of lines; ngspice
/// copies them). Errors with ngspice's error lines if it rejects them.
fn load_circuit(lib: &ffi::Lib, lines: &[String]) -> Result<(), EngineError> {
    log().clear();
    let owned: Vec<CString> = lines
        .iter()
        .map(|l| CString::new(l.as_str()))
        .collect::<Result<_, _>>()
        .map_err(|e| other(e.to_string()))?;
    let mut ptrs: Vec<*mut c_char> = owned.iter().map(|c| c.as_ptr().cast_mut()).collect();
    ptrs.push(std::ptr::null_mut());
    // SAFETY: NULL-terminated array of NUL-terminated lines; ngspice copies.
    let rc = unsafe { (lib.circ)(ptrs.as_mut_ptr()) };
    let errs = logged_errors();
    if rc != 0 || !errs.is_empty() {
        return Err(other(format!(
            "ngspice rejected the netlist (rc {rc}): {}",
            errs.join(" | ")
        )));
    }
    Ok(())
}

/// `optran` with a zero step size: no transient-op fallback (ngspice 42
/// `src/spicelib/analysis/optran.c`: "optran deselected by setting
/// opstepsize to 0"); the other five values as the default.
const OPTRAN_OFF: &str = "optran 1 1 1 0 10u 0";
/// ngspice 42's built-in `optran` default (`src/frontend/init.c`).
const OPTRAN_DEFAULT: &str = "optran 1 1 1 100n 10u 0";

/// ADR-015 D4: solve the DC operating point of `cfg`'s deck alone (an `.op`
/// analysis, run to completion on the calling thread, with the deck's own
/// `.options`, so the option set's iteration limits apply: Newton, gmin
/// stepping and source stepping, without the transient-op fallback), every
/// input at its initial value. `Ok` when it converges; otherwise
/// [`EngineError::OperatingPoint`] with the nodes ngspice names, in the time
/// ngspice's DC iteration takes, never after a transient. Needs the ngspice
/// singleton, like [`NgspiceEngine::new`], and releases it.
pub fn operating_point(cfg: &NgspiceConfig) -> Result<(), EngineError> {
    let p = prepare(cfg)?;
    let lib = init_lib().map_err(other)?;
    let mut lines = p.lines;
    lines.push(".op".to_string());
    lines.push(".end".to_string());
    claim(cfg, &p.watch, p.sources, p.tables)?;
    let r = (|| {
        load_circuit(lib, &lines)?;
        // No transient-op fallback: ngspice 42 makes `optran` its standard
        // (`src/frontend/init.c`: "1 1 1 100n 10u 0"), a 10 us transient
        // after gmin and source stepping fail, which is where a large deck
        // spends its time before it gives up (and, in an `.op` analysis of
        // the shared library, it fails on a missing `time` vector and does
        // not return). A step size of 0 deselects it for this circuit only;
        // the default is restored below before the circuit is removed, so a
        // later transient keeps it.
        command(lib, OPTRAN_OFF).map_err(other)?;
        // `run` executes the deck's analysis synchronously (no sync
        // callbacks: those belong to a transient).
        let ran = command(lib, "run");
        let log = log();
        let failed = ran.is_err()
            || log.iter().any(|l| {
                l.contains("could not be simulated")
                    || l.contains("simulation(s) aborted")
                    || l.starts_with("stderr Error")
            });
        if failed {
            let lines = failure_lines(&log);
            return Err(EngineError::OperatingPoint {
                nodes: op_failure_nodes(&log),
                detail: if lines.is_empty() {
                    "(ngspice printed no error or warning)".into()
                } else {
                    lines.join(" | ")
                },
            });
        }
        Ok(())
    })();
    let _ = command(lib, OPTRAN_DEFAULT);
    let _ = command(lib, "remcirc");
    let _ = command(lib, "destroy all");
    reset_state(&mut state());
    r
}

impl NgspiceEngine {
    /// Load the circuit, start the transient, and park the solver at t = 0
    /// (after the DC operating point).
    pub fn new(cfg: NgspiceConfig) -> Result<NgspiceEngine, EngineError> {
        let Prepared {
            inputs,
            outputs,
            digital,
            adc,
            watch,
            sources,
            tables,
            mut lines,
        } = prepare(&cfg)?;
        let lib = init_lib().map_err(other)?;
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
        claim(&cfg, &watch, sources, tables)?;
        let mut engine = NgspiceEngine {
            name: cfg.name.clone(),
            lib,
            now: Time::ZERO,
            t_stop: cfg.t_stop,
            lookahead: cfg.lookahead,
            inputs,
            outputs,
            digital,
            adc,
            adc_push: cfg.adc_push,
            trace: Vec::new(),
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
            deck: lines.join("\n") + "\n",
        };
        // From here on, Drop releases the singleton.
        engine.start(&lines)?;
        Ok(engine)
    }

    /// The deck the engine handed to ngspice: the title, the circuit body,
    /// the glue, the engine's `.save` cards (ADR-015 D2), `.tran` and
    /// `.end`, one line each.
    pub fn deck(&self) -> &str {
        &self.deck
    }

    fn start(&mut self, lines: &[String]) -> Result<(), EngineError> {
        load_circuit(self.lib, lines)?;
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

    /// Run the solver to `t` (or, with an early-stop watch, to where it
    /// stops) and take the boundary there. Returns the boundary reached.
    fn grant(&mut self, t: Time, until: Option<Until>) -> Result<Time, EngineError> {
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
        st.until = until;
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
        // The grant end, lowered by an early stop.
        let r = Time::from_fs(st.target_fs);
        st.until = None;
        drop(st);
        self.breakpoints = self.breakpoints.split_off(&Time::from_fs(r.as_fs() + 1));
        self.take_boundary(r)?;
        Ok(r)
    }

    /// After the solver parked at `t`: check errors, take the new samples.
    fn take_boundary(&mut self, t: Time) -> Result<(), EngineError> {
        let mut st = state();
        if let Some((at, detail)) = st.stall.clone() {
            drop(st);
            self.failed = true;
            return Err(EngineError::NoProgress {
                at: Time::from_fs(at),
                detail,
            });
        }
        if let Some(e) = st.error.clone() {
            drop(st);
            self.failed = true;
            return Err(other(format!("{e}; ngspice: {}", failure_detail())));
        }
        if st.parked != Some(st.seq) {
            drop(st);
            self.failed = true;
            if t.is_zero() {
                // The transient's first point is its operating point
                // (ADR-015 D4): name the nodes ngspice names.
                let log = log();
                let lines = failure_lines(&log);
                return Err(EngineError::OperatingPoint {
                    nodes: op_failure_nodes(&log),
                    detail: if lines.is_empty() {
                        "(ngspice printed no error or warning)".into()
                    } else {
                        lines.join(" | ")
                    },
                });
            }
            return Err(other(format!(
                "ngspice stopped before reaching {t}: {}",
                failure_detail()
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
        // Interior points: a point on a lattice point is stored as exactly
        // that point, as it is when a grant ends there (`point_fs`, ADR-003
        // Amendment 12); others are rounded to fs. All are kept inside
        // [previous boundary, t] (rounding can push a point within one ulp
        // of a boundary past it once ulp > 1 fs).
        let lo = self.now.as_fs();
        let hi = t.as_fs();
        let l = self.lattice.map_or(0, Time::as_fs);
        h.times_fs
            .extend(r.times.iter().map(|&s| point_fs(s, l).clamp(lo, hi)));
        h.times_s.extend_from_slice(&r.times);
        for (dst, src) in h.values.iter_mut().zip(r.values) {
            dst.extend(src);
        }
        // The last point is the boundary itself, at exactly `t`.
        if let Some(last) = h.times_fs.last_mut() {
            *last = t.as_fs();
        }
        self.threshold_window(t, t.is_zero());
        if let Err(e) = self.adc_window(t, t.is_zero()) {
            self.failed = true;
            return Err(other(e));
        }
        self.trace.sort_by_key(|tr| tr.t);
        self.now = t;
        Ok(())
    }

    /// Push the ADC outputs over the window just taken (ADR-003 D6): at
    /// every lattice point `kL` in `(now, t]`, the solver point on it gives
    /// the delivered value; a transition at `kL` is emitted by
    /// [`NgspiceConfig::adc_push`]. At t = 0 only the initial value is set.
    /// Every lattice point of the window must have its solver point (the
    /// lattice clamp, D2), or this is an error.
    fn adc_window(&mut self, t: Time, initial: bool) -> Result<(), String> {
        if self.adc.is_empty() {
            return Ok(());
        }
        let l = self.lattice.expect("ADC outputs need a lattice").as_fs();
        let h = &self.hist;
        let from = self.now.as_fs();
        // The solver point of every lattice point in (from, t], in order:
        // the first accepted point recognised as on it.
        let mut points: Vec<(u64, usize)> = Vec::new();
        if !initial {
            for k in 1..h.times_s.len() {
                let Some(lp) = lattice_point_of(h.times_s[k], l) else {
                    continue;
                };
                if lp > from && lp <= t.as_fs() && points.last().is_none_or(|&(p, _)| lp > p) {
                    points.push((lp, k));
                }
            }
            let want = (t.as_fs() - from) / l;
            if points.len() as u64 != want
                || points.last().is_some_and(|&(p, _)| p != t.as_fs())
                || !from.is_multiple_of(l)
            {
                return Err(format!(
                    "ADC outputs: {} solver points on the {want} lattice points of ({}, {t}]",
                    points.len(),
                    self.now
                ));
            }
        }
        for (pin, a) in self.adc.iter_mut() {
            let vals = &h.values[a.watch];
            a.start_uv = a.uv;
            if initial {
                let v = *vals.last().ok_or("no sample at t = 0")?;
                a.uv = a.model.delivered_microvolts(v);
                a.key = a.model.push_key(a.uv);
                if self.adc_push == AdcPush::OnChange {
                    a.uv = a.model.key_microvolts(a.uv);
                }
                a.start_uv = a.uv;
                continue;
            }
            for &(lp, k) in &points {
                let uv = a.model.delivered_microvolts(vals[k]);
                let key = a.model.push_key(uv);
                let delivered = match self.adc_push {
                    AdcPush::EveryLattice => uv,
                    AdcPush::OnChange if key != a.key => a.model.key_microvolts(uv),
                    AdcPush::OnChange => continue,
                };
                a.key = key;
                a.uv = delivered;
                self.trace.push(Transition {
                    t: Time::from_fs(lp),
                    pin: pin.clone(),
                    signal: PinSignal::analog(f64::from(delivered) / 1e6),
                });
            }
        }
        Ok(())
    }

    /// Threshold every digital output over the solver points of the window
    /// just taken (ADR-003 D3) and rebuild the trace of the grant `(now, t]`.
    /// At the first boundary (t = 0) only the initial level is set.
    fn threshold_window(&mut self, t: Time, initial: bool) {
        self.trace.clear();
        let h = &self.hist;
        let n = h.times_fs.len();
        for (pin, d) in self.digital.iter_mut() {
            let vals = &h.values[d.watch];
            if initial {
                if let Some(&v) = vals.last() {
                    d.level = d.model.threshold(Level::Low, v);
                }
                d.edge_time = Time::ZERO;
                continue;
            }
            // Point 0 is the previous boundary, already thresholded.
            for k in 1..n {
                let new = d.model.threshold(d.level, vals[k]);
                if new == d.level {
                    continue;
                }
                let at = crossing_fs(
                    (h.times_s[k - 1], vals[k - 1]),
                    (h.times_s[k], vals[k]),
                    if new == Level::High {
                        d.model.vih
                    } else {
                        d.model.vil
                    },
                );
                // Strictly after the earlier point (so after the previous
                // boundary and any earlier crossing) and not after this one.
                let lo = h.times_fs[k - 1] + 1;
                let at = Time::from_fs(at.max(lo).min(h.times_fs[k].max(lo)).min(t.as_fs()));
                d.level = new;
                d.edge_time = at;
                self.trace.push(Transition {
                    t: at,
                    pin: pin.clone(),
                    signal: PinSignal::digital(new, at),
                });
            }
        }
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
        let slew = match p.drive {
            Drive::Analog { slew, .. } => slew,
            // A bench output (ADR-010 D4): a 1 ns ramp from its time.
            Drive::Timed { .. } => TIMED_RAMP,
            Drive::Digital { .. } => {
                return Err(other(format!("pin {pin} is digital; use push_edge")))
            }
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
        self.grant(t, None).map(|_| ())
    }

    /// With a grant lattice and at least one thresholded digital output or
    /// ADC output.
    fn can_stop_early(&self) -> bool {
        self.lattice.is_some() && !(self.digital.is_empty() && self.adc.is_empty())
    }

    /// ADR-003 D5: advance towards `t`, but at the first level change of a
    /// watched digital output, at `t_x`, keep stepping to `ceil_L(t_x)` and
    /// park there. The solver takes the same steps as in any other
    /// partition (D2), so this is exactly the state a run with a grant
    /// ending at `ceil_L(t_x)` reaches. `watch` must name thresholded
    /// digital outputs or ADC outputs; an ADC output's change (a push,
    /// ADR-003 D6) is at a lattice point, where the engine then stops.
    fn advance_until(&mut self, t: Time, watch: &[String]) -> Result<Time, EngineError> {
        let Some(l) = self.lattice else {
            return Err(EngineError::Protocol(
                "advance_until needs a grant lattice (NgspiceConfig::lattice)".into(),
            ));
        };
        let mut pins = Vec::new();
        let mut adcs = Vec::new();
        let mut names: Vec<&str> = Vec::new();
        for w in watch {
            if names.contains(&w.as_str()) {
                continue;
            }
            if let Some(a) = self.adc.get(w) {
                names.push(w);
                adcs.push(UntilAdc {
                    node: self.watch[a.watch].clone(),
                    model: a.model,
                    key: a.key,
                    push: self.adc_push,
                });
                continue;
            }
            let d = self.digital.get(w).ok_or_else(|| {
                EngineError::Protocol(format!(
                    "advance_until: {w} is not a thresholded digital output or an ADC output \
                     of {}",
                    self.name
                ))
            })?;
            names.push(w);
            pins.push(UntilPin {
                node: self.watch[d.watch].clone(),
                model: d.model,
                level: d.level,
                prev_v: self.hist.values[d.watch].last().copied().unwrap_or(0.0),
            });
        }
        if pins.is_empty() && adcs.is_empty() {
            return self.grant(t, None);
        }
        let until = Until {
            lattice_fs: l.as_fs(),
            from_fs: self.now.as_fs(),
            pins,
            adcs,
            vecs: None,
            prev_s: self.hist.times_s.last().copied().unwrap_or(0.0),
            prev_fs: self.now.as_fs(),
            stop_fs: None,
        };
        let r = self.grant(t, Some(until))?;
        // The stop must be the first lattice point at or after the first
        // watched transition of the trace, which `threshold_window` has just
        // rebuilt from the same points.
        let expected = self
            .trace
            .iter()
            .find(|tr| names.contains(&tr.pin.as_str()))
            .map_or(t, |tr| {
                Time::from_fs(tr.t.as_fs().div_ceil(l.as_fs()) * l.as_fs()).min(t)
            });
        if r != expected {
            self.failed = true;
            return Err(other(format!(
                "early stop at {r}, but the trace's first watched transition gives {expected}"
            )));
        }
        Ok(r)
    }

    fn read_outputs(&mut self) -> Result<PinSignals, EngineError> {
        let mut out = PinSignals::new();
        for (pin, node) in &self.outputs {
            out.insert(pin.clone(), PinSignal::analog(self.voltage(node)?));
        }
        for (pin, d) in &self.digital {
            out.insert(pin.clone(), PinSignal::digital(d.level, d.edge_time));
        }
        for (pin, a) in &self.adc {
            out.insert(pin.clone(), PinSignal::analog(f64::from(a.uv) / 1e6));
        }
        Ok(out)
    }

    /// With thresholded digital outputs, every level change of the grant
    /// (ADR-003 D3), and with ADC outputs, every push (D6). The analog
    /// outputs stay out of the trace: they are reported by their boundary
    /// values and sampled through [`Engine::sample_output`]. Without either:
    /// `None`, as before.
    fn read_output_trace(&mut self) -> Result<Option<Vec<Transition>>, EngineError> {
        if self.digital.is_empty() && self.adc.is_empty() {
            return Ok(None);
        }
        Ok(Some(self.trace.clone()))
    }

    /// Analog outputs: interpolated between solver points. ADC outputs: the
    /// value delivered as of `t` (the last push at or before `t`).
    fn sample_output(&self, pin: &str, t: Time) -> Option<PinSignal> {
        if let Some(a) = self.adc.get(pin) {
            let uv = self
                .trace
                .iter()
                .rev()
                .find(|tr| tr.pin == pin && tr.t <= t)
                .and_then(|tr| tr.signal.volts())
                .unwrap_or(f64::from(a.start_uv) / 1e6);
            return Some(PinSignal::analog(uv));
        }
        let node = self.outputs.get(pin)?;
        self.sample(node, t).ok().map(PinSignal::analog)
    }

    /// An analog output's solver points since the previous boundary (ADR-010
    /// D6): the retained window, with the fs times `sample` uses.
    fn solver_points(&self, pin: &str) -> Option<Vec<bbcore::SolverPoint>> {
        let node = self.outputs.get(pin)?;
        let i = self.watch_index(node).ok()?;
        let h = &self.hist;
        Some(
            h.times_fs
                .iter()
                .zip(&h.times_s)
                .zip(&h.values[i])
                .map(|((&t, &secs), &v)| bbcore::SolverPoint {
                    t: Time::from_fs(t),
                    secs,
                    v,
                })
                .collect(),
        )
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

    /// The default (each entry as a snapshot through `apply_inputs`), except
    /// for a [`Drive::Timed`] pin (a bench output, ADR-010 D4): its value
    /// steps at the entry's own time, never before `now`.
    fn apply_input_trace(
        &mut self,
        base: &PinSignals,
        trace: &[Transition],
    ) -> Result<(), EngineError> {
        let now = self.now;
        let mut snapshot = base.clone();
        for tr in trace {
            snapshot.insert(tr.pin.clone(), tr.signal);
            let timed = self
                .inputs
                .get(&tr.pin)
                .is_some_and(|p| matches!(p.drive, Drive::Timed { .. }));
            match (timed, tr.signal) {
                (true, PinSignal::Analog { volts }) => {
                    self.push_value(&tr.pin, volts, tr.t.max(now))?
                }
                (true, PinSignal::Digital { .. }) => {
                    return Err(other(format!(
                        "pin {}: a digital level on a bench (Timed) input",
                        tr.pin
                    )))
                }
                (false, _) => self.apply_inputs(&snapshot)?,
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

    /// A vector to leave out of the engine's `.save` list (the G2 negative
    /// control of ADR-015 D2).
    pub(super) static SAVE_OMIT: Mutex<Option<String>> = Mutex::new(None);
    /// ngspice is a process-wide singleton: the tests that start it run one
    /// at a time.
    static NGSPICE: Mutex<()> = Mutex::new(());

    fn ngspice() -> MutexGuard<'static, ()> {
        NGSPICE.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// An RC driven by a digital input, read as an analog output and
    /// through a `digital_in` threshold, on a 10 us lattice.
    fn rc(t_stop: Time) -> NgspiceConfig {
        let mut cfg =
            NgspiceConfig::new("R1 in out 1k\nC1 out 0 100n\n", t_stop, Time::from_us(10));
        cfg.lattice = Some(Time::from_us(10));
        cfg.inputs.push(InputBinding {
            pin: "IN".into(),
            source: "V_IN".into(),
            drive: Drive::Digital {
                model: DigitalOut {
                    vol: 0.0,
                    voh: 3.3,
                    rise: Time::from_ns(5),
                    fall: Time::from_ns(5),
                    zout: 25.0,
                },
                initial: Level::Low,
            },
            glue_node: Some("in".into()),
        });
        cfg.outputs.push(OutputBinding {
            pin: "OUT".into(),
            node: "out".into(),
        });
        cfg.digital_outputs.push(DigitalOutputBinding {
            pin: "SENSE".into(),
            node: "out".into(),
            model: DigitalIn {
                vih: 2.0,
                vil: 1.0,
                cin: 5e-12,
            },
        });
        cfg
    }

    /// Run `cfg` in grants of 100 us, the input rising at 50 us; returns
    /// the boundary values of OUT, or the first error.
    fn drive(cfg: NgspiceConfig) -> Result<Vec<u64>, EngineError> {
        let stop = cfg.t_stop;
        let mut e = NgspiceEngine::new(cfg)?;
        let mut out = Vec::new();
        let mut t = Time::ZERO;
        while t < stop {
            if t == Time::ZERO {
                e.push_edge("IN", Level::High, Time::from_us(50))?;
            }
            t = t + Time::from_us(100);
            e.advance_to(t)?;
            out.push(e.voltage("out")?.to_bits());
        }
        Ok(out)
    }

    #[test]
    fn the_engine_saves_exactly_what_it_reads() {
        let _g = ngspice();
        // ADR-015 D2: the deck carries one .save of the read vectors (the
        // watched node, the input's glue node), and nothing else.
        let e = NgspiceEngine::new(rc(Time::from_us(500))).expect("start");
        let saves: Vec<&str> = e
            .deck()
            .lines()
            .filter(|l| l.starts_with(".save"))
            .collect();
        assert_eq!(saves, vec![".save out in"]);
        for w in &e.watch {
            assert!(saves[0].split_whitespace().any(|s| s == w), "{w} not saved");
        }
        drop(e);
        let full = drive(rc(Time::from_us(500))).expect("run");
        // The negative control: without the watched node in the list, the
        // engine cannot read it, and the run fails rather than reading
        // something else.
        *SAVE_OMIT.lock().unwrap() = Some("out".into());
        let r = drive(rc(Time::from_us(500)));
        *SAVE_OMIT.lock().unwrap() = None;
        let err = r.expect_err("a run without its read vector must fail");
        assert!(err.to_string().contains("out"), "{err}");
        // A deck with its own .save is refused: the engine owns it.
        let mut cfg = rc(Time::from_us(500));
        cfg.netlist.push_str(".save all\n");
        let err = NgspiceEngine::new(cfg).err().expect("refused");
        assert!(err.to_string().contains(".save"), "{err}");
        // And the run is repeatable.
        assert_eq!(drive(rc(Time::from_us(500))).expect("run"), full);
    }

    #[test]
    fn the_watchdog_fails_a_cell_with_too_many_points_identically() {
        let _g = ngspice();
        // A limit of 2 accepted points per 10 us cell: ngspice's first steps
        // after t = 0 are short, so the first cell trips, at the same point
        // in every run; the engine then fails every later call.
        let run = || {
            let mut cfg = rc(Time::from_us(500));
            cfg.watchdog.max_points_per_cell = 2;
            drive(cfg).expect_err("the watchdog trips")
        };
        let (a, b) = (run(), run());
        let EngineError::NoProgress { at, detail } = &a else {
            panic!("expected NoProgress, got {a}");
        };
        assert!(*at > Time::ZERO && *at <= Time::from_us(10), "{at}");
        assert!(
            detail.contains("accepted solver points in the lattice cell"),
            "{detail}"
        );
        assert!(detail.contains("Reference value : "), "{detail}");
        assert_eq!(a.to_string(), b.to_string(), "deterministic");
        // The default limits pass the same run.
        drive(rc(Time::from_us(500))).expect("default watchdog");
    }

    #[test]
    fn an_operating_point_failure_names_its_nodes() {
        let _g = ngspice();
        // A regulator whose B source has no conductance above regulation
        // (ADR-009 D2's flat region): the DC operating point fails.
        let mut cfg = NgspiceConfig::new(
            "V1 in 0 5\nB1 in out I = 10*max(0, min(1, (3.3-V(out))/0.01))\nR1 out 0 1Meg\n",
            Time::from_us(100),
            Time::from_us(10),
        );
        cfg.outputs.push(OutputBinding {
            pin: "OUT".into(),
            node: "out".into(),
        });
        // The transient's own operating point fails, at t = 0.
        let tran = || {
            NgspiceEngine::new(cfg.clone())
                .err()
                .expect("fails at t = 0")
        };
        let err = tran();
        assert!(
            matches!(&err, EngineError::OperatingPoint { nodes, .. } if nodes.contains(&"out".to_string())),
            "{err}"
        );
        let first = err.to_string();
        let t0 = Instant::now();
        let err = operating_point(&cfg).expect_err("flat region");
        let EngineError::OperatingPoint { nodes, .. } = &err else {
            panic!("expected OperatingPoint, got {err}");
        };
        assert!(nodes.contains(&"out".to_string()), "{err}");
        assert!(t0.elapsed() < Duration::from_secs(10), "fail fast");
        // The check leaves the transient's own fallback as it was.
        assert_eq!(tran().to_string(), first);
        // A regulator written with a slope everywhere (ADR-009 D2) converges,
        // and the singleton is free again afterwards.
        cfg.netlist = "V1 in 0 5\nB1 in out I = 1m*(3.3-V(out))/0.01\nR1 out 0 1Meg\n".into();
        operating_point(&cfg).expect("converges");
        operating_point(&rc(Time::from_us(100))).expect("rc converges");
        drop(NgspiceEngine::new(rc(Time::from_us(100))).expect("singleton released"));
    }

    #[test]
    fn watch_count_limits() {
        let l = 10_000_000_000; // 10 us
        let mut w = WatchCount::new(Watchdog {
            max_points_per_cell: 3,
            max_points_without_progress: 4,
        });
        // Three points in (0, 10us], the fourth trips.
        assert_eq!(w.accept(0.0, l), None);
        assert_eq!(w.accept(2e-6, l), None);
        assert_eq!(w.accept(4e-6, l), None);
        assert_eq!(w.accept(6e-6, l), None);
        let e = w.accept(8e-6, l).expect("fourth point of the cell");
        assert!(e.contains("(0s, 10us]"), "{e}");
        // Points that do not advance the fs time: the fourth repeat trips.
        let mut w = WatchCount::new(Watchdog {
            max_points_per_cell: 1000,
            max_points_without_progress: 4,
        });
        let t = 77.43e-3;
        assert_eq!(w.accept(t, l), None);
        for _ in 0..3 {
            assert_eq!(w.accept(t + 1e-19, l), None);
        }
        let e = w.accept(t, l).expect("no progress");
        assert!(e.contains("did not advance past 77430us"), "{e}");
        // Progress resets the count; without a lattice only that rule.
        let mut w = WatchCount::new(Watchdog {
            max_points_per_cell: 1,
            max_points_without_progress: 2,
        });
        for k in 0..100 {
            assert_eq!(w.accept(f64::from(k) * 1e-9, 0), None);
        }
        assert_eq!(c_exp5(7.743e-2), "7.74300e-02");
        assert_eq!(c_exp5(1.5), "1.50000e+00");
        assert_eq!(c_exp5(2.5e-120), "2.50000e-120");
    }

    #[test]
    fn op_failure_nodes_are_parsed_from_the_log() {
        let s = |x: &str| x.to_string();
        let log = vec![
            s("stderr Warning: singular matrix:  check node xu1.n2"),
            s("stderr Warning: singular matrix:  check nodes a and b"),
            s("stdout Last Node Voltages"),
            s("stdout ------------------"),
            s("stdout Node                                   Last Voltage        Previous Iter"),
            s("stdout ----                                   ------------        -------------"),
            s("stdout in                                                5                    5"),
            s("stdout out                                               0          9.99999e+06 *"),
            s("stdout v1#branch                                         0                  -10 *"),
            s("stderr doAnalyses: TRAN:  Timestep too small; initial timepoint: trouble with node \"OUT\""),
        ];
        assert_eq!(op_failure_nodes(&log), vec!["xu1.n2", "a", "b", "out"]);
    }

    #[test]
    fn a_stop_without_an_error_line_reports_the_warnings() {
        // The post-M7 review: a singular matrix stopped the transient with an
        // empty reason, because ngspice prints it as a warning.
        let s = |x: &str| x.to_string();
        let log = vec![
            s("stderr Using SPARSE 1.3 as Direct Linear Solver"),
            s("stderr Warning: singular matrix:  check node qf"),
            s("stdout Background thread stopped with timeout = 1"),
            s("stderr Warning: singular matrix:  check node qf"),
        ];
        assert_eq!(
            failure_lines(&log),
            vec![s("stderr Warning: singular matrix:  check node qf")]
        );
        // Error lines win over warnings.
        let mut l2 = log.clone();
        l2.push(s("stderr Error: no such vector x"));
        assert_eq!(
            failure_lines(&l2),
            vec![s("stderr Error: no such vector x")]
        );
        // At most three distinct warnings, oldest first.
        let l3: Vec<String> = (0..5).map(|i| format!("stderr Warning: w{i}")).collect();
        assert_eq!(
            failure_lines(&l3),
            vec![
                s("stderr Warning: w2"),
                s("stderr Warning: w3"),
                s("stderr Warning: w4")
            ]
        );
        assert!(failure_lines(&[s("stdout x")]).is_empty());
    }

    /// The H1 follow-up: ngspice's `doAnalyses` prints "Timestep too
    /// small" with a capital T (and other paths print it in lower case);
    /// a collapsed step is reported either way, as are other spellings of
    /// the warning and error texts.
    #[test]
    fn failure_text_matches_ignore_case() {
        let s = |x: &str| x.to_string();
        let cap = s("stderr doAnalyses: TRAN:  Timestep too small; time = 7.743e-02, timestep = 1.25e-20: trouble with node \"vpre\"");
        let log = vec![s("stdout Reference value : 7.74300e-02"), cap.clone()];
        assert_eq!(failure_lines(&log), vec![cap.clone()]);
        for line in [
            "stdout tran: timestep too small",
            "stdout TIMESTEP TOO SMALL",
            "stdout Singular Matrix: check node qf",
            "stderr WARNING: something",
        ] {
            assert_eq!(failure_lines(&[s(line)]), vec![s(line)], "{line}");
        }
        let e = s("stderr ERROR: no such vector x");
        assert_eq!(failure_lines(&[cap, e.clone()]), vec![e]);
    }

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
    fn tables_interpolate_hold_and_release_breakpoints() {
        // ADR-010 D3: linear between points, the first value before them and
        // the last after; hold is left-continuous (the old value at a point).
        let pts = vec![
            (Time::ZERO, 0.0),
            (Time::from_us(10), 1.0),
            (Time::from_us(30), 3.0),
        ];
        let lin = Table::new(
            &TableSource {
                source: "V1".into(),
                points: pts.clone(),
                interp: Interp::Linear,
            },
            10_000_000_000,
        );
        assert_eq!(lin.value_at(-1.0), 0.0);
        assert!((lin.value_at(5e-6) - 0.5).abs() < 1e-12);
        assert!((lin.value_at(20e-6) - 2.0).abs() < 1e-12);
        assert_eq!(lin.value_at(1.0), 3.0);
        let mut hold = Table::new(
            &TableSource {
                source: "V1".into(),
                points: pts,
                interp: Interp::Hold,
            },
            10_000_000_000,
        );
        assert_eq!(hold.value_at(5e-6), 0.0);
        assert_eq!(hold.value_at(Time::from_us(10).as_secs_f64()), 0.0);
        assert_eq!(hold.value_at(10.000001e-6), 1.0);
        assert_eq!(hold.value_at(29e-6), 1.0);
        assert_eq!(hold.value_at(31e-6), 3.0);
        // The corners and their release: a point is released at the lattice
        // point opening the cell before its own (10 us -> 0, 30 us -> 10 us).
        let mut due = Vec::new();
        hold.accept(0.0, 0.5e-15, &mut due);
        assert_eq!(due, vec![Time::from_us(10).as_secs_f64()]);
        assert_eq!(hold.next_corner(0.0), Some(Time::from_us(10).as_secs_f64()));
        due.clear();
        hold.accept(Time::from_us(10).as_secs_f64(), 0.5e-15, &mut due);
        assert_eq!(due, vec![Time::from_us(30).as_secs_f64()]);
        assert_eq!(hold.cur, 1);
        // A long linear table sets no breakpoints (ADR-010 Amendment 1).
        let long: Vec<(Time, f64)> = (0..=TABLE_BREAKPOINTS_MAX as u64)
            .map(|k| (Time::from_us(10 * k), k as f64))
            .collect();
        let mut t = Table::new(
            &TableSource {
                source: "V1".into(),
                points: long,
                interp: Interp::Linear,
            },
            10_000_000_000,
        );
        assert_eq!(t.next_corner(0.0), Some(Time::from_us(10).as_secs_f64()));
        due.clear();
        t.accept(1.0, 0.5e-15, &mut due);
        assert!(due.is_empty());
    }

    #[test]
    fn next_lattice_point_is_consistent_with_the_park_test() {
        let l = 10_000_000_000; // 10 us
        let k = Time::from_us(16_200).as_secs_f64();
        let next = Time::from_us(16_210).as_secs_f64();
        // Seen in examples/feedback-open: 0.5 fs (just beyond the tolerance)
        // below 16.2 ms. It rounds to 16.2 ms in fs, but it is not on the
        // lattice point, so the next one is 16.2 ms itself, as for a grant
        // ending there.
        let below = 0.0161999999999995;
        assert!(k - below > grant_tolerance_s(k));
        assert_eq!(secs_to_fs(below), 16_200_000_000_000);
        assert_eq!(next_lattice_s(below, l), Some(k));
        // Within the tolerance: on the lattice point, so the next one.
        assert_eq!(
            next_lattice_s(f64::from_bits(k.to_bits() - 2), l),
            Some(next)
        );
        assert_eq!(next_lattice_s(k, l), Some(next));
        assert_eq!(
            next_lattice_s(f64::from_bits(k.to_bits() + 2), l),
            Some(next)
        );
        // Mid-cell.
        assert_eq!(next_lattice_s(16.2049e-3, l), Some(next));
        assert_eq!(next_lattice_s(16.1951e-3, l), Some(k));
        assert_eq!(
            next_lattice_s(0.0, l),
            Some(Time::from_us(10).as_secs_f64())
        );
        assert_eq!(next_lattice_s(1.0, 0), None);
    }

    #[test]
    fn crossing_is_interpolated_and_rounded_up() {
        // 0 V at 1 us, 2 V at 3 us: 1.5 V at 2.5 us exactly.
        assert_eq!(crossing_fs((1e-6, 0.0), (3e-6, 2.0), 1.5), 2_500_000_000);
        // Rounded up to the next whole fs.
        let t = crossing_fs((0.0, 0.0), (3e-15, 3.0), 1.0);
        assert!((1..=2).contains(&t), "{t}");
        // Falling, and a flat segment (the later point).
        assert_eq!(crossing_fs((1e-6, 2.0), (2e-6, 0.0), 1.0), 1_500_000_000);
        assert_eq!(crossing_fs((1e-6, 1.0), (2e-6, 1.0), 1.0), 2_000_000_000);
        assert_eq!(cin_glue_name("U1.PB0"), "C_bbin_U1_PB0");
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

    #[test]
    fn solver_point_on_a_lattice_point_is_exactly_that_point() {
        // ADR-003 Amendment 12: at 4.153 s one ulp is 0.888 fs, so a point
        // one or two ulps above f64(kL) rounds to kL + 1 or kL + 2 fs, yet
        // the lattice clamp and the park test treat it as on kL. Its fs must
        // be kL, as it is when a grant ends there.
        let l = 10_000_000_000; // 10 us
        let k = Time::from_ms(4153);
        let ks = k.as_secs_f64();
        for d in [-3i64, -2, -1, 0, 1, 2, 3] {
            let s = f64::from_bits(ks.to_bits().wrapping_add_signed(d));
            assert!((s - ks).abs() <= grant_tolerance_s(ks), "{d} ulps");
            assert_eq!(point_fs(s, l), k.as_fs(), "{d} ulps from kL");
            assert_eq!(lattice_point_of(s, l), Some(k.as_fs()));
        }
        assert_ne!(
            secs_to_fs(f64::from_bits(ks.to_bits() + 1)),
            k.as_fs(),
            "rounding alone gives kL + 1 here (the bug)"
        );
        // Off the lattice: rounded to fs, unchanged. Without a lattice:
        // rounded to fs.
        let mid = Time::from_ms(4153) + Time::from_us(5);
        assert_eq!(
            point_fs(mid.as_secs_f64(), l),
            secs_to_fs(mid.as_secs_f64())
        );
        let up = f64::from_bits(ks.to_bits() + 1);
        assert_eq!(point_fs(up, 0), secs_to_fs(up));
        // Small times: the half-fs tolerance, as before.
        assert_eq!(point_fs(1e-3, l), 1_000_000_000_000);
    }
}
