//! `DeviceEngine`: every device part of a run as one engine (ADR-006 D7,
//! Amendment 6 A6.3).
//!
//! - `next_safe_time` is [`Time::MAX`]: every output change is in the trace
//!   at its exact time ("I report everything", `spec/time-grant.md`).
//! - `quiet_until` is the earliest pending timer: by rule 1 an output
//!   changes only at a timer, so with constant inputs nothing changes
//!   before it. For a device with promise-independent inputs (ADR-014 D2)
//!   it is the earliest pending *output* timer, and the device's inputs
//!   cannot end it (`input_ends_promise` is `false` for them).
//! - An input change delivered before an advance (`t > now`) is applied at
//!   its own time; one delivered at a boundary (`t <= now`) at `now`.
//! - Devices are called only at their events (rule 3), timers before inputs
//!   at one instant (rule 2), and in instance order (sorted references).
//! - `advance_until` (with a stop lattice, like the ngspice engine): stop at
//!   `ceil_L` of the first change of a watched output.
//! - No grant lattice is declared: a device applies inputs at their times.

use std::collections::{BTreeMap, BTreeSet};

use bbcore::{Engine, EngineError, Level, LogRecord, PinSignal, PinSignals, Time, Transition};

use crate::{Cmd, Ctx, Device, PinDir, Rng};

/// One device part.
pub struct DevicePart {
    /// The part reference (`M1`); pins are `<reference>.<pin>`.
    pub reference: String,
    pub device: Box<dyn Device>,
}

/// Configuration of a [`DeviceEngine`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceEngineConfig {
    /// The engine's name in events (`engine:<name>`, `<name>.<pin>`).
    pub name: String,
    /// The run seed; each instance's [`Rng`] comes from it and its reference.
    pub seed: u64,
    /// The grant lattice `L` of the run, for `advance_until`'s stop at
    /// `ceil_L`. `None`: the engine cannot stop early.
    pub stop_lattice: Option<Time>,
}

struct Instance {
    reference: String,
    device: Box<dyn Device>,
    rng: Rng,
    /// Pending timers: id -> (time, output timer).
    timers: BTreeMap<u32, (Time, bool)>,
    /// The device declares promise-independent inputs (ADR-014 D2).
    independent: bool,
    /// Engine pin index of each device pin.
    pins: Vec<usize>,
}

struct EnginePin {
    /// `<reference>.<pin>`.
    name: String,
    instance: usize,
    /// Index into the device's pins.
    local: usize,
    dir: PinDir,
    level: Level,
    edge_time: Time,
    /// The edge time before the latest change (an output set back at the
    /// same instant returns to it).
    edge_before: Time,
}

/// The devices of a run as one engine.
pub struct DeviceEngine {
    name: String,
    stop_lattice: Option<Time>,
    instances: Vec<Instance>,
    pins: Vec<EnginePin>,
    by_name: BTreeMap<String, usize>,
    now: Time,
    /// Pending timers of every instance: (time, instance, id).
    queue: BTreeSet<(Time, usize, u32)>,
    /// Input changes delivered for the coming advance: (time, seq) ->
    /// (pin, level).
    inputs: BTreeMap<(Time, u64), (usize, Level)>,
    seq: u64,
    /// Output changes in `(prev, now]`.
    trace: Vec<Transition>,
    /// The devices' log records not yet drained, in event order.
    logs: Vec<LogRecord>,
}

