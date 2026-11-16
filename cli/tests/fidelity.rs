//! Phase 2 P1 (ADR-009 D1, D8; ADR-010 D2): the fidelity dial, corners and
//! the battery element, through `bb run --elaborate-only` on a copy of the
//! dial-me-spinner main board project. No Renode and no ngspice run.
//!
//! At `pi` the RT9080 (U1, a `pi`-only view) enters the deck at the bench's
//! per-instance corner and drives +3V3; at `fast` (the command line over the
//! bench's `fidelity = "pi"`) the ladder finds no view for it and it stays
//! out, so the deck is the Phase 1 one. The bench's cell is the battery
//! element: its OCV at the state of charge as a Norton current behind R0.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use bbcore::cjson::Json;

fn bb() -> &'static str {
    env!("CARGO_BIN_EXE_bb")
}

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

struct Tmp(PathBuf);
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const BENCH: &str = r#"schema = "bb-bench/1"
fidelity = "pi"
corner = "typ"

[corners]
"main:U1" = "worst_dropout"

[[battery]]
net = "VSYS"
ocv = "cell.csv"
soc = "0.25"
r0 = "250m"
rc = [["50m", "2"]]

[[load]]
net = "+3V3"
amps = "55mA"

[nominal]
"+3V3" = "3.3V"
"#;

/// The bench, with E1's analog scope (bench/cold.toml's `[analog]`, which
/// the project's probes need).
fn bench(src: &Path) -> String {
    let cold = std::fs::read_to_string(src.join("bench/cold.toml")).unwrap();
    let a = cold.find("[analog]").expect("[analog]");
    let o = cold.find("[options]").expect("[options]");
    format!("{BENCH}\n{}", &cold[a..o])
}

/// A copy of examples/dial-me-spinner (the main board project) beside a
/// `packages` link, with the P1 bench.
fn project(name: &str) -> (Tmp, PathBuf) {
    let root = std::env::temp_dir().join(format!("bb-fidelity-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let d = root.join("examples/dial-me-spinner");
    std::fs::create_dir_all(d.join("kicad/main")).unwrap();
    std::fs::create_dir_all(d.join("firmware/prebuilt")).unwrap();
    std::fs::create_dir_all(d.join("bench")).unwrap();
    std::os::unix::fs::symlink(repo().join("packages"), root.join("packages")).unwrap();
    let src = repo().join("examples/dial-me-spinner");
    for f in ["bb.toml", "board.bbm", "bb.lock"] {
        std::fs::copy(src.join(f), d.join(f)).unwrap();
    }
    for f in [
        "kicad/main/dial-me-spinner-main.net",
        "firmware/prebuilt/led_rail.elf",
    ] {
        std::fs::copy(src.join(f), d.join(f)).unwrap();
    }
    std::fs::write(d.join("bench/p1.toml"), bench(&src)).unwrap();
    std::fs::write(
        d.join("bench/cell.csv"),
        "soc,volts\n0,3.0V\n0.5,3.7V\n1,4.2V\n",
    )
    .unwrap();
    (Tmp(root), d)
}

fn run(args: &[&str], dir: &Path) -> Output {
    Command::new(bb())
        .args(args)
        .current_dir(dir)
        .env(
            "BB_FIRST_PARTY_PACKAGES",
            repo().join("packages").to_string_lossy().to_string(),
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

fn analog<'a>(rj: &'a Json, r: &str) -> Option<&'a Json> {
    rj.get("analog")
        .and_then(Json::as_arr)
        .unwrap_or(&[])
        .iter()
        .find(|a| a.get("ref").and_then(Json::as_str) == Some(r))
}

fn s<'a>(j: &'a Json, k: &str) -> &'a str {
    j.get(k).and_then(Json::as_str).unwrap_or_default()
}

