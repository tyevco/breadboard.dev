//! Phase 1 M11 (ADR-007 D5, D7 and Amendment 5): the dial-me-spinner system,
//! `examples/dial-me-spinner/system`, imported from the netlists vendored at
//! 6854d25. The base board is 21 parts and 17 nets; the shared references
//! U1, U2, J1 and TP1 resolve by qualified name, and a bare one is an error
//! that lists the candidates; `[[connection]]` mates main J1 with base J3
//! through the harness view. No KiCad and no Renode are needed.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use bbcore::cjson::Json;

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

/// A copy of the system project (with the two vendored netlists at the
/// relative paths its bb.toml names), `edit` applied to its bb.toml. It sits
/// at `<tmp>/examples/dial-me-spinner/system` beside a `<tmp>/packages` that
/// links to the first-party directory, so every bb.lock `path` still points
/// where its package resolves (no ADR-015 D7 warning: the same directory,
/// spelled through the link).
fn project(name: &str, edit: impl Fn(&str) -> String) -> (Tmp, PathBuf) {
    let root = std::env::temp_dir().join(format!("bb-system-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::os::unix::fs::symlink(repo().join("packages"), root.join("packages")).unwrap();
    let d = root.join("examples/dial-me-spinner");
    let sys = d.join("system");
    copy_dir(&dms().join("system"), &sys);
    for (b, f) in [
        ("main", "dial-me-spinner-main.net"),
        ("base", "dial-me-spinner-base.net"),
    ] {
        let to = d.join("kicad").join(b);
        std::fs::create_dir_all(&to).unwrap();
        std::fs::copy(dms().join("kicad").join(b).join(f), to.join(f)).unwrap();
    }
    let t = sys.join("bb.toml");
    let s = std::fs::read_to_string(&t).unwrap();
    std::fs::write(&t, edit(&s)).unwrap();
    (Tmp(root), sys)
}

fn append(extra: &'static str) -> impl Fn(&str) -> String {
    move |s: &str| format!("{s}{extra}")
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

fn bbm(d: &Path) -> Json {
    Json::parse(&std::fs::read_to_string(d.join("board.bbm")).unwrap()).unwrap()
}

fn arr<'a>(j: &'a Json, k: &str) -> &'a [Json] {
    j.get(k).and_then(Json::as_arr).unwrap_or(&[])
}

fn st<'a>(j: &'a Json, k: &str) -> &'a str {
    j.get(k).and_then(Json::as_str).unwrap_or_default()
}

fn instance<'a>(b: &'a Json, board: &str, r: &str) -> &'a Json {
    arr(b, "instances")
        .iter()
        .find(|i| st(i, "board") == board && st(i, "ref") == r)
        .unwrap_or_else(|| panic!("no instance {board}:{r}"))
}

/// `<r> is ambiguous: ` followed by both candidates of `want` (`x, y`), in
/// either order: `bb open` lists them in bb.toml's board order, a command
/// that reads board.bbm in its sorted order.
fn ambiguous(t: &str, r: &str, want: &str) -> bool {
    let (x, y) = want.split_once(", ").expect("two candidates");
    t.contains(&format!("{r} is ambiguous: {x}, {y}"))
        || t.contains(&format!("{r} is ambiguous: {y}, {x}"))
}

