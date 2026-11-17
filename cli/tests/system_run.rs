//! Phase 2 R2 (ADR-013 D2, gates G2's deck and G3): single-MCU system runs on
//! `examples/dial-me-spinner/e1-system` (`simulate = ["main", "base"]`),
//! elaborated only (`bb run --elaborate-only`): no Renode and no ngspice are
//! needed. The runs themselves are `make dms-e1-system`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn bb() -> &'static str {
    env!("CARGO_BIN_EXE_bb")
}

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn dms() -> PathBuf {
    repo().join("examples/dial-me-spinner")
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
        let name = e.file_name();
        if name == ".bb" {
            continue;
        }
        if p.is_dir() {
            copy_dir(&p, &to.join(name));
        } else {
            std::fs::copy(&p, to.join(name)).unwrap();
        }
    }
}

/// A copy of e1-system with what it names outside itself (../kicad,
/// ../system/packages, ../firmware/prebuilt/led_rail.elf), at
/// `<tmp>/examples/dial-me-spinner/e1-system` beside a `<tmp>/packages` link
/// to the first-party directory (so bb.lock's paths hold), `edit` applied
/// to its bb.toml and `bench` to bench/cold.toml.
fn project(
    name: &str,
    edit: impl Fn(&str) -> String,
    bench: impl Fn(&str) -> String,
) -> (Tmp, PathBuf) {
    let root = std::env::temp_dir().join(format!("bb-system-run-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::os::unix::fs::symlink(repo().join("packages"), root.join("packages")).unwrap();
    let d = root.join("examples/dial-me-spinner");
    let p = d.join("e1-system");
    copy_dir(&dms().join("e1-system"), &p);
    copy_dir(&dms().join("system/packages"), &d.join("system/packages"));
    for (b, f) in [
        ("main", "dial-me-spinner-main.net"),
        ("base", "dial-me-spinner-base.net"),
    ] {
        let to = d.join("kicad").join(b);
        std::fs::create_dir_all(&to).unwrap();
        std::fs::copy(dms().join("kicad").join(b).join(f), to.join(f)).unwrap();
    }
    std::fs::create_dir_all(d.join("firmware/prebuilt")).unwrap();
    std::fs::copy(
        dms().join("firmware/prebuilt/led_rail.elf"),
        d.join("firmware/prebuilt/led_rail.elf"),
    )
    .unwrap();
    let t = p.join("bb.toml");
    let s = std::fs::read_to_string(&t).unwrap();
    std::fs::write(&t, edit(&s)).unwrap();
    let b = p.join("bench/cold.toml");
    let s = std::fs::read_to_string(&b).unwrap();
    std::fs::write(&b, bench(&s)).unwrap();
    (Tmp(root), p)
}

fn same(s: &str) -> String {
    s.to_string()
}

fn run(args: &[&str], dir: &Path) -> Output {
    Command::new(bb())
        .args(args)
        .current_dir(dir)
        .env("BB_FIRST_PARTY_PACKAGES", repo().join("packages"))
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

fn elaborate(d: &Path, out: &str) -> Output {
    run(
        &[
            "run",
            "--elaborate-only",
            "--for",
            "1ms",
            "--bench",
            "bench/cold.toml",
            "--out",
            out,
        ],
        d,
    )
}

fn line<'a>(deck: &'a str, start: &str) -> &'a str {
    deck.lines()
        .find(|l| l.starts_with(start))
        .unwrap_or_else(|| panic!("no line {start:?} in the deck:\n{deck}"))
}

#[test]
fn the_system_deck_holds_both_boards_and_the_harness() {
    // ADR-013 D2: every simulated board's in-scope instances, sorted by board
    // then reference; each board its own ground (main's node 0, base's
    // base_GND); the connection through its harness view; bench elements
    // return to their net's board's ground; run.json records `simulated`.
    let (_t, d) = project("deck", same, same);
    let o = run(&["open", "--check", "."], &d);
    assert!(o.status.success(), "{}", text(&o));
    let o = elaborate(&d, "out");
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    let deck = std::fs::read_to_string(d.join("out/deck.cir")).unwrap();
    assert!(
        deck.starts_with(
            "* Breadboard elaborated deck (ADR-007 D7, ADR-013 D2): boards main, base (ground \
             main), fidelity pi, option set phase0\n"
        ),
        "{deck}"
    );
    assert_eq!(
        line(&deck, "Xbase_U1 "),
        "Xbase_U1 base_VBUS base_VSYS base_VBAT base_GND TI_BQ25180_PP_PI buvlo=3 buvlo_hys=0.15 \
         glim=1e-5 ilim=0.475 ronbat=0.055 ronin=0.27 sup_gate=0 tbuvlo=6e-5 vbsup=0.03 \
         vsysreg=4.5"
    );
    assert_eq!(
        line(&deck, "Xbase_C2 "),
        "Xbase_C2 base_VSYS base_GND GENERIC_CAPACITOR_IDEAL value=1e-5"
    );
    // Ports A1..A12, then B1..B12: VSYS on 1-2, GND on 3, 4 and 8, the dead
    // wires (USB_DM, USB_DP, BTN_N outside the scope at both ends) on node 0.
    assert_eq!(
        line(&deck, "X_conn1_main_J1_base_J3 "),
        "X_conn1_main_J1_base_J3 main_VSYS main_VSYS 0 0 main_VBUS 0 0 0 main_I2C_SCL \
         main_I2C_SDA main_CHG_INT_N 0 base_VSYS base_VSYS base_GND base_GND base_VBUS 0 0 \
         base_GND base_Base_board_I2C_SCL base_Base_board_I2C_SDA base_Base_board_CHG_INT_N 0 \
         dms_link_harness_wires rc=0.02 rw_pwr=0.21 rw_sig=0.538 wlen=0.08"
    );
    // Instances before the harness, base before main.
    let pos = |s: &str| deck.find(s).unwrap();
    assert!(pos("\nXbase_U1 ") < pos("\nXmain_U2 ") && pos("\nXmain_U2 ") < pos("\nX_conn1_"));
    assert!(
        !deck.contains("\nXmain_U1 "),
        "main:U1 is excluded by the bench"
    );
    assert_eq!(
        line(&deck, "I_bench_bat0 "),
        "I_bench_bat0 base_GND base_VBAT DC {3 / 0.5}"
    );
    assert_eq!(
        line(&deck, "R_bench_bat0_0 "),
        "R_bench_bat0_0 base_VBAT base_GND 0.5"
    );
    assert_eq!(
        line(&deck, "B_bench_base_VBUS "),
        "B_bench_base_VBUS base_GND base_VBUS I = (0 - V(base_VBUS,base_GND)) / 1000"
    );
    // A main-board element stays on node 0, in its one-board form.
    assert_eq!(
        line(&deck, "B_bench_main_3V3 "),
        "B_bench_main_3V3 0 main_3V3 I = (min(3.3, V(main_VSYS) - 0.044167) - V(main_3V3)) / 0.01"
    );
    assert!(
        t.contains(
            "[[connection]] main:J1 - base:J3: these mated pairs are outside the analog scope at \
             both ends, so their wires carry nothing: main:BTN_N/base:/Base board/BTN_N, \
             main:USB_DM/base:/Base board/USB_DM, main:USB_DP/base:/Base board/USB_DP"
        ),
        "{t}"
    );
    let rj = std::fs::read_to_string(d.join("out/run.json")).unwrap();
    assert!(rj.contains("\"simulated\":[\"main\",\"base\"]"), "{rj}");
    assert!(
        rj.contains(
            "{\"fidelity\":\"pi\",\"kind\":\"harness\",\"package\":\"dial-me-spinner/link-harness\""
        ) && rj.contains(
            "\"ref\":\"main:J1=base:J3\",\"rung\":\"pi\",\"version\":\"0.1.0\",\"view\":\"wires\"}"
        ),
        "{rj}"
    );
    // A pure function of its inputs: elaborated twice, the same bytes.
    let o = elaborate(&d, "out2");
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(
        deck,
        std::fs::read_to_string(d.join("out2/deck.cir")).unwrap()
    );
}

#[test]
fn a_second_target_is_refused() {
    // Gate G3: multi-MCU simulation is Phase 3.
    let (_t, d) = project(
        "targets",
        |s| {
            let img = s
                .lines()
                .find(|l| l.starts_with("images = "))
                .unwrap()
                .to_string();
            format!("{s}\n[targets.\"base:U1\"]\n{img}\n")
        },
        same,
    );
    let o = run(&["open", "."], &d);
    assert!(o.status.success(), "{}", text(&o));
    let o = elaborate(&d, "out");
    let t = text(&o);
    assert!(!o.status.success(), "{t}");
    assert!(
        t.contains(
            "targets base:U1, main:U10: multi-MCU simulation is Phase 3 (ADR-013 D2): a system \
             run has exactly one target across its simulated boards (main, base)"
        ),
        "{t}"
    );
}

#[test]
fn an_ambiguous_bench_name_is_refused() {
    // Gate G3: a leaf name on two simulated boards must be qualified, in
    // every place a bench names a net or an instance.
    for (from, to, want) in [
        (
            "net = \"main:VSYS\"\namps",
            "net = \"VSYS\"\namps",
            "bench [[load]] VSYS: net VSYS is ambiguous: \"base:VSYS\", \"main:VSYS\"",
        ),
        (
            "\"main:GND\", \"main:VSYS\"",
            "\"main:GND\", \"VSYS\"",
            "bench [analog] nets VSYS: net VSYS is ambiguous",
        ),
        (
            "\"base:U1\" = \"suite_compat\"",
            "\"U1\" = \"suite_compat\"",
            "U1 is ambiguous: base:U1 (BQ25180YBGR), main:U1 (RT9080-33GJ5)",
        ),
    ] {
        let (_t, d) = project("ambiguous", same, |s| {
            assert!(s.contains(from), "{from}");
            s.replacen(from, to, 1)
        });
        let o = elaborate(&d, "out");
        let t = text(&o);
        assert!(!o.status.success() && t.contains(want), "{from}: {t}");
    }
}

#[test]
fn a_harness_wire_needs_both_ends_in_scope_or_neither() {
    let (_t, d) = project("half", same, |s| s.replacen("\"main:I2C_SCL\", ", "", 1));
    let o = elaborate(&d, "out");
    let t = text(&o);
    assert!(!o.status.success(), "{t}");
    assert!(
        t.contains(
            "[[connection]] main:J1 - base:J3: pins 9/9: base:/Base board/I2C_SCL is in the \
             analog scope and its mate main:I2C_SCL is not"
        ),
        "{t}"
    );
}

#[test]
fn a_connection_without_a_harness_ties_the_nets() {
    // ADR-013 D2: no harness view, the mated nets are one node: node 0 where
    // the primary board's ground is in the group, else the primary board's
    // net's name. Pin n mates pin n (MP too, both on GND).
    let (_t, d) = project(
        "direct",
        |s| s.replace("model = \"dial-me-spinner/link-harness:wires\"\n", ""),
        same,
    );
    let o = run(&["open", "."], &d);
    assert!(o.status.success(), "{}", text(&o));
    let o = elaborate(&d, "out");
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    let deck = std::fs::read_to_string(d.join("out/deck.cir")).unwrap();
    assert!(!deck.contains("dms_link_harness_wires"), "{deck}");
    assert!(
        !deck.contains("base_GND") && !deck.contains("base_VSYS"),
        "{deck}"
    );
    assert_eq!(
        line(&deck, "Xbase_U1 "),
        "Xbase_U1 main_VBUS main_VSYS base_VBAT 0 TI_BQ25180_PP_PI buvlo=3 buvlo_hys=0.15 \
         glim=1e-5 ilim=0.475 ronbat=0.055 ronin=0.27 sup_gate=0 tbuvlo=6e-5 vbsup=0.03 \
         vsysreg=4.5"
    );
    assert_eq!(
        line(&deck, "I_bench_bat0 "),
        "I_bench_bat0 0 base_VBAT DC {3 / 0.5}"
    );
}
