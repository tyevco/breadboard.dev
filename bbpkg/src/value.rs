//! The value grammar (ADR-006 D5, `spec/package.md` "Values") and the
//! integer units (ADR-007 D6).
//!
//! Every numeric string in `part.toml`, `pins.toml`, test vectors and the
//! bench is parsed here, exactly, into an integer in a fixed sub-unit of its
//! dimension; nothing is ever passed through to SPICE as text. Two grammars:
//!
//! - **Package grammar** ([`parse_quantity`], [`parse_number`]): a decimal
//!   number with an optional exponent, an optional SI prefix
//!   (`f p n u µ m k Meg G T`; `m` is milli, `Meg` is mega), and an optional
//!   unit checked against the dimension. A bare `M` is an error that
//!   suggests `Meg` or `m`: ngspice reads it as milli, EE text as mega.
//! - **KiCad value fields** ([`parse_kicad_value`]): EE convention, as the
//!   dial-me-spinner suite's `spicelib.parse_value` reads them: `M` (and
//!   `Meg`, `MEG`) is mega, `K` is kilo, RKM infix (`4k7`, `2R2`, `51R`), an
//!   optional unit letter, an optional `/<rating>V`, and the qualifiers of
//!   ADR-006 Amendment 1 (tolerance, voltage and power ratings, dielectric,
//!   technology). Anything else is an error, never a guess.
//!
//! The result is exact: a value that is not a whole number of sub-units
//! (e.g. `0.5fs`) is an error, never rounded.

use std::fmt;

/// A physical dimension and its integer sub-unit (ADR-007 D6).
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Dim {
    /// µV
    Voltage,
    /// fs
    Time,
    /// µΩ
    Resistance,
    /// aF
    Capacitance,
    /// fH
    Inductance,
    /// pA
    Current,
    /// m°C
    Temperature,
    /// A plain count (bits): exact integer, unit 1.
    Count,
    /// A ratio given in percent, in ppm (1 % = 10 000 ppm).
    Percent,
    /// µW (power ratings only).
    Power,
}

impl Dim {
    /// Decimal exponent of the sub-unit: value in base units × 10^this.
    pub fn subunit_exp(self) -> i32 {
        match self {
            Dim::Voltage | Dim::Resistance | Dim::Power => 6,
            Dim::Time | Dim::Inductance => 15,
            Dim::Capacitance => 18,
            Dim::Current => 12,
            Dim::Temperature => 3,
            Dim::Count => 0,
            Dim::Percent => 4,
        }
    }

    /// The units accepted for this dimension (case-sensitive).
    pub fn units(self) -> &'static [&'static str] {
        match self {
            Dim::Voltage => &["V"],
            Dim::Time => &["s"],
            Dim::Resistance => &["Ω", "ohm", "Ohm"],
            Dim::Capacitance => &["F"],
            Dim::Inductance => &["H"],
            Dim::Current => &["A"],
            Dim::Temperature => &["°C", "degC"],
            Dim::Count => &[],
            Dim::Percent => &["%"],
            Dim::Power => &["W"],
        }
    }

    /// Name of the sub-unit, for messages and `board.bbm` keys.
    pub fn subunit(self) -> &'static str {
        match self {
            Dim::Voltage => "uV",
            Dim::Time => "fs",
            Dim::Resistance => "uohm",
            Dim::Capacitance => "aF",
            Dim::Inductance => "fH",
            Dim::Current => "pA",
            Dim::Temperature => "mdegC",
            Dim::Count => "1",
            Dim::Percent => "ppm",
            Dim::Power => "uW",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Dim::Voltage => "voltage",
            Dim::Time => "time",
            Dim::Resistance => "resistance",
            Dim::Capacitance => "capacitance",
            Dim::Inductance => "inductance",
            Dim::Current => "current",
            Dim::Temperature => "temperature",
            Dim::Count => "count",
            Dim::Percent => "percentage",
            Dim::Power => "power",
        }
    }
}

/// An exact decimal: `mant × 10^exp`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Decimal {
    pub mant: i128,
    pub exp: i32,
}

