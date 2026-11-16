//! `BenchEngine` (ADR-010 D4): the bench's digital outputs into MCU pins,
//! and every action with a trigger.
//!
//! - `next_safe_time` is [`Time::MAX`]: every output change is in the trace
//!   at its exact time.
//! - `quiet_until` is the next scheduled change: with constant inputs no
//!   trigger fires, so nothing else changes before it.
//! - A trigger input change delivered before an advance (`t > now`) is
//!   handled at its own time. A trigger fires once, at the first change of
//!   its input to its level after `t = 0`, and schedules its program at
//!   `t_edge + delay + offset`.
//! - At one instant: scheduled changes first (in scheduling order), then
//!   input changes, then the changes they scheduled at that instant.
//! - `advance_until` (with a stop lattice): stop at `ceil_L` of the first
//!   change of a watched output.
//! - No grant lattice is declared: an MCU applies the bench's edges at
//!   `ceil_L`, the analog engine a `Timed` value at its own time.

use std::collections::BTreeMap;

use bbcore::{Engine, EngineError, Level, PinSignal, PinSignals, Time, Transition};

/// A value an output takes: a digital level, or an analog value (volts, or
/// a switch control's 0/1).
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Value {
    Level(Level),
    Volts(f64),
}

impl Value {
    fn same_kind(self, o: Value) -> bool {
        matches!(
            (self, o),
            (Value::Level(_), Value::Level(_)) | (Value::Volts(_), Value::Volts(_))
        )
    }

    fn eq_exact(self, o: Value) -> bool {
        match (self, o) {
            (Value::Level(a), Value::Level(b)) => a == b,
            (Value::Volts(a), Value::Volts(b)) => a.to_bits() == b.to_bits(),
            _ => false,
        }
    }
}

/// One output pin of the bench.
#[derive(Clone, Debug, PartialEq)]
pub struct OutputSpec {
    /// The engine pin (the kernel net it drives).
    pub pin: String,
    /// The value at `t = 0`.
    pub initial: Value,
    /// Changes known in advance, at `t > 0`, in time order (equal times in
    /// order).
    pub schedule: Vec<(Time, Value)>,
}

/// An action with a trigger (ADR-010 D2 `after`).
#[derive(Clone, Debug, PartialEq)]
pub struct TriggerSpec {
    /// The engine input pin (the kernel net of the MCU output it watches).
    pub input: String,
    pub level: Level,
    pub delay: Time,
    /// Index into [`BenchConfig::outputs`].
    pub output: usize,
    /// Changes relative to `t_edge + delay`, offsets increasing from 0.
    pub program: Vec<(Time, Value)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BenchConfig {
    pub name: String,
    /// The run's grant lattice `L`, for `advance_until`'s stop at `ceil_L`.
    pub stop_lattice: Option<Time>,
    pub outputs: Vec<OutputSpec>,
    pub triggers: Vec<TriggerSpec>,
}

struct Out {
    pin: String,
    value: Value,
    /// The time of the last change (a digital pin's `edge_time`).
    edge: Time,
    /// The value and edge before a change at `edge` (a change undone at the
    /// same instant restores them).
    before: (Value, Time),
}

/// The bench engine. See the module docs.
pub struct BenchEngine {
    name: String,
    stop_lattice: Option<Time>,
    outs: Vec<Out>,
    by_pin: BTreeMap<String, usize>,
    triggers: Vec<TriggerSpec>,
    armed: Vec<bool>,
    /// Input pin -> its level (`None` until the first snapshot).
    inputs: BTreeMap<String, Option<Level>>,
    now: Time,
    started: bool,
    seq: u64,
    /// Pending output changes: (time, seq) -> (output, value).
    queue: BTreeMap<(Time, u64), (usize, Value)>,
    /// Input changes delivered for the coming advance: (time, seq) -> (pin,
    /// level).
    pending: BTreeMap<(Time, u64), (String, Level)>,
    trace: Vec<Transition>,
}

impl BenchEngine {
    pub fn new(cfg: BenchConfig) -> Result<BenchEngine, String> {
        let mut e = BenchEngine {
            name: cfg.name,
            stop_lattice: cfg.stop_lattice.filter(|l| !l.is_zero()),
            outs: Vec::new(),
            by_pin: BTreeMap::new(),
            triggers: Vec::new(),
            armed: Vec::new(),
            inputs: BTreeMap::new(),
            now: Time::ZERO,
            started: false,
            seq: 0,
            queue: BTreeMap::new(),
            pending: BTreeMap::new(),
            trace: Vec::new(),
        };
        for (k, o) in cfg.outputs.into_iter().enumerate() {
            if e.by_pin.insert(o.pin.clone(), k).is_some() {
                return Err(format!("bench output {} given twice", o.pin));
            }
            let mut prev = Time::ZERO;
            for (t, v) in &o.schedule {
                if t.is_zero() || *t < prev || !v.same_kind(o.initial) {
                    return Err(format!(
                        "bench output {}: a scheduled change at {t} is not after 0, out of order \
                         or of another kind",
                        o.pin
                    ));
                }
                prev = *t;
                e.seq += 1;
                e.queue.insert((*t, e.seq), (k, *v));
            }
            e.outs.push(Out {
                pin: o.pin,
                value: o.initial,
                edge: Time::ZERO,
                before: (o.initial, Time::ZERO),
            });
        }
        for t in &cfg.triggers {
            let Some(out) = e.outs.get(t.output) else {
                return Err(format!(
                    "bench trigger on {}: no output {}",
                    t.input, t.output
                ));
            };
            if e.by_pin.contains_key(&t.input) {
                return Err(format!("bench trigger input {} is also an output", t.input));
            }
            if t.program.is_empty()
                || !t.program[0].0.is_zero()
                || t.program.windows(2).any(|w| w[1].0 <= w[0].0)
                || t.program.iter().any(|(_, v)| !v.same_kind(out.value))
            {
                return Err(format!(
                    "bench trigger on {}: its program must start at offset 0, increase and \
                     match {}'s kind",
                    t.input, out.pin
                ));
            }
            e.inputs.insert(t.input.clone(), None);
        }
        e.armed = vec![true; cfg.triggers.len()];
        e.triggers = cfg.triggers;
        Ok(e)
    }

