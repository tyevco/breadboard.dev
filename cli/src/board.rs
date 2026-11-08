//! Board files for `bb run`: a SPICE circuit body plus `*bb` directives.
//!
//! Format (see `examples/spike/board.cir`): every line that starts with `*bb`
//! is a directive, `*bb <keyword> <name> key=value ...`, split on whitespace.
//! Every other line is SPICE and goes to the analog engine unchanged (the
//! directives are SPICE comments, so ngspice ignores them too).
//!
//! Directives:
//! - `*bb part <ref> engine=renode platform=stm32f103`
//! - `*bb pin <ref>.<pin> net=<net> model=digital_out voh=<V> vol=<V> rise=<t> fall=<t> zout=<ohm>`:
//!   an MCU output driving circuit node `<net>` through generated glue.
//! - `*bb pin <ref>.<pin> model=digital_out` (no `net=`, no electrical keys):
//!   an MCU output not connected to the circuit. Its kernel net
//!   `<ref>.<pin>` can still be probed; nothing loads it.
//! - `*bb pin <ref>.<pin> net=<net> model=digital_in vih=<V> vil=<V> cin=<F>`:
//!   an MCU input reading circuit node `<net>` through the `digital_in` model
//!   (hysteresis, `vil < vih`; `cin` in farads, e.g. `5e-12`). The analog
//!   engine thresholds the node and adds `cin` as glue (ADR-003 D3).
//!
//! The parser is a tokenizer over a line-oriented grammar; no regex.

use std::collections::BTreeMap;

use bbcore::{DigitalIn, DigitalOut, Time};

/// A part simulated by a CPU engine.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub reference: String,
    pub engine: String,
    pub platform: String,
}

/// How an MCU pin meets the circuit.
#[derive(Clone, Debug, PartialEq)]
pub enum PinKind {
    /// `digital_out` driving circuit node `net` through generated glue.
    Out { net: String, model: DigitalOut },
    /// `digital_out` not connected to the circuit (probe only).
    OutUnconnected,
    /// `digital_in` reading circuit node `net`.
    In { net: String, model: DigitalIn },
}

/// An MCU pin bound through its pin model.
#[derive(Clone, Debug, PartialEq)]
pub struct PinBinding {
    /// Part reference, e.g. `U1`.
    pub part: String,
    /// Pin name on the part, e.g. `PA5`.
    pub pin: String,
    pub kind: PinKind,
}

impl PinBinding {
    /// Kernel net carrying the pin's digital signal: `<part>.<pin>`.
    pub fn kernel_net(&self) -> String {
        format!("{}.{}", self.part, self.pin)
    }
}

/// A parsed board file.
#[derive(Clone, Debug, PartialEq)]
pub struct Board {
    pub parts: Vec<Part>,
    pub pins: Vec<PinBinding>,
    /// The SPICE circuit body (directive lines included, as comments).
    pub spice: String,
}

fn parse_kv<'a>(
    lineno: usize,
    tokens: impl Iterator<Item = &'a str>,
) -> Result<BTreeMap<&'a str, &'a str>, String> {
    let mut kv = BTreeMap::new();
    for tok in tokens {
        let (k, v) = tok
            .split_once('=')
            .ok_or_else(|| format!("line {lineno}: expected key=value, got {tok:?}"))?;
        if k.is_empty() || v.is_empty() {
            return Err(format!("line {lineno}: empty key or value in {tok:?}"));
        }
        if kv.insert(k, v).is_some() {
            return Err(format!("line {lineno}: duplicate key {k:?}"));
        }
    }
    Ok(kv)
}

fn take<'a>(
    lineno: usize,
    kv: &mut BTreeMap<&'a str, &'a str>,
    key: &str,
) -> Result<&'a str, String> {
    kv.remove(key)
        .ok_or_else(|| format!("line {lineno}: missing {key}="))
}

fn volts(lineno: usize, key: &str, v: &str) -> Result<f64, String> {
    let x: f64 = v
        .parse()
        .map_err(|_| format!("line {lineno}: {key}={v:?} is not a number"))?;
    if !x.is_finite() {
        return Err(format!("line {lineno}: {key}={v:?} is not finite"));
    }
    Ok(x)
}

fn time(lineno: usize, key: &str, v: &str) -> Result<Time, String> {
    v.parse().map_err(|e| format!("line {lineno}: {key}: {e}"))
}

fn no_extra(lineno: usize, kv: &BTreeMap<&str, &str>) -> Result<(), String> {
    match kv.keys().next() {
        Some(k) => Err(format!("line {lineno}: unknown key {k:?}")),
        None => Ok(()),
    }
}

