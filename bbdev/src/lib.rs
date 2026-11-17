//! `bbdev`: Breadboard's Phase 1 device API (ADR-006 D7, Amendment 6).
//!
//! A device is a pin-level behavioural model compiled into `bb`: a lead
//! screw and a Hall switch, a button, a sensor without a bus. It sees its
//! `digital_in` pins change and drives its `digital_out` pins; it has no
//! wall clock, no I/O and no time source of its own besides the timers it
//! schedules. [`DeviceEngine`] runs every device of a run as one engine
//! under the time-grant protocol (`spec/time-grant.md`).
//!
//! **The three rules** (ADR-006 Amendment 6, A6.2) that make a device's
//! outputs independent of the grant partition:
//!
//! 1. Outputs change only in [`Device::on_timer`] (or [`Device::start`]). A
//!    reaction to an input is a timer strictly after the input instant;
//!    [`Ctx::set_pin`] inside [`Device::on_pin_change`] fails the run.
//! 2. At one instant, timers fire before input changes: timers in
//!    (instance, id) order, inputs in delivery order.
//! 3. A device's state advances only at its own events (timers and input
//!    changes), never at grant boundaries: the engine never calls a device
//!    at a boundary.
//!
//! **Output timers and promise-independent inputs** (ADR-014 D2). A timer
//! scheduled with [`Ctx::schedule_output`] is an *output timer*: its handler
//! may call [`Ctx::set_pin`]. A device that declares
//! [`Device::promise_independent_inputs`] promises more: its outputs change
//! only at its output timers, a timer scheduled with [`Ctx::schedule`] is
//! internal (its handler calling `set_pin` fails the run), and
//! `on_pin_change` never schedules an output timer earlier than the device's
//! pending ones. Its quiet promise is then its next output timer, and an
//! input change cannot end it, so the kernel does not count the device's
//! input drivers as producers of a quiet grant (ADR-014 D1). The kernel's
//! promise audit (`PromiseBroken`) catches a device that breaks this. A
//! device that does not declare it keeps the Phase 1 rule: every timer may
//! set a pin, and its promise is its next timer of any kind.
//!
//! A package names a device with `[behavioral] engine = "device"` and
//! `device = "<id>@<major>"`; the table [`DEVICES`] holds every device this
//! `bb` provides, with the sources that decide its behaviour (their content
//! hash is locked as `device:<id>@<major>`, `spec/package.md` section 9).

use std::collections::BTreeMap;

pub use bbcore::LogLevel;
use bbcore::{Level, Time};

pub mod dms_mechanism;
mod engine;

pub use engine::{DeviceEngine, DeviceEngineConfig, DevicePart};

/// Direction of a device pin, seen from the device.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PinDir {
    /// A `digital_in`: the kernel delivers its changes.
    In,
    /// A `digital_out`: the device drives it.
    Out,
}

/// One pin of a device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DevicePin {
    /// The pin name, as the package's `pins.toml` names it.
    pub name: &'static str,
    pub dir: PinDir,
}

/// The kind of a device parameter. The board file gives it in the package
/// value grammar (`spec/package.md` section 6); `bb` parses it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ParamKind {
    /// A time (`50ms`).
    Time,
    /// A resistance in ohms (`20.28`).
    Resistance,
    /// An inductance in henries (`4.3mH`).
    Inductance,
    /// A plain decimal number (`2.6`), its unit in the key's documentation.
    Number,
}

/// A parsed parameter value.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ParamValue {
    Time(Time),
    /// Ohms, henries or a plain number.
    Real(f64),
}

/// One parameter a device accepts.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ParamSpec {
    pub key: &'static str,
    pub kind: ParamKind,
    /// The default; `None` for a parameter that has none (it is then
    /// absent unless given).
    pub default: Option<ParamValue>,
    pub doc: &'static str,
}

/// The parameters of one device part: every key of its [`ParamSpec`] list
/// that has a value (the board file's, else the default).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Params(pub BTreeMap<&'static str, ParamValue>);