    /// The engine's output and input pins.
    pub fn pins(&self) -> (Vec<String>, Vec<String>) {
        (
            self.outs.iter().map(|o| o.pin.clone()).collect(),
            self.inputs.keys().cloned().collect(),
        )
    }

    fn signal(o: &Out) -> PinSignal {
        match o.value {
            Value::Level(l) => PinSignal::digital(l, o.edge),
            Value::Volts(v) => PinSignal::analog(v),
        }
    }

    /// Output `k` takes `v` at `now`.
    fn set(&mut self, k: usize, v: Value) {
        let now = self.now;
        let o = &mut self.outs[k];
        if o.value.eq_exact(v) {
            return;
        }
        if o.edge == now && now > Time::ZERO {
            if let Some(pos) = self
                .trace
                .iter()
                .rposition(|tr| tr.pin == o.pin && tr.t == now)
            {
                // A second change at this instant: it replaces the first,
                // or undoes it.
                if o.before.0.eq_exact(v) {
                    self.trace.remove(pos);
                    o.value = v;
                    o.edge = o.before.1;
                    return;
                }
                o.value = v;
                self.trace[pos].signal = Self::signal(o);
                return;
            }
        }
        o.before = (o.value, o.edge);
        o.value = v;
        o.edge = now;
        self.trace.push(Transition {
            t: now,
            pin: o.pin.clone(),
            signal: Self::signal(o),
        });
    }

    fn schedule(&mut self, t: Time, k: usize, v: Value) {
        self.seq += 1;
        self.queue.insert((t, self.seq), (k, v));
    }

    /// Input `pin` changed to `level` at `now`: fire its armed triggers.
    fn input_change(&mut self, pin: &str, level: Level) -> Result<(), EngineError> {
        let prev = self.inputs.get_mut(pin).ok_or_else(|| {
            EngineError::Protocol(format!("{}: {pin} is not a bench input", self.name))
        })?;
        let was = prev.replace(level);
        if was == Some(level) || was.is_none() {
            // No change, or the first level seen (the initial state is not
            // an edge).
            return Ok(());
        }
        for i in 0..self.triggers.len() {
            if !self.armed[i] || self.triggers[i].input != pin || self.triggers[i].level != level {
                continue;
            }
            self.armed[i] = false;
            let start = self.now.saturating_add(self.triggers[i].delay);
            let (k, program) = (self.triggers[i].output, self.triggers[i].program.clone());
            for (off, v) in program {
                self.schedule(start.saturating_add(off), k, v);
            }
        }
        Ok(())
    }

