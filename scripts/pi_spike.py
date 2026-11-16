"""Phase 2 plan P0 (numerics and feasibility spike, ADR-009 D2 and G3): W1 and W2 through NgspiceEngine.

Subcommands (scripts/pi_spike.sh drives them):

  lint   <view.lib>...
      ADR-009 D2's rules on the spike's `pi` views, mechanically: no branch-current unknown (no V, E,
      F, G or H element, no V-type B source), no XSPICE element, min()/max() with exactly two
      arguments, every B source an I-type expression. Exit 1 on a finding.
  decks  --src <export root> --views <dir> --packages <dir> --lib-fixed <lib> --out <dir>
      Build every case with the suite's own functions (hardware/sim/tests/test_load_switches.py at the
      reference commit) in three variants and write <dir>/manifest.json:
        suite     the suite's deck unchanged (its own models, the TI-derived TPS22917): the REPLICA;
        suitefix  the same with the S1-fixed TPS22917 view (ADR-006 Amendment 2);
        pi        STATIC-LOAD MODE: the same passives from the netlist, with Breadboard's views in
                  place of the suite's models: the spike's RT9080, BQ25180 and TPS61040 `pi` views
                  (examples/dial-me-spinner/pi-spike/views), the committed ti/tps22917 and
                  worldsemi/ws2812c-2020 package views, the suite's static +3V3 budget as a bench
                  load on +3V3 (ADR-009 D4's `[[load]] replace`), a 5 V bench VBUS and a bench cell.
      Bodies have no .options and no .control; the firmware's edge is listed in the manifest and
      delivered as digital_out glue (deck_run --edge), as S1 did. Every deck is LOCAL-ONLY: the
      suite variants hold the TI-derived TPS22917, and every variant holds the suite's netlist
      extraction and diode model; they stay in the scratch directory.
  gate   --dir <dir> --summary <json> --suite-results <results.json> --tolerances <toml>
      Measure every run, compare, and write the summary; exit 1 unless every P0 gate passes.
"""
from __future__ import annotations

import argparse
import functools
import hashlib
import json
import os
import re
import sys

US = 1_000_000_000          # fs per us
MS = 1000 * US

# Option sets. phase0 and suite_fallback are bbpkg::optsets (ADR-006 D6); suite_l1 is the suite's
# fallback level 1 (the option set its W1 result was computed at, results.json "decks");
# suite_default its level 0; phase0_gear is phase0 with Gear integration (the review's measurement).
OPTION_SETS = {
    "phase0": ".options RELTOL=1e-5 VNTOL=1e-8 ABSTOL=1e-13",
    "phase0_gear": ".options RELTOL=1e-5 VNTOL=1e-8 ABSTOL=1e-13 METHOD=GEAR",
    "suite_fallback": ".options RELTOL=1e-3 METHOD=GEAR RSHUNT=1e12",
    "suite_l1": ".options RELTOL=1e-3",
    "suite_default": ".options RELTOL=1e-4",
}

# The cases. t_on: the firmware's enable edge (the suite's VCTL PWL, 0 -> 1 in 100 ns at 0.2 ms).
T_ON = 200 * US
SUITE_W1 = "test_u21_turn_on_dead_battery_usb_100ma_with_fw_rule[ILIM80min-SYSREG4.4-2pct]"
SUITE_W2R = "test_u21_turn_on_dead_battery_usb_100ma_with_fw_rule[ILIM100-SYSREG4.5]"
SUITE_W2O = "test_u21_turn_on_dead_battery_usb_100ma"
CASES = {
    # W1: ILIM 80 mA min, SYS_REG 4.4 V - 2 %, the LED charging cue (LED_PWR_EN), the 50 mA budget
    "w1": dict(kind="cue", ilim=0.080, vsysreg=4.4 * 0.98, i3v3=0.050, stop=15_200 * US, pad="LED_PWR_EN",
               suite_test=SUITE_W1),
    # W1's load sweep (plan section 2 table; the suite's own deck at 30/40/60 mA)
    "w1_l30": dict(kind="cue", ilim=0.080, vsysreg=4.4 * 0.98, i3v3=0.030, stop=15_200 * US, pad="LED_PWR_EN"),
    "w1_l40": dict(kind="cue", ilim=0.080, vsysreg=4.4 * 0.98, i3v3=0.040, stop=15_200 * US, pad="LED_PWR_EN"),
    "w1_l60": dict(kind="cue", ilim=0.080, vsysreg=4.4 * 0.98, i3v3=0.060, stop=15_200 * US, pad="LED_PWR_EN"),
    # W2 rule: ILIM 100 mA, SYS_REG 4.5 V, the cue instead of the OLED
    "w2rule": dict(kind="cue", ilim=0.100, vsysreg=4.5, i3v3=0.050, stop=15_200 * US, pad="LED_PWR_EN",
                   suite_test=SUITE_W2R),
    # W2 old rule: the OLED (U21 + TPS61040) enabled at ILIM 100 mA, 5 ms (the suite's window)
    "w2old": dict(kind="old", ilim=0.100, vsysreg=4.5, i3v3=0.050, stop=5_000 * US, pad="OLED_VCC_EN",
                  suite_test=SUITE_W2O),
}
# Q1's stress-corner sweep: W1 at every whole mA from 51 to 59, to find the smallest static load at
# which the static-load deck crosses 2.94 V for 0.5 ms. The approver invoked the fallback (decision
# D-W1, 2026-11-15, ADR-009 Amendment 2): W1 is gated at that load, labelled the stress corner
# (tolerances.toml [w1]), and the suite's 50 mA ("w1") is its margin check.
for _ma in range(51, 60):
    CASES[f"w1_l{_ma}"] = dict(CASES["w1"], i3v3=_ma / 1000, suite_test=None)
PROBES = {"cue": ["n_vsys", "n_p3v3", "n_vled", "n_oled_vin", "n_iu2", "n_vsys_b"],
          "old": ["n_vsys", "n_p3v3", "n_oled_vin", "n_iu21", "n_p13v_oled", "n_vsys_b"]}