impl Decimal {
    /// The value scaled by `10^shift`, if it is a whole number that fits.
    pub fn to_int(self, shift: i32) -> Option<i128> {
        let e = self.exp + shift;
        if e >= 0 {
            let p = 10i128.checked_pow(u32::try_from(e).ok()?)?;
            self.mant.checked_mul(p)
        } else {
            let p = 10i128.checked_pow(u32::try_from(-e).ok()?)?;
            (self.mant % p == 0).then(|| self.mant / p)
        }
    }

    /// Normalised: no trailing zeros in the mantissa (zero is `0e0`).
    pub fn normalized(mut self) -> Decimal {
        if self.mant == 0 {
            return Decimal { mant: 0, exp: 0 };
        }
        while self.mant % 10 == 0 {
            self.mant /= 10;
            self.exp += 1;
        }
        self
    }

    /// Product of two decimals.
    pub fn checked_mul(self, o: Decimal) -> Option<Decimal> {
        Some(Decimal {
            mant: self.mant.checked_mul(o.mant)?,
            exp: self.exp.checked_add(o.exp)?,
        })
    }

    /// The nearest `f64` (correctly rounded when `mant` and `10^|exp|` are
    /// exact in `f64`, which covers every value the engines take).
    pub fn to_f64(self) -> f64 {
        let n = self.normalized();
        // Parse the decimal text: Rust's parser rounds correctly.
        format!("{}e{}", n.mant, n.exp)
            .parse()
            .expect("a decimal literal")
    }
}

/// A SPICE number for an exact decimal, never with a letter suffix
/// (ADR-006 D5): `2.2e6`, `3.3`, `5e-12`, `-1.5e-3`, `0`. ngspice reads
/// each exactly as the decimal it denotes.
pub fn spice_number(d: Decimal) -> String {
    let d = d.normalized();
    if d.mant == 0 {
        return "0".into();
    }
    let neg = d.mant < 0;
    let digits = d.mant.unsigned_abs().to_string();
    let n = digits.len() as i32;
    // Value = 0.digits × 10^(exp + n) = d.ddd × 10^(exp + n - 1).
    let sci_exp = d.exp + n - 1;
    let body = if (0..=6).contains(&sci_exp) && d.exp >= 0 {
        // A plain integer.
        format!("{digits}{}", "0".repeat(d.exp as usize))
    } else if (-3..=6).contains(&sci_exp) {
        // A plain decimal with a point.
        let point = sci_exp + 1; // digits before the point
        if point <= 0 {
            format!("0.{}{digits}", "0".repeat((-point) as usize))
        } else {
            let (a, b) = digits.split_at(point as usize);
            format!("{a}.{b}")
        }
    } else {
        let (a, b) = digits.split_at(1);
        if b.is_empty() {
            format!("{a}e{sci_exp}")
        } else {
            format!("{a}.{b}e{sci_exp}")
        }
    };
    if neg {
        format!("-{body}")
    } else {
        body
    }
}

/// A parse error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValueError(pub String);

impl fmt::Display for ValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ValueError {}

fn err<T>(msg: impl Into<String>) -> Result<T, ValueError> {
    Err(ValueError(msg.into()))
}

/// Split a leading decimal literal (`[+-]digits[.digits][(e|E)[+-]digits]`
/// or `.digits`) from `s`: `(decimal, rest)`.
fn split_decimal(s: &str) -> Option<(Decimal, &str)> {
    let b = s.as_bytes();
    let mut i = 0;
    let neg = match b.first() {
        Some(b'-') => {
            i += 1;
            true
        }
        Some(b'+') => {
            i += 1;
            false
        }
        _ => false,
    };
    let int_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let int_digits = &s[int_start..i];
    let mut frac_digits = "";
    if i < b.len() && b[i] == b'.' {
        let fs = i + 1;
        let mut j = fs;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > fs {
            frac_digits = &s[fs..j];
            i = j;
        }
    }
    if int_digits.is_empty() && frac_digits.is_empty() {
        return None;
    }
    let mut exp: i32 = 0;
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        // An exponent only if digits follow (else `e` is not part of it).
        let mut j = i + 1;
        let eneg = match b.get(j) {
            Some(b'-') => {
                j += 1;
                true
            }
            Some(b'+') => {
                j += 1;
                false
            }
            _ => false,
        };
        let es = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > es {
            let e: i32 = s[es..j].parse().ok()?;
            exp = if eneg { -e } else { e };
            i = j;
        }
    }
    let digits = format!("{int_digits}{frac_digits}");
    let mant: i128 = digits.parse().ok()?;
    let mant = if neg { -mant } else { mant };
    let exp = exp.checked_sub(i32::try_from(frac_digits.len()).ok()?)?;
    Some((Decimal { mant, exp }, &s[i..]))
}

