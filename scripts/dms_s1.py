"""Phase 1 plan S1 (numerics spike): the E1 deck through NgspiceEngine at Phase 0's options.

Subcommands (scripts/dms_s1.sh drives them):

  fixview  --in <load_switches__tps22917.lib> --out <lib>
      Apply the S1 view-local fix to the suite's TPS22917 model (ADR-006 Amendment 1). The input is
      the TI-derived file from a `git archive` export of the reference commit; the output is
      LOCAL-ONLY (LicenseRef-local-only, plan approver decision A3) and is never written into the
      repository. Only the replacement element lines below are breadboard.dev's own text.
  decks    --src <export root> --lib-fixed <lib> --out <dir>
      Build every case with the suite's own functions (hardware/sim/tests/test_load_switches.py at
      the reference commit), once with the suite's model and once with the fixed one, and write
      <dir>/manifest.json. Bodies have no .options, no .control and no PWL control sources: the
      control edges are listed in the manifest and delivered as digital glue (deck_run --edge).
  ref      --manifest <m> --case <name> --view unmod|fixed --options <set> --tran <step> [--tmax <t>]
           --out <csv> [--timeout s]
      Standalone ngspice batch run of a case with the same edges as PWL sources behind the same
      glue resistor, sampled like NgspiceEngine::sample (linear between solver points) on the
      case's grid. Prints a JSON line.
  gate     --dir <dir> --summary <json>
      Compare the runs dms_s1.sh made and write the summary; exit 1 unless every S1 gate passes.

Option sets (ADR-006 D6): phase0 = RELTOL=1e-5 VNTOL=1e-8 ABSTOL=1e-13 (every Phase 0 board.cir);
suite_fallback = reltol 1e-3, Gear, rshunt 1e12 (the suite's last numeric fallback, the option set
of its LED rail sequence deck); suite_default = reltol 1e-4 (spicelib.Deck's default).
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
import time

OPTION_SETS = {
    "phase0": ".options RELTOL=1e-5 VNTOL=1e-8 ABSTOL=1e-13",
    "suite_fallback": ".options method=gear rshunt=1e12 reltol=1e-3",
    "suite_default": ".options reltol=1e-4",
}

US = 1_000_000_000  # fs per us


# --------------------------------------------------------------------------------------------- fixview
# The fix (ADR-006 Amendment 1): Phase 0's ABSTOL = 1e-13 A is the Newton convergence tolerance of
# every branch-current unknown. In the suite's TPS22917 view, TI's RVAR U4 (a B voltage source R*I
# with a 0 V ammeter and a current-copy CCCS) and TI's current sense H1 (a 0 V ammeter and a CCVS)
# put branch unknowns in series with the pass path, where tens of milliohms separate ~3 V nodes:
# 1e-13 A then asks for those node voltages to about 1e-14 V, below double resolution at 3 V, so
# Newton cannot certify convergence and the step collapses (1e-13 s at the LED_PWR_EN fall, 1e-16 s
# at a later P0.06 rise). The fix states the same I-V without branch unknowns:
#   RVAR U4  -> one B conductance VIN -> N145001, I = V/R with R = V(E4 table) (floored at 1 uOhm,
#               which only matters below VIN = 5 mV where the table gives R < 1 uOhm);
#   H1       -> its two terminals merged (a 0 V ammeter is a short) and its 1 Ohm transresistance
#               output I_REVERSE computed from the same branch current (VOUT side -> VIN through RVAR).
FIX_LINES = {
    r"^X_U1_U2_U4\s+VIN\s+U1_U2_N145001\s+U1_U2_N16813658\s+0\s+lsw17_RVAR\s+params:\s*RREF=1\s*$":
        "* S1 fix (breadboard.dev, ADR-006 Amendment 1): RVAR U4 as one B conductance, no branch unknown\n"
        "B_U1_U2_U4      VIN U1_U2_N145001 I = V(VIN,U1_U2_N145001)/max(V(U1_U2_N16813658),1e-6)",
    r"^X_U1_U2_H1\s+U1_U2_N16803039\s+U1_U2_N145001\s+U1_U2_I_REVERSE\s+0\s+lsw17_DRIVER_H1\s*$":
        "* S1 fix (breadboard.dev, ADR-006 Amendment 1): current sense H1 without its 0 V ammeter (nodes\n"
        "* U1_U2_N16803039 and U1_U2_N145001 merged); I_REVERSE = 1 Ohm x the current VOUT side -> VIN\n"
        "B_U1_U2_H1      U1_U2_I_REVERSE 0 V = V(U1_U2_N145001,VIN)/max(V(U1_U2_N16813658),1e-6)",
}
MERGE = ("U1_U2_N16803039", "U1_U2_N145001")


def fix_view(text: str) -> str:
    head, sep, rest = text.partition(".subckt LSW_TPS22917")
    if not sep:
        raise SystemExit("fixview: no .subckt LSW_TPS22917")
    body, sep2, tail = rest.partition(".ends LSW_TPS22917")
    if not sep2:
        raise SystemExit("fixview: no .ends LSW_TPS22917")
    for pat, rep in FIX_LINES.items():
        body, n = re.subn(pat, lambda m: rep, body, flags=re.M)
        if n != 1:
            raise SystemExit(f"fixview: expected one match of {pat!r}, found {n}")
    body = re.sub(rf"\b{MERGE[0]}\b", MERGE[1], body)
    return head + sep + body + sep2 + tail


def cmd_fixview(a):
    src = open(a.inp, encoding="utf-8").read()
    out = fix_view(src)
    out = ("* LOCAL-ONLY (LicenseRef-local-only): derived from TI SLVMCE8 via dial-me-spinner's "
           "load_switches__tps22917.lib;\n* never commit this file (Phase 1 plan approver decision A3). "
           "Generated by breadboard.dev scripts/dms_s1.py fixview.\n") + out
    with open(a.out, "w", encoding="utf-8") as f:
        f.write(out)
    print(json.dumps({"in_sha256": hashlib.sha256(src.encode()).hexdigest(),
                      "out_sha256": hashlib.sha256(out.encode()).hexdigest()}))


# ----------------------------------------------------------------------------------------------- decks
E1_EDGES_US = [("n_seq_en", 500, 1), ("n_seq_din", 10_500, 1), ("n_seq_din", 15_500, 0), ("n_seq_en", 15_600, 0),
               ("n_seq_en", 50_600, 1), ("n_seq_din", 60_600, 1), ("n_seq_din", 65_600, 0), ("n_seq_en", 65_700, 0)]
E1_STOP_US = 120_000

# plan §2 E1 benches: VSYS source, series R (RPACK + BAT-FET 55 mOhm + harness 14.2 mOhm), temperature;
# + the 50 mA VSYS load and the tracking +3V3 (min(3.3, VSYS - 44.2 mV)) of the suite's vsys_network.
BENCHES = {"nominal": dict(v=3.7, r=0.25 + 0.055 + 0.0142, temp=25, corner="bat3.7"),
           "cold": dict(v=3.0, r=0.50 + 0.055 + 0.0142, temp=-10, corner="bat3.0")}


def _body(text: str, drop_sources: tuple[str, ...]) -> str:
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
        if any(low.startswith(s.lower() + " ") for s in drop_sources):
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
    captured = {}

    def fake_run(deck, need=(), timeout=90):
        captured["text"] = deck.text()
        captured["options"] = list(deck.options)
        raise _Captured()

    tls.run = fake_run
    orig_vsys = tls.vsys_network
    lib_unmod = tls.LIB_SW
    lib_fixed = os.path.abspath(a.lib_fixed)

    def bench_vsys(name):
        b = BENCHES[name]

        def f(deck, main, base, corner="bat3.7", temp=25, i3v3=tls.I_3V3, **kw):
            deck += f"VBENCH n_vbench 0 {b['v']}"
            deck += f"RBENCH n_vbench n_vsys {b['r']:.6g}"
            deck += f".nodeset v(n_vsys)={b['v']} v(n_p3v3)={min(3.3, b['v'])}"
            deck += main.passives(nets={"VSYS", "GND", "VSYS_SENSE"})
            deck += f"I3V3 n_vsys 0 DC {i3v3}"
            deck += f"B3V3 n_p3v3 0 V = min(3.3, V(n_vsys) - {i3v3 * tls.LDO_RDO:.6g})"
            return deck
        return f

    def capture(fn):
        try:
            fn()
        except _Captured:
            pass
        return captured.pop("text"), captured.pop("options")

    def e1(corner, temp, bench):
        def go():
            tls.vsys_network = orig_vsys if bench is None else bench_vsys(bench)
            try:
                tls.led_rail_sequence(spicelib.Design("main"), spicelib.Design("base"), corner, temp)
            finally:
                tls.vsys_network = orig_vsys
        return go

    def qod(corner):
        return lambda: tls.u2_turn_off(spicelib.Design("main"), spicelib.Design("base"), corner)

    def table76(vin):
        return lambda: tls.test_model_turn_on_matches_table_7_6(None, vin)

    e1_edges = [[n, t * US, lv] for n, t, lv in E1_EDGES_US]
    cases = []
    for bench, b in BENCHES.items():
        cases.append(dict(name=f"e1_bench_{bench}", kind="e1", build=e1(b["corner"], b["temp"], bench),
                          drop=("VEN", "VDIN"), edges=e1_edges, stop=E1_STOP_US * US, voh=1.0,
                          probes=["n_vled", "n_din_over"], bench=bench))
    for corner, temp, test in (("bat3.0", -10, "test_phantom_din_abs_max_with_fw_rule[bat3.0--10]"),
                               ("bat4.2", 25, "test_phantom_din_abs_max_with_fw_rule[bat4.2-25]")):
        cases.append(dict(name=f"e1_suite_{corner}_{temp}", kind="e1", build=e1(corner, temp, None),
                          drop=("VEN", "VDIN"), edges=e1_edges, stop=E1_STOP_US * US, voh=1.0,
                          probes=["n_vled", "n_din_over"], suite_test=test))
    for corner in ("bat3.0", "bat4.2", "usb5.0"):
        cases.append(dict(name=f"qod_{corner}", kind="qod", build=qod(corner), drop=("VCTL",),
                          edges=[["n_ctl_led", 100 * US, 1], ["n_ctl_led", 5000 * US, 0]], stop=50_000 * US,
                          voh=1.0, probes=["n_vled"], suite_test=f"test_u2_qod_discharge[{corner}]"))
    for vin in (3.6, 5.0):
        cases.append(dict(name=f"table76_{vin}", kind="table76", build=table76(vin), drop=("VON",),
                          edges=[["on", 100 * US, 1]], stop=7000 * US, voh=3.3, probes=["vout"], vin=vin,
                          suite_test=f"test_model_turn_on_matches_table_7_6[{vin}]"))

    manifest = {"schema": "bb-dms-s1/1", "option_sets": OPTION_SETS, "cases": []}
    for c in cases:
        out = {k: v for k, v in c.items() if k not in ("build", "drop")}
        for view, lib in (("unmod", lib_unmod), ("fixed", lib_fixed)):
            tls.LIB_SW = lib
            text, opts = capture(c["build"])
            path = os.path.join(a.out, f"{c['name']}.{view}.cir")
            with open(path, "w", encoding="utf-8") as f:
                f.write(_body(text, c["drop"]))
            out[f"body_{view}"] = os.path.basename(path)
            if view == "unmod":
                out["suite_options"] = opts
                m = re.search(r"(?m)^tran\s+(\S+)\s+(\S+)", text)
                out["suite_tran"] = [m.group(1), m.group(2)]
        tls.LIB_SW = lib_unmod
        manifest["cases"].append(out)
    with open(os.path.join(a.out, "manifest.json"), "w") as f:
        json.dump(manifest, f, indent=1)
    print(f"decks: {len(cases)} cases in {a.out}")


class _Captured(Exception):
    pass


# ------------------------------------------------------------------------------------------------- ref
def _pwl(edges, node, voh, ramp_fs=100_000_000):
    pts, v = [(0, 0.0)], 0.0
    for n, t, lv in edges:
        if n != node:
            continue
        pts.append((t, v))
        v = voh if lv else 0.0
        pts.append((t + ramp_fs, v))
    # seconds as Python spells a float, as the suite writes its PWL sources
    return " ".join(f"{t / 1e15!r} {v:g}" for t, v in pts)


def cmd_ref(a):
    import numpy as np
    man = json.load(open(a.manifest))
    case = next(c for c in man["cases"] if c["name"] == a.case)
    d = os.path.dirname(os.path.abspath(a.manifest))
    text = open(os.path.join(d, case[f"body_{a.view}"]), encoding="utf-8").read()
    work = tempfile.mkdtemp(prefix="bb-dms-s1-")
    wav = os.path.join(work, "w.txt")
    for node in sorted({e[0] for e in case["edges"]}):
        src = "V_" + re.sub(r"[^A-Za-z0-9]", "_", node)
        text += f"{src} {src}_int 0 PWL({_pwl(case['edges'], node, case['voh'])})\nR_bbz_{src} {src}_int {node} 1\n"
    text += OPTION_SETS[a.options] + "\n"
    stop_s = f"{case['stop']}e-15"
    tmax = f" 0 {a.tmax}" if a.tmax else ""
    text += (".control\nset filetype=ascii\nset wr_singlescale\nset wr_vecnames\n"
             f"tran {a.tran} {stop_s}{tmax}\n"
             f"wrdata {wav} {' '.join(f'v({p})' for p in case['probes'])}\nquit\n.endc\n.end\n")
    deck = os.path.join(work, "deck.cir")
    with open(deck, "w") as f:
        f.write("* dms_s1 ref " + a.case + "\n" + text)
    t0 = time.monotonic()
    status = "ok"
    try:
        r = subprocess.run(["ngspice", "-b", deck], capture_output=True, text=True, timeout=a.timeout, cwd=work)
        log = r.stdout + r.stderr
        if r.returncode:
            status = f"exit {r.returncode}"
        if re.search(r"(?i)timestep too small", log):
            status = "timestep too small"
    except subprocess.TimeoutExpired:
        status = f"timeout {a.timeout:g} s"
    wall = time.monotonic() - t0
    res = {"status": status, "wall_s": round(wall, 3), "case": a.case, "view": a.view, "options": a.options,
           "tran": a.tran, "tmax": a.tmax}
    if os.path.exists(wav):
        data = np.loadtxt(wav, skiprows=1, ndmin=2)
        t = data[:, 0]
        res["reached_fs"] = int(round(t[-1] * 1e15))
        res["solver_points"] = len(t)
        if status == "ok":
            grid = np.arange(0, case["stop"] + 1, 10 * US)
            with open(a.out, "w") as f:
                f.write("t_fs," + ",".join(case["probes"]) + "\n")
                cols = [np.interp(grid * 1e-15, t, data[:, k + 1]) for k in range(len(case["probes"]))]
                for i, g in enumerate(grid):
                    f.write(f"{g}," + ",".join(f"{c[i]:.9e}" for c in cols) + "\n")
            res["probes"] = {p: {"min": float(data[:, k + 1].min()), "max": float(data[:, k + 1].max())}
                             for k, p in enumerate(case["probes"])}
    print(json.dumps(res))


# ------------------------------------------------------------------------------------------------ gate
def _load(path):
    import numpy as np
    d = np.loadtxt(path, delimiter=",", skiprows=1, ndmin=2)
    return d[:, 0].astype(np.int64), d[:, 1:]


def _at(t, v, tfs, col=0):
    import numpy as np
    i = int(np.searchsorted(t, tfs))
    assert t[i] == tfs, tfs
    return float(v[i, col])


def _t_off10(t, v, t_off, v0=None):
    """Time (s) from t_off until VLED < 10 % of v0 (default: its value at the grid point before t_off),
    as the suite's first_cross of 0.1 * vled0, linear between grid points."""
    import numpy as np
    i0 = int(np.searchsorted(t, t_off))
    if v0 is None:
        v0 = v[i0 - 1, 0]
    for k in range(i0, len(t)):
        if v[k, 0] < 0.1 * v0:
            f = (v[k - 1, 0] - 0.1 * v0) / (v[k - 1, 0] - v[k, 0])
            return (t[k - 1] + f * (t[k] - t[k - 1]) - t_off) * 1e-15
    return float("inf")


