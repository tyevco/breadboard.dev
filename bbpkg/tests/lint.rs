//! `bb pkg lint` on synthetic packages (ADR-006 D2-D4, D9; gates G1, G7):
//! a clean package lints clean, and each rule fires on its own violation.

use std::path::{Path, PathBuf};

use bbpkg::lint::{lint, Context, Level};
use bbpkg::package;
use bbpkg::schema::Registry;

const PART: &str = r#"schema = "bb-part/0"
[part]
id = "test/part"
version = "0.1.0"
kind = "analog_ic"
license = "Apache-2.0"
[pins]
file = "pins.toml"
[[analog]]
name = "a"
fidelity = ["fast", "pi"]
default_for = ["fast"]
file = "analog/a.lib"
subckt = "PART_A"
ports = ["IN", "OUT", "GNDP"]
[analog.provenance]
source = "datasheet"
confidence = "datasheet_checked"
[[analog]]
name = "b"
fidelity = ["fast"]
file = "analog/b.lib"
subckt = "PART_B"
ports = ["IN", "OUT", "GNDP"]
[analog.provenance]
source = "estimated"
confidence = "unchecked"
"#;

const PINS: &str = r#"schema = "bb-pins/0"
[[pin]]
number = "1"
name = "IN"
role = "input"
models = ["digital_in"]
[[pin]]
number = "2"
name = "OUT"
role = "output"
models = ["digital_out"]
[[pin]]
number = "3"
name = "GNDP"
role = "ground"
[[pin]]
number = "4"
name = "VDD"
role = "power_in"
[defaults.digital_out.default]
voh = "VDD"
vol = "0V"
rise = "5ns"
fall = "5ns"
zout = "25"
[defaults.digital_in.default]
vih = "0.7*VDD"
vil = "0.3*VDD"
cin = "3p"
"#;

const A: &str =
    ".subckt PART_A IN OUT GNDP\nR1 IN OUT 1k\nC1 OUT GNDP 1p\n.ends\n.model DX D(IS=1e-14)\n";
const B: &str = ".subckt PART_B IN OUT GNDP\nR1 IN OUT 2k\n.ends\n.model DX D(IS=1e-14)\n";

