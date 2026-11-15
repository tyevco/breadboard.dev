//! `dms/mechanism@1`: the dial-me-spinner detent mechanism and its home
//! switch (ADR-008 Amendment 9; `docs/firmware.md` §5.1 and §5.4 of
//! dial-me-spinner at `6854d25`).
//!
//! One device holds the micro linear stepper's rotor, the lead screw and
//! carriage, the detent magnet's field at U42 and the DRV5032FA itself,
//! because the switch's input is the magnet's field, which is internal
//! state and not a net. It reads the five driver inputs and drives HOME_N.
//!
//! **Drive (the commanded polarity, not the coil current).** For each coil
//! (A = U40, B = U41) the applied polarity is `+1` for IN1, IN2 = 1, 0; `-1`
//! for 0, 1; and `0` for brake (1, 1), coast (0, 0), or while the bridge is
//! asleep (nSLEEP low, or less than `twake` since it rose; DRV8837 Table 6.6).
//! Each coil's normalised current (1 = the full drive VM / R on that coil)
//! follows its polarity through the coil's L/R time constant
//! `tau = lph / rloop`, exactly between events: `i(t) = p + (i0 - p)
//! exp(-(t - t0) / tau)`. A 25 kHz PWM with slow decay (forward, brake)
//! therefore averages to its duty, with the ripple the edges give.
//!
//! **Rotor.** The field angle is `atan2(i_B, i_A)`. Both coils share `tau`,
//! so between two events the current vector moves on a straight line, and
//! the sweep of its angle is exact: while `|i| >= pullin` the rotor moves
//! the shortest way to the field angle; below it the rotor holds (the lead
//! screw is self-locking: zero hold power). One electrical cycle (2 pi) is
//! 4 full steps; one full step is `step_mm` of carriage (0.015 mm,
//! firmware.md §5.1). Full steps are counted from the park stop (`s = 0`)
//! to the click end (`s = stroke`). The carriage is clamped at both stops;
//! past a stop the field slips over the rotor: the lag between field and
//! rotor grows, and once it passes half a cycle the rotor is pulled back the
//! other way, so the overdrive steps of a stall are lost (§5.4 H4).
//!
//! **Field and switch.** `Bz` at the die comes from firmware.md §5.4's
//! table (die 2.4, 2.6, 2.8 and 3.5 mm above the magnet; 0, 3, 4, 5, 6 and
//! 7.5 mm from the park stop), linear in the distance and in the die
//! height (default 2.6 mm). The DRV5032FA is omnipolar: HOME_N goes low
//! once `|Bz| > bop` and high once `|Bz| < brp`, held in between (BOP/BRP
//! typ 3.0/1.5 mT, Table 6.6). It samples every `ts` (FA: 27/50/75 ms,
//! Table 6.5): windows start at `phase + k ts`, the field is read and the
//! output settles at the end of each `tactive` = 40 us window. `phase`
//! defaults to a seeded value in `(0, ts]` from the device's [`Ctx::rng`]
//! (the run seed and the instance reference). At `t = 0` the latch holds
//! the comparator's verdict for the initial position (a switch that was
//! powered and sampling before the run).
//!
//! **Not modelled** (stated limits): thrust and load (MD-4), so a stall in
//! the loaded zone cannot happen; rotor inertia, detent torque and
//! pull-out (the rotor follows at once); fast decay in coast (it decays with
//! `tau` like brake); the DRV8837's UVLO and OCP; the DRV5032's supply, its
//! power-up tON and temperature drift of BOP/BRP; the 48-magnet track's
//! 0.01 mT. Inputs are the driver *commands*: an IN pin's electrical level
//! is not seen.

use bbcore::{Level, Time};

use crate::{Ctx, Device, DevicePin, LogLevel, ParamKind, ParamSpec, ParamValue, Params, PinDir};

/// Pin indices.
pub const IN_A1: usize = 0;
pub const IN_A2: usize = 1;
pub const IN_B1: usize = 2;
pub const IN_B2: usize = 3;
pub const NSLEEP: usize = 4;
pub const HOME_N: usize = 5;

const PINS: [DevicePin; 6] = [
    DevicePin {
        name: "IN_A1",
        dir: PinDir::In,
    },
    DevicePin {
        name: "IN_A2",
        dir: PinDir::In,
    },
    DevicePin {
        name: "IN_B1",
        dir: PinDir::In,
    },
    DevicePin {
        name: "IN_B2",
        dir: PinDir::In,
    },
    DevicePin {
        name: "NSLEEP",
        dir: PinDir::In,
    },
    DevicePin {
        name: "HOME_N",
        dir: PinDir::Out,
    },
];

/// Timer ids.
const T_SAMPLE: u32 = 1;
const T_WAKE: u32 = 2;

