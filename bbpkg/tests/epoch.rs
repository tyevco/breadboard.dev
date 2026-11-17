//! The Phase 2 schema epoch (milestone S): the `bb-part/1` and `bb-pins/1`
//! keys of ADR-009, ADR-010 and ADR-012 are accepted, checked and inert;
//! they are refused in a `/0` file; and the view-name rule of ADR-011 D11
//! warns, or with `--publish` fails.

use std::path::{Path, PathBuf};

use bbpkg::lint::{lint, name_prefix, Context, Level};
use bbpkg::package::{self, parse_bound, parse_part, parse_pins, Bound, Port};
use bbpkg::schema::Registry;

const PART: &str = r#"schema = "bb-part/1"
[part]
id = "test/mcu-x"
version = "0.1.0"
kind = "soc"
license = "Apache-2.0"
[pins]
file = "pins.toml"
[claims]
file = "claims.toml"
[power]
vdd_min = "1.7V"
io_rail = "VDD"
[power.idd.run]
typ = "3.3mA"
max = "4mA"
ref = "PS v1.1 Table 5.2.1.1, ICPU0"
[power.idd.sleep]
typ = "3.16uA"
ref = "PS v1.1, ION_RAMON_RTC"
[[document]]
id = "ps-v1.1"
title = "Product Specification"
revision = "1.1"
url = "https://example.invalid/ps.pdf"
sha256 = "0000000000000000000000000000000000000000000000000000000000000000"
fetched_by = "user:someone"
[[analog]]
name = "idd"
kind = "supply_load"
fidelity = ["pi"]
file = "analog/idd.lib"
subckt = "TEST_MCU_X_IDD"
ports = [{ role = "pin", pin = "VDD" }, { role = "pin", pin = "VSS" }, { role = "state", state = "run" }, { role = "state", state = "sleep" }]
rails = ["VDD"]
params = { i_run = "3.3m" }
[[analog.corners]]
name = "worst_hot"
params = { i_run = "4m" }
ref = "PS v1.1, ICPU0 max"
[analog.provenance]
source = "datasheet"
confidence = "unchecked"
upstream_license = "BSD-3-Clause"
"#;

const PINS: &str = r#"schema = "bb-pins/1"
[[pin]]
number = "1"
name = "VDD"
role = "power_in"
[[pin]]
number = "2"
name = "VSS"
role = "ground"
[[pin]]
number = "3"
name = "P0"
role = "io"
supply = "VDD"
models = ["digital_out"]
[[limits]]
pin = "P0"
max = "VDD + 0.3V"
min = "-0.3V"
source = "PS v1.1, absolute maximum ratings"
[defaults.digital_out.default]
voh = "VDD"
vol = "0V"
rise = "5ns"
fall = "5ns"
zout = "25"
rail = "VDD"
rhiz = "100Meg"
"#;

const IDD: &str = "* idd\n.subckt TEST_MCU_X_IDD VDD VSS run sleep params: i_run=3.3m\n\
                   B1 VDD VSS I = i_run*V(run)\n.ends\n";