impl DeviceEngine {
    /// The engine over `parts` (in any order: they are sorted by
    /// reference). Calls every device's `start` at `t = 0`.
    pub fn new(cfg: DeviceEngineConfig, mut parts: Vec<DevicePart>) -> Result<Self, String> {
        if cfg.name.is_empty() {
            return Err("device engine: empty name".into());
        }
        parts.sort_by(|a, b| a.reference.cmp(&b.reference));
        let mut e = DeviceEngine {
            name: cfg.name,
            stop_lattice: cfg.stop_lattice.filter(|l| !l.is_zero()),
            instances: Vec::new(),
            pins: Vec::new(),
            by_name: BTreeMap::new(),
            now: Time::ZERO,
            queue: BTreeSet::new(),
            inputs: BTreeMap::new(),
            seq: 0,
            trace: Vec::new(),
            logs: Vec::new(),
        };
        for (k, p) in parts.into_iter().enumerate() {
            if p.reference.is_empty() || p.reference.contains('.') {
                return Err(format!(
                    "device part reference {:?}: empty or has a '.'",
                    p.reference
                ));
            }
            if e.instances.iter().any(|i| i.reference == p.reference) {
                return Err(format!("device part {} given twice", p.reference));
            }
            let mut local = Vec::new();
            for (j, dp) in p.device.pins().iter().enumerate() {
                let name = format!("{}.{}", p.reference, dp.name);
                if e.by_name.contains_key(&name) {
                    return Err(format!("device pin {name} declared twice"));
                }
                e.by_name.insert(name.clone(), e.pins.len());
                local.push(e.pins.len());
                e.pins.push(EnginePin {
                    name,
                    instance: k,
                    local: j,
                    dir: dp.dir,
                    level: Level::Low,
                    edge_time: Time::ZERO,
                    edge_before: Time::ZERO,
                });
            }
            e.instances.push(Instance {
                rng: Rng::for_instance(cfg.seed, &p.reference),
                independent: p.device.promise_independent_inputs(),
                reference: p.reference,
                device: p.device,
                timers: BTreeMap::new(),
                pins: local,
            });
        }
        for k in 0..e.instances.len() {
            e.call(k, Call::Start).map_err(|x| x.to_string())?;
        }
        // The initial outputs are boundary values at t = 0, not trace
        // entries (the initial exchange reads no trace).
        e.trace.clear();
        Ok(e)
    }

    /// The engine's pins: `(name, direction)`, in engine order.
    pub fn pins(&self) -> Vec<(String, PinDir)> {
        self.pins.iter().map(|p| (p.name.clone(), p.dir)).collect()
    }

    /// The earliest pending timer.
    fn next_timer(&self) -> Option<Time> {
        self.queue.first().map(|(t, ..)| *t)
    }

    fn next_input(&self) -> Option<Time> {
        self.inputs.first_key_value().map(|((t, _), _)| *t)
    }

    /// Call one device and apply what it asked for.
    fn call(&mut self, k: usize, what: Call) -> Result<(), EngineError> {
        let now = self.now;
        let inst = &mut self.instances[k];
        // ADR-014 D2: an internal timer of a device with promise-independent
        // inputs must not set a pin.
        let internal = inst.independent && matches!(what, Call::Timer(_, false));
        let mut ctx = Ctx::new(now, matches!(what, Call::Pin(..)), internal, &mut inst.rng);
        match what {
            Call::Start => inst.device.start(&mut ctx),
            Call::Timer(id, _) => inst.device.on_timer(&mut ctx, id, now),
            Call::Pin(pin, level) => inst.device.on_pin_change(&mut ctx, pin, level, now),
        }
        let Ctx { cmds, error, .. } = ctx;
        if let Some(msg) = error {
            return Err(EngineError::Protocol(format!(
                "device {}: {msg}",
                self.instances[k].reference
            )));
        }
        for c in cmds {
            match c {
                Cmd::Schedule(id, at, output) => {
                    let inst = &mut self.instances[k];
                    if let Some((old, _)) = inst.timers.insert(id, (at, output)) {
                        self.queue.remove(&(old, k, id));
                    }
                    self.queue.insert((at, k, id));
                }
                Cmd::Cancel(id) => {
                    if let Some((old, _)) = self.instances[k].timers.remove(&id) {
                        self.queue.remove(&(old, k, id));
                    }
                }
                Cmd::SetPin(local, level) => {
                    let ep = *self.instances[k].pins.get(local).ok_or_else(|| {
                        EngineError::Protocol(format!(
                            "device {}: set_pin({local}): no such pin",
                            self.instances[k].reference
                        ))
                    })?;
                    self.drive(ep, level)?;
                }
                Cmd::Log(level, msg) => self.logs.push(LogRecord {
                    t: now,
                    level,
                    msg: format!("{}: {msg}", self.instances[k].reference),
                }),
            }
        }
        Ok(())
    }

