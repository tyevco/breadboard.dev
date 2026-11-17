#!/usr/bin/env python3
"""W2's checks on one run of examples/dial-me-spinner/w2 (Phase 2 P5; plan section 2 W2; ADR-009 D7,
G4, Amendment 9): the S4 brown-out reboot loop on a dead battery, in static-load mode (the suite's
50 mA +3V3 budget in place of U10's supply_load), USB at ILIM 100 mA.

A run is a `bb run` of the project with one W2 image at one bench. Read from it: the `assert` records
(asserts/w2-old.toml or asserts/w2-rule.toml), the `reset` records, the pin_edge records of
OLED_VCC_EN (U10.P1.02), its #oe, LED_PWR_EN (U10.P1.04), LED_DIN_MCU (U10.P0.06) and #bor, the
firmware's console lines (`uart0: w2: boot resetreas=...`), and +3V3, VSYS from nets.csv.

Cases (--case):
  old       the old-rule firmware (GATED): every assertion passes with rung pi; the reset records pair
            up, a hold (`enter`) at ceil_L of each #bor rise and its release (`exit`) at ceil_L of the
            fall; while held every U10 output and #oe is low from the hold on and nothing else changes;
            each boot after a release reads RESETREAS as 0; after a release the firmware enables the
            OLED again (when the run lasts that long); the static-load tolerance against the suite
            (ADR-009 Amendment 1: the VBOR crossing, OLED_VCC_EN to #bor's rise, within 500 us of the
            suite's crossing of the bench's VBOR corner, --vbor); at least one brown-out (`reset`
            records with phase `enter`, decision D-close 1). REPORTED: the number of brown-outs,
            +3V3 and VSYS minima of each OLED attempt (the first against the suite's, which no reset
            ends), and whether the loop repeats
  rule      the rule firmware (GATED): every assertion passes with rung pi (no reset record, +3V3 >=
            2.7 V, OLED_VCC_EN never rises); the static-load tolerances against the suite (Amendment 1:
            VSYS min within 10 mV, +3V3 min within 20 mV of [ILIM100-SYSREG4.5]); the cue rises
  corner    the old-rule firmware at another VBOR corner (GATED: the reset records pair up at #bor's
            crossings and the VBOR crossing against the suite's at that corner, --vbor)

Usage: check_dms_w2.py <run dir> --case <case> [--vbor 1.57|1.60|1.63] [--json FILE]
Exit status 0 when every gated check of the case passes.
"""
import argparse
import json
import sys
from pathlib import Path

L_FS = 10_000_000_000          # the lattice, 10 us
MS = 1_000_000_000_000
VIL_U21 = 0.4                  # TPS22917 ON VIL max (U21, OLED_VCC_EN's load)
# The suite's replica (P0's measurements through NgspiceEngine, ADR-009 Amendment 1;
# examples/dial-me-spinner/pi/static_load.toml [reference.w2old], [reference.w2rule]) and the
# tolerances (Amendment 1 item 4).
OLD = {"vsys_min": 1.4611206, "p3v3_min": 1.4169539,
       "cross_ms": {1.57: 3.8511000, 1.60: 3.8392080, 1.63: 3.8274075}}
RULE = {"vsys_min": 3.2666277, "p3v3_min": 3.2224610}
TOL = {"vsys_min_rule": 0.010, "p3v3_min_rule": 0.020, "cross_ms": 0.500}
LABEL = {"old": "W2 old rule: USB at ILIM 100 mA, dead cell, the suite's 50 mA static +3V3 load",
         "corner": "W2 old rule: USB at ILIM 100 mA, dead cell, the suite's 50 mA static +3V3 load",
         "rule": "W2 rule (NEW-1): USB at ILIM 100 mA, dead cell, the suite's 50 mA static +3V3 load"}


