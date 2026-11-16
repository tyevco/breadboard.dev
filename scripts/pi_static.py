"""Phase 2 P1 (plan section 4; ADR-009 D1-D3; decision D-W1, ADR-009 Amendment 2): W1 and W2 in static-load
mode on the `pi` package views, against the suite replica with P0's tolerances.

Subcommands (scripts/pi_static.sh drives them):

  decks --bbm <board.bbm> --packages <dir> --spec <static_load.toml> --out <dir>
      Assemble every case's deck from the package views (each instance's view chosen at `pi` with
      ADR-006 D3's ladder, its corner applied, as `bb run` elaborates a project) and the system import
      (examples/dial-me-spinner/system/board.bbm), with the bench elements of static-load mode; write
      <dir>/<case>.cir and <dir>/manifest.json (every instance: package, view, rung, corner, params).
      A pure function of its inputs: two runs give the same bytes.
  gate --dir <dir> --spec <static_load.toml> --tolerances <tolerances.toml> --summary <json> [--full]
      Measure every run and check P1's static-load rows; exit 1 unless every gate passes. --full adds
      the grant-partition and determinism runs of the stress corner.

Python standard library only (it runs in make test).
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import sys
import tomllib
from decimal import Decimal

US = 1_000_000_000
MS = 1000 * US
LADDER = ["fast", "pi", "si", "em"]
SUFFIX = {"f": -15, "p": -12, "n": -9, "u": -6, "m": -3, "k": 3, "meg": 6, "g": 9, "t": 12}
# ngspice probes of each kind of deck (deck_run --probe), by qualified net
PROBES = {"cue": ["main:VSYS", "main:+3V3", "base:VSYS", "main:VLED", "main:OLED_VIN"],
          "old": ["main:VSYS", "main:+3V3", "base:VSYS", "main:OLED_VIN", "main:+13V_OLED"]}
VBOR = {"vbor_min": 1.57, "vbor_typ": 1.60, "vbor_max": 1.63}
FW_LED_CUE = Decimal("0.020")          # A: the suite's LED charging cue (FW_LED_CUE)
N_LEDS_FULL_A = Decimal(48 * 3) * Decimal("0.005")   # A: 48 pixels x 3 channels x 5 mA (N_LEDS_FULL_A)
STOP = {"cue": 15_200 * US, "old": 5_000 * US}
EDGE = {"cue": "main:LED_PWR_EN", "old": "main:OLED_VCC_EN"}
SUITE_CONSTANTS = {
    "VBUS": Decimal("5.0"),        # the suite's _dead corner: USB 5.0 V
    "RPACK": Decimal("0.25"),      # RPACK[25 C]
    "R_HARNESS": Decimal("0.0142"),  # 8.5 mOhm (2 x 28 AWG VSYS) + 5.7 mOhm (3 x GND)
}


# ------------------------------------------------------------------------------------------- numbers
def value(text: str) -> Decimal:
    """A value-grammar number (spec/package.md section 6: a decimal with an optional SI prefix, Meg for
    mega) as an exact Decimal."""
    m = re.fullmatch(r"\s*([-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?)\s*(Meg|[fpnumkGT])?\s*", text)
    if not m:
        raise ValueError(f"not a value: {text!r}")
    d = Decimal(m.group(1))
    if m.group(2):
        d = d.scaleb(SUFFIX[m.group(2).lower()])
    return d


def spice(d: Decimal) -> str:
    """A plain SPICE number for an exact Decimal (no suffix: ADR-006 D5)."""
    d = d.normalize()
    s = format(d, "f") if -7 <= d.adjusted() <= 7 else format(d, "E")
    return s.rstrip("0").rstrip(".") if "." in s and "E" not in s else s


# ------------------------------------------------------------------------------------------- packages
def load_package(root: str, pid: str):
    d = os.path.join(root, pid)
    part = tomllib.load(open(os.path.join(d, "part.toml"), "rb"))
    pins = tomllib.load(open(os.path.join(d, part["pins"]["file"]), "rb"))
    return {"dir": d, "part": part, "pins": pins.get("pin", [])}


def select_view(pkg, fidelity):
    """ADR-006 D3's ladder (cli/src/elab.rs select_view): the view whose default_for has the rung,
    else walking down; pin_load views are not instance views."""
    start = LADDER.index(fidelity)
    for rung in reversed(LADDER[: start + 1]):
        for v in pkg["part"].get("analog", []):
            if v.get("kind", "circuit") != "pin_load" and rung in v.get("default_for", []):
                return v, rung
    return None, None


# ------------------------------------------------------------------------------------------- decks
def cmd_decks(a):
    spec = tomllib.load(open(a.spec, "rb"))
    bbm = json.load(open(a.bbm))
    grounds = {b["ground"] for b in bbm["boards"]}
    nets = {n["name"]: n for n in bbm["nets"]}

    def node(net):
        if net in grounds:
            return "0"
        return nets[net]["spice"]

    pkgs = {}

    def pkg(pid):
        if pid not in pkgs:
            pkgs[pid] = load_package(a.packages, pid)
        return pkgs[pid]

    insts = sorted((i for i in bbm["instances"] if i["status"] == "ok" and i.get("package")),
                   key=lambda i: (i["board"], i["ref"]))
    by_ref = {f"{i['board']}:{i['ref']}": i for i in bbm["instances"]}

    def pin_net(inst, number):
        for p in inst["pins"]:
            if p["number"] == number:
                return p.get("net")
        return None

    def port_nets(inst, p, view):
        out = []
        for port in view["ports"]:
            numbers = [x["number"] for x in p["pins"] if x["name"] == port]
            ns = {pin_net(inst, num) for num in numbers}
            if len(ns) != 1 or None in ns:
                return None
            out.append(ns.pop())
        return out

    def inst_value(inst):
        v = inst.get("value") or {}
        for key, exp in (("ohm_u", -6), ("farad_a", -18), ("henry_f", -15)):
            if key in v:
                return Decimal(v[key]).scaleb(exp)
        return None

    # the TPS61040's output, from the board (spec [boost])
    bst = spec["boost"]

    def two_pin(inst):
        return [p.get("net") for p in sorted(inst["pins"], key=lambda p: p["number"])]

    cout = Decimal(0)
    rtop = rbot = None
    for i in insts:
        pid = i["package"]["id"]
        ns = two_pin(i)
        if pid == "generic/capacitor" and set(ns) == {bst["output_net"], "main:GND"}:
            cout += inst_value(i)
        if pid == "generic/resistor" and set(ns) == {bst["output_net"], bst["fb_net"]}:
            rtop = inst_value(i)
        if pid == "generic/resistor" and set(ns) == {bst["fb_net"], "main:GND"}:
            rbot = inst_value(i)
    if not (cout and rtop and rbot):
        sys.exit(f"pi-static: the board's boost output: cout {cout}, divider {rtop} / {rbot}")
    vout = value(bst["vref"]) * (1 + rtop / rbot)
    boost_params = {"cout": cout, "rload": rtop + rbot, "vout": vout.quantize(Decimal("0.0001"))}

    os.makedirs(a.out, exist_ok=True)
    manifest = {"schema": "bb-pi-static-decks/0", "fidelity": "pi", "w1_label": spec["w1"]["label"],
                "boost": {k: spice(v) for k, v in boost_params.items()}, "cases": {}}
    for case, c in spec["case"].items():
        kind = c["kind"]
        scope = set(spec["scope"][kind])
        if kind == "cue":
            scope |= {f"main:/LED ring/LED_D{k}" for k in range(1, spec["scope"]["cue_led_chain"] + 1)}
        views, lines, entries, skipped = {}, [], [], []
        corners = {"main:U1": "worst_dropout", "base:U1": "suite_compat"}
        scale = c.get("cap_scale")
        for i in insts:
            q = f"{i['board']}:{i['ref']}"
            p = pkg(i["package"]["id"])
            view, rung = select_view(p, "pi")
            if view is None:
                skipped.append([q, "no analog view at pi or below"])
                continue
            pn = port_nets(i, p, view)
            if pn is None:
                skipped.append([q, "a view port is on no net or on several"])
                continue
            out = [n for n in pn if n not in scope]
            if out:
                skipped.append([q, f"port net {out[0]} is outside the scope"])
                continue
            params = {k: value(v) for k, v in view.get("params", {}).items()}
            iv = inst_value(i)
            if iv is not None:
                params["value"] = iv
            corner = None
            if q in corners:
                corner = corners[q]
                cdef = next((x for x in view.get("corners", []) if x["name"] == corner), None)
                if cdef is None:
                    sys.exit(f"pi-static: {q}: view {view['name']} has no corner {corner}")
                for k, v in cdef["params"].items():
                    params[k] = value(v)
            bench = {}
            if q == "base:U1":
                bench = {"ilim": value(c["ilim"]), "vsysreg": value(c["vsysreg"])}
            if q == bst["instance"]:
                bench = dict(boost_params)
            if p["part"]["part"]["id"] == "worldsemi/ws2812c-2020" and kind == "cue":
                bench = {"duty": (FW_LED_CUE / N_LEDS_FULL_A).quantize(Decimal("1e-7"))}
            params.update(bench)
            if scale and i["package"]["id"] == "generic/capacitor" and scale["net"] in pn:
                params["value"] = params["value"] * value(scale["factor"])
            views[(p["part"]["part"]["id"], view["name"])] = os.path.join(p["dir"], view["file"])
            el = "X" + re.sub(r"[^A-Za-z0-9]", "_", f"{i['board']}_{i['ref']}")
            lines.append(f"{el} {' '.join(node(n) for n in pn)} {view['subckt']}"
                         + "".join(f" {k}={spice(v)}" for k, v in params.items()))
            entries.append({"ref": q, "package": p["part"]["part"]["id"], "version": p["part"]["part"]["version"],
                            "view": view["name"], "rung": rung, "corner": corner,
                            "bench_params": {k: spice(v) for k, v in bench.items()},
                            "ports": dict(zip(view["ports"], pn))})
        # bench elements of static-load mode (header of the spec)
        vbus, vbat = node("base:VBUS"), node("base:VBAT")
        vsys_b, vsys, p3v3 = node("base:VSYS"), node("main:VSYS"), node("main:+3V3")
        guess = min(value(c["vsysreg"]), SUITE_CONSTANTS["VBUS"])
        i3v3 = (Decimal(c["i3v3_mA"]) / 1000).normalize()
        bench_lines = [
            f"* bench: VBUS from USB, the suite's {SUITE_CONSTANTS['VBUS']} V (an ideal source)",
            f"VBENCH_VBUS {vbus} 0 {spice(SUITE_CONSTANTS['VBUS'])}",
            "* bench [[battery]] form (ADR-010 D2): the dead cell, below BUVLO, as the suite models it: 0 V OCV "
            f"behind RPACK {SUITE_CONSTANTS['RPACK']} Ohm (25 C), as the Norton current OCV/R0 and R0",
            f"I_bench_bat0 0 {vbat} DC 0",
            f"R_bench_bat0_0 {vbat} 0 {spice(SUITE_CONSTANTS['RPACK'])}",
            f"* bench: the link harness as the suite lumps it (R_HARNESS, VSYS and GND wires), base VSYS -> main VSYS",
            f"RBENCH_HARNESS {vsys_b} {vsys} {spice(SUITE_CONSTANTS['R_HARNESS'])}",
            f"* static-load mode (ADR-009 D4): the suite's static +3V3 budget, {c['i3v3_mA']} mA, in place of the "
            "CPU's supply_load",
            f"BBENCH_LOAD {p3v3} 0 I = {spice(i3v3)}*min(1, max(0, V({p3v3})/0.5))",
            ".temp 25",
            f".nodeset v({vsys})={spice(guess)} v({vsys_b})={spice(guess)} v({p3v3})={spice(min(Decimal('3.3'), guess))}",
        ]
        head = [f"* Breadboard P1 static-load deck (ADR-009 D2, D4; plan section 2): case {case} ({kind}), "
                "fidelity pi, from the package views and examples/dial-me-spinner/system/board.bbm",
                f"* {spec['w1']['label']}" if case.startswith("w1") else f"* W2 ({'rule' if case == 'w2rule' else 'old rule'})"]
        defs = []
        for key in sorted(views):
            defs.append(open(views[key]).read().rstrip("\n"))
        body = "\n".join(head + defs + lines + bench_lines) + "\n"
        path = os.path.join(a.out, f"{case}.cir")
        with open(path, "w") as f:
            f.write(body)
        manifest["cases"][case] = {
            "kind": kind, "stop_fs": STOP[kind], "edge": [node(EDGE[kind]), spec["w1"]["t_on_us"] * US, 1],
            "probes": {n: node(n) for n in PROBES[kind]}, "instances": entries, "skipped": skipped,
            "sha256": hashlib.sha256(body.encode()).hexdigest()}
    with open(os.path.join(a.out, "manifest.json"), "w") as f:
        json.dump(manifest, f, indent=1, sort_keys=True)
    n = {c: len(m["instances"]) for c, m in manifest["cases"].items()}
    print(f"pi-static decks: {len(n)} cases in {a.out}: instances {n}; boost output {manifest['boost']}")


# ------------------------------------------------------------------------------------------- gate
def load_csv(path):
    with open(path) as f:
        head = f.readline().strip().split(",")
        rows = [[float(x) for x in ln.split(",")] for ln in f if ln.strip()]
    t = [int(r[0]) for r in rows]
    return t, {h: [r[k] for r in rows] for k, h in enumerate(head) if k}


def windows(t, v, thr, t_from):
    """Intervals (ms after t_from) where v < thr, with linear crossings; an end of None never recovers
    (scripts/pi_spike.py's rule)."""
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
            out.append([(start - t_from) / 1e12, (end - t_from) / 1e12])
            start = None
    if start is not None:
        out.append([(start - t_from) / 1e12, None])
    return out


def verdict(w):
    return "no crossing" if not w else "collapse" if w[-1][1] is None else "cross and recover"


def cmd_gate(a):
    spec = tomllib.load(open(a.spec, "rb"))
    tol = tomllib.load(open(a.tolerances, "rb"))
    man = json.load(open(os.path.join(a.dir, "decks", "manifest.json")))
    runs = {}
    for ln in open(os.path.join(a.dir, "runs.jsonl")):
        r = json.loads(ln.replace(":inf", ":Infinity").replace(":-inf", ":-Infinity"))
        p = os.path.join(a.dir, "runs", r["id"] + ".csv")
        if os.path.exists(p) and r.get("status") == "ok":
            r["csv_sha256"] = hashlib.sha256(open(p, "rb").read()).hexdigest()
        runs[r["id"]] = r
    w1 = spec["w1"]
    label = w1["label"]
    thr = w1["threshold_V"]
    t_on = w1["t_on_us"] * US
    blank = w1["blanking_ms"]
    out = {"schema": "bb-pi-static-summary/0", "w1_label": label, "decision": tol["w1"]["decision"],
           "gates": [], "measured": {}, "runs": {}, "rung": {}}
    ok = True
    print(f"pi-static: {label}")

    def gate(name, passed, detail):
        nonlocal ok
        ok &= bool(passed)
        out["gates"].append({"gate": name, "pass": bool(passed), "detail": detail})
        print(f"pi-static: {'PASS' if passed else 'FAIL'}: {name}: {detail}")

    def meas(case, grant="g1ms"):
        rid = f"{case}.{grant}"
        r = runs.get(rid)
        if r is None or r.get("status") != "ok":
            return None
        if rid in out["measured"]:
            return out["measured"][rid]
        c = man["cases"][case]
        t, v = load_csv(os.path.join(a.dir, "runs", rid + ".csv"))
        pr = c["probes"]
        p3 = v[pr["main:+3V3"]]
        m = {"vsys_min": r["probes"][pr["main:VSYS"]]["min"], "p3v3_min": r["probes"][pr["main:+3V3"]]["min"],
             "v28p5": windows(t, p3, thr, t_on)}
        if c["kind"] == "cue":
            m["p3v3_min_after_blanking"] = min(x for tt, x in zip(t, p3) if tt >= t_on + blank * MS)
        else:
            for k, th in VBOR.items():
                w = windows(t, p3, th, t_on)
                m[f"{k}_cross"] = w[0][0] if w else None
        out["measured"][rid] = m
        return m

    for rid, r in sorted(runs.items()):
        out["runs"][rid] = {k: r.get(k) for k in ("status", "reached_fs", "wall_s", "solver_points", "csv_sha256")}
        gate(f"run {rid} completes", r.get("status") == "ok",
             f"status {r.get('status')}, {r.get('wall_s')} s, {r.get('solver_points')} solver points")

    # ---- the decks: every instance at pi; the rung of every W1 check (ADR-009 D1) -----------------
    for case, c in man["cases"].items():
        rungs = sorted({e["rung"] for e in c["instances"]})
        reads = set(w1["reads"]) if case.startswith("w1") else {"main:+3V3", "main:VSYS"}
        on = [e for e in c["instances"] if set(e["ports"].values()) & reads]
        lowest = min((LADDER.index(e["rung"]) for e in on), default=None)
        rung = LADDER[lowest] if lowest is not None else None
        out["rung"][case] = {"rung": rung, "instances_on_read_nets": len(on), "rungs_in_deck": rungs}
        if case.startswith("w1"):
            gate(f"{case}: every W1 check reports rung: pi (the lowest chosen rung over the {len(on)} in-scope "
                 f"instances with a port on {', '.join(sorted(reads))})", rung == "pi",
                 f"rung {rung}; rungs in the deck {rungs}")
    stress = w1["stress_case"]
    sc = man["cases"][stress]
    views = sorted({(e["package"], e["view"], e["rung"], e["corner"] or "-") for e in sc["instances"]})
    print(f"pi-static: report: {stress}: {len(sc['instances'])} instances: "
          + "; ".join(f"{p}:{v} at {r} (corner {c})" for p, v, r, c in views))

    # ---- W1 at the stress corner against the replica (decision D-W1) -----------------------------
    ref = spec["reference"]

    def ends(w):
        return (w[0][0], w[-1][1]) if w else (None, None)

    def refwin(case):
        return [[s, None if e == "none" else e] for s, e in ref[case].get("v28p5", [])]

    m = meas(stress)
    lim = tol["static_load"]["w1_stress"]
    if m:
        rs, re_ = ends(refwin(stress))
        ms_, me = ends(m["v28p5"])
        diff = {"vsys_min_mV": (m["vsys_min"] - ref[stress]["vsys_min"]) * 1e3,
                "p3v3_min_mV": (m["p3v3_min"] - ref[stress]["p3v3_min"]) * 1e3,
                "v28p5_start_us": None if ms_ is None else (ms_ - rs) * 1e3,
                "v28p5_end_us": None if me is None or re_ is None else (me - re_) * 1e3}
        out["stress_corner"] = {"label": label, "measured": m, "replica": ref[stress], "difference": diff,
                                "tolerances": lim}
        within = all(diff[k] is not None and abs(diff[k]) <= lim[k] for k in lim)
        gate(f"{label}: static-load deck against the suite replica at 55 mA within the stress corner's "
             "tolerances (tolerances.toml [static_load.w1_stress])", within,
             ", ".join(f"{k} {diff[k]:+.3f} (limit {lim[k]:g})" if diff[k] is not None else f"{k} n/a" for k in lim))
        w = m["v28p5"]
        below = sum((iv[1] if iv[1] is not None else float("inf")) - iv[0] for iv in w)
        rec = w[-1][1] if w else None
        rec_by = blank - w1["recover_before_blanking_end_ms"]
        feas = below >= w1["min_below_ms"] and rec is not None and rec <= rec_by
        out["feasibility"] = {"label": label, "window_ms": w, "below_ms": below, "recovered_ms": rec,
                              "p3v3_min_after_blanking": m["p3v3_min_after_blanking"]}
        gate(f"{label}: feasibility at V28 +5 % ({thr} V): +3V3 below {thr} V for at least {w1['min_below_ms']} ms, "
             f"recovered by {rec_by:g} ms", feas, f"windows {w} ms after LED_PWR_EN (total {below:.3f} ms)")
        gate(f"{label}: +3V3 at least 2.7 V (flash VCC min) after the {blank:g} ms blanking",
             m["p3v3_min_after_blanking"] >= 2.7, f"{m['p3v3_min_after_blanking']:.4f} V")
        p0 = spec["p0_static"][stress]
        print(f"pi-static: report: {label}: VSYS min {m['vsys_min']:.4f} V, +3V3 min {m['p3v3_min']:.4f} V; P0's "
              f"static deck {p0['vsys_min']:.4f} V / {p0['p3v3_min']:.4f} V (window {p0['v28p5']})")
    else:
        gate(f"{label}: the stress-corner run", False, "failed")

    # ---- the margin check at the suite's 50 mA (decision D-W1) ------------------------------------
    mg = meas(w1["margin_case"])
    if mg:
        out["margin"] = {"label": "W1 margin check: 50 mA (the suite's budget), RT9080 worst_dropout",
                         "nominal": {"p3v3_min": mg["p3v3_min"], "margin_mV": (mg["p3v3_min"] - thr) * 1e3,
                                     "v28p5": mg["v28p5"]}}
        gate(f"W1 margin check: 50 mA (the suite's budget), RT9080 worst_dropout, nominal +3V3 capacitance: no "
             f"POFWARN at the +5 % corner (+3V3 never below {thr} V)", not mg["v28p5"],
             f"+3V3 min {mg['p3v3_min']:.4f} V, margin {(mg['p3v3_min'] - thr) * 1e3:+.1f} mV")
    mh = meas(w1["margin_halved_case"])
    if mh:
        out.setdefault("margin", {})["halved"] = {"p3v3_min": mh["p3v3_min"], "margin_mV": (mh["p3v3_min"] - thr) * 1e3,
                                                  "v28p5": mh["v28p5"]}
        print(f"pi-static: report: W1 margin check at 50 mA, halved +3V3 capacitance (DC-bias stand-in): +3V3 min "
              f"{mh['p3v3_min']:.4f} V, margin {(mh['p3v3_min'] - thr) * 1e3:+.1f} mV, below {thr} V {mh['v28p5']} ms")

    # ---- the load sweep -----------------------------------------------------------------------------
    out["sweep"] = {}
    for case, want in w1["sweep"].items():
        mm = meas(case)
        got = verdict(mm["v28p5"]) if mm else None
        rv = verdict(refwin(case))
        out["sweep"][case] = {"verdict": got, "expected": want, "replica": rv,
                              "p3v3_min": mm and mm["p3v3_min"]}
        tag = f" ({label})" if case == stress else " (the margin check)" if case == w1["margin_case"] else ""
        gate(f"W1 load sweep {case}{tag}: {want}", got == want,
             f"{got}; the replica: {rv}" + (" (the one verdict that differs from the suite's, recorded)"
                                            if got != rv and got == want else ""))

    # ---- W2: the rule and the old rule against the replica (P0's tolerances) -----------------------
    for case in ("w2rule", "w2old"):
        mm = meas(case)
        lim = tol["static_load"][case]
        if not mm:
            gate(f"{case}: static-load deck against the suite replica", False, "the run failed")
            continue
        diff = {"vsys_min_mV": (mm["vsys_min"] - ref[case]["vsys_min"]) * 1e3,
                "p3v3_min_mV": (mm["p3v3_min"] - ref[case]["p3v3_min"]) * 1e3}
        if case == "w2old":
            for k in VBOR:
                x = mm[f"{k}_cross"]
                diff[f"{k}_cross_us"] = None if x is None else (x - ref[case][f"{k}_cross"]) * 1e3
        out.setdefault("w2", {})[case] = {"measured": mm, "replica": ref[case], "difference": diff, "tolerances": lim}
        within = all(diff.get(k) is not None and abs(diff[k]) <= lim[k] for k in lim)
        gate(f"{case}: static-load deck against the suite replica within P0's tolerances "
             f"(tolerances.toml [static_load.{case}])", within,
             ", ".join(f"{k} {diff[k]:+.3f} (limit {lim[k]:g})" if diff.get(k) is not None else f"{k} n/a"
                       for k in lim))

    # ---- partitions and determinism (--full) ----------------------------------------------------------
    if a.full:
        base = runs.get(f"{stress}.g1ms", {}).get("csv_sha256")
        for g in ("g10us", "g10ms", "g1ms_again"):
            o = runs.get(f"{stress}.{g}", {}).get("csv_sha256")
            gate(f"{label}: {'a second 1 ms run' if g == 'g1ms_again' else 'grants of ' + g[1:]} gives the 1 ms "
                 "run's samples byte for byte (a consistency check: deck_run's own partitions)",
                 base is not None and o == base, f"{o and o[:16]} vs {base and base[:16]}")

    out["pass"] = ok
    with open(a.summary, "w") as f:
        json.dump(out, f, indent=1, sort_keys=True)
    print(f"pi-static: {'PASS' if ok else 'FAIL'} ({sum(g['pass'] for g in out['gates'])}/{len(out['gates'])} "
          f"gates); {label}; summary {a.summary}")
    sys.exit(0 if ok else 1)


def main():
    p = argparse.ArgumentParser()
    sp = p.add_subparsers(dest="cmd", required=True)
    d = sp.add_parser("decks")
    d.add_argument("--bbm", required=True)
    d.add_argument("--packages", required=True)
    d.add_argument("--spec", required=True)
    d.add_argument("--out", required=True)
    d.set_defaults(fn=cmd_decks)
    g = sp.add_parser("gate")
    g.add_argument("--dir", required=True)
    g.add_argument("--spec", required=True)
    g.add_argument("--tolerances", required=True)
    g.add_argument("--summary", required=True)
    g.add_argument("--full", action="store_true")
    g.set_defaults(fn=cmd_gate)
    a = p.parse_args()
    a.fn(a)


if __name__ == "__main__":
    main()
