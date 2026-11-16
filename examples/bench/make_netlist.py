#!/usr/bin/env python3
"""Write examples/bench/bench.net, the synthetic board of milestone B1
(ADR-010 D2-D4; Phase 2 plan section 4, track B).

    python3 examples/bench/make_netlist.py          # rewrite bench.net
    python3 examples/bench/make_netlist.py --check  # require it byte for byte

A KiCad netlist in the S-expression form `kicad-cli sch export netlist`
writes (version "E"), written by hand rather than drawn: the board exists to
exercise the bench, not a schematic. U1's symbol pins are the package's
(st/stm32f103c8 pins.toml, ADR-007 D3).

The circuit:
- U1 (STM32F103C8T6) runs examples/feedback's firmware: SysTick toggles PA5
  every 50 ms (net TRIG, R5 10k to GND); the EXTI0 ISR copies PB0 (net BTN)
  to PA6 (net LED_DRV, R2 1k to GND).
- BTN: R1 10k pull-up to +3V3, C1 100n to GND (tau 1 ms), SW1 a pushbutton
  (example/pushbutton: the switch view kind, 50 mOhm / 100 MOhm) to GND.
- AIN -> R3 1k -> AFILT, C2 1u to GND: an RC that a playback drives.
- VSW -> SW2 (example/switch-1k: a 1 kOhm contact, for the analytic RC
  settling of ADR-010 gate G8) -> RC_N, C3 1u and R4 1Meg to GND.
"""
import pathlib
import sys
import tomllib

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parent.parent
OUT = HERE / "bench.net"

MCU_PINS = tomllib.loads((REPO / "packages/st/stm32f103c8/pins.toml").read_text())["pin"]

# ref, value, footprint, lib, part, extra fields, {pin number: net}
COMPONENTS = [
    ("C1", "100n", "Capacitor_SMD:C_0603_1608Metric", "Device", "C", {}, {"1": "BTN", "2": "GND"}),
    ("C2", "1u", "Capacitor_SMD:C_0603_1608Metric", "Device", "C", {}, {"1": "AFILT", "2": "GND"}),
    ("C3", "1u", "Capacitor_SMD:C_0603_1608Metric", "Device", "C", {}, {"1": "RC_N", "2": "GND"}),
    ("R1", "10k", "Resistor_SMD:R_0603_1608Metric", "Device", "R", {}, {"1": "+3V3", "2": "BTN"}),
    ("R2", "1k", "Resistor_SMD:R_0603_1608Metric", "Device", "R", {}, {"1": "LED_DRV", "2": "GND"}),
    ("R3", "1k", "Resistor_SMD:R_0603_1608Metric", "Device", "R", {}, {"1": "AIN", "2": "AFILT"}),
    ("R4", "1Meg", "Resistor_SMD:R_0603_1608Metric", "Device", "R", {}, {"1": "RC_N", "2": "GND"}),
    ("R5", "10k", "Resistor_SMD:R_0603_1608Metric", "Device", "R", {}, {"1": "TRIG", "2": "GND"}),
    ("SW1", "SW_Push", "Button_Switch_SMD:SW_SPST_PTS645", "Switch", "SW_Push",
     {"bb:part": "example/pushbutton"}, {"1": "BTN", "2": "GND"}),
    ("SW2", "SW_Push", "Button_Switch_SMD:SW_SPST_PTS645", "Switch", "SW_Push",
     {"bb:part": "example/switch-1k"}, {"1": "VSW", "2": "RC_N"}),
]

U1_NETS = {"PA5": "TRIG", "PA6": "LED_DRV", "PB0": "BTN"}


def q(s):
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


def build():
    nodes = {}  # net -> [(ref, pin)]
    comps = []
    u1 = ["    (comp (ref \"U1\") (value \"STM32F103C8T6\") (footprint \"Package_QFP:LQFP-48_7x7mm_P0.5mm\")",
          "      (fields (field (name \"MPN\") \"STM32F103C8T6\"))",
          "      (libsource (lib \"MCU_ST_STM32F1\") (part \"STM32F103C8Tx\"))",
          "      (sheetpath (names \"/\") (tstamps \"/\")))"]
    comps.append("\n".join(u1))
    for p in MCU_PINS:
        name, role = p["name"], p["role"]
        if role == "power_in":
            net = "+3V3"
        elif role == "ground":
            net = "GND"
        else:
            net = U1_NETS.get(name)
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
        f"    (libpart (lib \"MCU_ST_STM32F1\") (part \"STM32F103C8Tx\")\n      (pins {u1_pins}))",
        "    (libpart (lib \"Device\") (part \"R\") (pins (pin (num \"1\") (name \"~\") (type \"passive\")) (pin (num \"2\") (name \"~\") (type \"passive\"))))",
        "    (libpart (lib \"Device\") (part \"C\") (pins (pin (num \"1\") (name \"~\") (type \"passive\")) (pin (num \"2\") (name \"~\") (type \"passive\"))))",
        "    (libpart (lib \"Switch\") (part \"SW_Push\") (pins (pin (num \"1\") (name \"1\") (type \"passive\")) (pin (num \"2\") (name \"2\") (type \"passive\"))))",
    ]
    nets = []
    for code, net in enumerate(sorted(nodes), start=1):
        ns = " ".join(f"(node (ref {q(r)}) (pin {q(p)}))" for r, p in sorted(nodes[net], key=lambda x: (x[0], int(x[1]))))
        nets.append(f"    (net (code {q(str(code))}) (name {q(net)}) {ns})")
    return (
        "(export (version \"E\")\n"
        "  (design (source \"bench.kicad_sch\") (date \"2026-11-15T00:00:00\") (tool \"Eeschema 10.0.8\"))\n"
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