def cmd_gate(a):
    import numpy as np
    d = a.dir
    man = json.load(open(os.path.join(d, "decks", "manifest.json")))
    runs = {}
    for ln in open(os.path.join(d, "runs.jsonl")):
        r = json.loads(ln)
        p = os.path.join(d, "runs", r["id"] + ".csv")
        if os.path.exists(p):
            r["csv_sha256"] = hashlib.sha256(open(p, "rb").read()).hexdigest()
        r.pop("wall_s_note", None)
        runs[r["id"]] = r
    suite = json.load(open(a.suite_results))
    checks = {(c["test"].split("::")[-1], c["check"]): c["value"] for c in suite["checks"]}
    out = {"schema": "bb-dms-s1-summary/1", "gates": [], "e1": {}, "vectors": {}, "runs": runs}
    ok = True

    def gate(name, passed, detail):
        nonlocal ok
        ok &= bool(passed)
        out["gates"].append({"gate": name, "pass": bool(passed), "detail": detail})
        print(f"dms-s1: {'PASS' if passed else 'FAIL'}: {name}: {detail}")

    def csv(rid):
        return os.path.join(d, "runs", rid + ".csv")

    for bench in BENCHES:
        c = f"e1_bench_{bench}"
        eng = f"eng.{c}.fixed.phase0.g1ms"
        r = runs[eng]
        gate(f"{c}: 120 ms through NgspiceEngine at phase0 (fixed view)",
             r["status"] == "ok" and r["reached_fs"] == E1_STOP_US * US,
             f"status {r['status']}, reached {r['reached_fs'] / 1e12:g} ms, {r.get('solver_points')} solver points, "
             f"{r['wall_s']} s")
        if r["status"] != "ok":
            continue
        te, ve = _load(csv(eng))
        for p in ("g10us", "g10ms"):
            other = f"eng.{c}.fixed.phase0.{p}"
            same = open(csv(other), "rb").read() == open(csv(eng), "rb").read()
            gate(f"{c}: grant partition {p[1:]} gives the 1 ms run's samples byte for byte", same,
                 f"{other}.csv vs {eng}.csv")
        # The suite's result: the suite's own (unmodified) view at its fallback option set, standalone, as
        # the suite runs it (tran 20u, TMAX 20 us); also at the engine's TMAX (10 us = the lattice).
        # The converged reference: TMAX 1 us, both views (ADR-006 Amendment 1 explains why the
        # whole-waveform comparison is against the converged run: the suite_fallback run moves by mV on
        # the turn-on ramp knee under a 1-ulp change of the edge times).
        ref10 = f"ref.{c}.unmod.suite_fallback.tmax10us"
        ref20 = f"ref.{c}.unmod.suite_fallback.tran20us"
        conv_f, conv_u = f"ref.{c}.fixed.phase0.tmax1us", f"ref.{c}.unmod.suite_fallback.tmax1us"
        tr, vr = _load(csv(ref10))
        t2, v2 = _load(csv(ref20))
        tc, vc = _load(csv(conv_f))
        tu, vu = _load(csv(conv_u))
        inst = {}
        for nm, tus in (("p006_rise_1", 10_500), ("qod_end_1", 50_600), ("p006_rise_2", 60_600), ("qod_end_2", 120_000)):
            inst[nm] = {"t_ms": tus / 1000, "engine": _at(te, ve, tus * US), "suite_fallback_tran20us": _at(t2, v2, tus * US),
                        "suite_fallback_tmax10us": _at(tr, vr, tus * US), "converged": _at(tu, vu, tus * US)}
        dinst = max(abs(x["engine"] - x["suite_fallback_tran20us"]) for x in inst.values())
        gate(f"{c}: VLED at the E1 check instants (P0.06 rises, end of each QOD wait) within 1 mV of the suite's "
             "suite_fallback result (its view, tran 20u)", dinst <= 1e-3,
             f"max {dinst * 1e3:.4f} mV over {', '.join(inst)}")
        toff = {f"cycle{i + 1}": {"engine_ms": _t_off10(te, ve, t * US) * 1e3,
                                  "suite_fallback_tran20us_ms": _t_off10(t2, v2, t * US) * 1e3,
                                  "converged_ms": _t_off10(tu, vu, t * US) * 1e3}
                for i, t in enumerate((15_600, 65_700))}
        dto = max(abs(x["engine_ms"] - x["suite_fallback_tran20us_ms"]) for x in toff.values())
        gate(f"{c}: LSW-2 time (VLED < 10 % after LED_PWR_EN falls) within one lattice cell (10 us) of the "
             "suite's suite_fallback result", dto <= 0.010,
             f"max {dto * 1e3:.3f} us ({', '.join(f'{k} {x['engine_ms']:.4f} ms' for k, x in toff.items())})")
        dconv = float(np.abs(ve[:, 0] - vu[:, 0]).max())
        k = int(np.abs(ve[:, 0] - vu[:, 0]).argmax())
        gate(f"{c}: VLED at every 10 us within 1 mV of the converged suite_fallback result (its view, TMAX 1 us)",
             dconv <= 1e-3, f"max |dVLED| {dconv * 1e3:.4f} mV at {te[k] / 1e12:g} ms")
        dview = float(np.abs(vc[:, 0] - vu[:, 0]).max())
        gate(f"{c}: the fix does not change results (converged, TMAX 1 us: fixed view phase0 vs suite view "
             "suite_fallback) within 0.1 mV", dview <= 1e-4, f"max |dVLED| {dview * 1e3:.4f} mV")

        def mx(x, y):
            return float(np.abs(x[:, 0] - y[:, 0]).max()) * 1e3
        out["e1"][c] = {
            "check_instants_V": inst, "t_off10": toff,
            "max_dVLED_mV": {"engine_vs_converged": dconv * 1e3,
                             "engine_vs_converged_fixed_phase0": mx(ve, vc),
                             "engine_vs_suite_fallback_tran20us": mx(ve, v2),
                             "engine_vs_suite_fallback_tmax10us": mx(ve, vr),
                             "suite_fallback_tran20us_vs_converged": mx(v2, vu),
                             "suite_fallback_tmax10us_vs_converged": mx(vr, vu),
                             "fixed_vs_unmod_converged": dview * 1e3},
            "max_din_over_V": r["probes"]["n_din_over"]["max"],
        }

    # the suite corners of the E1 test: engine values against the committed results.json (M0)
    for c, test, corner in (("e1_suite_bat3.0_-10", "test_phantom_din_abs_max_with_fw_rule[bat3.0--10]", "bat3.0 -10C"),
                            ("e1_suite_bat4.2_25", "test_phantom_din_abs_max_with_fw_rule[bat4.2-25]", "bat4.2 25C")):
        eng = f"eng.{c}.fixed.phase0.g1ms"
        r = runs[eng]
        gate(f"{c}: 120 ms through NgspiceEngine at phase0 (fixed view)", r["status"] == "ok",
             f"status {r['status']}, reached {r['reached_fs'] / 1e12:g} ms")
        if r["status"] != "ok":
            continue
        te, ve = _load(csv(eng))
        hi = checks[(test, f"VLED when P0.06 first goes high, 10 ms after LED_PWR_EN {corner}")]
        end = checks[(test, f"VLED 35 ms after LED_PWR_EN low (record) {corner}")]
        over = checks[(test, f"max(DIN - VDD) over the documented on/off sequence {corner}")]
        e_hi, e_end = _at(te, ve, 10_500 * US), _at(te, ve, 50_600 * US)
        dd = max(abs(e_hi - hi), abs(e_end - end))
        gate(f"{c}: VLED at 10.5 ms and 50.6 ms within 1 mV of results.json (M0)", dd <= 1e-3,
             f"engine {e_hi:.6f} / {e_end:.7f} V, results.json {hi} / {end} V")
        out["e1"][c] = {"vled_hi": [e_hi, hi], "vled_end": [e_end, end],
                        "overmax": [r["probes"]["n_din_over"]["max"], over]}

    # view vectors at phase0 (report: they inform M5's qualification, not an S1 gate)
    for case in man["cases"]:
        if case["kind"] == "e1":
            continue
        rid = f"eng.{case['name']}.fixed.phase0.g1ms"
        r = runs.get(rid)
        if r is None:
            continue
        rep = {"status": r["status"], "wall_s": r["wall_s"], "solver_points": r.get("solver_points")}
        if r["status"] == "ok" and case["kind"] == "qod":
            t, v = _load(csv(rid))
            t_off = 5000 * US
            v0 = _at(t, v, int(t_off * 0.99) // (10 * US) * 10 * US)   # the suite's vled0 at 0.99 t_off
            rep["t_off10_s"] = [_t_off10(t, v, t_off, v0), checks[(case["suite_test"],
                                f"VLED to 10 % after LED_PWR_EN low {case['name'][4:]}")]]
            rep["vled0"] = v0
        if r["status"] == "ok" and case["kind"] == "table76":
            t, v = _load(csv(rid))
            vin = case["vin"]

            def cross(level):
                for k in range(1, len(t)):
                    if v[k - 1, 0] < level <= v[k, 0]:
                        return (t[k - 1] + (level - v[k - 1, 0]) / (v[k, 0] - v[k - 1, 0]) * (t[k] - t[k - 1])) * 1e-15
                return float("inf")
            t10, t90 = cross(0.1 * vin), cross(0.9 * vin)
            rep["tR_s"] = [t90 - t10, checks[(case["suite_test"], f"tR model @{vin} V")]]
            rep["tON_s"] = [t90 - 100.05e-6, checks[(case["suite_test"], f"tON model @{vin} V")]]
        out["vectors"][case["name"]] = rep
        print(f"dms-s1: vector {case['name']} (report only): {json.dumps(rep)}")

    out["pass"] = ok
    with open(a.summary, "w") as f:
        json.dump(out, f, indent=1, sort_keys=True)
    print(f"dms-s1: {'PASS' if ok else 'FAIL'} ({sum(g['pass'] for g in out['gates'])}/{len(out['gates'])} gates); "
          f"summary {a.summary}")
    sys.exit(0 if ok else 1)


def main():
    p = argparse.ArgumentParser()
    sp = p.add_subparsers(dest="cmd", required=True)
    f = sp.add_parser("fixview")
    f.add_argument("--in", dest="inp", required=True)
    f.add_argument("--out", required=True)
    f.set_defaults(fn=cmd_fixview)
    d = sp.add_parser("decks")
    d.add_argument("--src", required=True)
    d.add_argument("--lib-fixed", required=True)
    d.add_argument("--out", required=True)
    d.set_defaults(fn=cmd_decks)
    r = sp.add_parser("ref")
    r.add_argument("--manifest", required=True)
    r.add_argument("--case", required=True)
    r.add_argument("--view", choices=("unmod", "fixed"), required=True)
    r.add_argument("--options", choices=sorted(OPTION_SETS), required=True)
    r.add_argument("--tran", required=True)
    r.add_argument("--tmax", default="")
    r.add_argument("--out", required=True)
    r.add_argument("--timeout", type=float, default=600)
    r.set_defaults(fn=cmd_ref)
    g = sp.add_parser("gate")
    g.add_argument("--dir", required=True)
    g.add_argument("--summary", required=True)
    g.add_argument("--suite-results", required=True)
    g.set_defaults(fn=cmd_gate)
    a = p.parse_args()
    a.fn(a)


if __name__ == "__main__":
    main()