#[test]
fn the_mated_system_imports() {
    let (_t, d) = project("import", |s| s.to_string());
    let o = run(&["open", "."], &d);
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    assert!(!t.contains("warning"), "{t}");
    // board.bbm is a function of content: the copy gives the committed bytes.
    assert_eq!(
        std::fs::read(d.join("board.bbm")).unwrap(),
        std::fs::read(dms().join("system/board.bbm")).unwrap(),
        "the committed examples/dial-me-spinner/system/board.bbm is not current (run `bb open`)"
    );
    let b = bbm(&d);
    // Plan M11: the base board is 21 parts and 17 nets (81 pin nodes,
    // ADR-007 Context 12); the main board as at M4b.
    for (board, parts, nets, nodes) in [("main", 201, 142, 657), ("base", 21, 17, 81)] {
        let ins = arr(&b, "instances")
            .iter()
            .filter(|i| st(i, "board") == board)
            .count();
        let ns: Vec<&Json> = arr(&b, "nets")
            .iter()
            .filter(|n| st(n, "board") == board)
            .collect();
        let nn: usize = ns.iter().map(|n| arr(n, "nodes").len()).sum();
        assert_eq!((ins, ns.len(), nn), (parts, nets, nodes), "board {board}");
    }
    assert_eq!(
        arr(&b, "boards")
            .iter()
            .map(|x| (st(x, "id"), st(x, "ground")))
            .collect::<Vec<_>>(),
        [("base", "base:GND"), ("main", "main:GND")],
        "each board keeps its own ground"
    );
    // The 15 shared references (ADR-007 Context 10).
    let refs = |board: &str| -> std::collections::BTreeSet<String> {
        arr(&b, "instances")
            .iter()
            .filter(|i| st(i, "board") == board)
            .map(|i| st(i, "ref").to_string())
            .collect()
    };
    let shared: Vec<String> = refs("main").intersection(&refs("base")).cloned().collect();
    assert_eq!(
        shared,
        [
            "C1", "C2", "C3", "C4", "J1", "R1", "R2", "R3", "R4", "TP1", "TP2", "TP3", "TP4", "U1",
            "U2"
        ]
    );
    for (board, r, value) in [
        ("main", "U1", "RT9080-33GJ5"),
        ("base", "U1", "BQ25180YBGR"),
        ("main", "U2", "TPS22917DBVR"),
        ("base", "U2", "USBLC6-2SC6"),
        ("main", "J1", "LINK"),
        ("base", "J1", "TYPE-C-31-M-12"),
        ("main", "TP1", "VSYS"),
        ("base", "TP1", "VBUS"),
    ] {
        assert_eq!(
            st(instance(&b, board, r), "value_text"),
            value,
            "{board}:{r}"
        );
    }
    // The connection: main J1 to base J3 through the harness, pins 1-12
    // (not the mount pins), in bytewise order.
    let c = arr(&b, "connections");
    assert_eq!(c.len(), 1);
    assert_eq!(
        (st(&c[0], "a"), st(&c[0], "b"), st(&c[0], "model")),
        (
            "main:J1",
            "base:J3",
            "dial-me-spinner/link-harness@0.1.0:wires"
        )
    );
    let pins: Vec<(String, String)> = arr(&c[0], "pins")
        .iter()
        .map(|p| {
            let a = p.as_arr().unwrap();
            (
                a[0].as_str().unwrap().to_string(),
                a[1].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let mut want: Vec<(String, String)> =
        (1..=12).map(|n| (n.to_string(), n.to_string())).collect();
    want.sort();
    assert_eq!(pins, want);
    for l in [
        "connection main:J1 - base:J3 through dial-me-spinner/link-harness@0.1.0:wires: 12 mated pins",
        "main:J1.1 main:VSYS = base:J3.1 base:VSYS",
        "main:J1.3 main:GND = base:J3.3 base:GND",
        "main:J1.9 main:I2C_SCL = base:J3.9 base:/Base board/I2C_SCL",
    ] {
        assert!(t.contains(l), "{l} not in {t}");
    }
    // The harness is locked like an instance's package.
    let lock = std::fs::read_to_string(d.join("bb.lock")).unwrap();
    assert!(
        lock.contains("id = \"dial-me-spinner/link-harness\""),
        "{lock}"
    );
    // Probe aliases are qualified: TP1 is VSYS on main and VBUS on base.
    let probes: Vec<(&str, &str)> = arr(&b, "probes")
        .iter()
        .map(|p| (st(p, "alias"), st(p, "net")))
        .collect();
    assert!(probes.contains(&("main:TP1", "main:VSYS")), "{probes:?}");
    assert!(probes.contains(&("base:TP1", "base:VBUS")), "{probes:?}");
    // Two imports give the same bytes (M4d), and --check agrees.
    let files = [
        "board.bbm",
        "bb.lock",
        ".bb/resolution.json",
        ".bb/unresolved.toml",
    ];
    let before: Vec<Vec<u8>> = files
        .iter()
        .map(|f| std::fs::read(d.join(f)).unwrap())
        .collect();
    assert!(run(&["open", "."], &d).status.success());
    for (f, b) in files.iter().zip(before) {
        assert_eq!(std::fs::read(d.join(f)).unwrap(), b, "{f}");
    }
    let o = run(&["open", ".", "--check"], &d);
    assert!(o.status.success(), "{}", text(&o));
}

#[test]
fn shared_refs_resolve_by_qualified_name() {
    let (_t, d) = project(
        "qualified",
        append(
            "\n[instances.\"base:U1\"]\npackage = \"none\"\nreason = \"BQ25180 charger: the base board is not simulated\"\n\
             \n[instances.\"base:U2\"]\npackage = \"none\"\nreason = \"USBLC6 ESD array: the base board is not simulated\"\n\
             \n[instances.\"main:U2\"]\nanalog = \"lsw\"\n\
             \n[instances.\"base:J1\"]\npackage = \"none\"\nreason = \"USB-C receptacle: no model\"\n\
             \n[instances.\"main:TP1\"]\npackage = \"none\"\nreason = \"a test point\"\n",
        ),
    );
    let o = run(&["open", "."], &d);
    assert!(o.status.success(), "{}", text(&o));
    let b = bbm(&d);
    for (board, r, status, reason) in [
        (
            "base",
            "U1",
            "excluded",
            "BQ25180 charger: the base board is not simulated",
        ),
        (
            "base",
            "U2",
            "excluded",
            "USBLC6 ESD array: the base board is not simulated",
        ),
        ("base", "J1", "excluded", "USB-C receptacle: no model"),
        ("main", "TP1", "excluded", "a test point"),
        // The other board's namesakes are untouched.
        ("main", "U1", "unmodelled", ""),
        ("main", "U2", "ok", ""),
        ("main", "J1", "unmodelled", ""),
        ("base", "TP1", "no_model", ""),
    ] {
        let i = instance(&b, board, r);
        assert_eq!(
            (st(i, "status"), st(i, "reason")),
            (status, reason),
            "{board}:{r}"
        );
    }
    // Probes and test-point aliases by qualified name, and leaf net names:
    // within the simulated board first (VSYS is on both), else the project.
    for (n, want) in [
        ("main:TP1", "main:VSYS"),
        ("base:TP1", "base:VBUS"),
        ("VSYS", "main:VSYS"),
        ("base:VSYS", "base:VSYS"),
        ("I2C_SDA", "main:I2C_SDA"),
        ("base:/Base board/I2C_SDA", "base:/Base board/I2C_SDA"),
        ("CHG_TSMR", "base:/Base board/CHG_TSMR"),
    ] {
        let o = run(&["probe", &format!("net:{n}")], &d);
        assert!(o.status.success(), "{n}: {}", text(&o));
        assert!(
            text(&o).contains(&format!("bb probe: {n} -> {want}\n")),
            "{n}: {}",
            text(&o)
        );
    }
    // bb flash resolves a qualified target (and then refuses an instance
    // with no behavioural layer, which is a different error).
    std::fs::write(d.join("x.elf"), b"\x7fELF").unwrap();
    for t in ["main:U1", "base:U1"] {
        let o = run(&["flash", "--target", t, "x.elf"], &d);
        let msg = text(&o);
        assert!(!o.status.success(), "{t}: {msg}");
        assert!(
            msg.contains(&format!("{t} has no package ("))
                && msg.contains("it cannot run firmware"),
            "{t}: {msg}"
        );
    }
}

#[test]
fn a_bare_shared_ref_is_an_error_listing_the_candidates() {
    let cands = [
        ("U1", "main:U1 (RT9080-33GJ5), base:U1 (BQ25180YBGR)"),
        ("U2", "main:U2 (TPS22917DBVR), base:U2 (USBLC6-2SC6)"),
        ("J1", "main:J1 (LINK), base:J1 (TYPE-C-31-M-12)"),
        ("TP1", "main:TP1 (VSYS), base:TP1 (VBUS)"),
    ];
    // [instances]: bb open refuses, naming both.
    for (r, want) in cands {
        let extra = format!("\n[instances.\"{r}\"]\npackage = \"none\"\nreason = \"x\"\n");
        let (_t, d) = project(&format!("inst-{r}"), move |s| format!("{s}{extra}"));
        let o = run(&["open", "."], &d);
        let t = text(&o);
        assert!(!o.status.success(), "{r}: {t}");
        assert!(ambiguous(&t, r, want), "{r}: {t}");
        assert_eq!(
            std::fs::read(d.join("board.bbm")).unwrap(),
            std::fs::read(dms().join("system/board.bbm")).unwrap(),
            "{r}: board.bbm rewritten"
        );
    }
    // [[connection]]: a bare J1 is ambiguous.
    let (_t, d) = project("conn", |s| s.replace("a = \"main:J1\"", "a = \"J1\""));
    let o = run(&["open", "."], &d);
    let t = text(&o);
    assert!(!o.status.success(), "{t}");
    assert!(
        t.contains("[[connection]]") && ambiguous(&t, "J1", cands[2].1),
        "{t}"
    );
    // [[bind]] and [targets]: bb open writes the board but fails, naming both.
    let (_t, d) = project(
        "bind",
        append("\n[[bind]]\npin = \"U2.ON\"\nmodel = \"digital_out\"\n"),
    );
    let o = run(&["open", "."], &d);
    let t = text(&o);
    assert!(!o.status.success(), "{t}");
    assert!(ambiguous(&t, "U2", cands[1].1), "{t}");
    // A probe alias, on the command line and in [probes].
    let (_t, d) = project("probe", |s| s.to_string());
    assert!(run(&["open", "."], &d).status.success());
    let o = run(&["probe", "net:TP1"], &d);
    let t = text(&o);
    assert!(
        !o.status.success() && t.contains("TP1 is ambiguous: base:TP1, main:TP1"),
        "{t}"
    );
    let (_t, d) = project("probes", |s| s.replace("\"main:TP1\", ", "\"TP1\", "));
    let o = run(&["open", "."], &d);
    let t = text(&o);
    assert!(
        !o.status.success() && t.contains("TP1 is ambiguous: base:TP1, main:TP1"),
        "{t}"
    );
    // bb flash --target.
    let (_t, d) = project("flash", |s| s.to_string());
    assert!(run(&["open", "."], &d).status.success());
    std::fs::write(d.join("x.elf"), b"\x7fELF").unwrap();
    let o = run(&["flash", "--target", "U1", "x.elf"], &d);
    let t = text(&o);
    assert!(
        !o.status.success() && ambiguous(&t, "U1", cands[0].1),
        "{t}"
    );
}

#[test]
fn a_connection_is_checked() {
    let conn = "[[connection]]\na = \"main:J1\"\nb = \"base:J3\"\nmodel = \"dial-me-spinner/link-harness:wires\"\n";
    let cases: [(&str, String, &str); 9] = [
        (
            "noview",
            conn.replace(":wires", ":nope"),
            "package dial-me-spinner/link-harness has no view \"nope\"",
        ),
        (
            "kind",
            conn.replace("dial-me-spinner/link-harness:wires", "generic/resistor"),
            "package generic/resistor is of kind passive; a harness view's package is a connector",
        ),
        (
            "oneboard",
            conn.replace("base:J3", "main:J10"),
            "both ends are on board main",
        ),
        (
            "twice",
            format!("{conn}pins = [[\"1\", \"1\"], [\"1\", \"2\"]]\n"),
            "main:J1 pin 1 is mated twice",
        ),
        (
            "crossed",
            format!("{conn}pins = [[\"1\", \"2\"], [\"2\", \"1\"]]\n"),
            "harness dial-me-spinner/link-harness:wires's ports must be exactly A<pin> for main:J1's mated pins",
        ),
        (
            "nopin",
            format!("{conn}pins = [[\"13\", \"13\"]]\n"),
            "main:J1 has no pin 13",
        ),
        // An explicit empty list is not "pins left out" (the M10 review).
        (
            "empty",
            format!("{conn}pins = []\n"),
            "pins = []: a connection mates at least one pin",
        ),
        (
            "empty-direct",
            format!(
                "{}pins = []\n",
                conn.replace("model = \"dial-me-spinner/link-harness:wires\"\n", "")
            ),
            "pins = []: a connection mates at least one pin",
        ),
        // A pin is mated at most once across the project's connections too.
        (
            "duplicate",
            format!("{conn}\n{conn}"),
            "main:J1 pin 1 is already mated by [[connection]] 1",
        ),
    ];
    for (name, new, want) in cases {
        let (_t, d) = project(&format!("conn-{name}"), move |s| s.replace(conn, &new));
        let o = run(&["open", "."], &d);
        let t = text(&o);
        assert!(!o.status.success(), "{name}: {t}");
        assert!(t.contains(want), "{name}: {want} not in {t}");
    }
    // Without a harness view the connectors mate pin n to pin n directly,
    // the mount pins too (both J1 and J3 have 1-12 and MP).
    let (_t, d) = project("direct", |s| {
        s.replace("model = \"dial-me-spinner/link-harness:wires\"\n", "")
    });
    let o = run(&["open", "."], &d);
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    assert!(
        t.contains("connection main:J1 - base:J3: 13 mated pins"),
        "{t}"
    );
    assert!(
        t.contains("main:J1.MP main:GND = base:J3.MP base:GND"),
        "{t}"
    );
    assert!(!bbm(&d).to_canonical().contains("link-harness"));
    // A mis-pinned mating is reported: VBUS on VSYS.
    let (_t, d) = project("mispin", |s| {
        s.replace(
            "model = \"dial-me-spinner/link-harness:wires\"\n",
            "pins = [[\"5\", \"1\"]]\n",
        )
    });
    let o = run(&["open", "."], &d);
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    assert!(
        t.contains(
            "main:J1.5 on main:VBUS is mated with base:J3.1 on base:VSYS: the net names differ"
        ),
        "{t}"
    );
}

#[test]
fn only_the_simulated_board_is_elaborated() {
    // E1's bindings, firmware and cold bench on the system: the deck holds
    // main's instances only (qualified), the base side of J1 is a bench
    // port, and a bench net on the base board is refused (ADR-007 D5).
    // The base board's nets are not in the deck, so they cannot be probed:
    // [probes] keeps main's.
    let (_t, d) = project("elab", |s| {
        s.replace(
            "nets = [\"main:TP1\", \"base:TP1\", \"base:TP2\", \"base:TP3\"]",
            "nets = [\"main:TP1\", \"VLED\"]",
        ) + "\n[[bind]]\npin = \"main:U10.P1.04\"\nmodel = \"digital_out\"\nset = \"S0S1\"\nnet = \"LED_PWR_EN\"\n\
             \n[[bind]]\npin = \"main:U10.P0.06\"\nmodel = \"digital_out\"\nset = \"H0H1_max\"\nnet = \"LED_DIN_MCU\"\n"
    });
    std::fs::create_dir_all(d.join("bench")).unwrap();
    std::fs::copy(dms().join("bench/cold.toml"), d.join("bench/cold.toml")).unwrap();
    std::fs::copy(
        dms().join("firmware/prebuilt/led_rail.elf"),
        d.join("led_rail.elf"),
    )
    .unwrap();
    assert!(run(&["open", "."], &d).status.success());
    let o = run(&["flash", "--target", "main:U10", "led_rail.elf"], &d);
    assert!(o.status.success(), "{}", text(&o));
    let o = run(
        &[
            "run",
            "--elaborate-only",
            "--for",
            "1ms",
            "--bench",
            "bench/cold.toml",
            "--out",
            "out",
        ],
        &d,
    );
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    assert!(
        t.contains(
            "[[connection]] main:J1 - base:J3: base:J3 is not simulated (Phase 1 simulates main \
             only, ADR-007 D5), so these nets at main:J1 are left open: main:BTN_N, \
             main:CHG_INT_N, main:I2C_SCL, main:I2C_SDA, main:USB_DM, main:USB_DP, main:VBUS"
        ),
        "{t}"
    );
    let deck = std::fs::read_to_string(d.join("out/deck.cir")).unwrap();
    assert!(deck.contains("\nXmain_U2 "), "{deck}");
    assert!(!deck.contains("Xbase_"), "{deck}");
    assert!(!deck.contains("dms_link_harness_wires"), "{deck}");
    // A bench supply on the base board's VSYS is refused.
    let bench = std::fs::read_to_string(d.join("bench/cold.toml")).unwrap();
    std::fs::write(
        d.join("bench/base.toml"),
        bench.replacen("net = \"VSYS\"", "net = \"base:VSYS\"", 1),
    )
    .unwrap();
    let o = run(
        &[
            "run",
            "--elaborate-only",
            "--for",
            "1ms",
            "--bench",
            "bench/base.toml",
            "--out",
            "out2",
        ],
        &d,
    );
    let t = text(&o);
    assert!(!o.status.success(), "{t}");
    assert!(
        t.contains("base:VSYS is on board base, which is not simulated"),
        "{t}"
    );
    // A bare shared ref in the bench is ambiguous too.
    std::fs::write(
        d.join("bench/excl.toml"),
        bench.replace("[options]", "exclude = [\"U2\"]\n\n[options]"),
    )
    .unwrap();
    let o = run(
        &[
            "run",
            "--elaborate-only",
            "--for",
            "1ms",
            "--bench",
            "bench/excl.toml",
            "--out",
            "out3",
        ],
        &d,
    );
    let t = text(&o);
    assert!(
        !o.status.success() && ambiguous(&t, "U2", "main:U2 (TPS22917DBVR), base:U2 (USBLC6-2SC6)"),
        "{t}"
    );
}
