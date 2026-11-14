//! Device parts in `bb run` (ADR-006 D7, Amendment 6; `spec/package.md`
//! section 12): the direct digital routes and the run's engine graph.
//!
//! - A device `digital_out` drives its kernel net `<ref>.<pin>`; every
//!   `digital_in` on that circuit net (the CPU's or another device's) reads
//!   the kernel net itself, with no threshold.
//! - A device `digital_in` on a net that the CPU's or another device's
//!   `digital_out` drives reads that kernel net the same way.
//! - Every other binding is unchanged: the analog engine gets an output as
//!   glue and thresholds a node for an input.
//!
//! The CPU reading its own output's net keeps the Phase 0 route (through
//! the analog engine), so no board without devices changes.

use std::collections::BTreeMap;

use bbcore::Direction;

use crate::board::{Board, PinKind};

/// Engine indices of a run's binding list.
pub const CPU: usize = 0;
pub const ANALOG: usize = 1;
pub const DEVICES: usize = 2;

/// One kernel binding: `engine.pin` on `net`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bind {
    pub engine: usize,
    pub pin: String,
    pub net: String,
    pub dir: Direction,
}

/// The directly routed digital inputs: the reader's kernel net (`U1.P1.14`,
/// `M1.IN_A1`) to the kernel net of the output that drives its circuit net.
/// Only where a device is the reader or the driver.
pub fn direct_inputs(board: &Board) -> BTreeMap<String, String> {
    let is_dev = |part: &str| {
        board
            .parts
            .iter()
            .any(|p| p.reference == part && p.is_device())
    };
    let mut out = BTreeMap::new();
    for r in &board.pins {
        let PinKind::In { net, .. } = &r.kind else {
            continue;
        };
        let driver = board.pins.iter().find(|d| {
            d.part != r.part
                && (is_dev(&d.part) || is_dev(&r.part))
                && matches!(&d.kind, PinKind::Out { net: n, .. } if n.eq_ignore_ascii_case(net))
        });
        if let Some(d) = driver {
            out.insert(r.kernel_net(), d.kernel_net());
        }
    }
    out
}

