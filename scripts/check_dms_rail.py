#!/usr/bin/env python3
"""E1 checks on a `bb run` of examples/dial-me-spinner (Phase 1 plan section 2 E1, M6).

    python3 scripts/check_dms_rail.py <out> [--expect-fail LSW-3] [--project <dir>] [--json <file>]

<out> is a `bb run --project examples/dial-me-spinner ... --out <out>` directory (nets.csv,
events.jsonl, run.json). The checks use the dial-me-spinner suite's LIMITS (hardware/sim at
6854d25), never its measured values, which are reported next to each result when the suite ran
the same corner (examples/dial-me-spinner/suite/results.json):

  LED-DATA-V4  VLED at every instant P0.06 (LED_DIN_MCU) goes high       >= 2.67 V (FW_VLED_FRAME_MIN)
  LSW-3        max(V(LED_DIN) - V(VLED)) over every 10 us sample           <= 0.7 V  (WS2812C DI abs max)
  LSW-2        VLED below 10 % of its value at the LED_PWR_EN fall,       within 30 ms (FW_LED_OFF)
               for every fall followed by 30 ms of run (a later fall is reported as not evaluable)
  FW-6.4       the rule's own timing, from the pin_edge records: every P0.06 rise while LED_PWR_EN
               is high and at least 10 ms after its rise; every LED_PWR_EN rise at least 30 ms after
               the preceding fall (firmware.md section 6.4). The off ORDER (P0.06 low before
               LED_PWR_EN falls) is not part of it: LSW-3 checks its electrical consequence, and the
               wrong-order firmware must fail exactly LSW-3 (post-M7 review)

A failed LSW-3 gives the excursion, where it happened, and the DIN clamp current from the
series-resistor drop: (V(LED_DIN_MCU) - V(LED_DIN_R)) / R40 - V(LED_DIN_R) / R41, with R40 and R41
read from the project's board.bbm (whose sha256 must be run.json's).

Exit status: 0 when every check passes; with --expect-fail LSW-3 (the wrong-order firmware), 0 when
exactly LSW-3 fails and every other check passes. 1 otherwise. Needs only the standard library.
"""
from __future__ import annotations

import argparse
import csv
import hashlib
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULT_PROJECT = os.path.join(HERE, "..", "examples", "dial-me-spinner")

FW_VLED_FRAME_MIN = 2.67  # V: test_load_switches.py, LED-DATA-V4 (+3V3 max 3.366 V - 0.7 V)
DI_ABS_MAX_OVER_VDD = 0.7  # V: WS2812C-2020-V6 DS p.2, VI <= VDD + 0.7 V (LSW-3)
FW_LED_OFF_S = 0.030  # s: firmware.md section 6.4 off-on wait (LSW-2, FW-6.4)
FW_LED_ON_WAIT_S = 0.010  # s: firmware.md section 6.4, LED_PWR_EN high to the first data (FW-6.4)

EN, DIN = "U10.P1.04", "U10.P0.06"  # the bound pins' kernel nets (bb.toml [[bind]])
COL_VLED, COL_DIN, COL_MCU, COL_DIN_R = "VLED", "/LED ring/LED_DIN", "LED_DIN_MCU", "/LED ring/LED_DIN_R"

# The suite's own runs of the same corner (results.json checks), reported, never limits.
SUITE_CORNER = {"cold": "bat3.0 -10C", "nominal": None}
SUITE_QOD = {"cold": "VLED to 10 % after LED_PWR_EN low bat3.0", "nominal": None}


def load_csv(path):
    with open(path, newline="") as f:
        r = csv.reader(f)
        head = next(r)
        cols = {name: [] for name in head}
        for row in r:
            for name, v in zip(head, row):
                cols[name].append(int(v) if name == "t_fs" else float(v))
    return cols


def edges(path, net):
    out = []
    with open(path) as f:
        for ln in f:
            if '"pin_edge"' not in ln:
                continue
            e = json.loads(ln)
            if e["net"] == net:
                out.append((e["t_fs"], e["level"]))
    return out


def at_or_after(ts, t):
    """Index of the first sample at or after t (fs)."""
    lo, hi = 0, len(ts)
    while lo < hi:
        mid = (lo + hi) // 2
        if ts[mid] < t:
            lo = mid + 1
        else:
            hi = mid
    return lo


def suite_values(project):
    try:
        with open(os.path.join(project, "suite", "results.json")) as f:
            return {(c["check"]): c["value"] for c in json.load(f)["checks"]}
    except OSError:
        return {}


def bench_key(run):
    name = os.path.basename(str((run.get("bench") or {}).get("file") or ""))
    for k in ("cold", "nominal"):
        if name.startswith(k):
            return k
    return None