/// The parameters of a `dms/mechanism@1` part (the board file's keys).
pub const PARAMS: &[ParamSpec] = &[
    ParamSpec {
        key: "s0",
        kind: ParamKind::Number,
        default: Some(ParamValue::Real(500.0)),
        doc: "initial carriage position, full steps from the park stop (500 = the click end)",
    },
    ParamSpec {
        key: "stroke",
        kind: ParamKind::Number,
        default: Some(ParamValue::Real(500.0)),
        doc: "full steps from the park stop to the click end (firmware.md 5.1)",
    },
    ParamSpec {
        key: "phase0",
        kind: ParamKind::Number,
        default: Some(ParamValue::Real(0.0)),
        doc: "the rotor's initial electrical full-step position, 0-3 (field angle phase0 x 90 degrees)",
    },
    ParamSpec {
        key: "dir",
        kind: ParamKind::Number,
        default: Some(ParamValue::Real(1.0)),
        doc: "+1: an increasing field angle moves the carriage away from the park stop; -1: towards it",
    },
    ParamSpec {
        key: "step_mm",
        kind: ParamKind::Number,
        default: Some(ParamValue::Real(0.015)),
        doc: "carriage travel per full step, mm (firmware.md 5.1)",
    },
    ParamSpec {
        key: "lph",
        kind: ParamKind::Inductance,
        default: Some(ParamValue::Real(4.3e-3)),
        doc: "phase inductance (motor_drive__stepper.lib lph, 4.3 mH nominal)",
    },
    ParamSpec {
        key: "rloop",
        kind: ParamKind::Resistance,
        default: Some(ParamValue::Real(20.28)),
        doc: "coil loop resistance: 20 Ohm phase at 25 C plus the DRV8837's 0.28 Ohm HS+LS",
    },
    ParamSpec {
        key: "pullin",
        kind: ParamKind::Number,
        default: Some(ParamValue::Real(0.2)),
        doc: "normalised current-vector magnitude above which the rotor follows the field (1 = VM/R)",
    },
    ParamSpec {
        key: "twake",
        kind: ParamKind::Time,
        default: Some(ParamValue::Time(Time::from_us(30))),
        doc: "DRV8837 wake time after nSLEEP rises (Table 6.6 maximum)",
    },
    ParamSpec {
        key: "ts",
        kind: ParamKind::Time,
        default: Some(ParamValue::Time(Time::from_ms(50))),
        doc: "DRV5032FA sampling period (Table 6.5: 27 / 50 / 75 ms)",
    },
    ParamSpec {
        key: "tactive",
        kind: ParamKind::Time,
        default: Some(ParamValue::Time(Time::from_us(40))),
        doc: "DRV5032 active window; the output settles at its end",
    },
    ParamSpec {
        key: "phase",
        kind: ParamKind::Time,
        default: None,
        doc: "start of the first sampling window, in (0, ts]; default: seeded from the run seed and the instance reference",
    },
    ParamSpec {
        key: "bop",
        kind: ParamKind::Number,
        default: Some(ParamValue::Real(3.0)),
        doc: "operate point |B|, mT (DRV5032FA Table 6.6: 1.5 / 3 / 4.8)",
    },
    ParamSpec {
        key: "brp",
        kind: ParamKind::Number,
        default: Some(ParamValue::Real(1.5)),
        doc: "release point |B|, mT (Table 6.6: 0.5 / 1.5 / 3)",
    },
    ParamSpec {
        key: "die",
        kind: ParamKind::Number,
        default: Some(ParamValue::Real(2.6)),
        doc: "die height above the magnet's top face, mm (2.4 to 3.5, firmware.md 5.4)",
    },
];

/// Distances from the park stop of firmware.md §5.4's table (mm).
const TABLE_D: [f64; 6] = [0.0, 3.0, 4.0, 5.0, 6.0, 7.5];
/// Its rows: die height (mm) and Bz (mT) at each distance.
const TABLE_B: [(f64, [f64; 6]); 4] = [
    (2.4, [70.9, 12.9, 3.4, -0.2, -1.3, -1.40]),
    (2.6, [60.9, 12.9, 4.0, 0.4, -0.9, -1.22]),
    (2.8, [52.6, 12.7, 4.5, 0.8, -0.6, -1.03]),
    (3.5, [32.9, 11.2, 5.2, 2.0, 0.4, -0.47]),
];

/// Linear interpolation of `ys` over `xs` (increasing) at `x`, clamped.
fn interp(xs: &[f64], ys: &[f64], x: f64) -> f64 {
    if x <= xs[0] {
        return ys[0];
    }
    for k in 1..xs.len() {
        if x <= xs[k] {
            let f = (x - xs[k - 1]) / (xs[k] - xs[k - 1]);
            return ys[k - 1] + f * (ys[k] - ys[k - 1]);
        }
    }
    ys[ys.len() - 1]
}

/// `Bz` (mT) at the die, `d_mm` from the park stop, the die `die_mm` above
/// the magnet: linear in `d` along each row, then linear between rows.
pub fn field_mt(d_mm: f64, die_mm: f64) -> f64 {
    let d = d_mm.clamp(TABLE_D[0], TABLE_D[TABLE_D.len() - 1]);
    let heights: Vec<f64> = TABLE_B.iter().map(|(h, _)| *h).collect();
    let at: Vec<f64> = TABLE_B
        .iter()
        .map(|(_, b)| interp(&TABLE_D, b, d))
        .collect();
    interp(&heights, &at, die_mm)
}

/// `x` wrapped into `(-pi, pi]`.
fn wrap(x: f64) -> f64 {
    use std::f64::consts::{PI, TAU};
    let mut y = x % TAU;
    if y > PI {
        y -= TAU;
    } else if y <= -PI {
        y += TAU;
    }
    y
}

/// The mechanism and its DRV5032.
pub struct Mechanism {
    // --- parameters
    tau_s: f64,
    pullin: f64,
    twake: Time,
    ts: Time,
    tactive: Time,
    phase: Option<Time>,
    bop: f64,
    brp: f64,
    die: f64,
    step_mm: f64,
    s0: f64,
    dir: f64,
    theta0: f64,
    theta_min: f64,
    theta_max: f64,
    // --- state
    inputs: [Level; 5],
    /// The bridges are awake (nSLEEP high for at least `twake`).
    active: bool,
    /// Normalised coil currents at `t_last`, and the polarities applied
    /// since then.
    i: [f64; 2],
    p: [f64; 2],
    t_last: Time,
    /// Rotor electrical angle (unwrapped, rad) and the field's lead over it.
    theta: f64,
    lag: f64,
    /// The DRV5032's latch: field above BOP (HOME_N low).
    home: bool,
    /// Sampling windows handled (diagnostics).
    samples: u64,
    /// The rotor angles of the park stop and the click end.
    theta_park: f64,
    theta_click: f64,
    /// What the last event's position gave, for the records (`watch`):
    /// `|Bz| > bop`, `|Bz| < brp`, and the stop the carriage touches (-1
    /// park, 1 click, 0 none).
    above_bop: bool,
    below_brp: bool,
    at_stop: i8,
}