/// SI prefixes of the package grammar, longest first.
const PREFIXES: &[(&str, i32)] = &[
    ("Meg", 6),
    ("f", -15),
    ("p", -12),
    ("n", -9),
    ("u", -6),
    ("µ", -6),
    ("μ", -6),
    ("m", -3),
    ("k", 3),
    ("G", 9),
    ("T", 12),
];

/// Every unit any dimension accepts (for dimensionless view parameters).
const ALL_UNITS: &[&str] = &[
    "V", "s", "Ω", "ohm", "Ohm", "F", "H", "A", "°C", "degC", "%", "Hz", "W",
];

/// Parse `<number>[prefix][unit]` (package grammar) into an exact decimal in
/// base units; `units` lists the accepted unit spellings (empty: none; the
/// percent sign scales by 1/100 only through [`parse_quantity`]).
fn parse_si(text: &str, units: &[&str]) -> Result<(Decimal, Option<String>), ValueError> {
    let t = text.trim();
    let (dec, rest) = split_decimal(t).ok_or_else(|| {
        ValueError(format!(
            "{text:?}: expected a number (e.g. 3.3V, 10us, 4.7k)"
        ))
    })?;
    let mut rest = rest;
    let mut pexp = 0;
    if rest.starts_with('M') && !rest.starts_with("Meg") {
        return err(format!(
            "{text:?}: a bare M is ambiguous (ngspice reads it as milli, EE text as mega): \
             write Meg for mega or m for milli"
        ));
    }
    for (p, e) in PREFIXES {
        if let Some(r) = rest.strip_prefix(p) {
            // A prefix only if what follows is empty or a unit: `m` alone is
            // milli, `ms` milliseconds; but `F` is farad, not femto.
            if r.is_empty() || units.contains(&r) {
                rest = r;
                pexp = *e;
                break;
            }
        }
    }
    let unit = if rest.is_empty() {
        None
    } else if units.contains(&rest) {
        Some(rest.to_string())
    } else if ALL_UNITS.contains(&rest) {
        return err(format!(
            "{text:?}: unit {rest:?} does not fit here (expected {})",
            if units.is_empty() {
                "no unit".to_string()
            } else {
                units.join(", ")
            }
        ));
    } else {
        return err(format!("{text:?}: unknown suffix {rest:?}"));
    };
    Ok((
        Decimal {
            mant: dec.mant,
            exp: dec.exp + pexp,
        },
        unit,
    ))
}

/// Parse a quantity of dimension `dim` (package grammar) into integer
/// sub-units: `parse_quantity("3.3V", Voltage) = 3_300_000` µV,
/// `parse_quantity("5ns", Time) = 5_000_000` fs, `parse_quantity("1%",
/// Percent) = 10_000` ppm. The unit is optional; another dimension's unit
/// is an error. A value that is not a whole number of sub-units is an error.
pub fn parse_quantity(text: &str, dim: Dim) -> Result<i128, ValueError> {
    // A percentage is stored in ppm whether or not the % is written: "1%"
    // and "1" both mean one percent.
    let (d, _unit) = parse_si(text, dim.units())?;
    d.to_int(dim.subunit_exp()).ok_or_else(|| {
        ValueError(format!(
            "{text:?}: not a whole number of {} (the {} sub-unit) or too large",
            dim.subunit(),
            dim.name()
        ))
    })
}

/// Parse a dimensionless view parameter (`params` of an analog view): a
/// number with an optional prefix and any unit (units are not checked:
/// a view does not declare its parameters' dimensions). Returns the exact
/// decimal in base units, for [`spice_number`].
pub fn parse_number(text: &str) -> Result<Decimal, ValueError> {
    let (d, unit) = parse_si(text, ALL_UNITS)?;
    if unit.as_deref() == Some("%") {
        return Ok(Decimal {
            mant: d.mant,
            exp: d.exp - 2,
        });
    }
    Ok(d)
}