    /// Drive engine pin `ep` to `level` at `now`.
    fn drive(&mut self, ep: usize, level: Level) -> Result<(), EngineError> {
        let now = self.now;
        let p = &mut self.pins[ep];
        if p.dir != PinDir::Out {
            return Err(EngineError::Protocol(format!(
                "set_pin on {}, an input",
                p.name
            )));
        }
        if p.level == level {
            return Ok(());
        }
        // A pin set back at the instant of its last change: the two changes
        // cancel (the trace is strictly increasing per pin, and a digital
        // pin has only two levels).
        if p.edge_time == now && now > Time::ZERO {
            if let Some(pos) = self
                .trace
                .iter()
                .rposition(|tr| tr.pin == p.name && tr.t == now)
            {
                self.trace.remove(pos);
                p.level = level;
                p.edge_time = p.edge_before;
                return Ok(());
            }
        }
        p.level = level;
        p.edge_before = p.edge_time;
        p.edge_time = now;
        self.trace.push(Transition {
            t: now,
            pin: p.name.clone(),
            signal: PinSignal::digital(level, now),
        });
        Ok(())
    }

    /// Handle every event at the earliest pending instant if it is `<= t`.
    /// Returns that instant, or `None` (no event `<= t`).
    fn step(&mut self, t: Time) -> Result<Option<Time>, EngineError> {
        let next = match (self.next_timer(), self.next_input()) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => return Ok(None),
        };
        if next > t {
            return Ok(None);
        }
        self.now = next;
        // Rule 2: timers first, in (instance, id) order; a timer handler may
        // schedule only later timers, so this loop ends.
        while let Some(&(at, k, id)) = self.queue.first() {
            if at != next {
                break;
            }
            self.queue.pop_first();
            let output = self.instances[k]
                .timers
                .remove(&id)
                .is_some_and(|(_, output)| output);
            self.call(k, Call::Timer(id, output))?;
        }
        while let Some((&(at, s), &(ep, level))) = self.inputs.first_key_value() {
            if at != next {
                break;
            }
            self.inputs.remove(&(at, s));
            self.apply_now(ep, level)?;
        }
        Ok(Some(next))
    }

    /// Apply an input change of engine pin `ep` at `now`.
    fn apply_now(&mut self, ep: usize, level: Level) -> Result<(), EngineError> {
        let p = &mut self.pins[ep];
        if p.level == level {
            return Ok(());
        }
        p.level = level;
        p.edge_time = self.now;
        let (k, local) = (p.instance, p.local);
        self.call(k, Call::Pin(local, level))
    }

    fn input_pin(&self, name: &str) -> Result<usize, EngineError> {
        match self.by_name.get(name) {
            Some(&i) if self.pins[i].dir == PinDir::In => Ok(i),
            _ => Err(EngineError::Protocol(format!(
                "{}: {name} is not a device input",
                self.name
            ))),
        }
    }

    fn ceil_l(&self, t: Time) -> Time {
        match self.stop_lattice {
            Some(l) => Time::from_fs(t.as_fs().div_ceil(l.as_fs()) * l.as_fs()),
            None => t,
        }
    }
}

enum Call {
    Start,
    /// `(id, output timer)`.
    Timer(u32, bool),
    Pin(usize, Level),
}

impl Engine for DeviceEngine {
    fn name(&self) -> &str {
        &self.name
    }

    fn next_safe_time(&mut self, _now: Time) -> Time {
        Time::MAX
    }

    fn advance_to(&mut self, t: Time) -> Result<(), EngineError> {
        self.advance_until(t, &[]).map(|_| ())
    }

    fn read_outputs(&mut self) -> Result<PinSignals, EngineError> {
        Ok(self
            .pins
            .iter()
            .filter(|p| p.dir == PinDir::Out)
            .map(|p| (p.name.clone(), PinSignal::digital(p.level, p.edge_time)))
            .collect())
    }

    fn read_output_trace(&mut self) -> Result<Option<Vec<Transition>>, EngineError> {
        Ok(Some(self.trace.clone()))
    }

