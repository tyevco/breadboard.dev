#!/usr/bin/env python3
"""W1's ramped static-load sweep with the firmware in the loop (approver decision D-close 5,
2026-11-17; ADR-009 Amendment 12): runs of examples/dial-me-spinner/w1 with the rule firmware
(w1_rule.elf, the 10 ms blanking) at bench/w1-collapse.toml's form, the static +3V3 load ramped in
from 0 A at 0.1 ms to its value at 0.2 ms (a current profile ramp-<L>mA.csv, [[load]] replace =
"main:U10"), one run per load L, each 20 ms at bb's defaults, no assertion file (asserts/w1.toml
labels the 55 mA stress corner).

Per run, read back from its run.json first (GATED): the bench file, the profile load that replaces
main:U10 on +3V3 and its CSV (the file beside the bench, its sha256 the one run.json records), the
load it ramps to (its last row), main:U1's corner (worst_dropout) and the POWER comparator's (max,
V28 +5 % = 2.94 V). Then from nets.csv and events.jsonl: +3V3 and VSYS minima after LED_PWR_EN
rises (t_e), the windows below 2.94 V after t_e (linear crossings between the 10 us samples, as
check_dms_w1.py), EVENTS_POFWARN raises, and whether the cue is shed (LED_PWR_EN falls).

Classes: none (+3V3 never below 2.94 V), recover (below it, and above it again before the blanking
ends at t_e + 10 ms), collapse (still below 2.94 V when the blanking ends).

GATED:
  - every run completed (its operating point solved: the ramp's purpose) and is at its corner;
  - +3V3 stays above 0.5 V in every run (the profile load is a pure sink; above the knee it equals
    the `amps` form's P1 knee, spec/project.md section 3.2);
  - the classes are monotone in the load (none, then recover, then collapse);
  - plan section 2 W1's sweep rows: no crossing at 30 and 50 mA and at every load from 40 to 48 mA;
    cross and recover at 55 mA; collapse at 60 mA and at every load from 59 to 62 mA that collapses
    (59 mA is reported: the cliff's edge), and at 61, 62 and 65 mA;
  - a collapse sheds the cue (the re-enable at the blanking's end raises POFWARN), a recovery keeps it.
REPORTED: each load's minima, window, POFWARN count and shed time; with --gated-50 / --gated-55, the
ramped 50 and 55 mA runs against the gated (unramped) margin and stress-corner runs.

Usage: check_dms_w1_sweep.py --bench-dir <dir> [--gated-50 <run>] [--gated-55 <run>] [--json FILE]
                             <load_mA>=<run dir> ...
Exit status 0 when every gated check passes.
"""
import argparse
import hashlib
import json
import sys
import tomllib
from pathlib import Path

MS = 1_000_000_000_000
THR = 2.94
BLANK = 10 * MS
CLASSES = ("none", "recover", "collapse")
NONE_AT = [30, 50] + list(range(40, 49))
COLLAPSE_AT = [60, 61, 62, 65]
RECOVER_AT = [55]


def window(t, v, thr, t_from):
    """[(start_fs, end_fs or None)] where v < thr after t_from, linear crossings (check_dms_w1.py)."""
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


def measure(d):
    ev = [json.loads(x) for x in open(d / "events.jsonl")]
    lines = open(d / "nets.csv").readlines()
    head = lines[0].strip().split(",")
    rows = [list(map(float, x.split(","))) for x in lines[1:]]
    t = [r[0] for r in rows]
    col = {h: [r[k] for r in rows] for k, h in enumerate(head) if k}
    edges = {}
    for r in ev:
        if r["type"] == "pin_edge":
            edges.setdefault(r["net"], []).append((r["t_fs"], r["level"]))
    rises = [x for x, lv in edges.get("U10.P1.04", []) if lv == 1]
    falls = [x for x, lv in edges.get("U10.P1.04", []) if lv == 0]
    t_e = rises[0] if rises else None
    pof = [r["t_fs"] for r in ev if r["type"] == "log" and "EVENTS_POFWARN" in r.get("msg", "")]
    out = {"t_e": t_e, "pofwarn": pof, "shed": [x for x in falls if t_e is not None and x > t_e]}
    p3 = col["main:+3V3"]
    out["p3v3_min_all"] = min(p3)
    if t_e is not None:
        idx = [i for i in range(len(t)) if t[i] >= t_e]
        for key, name in (("p3v3", "main:+3V3"), ("vsys", "main:VSYS")):
            i = min(idx, key=lambda k: col[name][k])
            out[f"{key}_min"], out[f"{key}_min_at"] = col[name][i], t[i]
        out["windows"] = window(t, p3, THR, t_e)
    return out


