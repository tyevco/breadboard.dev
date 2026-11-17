#!/usr/bin/env python3
"""W1's checks on one run of examples/dial-me-spinner/w1 (Phase 2 P4; plan section 2 W1; ADR-009 D6,
G4, Amendments 2 and 8). Every line names the corner it is about: W1 is gated at a LABELLED STRESS
CORNER (decision D-W1: 55 mA static +3V3 load, suite budget 50 mA, RT9080 worst_dropout), and the
suite's 50 mA is a margin check.

A run is a `bb run` of the project with one W1 image (firmware/prebuilt/w1_*.elf) at one bench. Read
from it: the `assert` records (asserts/w1.toml at the stress corner, asserts/w1-margin.toml at the
margin check), the POWER model's `log` records of each EVENTS_POFWARN raise, the pin_edge records of
LED_PWR_EN (U10.P1.04), LED_DIN_MCU (U10.P0.06), #pofen and #pofwarn, and +3V3, VSYS from nets.csv
(the 2.94 V window: linear crossings between the 10 us samples, as scripts/pi_static.py measures it).

Cases (--case):
  rule      the rule firmware at the stress corner (GATED): every assertion passes (cue-held, pof-dip,
            flash-vcc, no-reset), each with rung pi; no EVENTS_POFWARN (the blanking masks the dip);
            P0.06 rises after the blanking; the window is at least 0.5 ms below 2.94 V and recovered
            2 ms before the blanking ends; the static-load tolerances of the stress corner against
            the suite's replica at 55 mA (ADR-009 Amendment 2: +3V3 min and VSYS min within 200 mV,
            the window's start within 50 us, its end within 5 ms); the window-midpoint control's
            premise (Amendment 2): the 4 ms build's re-enable (LED_PWR_EN + 4 ms + its measured
            offset) lies inside this window and more than one lattice cell before its end
  blank4    the window-midpoint control (GATED): exactly cue-held fails; the first EVENTS_POFWARN is
            at ceil_L of the re-enable (the #pofen rise after LED_PWR_EN), the shed caused by the
            re-enable, and before the rule run's window ends (--window-end); LED_PWR_EN falls at it
  off       the blanking-off control (GATED): exactly cue-held fails; the first EVENTS_POFWARN within
            one lattice cell of the window's start (the 2.94 V crossing); LED_PWR_EN falls at it
  blank5    the 5 ms blanking (REPORTED; the suite predicts a pass)
  corner    the rule firmware at the nominal or -5 % comparator corner (REPORTED; POFWARN expected 0)
  margin    the margin check at the suite's 50 mA (GATED: asserts/w1-margin.toml all pass, no
            EVENTS_POFWARN); +3V3 min and its margin to 2.94 V reported
  cosim     the co-simulation (REPORTED: +3V3 and VSYS minima, the window if any, POFWARN count)

Every case first checks (GATED) that the run is at the case's corner, read from its run.json: the
bench file, the static +3V3 [[load]] that replaces U10's supply_load (55 mA for the stress cases,
50 mA for margin, none for cosim), main:U1's corner (worst_dropout) and the POWER comparator's
corner (max, i.e. +5 %; typ or min for `corner`). The label on every line is built from what it
read, so a run at another load cannot pass, or print, as the stress corner.

Usage: check_dms_w1.py <run dir> --case <case> [--window-end-ms X] [--json FILE]
Exit status 0 when every gated check of the case passes.
"""
import argparse
import json
import sys
from pathlib import Path

L_FS = 10_000_000_000          # the lattice, 10 us
MS = 1_000_000_000_000
THR = 2.94                     # V28 x (1 + 5 %)
# ADR-009 Amendment 2, table 1: the suite's replica at 55 mA (P0's measurements) and the stress
# corner's tolerances.
REPLICA = {"vsys_min": 2.8564, "p3v3_min": 2.8078, "start_ms": 3.2341, "end_ms": 6.9472}
TOL = {"vsys_min": 0.200, "p3v3_min": 0.200, "start_ms": 0.050, "end_ms": 5.0}
BLANK_MS = {"rule": 10, "blank4": 4, "blank5": 5, "off": 0, "corner": 10, "margin": 10, "cosim": 10}
# The corner each case must have run at, read back from the run's run.json (bench file, the static
# +3V3 [[load]] that replaces U10's supply_load, the RT9080's corner, the POWER comparator's corner),
# so the label is the run's, not the caller's: (bench files, static load, RT9080 corner, POF corners).
STRESS_AT = (("w1-stress.toml",), "55mA", "worst_dropout", ("max",))
EXPECT = {
    "rule": STRESS_AT, "blank4": STRESS_AT, "off": STRESS_AT, "blank5": STRESS_AT,
    "corner": (("w1-stress-typ.toml", "w1-stress-min.toml"), "55mA", "worst_dropout", ("typ", "min")),
    "margin": (("w1-margin.toml",), "50mA", "worst_dropout", ("max",)),
    "cosim": (("w1-cosim.toml", "w1-cosim-budget.toml"), None, "worst_dropout", ("max",)),
}
POF_TEXT = {"max": "V28 +5 % (2.94 V)", "typ": "V28 nominal (2.80 V)", "min": "V28 -5 % (2.66 V)"}


