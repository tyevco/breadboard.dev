#!/usr/bin/env python3
"""Write examples/nrf-idd/nrf-idd.net, the board of Phase 2 milestone P3
(ADR-009 D4: the nRF52840's supply_load and its CPU state outputs).

    python3 examples/nrf-idd/make_netlist.py          # rewrite nrf-idd.net
    python3 examples/nrf-idd/make_netlist.py --check  # require it byte for byte

A KiCad netlist in the S-expression form `kicad-cli sch export netlist`
writes (version "E"), written by hand rather than drawn, as
examples/bench/make_netlist.py does. U1's symbol pins are the package's
(nordic/nrf52840-qiaa pins.toml, ADR-007 D3).

The circuit:
- the supply: VSUP (a bench [[supply]]) -> R1 10 Ohm -> VDD, C1 4.7 uF on VDD
  (the PS's typical CVDD). U1's VDD and VDDH pins are on VDD (normal voltage
  mode, VDDH shorted to VDD); VSS and VSS_PA on GND. U1's current is
  (V(VSUP) - V(VDD)) / R1: at pi the supply_load view `idd` draws it;
- examples/nrf-feedback's loop: U1.P0.13 (DRV) -> R2 1k -> SENSE, C2 1 uF
  to GND, SENSE -> U1.P0.11 (digital_in); U1.P0.15 (COPY) -> R3 330 Ohm ->
  LED_B -> R4 1k to GND (a resistor where examples/nrf-feedback has its red
  LED, so every part has a first-party package).
"""
import pathlib
import sys
import tomllib

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parent.parent
OUT = HERE / "nrf-idd.net"

MCU_PINS = tomllib.loads((REPO / "packages/nordic/nrf52840-qiaa/pins.toml").read_text())["pin"]

# ref, value, footprint, lib, part, extra fields, {pin number: net}
COMPONENTS = [
    ("C1", "4.7u", "Capacitor_SMD:C_0603_1608Metric", "Device", "C", {}, {"1": "VDD", "2": "GND"}),
    ("C2", "1u", "Capacitor_SMD:C_0603_1608Metric", "Device", "C", {}, {"1": "SENSE", "2": "GND"}),
    ("R1", "10", "Resistor_SMD:R_0603_1608Metric", "Device", "R", {}, {"1": "VSUP", "2": "VDD"}),
    ("R2", "1k", "Resistor_SMD:R_0603_1608Metric", "Device", "R", {}, {"1": "DRV", "2": "SENSE"}),
    ("R3", "330", "Resistor_SMD:R_0603_1608Metric", "Device", "R", {}, {"1": "COPY", "2": "LED_B"}),
    ("R4", "1k", "Resistor_SMD:R_0603_1608Metric", "Device", "R", {}, {"1": "LED_B", "2": "GND"}),
]

U1_NETS = {"P0.13": "DRV", "P0.11": "SENSE", "P0.15": "COPY", "VDD": "VDD", "VDDH": "VDD"}


def q(s):
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


def build():
    nodes = {}  # net -> [(ref, pin)]
    comps = []
    u1 = ["    (comp (ref \"U1\") (value \"nRF52840\") (footprint \"Package_DFN_QFN:Nordic_AQFN-73-1EP_7x7mm_P0.5mm\")",
          "      (fields (field (name \"MPN\") \"NRF52840-QIAA\"))",
          "      (libsource (lib \"MCU_Nordic\") (part \"nRF52840\"))",
          "      (sheetpath (names \"/\") (tstamps \"/\")))"]
    comps.append("\n".join(u1))
    for p in MCU_PINS:
        name, role = p["name"], p["role"]
        net = "GND" if role == "ground" else U1_NETS.get(name)
        if net:
            nodes.setdefault(net, []).append(("U1", p["number"]))
    for ref, value, fp, lib, part, fields, pins in COMPONENTS:
        f = ""
        if fields:
            f = "\n      (fields " + " ".join(f"(field (name {q(k)}) {q(v)})" for k, v in fields.items()) + ")"
        comps.append(
            f"    (comp (ref {q(ref)}) (value {q(value)}) (footprint {q(fp)}){f}\n"
            f"      (libsource (lib {q(lib)}) (part {q(part)}))\n"
            f"      (sheetpath (names \"/\") (tstamps \"/\")))"
        )
        for n, net in pins.items():
            nodes.setdefault(net, []).append((ref, n))
    u1_pins = " ".join(
        f"(pin (num {q(p['number'])}) (name {q(p['name'])}) (type \"passive\"))" for p in MCU_PINS
    )
    libparts = [
        f"    (libpart (lib \"MCU_Nordic\") (part \"nRF52840\")\n      (pins {u1_pins}))",
        "    (libpart (lib \"Device\") (part \"R\") (pins (pin (num \"1\") (name \"~\") (type \"passive\")) (pin (num \"2\") (name \"~\") (type \"passive\"))))",
        "    (libpart (lib \"Device\") (part \"C\") (pins (pin (num \"1\") (name \"~\") (type \"passive\")) (pin (num \"2\") (name \"~\") (type \"passive\"))))",
    ]

    def pin_key(x):
        return (x[0], x[1])

    nets = []
    for code, net in enumerate(sorted(nodes), start=1):
        ns = " ".join(f"(node (ref {q(r)}) (pin {q(p)}))" for r, p in sorted(nodes[net], key=pin_key))
        nets.append(f"    (net (code {q(str(code))}) (name {q(net)}) {ns})")
    return (
        "(export (version \"E\")\n"
        "  (design (source \"nrf-idd.kicad_sch\") (date \"2026-11-16T00:00:00\") (tool \"Eeschema 10.0.8\"))\n"
        "  (components\n" + "\n".join(comps) + ")\n"
        "  (libparts\n" + "\n".join(libparts) + ")\n"
        "  (nets\n" + "\n".join(nets) + "))\n"
    )


def main():
    text = build()
    if "--check" in sys.argv[1:]:
        if OUT.read_text() != text:
            sys.exit(f"{OUT} is not what make_netlist.py writes")
        print(f"{OUT.relative_to(REPO)}: current")
        return
    OUT.write_text(text)
    print(f"wrote {OUT.relative_to(REPO)}")


if __name__ == "__main__":
    main()
