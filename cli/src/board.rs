//! Board files for `bb run`: a SPICE circuit body plus `*bb` directives.
//!
//! Format (see `examples/spike/board.cir`): every line that starts with `*bb`
//! is a directive, `*bb <keyword> <name> key=value ...`, split on whitespace.
//! Every other line is SPICE and goes to the analog engine unchanged (the
//! directives are SPICE comments, so ngspice ignores them too).
//!
//! Directives:
//! - `*bb part <ref> engine=renode platform=stm32f103`
//! - `*bb pin <ref>.<pin> net=<net> model=digital_out voh=<V> vol=<V> rise=<t> fall=<t> zout=<ohm>`
//!
//! The parser is a tokenizer over a line-oriented grammar; no regex.

use std::collections::BTreeMap;

use bbcore::{DigitalOut, Time};

/// A part simulated by a CPU engine.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub reference: String,
    pub engine: String,
    pub platform: String,
}

/// An MCU pin bound to a circuit net through a digital-out pin model.
#[derive(Clone, Debug, PartialEq)]
pub struct PinBinding {
    /// Part reference, e.g. `U1`.
    pub part: String,
    /// Pin name on the part, e.g. `PA5`.
    pub pin: String,
    /// Circuit net the generated glue drives, e.g. `PA5`.
    pub net: String,
    pub model: DigitalOut,
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
                let net = take(lineno, &mut kv, "net")?.to_string();
                if pins.iter().any(|p| p.net.eq_ignore_ascii_case(&net)) {
                    return Err(format!(
                        "line {lineno}: net {net} already driven by another pin"
                    ));
                }
                let kind = take(lineno, &mut kv, "model")?;
                if kind != "digital_out" {
                    return Err(format!(
                        "line {lineno}: model={kind} not supported (Phase 0: digital_out)"
                    ));
                }
                let model = DigitalOut {
                    voh: volts(lineno, "voh", take(lineno, &mut kv, "voh")?)?,
                    vol: volts(lineno, "vol", take(lineno, &mut kv, "vol")?)?,
                    rise: time(lineno, "rise", take(lineno, &mut kv, "rise")?)?,
                    fall: time(lineno, "fall", take(lineno, &mut kv, "fall")?)?,
                    zout: volts(lineno, "zout", take(lineno, &mut kv, "zout")?)?,
                };
                no_extra(lineno, &kv)?;
                if model.zout < 0.0 {
                    return Err(format!("line {lineno}: zout must be >= 0"));
                }
                pins.push(PinBinding {
                    part: part.to_string(),
                    pin: pin.to_string(),
                    net,
                    model,
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
        assert_eq!(
            (p.part.as_str(), p.pin.as_str(), p.net.as_str()),
            ("U1", "PA5", "PA5")
        );
        assert_eq!(p.model, DigitalOut::STM32_GPIO_3V3);
        assert_eq!(p.kernel_net(), "U1.PA5");
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