# RT9080 corners (DS9080-09; examples/dial-me-spinner/pi-spike/views/rt9080_ldo_pi.lib): "worst"
# is the dropout and load regulation maxima (the dropout the suite's ideal LDO uses), "typ" the
# typical figures.
RT_CORNERS = {"worst": "rdrop=0.883333 gm=18.18", "typ": "rdrop=0.516667 gm=36.36"}
# variant: (power views, TPS22917 view, RT9080 corner). suite = the suite's models; pi = the spike's
# views; ti = the suite's TI-derived TPS22917, tifix = it with the S1 fix (ADR-006 Amendment 2; the
# local-only package of `make dms-local-packages`), fit = the committed ti/tps22917 view. The mixed
# variants attribute the model difference to the power views and to the load switch; the fourth
# field scales the board's +3V3 capacitors (0: left out, as the suite's ideal +3V3 source has none;
# 0.5: a DC-bias stand-in, report only).
VARIANTS = {
    "suite": ("suite", "ti", None, 0),
    "suitefix": ("suite", "tifix", None, 0),
    "suite_lswfit": ("suite", "fit", None, 0),
    "pi": ("pi", "fit", "worst", 1),
    "pi_typ": ("pi", "fit", "typ", 1),
    "pi_lswfix": ("pi", "tifix", "worst", 1),
    "pi_no3v3c": ("pi", "fit", "worst", 0),
    "pi_c3half": ("pi", "fit", "worst", 0.5),
}

# Thresholds on +3V3 (ADR-009 D6, plan section 2): POFWARN V28 at VPOFTOL +5 %, 0, -5 %, and the
# suite's own limit 2.9 V + 5 %; VBOR,ON 1.57 / 1.60 / 1.63 V.
THR = {"pof_suite": 3.045, "v28p5": 2.94, "v28": 2.80, "v28m5": 2.66}
VBOR = {"vbor_min": 1.57, "vbor_typ": 1.60, "vbor_max": 1.63}
FW_POF_BLANK = 10 * MS


# ------------------------------------------------------------------------------------------------ lint
def _logical_lines(text):
    out = []
    for ln in text.splitlines():
        s = ln.strip()
        if not s or s.startswith("*"):
            continue
        if s.startswith("+") and out:
            out[-1] += " " + s[1:]
        else:
            out.append(s)
    return out


def _call_args(expr, fn):
    """Argument counts of every call of fn( in expr (top-level commas inside its parentheses)."""
    counts = []
    for m in re.finditer(rf"(?<![A-Za-z0-9_]){fn}\s*\(", expr, flags=re.I):
        depth, n, i = 1, 1, m.end()
        while i < len(expr) and depth:
            c = expr[i]
            if c == "(":
                depth += 1
            elif c == ")":
                depth -= 1
            elif c == "," and depth == 1:
                n += 1
            i += 1
        counts.append(n)
    return counts


def lint_view(text):
    findings = []
    for ln in _logical_lines(text):
        low = ln.lower()
        if low.startswith("."):
            continue
        name = low.split()[0]
        k = name[0]
        if k in "vefgh":
            findings.append(f"branch-current element {ln.split()[0]} (rule 1)")
        elif k == "a":
            findings.append(f"XSPICE element {ln.split()[0]} (rule 1: a conductance, not a code model)")
        elif k == "b":
            m = re.match(r"\S+\s+\S+\s+\S+\s+([iv])\s*=\s*(.*)$", ln, flags=re.I)
            if not m:
                findings.append(f"B source {ln.split()[0]}: cannot parse")
                continue
            if m.group(1).lower() != "i":
                findings.append(f"V-type B source {ln.split()[0]} (rule 1)")
            for fn in ("min", "max"):
                for n in _call_args(m.group(2), fn):
                    if n != 2:
                        findings.append(f"{ln.split()[0]}: {fn}() with {n} arguments (rule 4)")
    return findings


def cmd_lint(a):
    bad = 0
    for p in a.views:
        f = lint_view(open(p, encoding="utf-8").read())
        print(f"pi-spike lint: {os.path.basename(p)}: {'clean' if not f else '; '.join(f)}")
        bad += len(f)
    # negative control: the rules must fire on a view that breaks each of them
    probe = ".subckt X A B\nV1 A n1 0\nE1 n2 0 A B 1\nB1 n3 0 V = min(V(A), V(B), 1)\nB2 A B I = max(V(A))\n.ends\n"
    got = lint_view(probe)
    want = 4
    print(f"pi-spike lint: negative control ({len(got)} findings, expected >= {want}): {'; '.join(got)}")
    if len(got) < want:
        bad += 1
    sys.exit(1 if bad else 0)


# ----------------------------------------------------------------------------------------------- decks
class _Captured(Exception):
    pass


def _body(text, drop):
    keep, in_ctl = [], False
    for ln in text.splitlines():
        low = ln.strip().lower()
        if low.startswith(".control"):
            in_ctl = True
            continue
        if in_ctl:
            if low.startswith(".endc"):
                in_ctl = False
            continue
        if low.startswith(".option") or low == ".end":
            continue
        if any(low.startswith(s.lower() + " ") for s in drop):
            continue
        keep.append(ln)
    return "\n".join(keep) + "\n"