def ceil_l(t):
    return -(-t // L_FS) * L_FS


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("run")
    ap.add_argument("--case", required=True, choices=sorted(LABEL))
    ap.add_argument("--vbor", type=float, default=1.60, help="the bench's VBOR,ON corner (V)")
    ap.add_argument("--json")
    a = ap.parse_args()
    d = Path(a.run)
    ev = [json.loads(x) for x in open(d / "events.jsonl")]
    head = open(d / "nets.csv").readline().strip().split(",")
    rows = [list(map(float, x.split(","))) for x in open(d / "nets.csv").readlines()[1:]]
    t = [r[0] for r in rows]
    col = {h: [r[k] for r in rows] for k, h in enumerate(head) if k}
    case = a.case
    label = LABEL[case] + (f", VBOR,ON {a.vbor:.2f} V" if case != "rule" else "")
    out = {"case": case, "label": label, "checks": [], "report": {}}
    ok = True

    def check(name, cond, text, gated=True):
        nonlocal ok
        verdict = "PASS" if cond else ("FAIL" if gated else "note")
        print(f"check_dms_w2: {verdict}: {name}: {text} [{label}]")
        out["checks"].append({"name": name, "pass": bool(cond), "gated": gated, "text": text})
        if gated and not cond:
            ok = False

    def report(name, text, value=None):
        print(f"check_dms_w2: report: {name}: {text} [{label}]")
        out["report"][name] = value if value is not None else text

    edges = {}
    for r in ev:
        if r["type"] == "pin_edge":
            edges.setdefault(r["net"], []).append((r["t_fs"], r["level"]))
    asserts = {r["name"]: r for r in ev if r["type"] == "assert"}
    resets = [r for r in ev if r["type"] == "reset"]
    boots = [(r["t_fs"], r["msg"]) for r in ev if r["type"] == "log" and "w2: boot resetreas=" in r["msg"]]
    t_end = t[-1]
    failed = sorted(n for n, r in asserts.items() if r["outcome"] != "pass")
    if case in ("old", "rule"):
        check("assertions", asserts and not failed,
              f"all {len(asserts)} pass" if not failed else f"failing: {failed}")
        check("rung", asserts and all(r.get("rung") == "pi" for r in asserts.values()),
              f"every assertion reports rung pi ({sorted({r.get('rung') for r in asserts.values()})})")
    for n, r in sorted(asserts.items()):
        report(f"assert {n}", f"{r['outcome']} (rung {r.get('rung')}): {r.get('msg', '')}", r["outcome"])

    def minimum(name, t0, t1):
        v = col[name]
        idx = [i for i in range(len(t)) if t0 <= t[i] <= t1]
        i = min(idx, key=lambda k: v[k])
        return v[i], t[i]

    if case == "rule":
        check("no reset", not resets, f"{len(resets)} reset records")
        oled = [x for x, lv in edges.get("U10.P1.02", []) if lv == 1]
        check("OLED off", not oled, f"OLED_VCC_EN rises {len(oled)} times")
        cue = [x for x, lv in edges.get("U10.P1.04", []) if lv == 1]
        din = [x for x, lv in edges.get("U10.P0.06", []) if lv == 1]
        check("cue", len(cue) == 1, f"LED_PWR_EN rises at {[x / MS for x in cue]} ms; P0.06 at "
              f"{[x / MS for x in din]} ms")
        got = {"vsys_min": minimum("main:VSYS", 0, t_end), "p3v3_min": minimum("main:+3V3", 0, t_end)}
        for k in ("p3v3_min", "vsys_min"):
            v, at = got[k]
            dlt = v - RULE[k]
            tol = TOL[k + "_rule"]
            check(f"static-load {k}", abs(dlt) <= tol,
                  f"{v:.6f} V at {at / MS:.3f} ms against the suite's [ILIM100-SYSREG4.5] {RULE[k]} V "
                  f"(difference {dlt * 1e3:+.1f} mV; tolerance {tol * 1e3:g} mV, ADR-009 Amendment 1)")
        report("boot", f"{boots}")
    else:
        bor = edges.get("U10.#bor", [])
        rises = [x for x, lv in bor if lv == 1]
        falls = [x for x, lv in bor if lv == 0]
        oled = [x for x, lv in edges.get("U10.P1.02", []) if lv == 1]
        # The reset records pair up at ceil_L of #bor's crossings.
        want = []
        for k, r0 in enumerate(rises):
            want.append((ceil_l(r0), "enter"))
            if k < len(falls):
                want.append((ceil_l(falls[k]), "exit"))
        got = [(r["t_fs"], r["phase"]) for r in resets]
        check("reset records", bool(rises) and got == want and all(
            r.get("cause") == "bor" and (r.get("ram") == "kept") == (r["phase"] == "exit") for r in resets),
            f"{got} against #bor's crossings rising {rises} and falling {falls} fs (each at ceil_L)")
        holds = [(got[k][0], got[k + 1][0] if k + 1 < len(got) else None) for k in range(0, len(got), 2)]
        # While held: every U10 output and #oe is low from the hold (1 fs after it: a change at a lattice
        # point) and nothing changes until the release.
        for h0, h1 in holds:
            end = h1 if h1 is not None else t_end
            moved = [(n, x, lv) for n, es in edges.items() if n.startswith("U10.") and n != "U10.#bor"
                     for x, lv in es if h0 < x <= end and not (x == h0 + 1 and lv == 0)]
            low = all(lv == 0 for n, es in edges.items() if n.startswith("U10.P") for x, lv in es
                      if x == h0 + 1)
            oe = [lv for x, lv in edges.get("U10.P1.02#oe", []) if x == h0 + 1]
            check(f"hold at {h0 / MS:.6f} ms", not moved and low and oe == [0],
                  f"every U10 output and #oe low at the hold (OLED_VCC_EN's #oe {oe}), no change until "
                  f"{end / MS:.6f} ms ({moved[:4]})")
        if case == "old":
            # Each boot after a release reads RESETREAS as 0 (a brown-out leaves no reset reason).
            for h0, h1 in holds:
                if h1 is None or h1 + MS // 4 > t_end:
                    continue
                nxt = [b for b in boots if b[0] > h1]
                first = nxt[0] if nxt else None
                check(f"RESETREAS after the release at {h1 / MS:.6f} ms",
                      first is not None and "resetreas=0x00000000" in first[1],
                      f"the next boot line {first}")
                if h1 + 2 * MS <= t_end:
                    again = [x for x in oled if x > h1]
                    check(f"OLED again after {h1 / MS:.6f} ms", bool(again),
                          f"the reboot enables OLED_VCC_EN again at {[x / MS for x in again[:1]]} ms "
                          "(the uncapped boot repeats)")
            check("boots read RESETREAS 0", boots and all("resetreas=0x00000000" in m for _, m in boots),
                  f"{len(boots)} boot lines, every one RESETREAS 0x00000000")
        # The VBOR crossing against the suite's (static-load mode; Amendment 1's 500 us).
        if oled and rises:
            x = (rises[0] - oled[0]) / MS
            ref = OLD["cross_ms"][round(a.vbor, 2)]
            check("static-load VBOR crossing", abs(x - ref) <= TOL["cross_ms"],
                  f"+3V3 through VBOR,ON {a.vbor:.2f} V (#bor rises) {x:.6f} ms after OLED_VCC_EN, against "
                  f"the suite's {ref} ms (difference {(x - ref) * 1e3:+.1f} us; tolerance 500 us, ADR-009 "
                  "Amendment 1)")
        else:
            check("static-load VBOR crossing", False, f"OLED_VCC_EN rises {oled}, #bor rises {rises}")
        if case == "old":
            n = len(rises)
            # Approver decision D-close 1 (ADR-009 Amendment 12): W2 counts BROWN-OUTS, the holds
            # (`reset` records with phase `enter`), not reset records; at least one.
            enters = sum(1 for r in resets if r["phase"] == "enter")
            check("brown-outs", enters >= 1 and enters == n,
                  f"{enters} brown-out(s) (reset records with phase enter) in {t_end / MS:.1f} ms, "
                  f"{len(resets)} reset records, {n} #bor rises (at least 1, D-close 1)")
            report("brown-outs", f"{n} in {t_end / MS:.1f} ms ({len(resets)} reset records)", n)
            for k, on in enumerate(oled):
                nxt = oled[k + 1] if k + 1 < len(oled) else t_end
                p3, at3 = minimum("main:+3V3", on, nxt)
                vs, atv = minimum("main:VSYS", on, nxt)
                held = any(on < r0 <= nxt for r0 in rises)
                extra = ""
                if k == 0:
                    extra = (f"; the suite (no reset ends its collapse): +3V3 min {OLD['p3v3_min']} V, VSYS "
                             f"min {OLD['vsys_min']} V")
                report(f"attempt {k + 1}", f"OLED_VCC_EN at {on / MS:.6f} ms: +3V3 min {p3:.6f} V at "
                       f"{at3 / MS:.3f} ms, VSYS min {vs:.6f} V at {atv / MS:.3f} ms, "
                       f"{'brown-out' if held else 'no brown-out'}{extra}",
                       {"t_ms": on / MS, "p3v3_min": p3, "vsys_min": vs, "brownout": held})
            if len(oled) >= 2:
                report("loop", "repeats" if n >= 2 else
                       f"does not repeat: the OLED attempt after the first release completes without a "
                       f"brown-out ({n} brown-out in {t_end / MS:.1f} ms)", n >= 2)
    if a.json:
        out["pass"] = ok
        Path(a.json).write_text(json.dumps(out, indent=1, sort_keys=True) + "\n")
    print(f"check_dms_w2: {case}: {'PASS' if ok else 'FAIL'} [{label}]")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