/// Build a `dms/mechanism@1` from its parameters.
pub fn build(p: &Params) -> Result<Box<dyn Device>, String> {
    Mechanism::new(p).map(|m| Box::new(m) as Box<dyn Device>)
}

impl Mechanism {
    pub fn new(p: &Params) -> Result<Mechanism, String> {
        use std::f64::consts::FRAC_PI_2;
        let stroke = p.real("stroke")?;
        let s0 = p.real("s0")?;
        let dir = p.real("dir")?;
        let phase0 = p.real("phase0")?;
        let lph = p.real("lph")?;
        let rloop = p.real("rloop")?;
        let pullin = p.real("pullin")?;
        let ts = p.time("ts")?;
        let tactive = p.time("tactive")?;
        let phase = p.time_opt("phase")?;
        let bop = p.real("bop")?;
        let brp = p.real("brp")?;
        let die = p.real("die")?;
        let step_mm = p.real("step_mm")?;
        if !(stroke > 0.0 && stroke.fract() == 0.0) {
            return Err(format!("stroke={stroke}: a positive whole number of steps"));
        }
        if !(0.0..=stroke).contains(&s0) {
            return Err(format!("s0={s0}: outside 0..stroke ({stroke})"));
        }
        if dir != 1.0 && dir != -1.0 {
            return Err(format!("dir={dir}: 1 or -1"));
        }
        if !(phase0.fract() == 0.0 && (0.0..4.0).contains(&phase0)) {
            return Err(format!("phase0={phase0}: 0, 1, 2 or 3"));
        }
        if !(lph > 0.0 && rloop > 0.0) {
            return Err("lph and rloop must be > 0".into());
        }
        if !(pullin > 0.0 && pullin < 1.0) {
            return Err(format!("pullin={pullin}: between 0 and 1"));
        }
        if ts.is_zero() || tactive >= ts {
            return Err(format!("ts={ts}, tactive={tactive}: need 0 < tactive < ts"));
        }
        if let Some(ph) = phase {
            if ph.is_zero() || ph > ts {
                return Err(format!("phase={ph}: in (0, ts]"));
            }
        }
        if !(brp < bop && brp >= 0.0) {
            return Err(format!("bop={bop}, brp={brp}: need 0 <= brp < bop"));
        }
        if !(2.4..=3.5).contains(&die) {
            return Err(format!("die={die}: the table covers 2.4 to 3.5 mm"));
        }
        if step_mm.is_nan() || step_mm <= 0.0 {
            return Err("step_mm must be > 0".into());
        }
        let theta0 = phase0 * FRAC_PI_2;
        // theta = theta0 + dir (s - s0) pi/2.
        let at = |s: f64| theta0 + dir * (s - s0) * FRAC_PI_2;
        let (a, b) = (at(0.0), at(stroke));
        Ok(Mechanism {
            tau_s: lph / rloop,
            pullin,
            twake: p.time("twake")?,
            ts,
            tactive,
            phase,
            bop,
            brp,
            die,
            step_mm,
            s0,
            dir,
            theta0,
            theta_min: a.min(b),
            theta_max: a.max(b),
            inputs: [Level::Low; 5],
            active: false,
            i: [0.0; 2],
            p: [0.0; 2],
            t_last: Time::ZERO,
            theta: theta0,
            lag: 0.0,
            home: false,
            samples: 0,
            theta_park: a,
            theta_click: b,
            above_bop: false,
            below_brp: false,
            at_stop: 0,
        })
    }

    /// The comparator inputs and the stop contact of the current position.
    fn position_state(&self) -> (bool, bool, i8) {
        let b = self.field().abs();
        let stop = if self.theta == self.theta_park {
            -1
        } else if self.theta == self.theta_click {
            1
        } else {
            0
        };
        (b > self.bop, b < self.brp, stop)
    }

    /// Records (no output, rule 1) at an event where the position crossed
    /// BOP or BRP, or touched or left a stop: the instant the device's
    /// field model passes a threshold is the event that moved the rotor
    /// there (the rotor moves only at events), which is what the HOME_N
    /// latency gate measures from (ADR-008 Amendment 10).
    fn watch(&mut self, ctx: &mut Ctx) {
        let (above, below, stop) = self.position_state();
        let s = self.steps();
        let b = self.field().abs();
        if above != self.above_bop {
            ctx.log(
                LogLevel::Info,
                format!(
                    "field {} BOP: |Bz| {b:.4} mT {} {} mT at s={s:.3}",
                    if above { "above" } else { "below" },
                    if above { ">" } else { "<=" },
                    self.bop
                ),
            );
        }
        if below != self.below_brp {
            ctx.log(
                LogLevel::Info,
                format!(
                    "field {} BRP: |Bz| {b:.4} mT {} {} mT at s={s:.3}",
                    if below { "below" } else { "above" },
                    if below { "<" } else { ">=" },
                    self.brp
                ),
            );
        }
        if stop != self.at_stop {
            let what = match (self.at_stop, stop) {
                (_, -1) => "carriage at the park stop".to_string(),
                (_, 1) => "carriage at the click end".to_string(),
                (-1, _) => format!("carriage left the park stop at s={s:.3}"),
                _ => format!("carriage left the click end at s={s:.3}"),
            };
            ctx.log(LogLevel::Info, what);
        }
        (self.above_bop, self.below_brp, self.at_stop) = (above, below, stop);
    }