    /// The records of every event up to `now` (they are made at events, so
    /// none lies past it).
    fn drain_logs(&mut self) -> Vec<LogRecord> {
        std::mem::take(&mut self.logs)
    }

    fn apply_inputs(&mut self, inputs: &PinSignals) -> Result<(), EngineError> {
        // A snapshot at the boundary: a level that differs from what the
        // device has applied changes at `now` (the initial exchange, or a
        // change without a trace entry).
        for (name, sig) in inputs {
            let ep = self.input_pin(name)?;
            let Some(level) = sig.level() else {
                return Err(EngineError::Protocol(format!(
                    "{}: {name}: an analog value on a digital device input",
                    self.name
                )));
            };
            self.apply_now(ep, level)?;
        }
        Ok(())
    }

    fn apply_input_trace(
        &mut self,
        _base: &PinSignals,
        trace: &[Transition],
    ) -> Result<(), EngineError> {
        for tr in trace {
            let ep = self.input_pin(&tr.pin)?;
            let Some(level) = tr.signal.level() else {
                return Err(EngineError::Protocol(format!(
                    "{}: {}: an analog value on a digital device input",
                    self.name, tr.pin
                )));
            };
            if tr.t > self.now {
                self.seq += 1;
                self.inputs.insert((tr.t, self.seq), (ep, level));
            } else {
                // A boundary delivery takes effect at once (at `now`),
                // after every timer at `now` (rule 2).
                self.apply_now(ep, level)?;
            }
        }
        Ok(())
    }

    /// The earliest timer that may set a pin: every timer of a device
    /// without promise-independent inputs, only the output timers of one
    /// with them (ADR-014 D2).
    fn quiet_until(&mut self, now: Time) -> Time {
        let next = self
            .instances
            .iter()
            .flat_map(|i| {
                i.timers
                    .values()
                    .filter(move |(_, output)| !i.independent || *output)
                    .map(|(t, _)| *t)
            })
            .min();
        match next {
            Some(t) if t > now => t,
            Some(_) => now,
            None => Time::MAX,
        }
    }

    /// ADR-014 D2: an input of a device with promise-independent inputs
    /// cannot end its promise.
    fn input_ends_promise(&self, pin: &str) -> bool {
        match self.by_name.get(pin) {
            Some(&ep) => !self.instances[self.pins[ep].instance].independent,
            None => true,
        }
    }

    fn can_stop_early(&self) -> bool {
        self.stop_lattice.is_some()
    }