impl Params {
    /// The parameter `key` as a time.
    pub fn time(&self, key: &str) -> Result<Time, String> {
        match self.0.get(key) {
            Some(ParamValue::Time(t)) => Ok(*t),
            Some(_) => Err(format!("parameter {key}: expected a time")),
            None => Err(format!("parameter {key}: missing")),
        }
    }
    /// The optional parameter `key` as a time.
    pub fn time_opt(&self, key: &str) -> Result<Option<Time>, String> {
        match self.0.get(key) {
            None => Ok(None),
            Some(_) => self.time(key).map(Some),
        }
    }
    /// The parameter `key` as a real number.
    pub fn real(&self, key: &str) -> Result<f64, String> {
        match self.0.get(key) {
            Some(ParamValue::Real(x)) => Ok(*x),
            Some(_) => Err(format!("parameter {key}: expected a number")),
            None => Err(format!("parameter {key}: missing")),
        }
    }
}

/// A pin-level device (ADR-006 D7). Called only by [`DeviceEngine`], in
/// time order, at the device's own events.
pub trait Device {
    /// The device's pins, each with its direction. Fixed for the device's
    /// life; inputs start `Low` until the kernel says otherwise.
    fn pins(&self) -> &[DevicePin];
    /// Called once at `t = 0`, before the initial exchange: set the initial
    /// outputs (every output starts `Low` otherwise) and schedule the first
    /// timers.
    fn start(&mut self, ctx: &mut Ctx);
    /// The input pin `pin` (an index into [`Device::pins`]) changed to
    /// `level` at `t` (`== ctx.now()`; applied at `t`, never earlier). Must
    /// not call [`Ctx::set_pin`] (rule 1).
    fn on_pin_change(&mut self, ctx: &mut Ctx, pin: usize, level: Level, t: Time);
    /// A timer the device scheduled fired at `t` (`== ctx.now()`).
    fn on_timer(&mut self, ctx: &mut Ctx, id: u32, t: Time);
    /// ADR-014 D2: the device's outputs change only at its output timers
    /// ([`Ctx::schedule_output`]), its other timers never set a pin, and
    /// `on_pin_change` never schedules an output timer earlier than a
    /// pending one. A fixed property of the device. Default `false`.
    fn promise_independent_inputs(&self) -> bool {
        false
    }
}

/// What a device asked for during one call.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Cmd {
    /// `(id, at, output timer)`.
    Schedule(u32, Time, bool),
    Cancel(u32),
    SetPin(usize, Level),
    Log(LogLevel, String),
}

/// The device's view of the engine during one call (ADR-006 D7): the time,
/// its timers, its output pins and its seeded random numbers.
pub struct Ctx<'a> {
    now: Time,
    /// Inside `on_pin_change` (rule 1).
    in_input: bool,
    /// Inside the handler of an internal timer of a device with
    /// promise-independent inputs (ADR-014 D2).
    in_internal: bool,
    pub(crate) cmds: Vec<Cmd>,
    pub(crate) error: Option<String>,
    rng: &'a mut Rng,
}