    /// Handle every event at the earliest pending instant if it is `<= t`.
    fn step(&mut self, t: Time) -> Result<Option<Time>, EngineError> {
        let next = [
            self.queue.first_key_value().map(|((x, _), _)| *x),
            self.pending.first_key_value().map(|((x, _), _)| *x),
        ]
        .into_iter()
        .flatten()
        .min();
        let Some(next) = next.filter(|&n| n <= t) else {
            return Ok(None);
        };
        self.now = next;
        while let Some((&(at, s), &(k, v))) = self.queue.first_key_value() {
            if at != next {
                break;
            }
            self.queue.remove(&(at, s));
            self.set(k, v);
        }
        while let Some((&(at, s), (pin, level))) = self.pending.first_key_value() {
            if at != next {
                break;
            }
            let (pin, level) = (pin.clone(), *level);
            self.pending.remove(&(at, s));
            self.input_change(&pin, level)?;
        }
        Ok(Some(next))
    }

    fn ceil_l(&self, t: Time) -> Time {
        match self.stop_lattice {
            Some(l) => Time::from_fs(t.as_fs().div_ceil(l.as_fs()) * l.as_fs()),
            None => t,
        }
    }

    /// An input change at a boundary (`t <= now`): handled at `now`; an
    /// action it schedules must still lie ahead.
    fn boundary_change(&mut self, pin: &str, level: Level) -> Result<(), EngineError> {
        let mark = self.seq;
        self.input_change(pin, level)?;
        if let Some(((t, _), _)) = self
            .queue
            .iter()
            .find(|((t, s), _)| *s > mark && *t <= self.now)
        {
            return Err(EngineError::Protocol(format!(
                "{}: a trigger on {pin} reached the bench at the boundary {}, after its action's \
                 time {t} (ADR-010 D4: a trigger's delay must cover the plan's delivery)",
                self.name, self.now
            )));
        }
        Ok(())
    }
}

impl Engine for BenchEngine {
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
            .outs
            .iter()
            .map(|o| (o.pin.clone(), Self::signal(o)))
            .collect())
    }

    fn read_output_trace(&mut self) -> Result<Option<Vec<Transition>>, EngineError> {
        Ok(Some(self.trace.clone()))
    }

    fn apply_inputs(&mut self, inputs: &PinSignals) -> Result<(), EngineError> {
        for (pin, sig) in inputs {
            let Some(level) = sig.level() else {
                return Err(EngineError::Protocol(format!(
                    "{}: {pin}: an analog value on a bench trigger input",
                    self.name
                )));
            };
            if !self.started {
                // The initial exchange: the levels at t = 0 are not edges.
                if let Some(l) = self.inputs.get_mut(pin) {
                    *l = Some(level);
                    continue;
                }
            }
            if self.inputs.get(pin).copied().flatten() != Some(level) {
                self.boundary_change(pin, level)?;
            }
        }
        Ok(())
    }

    fn apply_input_trace(
        &mut self,
        _base: &PinSignals,
        trace: &[Transition],
    ) -> Result<(), EngineError> {
        for tr in trace {
            let Some(level) = tr.signal.level() else {
                return Err(EngineError::Protocol(format!(
                    "{}: {}: an analog value on a bench trigger input",
                    self.name, tr.pin
                )));
            };
            if !self.inputs.contains_key(&tr.pin) {
                return Err(EngineError::Protocol(format!(
                    "{}: {} is not a bench input",
                    self.name, tr.pin
                )));
            }
            if tr.t > self.now {
                self.seq += 1;
                self.pending
                    .insert((tr.t, self.seq), (tr.pin.clone(), level));
            } else {
                self.boundary_change(&tr.pin.clone(), level)?;
            }
        }
        Ok(())
    }

    fn quiet_until(&mut self, now: Time) -> Time {
        match self.queue.first_key_value().map(|((t, _), _)| *t) {
            Some(t) if t > now => t,
            Some(_) => now,
            None => Time::MAX,
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
        self.started = true;
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
        self.now = end;
        Ok(end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tr(t: Time, pin: &str, l: Level) -> Transition {
        Transition {
            t,
            pin: pin.into(),
            signal: PinSignal::digital(l, t),
        }
    }

    fn engine() -> BenchEngine {
        BenchEngine::new(BenchConfig {
            name: "bench".into(),
            stop_lattice: Some(Time::from_us(10)),
            outputs: vec![
                OutputSpec {
                    pin: "U1.PB0".into(),
                    initial: Value::Level(Level::High),
                    schedule: vec![
                        (Time::from_ns(12_345), Value::Level(Level::Low)),
                        (Time::from_us(40), Value::Level(Level::High)),
                        // Repeated level: no change.
                        (Time::from_us(41), Value::Level(Level::High)),
                    ],
                },
                OutputSpec {
                    pin: "V_bb_SW1_ctl".into(),
                    initial: Value::Volts(0.0),
                    schedule: vec![],
                },
            ],
            triggers: vec![TriggerSpec {
                input: "U1.PA5".into(),
                level: Level::High,
                delay: Time::from_us(15),
                output: 1,
                program: vec![
                    (Time::ZERO, Value::Volts(1.0)),
                    (Time::from_us(5), Value::Volts(0.0)),
                ],
            }],
        })
        .unwrap()
    }

    #[test]
    fn schedule_trigger_quiet_and_early_stop() {
        let mut e = engine();
        assert_eq!(e.next_safe_time(Time::ZERO), Time::MAX);
        assert_eq!(e.quiet_until(Time::ZERO), Time::from_ns(12_345));
        let mut init = PinSignals::new();
        init.insert("U1.PA5".into(), PinSignal::digital(Level::Low, Time::ZERO));
        e.apply_inputs(&init).unwrap();
        // The early stop at ceil_L of the first watched change.
        let r = e
            .advance_until(Time::from_ms(1), &["U1.PB0".into()])
            .unwrap();
        assert_eq!(r, Time::from_us(20));
        assert_eq!(
            e.read_output_trace().unwrap().unwrap(),
            vec![tr(Time::from_ns(12_345), "U1.PB0", Level::Low)]
        );
        // The trigger: PA5 rises at 22 us; the control is 1 from 37 us to
        // 42 us.
        e.apply_input_trace(
            &PinSignals::new(),
            &[tr(Time::from_us(22), "U1.PA5", Level::High)],
        )
        .unwrap();
        e.advance_to(Time::from_us(100)).unwrap();
        let t = e.read_output_trace().unwrap().unwrap();
        let got: Vec<(u64, &str)> = t.iter().map(|x| (x.t.as_fs(), x.pin.as_str())).collect();
        assert_eq!(
            got,
            vec![
                (37_000_000_000, "V_bb_SW1_ctl"),
                (40_000_000_000, "U1.PB0"),
                (42_000_000_000, "V_bb_SW1_ctl"),
            ]
        );
        assert_eq!(t[0].signal, PinSignal::analog(1.0));
        assert_eq!(e.quiet_until(Time::from_us(100)), Time::MAX);
        // The trigger fired once: a second rise does nothing.
        e.apply_input_trace(
            &PinSignals::new(),
            &[
                tr(Time::from_us(110), "U1.PA5", Level::Low),
                tr(Time::from_us(120), "U1.PA5", Level::High),
            ],
        )
        .unwrap();
        e.advance_to(Time::from_us(200)).unwrap();
        assert!(e.read_output_trace().unwrap().unwrap().is_empty());
    }

    #[test]
    fn a_late_trigger_is_refused() {
        let mut e = BenchEngine::new(BenchConfig {
            name: "bench".into(),
            stop_lattice: None,
            outputs: vec![OutputSpec {
                pin: "x".into(),
                initial: Value::Volts(0.0),
                schedule: vec![],
            }],
            triggers: vec![TriggerSpec {
                input: "U1.PA5".into(),
                level: Level::High,
                delay: Time::ZERO,
                output: 0,
                program: vec![(Time::ZERO, Value::Volts(1.0))],
            }],
        })
        .unwrap();
        e.advance_to(Time::from_us(10)).unwrap();
        let err = e
            .apply_input_trace(
                &PinSignals::new(),
                &[
                    tr(Time::from_us(5), "U1.PA5", Level::Low),
                    tr(Time::from_us(6), "U1.PA5", Level::High),
                ],
            )
            .unwrap_err()
            .to_string();
        assert!(err.contains("at the boundary"), "{err}");
    }
}
