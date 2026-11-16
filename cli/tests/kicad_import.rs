//! ADR-007 gates M4b (the synthetic two-board system), M4f (no ordering
//! code is stripped) and M4d (a second `bb open` is byte-identical), on
//! `tests/fixtures/kicad/two-board`: boards `a` and `b` both have `U1`,
//! `TP1` and a `VSYS` net. No KiCad and no Renode are needed.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn bb() -> &'static str {
    env!("CARGO_BIN_EXE_bb")
}

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/kicad/two-board")
}

struct Tmp(PathBuf);
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let p = e.path();
        if p.is_dir() {
            copy_dir(&p, &to.join(e.file_name()));
        } else {
            std::fs::copy(&p, to.join(e.file_name())).unwrap();
        }
    }
}

/// A copy of the fixture with `extra` appended to its bb.toml.
fn project(name: &str, extra: &str) -> (Tmp, PathBuf) {
    let d = std::env::temp_dir().join(format!("bb-kicad-import-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    copy_dir(&fixture(), &d);
    let t = d.join("bb.toml");
    let mut s = std::fs::read_to_string(&t).unwrap();
    s.push_str(extra);
    std::fs::write(&t, s).unwrap();
    (Tmp(d.clone()), d)
}

/// The fixture resolves against its own `packages` only: an empty
/// first-party directory, so the synthetic RT9080-33GJ5 and BQ25180YBGR
/// (pins that are not the real parts') stay unresolved whatever the
/// first-party packages list (Phase 2 P1 added richtek/rt9080 and
/// ti/bq25180).
fn empty_first_party() -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "bb-kicad-import-no-first-party-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The two-board fixture's own runs (the four tests that use its synthetic
/// U1s): no first-party packages.
fn run_fixture(args: &[&str], dir: &Path) -> Output {
    Command::new(bb())
        .args(args)
        .current_dir(dir)
        .env("BB_FIRST_PARTY_PACKAGES", empty_first_party())
        .output()
        .unwrap()
}

fn run(args: &[&str], dir: &Path) -> Output {
    Command::new(bb())
        .args(args)
        .current_dir(dir)
        .env(
            "BB_FIRST_PARTY_PACKAGES",
            env!("CARGO_MANIFEST_DIR").to_string() + "/../packages",
        )
        .output()
        .unwrap()
}

/// A minimal little-endian ELF32 (ARM) with one 4-byte PT_LOAD at 0.
fn tiny_elf() -> Vec<u8> {
    let mut f = vec![0u8; 52];
    f[..7].copy_from_slice(b"\x7fELF\x01\x01\x01");
    f[16] = 2; // ET_EXEC
    f[18] = 40; // EM_ARM
    f[20] = 1;
    f[28] = 52; // e_phoff
    f[40] = 52; // e_ehsize
    f[42] = 32; // e_phentsize
    f[44] = 1; // e_phnum
    let mut ph = [0u8; 32];
    ph[0] = 1; // PT_LOAD
    ph[4] = 84; // p_offset
    ph[16] = 4; // p_filesz
    ph[20] = 4; // p_memsz
    f.extend_from_slice(&ph);
    f.extend_from_slice(b"\x00\x00\x00\x20");
    f
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

#[test]
fn a_bare_shared_ref_is_ambiguous_everywhere() {
    // [instances]: an error that lists both candidates.
    let (_t, d) = project(
        "inst",
        "\n[instances.\"U1\"]\npackage = \"none\"\nreason = \"x\"\n",
    );
    let o = run_fixture(&["open", "."], &d);
    let t = text(&o);
    assert!(!o.status.success(), "{t}");
    assert!(
        t.contains("U1 is ambiguous")
            && t.contains("a:U1 (RT9080-33GJ5)")
            && t.contains("b:U1 (BQ25180YBGR)"),
        "{t}"
    );
    // [[bind]]: bb open writes the board but exits non-zero, naming both.
    let (_t, d) = project(
        "bind",
        "\n[[bind]]\npin = \"U1.EN\"\nmodel = \"digital_out\"\n",
    );
    let o = run_fixture(&["open", "."], &d);
    let t = text(&o);
    assert!(!o.status.success(), "{t}");
    assert!(t.contains("a:U1 (RT9080-33GJ5), b:U1 (BQ25180YBGR)"), "{t}");
    // --probe on bb run: refused before anything starts.
    let (_t, d) = project("probe", "");
    assert!(run_fixture(&["open", "."], &d).status.success());
    let o = run_fixture(
        &[
            "run",
            "--for",
            "1ms",
            "--probe",
            "net:U1.EN",
            "--out",
            "out",
        ],
        &d,
    );
    let t = text(&o);
    assert!(!o.status.success(), "{t}");
    assert!(
        t.contains("--probe U1.EN") && t.contains("a:U1 (RT9080-33GJ5), b:U1 (BQ25180YBGR)"),
        "{t}"
    );
    assert!(
        !d.join("out").exists(),
        "bb run wrote outputs before refusing"
    );
    // A test point alias is qualified too: a bare TP1 is ambiguous.
    let o = run_fixture(&["probe", "net:TP1"], &d);
    assert!(
        !o.status.success() && text(&o).contains("a:TP1, b:TP1"),
        "{}",
        text(&o)
    );
}

#[test]
fn qualified_names_resolve() {
    let (_t, d) = project(
        "ok",
        "\n[instances.\"a:U1\"]\npackage = \"none\"\nreason = \"regulator: no model needed here\"\n",
    );
    let o = run_fixture(&["open", "."], &d);
    assert!(o.status.success(), "{}", text(&o));
    let bbm = std::fs::read_to_string(d.join("board.bbm")).unwrap();
    // Nets and probe aliases are qualified in a system; SPICE names carry
    // the board; only the simulated board's ground is node 0.
    for s in [
        "\"name\":\"a:VSYS\"",
        "\"name\":\"b:VSYS\"",
        "\"spice\":\"a_VSYS\"",
        "\"name\":\"a:GND\",\"nodes\":[{\"pin\":\"2\",\"ref\":\"U1\"}],\"spice\":\"0\"",
        "\"spice\":\"b_GND\"",
        "\"alias\":\"a:TP1\"",
        "\"alias\":\"b:TP1\"",
        "\"connections\":[{\"a\":\"a:U1\",\"b\":\"b:U1\",\"pins\":[[\"1\",\"B1\"]]}]",
    ] {
        assert!(bbm.contains(s), "{s} not in {bbm}");
    }
    // a:U1 is excluded by its override; b:U1 stays unresolved.
    assert!(bbm.contains("\"reason\":\"regulator: no model needed here\""));
    // A leaf net name resolves within the simulated board first (VSYS is
    // on both boards; a's wins); a:TP1 resolves; b's VBUS by its leaf.
    for (n, want) in [
        ("VSYS", "a:VSYS"),
        ("LEAF", "a:/power/LEAF"),
        ("a:TP1", "a:VSYS"),
        ("VBUS", "b:VBUS"),
    ] {
        let o = run_fixture(&["probe", &format!("net:{n}")], &d);
        assert!(o.status.success(), "{n}: {}", text(&o));
        assert!(
            text(&o).contains(&format!(
                "bb probe: {n} -> {want}
"
            )),
            "{n}: {}",
            text(&o)
        );
        assert!(
            std::fs::read_to_string(d.join("bb.toml"))
                .unwrap()
                .contains(&format!("\"{n}\"")),
            "{n}"
        );
    }
    // M4d: a second import gives the same bytes.
    let before: Vec<Vec<u8>> = ["board.bbm", ".bb/resolution.json", ".bb/unresolved.toml"]
        .iter()
        .map(|f| std::fs::read(d.join(f)).unwrap())
        .collect();
    assert!(run_fixture(&["open", "."], &d).status.success());
    for (f, b) in ["board.bbm", ".bb/resolution.json", ".bb/unresolved.toml"]
        .iter()
        .zip(before)
    {
        assert_eq!(std::fs::read(d.join(f)).unwrap(), b, "{f}");
    }
    assert!(run_fixture(&["open", ".", "--check"], &d).status.success());
}

#[test]
fn an_ordering_code_is_never_stripped() {
    // M4f: the package lists only RT9080; RT9080-33GJ5 must not bind to it.
    let (_t, d) = project("mpn", "");
    let o = run_fixture(&["open", "."], &d);
    assert!(o.status.success(), "{}", text(&o));
    let r = std::fs::read_to_string(d.join(".bb/resolution.json")).unwrap();
    assert!(
        r.contains("\"key\":\"RT9080-33GJ5\",\"result\":\"no package\",\"stage\":\"mpn\""),
        "{r}"
    );
    assert!(r.contains("\"candidates\":[\"test/rt9080\"]"), "{r}");
    let bbm = std::fs::read_to_string(d.join("board.bbm")).unwrap();
    assert!(!bbm.contains("test/rt9080"));
    let stubs = std::fs::read_to_string(d.join(".bb/unresolved.toml")).unwrap();
    assert!(stubs.contains("[instances.\"a:U1\"]") && stubs.contains("[instances.\"b:U1\"]"));
    // With a pattern that names the ordering code, it binds.
    let p = d.join("packages/test/rt9080/part.toml");
    let s = std::fs::read_to_string(&p).unwrap().replace(
        "mpn = [\"RT9080\"]",
        "mpn = [\"RT9080\"]\nmpn_patterns = [\"RT9080-33GJ[5]\"]",
    );
    std::fs::write(&p, s).unwrap();
    let o = run_fixture(&["open", "."], &d);
    assert!(o.status.success(), "{}", text(&o));
    let bbm = std::fs::read_to_string(d.join("board.bbm")).unwrap();
    assert!(
        bbm.contains("\"resolution\":{\"key\":\"RT9080-33GJ5\",\"stage\":\"mpn\"}"),
        "{bbm}"
    );
}

#[test]
fn a_minimal_bb_toml_is_written_for_a_net() {
    let (_t, d) = project("minimal", "");
    let sub = d.join("solo");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::copy(d.join("b.net"), sub.join("b.net")).unwrap();
    let o = run_fixture(&["open", "b.net"], &sub);
    assert!(o.status.success(), "{}", text(&o));
    let t = std::fs::read_to_string(sub.join("bb.toml")).unwrap();
    assert!(
        t.contains("netlist = \"b.net\"") && t.contains("schema = \"bb-project/1\""),
        "{t}"
    );
    assert!(sub.join("board.bbm").is_file());
}

/// The deck lines of `bench/pwl.toml` in [`a_bench_elaborates_into_the_deck`].
const PWL_DECK_LINES: &[&str] = &[
    // An ideal PWL source drives its net directly (the net's SPICE name).
    "V_pwl_n3V3 n3V3 0 PWL(0 0 1e-4 0 2e-4 3.3)\n",
    // With rs, the source drives an internal node and rs connects it.
    "V_pwl_LED bb_pwl_LED 0 PWL(0 1 0.001 2.5)\n",
    "R_pwl_LED bb_pwl_LED LED 50\n",
    // The rest of the scope is unchanged by the bench.
    "XR3 ALARM LED GENERIC_RESISTOR_IDEAL value=330\n",
];

#[test]
fn a_bench_elaborates_into_the_deck() {
    // examples/adc/kicad with a bench (ADR-007 D5): the analog scope keeps
    // R3 out, the supply, load and temperature go into the deck as plain
    // SPICE numbers, and run.json records the scope. No engine starts.
    let d = std::env::temp_dir().join(format!("bb-kicad-bench-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let _t = Tmp(d.clone());
    copy_dir(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../examples/adc/kicad"),
        &d,
    );
    let _ = std::fs::remove_dir_all(d.join(".bb"));
    // bb flash records the ELF image hash (ADR-008 D5), so the file must
    // have a loadable segment: a minimal ELF32 with one 4-byte PT_LOAD.
    std::fs::write(d.join("fw.elf"), tiny_elf()).unwrap();
    // The per-instance analog view override (ADR-006 D3 step 1; the
    // post-M7 review found it untested): D1's only view, chosen by name.
    let toml = d.join("bb.toml");
    let s =
        std::fs::read_to_string(&toml).unwrap() + "\n[instances.\"main:D1\"]\nanalog = \"diode\"\n";
    std::fs::write(&toml, s).unwrap();
    std::fs::create_dir_all(d.join("bench")).unwrap();
    std::fs::write(
        d.join("bench/test.toml"),
        "schema = \"bb-bench/0\"\ntemp_c = \"-10\"\n\
         [[supply]]\nnet = \"LED\"\nvolts = \"2V\"\nrs = \"10\"\ntrack = { net = \"ADC_IN\", drop = \"100mV\" }\n\
         [[load]]\nnet = \"ADC_IN\"\namps = \"1mA\"\n\
         [analog]\nnets = [\"DRIVE\", \"ADC_IN\", \"GND\", \"LED\"]\n[options]\nset = \"phase0\"\n",
    )
    .unwrap();
    // [[pwl]] sources (plan section 3's minimal bench; the post-M7 review
    // found their elaboration untested): an ideal one and one with rs.
    std::fs::write(
        d.join("bench/pwl.toml"),
        "schema = \"bb-bench/0\"\n\
         [[pwl]]\nnet = \"+3V3\"\npoints = [[\"0\", \"0V\"], [\"100us\", \"0V\"], [\"200us\", \"3.3V\"]]\n\
         [[pwl]]\nnet = \"LED\"\npoints = [[\"0\", \"1V\"], [\"1ms\", \"2.5V\"]]\nrs = \"50\"\n\
         [analog]\nnets = [\"DRIVE\", \"ADC_IN\", \"GND\", \"LED\", \"ALARM\", \"+3V3\"]\n[options]\nset = \"phase0\"\n",
    )
    .unwrap();
    assert!(run(&["open", "."], &d).status.success());
    let o = run(&["flash", "--target", "U1", "fw.elf"], &d);
    assert!(o.status.success(), "{}", text(&o));
    let o = run(
        &[
            "run",
            "--for",
            "1ms",
            "--bench",
            "bench/test.toml",
            "--elaborate-only",
            "--out",
            "out",
        ],
        &d,
    );
    assert!(o.status.success(), "{}", text(&o));
    let deck = std::fs::read_to_string(d.join("out/deck.cir")).unwrap();
    for line in [
        "XC1 ADC_IN 0 GENERIC_CAPACITOR_IDEAL value=4.7e-7\n",
        "XD1 LED 0 example_led_red\n",
        "XR1 DRIVE ADC_IN GENERIC_RESISTOR_IDEAL value=10000\n",
        "XR2 ADC_IN 0 GENERIC_RESISTOR_IDEAL value=33000\n",
        // A supply with rs is its Norton equivalent (ADR-007 Amendment 3).
        "B_bench_LED 0 LED I = (min(2, V(ADC_IN) - 0.1) - V(LED)) / 10\n",
        "I_bench_load0_ADC_IN ADC_IN 0 DC 0.001\n",
        ".temp -10\n",
        ".options RELTOL=1e-5 VNTOL=1e-8 ABSTOL=1e-13\n",
    ] {
        assert!(deck.contains(line), "{line:?} not in\n{deck}");
    }
    assert!(!deck.contains("XR3"), "R3 is outside the scope:\n{deck}");
    assert_eq!(deck.matches(".subckt GENERIC_RESISTOR_IDEAL").count(), 1);
    let rj = std::fs::read_to_string(d.join("out/run.json")).unwrap();
    assert!(rj.contains("\"ref\":\"main:R3\""), "{rj}");
    assert!(rj.contains("\"options\":\"phase0\""), "{rj}");
    assert!(
        !rj.contains(&d.display().to_string()),
        "an absolute path in run.json"
    );
    // The override is applied and recorded: D1 (and only D1) at rung "override".
    assert_eq!(rj.matches("\"rung\":\"override\"").count(), 1, "{rj}");
    let d1 = &rj[rj.find("\"ref\":\"main:D1\"").expect("main:D1 in run.json")..];
    let d1 = &d1[..d1.find('}').unwrap()];
    assert!(d1.contains("\"rung\":\"override\""), "{d1}");
    // The [[pwl]] bench: an ideal PWL source on +3V3, and one behind rs on LED.
    let o = run(
        &[
            "run",
            "--for",
            "1ms",
            "--bench",
            "bench/pwl.toml",
            "--elaborate-only",
            "--out",
            "out-pwl",
        ],
        &d,
    );
    assert!(o.status.success(), "{}", text(&o));
    let deck = std::fs::read_to_string(d.join("out-pwl/deck.cir")).unwrap();
    for line in PWL_DECK_LINES {
        assert!(deck.contains(line), "{line:?} not in\n{deck}");
    }
    // A probe of an unbound MCU pin is refused (only bound pins have a
    // kernel net).
    let o = run(
        &[
            "run",
            "--for",
            "1ms",
            "--bench",
            "bench/test.toml",
            "--elaborate-only",
            "--probe",
            "net:U1.PB0",
            "--out",
            "out2",
        ],
        &d,
    );
    assert!(
        !o.status.success() && text(&o).contains("has no [[bind]]"),
        "{}",
        text(&o)
    );
    // An option set no view is qualified for is refused.
    let b = std::fs::read_to_string(d.join("bench/test.toml"))
        .unwrap()
        .replace("set = \"phase0\"", "set = \"suite_fallback\"");
    std::fs::write(d.join("bench/test.toml"), b).unwrap();
    let o = run(
        &[
            "run",
            "--for",
            "1ms",
            "--bench",
            "bench/test.toml",
            "--elaborate-only",
            "--out",
            "out3",
        ],
        &d,
    );
    assert!(
        !o.status.success() && text(&o).contains("not lattice-qualified"),
        "{}",
        text(&o)
    );
    // A changed source makes bb run refuse ("run bb open").
    let net = d.join("adc.net");
    let s = std::fs::read_to_string(&net)
        .unwrap()
        .replace("(value \"33k\")", "(value \"47k\")");
    std::fs::write(&net, s).unwrap();
    let o = run(
        &["run", "--for", "1ms", "--elaborate-only", "--out", "out4"],
        &d,
    );
    assert!(
        !o.status.success() && text(&o).contains("run `bb open`"),
        "{}",
        text(&o)
    );
}

/// examples/adc/kicad with a `[[device]]` (bb-project/1, ADR-013 D1).
const DEVICE: &str =
    "\n[[device]]\nref = \"M1\"\npackage = \"dms/detent-mechanism\"\nvirtual = true\n\
params = { ts = \"75ms\", s0 = \"250\" }\n\
[[device.bind]]\npin = \"IN_A1\"\nnet = \"DRIVE\"\nmodel = \"digital_in\"\n\
[[device.bind]]\npin = \"HOME_N\"\nnet = \"LED\"\nmodel = \"digital_out\"\n";

fn device_project(name: &str, extra: &str) -> (Tmp, PathBuf) {
    let d = std::env::temp_dir().join(format!("bb-kicad-device-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    copy_dir(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../examples/adc/kicad"),
        &d,
    );
    let _ = std::fs::remove_dir_all(d.join(".bb"));
    std::fs::write(d.join("fw.elf"), tiny_elf()).unwrap();
    let toml = d.join("bb.toml");
    let s = std::fs::read_to_string(&toml).unwrap() + extra;
    std::fs::write(&toml, s).unwrap();
    std::fs::create_dir_all(d.join("bench")).unwrap();
    // DRIVE outside the analog scope: a digital-only net (ADR-014 D4).
    std::fs::write(
        d.join("bench/digital.toml"),
        "schema = \"bb-bench/0\"\n[analog]\nnets = [\"ADC_IN\", \"GND\", \"LED\", \"ALARM\"]\n",
    )
    .unwrap();
    (Tmp(d.clone()), d)
}

fn elaborate(d: &Path, bench: &str, out: &str, extra: &[&str]) -> Output {
    let mut args = vec!["run", "--for", "1ms", "--elaborate-only", "--out", out];
    if !bench.is_empty() {
        args.extend(["--bench", bench]);
    }
    args.extend(extra);
    run(&args, d)
}

#[test]
fn a_virtual_device_is_board_content_and_runs_beside_the_mcu() {
    let (_t, d) = device_project("ok", DEVICE);
    let o = run(&["open", "."], &d);
    assert!(o.status.success(), "{}", text(&o));
    assert!(
        text(&o)
            .contains("device main:M1 -> dms/detent-mechanism (dms/mechanism@2, virtual), 2 pins"),
        "{}",
        text(&o)
    );
    let bbm = std::fs::read_to_string(d.join("board.bbm")).unwrap();
    // bb-board/1: the virtual instance, its pins on the bound nets (the
    // binding is board content), and the device identity as a platform.
    for s in [
        "\"schema\":\"bb-board/1\"",
        "\"ref\":\"M1\",\"resolution\":{\"key\":\"dms/detent-mechanism\",\"stage\":\"override\"}",
        "\"status\":\"ok\",\"value_text\":\"\",\"virtual\":true",
        "{\"name\":\"IN_A1\",\"net\":\"DRIVE\",\"number\":\"1\"}",
        "{\"name\":\"HOME_N\",\"net\":\"LED\",\"number\":\"6\"}",
        "{\"name\":\"NSLEEP\",\"number\":\"5\"}",
        "{\"pin\":\"1\",\"ref\":\"M1\"}",
        "\"id\":\"device:dms/mechanism@2\"",
    ] {
        assert!(bbm.contains(s), "{s} not in {bbm}");
    }
    let lock = std::fs::read_to_string(d.join("bb.lock")).unwrap();
    assert!(lock.contains("id = \"device:dms/mechanism@2\""), "{lock}");
    assert!(lock.contains("id = \"dms/detent-mechanism\""), "{lock}");
    assert!(run(&["open", ".", "--check"], &d).status.success());
    assert!(run(&["flash", "--target", "U1", "fw.elf"], &d)
        .status
        .success());
    // Every net in scope (no bench): DRIVE keeps its glue, and run.json
    // records the device but no digital-only net.
    let o = elaborate(&d, "", "out", &["--probe", "net:M1.HOME_N"]);
    assert!(o.status.success(), "{}", text(&o));
    let rj = std::fs::read_to_string(d.join("out/run.json")).unwrap();
    assert!(
        rj.contains(
            "\"devices\":[{\"device\":\"dms/mechanism@2\",\"package\":\"dms/detent-mechanism\",\
             \"params\":{\"s0\":\"250\",\"ts\":\"75ms\"},\"ref\":\"main:M1\",\"version\":\"0.2.0\",\
             \"virtual\":true}]"
        ),
        "{rj}"
    );
    assert!(!rj.contains("digital_only"), "{rj}");
    assert!(!std::fs::read_to_string(d.join("out/deck.cir"))
        .unwrap()
        .contains("XM1"));
    // DRIVE outside [analog] nets: digital-only, R1 out of the deck, and a
    // probe of DRIVE by net name is refused with its driver's pin.
    let o = elaborate(&d, "bench/digital.toml", "out-d", &[]);
    assert!(o.status.success(), "{}", text(&o));
    let rj = std::fs::read_to_string(d.join("out-d/run.json")).unwrap();
    assert!(rj.contains("\"digital_only\":[\"DRIVE\"]"), "{rj}");
    assert!(!std::fs::read_to_string(d.join("out-d/deck.cir"))
        .unwrap()
        .contains("XR1 "));
    let o = elaborate(&d, "bench/digital.toml", "out-p", &["--probe", "net:DRIVE"]);
    assert!(
        !o.status.success()
            && text(&o).contains("digital-only net (ADR-014 D4); probe its driver's pin U1.PA4"),
        "{}",
        text(&o)
    );
}

#[test]
fn a_device_binding_is_refused_when_it_cannot_be_honoured() {
    let refused = |name: &str, extra: &str, want: &str| {
        let (_t, d) = device_project(name, extra);
        let o = run(&["open", "."], &d);
        assert!(!o.status.success(), "{name}: {}", text(&o));
        assert!(
            text(&o).contains(want),
            "{name}: {want:?} not in {}",
            text(&o)
        );
    };
    // A virtual reference must not collide with a symbol's; a non-virtual
    // device needs its symbol, resolved to the device package.
    refused(
        "collide",
        &DEVICE.replace("ref = \"M1\"", "ref = \"R1\""),
        "main:R1 is already an instance of the board",
    );
    refused(
        "nosym",
        &DEVICE.replace("virtual = true\n", ""),
        "no instance main:M1 on the schematic",
    );
    refused(
        "wrongpkg",
        &DEVICE
            .replace("virtual = true\n", "")
            .replace("ref = \"M1\"", "ref = \"R1\""),
        "main:R1 resolves to generic/resistor (ok), not dms/detent-mechanism",
    );
    // spec/package.md section 12: adc_in, unknown pins and parameters, a
    // model against the pin's direction, a package that is not a device.
    refused(
        "adc",
        &DEVICE.replace("model = \"digital_in\"", "model = \"adc_in\""),
        "a device pin is digital_in or digital_out",
    );
    refused(
        "pin",
        &DEVICE.replace("pin = \"IN_A1\"", "pin = \"FOO\""),
        "has no pin FOO",
    );
    refused(
        "param",
        &DEVICE.replace("ts = \"75ms\"", "nope = \"1\""),
        "unknown parameter \"nope\"",
    );
    refused(
        "dir",
        &DEVICE.replace("model = \"digital_out\"", "model = \"digital_in\""),
        "HOME_N is an output of device",
    );
    refused(
        "notdev",
        &DEVICE.replace("dms/detent-mechanism", "generic/resistor"),
        "package generic/resistor is not a device",
    );
    refused(
        "net",
        &DEVICE.replace("net = \"LED\"", "net = \"NOPE\""),
        "NOPE",
    );
    // [[device]] is a bb-project/1 key.
    let (_t, d) = device_project("v0", DEVICE);
    let toml = d.join("bb.toml");
    let s = std::fs::read_to_string(&toml)
        .unwrap()
        .replace("bb-project/1", "bb-project/0");
    std::fs::write(&toml, s).unwrap();
    let o = run(&["open", "."], &d);
    assert!(
        !o.status.success() && text(&o).contains("[[device]] is a bb-project/1 key"),
        "{}",
        text(&o)
    );
    // bb run: one driver per net (ALARM is U1.PA5's), and a device input on
    // a digital-only net that no digital output drives.
    let (_t, d) = device_project(
        "drivers",
        &DEVICE.replace("net = \"LED\"", "net = \"ALARM\""),
    );
    assert!(run(&["open", "."], &d).status.success());
    assert!(run(&["flash", "--target", "U1", "fw.elf"], &d)
        .status
        .success());
    let o = elaborate(&d, "", "out", &[]);
    assert!(
        !o.status.success() && text(&o).contains("net ALARM is already driven by another pin"),
        "{}",
        text(&o)
    );
    let (_t, d) = device_project(
        "undriven",
        &DEVICE.replace("net = \"DRIVE\"", "net = \"ADC_IN\""),
    );
    assert!(run(&["open", "."], &d).status.success());
    assert!(run(&["flash", "--target", "U1", "fw.elf"], &d)
        .status
        .success());
    std::fs::write(
        d.join("bench/digital.toml"),
        "schema = \"bb-bench/0\"\n[analog]\nnets = [\"DRIVE\", \"GND\", \"LED\", \"ALARM\"]\n",
    )
    .unwrap();
    let o = elaborate(&d, "bench/digital.toml", "out", &[]);
    assert!(
        !o.status.success()
            && text(&o)
                .contains("M1.IN_A1: it reads ADC_IN, which is outside the bench's [analog] nets"),
        "{}",
        text(&o)
    );
}