    /// The carriage position in full steps from the park stop.
    pub fn steps(&self) -> f64 {
        self.s0 + self.dir * (self.theta - self.theta0) / std::f64::consts::FRAC_PI_2
    }

    /// The carriage's distance from the park stop (mm).
    pub fn distance_mm(&self) -> f64 {
        self.steps() * self.step_mm
    }

    /// `Bz` at the die for the current position (mT).
    pub fn field(&self) -> f64 {
        field_mt(self.distance_mm(), self.die)
    }

    /// The normalised coil currents at the last event.
    pub fn currents(&self) -> [f64; 2] {
        self.i
    }

    /// The DRV5032 latch (field above BOP).
    pub fn home(&self) -> bool {
        self.home
    }

    /// The polarity of coil `c` from its inputs.
    fn polarity(&self, c: usize) -> f64 {
        if !self.active {
            return 0.0;
        }
        let (a, b) = (self.inputs[2 * c], self.inputs[2 * c + 1]);
        match (a, b) {
            (Level::High, Level::Low) => 1.0,
            (Level::Low, Level::High) => -1.0,
            _ => 0.0,
        }
    }

    /// Advance the coil currents and the rotor from `t_last` to `t` under
    /// the polarities in force (rule 3: called only at events).
    fn advance(&mut self, t: Time) {
        if t <= self.t_last {
            return;
        }
        let dt = (t - self.t_last).as_secs_f64();
        let k = (-dt / self.tau_s).exp();
        let a = self.i;
        let b = [
            self.p[0] + (a[0] - self.p[0]) * k,
            self.p[1] + (a[1] - self.p[1]) * k,
        ];
        self.follow(a, b);
        self.i = b;
        self.t_last = t;
    }

    /// Move the rotor (shortest way, clamped) by the current lag.
    fn settle(&mut self) {
        self.lag = wrap(self.lag);
        let c = (self.theta + self.lag).clamp(self.theta_min, self.theta_max);
        self.lag -= c - self.theta;
        self.theta = c;
    }

    /// The rotor while the current vector moves on the straight line from
    /// `a` to `b`: it follows the field where `|i| >= pullin`.
    fn follow(&mut self, a: [f64; 2], b: [f64; 2]) {
        let r2 = self.pullin * self.pullin;
        let d = [b[0] - a[0], b[1] - a[1]];
        let qa = d[0] * d[0] + d[1] * d[1];
        let qb = 2.0 * (a[0] * d[0] + a[1] * d[1]);
        let qc = a[0] * a[0] + a[1] * a[1] - r2;
        // The parts of u in [0, 1] where |a + u d| >= pullin.
        let mut above: Vec<(f64, f64)> = Vec::with_capacity(2);
        if qa == 0.0 {
            if qc >= 0.0 {
                above.push((0.0, 1.0));
            }
        } else {
            let disc = qb * qb - 4.0 * qa * qc;
            if disc <= 0.0 {
                above.push((0.0, 1.0));
            } else {
                let s = disc.sqrt();
                let (u1, u2) = ((-qb - s) / (2.0 * qa), (-qb + s) / (2.0 * qa));
                if u1 > 0.0 {
                    above.push((0.0, u1.min(1.0)));
                }
                if u2 < 1.0 {
                    above.push((u2.max(0.0), 1.0));
                }
            }
        }
        let at = |u: f64| [a[0] + u * d[0], a[1] + u * d[1]];
        let angle = |p: [f64; 2]| p[1].atan2(p[0]);
        let mut held = qc < 0.0;
        for (ua, ub) in above {
            if ub < ua {
                continue;
            }
            let (pa, pb) = (at(ua), at(ub));
            if held {
                // Pull-in: the rotor moves the shortest way to the field.
                self.lag = wrap(angle(pa) - self.theta);
                self.settle();
            }
            self.lag += wrap(angle(pb) - angle(pa));
            self.settle();
            held = ub < 1.0;
        }
    }

    fn update_polarity(&mut self) {
        self.p = [self.polarity(0), self.polarity(1)];
    }

    /// The comparator with hysteresis on `|Bz|`.
    fn latch(&mut self) {
        let b = self.field().abs();
        if b > self.bop {
            self.home = true;
        } else if b < self.brp {
            self.home = false;
        }
    }

    fn home_level(&self) -> Level {
        if self.home {
            Level::Low
        } else {
            Level::High
        }
    }
}

impl Device for Mechanism {
    fn pins(&self) -> &[DevicePin] {
        &PINS
    }

    fn start(&mut self, ctx: &mut Ctx) {
        self.home = self.field().abs() > self.bop;
        ctx.set_pin(HOME_N, self.home_level());
        let (seeded, phase) = match self.phase {
            Some(p) => (false, p),
            None => (true, Time::from_fs(1 + ctx.rng().below(self.ts.as_fs()))),
        };
        ctx.schedule(phase + self.tactive, T_SAMPLE);
        (self.above_bop, self.below_brp, self.at_stop) = self.position_state();
        ctx.log(
            LogLevel::Info,
            format!(
                "DRV5032 tS {} ms, tactive {} us, BOP {} mT, BRP {} mT, die {} mm; first window \
                 at {} fs ({}); s={:.3}, |Bz| {:.4} mT, HOME_N {}",
                self.ts.as_fs() as f64 / 1e12,
                self.tactive.as_fs() as f64 / 1e9,
                self.bop,
                self.brp,
                self.die,
                phase.as_fs(),
                if seeded {
                    "seeded phase"
                } else {
                    "phase parameter"
                },
                self.steps(),
                self.field().abs(),
                if self.home { "low" } else { "high" }
            ),
        );
    }