struct Tmp(PathBuf);
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write_pkg(name: &str, files: &[(&str, &str)]) -> Tmp {
    let d = std::env::temp_dir().join(format!("bbpkg-epoch-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    for (rel, text) in files {
        let p = d.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    Tmp(d)
}

fn findings(dir: &Path, publish: bool) -> Vec<(Level, String)> {
    let reg = Registry::current();
    let pkg = match package::load(dir, &reg) {
        Ok(p) => p,
        Err(e) => return vec![(Level::Error, e)],
    };
    let ctx = Context {
        platforms: &[],
        devices: &[],
        first_party: false,
        chip: None,
        publish,
        check_behavioral: true,
    };
    lint(&pkg, &ctx, &reg)
        .into_iter()
        .map(|f| (f.level, f.msg))
        .collect()
}

fn errors(dir: &Path) -> Vec<String> {
    findings(dir, false)
        .into_iter()
        .filter(|(l, _)| *l == Level::Error)
        .map(|(_, m)| m)
        .collect()
}

fn files<'a>(part: &'a str, pins: &'a str) -> Vec<(&'a str, &'a str)> {
    vec![
        ("part.toml", part),
        ("pins.toml", pins),
        ("analog/idd.lib", IDD),
        ("claims.toml", "schema = \"bb-claims/0\"\n"),
    ]
}

#[test]
fn every_epoch_key_is_accepted_and_lints_clean() {
    let reg = Registry::current();
    let (p, note) = parse_part(PART, &reg).unwrap();
    assert!(note.is_none());
    let pw = p.power.as_ref().unwrap();
    assert_eq!(pw.io_rail.as_deref(), Some("VDD"));
    assert_eq!(pw.idd.len(), 2);
    assert_eq!(pw.idd[0].state, "run");
    assert_eq!(p.documents[0].id, "ps-v1.1");
    assert_eq!(p.claims_file.as_deref(), Some("claims.toml"));
    let v = &p.analog[0];
    assert_eq!(v.kind, "supply_load");
    assert_eq!(v.rails, vec!["VDD"]);
    assert_eq!(v.corners[0].name, "worst_hot");
    assert!(v.ports.contains(&Port::State {
        state: "run".into()
    }));
    assert_eq!(
        v.provenance.as_ref().unwrap().upstream_license.as_deref(),
        Some("BSD-3-Clause")
    );
    let (pins, note) = parse_pins(PINS, &reg).unwrap();
    assert!(note.is_none());
    assert_eq!(pins.limits.len(), 1);
    let set = &pins.defaults["digital_out"]["default"];
    assert!(set.iter().any(|(k, v)| k == "rail" && v == "VDD"));
    let t = write_pkg("clean", &files(PART, PINS));
    let f = clean(findings(&t.0, true));
    assert!(f.is_empty(), "{f:?}");
}

/// Findings other than the missing test vectors (the fixture has none).
fn clean(f: Vec<(Level, String)>) -> Vec<(Level, String)> {
    f.into_iter()
        .filter(|(_, m)| !m.contains("no [tests] dir"))
        .collect()
}

#[test]
fn limit_bounds() {
    assert_eq!(
        parse_bound("VDD + 0.7V"),
        Ok(Bound {
            pin: Some("VDD".into()),
            offset_uv: 700_000
        })
    );
    assert_eq!(
        parse_bound("VSS - 0.3V"),
        Ok(Bound {
            pin: Some("VSS".into()),
            offset_uv: -300_000
        })
    );
    assert_eq!(
        parse_bound("3.6V"),
        Ok(Bound {
            pin: None,
            offset_uv: 3_600_000
        })
    );
    assert_eq!(parse_bound("VDD").unwrap().offset_uv, 0);
    // (`VDD+0.7V` reads as one pin name, which lint then does not find.)
    assert_eq!(parse_bound("VDD + 0.7").unwrap().offset_uv, 700_000); // the unit is optional
    for bad in [
        "VDD + 0.7A",
        "VDD * 2",
        "VDD + VSS",
        "2 * VDD",
        "VDD + 0.7V + 1V",
    ] {
        assert!(parse_bound(bad).is_err(), "{bad}");
    }
}

#[test]
fn epoch_keys_are_refused_in_a_previous_major_file() {
    let reg = Registry::current();
    let part0 = PART.replacen("bb-part/1", "bb-part/0", 1);
    let e = parse_part(&part0, &reg).unwrap_err();
    assert!(e.contains("new in bb-part/1"), "{e}");
    let pins0 = PINS.replacen("bb-pins/1", "bb-pins/0", 1);
    let e = parse_pins(&pins0, &reg).unwrap_err();
    assert!(e.contains("new in bb-pins/1"), "{e}");
    let pins0_nolimits = pins0.replace(
        "[[limits]]\npin = \"P0\"\nmax = \"VDD + 0.3V\"\nmin = \"-0.3V\"\nsource = \"PS v1.1, absolute maximum ratings\"\n",
        "",
    );
    let e = parse_pins(&pins0_nolimits, &reg).unwrap_err();
    assert!(e.contains("rail: new in bb-pins/1"), "{e}");
}

#[test]
fn reserved_keys_stay_reserved() {
    let reg = Registry::current();
    for (from, to, what) in [
        (
            "[claims]",
            "[thermal]\nx = 1\n[claims]",
            "thermal: reserved",
        ),
        (
            "file = \"pins.toml\"",
            "file = \"pins.toml\"\nibis = \"x.ibs\"",
            "ibis: reserved",
        ),
    ] {
        let e = parse_part(&PART.replacen(from, to, 1), &reg).unwrap_err();
        assert!(e.contains(what), "{e}");
    }
}

/// ADR-009 D3 (Amendment 11): the `power{vmin, vmax, imax}` pin model is
/// defined in bb-pins/1: a supply pin's set (named after the pin, else
/// `default`), never on another role, never in a bb-pins/0 file.
#[test]
fn the_power_pin_model() {
    let reg = Registry::current();
    let powered = PINS.replacen(
        "name = \"VDD\"\nrole = \"power_in\"\n",
        "name = \"VDD\"\nrole = \"power_in\"\nmodels = [\"power\"]\n",
        1,
    )
        + "[defaults.power.default]\nvmin = \"1.7V\"\nvmax = \"3.6V\"\nimax = \"25mA\"\n";
    // Clean, and the pin's own set wins over `default`.
    let named = powered.clone() + "[defaults.power.VDD]\nvmax = \"3.9V\"\n";
    for (n, text) in [("pwr-default", &powered), ("pwr-named", &named)] {
        let t = write_pkg(n, &files(PART, text));
        assert!(clean(findings(&t.0, false)).is_empty(), "{n}");
        let pkg = package::load(&t.0, &reg).unwrap();
        let (set, p) = package::power_params(&pkg, "VDD").unwrap().unwrap();
        let want_set = if n == "pwr-named" { "VDD" } else { "default" };
        assert_eq!(set, want_set);
        let vmax = if n == "pwr-named" {
            3_900_000
        } else {
            3_600_000
        };
        assert_eq!(p.get("vmax"), Some(&package::Param::Quantity(vmax)));
        assert!(package::power_params(&pkg, "P0").unwrap().is_none());
    }
    // A bb-pins/0 file has no power model.
    let e = parse_pins(
        &powered
            .replacen("bb-pins/1", "bb-pins/0", 1)
            .replace("rail = \"VDD\"\nrhiz = \"100Meg\"\n", "")
            .replace(
                "[[limits]]\npin = \"P0\"\nmax = \"VDD + 0.3V\"\nmin = \"-0.3V\"\nsource = \"PS v1.1, absolute maximum ratings\"\n",
                "",
            ),
        &reg,
    )
    .unwrap_err();
    assert!(e.contains("power pin model is new in bb-pins/1"), "{e}");
    for (n, text, want) in [
        (
            "pwr-io",
            PINS.replacen(
                "models = [\"digital_out\"]",
                "models = [\"digital_out\", \"power\"]",
                1,
            ) + "[defaults.power.default]\nvmax = \"3.6V\"\n",
            "role io cannot take model power",
        ),
        (
            "pwr-empty",
            powered.replace(
                "[defaults.power.default]\nvmin = \"1.7V\"\nvmax = \"3.6V\"\nimax = \"25mA\"\n",
                "[defaults.power.default]\n",
            ),
            "defaults.power.default: give vmin, vmax or imax",
        ),
        (
            "pwr-set",
            powered.clone() + "[defaults.power.P0]\nvmax = \"3.9V\"\n",
            "defaults.power.P0: not `default` and not a pin that takes the power model",
        ),
        (
            "pwr-noset",
            powered.replace(
                "[defaults.power.default]\nvmin = \"1.7V\"\nvmax = \"3.6V\"\nimax = \"25mA\"\n",
                "",
            ),
            "no parameter set defaults.power.default",
        ),
        (
            "pwr-unit",
            powered.replace("imax = \"25mA\"", "imax = \"25mV\""),
            "defaults.power.default.imax",
        ),
        (
            "limit-twice",
            PINS.to_string() + "[[limits]]\npin = \"P0\"\nmax = \"3.9V\"\nsource = \"again\"\n",
            "limits: pin P0 has two max limits",
        ),
    ] {
        let t = write_pkg(n, &files(PART, &text));
        let e = errors(&t.0);
        assert!(e.iter().any(|m| m.contains(want)), "{n}: {e:?}");
    }
}

#[test]
fn each_epoch_rule_fires() {
    let cases: &[(&str, &str, &str, &str)] = &[
        // (file, from, to, expected error)
        (
            "part",
            "fidelity = [\"pi\"]",
            "fidelity = [\"fast\", \"pi\"]",
            "not valid at fast",
        ),
        (
            "part",
            "fidelity = [\"pi\"]",
            "fidelity = [\"pi\"]\ndefault_for = [\"pi\"]",
            "no rung's default",
        ),
        (
            "part",
            "state = \"sleep\"",
            "state = \"boot\"",
            "#boot has no [power] idd.boot",
        ),
        (
            "part",
            "rails = [\"VDD\"]",
            "rails = [\"VSS\"]",
            "rails: VSS",
        ),
        (
            "part",
            "params = { i_run = \"4m\" }",
            "params = { i_boot = \"4m\" }",
            "i_boot is not one of the view's params",
        ),
        (
            "part",
            "io_rail = \"VDD\"",
            "io_rail = \"VSS\"",
            "power.io_rail VSS",
        ),
        (
            "part",
            "file = \"claims.toml\"",
            "file = \"missing.toml\"",
            "claims.file missing.toml",
        ),
        (
            "part",
            "upstream_license = \"BSD-3-Clause\"",
            "upstream_license = \"BSD-3-Clause; MIT\"",
            "upstream_license",
        ),
        ("part", "max = \"4mA\"", "max = \"3mA\"", "max is below typ"),
        (
            "pins",
            "pin = \"P0\"\nmax",
            "pin = \"P9\"\nmax",
            "limits: pin P9",
        ),
        (
            "pins",
            "max = \"VDD + 0.3V\"",
            "max = \"VBAT + 0.3V\"",
            "VBAT is not a pin",
        ),
        (
            "pins",
            "rail = \"VDD\"",
            "rail = \"P0\"",
            "rail P0: not a power_in",
        ),
    ];
    for (i, (file, from, to, want)) in cases.iter().enumerate() {
        let (part, pins) = match *file {
            "part" => (PART.replacen(from, to, 1), PINS.to_string()),
            _ => (PART.to_string(), PINS.replacen(from, to, 1)),
        };
        assert!(part != PART || pins != PINS, "case {i}: {from} not found");
        let t = write_pkg(&format!("rule{i}"), &files(&part, &pins));
        let e = errors(&t.0);
        assert!(
            e.iter().any(|m| m.contains(want)),
            "case {i} ({want}): {e:?}"
        );
    }
    // Load-time refusals: shapes and names.
    let reg = Registry::current();
    for (from, to, want) in [
        ("name = \"worst_hot\"", "name = \"hot\"", "a corner is typ"),
        (
            "sha256 = \"0000000000000000000000000000000000000000000000000000000000000000\"",
            "sha256 = \"abc\"",
            "64 lower-case hex",
        ),
        (
            "fetched_by = \"user:someone\"",
            "fetched_by = \"someone\"",
            "agent:<who> or user:<who>",
        ),
        (
            "{ role = \"state\", state = \"run\" }",
            "{ role = \"state\", state = \"#run\" }",
            "a state name",
        ),
        (
            "{ role = \"state\", state = \"run\" }",
            "{ role = \"pad\" }",
            "role \"pin\" or \"state\"",
        ),
        ("typ = \"3.3mA\"", "typ = \"3.3mV\"", "power.idd.run.typ"),
    ] {
        let p = PART.replacen(from, to, 1);
        assert!(p != PART, "{from} not found");
        let e = parse_part(&p, &reg).unwrap_err();
        assert!(e.contains(want), "{want}: {e}");
    }
    // State ports only on a supply_load view.
    let p = PART.replacen("kind = \"supply_load\"", "kind = \"pin_load\"", 1);
    assert!(parse_part(&p, &reg).is_err());
}

#[test]
fn the_view_name_rule_warns_and_fails_for_publication() {
    // ADR-011 D11: <NS>_<NAME>_ prefixes and no top-level .param.
    assert_eq!(name_prefix("ti/tps22917"), "TI_TPS22917_");
    assert_eq!(
        name_prefix("worldsemi/ws2812c-2020"),
        "WORLDSEMI_WS2812C_2020_"
    );
    assert_eq!(name_prefix("a.b/c-d"), "A_B_C_D_");
    let bad_lib = ".param gleg = 1\n.model plain_d D(IS=1e-14)\n.subckt TEST_MCU_X_IDD VDD VSS run sleep params: i_run=3.3m\n\
                   B1 VDD VSS I = i_run*V(run)\n.ends\n";
    let mut fs = files(PART, PINS);
    fs[2] = ("analog/idd.lib", bad_lib);
    let t = write_pkg("names", &fs);
    let warn = clean(findings(&t.0, false));
    assert!(warn.iter().all(|(l, _)| *l == Level::Warning), "{warn:?}");
    assert!(
        warn.iter()
            .any(|(_, m)| m.contains("top-level .param gleg")),
        "{warn:?}"
    );
    assert!(
        warn.iter()
            .any(|(_, m)| m.contains(".model plain_d does not start with TEST_MCU_X_")),
        "{warn:?}"
    );
    let err = findings(&t.0, true);
    assert_eq!(
        err.iter().filter(|(l, _)| *l == Level::Error).count(),
        2,
        "{err:?}"
    );
    // The prefix compares without case (SPICE is case-insensitive).
    let lower = IDD.replace("TEST_MCU_X_IDD", "test_mcu_x_idd");
    let mut fs = files(PART, PINS);
    fs[2] = ("analog/idd.lib", &lower);
    let t = write_pkg("names-lower", &fs);
    assert!(clean(findings(&t.0, true)).is_empty());
}