/// Integer sub-units back to an exact decimal in base units.
pub fn to_decimal(value: i128, dim: Dim) -> Decimal {
    Decimal {
        mant: value,
        exp: -dim.subunit_exp(),
    }
}

/// A sub-unit integer as the `f64` the engines take (V, s, Ω, F): exact
/// numerator over an exact power of ten, so the result is the correctly
/// rounded value of the decimal, the same `f64` that parsing the decimal
/// text gives (`"3.3".parse()`, `"5e-12".parse()`).
pub fn to_f64(value: i128, dim: Dim) -> f64 {
    to_decimal(value, dim).to_f64()
}

// ---------------------------------------------------------------------------
// KiCad value fields (ADR-006 D5, Amendment 1)
// ---------------------------------------------------------------------------

/// Which generic passive a KiCad value belongs to.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PassiveKind {
    Resistor,
    Capacitor,
    Inductor,
}

impl PassiveKind {
    pub fn dim(self) -> Dim {
        match self {
            PassiveKind::Resistor => Dim::Resistance,
            PassiveKind::Capacitor => Dim::Capacitance,
            PassiveKind::Inductor => Dim::Inductance,
        }
    }
    /// From a reference designator prefix (`R`, `C`, `L`).
    pub fn from_ref(r: &str) -> Option<PassiveKind> {
        match r.chars().next()? {
            'R' => Some(PassiveKind::Resistor),
            'C' => Some(PassiveKind::Capacitor),
            'L' => Some(PassiveKind::Inductor),
            _ => None,
        }
    }
}

/// A parsed KiCad value field.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct KicadValue {
    /// The value in the kind's sub-unit (µΩ, aF, fH).
    pub value: i128,
    /// Tolerance in ppm (`0.5%` = 5 000), when given.
    pub tolerance_ppm: Option<i128>,
    /// Voltage rating in µV (`/25V`), when given.
    pub rating_uv: Option<i128>,
    /// Power rating in µW (`1/4W` = 250 000), when given.
    pub power_uw: Option<i128>,
    /// Dielectric code (`C0G`, `X7R`, ...), when given.
    pub dielectric: Option<String>,
    /// Technology (`Ta` tantalum, `Al` aluminium electrolytic), when given.
    pub technology: Option<String>,
}

/// Dielectric codes accepted as a qualifier (EIA class 1 and 2).
pub const DIELECTRICS: &[&str] = &[
    "C0G", "NP0", "NPO", "X5R", "X6S", "X7R", "X7S", "X7T", "X8R", "Y5V", "Z5U",
];

/// Technology qualifiers.
pub const TECHNOLOGIES: &[&str] = &["Ta", "Al"];

/// Parse the KiCad value field of a generic passive, as EE text: `M` is
/// mega. Returns the exact value and its qualifiers. Errors name the text.
pub fn parse_kicad_value(text: &str, kind: PassiveKind) -> Result<KicadValue, ValueError> {
    let mut toks = text.split_whitespace();
    let first = toks
        .next()
        .ok_or_else(|| ValueError(format!("{text:?}: empty value")))?;
    let mut out = KicadValue::default();
    // First token: <value>[/<rating>V]
    let (val, rating) = match first.split_once('/') {
        Some((v, r)) => (v, Some(r)),
        None => (first, None),
    };
    out.value = parse_kicad_number(val, kind).map_err(|e| ValueError(format!("{text:?}: {e}")))?;
    if let Some(r) = rating {
        out.rating_uv = Some(rating_volts(r).map_err(|e| ValueError(format!("{text:?}: {e}")))?);
    }
    for tok in toks {
        let bad = |what: &str| ValueError(format!("{text:?}: {what}"));
        if let Some(p) = tok.strip_suffix('%') {
            if out.tolerance_ppm.is_some() {
                return Err(bad("two tolerances"));
            }
            let (d, rest) = split_decimal(p).ok_or_else(|| bad("bad tolerance"))?;
            if !rest.is_empty() || d.mant < 0 {
                return Err(bad("bad tolerance"));
            }
            out.tolerance_ppm = Some(
                d.to_int(4)
                    .ok_or_else(|| bad("tolerance finer than 1 ppm"))?,
            );
        } else if let Some(w) = tok.strip_suffix('W') {
            if out.power_uw.is_some() {
                return Err(bad("two power ratings"));
            }
            out.power_uw = Some(power_watts(w).map_err(|e| bad(&e))?);
        } else if tok.ends_with('V') && tok.starts_with(|c: char| c.is_ascii_digit()) {
            if out.rating_uv.is_some() {
                return Err(bad("two voltage ratings"));
            }
            out.rating_uv =
                Some(rating_volts(tok).map_err(|e| ValueError(format!("{text:?}: {e}")))?);
        } else if DIELECTRICS.contains(&tok) {
            if kind != PassiveKind::Capacitor || out.dielectric.is_some() {
                return Err(bad(&format!("dielectric {tok} here")));
            }
            out.dielectric = Some(tok.to_string());
        } else if TECHNOLOGIES.contains(&tok) {
            if kind != PassiveKind::Capacitor || out.technology.is_some() {
                return Err(bad(&format!("technology {tok} here")));
            }
            out.technology = Some(tok.to_string());
        } else {
            return Err(bad(&format!(
                "unknown qualifier {tok:?} (expected <x>%, <v>V, <p>W, a dielectric code or Ta/Al)"
            )));
        }
    }
    Ok(out)
}

