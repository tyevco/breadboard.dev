"""Phase 1 plan M5b gate (ADR-006 D9 rule 4, Amendment 3): the committed datasheet-fitted TPS22917 view
against the LOCAL-ONLY vendor-derived one, on the E1 checks, through NgspiceEngine at phase0.

Subcommands (scripts/dms_packages.sh m5b drives them):

  decks --src <export root> --view vendor=<lib>:<subckt> --view fit=<lib>:<subckt> --out <dir>
      Build the E1 decks with the suite's own functions at the reference commit (the S1 deck of
      scripts/dms_s1.py: led_rail_sequence with each plan section 2 bench), once per view, for the
      correct-order firmware sequence (two firmware.md 6.4 on/off cycles, 120 ms) and for the wrong-order one
      (P0.06 high while VLED is off). Each view's file is wrapped as the suite's LSW_TPS22917 (a shim
      subckt with the suite's port order and parameters). The decks are TI-derived when the vendor view is
      in them, so <dir> is outside the repository.
  gate --dir <dir> --summary <json>
      Compare the runs: VLED at each P0.06 rise (LED-DATA-V4) and max(V(LED_DIN) - V(VLED)) (LSW-3) within
      10 mV, the LSW-2 time within 1 ms, on both benches; the wrong-order excursion within 10 mV. Exit 1
      unless every gate passes.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import dms_s1  # noqa: E402  (BENCHES, E1_EDGES_US, the deck body rules)

US = dms_s1.US
STOP_US = 120_000
# Correct order (plan section 2, firmware.md 6.4): the S1 sequence.
EDGES_OK = dms_s1.E1_EDGES_US
# Wrong order: P0.06 high while VLED is off, before each turn-on and through each QOD discharge.
EDGES_WRONG = [("n_seq_din", 500, 1), ("n_seq_en", 10_500, 1), ("n_seq_en", 15_500, 0), ("n_seq_din", 25_500, 0),
               ("n_seq_din", 50_600, 1), ("n_seq_en", 60_600, 1), ("n_seq_en", 65_600, 0), ("n_seq_din", 75_600, 0)]
PROBES = ["n_vled", "n_din_over", "n_led_din_mcu", "n_led_din_r"]
R40, R41 = 220.0, 100e3


def shim(lib_text: str, subckt: str) -> str:
    """The view as the suite's LSW_TPS22917 (port order CT GND ON QOD VIN VOUT, params ron_add isd_out)."""
    if subckt.upper() == "LSW_TPS22917":
        return lib_text
    return (lib_text + "\n* bb shim: the suite's LSW_TPS22917 interface around the view\n"
            ".subckt LSW_TPS22917 CT GNDP ON QOD VIN VOUT params: ron_add=0 isd_out=0\n"
            f"XV CT GNDP ON QOD VIN VOUT {subckt} ron_add={{ron_add}} isd_out={{isd_out}}\n"
            ".ends LSW_TPS22917\n")


def cmd_decks(a):
    sim = os.path.join(a.src, "hardware", "sim")
    sys.path.insert(0, sim)
    sys.path.insert(0, os.path.join(sim, "tests"))
    import spicelib  # noqa: E402
    import test_load_switches as tls  # noqa: E402
    os.makedirs(a.out, exist_ok=True)
    got = {}

    def fake_run(deck, need=(), timeout=90):
        got["text"] = deck.text()
        raise dms_s1._Captured()

    tls.run = fake_run
    orig_vsys, orig_lib = tls.vsys_network, tls.LIB_SW

    def bench_vsys(b):
        def f(deck, main, base, corner="bat3.7", temp=25, i3v3=tls.I_3V3, **kw):
            deck += f"VBENCH n_vbench 0 {b['v']}"
            deck += f"RBENCH n_vbench n_vsys {b['r']:.6g}"
            deck += f".nodeset v(n_vsys)={b['v']} v(n_p3v3)={min(3.3, b['v'])}"
            deck += main.passives(nets={"VSYS", "GND", "VSYS_SENSE"})
            deck += f"I3V3 n_vsys 0 DC {i3v3}"
            deck += f"B3V3 n_p3v3 0 V = min(3.3, V(n_vsys) - {i3v3 * tls.LDO_RDO:.6g})"
            return deck
        return f

    views = {}
    for spec in a.view:
        name, rest = spec.split("=", 1)
        lib, subckt = rest.rsplit(":", 1)
        text = open(lib, encoding="utf-8").read()
        path = os.path.join(a.out, f"view.{name}.lib")
        with open(path, "w", encoding="utf-8") as f:
            f.write(shim(text, subckt))
        views[name] = dict(lib=path, sha256=hashlib.sha256(text.encode()).hexdigest(), subckt=subckt)
    cases = []
    for bench, b in dms_s1.BENCHES.items():
        for order, edges in (("ok", EDGES_OK), ("wrong", EDGES_WRONG)):
            for vname, v in views.items():
                tls.vsys_network, tls.LIB_SW = bench_vsys(b), v["lib"]
                try:
                    tls.led_rail_sequence(spicelib.Design("main"), spicelib.Design("base"), b["corner"], b["temp"])
                except dms_s1._Captured:
                    pass
                finally:
                    tls.vsys_network, tls.LIB_SW = orig_vsys, orig_lib
                name = f"{order}_{bench}.{vname}"
                with open(os.path.join(a.out, name + ".cir"), "w", encoding="utf-8") as f:
                    f.write(dms_s1._body(got.pop("text"), ("VEN", "VDIN")))
                cases.append(dict(name=name, order=order, bench=bench, view=vname,
                                  edges=[[n, t * US, lv] for n, t, lv in edges], stop=STOP_US * US, voh=1.0,
                                  probes=PROBES))
    with open(os.path.join(a.out, "manifest.json"), "w") as f:
        json.dump({"schema": "bb-dms-m5b/1", "views": views, "cases": cases,
                   "options": dms_s1.OPTION_SETS["phase0"]}, f, indent=1)
    print(f"m5b decks: {len(cases)} in {a.out}")