def resistor_ohms(bbm, ref):
    for i in bbm["instances"]:
        if i["ref"] == ref and i.get("value", {}).get("ohm_u") is not None:
            return i["value"]["ohm_u"] / 1e6
    raise SystemExit(f"check_dms_rail: {ref} has no resistance in board.bbm")


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("out")
    ap.add_argument("--expect-fail", choices=["LSW-3"], default=None)
    ap.add_argument("--project", default=DEFAULT_PROJECT)
    ap.add_argument("--json", default=None, help="write the results as JSON")
    a = ap.parse_args()

    run = json.load(open(os.path.join(a.out, "run.json")))
    bbm_bytes = open(os.path.join(a.project, "board.bbm"), "rb").read()
    if "sha256:" + hashlib.sha256(bbm_bytes).hexdigest() != run["board_bbm"]:
        raise SystemExit("check_dms_rail: run.json's board_bbm is not this project's board.bbm")
    bbm = json.loads(bbm_bytes)
    r40, r41 = resistor_ohms(bbm, "R40"), resistor_ohms(bbm, "R41")
    cols = load_csv(os.path.join(a.out, "nets.csv"))
    for c in (COL_VLED, COL_DIN, COL_MCU, COL_DIN_R):
        if c not in cols:
            raise SystemExit(f"check_dms_rail: nets.csv has no column {c!r} (probe it)")
    ts, vled, din = cols["t_fs"], cols[COL_VLED], cols[COL_DIN]
    t_end = ts[-1]
    ev = os.path.join(a.out, "events.jsonl")
    en, dn = edges(ev, EN), edges(ev, DIN)
    bench = bench_key(run)
    suite = suite_values(a.project)
    corner = SUITE_CORNER.get(bench)
    res = {"bench": run.get("bench", {}).get("file"), "options": run.get("bench", {}).get("options"),
           "image": run.get("targets"), "checks": {}}

    def report(name, ok, msg, detail):
        res["checks"][name] = {"pass": ok, **detail}
        print(f"check_dms_rail: {'PASS' if ok else 'FAIL'}: {name}: {msg}")

    # LED-DATA-V4: VLED at each P0.06 rise.
    rises = [t for t, lv in dn if lv == 1]
    vals = []
    for t in rises:
        i = at_or_after(ts, t)
        if 0 < i < len(ts):  # VLED at the edge, linear between the straddling 10 us samples
            v = vled[i - 1] + (vled[i] - vled[i - 1]) * (t - ts[i - 1]) / (ts[i] - ts[i - 1])
            vals.append((t, v, ts[i]))
    ok = bool(vals) and all(v >= FW_VLED_FRAME_MIN for _, v, _ in vals)
    sv = suite.get(f"VLED when P0.06 first goes high, 10 ms after LED_PWR_EN {corner}") if corner else None
    en_rises = [t for t, lv in en if lv == 1]
    waits = []
    for t, _, _ in vals:
        before = [e for e in en_rises if e <= t]
        waits.append((t - before[-1]) / 1e12 if before else None)
    msg = ", ".join(f"VLED {v:.6f} V at {t / 1e12:.6f} ms (P0.06 rose {w:.3f} ms after LED_PWR_EN)"
                    if w is not None else f"VLED {v:.6f} V at {t / 1e12:.6f} ms"
                    for (t, v, _), w in zip(vals, waits)) or "P0.06 never went high"
    msg += f" (limit >= {FW_VLED_FRAME_MIN} V" + (f"; suite {corner}: {sv} V 10 ms after LED_PWR_EN" if sv else "") + ")"
    report("LED-DATA-V4", ok, msg, {"limit_V": FW_VLED_FRAME_MIN, "rises": [
        {"t_fs": t, "vled_V": v, "after_en_ms": w} for (t, v, _), w in zip(vals, waits)], "suite_V": sv})

    # LSW-3: max(DIN - VLED) over the run.
    over = [d - v for d, v in zip(din, vled)]
    k = max(range(len(over)), key=over.__getitem__)
    vm, vr = cols[COL_MCU][k], cols[COL_DIN_R][k]
    i_clamp = (vm - vr) / r40 - vr / r41
    ok = over[k] <= DI_ABS_MAX_OVER_VDD
    sv = suite.get(f"max(DIN - VDD) over the documented on/off sequence {corner}") if corner else None
    msg = (f"max(V(LED_DIN) - V(VLED)) = {over[k]:.6f} V at {ts[k] / 1e12:.3f} ms (VLED {vled[k]:.6f} V, "
           f"LED_DIN {din[k]:.6f} V; DIN clamp current {i_clamp * 1e3:.3f} mA from the R40 drop "
           f"(V(LED_DIN_MCU) {vm:.6f} V - V(LED_DIN_R) {vr:.6f} V) / {r40:g} Ohm - V(LED_DIN_R) / {r41:g} Ohm)"
           f" (limit <= {DI_ABS_MAX_OVER_VDD} V" + (f"; suite {corner}: {sv} V" if sv is not None else "") + ")")
    report("LSW-3", ok, msg, {"limit_V": DI_ABS_MAX_OVER_VDD, "max_din_over_vled_V": over[k], "at_fs": ts[k],
                              "clamp_current_A": i_clamp, "suite_V": sv})

    # LSW-2: VLED below 10 % within 30 ms of each LED_PWR_EN fall.
    falls = [t for t, lv in en if lv == 0]
    window = int(round(FW_LED_OFF_S * 1e15))
    rows, ok = [], bool(falls)
    sq = suite.get(SUITE_QOD[bench]) if SUITE_QOD.get(bench) else None
    for t in falls:
        i0 = at_or_after(ts, t)
        if i0 == 0 or i0 >= len(ts):
            continue
        v_on = vled[i0 - 1]  # the last sample before the fall
        lvl = 0.1 * v_on
        j = next((j for j in range(i0, len(ts)) if vled[j] < lvl), None)
        t10 = None
        if j is not None:
            # linear between the straddling samples
            t10 = ts[j - 1] + (ts[j] - ts[j - 1]) * (vled[j - 1] - lvl) / (vled[j - 1] - vled[j])
        if t10 is not None and t10 - t <= window:
            rows.append({"fall_fs": t, "v_on_V": v_on, "t10_ms": (t10 - t) / 1e12, "pass": True})
        elif t + window <= t_end:
            rows.append({"fall_fs": t, "v_on_V": v_on, "t10_ms": None if t10 is None else (t10 - t) / 1e12,
                         "pass": False})
            ok = False
        else:
            rows.append({"fall_fs": t, "v_on_V": v_on, "t10_ms": None, "pass": None,
                         "note": f"the run ends {(t_end - t) / 1e12:.3f} ms after the fall: not evaluable"})
    evaluated = [r for r in rows if r["pass"] is not None]
    ok = ok and bool(evaluated)
    msg = "; ".join(
        (f"LED_PWR_EN fell at {r['fall_fs'] / 1e12:.6f} ms from VLED {r['v_on_V']:.6f} V: below 10 % after "
         f"{r['t10_ms']:.3f} ms" if r["t10_ms"] is not None else
         f"LED_PWR_EN fell at {r['fall_fs'] / 1e12:.6f} ms: " + r.get("note", "VLED not below 10 % within 30 ms"))
        for r in rows) or "LED_PWR_EN never fell"
    msg += f" (limit {FW_LED_OFF_S * 1e3:g} ms" + (f"; suite bat3.0: {sq * 1e3:.3f} ms" if sq else "") + ")"
    report("LSW-2", ok, msg, {"limit_s": FW_LED_OFF_S, "falls": rows, "suite_s": sq})

    # FW-6.4: the rule's timing from the pin_edge records (the electrical checks above cannot see a
    # 10 ms wait shortened to anything VLED has settled by, about 4 ms).
    on_wait, off_wait = int(round(FW_LED_ON_WAIT_S * 1e15)), int(round(FW_LED_OFF_S * 1e15))
    bad, notes = [], []
    for t in rises:
        prev = [(te, lv) for te, lv in en if te <= t]
        if not prev or prev[-1][1] != 1:
            bad.append(f"P0.06 rose at {t / 1e12:.6f} ms while LED_PWR_EN was low")
        elif t - prev[-1][0] < on_wait:
            bad.append(f"P0.06 rose {(t - prev[-1][0]) / 1e12:.3f} ms after LED_PWR_EN (at {t / 1e12:.6f} ms)")
        else:
            notes.append(f"P0.06 rose {(t - prev[-1][0]) / 1e12:.3f} ms after LED_PWR_EN")
    for t in en_rises:
        before = [f for f in falls if f <= t]
        if not before:
            continue
        if t - before[-1] < off_wait:
            bad.append(f"LED_PWR_EN rose {(t - before[-1]) / 1e12:.3f} ms after it fell (at {t / 1e12:.6f} ms)")
        else:
            notes.append(f"LED_PWR_EN off for {(t - before[-1]) / 1e12:.3f} ms before rising again")
    ok = bool(rises) and not bad
    msg = "; ".join(bad or notes) or "P0.06 never went high"
    msg += f" (limits: P0.06 >= {FW_LED_ON_WAIT_S * 1e3:g} ms after LED_PWR_EN; off >= {FW_LED_OFF_S * 1e3:g} ms)"
    report("FW-6.4", ok, msg, {"on_wait_limit_s": FW_LED_ON_WAIT_S, "off_wait_limit_s": FW_LED_OFF_S,
                               "violations": bad})

    failed = sorted(n for n, c in res["checks"].items() if not c["pass"])
    if a.expect_fail:
        verdict = failed == [a.expect_fail]
        print(f"check_dms_rail: {'PASS' if verdict else 'FAIL'}: expected exactly {a.expect_fail} to fail; "
              f"failed: {', '.join(failed) or 'none'}")
    else:
        verdict = not failed
        print(f"check_dms_rail: {'PASS' if verdict else 'FAIL'}: {', '.join(res['checks'])}"
              + (f" (failed: {', '.join(failed)})" if failed else ""))
    res["failed"], res["verdict"] = failed, verdict
    if a.json:
        with open(a.json, "w") as f:
            json.dump(res, f, indent=1, sort_keys=True)
            f.write("\n")
    return 0 if verdict else 1


if __name__ == "__main__":
    sys.exit(main())