    fn advance_until(&mut self, t: Time, watch: &[String]) -> Result<Time, EngineError> {
        if t <= self.now {
            return Err(EngineError::Protocol(format!(
                "{}: advance to {t} from {}",
                self.name, self.now
            )));
        }
        self.trace.clear();
        let mut end = t;
        while let Some(at) = self.step(end)? {
            if end == t && !watch.is_empty() {
                let changed = self
                    .trace
                    .iter()
                    .any(|tr| tr.t == at && watch.contains(&tr.pin));
                if changed {
                    end = self.ceil_l(at).min(t);
                }
            }
        }
        // Inputs delivered for this grant that lie past an early stop stay
        // queued; the kernel only delivers changes inside the grant, and
        // after an early stop it delivers none past `end` (ADR-003 D5).
        self.now = end;
        Ok(end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DevicePin;

    /// Echo: OUT follows IN `delay` later; also a free-running toggle on
    /// TICK every `period`.
    struct Echo {
        delay: Time,
        period: Time,
        pending: Vec<(Time, Level)>,
        tick: Level,
        set_in_input: bool,
    }

    const PINS: [DevicePin; 3] = [
        DevicePin {
            name: "IN",
            dir: PinDir::In,
        },
        DevicePin {
            name: "OUT",
            dir: PinDir::Out,
        },
        DevicePin {
            name: "TICK",
            dir: PinDir::Out,
        },
    ];

    impl Device for Echo {
        fn pins(&self) -> &[DevicePin] {
            &PINS
        }
        fn start(&mut self, ctx: &mut Ctx) {
            ctx.schedule(self.period, 1);
        }
        fn on_pin_change(&mut self, ctx: &mut Ctx, _pin: usize, level: Level, t: Time) {
            if self.set_in_input {
                ctx.set_pin(1, level);
            }
            self.pending.push((t + self.delay, level));
            ctx.schedule(self.pending[0].0, 2);
        }
        fn on_timer(&mut self, ctx: &mut Ctx, id: u32, t: Time) {
            match id {
                1 => {
                    self.tick = self.tick.toggled();
                    ctx.set_pin(2, self.tick);
                    ctx.schedule(t + self.period, 1);
                }
                _ => {
                    let (at, l) = self.pending.remove(0);
                    assert_eq!(at, t);
                    ctx.set_pin(1, l);
                    if let Some(&(n, _)) = self.pending.first() {
                        ctx.schedule(n, 2);
                    }
                }
            }
        }
    }

    fn engine(set_in_input: bool) -> DeviceEngine {
        DeviceEngine::new(
            DeviceEngineConfig {
                name: "devices".into(),
                seed: 1,
                stop_lattice: Some(Time::from_us(10)),
            },
            vec![DevicePart {
                reference: "M1".into(),
                device: Box::new(Echo {
                    delay: Time::from_ns(1500),
                    period: Time::from_us(25),
                    pending: Vec::new(),
                    tick: Level::Low,
                    set_in_input,
                }),
            }],
        )
        .unwrap()
    }

    fn tr(t: Time, pin: &str, l: Level) -> Transition {
        Transition {
            t,
            pin: pin.into(),
            signal: PinSignal::digital(l, t),
        }
    }

    #[test]
    fn trace_quiet_and_early_stop() {
        let mut e = engine(false);
        assert_eq!(e.next_safe_time(Time::ZERO), Time::MAX);
        assert_eq!(e.quiet_until(Time::ZERO), Time::from_us(25));
        e.apply_input_trace(
            &PinSignals::new(),
            &[tr(Time::from_ns(3_333), "M1.IN", Level::High)],
        )
        .unwrap();
        e.advance_to(Time::from_us(10)).unwrap();
        let t = e.read_output_trace().unwrap().unwrap();
        assert_eq!(t, vec![tr(Time::from_ns(4_833), "M1.OUT", Level::High)]);
        assert_eq!(
            e.read_outputs().unwrap()["M1.OUT"],
            PinSignal::digital(Level::High, Time::from_ns(4_833))
        );
        // The early stop at ceil_L of the first watched change (TICK at 25 us).
        let r = e
            .advance_until(Time::from_us(100), &["M1.TICK".into()])
            .unwrap();
        assert_eq!(r, Time::from_us(30));
        let t = e.read_output_trace().unwrap().unwrap();
        assert_eq!(t, vec![tr(Time::from_us(25), "M1.TICK", Level::High)]);
        assert_eq!(e.quiet_until(r), Time::from_us(50));
    }

    #[test]
    fn rule_one_is_enforced() {
        let mut e = engine(true);
        e.apply_input_trace(
            &PinSignals::new(),
            &[tr(Time::from_ns(3_333), "M1.IN", Level::High)],
        )
        .unwrap();
        let err = e.advance_to(Time::from_us(10)).unwrap_err().to_string();
        assert!(err.contains("rule 1"), "{err}");
    }

    #[test]
    fn boundary_inputs_apply_at_now_after_timers() {
        // A change delivered at the boundary 25 us (where TICK's timer
        // fires) is applied after the timer: OUT follows at 26.5 us.
        let mut e = engine(false);
        e.advance_to(Time::from_us(25)).unwrap();
        e.apply_input_trace(
            &PinSignals::new(),
            &[tr(Time::from_us(25), "M1.IN", Level::High)],
        )
        .unwrap();
        e.advance_to(Time::from_us(30)).unwrap();
        assert_eq!(
            e.read_output_trace().unwrap().unwrap(),
            vec![tr(Time::from_ns(26_500), "M1.OUT", Level::High)]
        );
        // An unknown pin or an output is refused.
        assert!(e
            .apply_input_trace(
                &PinSignals::new(),
                &[tr(Time::from_us(31), "M1.OUT", Level::High)]
            )
            .is_err());
    }
}