/// `<number>` with an EE prefix or RKM infix, and an optional unit letter.
fn parse_kicad_number(tok: &str, kind: PassiveKind) -> Result<i128, String> {
    // EE prefixes; `M`, `Meg`, `MEG` mega; `K` kilo; `R`/`r` the unit point.
    const EE: &[(&str, i32)] = &[
        ("Meg", 6),
        ("MEG", 6),
        ("f", -15),
        ("p", -12),
        ("n", -9),
        ("u", -6),
        ("µ", -6),
        ("μ", -6),
        ("m", -3),
        ("k", 3),
        ("K", 3),
        ("M", 6),
        ("G", 9),
        ("R", 0),
        ("r", 0),
    ];
    let unit_ok = |u: &str| -> bool {
        match kind {
            PassiveKind::Resistor => matches!(u, "" | "Ω" | "ohm" | "Ohm" | "OHM"),
            PassiveKind::Capacitor => matches!(u, "" | "F"),
            PassiveKind::Inductor => matches!(u, "" | "H"),
        }
    };
    let (int, rest) = split_decimal(tok).ok_or_else(|| format!("{tok:?} is not a value"))?;
    if int.mant < 0 {
        return Err(format!("{tok:?}: negative value"));
    }
    let mut dec = int;
    let mut rest = rest;
    let mut pexp = 0;
    for (p, e) in EE {
        if let Some(r) = rest.strip_prefix(p) {
            // RKM: digits after the prefix are the fraction (`4k7`), only
            // when the leading number had no point or exponent.
            let nd = r.bytes().take_while(u8::is_ascii_digit).count();
            if nd > 0 {
                if int.exp != 0 {
                    return Err(format!("{tok:?}: RKM infix after a decimal point"));
                }
                let frac = &r[..nd];
                let m: i128 = format!("{}{frac}", int.mant)
                    .parse()
                    .map_err(|_| format!("{tok:?}: too many digits"))?;
                dec = Decimal {
                    mant: m,
                    exp: -(nd as i32),
                };
                rest = &r[nd..];
            } else {
                rest = r;
            }
            pexp = *e;
            break;
        }
    }
    if !unit_ok(rest) {
        return Err(format!("{tok:?}: unknown suffix {rest:?}"));
    }
    let d = Decimal {
        mant: dec.mant,
        exp: dec.exp + pexp,
    };
    d.to_int(kind.dim().subunit_exp())
        .ok_or_else(|| format!("{tok:?}: not a whole number of {}", kind.dim().subunit()))
}

fn rating_volts(tok: &str) -> Result<i128, String> {
    let v = tok
        .strip_suffix('V')
        .ok_or_else(|| format!("rating {tok:?}: expected <n>V"))?;
    let (d, rest) = split_decimal(v).ok_or_else(|| format!("rating {tok:?}: bad number"))?;
    let p = match rest {
        "" => 0,
        "k" => 3,
        "m" => -3,
        _ => return Err(format!("rating {tok:?}: bad number")),
    };
    Decimal {
        mant: d.mant,
        exp: d.exp + p,
    }
    .to_int(6)
    .ok_or_else(|| format!("rating {tok:?} finer than 1 uV"))
}