def run_corner(d):
    """The corner a run ran at, from its run.json: bench file name, the static +3V3 load (the amps of
    a [[load]] on +3V3 that replaces main:U10), its profile if it is one, main:U1's and main:U10's
    corners."""
    import tomllib
    rj = json.load(open(d / "run.json"))
    bench = rj.get("bench") or {}
    text = tomllib.loads(bench.get("text") or "")
    static, profile = None, None
    for ld in text.get("load", []):
        if ld.get("net") in ("+3V3", "main:+3V3"):
            if ld.get("replace") == "main:U10" and "amps" in ld:
                static = ld["amps"]
            elif "profile" in ld:
                profile = ld["profile"]
    corners = text.get("corners", {})
    return {"file": bench.get("file"), "static": static, "profile": profile,
            "u1": corners.get("main:U1"), "pof": (rj.get("pof") or {}).get("corner")}


def corner_label(c):
    pof = POF_TEXT.get(c["pof"], f"corner {c['pof']}")
    if c["static"] is None:
        load = "U10's supply_load" + (f" and the bench [[load]] profile {c['profile']}" if c["profile"] else " only")
        return f"W1 co-simulation: {load}, RT9080 {c['u1']}, POFWARN at {pof}; bench {c['file']}"
    kind = "W1 margin check" if c["static"] == "50mA" else "W1 stress corner"
    return (f"{kind}: {c['static']} static +3V3 load (suite budget 50 mA), RT9080 {c['u1']}, "
            f"POFWARN at {pof}; bench {c['file']}")