/// Whether the binding graph over `n` engines has a cycle (as the kernel's
/// plan defines it: a strongly connected component of more than one engine,
/// or an engine reading a net it drives), and which engines are in one.
pub fn cycles(n: usize, binds: &[Bind]) -> (bool, Vec<bool>) {
    let mut edge = vec![vec![false; n]; n];
    for o in binds.iter().filter(|b| b.dir == Direction::Output) {
        for i in binds
            .iter()
            .filter(|b| b.dir == Direction::Input && b.net == o.net)
        {
            edge[o.engine][i.engine] = true;
        }
    }
    // reach[a][b]: a path of at least one edge from a to b.
    let mut reach = edge.clone();
    for k in 0..n {
        let via = reach[k].clone();
        for row in reach.iter_mut() {
            if row[k] {
                for (b, &v) in via.iter().enumerate() {
                    if v {
                        row[b] = true;
                    }
                }
            }
        }
    }
    let in_cycle: Vec<bool> = (0..n).map(|a| reach[a][a]).collect();
    (in_cycle.iter().any(|&c| c), in_cycle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(engine: usize, net: &str, dir: Direction) -> Bind {
        Bind {
            engine,
            pin: net.into(),
            net: net.into(),
            dir,
        }
    }

    #[test]
    fn cycle_classes() {
        use Direction::{Input as I, Output as O};
        // CPU -> analog only: acyclic.
        assert_eq!(
            cycles(3, &[b(CPU, "a", O), b(ANALOG, "a", I)]),
            (false, vec![false, false, false])
        );
        // CPU <-> device, analog consumes: the CPU and the device loop.
        let (c, m) = cycles(
            3,
            &[
                b(CPU, "a", O),
                b(DEVICES, "a", I),
                b(ANALOG, "a", I),
                b(DEVICES, "h", O),
                b(CPU, "h", I),
            ],
        );
        assert!(c);
        assert_eq!(m, vec![true, false, true]);
        // A device reading another device's output: a self loop.
        assert_eq!(
            cycles(3, &[b(DEVICES, "x", O), b(DEVICES, "x", I)]).1,
            vec![false, false, true]
        );
    }
}

#[cfg(test)]
mod board_tests {
    use super::*;
    use crate::board::{parse_with, Board};
    use bbpkg::schema::Registry;

    fn parse(text: &str) -> Result<Board, String> {
        let sp = bbpkg::SearchPath {
            dirs: vec![bbpkg::first_party_dir()],
        };
        let reg = Registry::current();
        let lookup = |spec: &str| sp.find(spec, &reg);
        parse_with(text, Some(&lookup))
    }

    const BOARD: &str = "\
*bb part U1 package=st/stm32f103c8
*bb part M1 package=dms/detent-mechanism ts=75ms s0=250 die=2.8
*bb pin U1.PA5 net=MOT_A_IN1 model=digital_out
*bb pin U1.PA6 net=MOT_NSLEEP model=digital_out
*bb pin M1.IN_A1 net=MOT_A_IN1 model=digital_in
*bb pin M1.NSLEEP net=mot_nsleep model=digital_in
*bb pin M1.IN_B1 net=SOME_NODE model=digital_in
*bb pin M1.HOME_N net=HOME_N model=digital_out
*bb pin U1.PB0 net=HOME_N model=digital_in
*bb pin U1.PB1 net=SENSE model=digital_in
";

    #[test]
    fn device_parts_parse_and_route() {
        let b = parse(BOARD).unwrap();
        let m1 = b.parts.iter().find(|p| p.reference == "M1").unwrap();
        assert!(m1.is_device());
        assert_eq!(m1.platform, "dms/mechanism@1");
        assert_eq!(m1.package.as_deref(), Some("dms/detent-mechanism"));
        assert_eq!(
            m1.params,
            vec![
                ("die".to_string(), bbdev::ParamValue::Real(2.8)),
                ("s0".to_string(), bbdev::ParamValue::Real(250.0)),
                (
                    "ts".to_string(),
                    bbdev::ParamValue::Time(bbcore::Time::from_ms(75))
                ),
            ]
        );
        assert!(b
            .packages
            .iter()
            .any(|p| p.part.id == "dms/detent-mechanism"));
        let d = direct_inputs(&b);
        // The device reads the CPU's outputs (net names compare without
        // case), the CPU reads HOME_N; IN_B1 on an analog net and the CPU's
        // own SENSE input go through the analog engine.
        let want: BTreeMap<String, String> = [
            ("M1.IN_A1", "U1.PA5"),
            ("M1.NSLEEP", "U1.PA6"),
            ("U1.PB0", "M1.HOME_N"),
        ]
        .into_iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
        assert_eq!(d, want);
    }

    #[test]
    fn device_parts_refuse_what_they_cannot_honour() {
        let head = "*bb part U1 package=st/stm32f103c8\n";
        let err = |s: &str| parse(&format!("{head}{s}\n")).unwrap_err();
        let m1 = "*bb part M1 package=dms/detent-mechanism";
        assert!(err(&format!("{m1} nope=1")).contains("unknown parameter"));
        assert!(err(&format!("{m1} ts=3V")).contains("ts=3V"));
        assert!(err(&format!("{m1} s0=900")).contains("s0"));
        assert!(err(&format!("{m1} engine=device")).contains("engine"));
        assert!(
            err(&format!("{m1}\n*bb pin M1.HOME_N net=X model=digital_in"))
                .contains("an output of device")
        );
        assert!(
            err(&format!("{m1}\n*bb pin M1.IN_A1 net=X model=digital_out"))
                .contains("an input of device")
        );
        assert!(err(&format!("{m1}\n*bb pin M1.IN_A1 net=X model=adc_in")).contains("adc_in"));
        assert!(err(&format!("{m1}\n*bb pin M1.FOO net=X model=digital_in")).contains("FOO"));
        // One driver per net: a device output on a CPU output's net.
        assert!(err(&format!(
            "{m1}\n*bb pin U1.PA5 net=X model=digital_out\n*bb pin M1.HOME_N net=X model=digital_out"
        ))
        .contains("already driven"));
    }
}