def classify(m):
    w = m.get("windows") or []
    if not w:
        return "none"
    s, e = w[0]
    return "recover" if e is not None and e < m["t_e"] + BLANK else "collapse"


def corner(d, bench_dir):
    rj = json.load(open(d / "run.json"))
    b = rj.get("bench") or {}
    text = tomllib.loads(b.get("text") or "")
    prof = [ld for ld in text.get("load", []) if ld.get("net") in ("+3V3", "main:+3V3")
            and ld.get("replace") == "main:U10" and "profile" in ld]
    if len(prof) != 1 or any("amps" in ld for ld in text.get("load", [])):
        return None, f"no single ramped profile load replacing main:U10 in {b.get('file')}"
    name = prof[0]["profile"]
    rec = [x for x in b.get("data", []) if x.get("file") == name]
    path = bench_dir / name
    if not rec or not path.exists():
        return None, f"profile {name}: not recorded in run.json or not in {bench_dir}"
    sha = "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest()
    if sha != rec[0]["sha256"]:
        return None, f"profile {name}: {path} is not the file the run read ({sha} != {rec[0]['sha256']})"
    rows = [x.split(",") for x in path.read_text().splitlines() if x and not x.startswith("#")][1:]
    pts = [(float(a), float(v)) for a, v in rows]
    ramp_ok = len(pts) == 3 and pts[0] == (0.0, 0.0) and pts[1] == (0.0001, 0.0) and pts[2][0] == 0.0002
    corners = text.get("corners", {})
    c = {"file": b.get("file"), "profile": name, "amps": pts[-1][1], "ramp_ok": ramp_ok,
         "u1": corners.get("main:U1"), "pof": (rj.get("pof") or {}).get("corner")}
    return c, None


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("runs", nargs="+", help="<load_mA>=<run dir>")
    ap.add_argument("--bench-dir", required=True, type=Path)
    ap.add_argument("--gated-50", type=Path)
    ap.add_argument("--gated-55", type=Path)
    ap.add_argument("--json")
    a = ap.parse_args()
    ok = True
    out = {"loads": {}, "checks": []}

    def check(name, cond, text):
        nonlocal ok
        print(f"check_dms_w1_sweep: {'PASS' if cond else 'FAIL'}: {name}: {text}")
        out["checks"].append({"name": name, "pass": bool(cond), "text": text})
        ok = ok and bool(cond)

    def report(text):
        print(f"check_dms_w1_sweep: report: {text}")

    cls = {}
    for spec in a.runs:
        ma, d = spec.split("=", 1)
        ma, d = int(ma), Path(d)
        if not (d / "events.jsonl").exists():
            check(f"{ma} mA runs", False, f"no outputs in {d} (the operating point or the run failed)")
            continue
        c, why = corner(d, a.bench_dir)
        good = c is not None and round(c["amps"] * 1000, 6) == ma and c["ramp_ok"] \
            and c["u1"] == "worst_dropout" and c["pof"] == "max"
        check(f"{ma} mA corner", good, why or (f"bench {c['file']}, {c['profile']} ramps 0 -> {c['amps'] * 1e3:g} mA "
                                                f"over 0.1-0.2 ms (replace main:U10), RT9080 {c['u1']}, POFWARN "
                                                f"at corner {c['pof']} (V28 +5 %, 2.94 V)"))
        m = measure(d)
        k = classify(m) if m["t_e"] is not None else "no cue"
        cls[ma] = k
        check(f"{ma} mA above the knee", m["p3v3_min_all"] > 0.5,
              f"+3V3 min {m['p3v3_min_all']:.6f} V over the run (> 0.5 V: the profile equals the amps form)")
        if m["t_e"] is None:
            check(f"{ma} mA cue", False, "LED_PWR_EN never rises")
            continue
        w = m["windows"]
        te = m["t_e"]
        wt = ", ".join(f"{(s - te) / MS:.4f}-" + ("open" if e is None else f"{(e - te) / MS:.4f}") + " ms"
                       for s, e in w) or "none"
        shed = m["shed"][0] if m["shed"] else None
        report(f"{ma} mA: {k}; +3V3 min {m['p3v3_min']:.6f} V at t_e + {(m['p3v3_min_at'] - m['t_e']) / MS:.3f} ms, "
               f"VSYS min {m['vsys_min']:.6f} V; below 2.94 V at t_e + {wt}; EVENTS_POFWARN {len(m['pofwarn'])}"
               + (f", cue shed at t_e + {(shed - m['t_e']) / MS:.6f} ms" if shed else ", cue held"))
        out["loads"][str(ma)] = {"class": k, "p3v3_min": m["p3v3_min"], "vsys_min": m["vsys_min"],
                                 "windows_ms": [[(s - m["t_e"]) / MS, None if e is None else (e - m["t_e"]) / MS]
                                                for s, e in w],
                                 "pofwarn": len(m["pofwarn"]),
                                 "shed_ms": None if shed is None else (shed - m["t_e"]) / MS}
        if k == "collapse":
            check(f"{ma} mA sheds", shed is not None and bool(m["pofwarn"]) and m["pofwarn"][0] >= m["t_e"] + BLANK,
                  "the re-enable at the blanking's end raises POFWARN and sheds the cue"
                  f" (POFWARN at {[(x - m['t_e']) / MS for x in m['pofwarn'][:1]]} ms after t_e)")
        else:
            check(f"{ma} mA keeps the cue", shed is None and not m["pofwarn"],
                  f"no EVENTS_POFWARN ({len(m['pofwarn'])}), the cue held")
    loads = sorted(cls)
    order = [CLASSES.index(cls[x]) if cls[x] in CLASSES else -1 for x in loads]
    check("monotone", all(x >= 0 for x in order) and order == sorted(order),
          " ".join(f"{x}:{cls[x]}" for x in loads))
    for want, at in (("none", NONE_AT), ("recover", RECOVER_AT), ("collapse", COLLAPSE_AT)):
        got = {x: cls.get(x) for x in at}
        check(f"{want} at {at[0] if len(at) == 1 else at}", all(v == want for v in got.values()),
              ", ".join(f"{x} mA {v}" for x, v in sorted(got.items())))
    if 59 in cls:
        report(f"59 mA (the cliff's edge, not gated): {cls[59]}")
    for ma, g in ((50, a.gated_50), (55, a.gated_55)):
        if g and str(ma) in out["loads"]:
            gm = measure(g)
            r = out["loads"][str(ma)]
            report(f"{ma} mA ramped against the gated run ({g.name}, the amps form from t = 0): +3V3 min "
                   f"{r['p3v3_min']:.6f} V against {gm['p3v3_min']:.6f} V ({(r['p3v3_min'] - gm['p3v3_min']) * 1e3:+.4f} mV), "
                   f"VSYS min {r['vsys_min']:.6f} V against {gm['vsys_min']:.6f} V "
                   f"({(r['vsys_min'] - gm['vsys_min']) * 1e3:+.4f} mV)")
    out["pass"] = ok
    if a.json:
        Path(a.json).write_text(json.dumps(out, indent=1, sort_keys=True) + "\n")
    print(f"check_dms_w1_sweep: {'PASS' if ok else 'FAIL'} ({len(cls)} loads)")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