def ceil_l(t):
    return -(-t // L_FS) * L_FS


def load(d):
    ev = [json.loads(x) for x in open(d / "events.jsonl")]
    head = open(d / "nets.csv").readline().strip().split(",")
    rows = [list(map(float, x.split(","))) for x in open(d / "nets.csv").readlines()[1:]]
    return ev, head, rows


def window(t, v, thr, t_from):
    """[(start_fs, end_fs or None)] where v < thr after t_from, linear crossings."""
    out, start = [], None
    for i in range(1, len(t)):
        if t[i] < t_from:
            continue
        a, b = v[i - 1], v[i]
        if start is None and a >= thr > b:
            start = t[i - 1] + (a - thr) / (a - b) * (t[i] - t[i - 1])
        elif start is not None and a < thr <= b:
            out.append((start, t[i - 1] + (thr - a) / (b - a) * (t[i] - t[i - 1])))
            start = None
    if start is not None:
        out.append((start, None))
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("run")
    ap.add_argument("--case", required=True, choices=sorted(BLANK_MS))
    ap.add_argument("--blank-ms", type=int, help="the image's blanking (default: by case)")
    ap.add_argument("--window-end-ms", type=float, help="blank4: the rule run's window end after LED_PWR_EN")
    ap.add_argument("--json")
    a = ap.parse_args()
    d = Path(a.run)
    ev, head, rows = load(d)
    case = a.case
    # The label is read from the run (run.json), never given by the caller, and the run must be at
    # the case's corner: a run at another load or corner is not this case.
    rc = run_corner(d)
    label = corner_label(rc)
    blank = (a.blank_ms if a.blank_ms is not None else BLANK_MS[case]) * MS
    out = {"case": case, "label": label, "corner": rc, "checks": [], "report": {}}
    ok = True

    def check(name, cond, text, gated=True):
        nonlocal ok
        verdict = "PASS" if cond else ("FAIL" if gated else "note")
        print(f"check_dms_w1: {verdict}: {name}: {text} [{label}]")
        out["checks"].append({"name": name, "pass": bool(cond), "gated": gated, "text": text})
        if gated and not cond:
            ok = False

    def report(name, text, value=None):
        print(f"check_dms_w1: report: {name}: {text} [{label}]")
        out["report"][name] = value if value is not None else text

    files, static, u1, pofs = EXPECT[case]
    check("corner", rc["file"] in files and rc["static"] == static and rc["u1"] == u1 and rc["pof"] in pofs,
          f"run.json: bench {rc['file']}, static +3V3 load {rc['static']}, RT9080 {rc['u1']}, POF corner "
          f"{rc['pof']} (case {case} expects bench {' or '.join(files)}, static load {static}, RT9080 {u1}, "
          f"POF corner {' or '.join(pofs)})")

    edges = {}
    for r in ev:
        if r["type"] == "pin_edge":
            edges.setdefault(r["net"], []).append((r["t_fs"], r["level"]))
    pof = [r["t_fs"] for r in ev if r["type"] == "log" and r["msg"].startswith("power: EVENTS_POFWARN")]
    asserts = {r["name"]: r for r in ev if r["type"] == "assert"}
    resets = [r for r in ev if r["type"] == "reset"]
    en = edges.get("U10.P1.04", [])
    rises = [t for t, lv in en if lv == 1]
    falls = [t for t, lv in en if lv == 0]
    if not rises:
        check("LED_PWR_EN", False, "LED_PWR_EN never rose")
        sys.exit(1)
    t_e = rises[0]
    din = [t for t, lv in edges.get("U10.P0.06", []) if lv == 1]
    pofen_rise = [t for t, lv in edges.get("U10.#pofen", []) if lv == 1 and t > t_e]
    t = [r[0] for r in rows]
    col = {h: [r[k] for r in rows] for k, h in enumerate(head) if k}
    p3 = col["main:+3V3"]
    win = window(t, p3, THR, t_e)
    i3 = min(range(len(t)), key=lambda i: p3[i] if t[i] >= t_e else 9e9)
    iv = min(range(len(t)), key=lambda i: col["main:VSYS"][i] if t[i] >= t_e else 9e9)
    rel = lambda x: (x - t_e) / MS  # noqa: E731
    report("LED_PWR_EN", f"rises at {t_e / MS:.6f} ms (t_e); the blanking {blank // MS} ms")
    report("+3V3 min", f"{p3[i3]:.6f} V at t_e + {rel(t[i3]):.4f} ms (margin to {THR} V: "
           f"{(p3[i3] - THR) * 1e3:+.1f} mV)", p3[i3])
    report("VSYS min", f"{col['main:VSYS'][iv]:.6f} V at t_e + {rel(t[iv]):.4f} ms", col["main:VSYS"][iv])
    if win:
        s0, e0 = win[0]
        report("2.94 V window", f"t_e + {rel(s0):.4f} .. "
               f"{f'{rel(e0):.4f}' if e0 is not None else 'no recovery'} ms"
               + (f" ({(e0 - s0) / MS:.3f} ms)" if e0 is not None else ""),
               [rel(s0), rel(e0) if e0 is not None else None])
    else:
        report("2.94 V window", "+3V3 never below 2.94 V", None)
    report("EVENTS_POFWARN", f"{len(pof)} raise(s)" + (f", first at {pof[0] / MS:.6f} ms" if pof else ""),
           len(pof))
    for n, r in sorted(asserts.items()):
        report(f"assert {n}", f"{r['outcome']} (rung {r.get('rung')}): {r.get('msg', '')}", r["outcome"])

    failed = sorted(n for n, r in asserts.items() if r["outcome"] != "pass")
    if case in ("rule", "blank4", "off", "margin"):
        check("rung", asserts and all(r.get("rung") == "pi" for r in asserts.values()),
              f"every assertion reports rung pi ({sorted({r.get('rung') for r in asserts.values()})})")
    check("no reset", not resets, f"{len(resets)} reset records", gated=case != "cosim")

    if case == "rule":
        check("assertions", asserts and not failed, f"all {len(asserts)} pass" if not failed else f"failing: {failed}")
        check("no POFWARN", not pof, f"{len(pof)} EVENTS_POFWARN (the 10 ms blanking masks the dip)")
        check("cue frames", len(din) == 1 and din[0] - t_e >= blank,
              f"P0.06 rises {[rel(x) for x in din]} ms after t_e, once, after the blanking")
        if win and win[0][1] is not None:
            s0, e0 = win[0]
            check("window", (e0 - s0) >= MS // 2 and e0 <= t_e + blank - 2 * MS,
                  f"below 2.94 V for {(e0 - s0) / MS:.3f} ms (>= 0.5 ms), recovered at t_e + {rel(e0):.4f} ms "
                  f"(<= blanking - 2 ms = {blank / MS - 2:.0f} ms)")
            got = {"vsys_min": col["main:VSYS"][iv], "p3v3_min": p3[i3], "start_ms": rel(s0), "end_ms": rel(e0)}
            for k in ("p3v3_min", "vsys_min", "start_ms", "end_ms"):
                dlt = got[k] - REPLICA[k]
                unit, f = ("mV", 1e3) if k.endswith("min") else ("us", 1e3) if k == "start_ms" else ("ms", 1)
                check(f"static-load {k}", abs(dlt) <= TOL[k],
                      f"{got[k]:.6f} against the suite's replica at 55 mA {REPLICA[k]} (difference "
                      f"{dlt * f:+.1f} {unit}; tolerance {TOL[k] * f:g} {unit}, ADR-009 Amendment 2)")
            # The window-midpoint control's premise (Amendment 2): its re-enable is t_e + 4 ms plus
            # the offset this build shows between LED_PWR_EN and its own re-enable (10 ms build).
            off = (pofen_rise[0] - t_e - blank) if pofen_rise else 0
            mid = round((s0 + e0) / 2 / MS - t_e / MS)
            t_re = t_e + 4 * MS + off
            report("window midpoint", f"t_e + {((s0 + e0) / 2 - t_e) / MS:.4f} ms: the whole ms nearest is {mid} ms; "
                   f"the re-enable offset {off / 1e6:.3f} ns", mid)
            check("4 ms control premise", mid == 4 and s0 < t_re and t_re + L_FS < e0,
                  f"the 4 ms build re-enables at t_e + {rel(t_re):.6f} ms, inside the window and "
                  f"{(e0 - t_re) / 1e9:.1f} us (> one lattice cell) before its end")
        else:
            check("window", False, "+3V3 never below 2.94 V, or never recovers")
    elif case in ("blank4", "off"):
        check("exactly cue-held", failed == ["cue-held"], f"failing assertions {failed}")
        check("shed", len(falls) == 1 and not din, f"LED_PWR_EN falls at {[x / MS for x in falls]} ms, "
              f"P0.06 rises {len(din)} times")
        if case == "blank4":
            ref = pofen_rise[0] if pofen_rise else None
            check("re-enable", ref is not None and blank <= ref - t_e <= blank + L_FS,
                  f"#pofen rises at t_e + {rel(ref) if ref else None} ms (the blanking's end)")
            if ref is not None and pof:
                check("POFWARN at the re-enable", pof[0] == ceil_l(ref),
                      f"first EVENTS_POFWARN at {pof[0] / MS:.6f} ms = ceil_L(re-enable {ref / MS:.6f} ms): "
                      "the shed caused by the re-enable rule")
                if a.window_end_ms is not None:
                    check("inside the window", rel(pof[0]) < a.window_end_ms,
                          f"at t_e + {rel(pof[0]):.4f} ms, before the rule run's window ends "
                          f"(t_e + {a.window_end_ms:.4f} ms)")
            else:
                check("POFWARN at the re-enable", False, f"no EVENTS_POFWARN ({pof})")
        else:
            if win and pof:
                s0 = win[0][0]
                check("POFWARN at the window's start", 0 <= pof[0] - s0 <= L_FS,
                      f"first EVENTS_POFWARN at {pof[0] / MS:.6f} ms, {(pof[0] - s0) / 1e9:.3f} us after "
                      f"the 2.94 V crossing (t_e + {rel(s0):.4f} ms; limit one lattice cell)")
            else:
                check("POFWARN at the window's start", False, f"window {win}, EVENTS_POFWARN {pof}")
        if pof and falls:
            check("shed at POFWARN", falls[0] == pof[0] + 1,
                  f"LED_PWR_EN falls at {falls[0]} fs, POFWARN at {pof[0]} fs (+1 fs: an edge on a lattice point)")
    elif case == "margin":
        check("assertions", asserts and not failed, f"all {len(asserts)} pass" if not failed else f"failing: {failed}")
        check("no POFWARN", not pof and not win, f"{len(pof)} EVENTS_POFWARN; +3V3 min {p3[i3]:.6f} V "
              f"(margin {(p3[i3] - THR) * 1e3:+.1f} mV)")
    elif case in ("blank5", "corner"):
        report("verdict", "cue held" if not falls else "cue shed")
        if case == "corner":
            report("POFWARN count", f"{len(pof)} (expected 0)", len(pof))
    if a.json:
        out["pass"] = ok
        Path(a.json).write_text(json.dumps(out, indent=1, sort_keys=True) + "\n")
    print(f"check_dms_w1: {case}: {'PASS' if ok else 'FAIL'} [{label}]")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