def _load(path):
    import numpy as np
    d = np.loadtxt(path, delimiter=",", skiprows=1, ndmin=2)
    return d[:, 0].astype(np.int64), d[:, 1:]


def cmd_gate(a):
    import numpy as np
    d = a.dir
    man = json.load(open(os.path.join(d, "decks", "manifest.json")))
    runs = {}
    for ln in open(os.path.join(d, "runs.jsonl")):
        r = json.loads(ln)
        runs[r["id"]] = r
    out = {"schema": "bb-dms-m5b-summary/1", "views": man["views"], "gates": [], "e1": {}, "runs": runs}
    ok = True

    def gate(name, passed, detail):
        nonlocal ok
        ok &= bool(passed)
        out["gates"].append({"gate": name, "pass": bool(passed), "detail": detail})
        print(f"dms-m5b: {'PASS' if passed else 'FAIL'}: {name}: {detail}")

    def col(p):
        return PROBES.index(p)

    data = {}
    for c in man["cases"]:
        r = runs[c["name"]]
        gate(f"{c['name']}: 120 ms through NgspiceEngine at phase0", r["status"] == "ok" and r["reached_fs"] == c["stop"],
             f"status {r['status']}, reached {r['reached_fs'] / 1e12:g} ms, {r.get('solver_points')} solver points, "
             f"{r['wall_s']} s")
        if r["status"] == "ok":
            data[c["name"]] = _load(os.path.join(d, "runs", c["name"] + ".csv"))
    for bench in dms_s1.BENCHES:
        res = {}
        for v in ("vendor", "fit"):
            name = f"ok_{bench}.{v}"
            if name not in data:
                continue
            t, x = data[name]
            vled, over = x[:, col("n_vled")], x[:, col("n_din_over")]
            rises = {}
            for nm, tus in (("p006_rise_1", 10_500), ("p006_rise_2", 60_600)):
                i = int(np.searchsorted(t, tus * US))
                assert t[i] == tus * US
                rises[nm] = float(vled[i])
            toff = {}
            for nm, tus in (("cycle1", 15_600), ("cycle2", 65_700)):
                toff[nm] = float(dms_s1._t_off10(t, vled.reshape(-1, 1), tus * US) * 1e3)
            res[v] = dict(vled_at_p006_rise_V=rises, lsw3_max_din_over_V=float(runs[name]["probes"]["n_din_over"]["max"]),
                          lsw3_max_din_over_sampled_V=float(over.max()), lsw2_t_off10_ms=toff,
                          checks=dict(led_data_v4=bool(min(rises.values()) >= 2.67),
                                      lsw3=bool(float(runs[name]["probes"]["n_din_over"]["max"]) <= 0.7),
                                      lsw2=bool(max(toff.values()) <= 30.0)))
        if set(res) != {"vendor", "fit"}:
            continue
        vv, vf = res["vendor"], res["fit"]
        dv = max(abs(vv["vled_at_p006_rise_V"][k] - vf["vled_at_p006_rise_V"][k]) for k in vv["vled_at_p006_rise_V"])
        gate(f"{bench}: LED-DATA-V4, VLED at each P0.06 rise, fitted vs vendor-derived view within 10 mV", dv <= 10e-3,
             f"max |dV| {dv * 1e3:.3f} mV (vendor {vv['vled_at_p006_rise_V']}, fit {vf['vled_at_p006_rise_V']})")
        do = abs(vv["lsw3_max_din_over_V"] - vf["lsw3_max_din_over_V"])
        gate(f"{bench}: LSW-3, max(V(LED_DIN) - V(VLED)) over the run, within 10 mV", do <= 10e-3,
             f"|d| {do * 1e3:.4f} mV (vendor {vv['lsw3_max_din_over_V']:.6g} V, fit {vf['lsw3_max_din_over_V']:.6g} V)")
        dt = max(abs(vv["lsw2_t_off10_ms"][k] - vf["lsw2_t_off10_ms"][k]) for k in vv["lsw2_t_off10_ms"])
        gate(f"{bench}: LSW-2, VLED below 10 % after LED_PWR_EN falls, within 1 ms", dt <= 1.0,
             f"max |dt| {dt:.4f} ms (vendor {vv['lsw2_t_off10_ms']}, fit {vf['lsw2_t_off10_ms']})")
        same = all(vv["checks"][k] == vf["checks"][k] for k in vv["checks"])
        gate(f"{bench}: the E1 checks give the same verdicts with either view", same,
             f"vendor {vv['checks']}, fit {vf['checks']}")
        tv, xv = data[f"ok_{bench}.vendor"]
        tf, xf = data[f"ok_{bench}.fit"]
        dw = np.abs(xv[:, col("n_vled")] - xf[:, col("n_vled")])
        k = int(dw.argmax())
        res["whole_waveform_max_dVLED_mV"] = {"value": float(dw[k]) * 1e3, "at_ms": float(tv[k]) / 1e12}
        # wrong order: the LSW-3 excursion and the clamp current
        wr = {}
        for v in ("vendor", "fit"):
            name = f"wrong_{bench}.{v}"
            if name not in data:
                continue
            t, x = data[name]
            over = x[:, col("n_din_over")]
            i = int(over.argmax())
            vm, vr = x[i, col("n_led_din_mcu")], x[i, col("n_led_din_r")]
            # the settled phantom state just before the first turn-on (VLED held up by the clamp current
            # against U2's quick output discharge): where the two views' QOD resistances show
            j = int(np.searchsorted(t, 10_490 * US))
            sm, sr = x[j, col("n_led_din_mcu")], x[j, col("n_led_din_r")]
            wr[v] = dict(max_din_over_V=float(runs[name]["probes"]["n_din_over"]["max"]),
                         at_ms=float(t[i]) / 1e12, clamp_current_mA=float((vm - vr) / R40 - vr / R41) * 1e3,
                         lsw3_fails=bool(float(runs[name]["probes"]["n_din_over"]["max"]) > 0.7),
                         settled_10_49ms=dict(vled_V=float(x[j, col("n_vled")]),
                                              din_over_V=float(x[j, col("n_din_over")]),
                                              clamp_current_mA=float((sm - sr) / R40 - sr / R41) * 1e3))
        if set(wr) == {"vendor", "fit"}:
            dw2 = abs(wr["vendor"]["max_din_over_V"] - wr["fit"]["max_din_over_V"])
            gate(f"{bench}: wrong order, the LSW-3 excursion within 10 mV, and LSW-3 fails with both views",
                 dw2 <= 10e-3 and wr["vendor"]["lsw3_fails"] and wr["fit"]["lsw3_fails"],
                 f"|d| {dw2 * 1e3:.3f} mV (vendor {wr['vendor']['max_din_over_V']:.5f} V, "
                 f"{wr['vendor']['clamp_current_mA']:.3f} mA; fit {wr['fit']['max_din_over_V']:.5f} V, "
                 f"{wr['fit']['clamp_current_mA']:.3f} mA)")
            sv, sf = wr["vendor"]["settled_10_49ms"], wr["fit"]["settled_10_49ms"]
            ds = abs(sv["din_over_V"] - sf["din_over_V"])
            gate(f"{bench}: wrong order, the settled DIN - VLED before turn-on (10.49 ms) within 10 mV", ds <= 10e-3,
                 f"|d| {ds * 1e3:.3f} mV (VLED vendor {sv['vled_V']:.5f} V, fit {sf['vled_V']:.5f} V: the views' "
                 f"QOD resistances; clamp current {sv['clamp_current_mA']:.3f} / {sf['clamp_current_mA']:.3f} mA)")
        res["wrong_order"] = wr
        out["e1"][bench] = res
    out["pass"] = ok
    with open(a.summary, "w") as f:
        json.dump(out, f, indent=1, sort_keys=True)
    print(f"dms-m5b: {'PASS' if ok else 'FAIL'} ({sum(g['pass'] for g in out['gates'])}/{len(out['gates'])} gates); "
          f"summary {a.summary}")
    sys.exit(0 if ok else 1)


def main():
    p = argparse.ArgumentParser()
    sp = p.add_subparsers(dest="cmd", required=True)
    d = sp.add_parser("decks")
    d.add_argument("--src", required=True)
    d.add_argument("--view", action="append", required=True)
    d.add_argument("--out", required=True)
    d.set_defaults(fn=cmd_decks)
    g = sp.add_parser("gate")
    g.add_argument("--dir", required=True)
    g.add_argument("--summary", required=True)
    g.set_defaults(fn=cmd_gate)
    a = p.parse_args()
    a.fn(a)


if __name__ == "__main__":
    main()