/// `1/4` (a fraction), `0.1`, `100m` (watts, without the `W`), in µW.
fn power_watts(w: &str) -> Result<i128, String> {
    if let Some((a, b)) = w.split_once('/') {
        let a: i128 = a
            .parse()
            .map_err(|_| format!("power {w:?}W: bad fraction"))?;
        let b: i128 = b
            .parse()
            .map_err(|_| format!("power {w:?}W: bad fraction"))?;
        if b == 0 || (a * 1_000_000) % b != 0 {
            return Err(format!("power {w:?}W: not a whole number of uW"));
        }
        return Ok(a * 1_000_000 / b);
    }
    let (d, rest) = split_decimal(w).ok_or_else(|| format!("power {w:?}W: bad number"))?;
    let p = match rest {
        "" => 0,
        "m" => -3,
        "k" => 3,
        _ => return Err(format!("power {w:?}W: bad number")),
    };
    Decimal {
        mant: d.mant,
        exp: d.exp + p,
    }
    .to_int(6)
    .ok_or_else(|| format!("power {w:?}W: finer than 1 uW"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_grammar() {
        assert_eq!(parse_quantity("3.3V", Dim::Voltage), Ok(3_300_000));
        assert_eq!(parse_quantity("3.3", Dim::Voltage), Ok(3_300_000));
        assert_eq!(parse_quantity("0V", Dim::Voltage), Ok(0));
        assert_eq!(parse_quantity("-10", Dim::Temperature), Ok(-10_000));
        assert_eq!(parse_quantity("44.2mV", Dim::Voltage), Ok(44_200));
        assert_eq!(parse_quantity("5ns", Dim::Time), Ok(5_000_000));
        assert_eq!(parse_quantity("10us", Dim::Time), Ok(10_000_000_000));
        assert_eq!(parse_quantity("10µs", Dim::Time), Ok(10_000_000_000));
        assert_eq!(parse_quantity("1e-5s", Dim::Time), Ok(10_000_000_000));
        assert_eq!(parse_quantity("120ms", Dim::Time), Ok(120_000_000_000_000));
        assert_eq!(parse_quantity("5pF", Dim::Capacitance), Ok(5_000_000));
        assert_eq!(parse_quantity("5e-12", Dim::Capacitance), Ok(5_000_000));
        assert_eq!(parse_quantity("2.5p", Dim::Capacitance), Ok(2_500_000));
        assert_eq!(parse_quantity("25", Dim::Resistance), Ok(25_000_000));
        assert_eq!(parse_quantity("25Ω", Dim::Resistance), Ok(25_000_000));
        assert_eq!(
            parse_quantity("2.2Meg", Dim::Resistance),
            Ok(2_200_000_000_000)
        );
        assert_eq!(
            parse_quantity("4.7kohm", Dim::Resistance),
            Ok(4_700_000_000)
        );
        assert_eq!(parse_quantity("50mA", Dim::Current), Ok(50_000_000_000));
        assert_eq!(
            parse_quantity("1H", Dim::Inductance),
            Ok(1_000_000_000_000_000)
        );
        assert_eq!(parse_quantity("12", Dim::Count), Ok(12));
        assert_eq!(parse_quantity("0.5%", Dim::Percent), Ok(5_000));
        assert_eq!(
            parse_quantity("1F", Dim::Capacitance),
            Ok(1_000_000_000_000_000_000)
        );
        // m is milli, F is farad (not femto), and a bare M is refused.
        assert_eq!(parse_quantity("1m", Dim::Resistance), Ok(1_000));
        let e = parse_quantity("1M", Dim::Resistance).unwrap_err().0;
        assert!(e.contains("Meg"), "{e}");
        assert!(parse_quantity("1MV", Dim::Voltage).is_err());
        // Units are checked against the dimension.
        assert!(parse_quantity("3.3s", Dim::Voltage).is_err());
        assert!(parse_quantity("3.3x", Dim::Voltage).is_err());
        assert!(parse_quantity("", Dim::Voltage).is_err());
        assert!(parse_quantity("V", Dim::Voltage).is_err());
        // Exact or nothing.
        assert!(parse_quantity("0.5fs", Dim::Time).is_err());
        assert!(parse_quantity("1.5", Dim::Count).is_err());
    }

    #[test]
    fn f64_matches_decimal_text() {
        for (s, dim, txt) in [
            ("3.3V", Dim::Voltage, "3.3"),
            ("5pF", Dim::Capacitance, "5e-12"),
            ("25", Dim::Resistance, "25"),
            ("0.8", Dim::Voltage, "0.8"),
            ("2.0", Dim::Voltage, "2.0"),
            ("4.7u", Dim::Capacitance, "4.7e-6"),
        ] {
            let v = parse_quantity(s, dim).unwrap();
            assert_eq!(to_f64(v, dim), txt.parse::<f64>().unwrap(), "{s}");
        }
    }

    #[test]
    fn spice_numbers_have_no_letters() {
        let d = |m: i128, e: i32| Decimal { mant: m, exp: e };
        assert_eq!(spice_number(d(22, 5)), "2200000");
        assert_eq!(spice_number(d(22, 6)), "2.2e7");
        assert_eq!(spice_number(d(33, -1)), "3.3");
        assert_eq!(spice_number(d(5, -12)), "5e-12");
        assert_eq!(spice_number(d(47, -7)), "4.7e-6");
        assert_eq!(spice_number(d(-15, -4)), "-0.0015");
        assert_eq!(spice_number(d(0, 9)), "0");
        assert_eq!(spice_number(d(1000, 0)), "1000");
        assert_eq!(spice_number(d(12345, -2)), "123.45");
        for x in [
            "2.2e7", "3.3", "5e-12", "4.7e-6", "-0.0015", "123.45", "1000",
        ] {
            assert!(!x.chars().any(|c| c.is_ascii_alphabetic() && c != 'e'));
        }
        // Round trip through the grammar.
        let v = parse_number("2.2Meg").unwrap();
        assert_eq!(spice_number(v), "2200000");
        assert_eq!(spice_number(parse_number("1m").unwrap()), "0.001");
        assert_eq!(spice_number(parse_number("100n").unwrap()), "1e-7");
        assert_eq!(spice_number(parse_number("50%").unwrap()), "0.5");
    }

    #[test]
    fn kicad_values() {
        use PassiveKind::*;
        let v = |s: &str, k| parse_kicad_value(s, k).unwrap();
        assert_eq!(v("4k7", Resistor).value, 4_700_000_000);
        assert_eq!(v("4.7k", Resistor).value, 4_700_000_000);
        assert_eq!(v("2R2", Resistor).value, 2_200_000);
        assert_eq!(v("51R 1/4W", Resistor).value, 51_000_000);
        assert_eq!(v("51R 1/4W", Resistor).power_uw, Some(250_000));
        assert_eq!(v("1M", Resistor).value, 1_000_000_000_000);
        assert_eq!(v("2.2M", Resistor).value, 2_200_000_000_000);
        assert_eq!(v("1Meg", Resistor).value, 1_000_000_000_000);
        assert_eq!(v("0", Resistor).value, 0);
        let t = v("499k 0.5%", Resistor);
        assert_eq!((t.value, t.tolerance_ppm), (499_000_000_000, Some(5_000)));
        let c = v("10u/25V", Capacitor);
        assert_eq!(
            (c.value, c.rating_uv),
            (10_000_000_000_000, Some(25_000_000))
        );
        let c = v("4.7u/25V Ta", Capacitor);
        assert_eq!(c.technology.as_deref(), Some("Ta"));
        let c = v("12p C0G", Capacitor);
        assert_eq!(
            (c.value, c.dielectric.as_deref()),
            (12_000_000, Some("C0G"))
        );
        assert_eq!(v("10u", Inductor).value, 10_000_000_000);
        assert_eq!(v("4.7uF", Capacitor).value, 4_700_000_000_000);
        // Errors, never guesses.
        for bad in [
            "",
            "abc",
            "4k7k",
            "10u/25",
            "100n X9Q",
            "1k 5%  5%",
            "12p C0G",
            "4.7k7",
        ] {
            let kind = if bad.starts_with("12p") {
                Resistor
            } else {
                Capacitor
            };
            assert!(parse_kicad_value(bad, kind).is_err(), "{bad:?}");
        }
    }
}