impl<'a> Ctx<'a> {
    pub(crate) fn new(now: Time, in_input: bool, in_internal: bool, rng: &'a mut Rng) -> Ctx<'a> {
        Ctx {
            now,
            in_input,
            in_internal,
            cmds: Vec::new(),
            error: None,
            rng,
        }
    }

    /// The current simulated time (the event being handled).
    pub fn now(&self) -> Time {
        self.now
    }

    /// Schedule timer `id` at `at`, replacing a pending timer with the same
    /// id. `at` must be strictly after [`Ctx::now`] (ADR-006 Amendment 6).
    /// For a device with promise-independent inputs (ADR-014 D2) this is an
    /// internal timer, whose handler must not call [`Ctx::set_pin`].
    pub fn schedule(&mut self, at: Time, id: u32) {
        self.schedule_kind(at, id, false);
    }

    /// Schedule output timer `id` at `at` (ADR-014 D2): its handler may call
    /// [`Ctx::set_pin`], and the device's quiet promise ends at it.
    /// Otherwise as [`Ctx::schedule`].
    pub fn schedule_output(&mut self, at: Time, id: u32) {
        self.schedule_kind(at, id, true);
    }

    fn schedule_kind(&mut self, at: Time, id: u32, output: bool) {
        if at <= self.now {
            self.fail(format!(
                "timer {id} scheduled at {at}, not after now {} (a reaction must be strictly later)",
                self.now
            ));
            return;
        }
        self.cmds.push(Cmd::Schedule(id, at, output));
    }

    /// Cancel timer `id` (no effect if it is not pending).
    pub fn cancel(&mut self, id: u32) {
        self.cmds.push(Cmd::Cancel(id));
    }

    /// Drive output pin `pin` (an index into [`Device::pins`]) to `level`
    /// from [`Ctx::now`] on. Refused inside [`Device::on_pin_change`]
    /// (rule 1).
    pub fn set_pin(&mut self, pin: usize, level: Level) {
        if self.in_input {
            self.fail(format!(
                "set_pin({pin}) inside on_pin_change at {}: an output changes only in on_timer \
                 (schedule a timer after the input; ADR-006 Amendment 6, rule 1)",
                self.now
            ));
            return;
        }
        if self.in_internal {
            self.fail(format!(
                "set_pin({pin}) in an internal timer at {}: a device with promise-independent \
                 inputs changes an output only in an output timer (Ctx::schedule_output; \
                 ADR-014 D2)",
                self.now
            ));
            return;
        }
        self.cmds.push(Cmd::SetPin(pin, level));
    }

    /// A log record at [`Ctx::now`] (`<reference>: <msg>` in
    /// `events.jsonl`). Allowed in every call, `on_pin_change` too: a record
    /// is not an output (rule 1), and its time is the event's own, so it
    /// does not depend on the grant partition.
    pub fn log(&mut self, level: LogLevel, msg: String) {
        self.cmds.push(Cmd::Log(level, msg));
    }

    /// The device's random numbers, seeded from the run seed and the
    /// instance reference (deterministic).
    pub fn rng(&mut self) -> &mut Rng {
        self.rng
    }

    /// Fail the run with `msg` (a device's own consistency check).
    pub fn fail(&mut self, msg: String) {
        if self.error.is_none() {
            self.error = Some(msg);
        }
    }
}

/// SplitMix64 (Steele, Lea and Flood 2014): a small, fully specified
/// generator, seeded per instance from FNV-1a-64 of the run seed and the
/// instance reference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rng {
    state: u64,
}

impl Rng {
    /// The generator of instance `reference` in a run with seed `seed`.
    pub fn for_instance(seed: u64, reference: &str) -> Rng {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in seed
            .to_le_bytes()
            .iter()
            .chain([0u8].iter())
            .chain(reference.as_bytes())
        {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        Rng { state: h }
    }

    /// The next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A value in `[0, n)` for `n > 0` (rejection sampling, so unbiased).
    pub fn below(&mut self, n: u64) -> u64 {
        assert!(n > 0, "Rng::below(0)");
        let zone = u64::MAX - (u64::MAX % n);
        loop {
            let x = self.next_u64();
            if x < zone {
                return x % n;
            }
        }
    }
}

/// One device this `bb` provides.
pub struct DeviceDef {
    /// `<name>@<major>`, as packages name it (`[behavioral] device`).
    pub id: &'static str,
    /// The sources that decide its behaviour: `(file name, text)`, hashed
    /// into its content hash (`spec/package.md` section 9).
    pub sources: &'static [(&'static str, &'static str)],
    /// The parameters a device part may set.
    pub params: &'static [ParamSpec],
    /// Build an instance from its parameters (defaults filled in).
    pub build: fn(&Params) -> Result<Box<dyn Device>, String>,
}

/// The device API and engine sources, part of every device's content hash.
pub const API_SOURCES: [(&str, &str); 2] = [
    ("lib.rs", include_str!("lib.rs")),
    ("engine.rs", include_str!("engine.rs")),
];

/// Every device this `bb` provides.
pub const DEVICES: &[DeviceDef] = &[DeviceDef {
    id: "dms/mechanism@2",
    sources: &[
        ("lib.rs", include_str!("lib.rs")),
        ("engine.rs", include_str!("engine.rs")),
        ("dms_mechanism.rs", include_str!("dms_mechanism.rs")),
    ],
    params: dms_mechanism::PARAMS,
    build: dms_mechanism::build,
}];

/// The device with id `id`.
pub fn find(id: &str) -> Option<&'static DeviceDef> {
    DEVICES.iter().find(|d| d.id == id)
}