def cmd_decks(a):
    sim = os.path.join(a.src, "hardware", "sim")
    sys.path.insert(0, sim)
    sys.path.insert(0, os.path.join(sim, "tests"))
    import spicelib  # noqa: E402
    import test_load_switches as tls  # noqa: E402

    os.makedirs(a.out, exist_ok=True)
    views = {k: os.path.abspath(os.path.join(a.views, f)) for k, f in
             (("rt", "rt9080_ldo_pi.lib"), ("bq", "bq25180_pp_pi.lib"), ("bst", "tps61040_boost_pi.lib"))}
    lsw_fit = os.path.abspath(os.path.join(a.packages, "ti", "tps22917", "analog", "lsw_fit.lib"))
    ws_pkg = os.path.abspath(os.path.join(a.packages, "worldsemi", "ws2812c-2020", "analog",
                                          "load_switches__ws2812c.lib"))
    captured = {}

    def fake_run(deck, need=(), timeout=90):
        captured["text"] = deck.text()
        raise _Captured()

    tls.run = fake_run
    orig = {k: getattr(tls, k) for k in ("vsys_network", "tps22917", "oled_input", "gpio", "LIB_SW", "LIB_WS")}
    pads = []

    def pi_vsys(rt_params, i3v3, cap3v3, deck, main, base, corner="bat3.7", temp=25, i3v3_unused=None, ronbat="55m",
                ronin="0.27", vsysreg=4.5, i_extra=0.0, ilim=0.475, **kw):
        """vsys_network in static-load mode: the BQ25180 and RT9080 `pi` views, bench VBUS and cell."""
        if i_extra:
            deck += f"IEXTRA n_vsys 0 DC {i_extra:.6g}"
        c = tls.CORNERS[corner]
        deck.include(views["bq"])
        deck += f"VBENCH_VBUS n_vbus 0 {c['vbus'] if c['usb'] else 0}"
        deck += f"VBENCH_CELL n_vcell 0 {c['vbat']}"
        deck += f"RBENCH_PACK n_vcell n_vbat {tls.RPACK[temp]}"
        deck += (f"XBQ n_vbus n_vsys_b n_vbat 0 BQ25180_PP_PI params: vsysreg={vsysreg} ilim={ilim:.6g} "
                 f"ronin={ronin} ronbat={ronbat} sup_gate=0")           # suite_compat corner (ADR-009 D2)
        deck += tls._rename(base.passives(nets={"VSYS", "GND"}), "b")
        deck += f"RHARN n_vsys_b n_vsys {tls.R_HARNESS:.6g}"
        v_guess = min(vsysreg, c["vbus"]) if c["usb"] else c["vbat"]
        deck += f".nodeset v(n_vsys)={v_guess} v(n_vsys_b)={v_guess} v(n_p3v3)={min(3.3, v_guess)}"
        deck += main.passives(nets={"VSYS", "GND", "VSYS_SENSE"})
        deck.include(views["rt"])
        u1 = {f: main.node(n) for f, n in (("VIN", "VSYS"), ("VOUT", "+3V3"))}
        deck += f"XU1 {u1['VIN']} {u1['VOUT']} {u1['VIN']} 0 RT9080_LDO_PI params: {rt_params}"
        if cap3v3:
            # the board's +3V3 capacitors at their netlist values, or scaled (a DC-bias stand-in, report only)
            for ln in main.passives(nets={"+3V3", "GND"}):
                tok = ln.split()
                if cap3v3 != 1 and tok[0].upper().startswith("C"):
                    tok[-1] = f"{float(tok[-1]) * cap3v3:.6g}"
                deck += " ".join(tok)
        # the suite's static +3V3 budget as a bench load on +3V3 (static-load mode, ADR-009 D4)
        deck += f"BBENCH_LOAD n_p3v3 0 I = {i3v3:.6g}*min(1, max(0, V(n_p3v3)/0.5))"
        return deck

    def pi_oled(deck, main, boost=True, panel_esd=False, esd=None):
        orig["oled_input"](deck, main, boost=False, panel_esd=panel_esd, esd=esd)
        if boost:
            deck.include(views["bst"])
            u = {f: main.pin("U20", f) for f in ("VIN", "SW", "FB", "EN", "GND")}
            k = main.pin("D21", "K")
            deck += f"XU20 {u['VIN']} {u['SW']} {u['FB']} {u['EN']} {u['GND']} {k} TPS61040_BOOST_PI"
        return deck

    def pi_gpio(deck, main, net, out, oe, rout=None, ileak=0.0, name=None):
        pads.append((main.node(net), out))
        return deck

    def build(case, variant):
        c = CASES[case]
        main, base = spicelib.Design("main"), spicelib.Design("base")
        for k, v in orig.items():
            setattr(tls, k, v)
        pads.clear()
        power, lsw, rtc, cap3v3 = VARIANTS[variant]
        if power == "pi":
            tls.vsys_network = functools.partial(pi_vsys, RT_CORNERS[rtc], c["i3v3"], cap3v3)
            tls.oled_input = pi_oled
            tls.gpio = pi_gpio
            tls.LIB_WS = ws_pkg
        elif c["i3v3"] != tls.I_3V3:
            tls.vsys_network = functools.partial(orig["vsys_network"], i3v3=c["i3v3"])
        tls.LIB_SW = {"ti": orig["LIB_SW"], "tifix": os.path.abspath(a.lib_fixed), "fit": lsw_fit}[lsw]
        try:
            if c["kind"] == "cue":
                tls.usb_100ma_charging_cue(main, base, c["ilim"], c["vsysreg"])
            else:
                tls.u21_on_usb_dead_battery(main, base, c["ilim"], stop=c["stop"] / 1e15)
        except _Captured:
            pass
        finally:
            for k, v in orig.items():
                setattr(tls, k, v)
        text = captured.pop("text")
        if lsw == "fit":
            text = text.replace(" LSW_TPS22917 params:", " TI_TPS22917_LSW_FIT params:")
        if power == "pi":
            # The package's pixel view (renamed to its package prefix in the schema epoch, ADR-011 D11):
            # the suite's instances name the suite's subcircuit.
            text = re.sub(r"\bLSW_WS2812C\b", "WORLDSEMI_WS2812C_2020_LSW", text)
            body = _body(text, ("VCTL", "VOE", "VOLEDEN"))
            pad = [p for p, out in pads if out == "n_ctl"]
            assert len(pad) == 1, pads
            edge, voh = pad[0], 3.3
        else:
            body = _body(text, ("VCTL",))
            edge, voh = "n_ctl", 1.0
        m = re.search(r"(?m)^tran\s+(\S+)\s+(\S+)", text)
        return body, edge, voh, m.group(1)

    manifest = {"schema": "bb-pi-spike/1", "option_sets": OPTION_SETS, "cases": []}
    for case, c in CASES.items():
        ent = {"name": case, **{k: v for k, v in c.items()}, "t_on": T_ON, "probes": PROBES[c["kind"]],
               "variants": {}}
        for variant in VARIANTS:
            body, edge, voh, step = build(case, variant)
            path = os.path.join(a.out, f"{case}.{variant}.cir")
            with open(path, "w", encoding="utf-8") as f:
                f.write(body)
            ent["variants"][variant] = {"body": os.path.basename(path), "edge": [edge, T_ON, 1], "voh": voh,
                                        "suite_tran_step": step,
                                        "sha256": hashlib.sha256(body.encode()).hexdigest()}
        manifest["cases"].append(ent)
    with open(os.path.join(a.out, "manifest.json"), "w") as f:
        json.dump(manifest, f, indent=1)
    print(f"pi-spike decks: {len(manifest['cases'])} cases x {len(VARIANTS)} variants in {a.out}")