    fn on_pin_change(&mut self, ctx: &mut Ctx, pin: usize, level: Level, t: Time) {
        self.advance(t);
        self.watch(ctx);
        self.inputs[pin] = level;
        if pin == NSLEEP {
            ctx.log(
                LogLevel::Info,
                format!(
                    "nSLEEP {} at s={:.3} (rotor {:.4} full steps)",
                    if level == Level::High { "high" } else { "low" },
                    self.steps(),
                    self.theta / std::f64::consts::FRAC_PI_2
                ),
            );
            if level == Level::High {
                if self.twake.is_zero() {
                    self.active = true;
                } else {
                    ctx.schedule(t + self.twake, T_WAKE);
                }
            } else {
                self.active = false;
                ctx.cancel(T_WAKE);
            }
        }
        self.update_polarity();
    }

    fn on_timer(&mut self, ctx: &mut Ctx, id: u32, t: Time) {
        self.advance(t);
        self.watch(ctx);
        match id {
            T_WAKE => {
                self.active = true;
                self.update_polarity();
            }
            T_SAMPLE => {
                self.samples += 1;
                let was = self.home;
                self.latch();
                if self.home != was {
                    ctx.log(
                        LogLevel::Info,
                        format!(
                            "HOME_N {} at sample {} (s={:.3}, |Bz| {:.4} mT)",
                            if self.home { "low" } else { "high" },
                            self.samples,
                            self.steps(),
                            self.field().abs()
                        ),
                    );
                }
                ctx.set_pin(HOME_N, self.home_level());
                ctx.schedule(t + self.ts, T_SAMPLE);
            }
            _ => ctx.fail(format!("unknown timer {id}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{find, DeviceEngine, DeviceEngineConfig, DevicePart, Rng};
    use bbcore::{Engine, PinSignal, PinSignals, Transition};

    fn params(given: &[(&str, ParamValue)]) -> Params {
        let d = find("dms/mechanism@1").unwrap();
        let g: Vec<(String, ParamValue)> = given.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        d.params(&g).unwrap()
    }

    fn mech(given: &[(&str, ParamValue)]) -> Mechanism {
        Mechanism::new(&params(given)).unwrap()
    }

    fn real(x: f64) -> ParamValue {
        ParamValue::Real(x)
    }

    /// Drive the device directly (no engine): set inputs at `t`.
    struct Rig {
        m: Mechanism,
        rng: Rng,
        out: Vec<(Time, Level)>,
        timers: std::collections::BTreeMap<u32, Time>,
    }

    impl Rig {
        fn new(m: Mechanism) -> Rig {
            let mut r = Rig {
                m,
                rng: Rng::for_instance(1, "M1"),
                out: Vec::new(),
                timers: Default::default(),
            };
            let mut ctx = Ctx::new(Time::ZERO, false, &mut r.rng);
            r.m.start(&mut ctx);
            let cmds = ctx.cmds;
            r.apply(Time::ZERO, cmds);
            r
        }
        fn apply(&mut self, t: Time, cmds: Vec<crate::Cmd>) {
            for c in cmds {
                match c {
                    crate::Cmd::Schedule(id, at) => {
                        self.timers.insert(id, at);
                    }
                    crate::Cmd::Cancel(id) => {
                        self.timers.remove(&id);
                    }
                    crate::Cmd::SetPin(_, l) => self.out.push((t, l)),
                    crate::Cmd::Log(..) => {}
                }
            }
        }
        /// Fire every timer up to and including `t`, in time order.
        fn run_to(&mut self, t: Time) {
            loop {
                let next = self
                    .timers
                    .iter()
                    .map(|(id, at)| (*at, *id))
                    .min()
                    .filter(|(at, _)| *at <= t);
                let Some((at, id)) = next else { break };
                self.timers.remove(&id);
                let mut ctx = Ctx::new(at, false, &mut self.rng);
                self.m.on_timer(&mut ctx, id, at);
                assert!(ctx.error.is_none(), "{:?}", ctx.error);
                let cmds = ctx.cmds;
                self.apply(at, cmds);
            }
            self.m.advance(t);
        }
        fn set(&mut self, t: Time, pin: usize, level: Level) {
            self.run_to(t);
            let mut ctx = Ctx::new(t, true, &mut self.rng);
            self.m.on_pin_change(&mut ctx, pin, level, t);
            assert!(ctx.error.is_none(), "{:?}", ctx.error);
            let cmds = ctx.cmds;
            self.apply(t, cmds);
        }
        /// The two-phase full-step state with field angle 45 + 90 k degrees.
        fn full_step(&mut self, t: Time, k: i64) {
            let ang = (45.0 + 90.0 * k as f64).to_radians();
            let (a, b) = (ang.cos() > 0.0, ang.sin() > 0.0);
            let lv = Level::from_bool;
            self.set(t, IN_A1, lv(a));
            self.set(t, IN_A2, lv(!a));
            self.set(t, IN_B1, lv(b));
            self.set(t, IN_B2, lv(!b));
        }
    }

    fn ms(x: u64) -> Time {
        Time::from_ms(x)
    }

    #[test]
    fn field_table_and_edges() {
        assert_eq!(field_mt(0.0, 2.6), 60.9);
        assert_eq!(field_mt(7.5, 2.6), -1.22);
        assert_eq!(field_mt(4.0, 3.5), 5.2);
        assert!((field_mt(4.5, 2.6) - 2.2).abs() < 1e-12);
        // Between rows: halfway between 2.4 and 2.6 mm at the stop.
        assert!((field_mt(0.0, 2.5) - 65.9).abs() < 1e-9);
        // firmware.md 5.4: at 2.6 mm the assert edge (3 mT) and the release
        // edge (1.5 mT) lie inside the stated ranges (s_ast 253-307, s_rel
        // 273-333 steps).
        let edge = |bt: f64| {
            (0..=500)
                .map(|s| s as f64)
                .find(|s| field_mt(s * 0.015, 2.6).abs() < bt)
                .unwrap()
        };
        let (s_ast, s_rel) = (edge(3.0), edge(1.5));
        assert!((253.0..=307.0).contains(&s_ast), "{s_ast}");
        assert!((273.0..=333.0).contains(&s_rel), "{s_rel}");
        assert!(s_rel > s_ast);
        // |B| at the click end is below BOP min: no false home.
        assert!(field_mt(7.5, 2.4).abs() < 1.5);
    }

    #[test]
    fn coil_current_follows_lr() {
        let mut r = Rig::new(mech(&[]));
        r.set(Time::from_us(10), NSLEEP, Level::High);
        // Asleep until twake: no current yet.
        r.set(Time::from_us(20), IN_A1, Level::High);
        r.run_to(Time::from_us(40));
        assert_eq!(r.m.currents(), [0.0, 0.0]);
        // Awake at 40 us; one tau later the current is 1 - 1/e.
        let tau: f64 = 4.3e-3 / 20.28;
        let t1 = Time::from_us(40) + Time::from_fs((tau * 1e15).round() as u64);
        r.run_to(t1);
        let ia = r.m.currents()[0];
        assert!((ia - (1.0 - (-1.0f64).exp())).abs() < 1e-9, "{ia}");
        // Brake: decays towards 0 with the same tau.
        r.set(t1, IN_A2, Level::High);
        let t2 = t1 + Time::from_fs((tau * 1e15).round() as u64);
        r.run_to(t2);
        let ib = r.m.currents()[0];
        assert!((ib - ia / std::f64::consts::E).abs() < 1e-9, "{ib}");
        // nSLEEP low: polarity 0 at once.
        r.set(t2, IN_A2, Level::Low);
        r.set(t2, NSLEEP, Level::Low);
        r.run_to(t2 + ms(10));
        assert!(r.m.currents()[0].abs() < 1e-15);
    }

    #[test]
    fn full_steps_move_one_step_each() {
        // Start at s = 250, rotor at phase 0 (field 0 deg).
        let mut r = Rig::new(mech(&[("s0", real(250.0))]));
        r.set(Time::from_us(5), NSLEEP, Level::High);
        // Energise k = 0 (45 deg): the rotor pulls in to 45 deg = +0.5 step.
        r.full_step(ms(1), 0);
        r.run_to(ms(11));
        assert!((r.m.steps() - 250.5).abs() < 1e-6, "{}", r.m.steps());
        // Four more full steps forward (one electrical cycle): +4 steps.
        for k in 1..=4 {
            r.full_step(ms(11 + 3 * k as u64), k);
        }
        r.run_to(ms(40));
        assert!((r.m.steps() - 254.5).abs() < 1e-6, "{}", r.m.steps());
        // And back: two steps towards the park stop.
        r.full_step(ms(41), 3);
        r.full_step(ms(44), 2);
        r.run_to(ms(50));
        assert!((r.m.steps() - 252.5).abs() < 1e-6, "{}", r.m.steps());
        // dir = -1 reverses the direction.
        let mut r = Rig::new(mech(&[("s0", real(250.0)), ("dir", real(-1.0))]));
        r.set(Time::from_us(5), NSLEEP, Level::High);
        r.full_step(ms(1), 0);
        r.full_step(ms(11), 1);
        r.run_to(ms(20));
        assert!((r.m.steps() - 248.5).abs() < 1e-6, "{}", r.m.steps());
    }

    /// The firmware's 8-microstep sine drive with slow decay (firmware.md
    /// 5.3: forward for |d| of each 25 kHz period, then brake), as input
    /// changes `(t, pin, level)` in time order, starting from all inputs
    /// low except nSLEEP; `toward` = +1 or -1 (the field's direction).
    /// Returns the events and the end time.
    fn pwm_events(
        t0: Time,
        n_full: i64,
        d: f64,
        refresh: u64,
        toward: f64,
    ) -> (Vec<(Time, usize, Level)>, Time) {
        let period = Time::from_us(40);
        let mut levels = [Level::Low; 4];
        let mut ev = Vec::new();
        let mut put = |ev: &mut Vec<(Time, usize, Level)>, t: Time, pin: usize, l: Level| {
            if levels[pin] != l {
                levels[pin] = l;
                ev.push((t, pin, l));
            }
        };
        let mut t = t0;
        for u in 0..(8 * n_full) {
            let th = toward * (u as f64 + 1.0) * std::f64::consts::FRAC_PI_2 / 8.0;
            let c = [d * th.cos(), d * th.sin()];
            for _ in 0..refresh {
                let mut off = Vec::new();
                for (k, ck) in c.iter().enumerate() {
                    let on = Time::from_fs((period.as_fs() as f64 * ck.abs()).round() as u64);
                    if on.is_zero() {
                        // No forward part: brake for the whole period.
                        put(&mut ev, t, 2 * k, Level::High);
                        put(&mut ev, t, 2 * k + 1, Level::High);
                        continue;
                    }
                    put(&mut ev, t, 2 * k, Level::from_bool(*ck >= 0.0));
                    put(&mut ev, t, 2 * k + 1, Level::from_bool(*ck < 0.0));
                    if on < period {
                        off.push((on, k));
                    }
                }
                off.sort();
                for (on, k) in off {
                    put(&mut ev, t + on, 2 * k, Level::High);
                    put(&mut ev, t + on, 2 * k + 1, Level::High);
                }
                t = t + period;
            }
        }
        ev.sort_by_key(|e| e.0);
        (ev, t)
    }

    fn microstep(r: &mut Rig, t0: Time, n_full: i64, d: f64, refresh: u64, toward: f64) -> Time {
        // The rig's own input levels start where pwm_events assumes (low).
        for p in [IN_A1, IN_A2, IN_B1, IN_B2] {
            r.set(t0, p, Level::Low);
        }
        let (ev, end) = pwm_events(t0, n_full, d, refresh, toward);
        for (t, pin, l) in ev {
            r.set(t, pin, l);
        }
        end
    }

    #[test]
    fn microstepping_moves_four_steps_per_cycle_and_hold_is_free() {
        let mut r = Rig::new(mech(&[("s0", real(250.0))]));
        r.set(Time::from_us(5), NSLEEP, Level::High);
        // Energise phase 0 (A only, 0 deg) for 10 ms.
        r.set(ms(1), IN_A1, Level::High);
        r.run_to(ms(11));
        assert!((r.m.steps() - 250.0).abs() < 1e-3, "{}", r.m.steps());
        // One electrical cycle (4 full steps) at 446 PPS (REFRESH + 1 = 7).
        let t = microstep(&mut r, ms(11), 4, 0.88, 7, 1.0);
        r.run_to(t + ms(2));
        assert!((r.m.steps() - 254.0).abs() < 0.05, "{}", r.m.steps());
        // Brake then sleep: the rotor holds (self-locking screw).
        let s = r.m.steps();
        for p in [IN_A1, IN_A2, IN_B1, IN_B2] {
            r.set(t + ms(3), p, Level::High);
        }
        for p in [IN_A1, IN_A2, IN_B1, IN_B2] {
            r.set(t + ms(6), p, Level::Low);
        }
        r.set(t + ms(6), NSLEEP, Level::Low);
        r.run_to(t + ms(50));
        assert!((r.m.steps() - s).abs() < 1e-9);
    }

    #[test]
    fn park_stop_clamps_and_the_overdrive_is_lost() {
        // From s = 6 towards the park stop by 20 full steps: the carriage
        // stops at 0 and the field slips; reversing then moves away from the
        // stop within half a cycle, not after the 14 lost steps.
        let mut r = Rig::new(mech(&[("s0", real(6.0))]));
        r.set(Time::from_us(5), NSLEEP, Level::High);
        r.full_step(ms(1), 0); // 45 deg: s = 6.5
        r.run_to(ms(11));
        let mut k = 0;
        let mut t = ms(11);
        for _ in 0..20 {
            k -= 1;
            t = t + ms(3);
            r.full_step(t, k);
        }
        r.run_to(t + ms(3));
        let s_stall = r.m.steps();
        assert!((0.0..=2.0).contains(&s_stall), "{s_stall}");
        for _ in 0..4 {
            k += 1;
            t = t + ms(3);
            r.full_step(t, k);
        }
        r.run_to(t + ms(3));
        let s_back = r.m.steps();
        assert!((2.0..=6.0).contains(&s_back), "{s_stall} -> {s_back}");
        // The click end clamps too.
        let mut r = Rig::new(mech(&[("s0", real(499.0))]));
        r.set(Time::from_us(5), NSLEEP, Level::High);
        r.full_step(ms(1), 0);
        let mut t = ms(11);
        for k in 1..=8 {
            t = t + ms(3);
            r.full_step(t, k);
        }
        r.run_to(t + ms(3));
        assert!(
            r.m.steps() <= 500.0 && r.m.steps() >= 498.0,
            "{}",
            r.m.steps()
        );
    }

    #[test]
    fn below_pullin_the_rotor_holds() {
        // A 10 % duty on coil B alone never pulls the rotor (0.1 < 0.2).
        let mut r = Rig::new(mech(&[("s0", real(100.0))]));
        r.set(Time::from_us(5), NSLEEP, Level::High);
        let t = microstep(&mut r, ms(1), 1, 0.1, 30, 1.0);
        r.run_to(t);
        assert_eq!(r.m.steps(), 100.0);
    }

    #[test]
    fn drv5032_samples_with_hysteresis_at_ts() {
        // Parked (s = 0): HOME_N low from t = 0. Phase 10 ms, ts 50 ms.
        let p = [("s0", real(0.0)), ("phase", ParamValue::Time(ms(10)))];
        let mut r = Rig::new(mech(&p));
        assert_eq!(r.out, vec![(Time::ZERO, Level::Low)]);
        assert_eq!(r.timers[&T_SAMPLE], ms(10) + Time::from_us(40));
        r.run_to(ms(200));
        // Every sample sets the (unchanged) level at phase + tactive + k ts.
        let times: Vec<Time> = r.out.iter().skip(1).map(|(t, _)| *t).collect();
        let want: Vec<Time> = (0..4)
            .map(|k| ms(10) + Time::from_us(40) + ms(50 * k))
            .collect();
        assert_eq!(times, want);
        assert!(r.out.iter().all(|(_, l)| *l == Level::Low));
        // The seeded phase is in (0, ts], the same for the same seed and
        // reference.
        let a = Rig::new(mech(&[]));
        let b = Rig::new(mech(&[]));
        assert_eq!(a.timers[&T_SAMPLE], b.timers[&T_SAMPLE]);
        let first = a.timers[&T_SAMPLE] - Time::from_us(40);
        assert!(first > Time::ZERO && first <= ms(50));
        // At the click end (|B| 1.22 mT < BRP): HOME_N high.
        assert_eq!(a.out, vec![(Time::ZERO, Level::High)]);
        // Hysteresis: a position between the two edges keeps the latch.
        let mut m = mech(&[("s0", real(300.0))]);
        assert!(field_mt(300.0 * 0.015, 2.6).abs() > 1.5);
        assert!(field_mt(300.0 * 0.015, 2.6).abs() < 3.0);
        m.home = true;
        m.latch();
        assert!(m.home);
        m.home = false;
        m.latch();
        assert!(!m.home);
    }

    #[test]
    fn parameters_are_checked() {
        let d = find("dms/mechanism@1").unwrap();
        for bad in [
            ("s0", real(501.0)),
            ("dir", real(0.0)),
            ("phase0", real(1.5)),
            ("die", real(4.0)),
            ("pullin", real(1.0)),
            ("brp", real(3.0)),
            ("phase", ParamValue::Time(Time::ZERO)),
            ("tactive", ParamValue::Time(ms(60))),
        ] {
            let e = d.instantiate(&[(bad.0.to_string(), bad.1)]);
            assert!(e.is_err(), "{bad:?}");
        }
    }

    /// A homing-like input trace: wake, energise, then microsteps towards
    /// the park stop at 446 PPS from s = 330, crossing the assert edge.
    fn homing_trace() -> Vec<Transition> {
        let names = ["M1.IN_A1", "M1.IN_A2", "M1.IN_B1", "M1.IN_B2", "M1.NSLEEP"];
        let tr = |t: Time, pin: usize, l: Level| Transition {
            t,
            pin: names[pin].into(),
            signal: PinSignal::digital(l, t),
        };
        let mut log = vec![tr(Time::from_ns(1_003), NSLEEP, Level::High)];
        let (ev, _) = pwm_events(ms(1) + Time::from_fs(17), 60, 0.88, 7, -1.0);
        log.extend(ev.into_iter().map(|(t, pin, l)| tr(t, pin, l)));
        log
    }

    /// Run the trace through a DeviceEngine, delivered per grant of `grant`
    /// (pre-advance, as the kernel does for a consumer after its producer).
    fn run_engine(
        trace: &[Transition],
        grant: Time,
        end: Time,
        seed: u64,
    ) -> (Vec<Transition>, u64) {
        let def = find("dms/mechanism@1").unwrap();
        let dev = def.instantiate(&[("s0".to_string(), real(330.0))]).unwrap();
        let mut e = DeviceEngine::new(
            DeviceEngineConfig {
                name: "devices".into(),
                seed,
                stop_lattice: Some(Time::from_us(10)),
            },
            vec![DevicePart {
                reference: "M1".into(),
                device: dev,
            }],
        )
        .unwrap();
        let mut out = Vec::new();
        let mut now = Time::ZERO;
        let mut k = 0;
        while now < end {
            let g = (now + grant).min(end);
            let mut chunk = Vec::new();
            while k < trace.len() && trace[k].t <= g {
                chunk.push(trace[k].clone());
                k += 1;
            }
            e.apply_input_trace(&PinSignals::new(), &chunk).unwrap();
            e.advance_to(g).unwrap();
            out.extend(e.read_output_trace().unwrap().unwrap());
            now = g;
        }
        let q = e.quiet_until(now).as_fs();
        (out, q)
    }

    #[test]
    fn homing_asserts_and_is_partition_independent() {
        let trace = homing_trace();
        let end = ms(1100);
        let (a, qa) = run_engine(&trace, Time::from_us(10), end, 1);
        let (b, qb) = run_engine(&trace, ms(1), end, 1);
        let (c, qc) = run_engine(&trace, Time::from_us(370), end, 1);
        assert_eq!(a, b);
        assert_eq!(a, c);
        assert_eq!((qa, qb), (qc, qc));
        // 60 full steps towards the park stop from 330: s = 270 crosses the
        // assert edge (about s = 285 at 2.6 mm): HOME_N falls once.
        let falls: Vec<&Transition> = a
            .iter()
            .filter(|x| x.signal.level() == Some(Level::Low))
            .collect();
        assert_eq!(falls.len(), 1, "{a:?}");
        // The same seed gives the same sample phase; another seed may not.
        let (d, _) = run_engine(&trace, ms(1), end, 1);
        assert_eq!(a, d);
        let (e2, _) = run_engine(&trace, ms(1), end, 99);
        assert_eq!(e2.len(), 1);
    }

    #[test]
    fn stepping_at_446_pps_crosses_the_edge_with_ts_lag() {
        // The edge latency: from the carriage passing BOP to HOME_N low is
        // at most ts + tactive (ADR-008 Amendment 10).
        let trace = homing_trace();
        let (out, _) = run_engine(&trace, ms(1), ms(1100), 1);
        let fall = out
            .iter()
            .find(|x| x.signal.level() == Some(Level::Low))
            .unwrap()
            .t;
        // Find when the carriage passed BOP: replay with a rig.
        let mut r = Rig::new(mech(&[("s0", real(330.0))]));
        let names = ["M1.IN_A1", "M1.IN_A2", "M1.IN_B1", "M1.IN_B2", "M1.NSLEEP"];
        let mut cross = None;
        for x in &trace {
            let pin = names.iter().position(|n| *n == x.pin).unwrap();
            r.set(x.t, pin, x.signal.level().unwrap());
            if cross.is_none() && r.m.field().abs() > 3.0 {
                cross = Some(x.t);
            }
        }
        let cross = cross.unwrap();
        let lat = fall - cross;
        assert!(
            lat <= ms(50) + Time::from_us(40) + Time::from_us(40),
            "{lat}"
        );
    }
}
