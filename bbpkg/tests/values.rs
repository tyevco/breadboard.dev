//! ADR-006 gate G3, offline half: the KiCad value parser on every distinct
//! passive value of the dial-me-spinner main board equals the suite's own
//! parse (`spicelib.parse_value` / `parse_tol` at 6854d25), as recorded in
//! tests/fixtures/values/dms-main.json by `scripts/dms_values.py --write`
//! (which `make dms-values` re-derives from the suite when the
//! dial-me-spinner repository is present).

use bbpkg::value::{parse_kicad_value, spice_number, to_decimal, PassiveKind};

#[test]
fn main_board_values_equal_spicelib() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tests/fixtures/values/dms-main.json"
    );
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let values = doc["values"].as_array().unwrap();
    assert_eq!(
        values.len(),
        29,
        "ADR-006 D5: 29 distinct (kind, value) pairs"
    );
    for v in values {
        let kind = match v["kind"].as_str().unwrap() {
            "R" => PassiveKind::Resistor,
            "C" => PassiveKind::Capacitor,
            "L" => PassiveKind::Inductor,
            k => panic!("kind {k}"),
        };
        let text = v["text"].as_str().unwrap();
        let got = parse_kicad_value(text, kind).unwrap_or_else(|e| panic!("{text}: {e}"));
        let want = v["subunits"].as_i64().unwrap() as i128;
        assert_eq!(got.value, want, "{text}");
        if let Some(t) = v["tolerance_ppm"].as_i64() {
            assert_eq!(got.tolerance_ppm, Some(t as i128), "{text}");
        }
        let s = spice_number(to_decimal(got.value, kind.dim()));
        assert!(
            !s.chars().any(|c| c.is_ascii_alphabetic() && c != 'e'),
            "{text}: {s}"
        );
    }
    // M is mega in KiCad values (1M, 2.2M on the main board), never milli.
    assert_eq!(
        parse_kicad_value("1M", PassiveKind::Resistor)
            .unwrap()
            .value,
        1_000_000_000_000
    );
    assert_eq!(
        parse_kicad_value("2.2M", PassiveKind::Resistor)
            .unwrap()
            .value,
        2_200_000_000_000
    );
}