/// Every device id.
pub fn ids() -> Vec<String> {
    DEVICES.iter().map(|d| d.id.to_string()).collect()
}

impl DeviceDef {
    /// The parameters of a part: `given` (already parsed) over the
    /// defaults. An unknown key is an error.
    pub fn params(&self, given: &[(String, ParamValue)]) -> Result<Params, String> {
        let mut out = BTreeMap::new();
        for p in self.params {
            if let Some(d) = p.default {
                out.insert(p.key, d);
            }
        }
        for (k, v) in given {
            let spec = self
                .params
                .iter()
                .find(|p| p.key == k)
                .ok_or_else(|| format!("device {}: unknown parameter {k:?}", self.id))?;
            let ok = matches!(
                (spec.kind, v),
                (ParamKind::Time, ParamValue::Time(_))
                    | (
                        ParamKind::Resistance | ParamKind::Inductance | ParamKind::Number,
                        ParamValue::Real(_)
                    )
            );
            if !ok {
                return Err(format!("device {}: parameter {k}: wrong kind", self.id));
            }
            out.insert(spec.key, *v);
        }
        Ok(Params(out))
    }

    /// Build an instance from `given` parameters over the defaults.
    pub fn instantiate(&self, given: &[(String, ParamValue)]) -> Result<Box<dyn Device>, String> {
        let p = self.params(given)?;
        (self.build)(&p).map_err(|e| format!("device {}: {e}", self.id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rng_is_seeded_per_instance_and_run() {
        let a: Vec<u64> = {
            let mut r = Rng::for_instance(1, "M1");
            (0..4).map(|_| r.next_u64()).collect()
        };
        let b: Vec<u64> = {
            let mut r = Rng::for_instance(1, "M1");
            (0..4).map(|_| r.next_u64()).collect()
        };
        assert_eq!(a, b);
        let mut c = Rng::for_instance(2, "M1");
        let mut d = Rng::for_instance(1, "M2");
        assert_ne!(a[0], c.next_u64());
        assert_ne!(a[0], d.next_u64());
        // SplitMix64's published first output for state 0 (after one
        // increment): 0xe220a8397b1dcdaf.
        let mut z = Rng { state: 0 };
        assert_eq!(z.next_u64(), 0xe220_a839_7b1d_cdaf);
        let mut r = Rng::for_instance(7, "M1");
        for _ in 0..1000 {
            assert!(r.below(3) < 3);
        }
    }

    #[test]
    fn device_table() {
        let ids = ids();
        assert_eq!(ids, vec!["dms/mechanism@2".to_string()]);
        let d = find("dms/mechanism@2").unwrap();
        assert!(d.sources.iter().any(|(n, _)| *n == "dms_mechanism.rs"));
        for (n, s) in API_SOURCES {
            assert!(d.sources.iter().any(|(m, t)| *m == n && *t == s));
        }
        assert!(d.params(&[("nope".into(), ParamValue::Real(1.0))]).is_err());
        assert!(d.params(&[("ts".into(), ParamValue::Real(1.0))]).is_err());
        let p = d
            .params(&[("ts".into(), ParamValue::Time(Time::from_ms(75)))])
            .unwrap();
        assert_eq!(p.time("ts").unwrap(), Time::from_ms(75));
        assert!(d.instantiate(&[]).is_ok());
    }
}