# ------------------------------------------------------------------------------------------------ gate
def _load(path):
    import numpy as np
    with open(path) as f:
        head = f.readline().strip().split(",")
    d = np.loadtxt(path, delimiter=",", skiprows=1, ndmin=2)
    return d[:, 0].astype(np.int64), {h: d[:, i] for i, h in enumerate(head) if i}


def _windows(t, v, thr, t_from):
    """Intervals (fs, relative to t_from) where v < thr after t_from, with linear crossings."""
    out, start = [], None
    for i in range(len(t)):
        if t[i] < t_from:
            continue
        below = v[i] < thr
        if below and start is None:
            if i > 0 and t[i - 1] >= t_from and v[i - 1] >= thr:
                start = t[i - 1] + (v[i - 1] - thr) / (v[i - 1] - v[i]) * (t[i] - t[i - 1])
            else:
                start = t[i]
        elif not below and start is not None:
            end = t[i - 1] + (thr - v[i - 1]) / (v[i] - v[i - 1]) * (t[i] - t[i - 1])
            out.append([float(start - t_from), float(end - t_from)])
            start = None
    if start is not None:
        out.append([float(start - t_from), None])          # no recovery before the stop
    return out


def measure(csv, js, case):
    """The P0 measurements of one run (times in ms relative to t_on, voltages in V)."""
    import numpy as np
    t, v = _load(csv)
    p3, vs = v["n_p3v3"], v["n_vsys"]
    on = t >= T_ON
    m = {"vsys_min": js["probes"]["n_vsys"]["min"], "p3v3_min": js["probes"]["n_p3v3"]["min"],
         "vsys_min_csv": float(vs[on].min()), "p3v3_min_csv": float(p3[on].min()),
         "oled_vin_max": js["probes"]["n_oled_vin"]["max"]}
    if case["kind"] == "cue":
        post = t >= T_ON + FW_POF_BLANK
        m["p3v3_min_after_blanking"] = float(p3[post].min())
        m["iu2_max"] = js["probes"]["n_iu2"]["max"]
    for k, thr in {**THR, **(VBOR if case["kind"] == "old" else {})}.items():
        w = _windows(t, p3, thr, T_ON)
        m[f"win_{k}"] = [[x / 1e12 if x is not None else None for x in iv] for iv in w]
    return m


def _first_last(win):
    """(start of the first window, end of the last) in ms, end None if it never recovers."""
    if not win:
        return None, None
    return win[0][0], win[-1][1]