struct Tmp(PathBuf);
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write_pkg(name: &str, files: &[(&str, &str)]) -> Tmp {
    let d = std::env::temp_dir().join(format!("bbpkg-lint-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    for (rel, text) in files {
        let p = d.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    Tmp(d)
}

fn errors(dir: &Path, first_party: bool) -> Vec<String> {
    let reg = Registry::current();
    let pkg = match package::load(dir, &reg) {
        Ok(p) => p,
        Err(e) => return vec![e],
    };
    let platforms = vec!["stm32f103-offline@1".to_string()];
    let ctx = Context {
        platforms: &platforms,
        devices: &[],
        first_party,
        chip: None,
    };
    lint(&pkg, &ctx, &reg)
        .into_iter()
        .filter(|f| f.level == Level::Error)
        .map(|f| f.msg)
        .collect()
}

type Files = Vec<(&'static str, String)>;

fn base() -> Files {
    vec![
        ("part.toml", PART.to_string()),
        ("pins.toml", PINS.to_string()),
        ("analog/a.lib", A.to_string()),
        ("analog/b.lib", B.to_string()),
    ]
}

fn with(name: &str, edit: impl Fn(&mut Files)) -> Vec<String> {
    let mut files = base();
    edit(&mut files);
    let refs: Vec<(&str, &str)> = files.iter().map(|(a, b)| (*a, b.as_str())).collect();
    let t = write_pkg(name, &refs);
    errors(&t.0, true)
}

fn set(files: &mut Files, file: &str, from: &str, to: &str) {
    let f = files.iter_mut().find(|(n, _)| *n == file).unwrap();
    assert!(f.1.contains(from), "{from}");
    f.1 = f.1.replacen(from, to, 1);
}

fn has(e: Vec<String>, needle: &str) {
    assert!(
        e.iter().any(|m| m.contains(needle)),
        "expected {needle:?} in {e:?}"
    );
}

#[test]
fn clean_package_lints_clean() {
    let e = with("clean", |_| {});
    assert!(e.is_empty(), "{e:?}");
}

#[test]
fn value_licence_and_gnd_rules() {
    // A bare M in a package value (D5).
    has(
        with("bare-m", |f| {
            set(f, "pins.toml", "zout = \"25\"", "zout = \"1M\"")
        }),
        "Meg",
    );
    // A local-only licence inside first-party packages/ (D9).
    has(
        with("local-only", |f| {
            set(f, "part.toml", "Apache-2.0", "LicenseRef-local-only")
        }),
        "LicenseRef-local-only",
    );
    // The gnd rule (D4, Context 4): a port, and an internal node.
    has(
        with("gnd-port", |f| {
            set(f, "analog/a.lib", "PART_A IN OUT GNDP", "PART_A IN OUT GND")
        }),
        "gnd",
    );
    has(
        with("gnd-node", |f| {
            set(f, "analog/b.lib", "R1 IN OUT 2k", "R1 IN gnd 2k")
        }),
        "gnd",
    );
}

#[test]
fn view_rules() {
    // default_for must be a subset of fidelity; one default per rung (D3).
    has(
        with("default-for", |f| {
            set(
                f,
                "part.toml",
                "default_for = [\"fast\"]",
                "default_for = [\"si\"]",
            )
        }),
        "subset",
    );
    has(
        with("two-defaults", |f| {
            set(
                f,
                "part.toml",
                "fidelity = [\"fast\"]\nfile",
                "fidelity = [\"fast\"]\ndefault_for = [\"fast\"]\nfile",
            )
        }),
        "already the default",
    );
    // G7: two views define one top-level name differently.
    has(
        with("conflict", |f| {
            set(f, "analog/b.lib", "DX D(IS=1e-14)", "DX D(IS=2e-14)")
        }),
        "defined differently",
    );
    // A port that is not a pin; a port count that does not match.
    has(
        with("port-pin", |f| {
            set(
                f,
                "part.toml",
                "ports = [\"IN\", \"OUT\", \"GNDP\"]",
                "ports = [\"IN\", \"OUTX\", \"GNDP\"]",
            )
        }),
        "not a pin",
    );
    has(
        with("port-count", |f| {
            set(
                f,
                "analog/a.lib",
                "PART_A IN OUT GNDP",
                "PART_A IN OUT GNDP EXTRA",
            )
        }),
        "ports",
    );
    // A view file that instantiates at the top level.
    has(
        with("toplevel", |f| {
            set(f, "analog/a.lib", ".model DX", "R9 a b 1\n.model DX")
        }),
        "does not instantiate",
    );
    // An unknown behavioural platform.
    has(
        with("platform", |f| {
            set(
                f,
                "part.toml",
                "[pins]",
                "[behavioral]\nengine = \"renode\"\nplatform = \"nope@1\"\n[pins]",
            )
        }),
        "behavioral.platform",
    );
}

#[test]
fn pin_rules() {
    // vil < vih; a supply reference to a non-supply pin; incomplete sets.
    has(
        with("vil-vih", |f| {
            set(f, "pins.toml", "vil = \"0.3*VDD\"", "vil = \"0.8*VDD\"")
        }),
        "vil < vih",
    );
    has(
        with("supply-ref", |f| {
            set(f, "pins.toml", "voh = \"VDD\"", "voh = \"IN\"")
        }),
        "not a supply pin",
    );
    has(
        with("incomplete", |f| set(f, "pins.toml", "cin = \"3p\"\n", "")),
        "missing cin",
    );
    // Duplicate pin numbers; a role that cannot take the model.
    has(
        with("dup-number", |f| {
            set(f, "pins.toml", "number = \"2\"", "number = \"1\"")
        }),
        "listed twice",
    );
    has(
        with("role-model", |f| {
            set(
                f,
                "pins.toml",
                "models = [\"digital_out\"]",
                "models = [\"digital_in\"]",
            )
        }),
        "cannot take model",
    );
}

#[test]
fn local_only_is_fine_outside_first_party() {
    let mut files = base();
    set(
        &mut files,
        "part.toml",
        "Apache-2.0",
        "LicenseRef-local-only",
    );
    let refs: Vec<(&str, &str)> = files.iter().map(|(a, b)| (*a, b.as_str())).collect();
    let t = write_pkg("local-ok", &refs);
    assert!(errors(&t.0, false).is_empty());
}

#[test]
fn synthetic_part_1_bump_reads_a_part_0_package() {
    // G9: after a synthetic bb-part/1 bump, a bb-part/0 package is still
    // read (with the migration hint), migrate rewrites it, and a newer
    // major is refused.
    use bbpkg::schema::{rewrite_schema_line, Migration};
    fn up(t: &str) -> Result<String, String> {
        rewrite_schema_line(t, "bb-part/1")
    }
    let mut reg = Registry::current();
    for e in &mut reg.entries {
        if e.0 == "bb-part" {
            e.1 = 1;
            e.2 = vec![up as Migration];
        }
    }
    let (part, note) = package::parse_part(PART, &reg).unwrap();
    assert_eq!(part.id, "test/part");
    assert!(note.unwrap().contains("bb pkg migrate"));
    let migrated = reg.migrate("bb-part", "bb-part/0", PART).unwrap().unwrap();
    let (again, note) = package::parse_part(&migrated, &reg).unwrap();
    assert!(note.is_none());
    assert_eq!(again.analog, part.analog);
    let newer = PART.replace("bb-part/0", "bb-part/2");
    assert!(package::parse_part(&newer, &reg)
        .unwrap_err()
        .contains("upgrade bb"));
}

/// All findings (errors and warnings) of a package, with an optional chip.
fn findings(dir: &Path, chip: Option<&Path>) -> Vec<(Level, String)> {
    let reg = Registry::current();
    let pkg = package::load(dir, &reg).unwrap();
    let chip = chip.map(|c| package::load(c, &reg).unwrap());
    let platforms = vec!["nrf52840-bb@1".to_string()];
    let ctx = Context {
        platforms: &platforms,
        devices: &[],
        first_party: false,
        chip: chip.as_ref(),
    };
    lint(&pkg, &ctx, &reg)
        .into_iter()
        .map(|f| (f.level, f.msg))
        .collect()
}

const CHIP_PART: &str = r#"schema = "bb-part/0"
[part]
id = "test/chip"
version = "0.1.0"
kind = "soc"
license = "Apache-2.0"
[pins]
file = "pins.toml"
[behavioral]
engine = "renode"
platform = "nrf52840-bb@1"
[provenance]
source = "datasheet"
confidence = "datasheet_checked"
"#;

const CHIP_PINS: &str = r#"schema = "bb-pins/0"
[[pin]]
number = "A1"
name = "P0.01"
role = "io"
supply = "VDD"
models = ["digital_out", "digital_in"]
[[pin]]
number = "A2"
name = "VDD"
role = "power_in"
[[pin]]
number = "A3"
name = "VSS"
role = "ground"
[defaults.digital_out.default]
voh = "VDD"
vol = "0V"
rise = "5ns"
fall = "5ns"
zout = "200"
[defaults.digital_in.default]
vih = "0.7*VDD"
vil = "0.3*VDD"
cin = "3p"
"#;

const MOD_PART: &str = r#"schema = "bb-part/0"
[part]
id = "test/module"
version = "0.1.0"
kind = "module"
license = "Apache-2.0"
[pins]
file = "pins.toml"
[module]
chip = "test/chip"
pinmap = "module-pinmap.csv"
[provenance]
source = "datasheet"
confidence = "self_checked"
"#;

const MOD_PINS: &str = r#"schema = "bb-pins/0"
[[pin]]
number = "1"
name = "GND"
role = "ground"
[[pin]]
number = "2"
name = "P0.01"
role = "io"
supply = "VDD"
models = ["digital_out", "digital_in"]
[[pin]]
number = "3"
name = "VDD"
role = "power_in"
"#;

const MOD_MAP: &str = "pad,chip_pin\n1,VSS\n2,P0.01\n3,VDD\n";

fn module_case(name: &str, pins: &str, map: &str) -> Vec<(Level, String)> {
    let chip = write_pkg(
        &format!("{name}-chip"),
        &[("part.toml", CHIP_PART), ("pins.toml", CHIP_PINS)],
    );
    let m = write_pkg(
        &format!("{name}-mod"),
        &[
            ("part.toml", MOD_PART),
            ("pins.toml", pins),
            ("module-pinmap.csv", map),
        ],
    );
    findings(&m.0, Some(&chip.0))
}

#[test]
fn module_pinmap_rules() {
    // A clean module: every pad mapped once onto a chip pin of the same kind.
    let f = module_case("mod-clean", MOD_PINS, MOD_MAP);
    assert!(f.is_empty(), "{f:?}");
    let err = |f: Vec<(Level, String)>, needle: &str| {
        assert!(
            f.iter()
                .any(|(l, m)| *l == Level::Error && m.contains(needle)),
            "expected error {needle:?} in {f:?}"
        )
    };
    err(
        module_case("mod-unmapped", MOD_PINS, "pad,chip_pin\n1,VSS\n2,P0.01\n"),
        "pad 3 (VDD) is not mapped",
    );
    err(
        module_case(
            "mod-twice",
            MOD_PINS,
            "pad,chip_pin\n1,VSS\n2,P0.01\n2,P0.01\n3,VDD\n",
        ),
        "mapped twice",
    );
    err(
        module_case(
            "mod-nochip",
            MOD_PINS,
            "pad,chip_pin\n1,VSS\n2,P0.02\n3,VDD\n",
        ),
        "no such pin",
    );
    err(
        module_case(
            "mod-role",
            MOD_PINS,
            "pad,chip_pin\n1,VDD\n2,P0.01\n3,VDD\n",
        ),
        "the roles differ",
    );
    err(
        module_case("mod-header", MOD_PINS, "1,VSS\n2,P0.01\n3,VDD\n"),
        "header pad,chip_pin",
    );
    err(
        module_case(
            "mod-model",
            &MOD_PINS.replace(
                "\"digital_in\"]",
                "\"digital_in\", \"adc_in\"]\nadc = { channel = \"AIN0\" }",
            ),
            MOD_MAP,
        ),
        "offers adc_in",
    );
    err(
        module_case("mod-sets", &format!("{MOD_PINS}[defaults.digital_in.default]\nvih = \"0.7*VDD\"\nvil = \"0.3*VDD\"\ncin = \"3p\"\n"), MOD_MAP),
        "carry no parameter sets",
    );
    // Without the chip package, the chip pin names are not checked: a warning.
    let m = write_pkg(
        "mod-alone",
        &[
            ("part.toml", MOD_PART),
            ("pins.toml", MOD_PINS),
            ("module-pinmap.csv", MOD_MAP),
        ],
    );
    let f = findings(&m.0, None);
    assert!(
        f.iter().all(|(l, _)| *l == Level::Warning)
            && f.iter().any(|(_, m)| m.contains("package not found")),
        "{f:?}"
    );
}

#[test]
fn conflicting_view_parameters_are_reported() {
    // D9 rule 2: one parameter with different defaults in two views is a
    // warning naming both, never an error and never averaged.
    let mut files = base();
    set(
        &mut files,
        "part.toml",
        "subckt = \"PART_A\"\n",
        "subckt = \"PART_A\"\nparams = { rout = \"200\" }\n",
    );
    set(
        &mut files,
        "part.toml",
        "subckt = \"PART_B\"\n",
        "subckt = \"PART_B\"\nparams = { rout = \"800\" }\n",
    );
    set(
        &mut files,
        "analog/a.lib",
        ".subckt PART_A IN OUT GNDP",
        ".subckt PART_A IN OUT GNDP params: rout=200",
    );
    set(
        &mut files,
        "analog/b.lib",
        ".subckt PART_B IN OUT GNDP",
        ".subckt PART_B IN OUT GNDP params: rout=800",
    );
    let refs: Vec<(&str, &str)> = files.iter().map(|(a, b)| (*a, b.as_str())).collect();
    let t = write_pkg("param-conflict", &refs);
    let f = findings(&t.0, None);
    assert!(
        f.iter().any(|(l, m)| *l == Level::Warning
            && m.contains("conflict")
            && m.contains("a rout=200")
            && m.contains("b rout=800")),
        "{f:?}"
    );
    assert!(f.iter().all(|(l, _)| *l == Level::Warning), "{f:?}");
    // The same value written two ways is no conflict.
    set(
        &mut files,
        "part.toml",
        "rout = \"800\"",
        "rout = \"200.0\"",
    );
    let refs: Vec<(&str, &str)> = files.iter().map(|(a, b)| (*a, b.as_str())).collect();
    let t = write_pkg("param-no-conflict", &refs);
    assert!(findings(&t.0, None)
        .iter()
        .all(|(_, m)| !m.contains("conflict")));
}