#[test]
fn the_fidelity_dial_corners_and_the_battery() {
    let (_t, d) = project("pi");
    let elab = |extra: &[&str], out: &str| {
        let mut a = vec![
            "run",
            "--elaborate-only",
            "--for",
            "1ms",
            "--bench",
            "bench/p1.toml",
            "--out",
            out,
            "--allow-unqualified",
            "nordic/nrf52840-qiaa:saadc_se",
        ];
        a.extend_from_slice(extra);
        let o = run(&a, &d);
        assert!(o.status.success(), "{}", text(&o));
        let deck = std::fs::read_to_string(d.join(out).join("deck.cir")).unwrap();
        let rj =
            Json::parse(&std::fs::read_to_string(d.join(out).join("run.json")).unwrap()).unwrap();
        (deck, rj, text(&o))
    };

    // The bench's rung: pi. U1 at its per-instance corner, the generic
    // passives at pi (their own rung), the bench's global corner `typ`
    // nowhere else (U1 has its own): no other view declares it.
    let (deck, rj, _) = elab(&[], "out-pi");
    assert!(deck.contains("fidelity pi"), "{deck}");
    let u1 = deck
        .lines()
        .find(|l| l.starts_with("XU1 "))
        .unwrap_or_else(|| panic!("no XU1 in the deck:\n{deck}"));
    assert!(
        u1.contains("RICHTEK_RT9080_LDO_PI")
            && u1.contains("rdrop=0.883333")
            && u1.contains("gm=18.18"),
        "{u1}"
    );
    let a = analog(&rj, "main:U1").expect("U1 in run.json's analog list");
    assert_eq!((s(a, "rung"), s(a, "corner")), ("pi", "worst_dropout"));
    let c = analog(&rj, "main:C1").expect("C1");
    assert_eq!((s(c, "rung"), s(c, "corner")), ("pi", ""));
    // The battery: Norton current OCV/R0 (3.35 V at soc 0.25) and R0, then
    // the RC pair into the net.
    assert!(
        deck.contains("I_bench_bat0 0 bb_bat0_0 DC {3.35 / 0.25}"),
        "{deck}"
    );
    assert!(deck.contains("R_bench_bat0_0 bb_bat0_0 0 0.25"), "{deck}");
    assert!(
        deck.contains("R_bench_bat0_1 bb_bat0_0 VSYS 0.05"),
        "{deck}"
    );
    assert!(deck.contains("C_bench_bat0_1 bb_bat0_0 VSYS 2"), "{deck}");
    assert_eq!(
        rj.get("options")
            .and_then(|o| o.get("fidelity"))
            .and_then(Json::as_str),
        None,
        "an elaborate-only run.json has no run options"
    );

    // The command line wins: --fidelity fast keeps U1 out (no view at fast or
    // below), so +3V3 has no driver but the bench's [nominal].
    let (deck, rj, _) = elab(&["--fidelity", "fast"], "out-fast");
    assert!(deck.contains("fidelity fast"), "{deck}");
    assert!(!deck.contains("RT9080"), "{deck}");
    assert!(analog(&rj, "main:U1").is_none());
    let oos = rj.get("out_of_scope").and_then(Json::as_arr).unwrap_or(&[]);
    assert!(
        oos.iter().any(|x| s(x, "ref") == "main:U1"
            && s(x, "reason").contains("no analog view at fast or below")),
        "{oos:?}"
    );

    // A per-instance corner the view does not have is refused; si is Phase 3's.
    std::fs::write(
        d.join("bench/p1.toml"),
        bench(&repo().join("examples/dial-me-spinner")).replace("\"worst_dropout\"", "\"worst_x\""),
    )
    .unwrap();
    let o = run(
        &[
            "run",
            "--elaborate-only",
            "--for",
            "1ms",
            "--bench",
            "bench/p1.toml",
            "--out",
            "o3",
        ],
        &d,
    );
    assert!(!o.status.success());
    assert!(
        text(&o).contains("has no corner \"worst_x\""),
        "{}",
        text(&o)
    );
    let o = run(
        &[
            "run",
            "--elaborate-only",
            "--for",
            "1ms",
            "--fidelity",
            "si",
            "--out",
            "o4",
        ],
        &d,
    );
    assert!(!o.status.success());
    assert!(text(&o).contains("Phase 3"), "{}", text(&o));
}