def cmd_gate(a):
    import tomllib
    d = a.dir
    man = json.load(open(os.path.join(d, "decks", "manifest.json")))
    cases = {c["name"]: c for c in man["cases"]}
    runs = {}
    for ln in open(os.path.join(d, "runs.jsonl")):
        # deck_run prints the min/max of a run that took no solver point as inf / -inf
        r = json.loads(ln.replace(":inf", ":Infinity").replace(":-inf", ":-Infinity"))
        p = os.path.join(d, "runs", r["id"] + ".csv")
        if os.path.exists(p) and r.get("status") == "ok":
            r["csv_sha256"] = hashlib.sha256(open(p, "rb").read()).hexdigest()
        runs[r["id"]] = r
    suite = json.load(open(a.suite_results))
    checks = {(c["test"].split("::")[-1], c["check"]): c["value"] for c in suite["checks"]}
    tol = tomllib.load(open(a.tolerances, "rb"))
    # W1's gated corner (decision D-W1, ADR-009 Amendment 2): the stress corner, and the suite's
    # budget as its margin check. Every W1 line and the summary carry the label.
    w1c = tol["w1"]
    stress = f"w1_l{w1c['stress_load_mA']}"
    margin = "w1" if w1c["margin_load_mA"] == 50 else f"w1_l{w1c['margin_load_mA']}"
    blank_ms = w1c["blanking_ms"]
    if VARIANTS["pi"][2] != w1c["rt9080_corner"]:
        sys.exit(f"pi-spike: tolerances.toml [w1] rt9080_corner {w1c['rt9080_corner']!r}, but the gated variant "
                 f"'pi' runs the RT9080 {VARIANTS['pi'][2]!r} corner")
    label = (f"W1 {w1c['label']}: {w1c['stress_load_mA']} mA static +3V3 load (suite budget "
             f"{w1c['margin_load_mA']} mA), RT9080 {w1c['rt9080_corner']}")
    out = {"schema": "bb-pi-spike-summary/1", "gates": [], "measured": {}, "runs": {},
           "w1_corner": {"label": w1c["label"], "text": label, "stress_load_mA": w1c["stress_load_mA"],
                         "margin_load_mA": w1c["margin_load_mA"], "rt9080_corner": w1c["rt9080_corner"],
                         "decision": w1c["decision"]}}
    print(f"pi-spike: {label}")
    ok = True

    def gate(name, passed, detail):
        nonlocal ok
        ok &= bool(passed)
        out["gates"].append({"gate": name, "pass": bool(passed), "detail": detail})
        print(f"pi-spike: {'PASS' if passed else 'FAIL'}: {name}: {detail}")

    def rid(case, variant, opts, grant="g1ms"):
        return f"{case}.{variant}.{opts}.{grant}"

    def meas(case, variant, opts, grant="g1ms"):
        i = rid(case, variant, opts, grant)
        r = runs.get(i)
        if r is None or r.get("status") != "ok":
            return None
        if i not in out["measured"]:
            out["measured"][i] = measure(os.path.join(d, "runs", i + ".csv"), r, cases[case])
        return out["measured"][i]

    for i, r in sorted(runs.items()):
        out["runs"][i] = {k: r.get(k) for k in ("status", "reached_fs", "wall_s", "solver_points", "csv_sha256")}

    # ---- 1. the replicas: results.json within 1 mV and 10 us ----------------------------------------
    rep_opts = tol["replica"]["options"]
    rep_var = tol["replica"]["variant"]

    def suite_vals(case):
        c = cases[case]
        test = c["suite_test"]
        if case == "w2old":
            tag = "OLED on, dead battery, USB ILIM 100 mA"
            return {"vsys_min": checks[(test, f"VSYS min, {tag} (record)")],
                    "p3v3_min": checks[(test, f"+3V3 min, {tag}")]}
        ilim_ma = f"{c['ilim'] * 1e3:g}"
        tag = f"OLED off + LED cue, dead battery, USB ILIM {ilim_ma} mA, SYS_REG {c['vsysreg']:.3f} V"
        return {"vsys_min": checks[(test, f"VSYS min, {tag} (record)")],
                "p3v3_min": checks[(test, f"+3V3 min, {tag}")],
                "p3v3_min_after_blanking": checks[(test, f"+3V3 min after the 10 ms POF blanking (LED_PWR_EN), {tag}")],
                "oled_vin_max": checks[(test, f"OLED_VIN max, U21 held off (boost EN = VIN), {tag}")],
                "t_last_below_pof_ms": checks[(test, f"time +3V3 is last below the POF limit after LED_PWR_EN rises, {tag} (record)")] * 1e3}

    out["replica"] = {}
    for case in ("w1", "w2rule", "w2old"):
        r = runs.get(rid(case, rep_var, rep_opts))
        gate(f"replica {case}: {rep_var} deck through NgspiceEngine at {rep_opts} completes",
             r is not None and r.get("status") == "ok",
             f"status {r and r.get('status')}, {r and r.get('wall_s')} s, {r and r.get('solver_points')} points")
        m = meas(case, rep_var, rep_opts)
        if m is None:
            continue
        sv = suite_vals(case)
        eng = {"vsys_min": m["vsys_min"], "p3v3_min": m["p3v3_min"]}
        if cases[case]["kind"] == "cue":
            eng["p3v3_min_after_blanking"] = m["p3v3_min_after_blanking"]
            eng["oled_vin_max"] = m["oled_vin_max"]
            w = m["win_pof_suite"]
            eng["t_last_below_pof_ms"] = (w[-1][1] if w and w[-1][1] is not None else float("inf")) if w else 0.0
        # the suite derives +3V3 min from VSYS min (min(3.3, VSYS - I3V3 x RDO)); the deck's B3V3 is the same
        dv = {k: abs(eng[k] - sv[k]) for k in sv if not k.endswith("_ms")}
        dt = {k: abs(eng[k] - sv[k]) for k in sv if k.endswith("_ms")}
        out["replica"][case] = {"engine": eng, "results_json": sv, "dV": dv, "dt_ms": dt}
        gate(f"replica {case}: every value of results.json within 1 mV", max(dv.values()) <= 1e-3,
             ", ".join(f"{k} {eng[k]:.6f} vs {sv[k]:.6f}" for k in dv))
        if dt:
            gate(f"replica {case}: every time of results.json within 10 us", max(dt.values()) <= 0.010,
                 ", ".join(f"{k} {eng[k]:.4f} vs {sv[k]:.4f} ms" for k in dt))

    # the suite's own (unfixed) TPS22917 view through the engine, with the suite's own numeric
    # fallback ladder (spicelib.run: reltol 1e-4, then 1e-3, then Gear + rshunt): report
    out["replica_unfixed_view"] = {}
    for case in ("w1", "w2rule", "w2old"):
        for os_ in ("suite_default", "suite_l1", "suite_fallback"):
            m = meas(case, "suite", os_)
            if m is None:
                continue
            sv = suite_vals(case)
            out["replica_unfixed_view"][case] = {"options": os_, "dV_mV": {k: (m[k] - sv[k]) * 1e3 for k in
                                                                            ("vsys_min", "p3v3_min")}}
            break
        else:
            out["replica_unfixed_view"][case] = {"options": None}
        print(f"pi-spike: report: replica {case} with the suite's unfixed view: {out['replica_unfixed_view'][case]}")

    # ---- 2. the option set: named, and qualified on the static-load decks ----------------------------
    q = tol["option_set"]["name"]
    out["option_set"] = {"name": q, "card": OPTION_SETS[q], "runs": {}}
    for case in ("w1", stress, "w2rule", "w2old"):
        for variant in ("pi",):
            base = rid(case, variant, q)
            r = runs.get(base)
            gate(f"{case} {variant}: static-load deck at {q} completes", r is not None and r.get("status") == "ok",
                 f"status {r and r.get('status')}, {r and r.get('wall_s')} s, {r and r.get('solver_points')} points")
            if r is None or r.get("status") != "ok":
                continue
            for g in ("g10us", "g10ms"):
                o = runs.get(rid(case, variant, q, g))
                same = o is not None and o.get("csv_sha256") == r.get("csv_sha256")
                gate(f"{case} {variant} at {q}: grants of {g[1:]} give the 1 ms run's samples byte for byte", same,
                     f"{o and o.get('status')}")
            # convergence: TMAX 1 us (lattice 10 us) against the run, every 10 us sample
            ref = rid(case, variant, q, "tmax1us")
            rr = runs.get(ref)
            if rr is None or rr.get("status") != "ok":
                gate(f"{case} {variant} at {q}: converged reference (TMAX 1 us) completes", False,
                     f"{rr and rr.get('status')}")
                continue
            import numpy as np
            t1, v1 = _load(os.path.join(d, "runs", base + ".csv"))
            t2, v2 = _load(os.path.join(d, "runs", ref + ".csv"))
            dwave = {p: float(np.abs(v1[p] - v2[p]).max()) for p in ("n_vsys", "n_p3v3")}
            kmax = {p: int(np.abs(v1[p] - v2[p]).argmax()) for p in dwave}
            m1, m2 = meas(case, variant, q), meas(case, variant, q, "tmax1us")
            dq = max(abs(m1[k] - m2[k]) for k in ("vsys_min", "p3v3_min"))
            dwin = 0.0
            wkeys = ("win_v28p5", "win_pof_suite") if cases[case]["kind"] == "cue" else tuple(f"win_{k}" for k in VBOR)
            for k in wkeys:
                for a1, a2 in zip(_first_last(m1[k]), _first_last(m2[k])):
                    if a1 is not None and a2 is not None:
                        dwin = max(dwin, abs(a1 - a2))
            lim_v, lim_t = tol["option_set"]["converged_mV"] * 1e-3, tol["option_set"]["converged_us"] * 1e-3
            lim_w = tol["option_set"]["waveform_mV"] * 1e-3
            out["option_set"]["runs"][case] = {
                "max_dV_quantities_mV": dq * 1e3, "max_dt_windows_us": dwin * 1e3,
                "max_dV_waveform_mV": {p: x * 1e3 for p, x in dwave.items()},
                "max_dV_waveform_at_ms": {p: int(t1[k]) / 1e12 for p, k in kmax.items()}}
            gate(f"{case} {variant} at {q}: VSYS min and +3V3 min within {lim_v * 1e3:g} mV, the threshold windows' "
                 f"start and end within {lim_t * 1e3:g} us of the converged run (TMAX 1 us)",
                 dq <= lim_v and dwin <= lim_t, f"max |dV| {dq * 1e3:.4f} mV, max |dt| {dwin * 1e3:.2f} us")
            gate(f"{case} {variant} at {q}: VSYS and +3V3 at every 10 us within {lim_w * 1e3:g} mV of the converged run",
                 max(dwave.values()) <= lim_w,
                 ", ".join(f"{p} {x * 1e3:.4f} mV at {int(t1[kmax[p]]) / 1e12:g} ms" for p, x in dwave.items()))
        # the other option sets on the static-load decks (report): phase0, suite_fallback
        for other in sorted(OPTION_SETS):
            if other == q:
                continue
            r = runs.get(rid(case, "pi", other))
            if r is None:
                continue
            rep = {"status": r.get("status"), "wall_s": r.get("wall_s"), "solver_points": r.get("solver_points")}
            m2 = meas(case, "pi", other)
            m1 = meas(case, "pi", q)
            if m1 and m2:
                rep["d_p3v3_min_mV"] = (m2["p3v3_min"] - m1["p3v3_min"]) * 1e3
                rep["d_vsys_min_mV"] = (m2["vsys_min"] - m1["vsys_min"]) * 1e3
            out["option_set"].setdefault("others", {})[f"{case}.{other}"] = rep
            print(f"pi-spike: report: {case} pi at {other}: {json.dumps(rep)}")

    # ---- 3. the model difference (pi views vs the replica) and the static-load tolerances ------------
    out["model_difference"] = {}
    fixed = tol["static_load"]

    def difference(case, mp, ms):
        diff = {"vsys_min_mV": (mp["vsys_min"] - ms["vsys_min"]) * 1e3,
                "p3v3_min_mV": (mp["p3v3_min"] - ms["p3v3_min"]) * 1e3}
        if cases[case]["kind"] == "cue":
            for k in ("v28p5", "pof_suite"):
                (s1, e1), (s2, e2) = _first_last(mp[f"win_{k}"]), _first_last(ms[f"win_{k}"])
                diff[f"{k}_start_us"] = None if s1 is None or s2 is None else (s1 - s2) * 1e3
                diff[f"{k}_end_us"] = None if e1 is None or e2 is None else (e1 - e2) * 1e3
        else:
            for k in VBOR:
                (s1, _), (s2, _) = _first_last(mp[f"win_{k}"]), _first_last(ms[f"win_{k}"])
                diff[f"{k}_cross_us"] = None if s1 is None or s2 is None else (s1 - s2) * 1e3
        return diff

    def fmt(diff):
        return ", ".join(f"{k} {v:+.3f}" if v is not None else f"{k} n/a" for k, v in diff.items())

    def propose(diff):
        """The tolerance rule (ADR-009 Amendment 1): 1.5 x |measured difference|, rounded up in the
        1-2-5 series, at least 1 mV / 10 us."""
        import math
        out_ = {}
        for k, v in diff.items():
            if v is None:
                continue
            x, floor = 1.5 * abs(v), (10.0 if k.endswith("_us") else 1.0)
            if x <= floor:
                out_[k] = floor
                continue
            e = 10 ** math.floor(math.log10(x))
            out_[k] = next(m * e for m in (1, 2, 5, 10) if m * e >= x - 1e-12)
        return out_

    # gated: W1 at its stress corner (tolerances.toml [static_load.w1_stress]) and W2; W1 at the
    # suite's 50 mA is its margin check (section 4), and its difference is reported only
    for case, key in ((margin, None), (stress, "w1_stress"), ("w2rule", "w2rule"), ("w2old", "w2old")):
        mp, ms = meas(case, "pi", q), meas(case, rep_var, rep_opts)
        tag = f"{case} ({label})" if case == stress else case
        if mp is None or ms is None:
            gate(f"{tag}: model difference measured", False, "a run failed")
            continue
        diff = difference(case, mp, ms)
        out["model_difference"][case] = {"pi": mp, "suite": ms, "difference": diff, "attribution": {},
                                         "tolerance_by_rule": propose(diff)}
        print(f"pi-spike: report: {tag}: difference {fmt(diff)}; tolerances by the rule: {propose(diff)}")
        if key is None:
            out["model_difference"][case]["gated"] = False
        else:
            lim = {k: v for k, v in fixed.get(key, {}).items() if not isinstance(v, str)}
            # every tolerance must name a measured quantity, and every gated quantity must have been measured
            known = all(k in diff and diff[k] is not None for k in lim)
            within = known and all(abs(diff[k]) <= lim[k] for k in lim)
            rule = propose(diff)
            by_rule = all(lim[k] == rule.get(k) for k in lim)
            gate(f"{tag}: the measured pi-vs-suite difference is within the tolerances fixed from it "
                 f"(tolerances.toml [static_load.{key}])", within and bool(lim),
                 ", ".join(f"{k} {diff[k]:+.3f} (limit {lim[k]:g})" for k in lim if k in diff and diff[k] is not None))
            gate(f"{tag}: tolerances.toml [static_load.{key}] is the stated rule applied to the measured difference",
                 by_rule and bool(lim), f"fixed {lim}, by the rule {rule}")
        # attribution (report): each variant against the replica
        for v in ("pi_no3v3c", "pi_c3half", "suite_lswfit", "pi_lswfix", "pi_typ"):
            mv = meas(case, v, q)
            if mv:
                out["model_difference"][case]["attribution"][v] = difference(case, mv, ms)
                print(f"pi-spike: report: {case} {v} vs the replica: {fmt(out['model_difference'][case]['attribution'][v])}")

    # ---- 4. W1's stress corner and its margin check (decision D-W1, ADR-009 Amendment 2) -------------
    thr = w1c["threshold_V"]
    wkey = next(k for k, v in THR.items() if v == thr)          # "v28p5": V28 at VPOFTOL +5 %
    min_below = w1c["min_below_ms"]
    rec_by = blank_ms - w1c["recover_before_blanking_end_ms"]   # ms after LED_PWR_EN

    def below_ms(w):
        return sum((iv[1] if iv[1] is not None else float("inf")) - iv[0] for iv in w)

    def recovered(w):
        _, e = _first_last(w)
        return e

    # 4a. the sweep at every whole mA (RT9080 worst, which W1 gates, and typ, reported): the stress load
    # is the smallest whole mA with at least 0.5 ms below 2.94 V
    out["stress_corner"] = {}
    for variant in ("pi", "pi_typ"):
        rows, smallest_below, smallest_feas, complete = {}, None, None, True
        for ma in [50] + list(range(51, 60)) + [60]:
            case = "w1" if ma == 50 else f"w1_l{ma}"
            m = meas(case, variant, q)
            if m is None:
                rows[ma] = {"status": (runs.get(rid(case, variant, q)) or {}).get("status", "not run")}
                if smallest_below is None:
                    complete = False
                continue
            w = m[f"win_{wkey}"]
            b, e = below_ms(w), recovered(w)
            feas = b >= min_below and e is not None and e <= rec_by
            rows[ma] = {"p3v3_min": m["p3v3_min"], "vsys_min": m["vsys_min"], f"win_{wkey}": w,
                        "below_ms": b if b != float("inf") else None, "recovered_ms": e, "feasible": feas}
            if b >= min_below and smallest_below is None:
                smallest_below = ma
            if feas and smallest_feas is None:
                smallest_feas = ma
        out["stress_corner"][variant] = {"rows": rows, "smallest_below_mA": smallest_below,
                                         "smallest_feasible_mA": smallest_feas}
        print(f"pi-spike: report: stress sweep, {variant} (RT9080 {VARIANTS[variant][2]}): smallest whole mA with "
              f"+3V3 below {thr} V for >= {min_below} ms: {smallest_below}; also recovered by {rec_by:g} ms: "
              f"{smallest_feas}")
        if variant == "pi":
            gate(f"{label}: the stress load is the smallest whole mA (50-60 mA, RT9080 worst) with +3V3 below "
                 f"{thr} V for at least {min_below} ms", complete and smallest_below == w1c["stress_load_mA"],
                 f"smallest {smallest_below} mA, tolerances.toml {w1c['stress_load_mA']} mA; "
                 + ", ".join(f"{ma} mA {r['below_ms']:.3f} ms" if r.get("below_ms") is not None
                             else f"{ma} mA {r.get('status', 'no recovery')}" for ma, r in rows.items()))

    # 4b. the feasibility row at the stress corner (it replaces the 50 mA row)
    mp = meas(stress, "pi", q)
    if mp:
        w = mp[f"win_{wkey}"]
        b, e = below_ms(w), recovered(w)
        feas = b >= min_below and e is not None and e <= rec_by
        s0 = w[0][0] if w else None
        mid = (s0 + e) / 2 if w and e is not None else None
        ctl = round(mid) if mid is not None else None
        out["feasibility"] = {"label": label, "case": stress, "window_ms": w, "below_ms": b, "recovered_ms": e,
                              "pass": feas, "p3v3_min": mp["p3v3_min"], "vsys_min": mp["vsys_min"],
                              "p3v3_min_after_blanking": mp["p3v3_min_after_blanking"],
                              # P4's negative controls are derived from the measured window (plan section 2)
                              "window_midpoint_ms": mid, "midpoint_control_ms": ctl,
                              "midpoint_control_inside_by_us": None if ctl is None else
                              min(ctl - s0, e - ctl) * 1e3,
                              "blanking_5ms_ends_after_window": None if e is None else e < 5.0}
        gate(f"{label}: feasibility at V28 +5 % ({thr} V): +3V3 below {thr} V for at least {min_below} ms, recovered at "
             f"least {w1c['recover_before_blanking_end_ms']:g} ms before the {blank_ms:g} ms blanking ends", feas,
             f"windows {w} ms after LED_PWR_EN (total {b:.3f} ms, recovered at {e} ms, limit {rec_by:g} ms)")
        print(f"pi-spike: report: {label}: +3V3 min {mp['p3v3_min']:.4f} V, VSYS min {mp['vsys_min']:.4f} V, +3V3 min "
              f"after the blanking {mp['p3v3_min_after_blanking']:.4f} V; window midpoint {mid} ms, so the "
              f"window-midpoint control is {ctl} ms ({out['feasibility']['midpoint_control_inside_by_us']} us inside "
              f"the window); 5 ms blanking ends after the window: {out['feasibility']['blanking_5ms_ends_after_window']}")
    else:
        gate(f"{label}: feasibility", False, "the static-load run failed")

    # 4c. the margin check at the suite's budget: no POFWARN at the +5 % corner (gated, RT9080 worst,
    # nominal +3V3 capacitance); +3V3 min and its margin reported at nominal and halved capacitance
    out["margin"] = {"label": f"W1 margin check: {w1c['margin_load_mA']} mA (the suite's budget)", "case": margin}
    for v, cap in (("pi", "nominal"), ("pi_c3half", "halved (DC-bias stand-in)"), ("pi_typ", "nominal, RT9080 typ"),
                   ("pi_no3v3c", "none (as the suite's ideal LDO)"), ("pi_lswfix", "nominal, S1-fixed TPS22917")):
        mv = meas(margin, v, q)
        if mv is None:
            out["margin"][v] = None
            if v in ("pi", "pi_c3half"):
                gate(f"W1 margin check ({w1c['margin_load_mA']} mA, +3V3 capacitance {cap}) measured", False,
                     f"run {rid(margin, v, q)}: {(runs.get(rid(margin, v, q)) or {}).get('status', 'not run')}")
            continue
        w = mv[f"win_{wkey}"]
        rep = {"capacitance": cap, "p3v3_min": mv["p3v3_min"], "margin_mV": (mv["p3v3_min"] - thr) * 1e3,
               f"win_{wkey}": w, "below_ms": below_ms(w),
               "inside_blanking": all(iv[1] is not None and iv[1] <= blank_ms for iv in w)}
        out["margin"][v] = rep
        if v == "pi":
            gate(f"W1 margin check: {w1c['margin_load_mA']} mA (the suite's budget), RT9080 worst, nominal +3V3 "
                 f"capacitance: no POFWARN at the +5 % corner (+3V3 never below {thr} V after LED_PWR_EN)", not w,
                 f"+3V3 min {mv['p3v3_min']:.4f} V, margin {rep['margin_mV']:+.1f} mV")
        print(f"pi-spike: report: W1 margin check at {w1c['margin_load_mA']} mA, +3V3 capacitance {cap}: +3V3 min "
              f"{mv['p3v3_min']:.4f} V, margin {rep['margin_mV']:+.1f} mV to {thr} V, below for {rep['below_ms']:.3f} ms "
              f"{w}{' (inside the blanking)' if w and rep['inside_blanking'] else ''}")

    # 4d. the stress corner against the replica at the same load (the replica is near its cliff there)
    for variant in ("pi", "pi_typ"):
        mp2, ms2 = meas(stress, variant, q), meas(stress, rep_var, rep_opts)
        if mp2 and ms2:
            diff = difference(stress, mp2, ms2)
            out["stress_corner"][variant].update(
                {"difference_vs_replica": diff, "tolerance_by_rule": propose(diff),
                 "replica": {"vsys_min": ms2["vsys_min"], "p3v3_min": ms2["p3v3_min"],
                             f"win_{wkey}": ms2[f"win_{wkey}"]}})
            print(f"pi-spike: report: {label}, {variant} vs the replica at the same load: {fmt(diff)}; replica "
                  f"window {ms2[f'win_{wkey}']}")

    # ---- 5. the load sweep (report; P1 gates it): no crossing, margin, stress, collapse ----------------
    out["sweep"] = {}
    for case in ("w1_l30", "w1_l40", margin, stress, "w1_l60"):
        row = {}
        for variant, opts in (("pi", q), (rep_var, rep_opts)):
            m = meas(case, variant, opts)
            if m is None:
                row[variant] = None
                continue
            w = m[f"win_{wkey}"]
            verdict = ("no crossing" if not w else "collapse" if recovered(w) is None else "cross and recover")
            row[variant] = {"vsys_min": m["vsys_min"], "p3v3_min": m["p3v3_min"], "verdict": verdict,
                            "win_v28p5": m["win_v28p5"], "win_pof_suite": m["win_pof_suite"], "win_v28": m["win_v28"]}
        out["sweep"][case] = row
        print(f"pi-spike: sweep {case}{' (' + label + ')' if case == stress else ''}: "
              + "; ".join(f"{v}: {r['verdict']}, +3V3 min {r['p3v3_min']:.4f} V" if r else f"{v}: failed"
                          for v, r in row.items()))

    out["pass"] = ok
    with open(a.summary, "w") as f:
        json.dump(out, f, indent=1, sort_keys=True)
    print(f"pi-spike: {'PASS' if ok else 'FAIL'} ({sum(g['pass'] for g in out['gates'])}/{len(out['gates'])} gates); "
          f"summary {a.summary}")
    sys.exit(0 if ok else 1)


def main():
    p = argparse.ArgumentParser()
    sp = p.add_subparsers(dest="cmd", required=True)
    li = sp.add_parser("lint")
    li.add_argument("views", nargs="+")
    li.set_defaults(fn=cmd_lint)
    d = sp.add_parser("decks")
    d.add_argument("--src", required=True)
    d.add_argument("--views", required=True)
    d.add_argument("--packages", required=True)
    d.add_argument("--lib-fixed", required=True)
    d.add_argument("--out", required=True)
    d.set_defaults(fn=cmd_decks)
    g = sp.add_parser("gate")
    g.add_argument("--dir", required=True)
    g.add_argument("--summary", required=True)
    g.add_argument("--suite-results", required=True)
    g.add_argument("--tolerances", required=True)
    g.set_defaults(fn=cmd_gate)
    a = p.parse_args()
    a.fn(a)


if __name__ == "__main__":
    main()