/// Parse a board file.
pub fn parse(text: &str) -> Result<Board, String> {
    let mut parts: Vec<Part> = Vec::new();
    let mut pins: Vec<PinBinding> = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let lineno = i + 1;
        let Some(rest) = line.trim_start().strip_prefix("*bb") else {
            continue;
        };
        if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
            continue; // e.g. "*bbox": an ordinary comment
        }
        let mut toks = rest.split_whitespace();
        let keyword = toks
            .next()
            .ok_or_else(|| format!("line {lineno}: empty *bb directive"))?;
        let name = toks
            .next()
            .ok_or_else(|| format!("line {lineno}: *bb {keyword} needs a name"))?;
        let mut kv = parse_kv(lineno, toks)?;
        match keyword {
            "part" => {
                if parts.iter().any(|p| p.reference == name) {
                    return Err(format!("line {lineno}: part {name} declared twice"));
                }
                let engine = take(lineno, &mut kv, "engine")?.to_string();
                let platform = take(lineno, &mut kv, "platform")?.to_string();
                no_extra(lineno, &kv)?;
                if engine != "renode" {
                    return Err(format!(
                        "line {lineno}: engine={engine} not supported (Phase 0: renode)"
                    ));
                }
                if platform != "stm32f103" {
                    return Err(format!(
                        "line {lineno}: platform={platform} not supported (Phase 0: stm32f103)"
                    ));
                }
                parts.push(Part {
                    reference: name.to_string(),
                    engine,
                    platform,
                });
            }
            "pin" => {
                let (part, pin) = name.split_once('.').ok_or_else(|| {
                    format!("line {lineno}: pin name must be <part>.<pin>, got {name:?}")
                })?;
                if !parts.iter().any(|p| p.reference == part) {
                    return Err(format!(
                        "line {lineno}: pin {name}: part {part} not declared (declare it first)"
                    ));
                }
                if pins.iter().any(|p| p.part == part && p.pin == pin) {
                    return Err(format!("line {lineno}: pin {name} bound twice"));
                }
                let net = kv.remove("net").map(str::to_string);
                let model_kind = take(lineno, &mut kv, "model")?;
                let kind = match (model_kind, net) {
                    ("digital_out", Some(net)) => {
                        if pins.iter().any(|p| {
                            matches!(&p.kind, PinKind::Out { net: n, .. } if n.eq_ignore_ascii_case(&net))
                        }) {
                            return Err(format!(
                                "line {lineno}: net {net} already driven by another pin"
                            ));
                        }
                        let model = DigitalOut {
                            voh: volts(lineno, "voh", take(lineno, &mut kv, "voh")?)?,
                            vol: volts(lineno, "vol", take(lineno, &mut kv, "vol")?)?,
                            rise: time(lineno, "rise", take(lineno, &mut kv, "rise")?)?,
                            fall: time(lineno, "fall", take(lineno, &mut kv, "fall")?)?,
                            zout: volts(lineno, "zout", take(lineno, &mut kv, "zout")?)?,
                        };
                        if model.zout < 0.0 {
                            return Err(format!("line {lineno}: zout must be >= 0"));
                        }
                        PinKind::Out { net, model }
                    }
                    ("digital_out", None) => {
                        if let Some(k) = kv.keys().next() {
                            return Err(format!(
                                "line {lineno}: {k}=: a digital_out without net= is not \
                                 connected to the circuit and takes no electrical keys"
                            ));
                        }
                        PinKind::OutUnconnected
                    }
                    ("digital_in", Some(net)) => {
                        let model = DigitalIn {
                            vih: volts(lineno, "vih", take(lineno, &mut kv, "vih")?)?,
                            vil: volts(lineno, "vil", take(lineno, &mut kv, "vil")?)?,
                            cin: volts(lineno, "cin", take(lineno, &mut kv, "cin")?)?,
                        };
                        if model.vil >= model.vih {
                            return Err(format!("line {lineno}: digital_in needs vil < vih"));
                        }
                        if model.cin < 0.0 {
                            return Err(format!("line {lineno}: cin must be >= 0"));
                        }
                        PinKind::In { net, model }
                    }
                    ("digital_in", None) => {
                        return Err(format!("line {lineno}: digital_in needs net="))
                    }
                    (other, _) => {
                        return Err(format!(
                            "line {lineno}: model={other} not supported (digital_out, digital_in)"
                        ))
                    }
                };
                no_extra(lineno, &kv)?;
                pins.push(PinBinding {
                    part: part.to_string(),
                    pin: pin.to_string(),
                    kind,
                });
            }
            other => return Err(format!("line {lineno}: unknown directive *bb {other}")),
        }
    }
    Ok(Board {
        parts,
        pins,
        spice: text.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_spike_board() {
        let b = parse(include_str!("../../examples/spike/board.cir")).unwrap();
        assert_eq!(b.parts.len(), 1);
        assert_eq!(b.parts[0].reference, "U1");
        assert_eq!(b.pins.len(), 1);
        let p = &b.pins[0];
        assert_eq!((p.part.as_str(), p.pin.as_str()), ("U1", "PA5"));
        assert_eq!(
            p.kind,
            PinKind::Out {
                net: "PA5".into(),
                model: DigitalOut::STM32_GPIO_3V3
            }
        );
        assert_eq!(p.kernel_net(), "U1.PA5");
    }

    #[test]
    fn parses_feedback_boards() {
        let b = parse(include_str!("../../examples/feedback/board.cir")).unwrap();
        let kinds: Vec<(&str, &PinKind)> =
            b.pins.iter().map(|p| (p.pin.as_str(), &p.kind)).collect();
        assert_eq!(kinds.len(), 3);
        assert_eq!(
            kinds[1],
            (
                "PB0",
                &PinKind::In {
                    net: "SENSE".into(),
                    model: DigitalIn {
                        vih: 2.0,
                        vil: 0.8,
                        cin: 5e-12
                    }
                }
            )
        );
        assert!(matches!(kinds[2], ("PA6", PinKind::Out { net, .. }) if net == "PA6"));
        let open = parse(include_str!("../../examples/feedback-open/board.cir")).unwrap();
        let kinds: Vec<(&str, &PinKind)> = open
            .pins
            .iter()
            .map(|p| (p.pin.as_str(), &p.kind))
            .collect();
        assert!(matches!(kinds[0], ("PB0", PinKind::In { .. })));
        assert_eq!(kinds[1], ("PA6", &PinKind::OutUnconnected));
    }

    #[test]
    fn digital_in_and_unconnected_pins() {
        let part = "*bb part U1 engine=renode platform=stm32f103\n";
        let ok = |s: &str| parse(&format!("{part}{s}\n"));
        assert!(ok("*bb pin U1.PB0 net=S model=digital_in vih=2 vil=0.8 cin=5e-12").is_ok());
        assert!(ok("*bb pin U1.PB0 model=digital_in vih=2 vil=0.8 cin=0").is_err());
        assert!(ok("*bb pin U1.PB0 net=S model=digital_in vih=2 vil=2 cin=0").is_err());
        assert!(ok("*bb pin U1.PB0 net=S model=digital_in vih=2 vil=1 cin=-1").is_err());
        assert!(ok("*bb pin U1.PB0 net=S model=digital_in vih=2 vil=1").is_err());
        assert!(ok("*bb pin U1.PA6 model=digital_out").is_ok());
        assert!(ok("*bb pin U1.PA6 model=digital_out voh=3.3").is_err());
        assert!(ok("*bb pin U1.PA6 net=S model=analog").is_err());
        // Two inputs may read one node; an input may read a driven node.
        let out = "*bb pin U1.PA5 net=S model=digital_out voh=3.3 vol=0 rise=5ns fall=5ns zout=25";
        let b = ok(&format!(
            "{out}\n*bb pin U1.PB0 net=S model=digital_in vih=2 vil=1 cin=0\n\
             *bb pin U1.PB1 net=S model=digital_in vih=2 vil=1 cin=0"
        ))
        .unwrap();
        assert_eq!(b.pins.len(), 3);
        assert!(ok(&format!("{out}\n{}", out.replace("PA5", "PA6"))).is_err());
    }

    #[test]
    fn rejects_bad_directives() {
        let part = "*bb part U1 engine=renode platform=stm32f103\n";
        assert!(parse("*bb pin U1.PA5 net=X model=digital_out\n").is_err());
        assert!(parse(&format!(
            "{part}*bb pin U1.PA5 net=X model=digital_out voh=3.3\n"
        ))
        .is_err());
        assert!(parse(&format!("{part}{part}")).is_err());
        assert!(parse("*bb part U1 engine=qemu platform=stm32f103\n").is_err());
        assert!(parse("*bb wire A B\n").is_err());
        assert!(parse(&format!("{part}*bb pin U1PA5 net=X\n")).is_err());
        // Not directives.
        assert_eq!(
            parse("*bbox\n* bb part\nR1 a 0 1k\n").unwrap().parts.len(),
            0
        );
    }
}
