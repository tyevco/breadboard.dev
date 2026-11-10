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
    let o = run(&["open", "."], &d);
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
    let o = run(&["open", "."], &d);
    let t = text(&o);
    assert!(!o.status.success(), "{t}");
    assert!(t.contains("a:U1 (RT9080-33GJ5), b:U1 (BQ25180YBGR)"), "{t}");
    // --probe on bb run: refused before anything starts.
    let (_t, d) = project("probe", "");
    assert!(run(&["open", "."], &d).status.success());
    let o = run(
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
    let o = run(&["probe", "net:TP1"], &d);
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
    let o = run(&["open", "."], &d);
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
        "\"connections\":[{\"a\":\"a:U1\",\"b\":\"b:U1\"}]",
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
        let o = run(&["probe", &format!("net:{n}")], &d);
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
    assert!(run(&["open", "."], &d).status.success());
    for (f, b) in ["board.bbm", ".bb/resolution.json", ".bb/unresolved.toml"]
        .iter()
        .zip(before)
    {
        assert_eq!(std::fs::read(d.join(f)).unwrap(), b, "{f}");
    }
    assert!(run(&["open", ".", "--check"], &d).status.success());
}

#[test]
fn an_ordering_code_is_never_stripped() {
    // M4f: the package lists only RT9080; RT9080-33GJ5 must not bind to it.
    let (_t, d) = project("mpn", "");
    let o = run(&["open", "."], &d);
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
    let o = run(&["open", "."], &d);
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
    let o = run(&["open", "b.net"], &sub);
    assert!(o.status.success(), "{}", text(&o));
    let t = std::fs::read_to_string(sub.join("bb.toml")).unwrap();
    assert!(
        t.contains("netlist = \"b.net\"") && t.contains("schema = \"bb-project/0\""),
        "{t}"
    );
    assert!(sub.join("board.bbm").is_file());
}

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
    std::fs::write(d.join("fw.elf"), b"\x7fELF not really an image").unwrap();
    std::fs::create_dir_all(d.join("bench")).unwrap();
    std::fs::write(
        d.join("bench/test.toml"),
        "schema = \"bb-bench/0\"\ntemp_c = \"-10\"\n\
         [[supply]]\nnet = \"LED\"\nvolts = \"2V\"\nrs = \"10\"\ntrack = { net = \"ADC_IN\", drop = \"100mV\" }\n\
         [[load]]\nnet = \"ADC_IN\"\namps = \"1mA\"\n\
         [analog]\nnets = [\"DRIVE\", \"ADC_IN\", \"GND\", \"LED\"]\n[options]\nset = \"phase0\"\n",
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
        "XC1 ADC_IN 0 bb_generic_capacitor value=4.7e-7\n",
        "XD1 LED 0 example_led_red\n",
        "XR1 DRIVE ADC_IN bb_generic_resistor value=10000\n",
        "XR2 ADC_IN 0 bb_generic_resistor value=33000\n",
        "B_bench_LED bb_bench_LED 0 V = min(2, V(ADC_IN) - 0.1)\n",
        "R_bench_LED bb_bench_LED LED 10\n",
        "I_bench_load0_ADC_IN ADC_IN 0 DC 0.001\n",
        ".temp -10\n",
        ".options RELTOL=1e-5 VNTOL=1e-8 ABSTOL=1e-13\n",
    ] {
        assert!(deck.contains(line), "{line:?} not in\n{deck}");
    }
    assert!(!deck.contains("XR3"), "R3 is outside the scope:\n{deck}");
    assert_eq!(deck.matches(".subckt bb_generic_resistor").count(), 1);
    let rj = std::fs::read_to_string(d.join("out/run.json")).unwrap();
    assert!(rj.contains("\"ref\":\"main:R3\""), "{rj}");
    assert!(rj.contains("\"options\":\"phase0\""), "{rj}");
    assert!(
        !rj.contains(&d.display().to_string()),
        "an absolute path in run.json"
    );
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
