#!/usr/bin/env python3
"""The assertions' cross-check against the Phase 1 check scripts (milestone B2; ADR-010 G3, D10,
Amendment 3): on one run, the `assert` records of events.jsonl against the script's own JSON.

    check_asserts.py e1 <run> <check_dms_rail.json>
    check_asserts.py e2 <run> <check_dms_vsense.json> --case <case>
    check_asserts.py e3 <run> <check_dms_home.txt> [--park]
    check_asserts.py replay <run> <bb-assert.jsonl>

Verdicts must be the script's. Values compare by what both read (Amendment 3, A3.6):
  - exact: everything read from records (edge times and gaps, levels, counts, log fields: the E2
    codes, the E3 latencies) and analog values at an instant (LED-DATA-V4, in uV: the solver value
    at the edge, which the script's linear interpolation of the uV-rounded 10 us samples gives too);
  - an extremum (LSW-3): the assertion reads every accepted solver point, the script the 10 us
    samples, which are a subset, so the assertion's value is at least the script's (less 1 uV for
    the script's difference of two rounded samples), and at most the script's plus the checked
    quantity's change over the script's sample period at its extremum (plus 1 uV);
  - a crossing time (LSW-2's settle time, the shed timing): the script interpolates between (LSW-2)
    or stops at (the shed) its samples; LSW-2 must agree at the script's printed resolution (1 us),
    the shed's crossing must lie in the script's sample period ending at its crossing sample.
`replay`: every record `bb assert` gives again equals the run's own, except the analog kinds,
which replay as not_evaluable.
Exit status 0 when every comparison holds.
"""
import argparse
import csv
import json
import math
import re
import sys

FAILS = []


def say(ok, what):
    print(f"check_asserts: {'ok  ' if ok else 'FAIL'} {what}")
    if not ok:
        FAILS.append(what)


def uv(volts):
    """Half away from zero, as bbcore::volts_to_microvolts."""
    x = volts * 1e6
    return int(math.copysign(math.floor(abs(x) + 0.5), x))


RECORDS = None  # --records: read the assert records from this file (bb assert's), not events.jsonl


def records(run):
    out = {}
    with open(RECORDS or f"{run}/events.jsonl") as f:
        for ln in f:
            if ln.startswith('{"type":"assert"'):
                r = json.loads(ln)
                if r["name"] in out:
                    say(False, f"{r['name']}: two assert records")
                out[r["name"]] = r
    return out


def need(recs, name):
    r = recs.get(name)
    say(r is not None, f"{name}: an assert record")
    return r or {"outcome": None, "value": None}


def verdict(name, rec_ok, script_ok, extra=""):
    say(rec_ok == script_ok, f"{name}: verdict {'pass' if rec_ok else 'fail'} = the script's "
        f"{'pass' if script_ok else 'fail'}{extra}")


def column(run, name):
    ts, vs = [], []
    with open(f"{run}/nets.csv") as f:
        rd = csv.reader(f)
        head = next(rd)
        i = head.index(name)
        for row in rd:
            ts.append(int(row[0]))
            vs.append(float(row[i]))
    return ts, vs


def e1(a):
    recs = records(a.run)
    js = json.load(open(a.script))["checks"]
    passed = lambda n: recs.get(n, {}).get("outcome") == "pass"  # noqa: E731
    # LED-DATA-V4: VLED at the rises, the minimum, exact in uV.
    r = need(recs, "LED-DATA-V4")
    verdict("LED-DATA-V4", passed("LED-DATA-V4"), js["LED-DATA-V4"]["pass"])
    vals = [uv(x["vled_V"]) for x in js["LED-DATA-V4"]["rises"]]
    say(vals and r["value"] == min(vals), f"LED-DATA-V4: value {r['value']} uV = the script's minimum "
        f"{min(vals) if vals else None} uV over {len(vals)} rises (exact)")
    # LSW-3: the extremum.
    r = need(recs, "LSW-3")
    verdict("LSW-3", passed("LSW-3"), js["LSW-3"]["pass"])
    s = uv(js["LSW-3"]["max_din_over_vled_V"])
    ts, din = column(a.run, "/LED ring/LED_DIN")
    _, vled = column(a.run, "VLED")
    q = [uv(d) - uv(v) for d, v in zip(din, vled)]
    k = ts.index(js["LSW-3"]["at_fs"])
    step = max(abs(q[k] - q[k - 1]) if k > 0 else 0, abs(q[k + 1] - q[k]) if k + 1 < len(q) else 0)
    d = (r["value"] or 0) - s
    say(-1 <= d <= step + 1, f"LSW-3: value {r['value']} uV vs the script's {s} uV at {ts[k]} fs: "
        f"{d:+d} uV, within [-1, {step + 1}] uV (every solver point vs the 10 us samples; the "
        f"quantity changes {step} uV in one sample period there)")
    # LSW-2: the worst evaluated settle time, at 1 us.
    r = need(recs, "LSW-2")
    verdict("LSW-2", passed("LSW-2"), js["LSW-2"]["pass"])
    ev = [x for x in js["LSW-2"]["falls"] if x["pass"] is not None]
    ne = len(js["LSW-2"]["falls"]) - len(ev)
    m = re.search(r"\((\d+) evaluated, (\d+) not evaluable", r.get("msg", ""))
    say(m is not None and (int(m.group(1)), int(m.group(2))) == (len(ev), ne),
        f"LSW-2: {len(ev)} falls evaluated and {ne} not evaluable, as the script's")
    t10 = [x["t10_ms"] for x in ev if x["t10_ms"] is not None]
    if t10 and r["value"] is not None:
        sv, av = round(max(t10) * 1e3), round(r["value"] / 1e9)
        say(sv == av, f"LSW-2: settle time {r['value']} fs = {av} us = the script's {max(t10):.6f} ms = "
            f"{sv} us (the script's resolution: it interpolates uV-rounded 10 us samples)")
    else:
        say(not t10 and r["value"] is None, "LSW-2: no settle time in either")
    # FW-6.4: three assertions, exact gaps.
    fw = js["FW-6.4"]
    rl, ron, roff = need(recs, "FW-6.4-level"), need(recs, "FW-6.4-on"), need(recs, "FW-6.4-off")
    verdict("FW-6.4", passed("FW-6.4-level") and passed("FW-6.4-on") and passed("FW-6.4-off"), fw["pass"],
            " (FW-6.4-level, -on and -off together)")
    say(rl["value"] == (0 if fw["rises_while_low"] else 1),
        f"FW-6.4-level: value {rl['value']} (the script: {fw['rises_while_low']} rises while LED_PWR_EN low)")
    if fw["rises_while_low"] == 0 and fw["on_gaps_fs"]:
        say(ron["value"] == min(fw["on_gaps_fs"]),
            f"FW-6.4-on: value {ron['value']} fs = the script's smallest gap {min(fw['on_gaps_fs'])} fs (exact)")
    if fw["off_gaps_fs"]:
        say(roff["value"] == min(fw["off_gaps_fs"]),
            f"FW-6.4-off: value {roff['value']} fs = the script's smallest gap {min(fw['off_gaps_fs'])} fs (exact)")
    else:
        say(roff["outcome"] == "fail" and roff["value"] is None, "FW-6.4-off: no pair in either")


def e2(a):
    recs = records(a.run)
    js = json.load(open(a.script))
    res = js["results"]
    case = a.case
    codes = res["readings"][case]
    base = case.split("-")[0]
    want = None
    if case in ("s1", "s50", "c5") and case in res:
        want = res[case]["pred_codes"]
    elif case.endswith("-none") and base in res:
        vth = res[base]["vth_uv"]
        want = [math.floor(vth * 4096 / 3_600_000 + 0.5)] * len(codes)
    script_ok = not any(f.startswith(f"{case}:") for f in js["failed"])
    if want is not None:
        for k, c in enumerate(codes):
            n = f"E2-code-n{k}"
            r = need(recs, n)
            say(r["value"] == c, f"{n}: value {r['value']} = the run's reading code {c} (exact)")
            verdict(n, r["outcome"] == "pass", c == want[k] and script_ok,
                    f" (the script's prediction {want[k]})")
        r = need(recs, "E2-readings")
        say(r["value"] == len(codes), f"E2-readings: {r['value']} readings, the script's {len(codes)}")
    if case in ("sag", "rec") and case in res:
        sh = res[case]["shed_reading"]
        r = need(recs, "E2-before-shed")
        say(r["value"] == min(codes[:sh]), f"E2-before-shed: value {r['value']} = the smallest code before "
            f"the shed {min(codes[:sh])}")
        verdict("E2-before-shed", r["outcome"] == "pass", script_ok)
        r = need(recs, "E2-shed")
        say(r["value"] == codes[sh], f"E2-shed: value {r['value']} = the shed reading's code {codes[sh]}")
        verdict("E2-shed", r["outcome"] == "pass", script_ok)
    replayed = lambda r: r.get("outcome") == "not_evaluable" and "bb assert replays" in r.get("msg", "")  # noqa: E731
    if case == "sag" and "sag" in res and replayed(recs.get("E2-shed-timing", {})):
        print("check_asserts: skip E2-shed-timing: it reads solver points (not evaluable in a replay)")
    elif case == "sag" and "sag" in res:
        s = res["sag"]
        r = need(recs, "E2-shed-timing")
        verdict("E2-shed-timing", r["outcome"] == "pass", script_ok)
        # The script compares uV-rounded samples (round(v) + e < B, i.e. v < T - 0.5 uV with T the
        # threshold rounded up to whole uV, which the assertion uses on the exact value), at its
        # sample period. So its crossing is the assertion's one half uV further down the ramp,
        # rounded up to its sample grid: within one sample period of the assertion's crossing
        # plus 0.5 uV / slope (the slope from its own samples 1 ms either side).
        period = s["sample_period_fs"]
        t_cross = s["t_fall_fs"] - (r["value"] or 0)
        ts, vs = column(a.run, "VSYS_SENSE")
        k = ts.index(s["t_cross_fs"])
        span = 10**12 // period
        slope = abs(uv(vs[k + span]) - uv(vs[k - span])) / (ts[k + span] - ts[k - span])  # uV per fs
        pred = t_cross + (0.5 / slope if slope > 0 else 0)
        say(abs(s["t_cross_fs"] - pred) <= period,
            f"E2-shed-timing: value {r['value']} fs (crossing at {t_cross} fs) vs the script's "
            f"{s['t_fall_fs'] - s['t_cross_fs']} fs (its first sample below, {s['t_cross_fs']} fs): the crossing "
            f"plus 0.5 uV at the ramp's {slope * 1e12:.3f} uV/ms is {(s['t_cross_fs'] - pred) / 1e9:+.1f} us from "
            f"the script's, within its sample period ({period / 1e9:g} us)")
    others = [n for n, r in recs.items() if r["outcome"] != "pass" and not replayed(r)]
    say(script_ok == (not others), f"{case}: every assertion passes exactly when the script passes "
        f"(failing: {others or 'none'})")


def e3(a):
    recs = records(a.run)
    text = open(a.script).read()
    summ = json.loads(re.search(r"^check_dms_home: summary (.*)$", text, re.M).group(1))
    line = lambda what: re.search(rf"^check_dms_home: (ok  |FAIL) {what}", text, re.M)  # noqa: E731
    lat = line("HOME_N assert latency")
    script_ok = lat is not None and lat.group(1) == "ok  "
    ok3 = all(recs.get(n, {}).get("outcome") == "pass" for n in ("HOME_N-tS", "HOME_N-tACTIVE", "HOME_N-latency"))
    verdict("HOME_N assert latency", ok3, script_ok, " (HOME_N-tS, -tACTIVE and -latency together)")
    r = need(recs, "HOME_N-latency")
    say(r["value"] == summ["assert_latency_fs"], f"HOME_N-latency: value {r['value']} fs = the script's "
        f"{summ['assert_latency_fs']} fs (exact)")
    r = need(recs, "HOME_N-tS")
    say(r["value"] == summ["ts_fs"], f"HOME_N-tS: value {r['value']} fs = the device's tS {summ['ts_fs']} fs")
    r = need(recs, "HOME_N-tACTIVE")
    say(r["value"] == summ["tactive_fs"], f"HOME_N-tACTIVE: value {r['value']} fs = {summ['tactive_fs']} fs")
    if a.park:
        rel = line("HOME_N release latency")
        r = need(recs, "HOME_N-release")
        verdict("HOME_N release latency", r["outcome"] == "pass", rel is not None and rel.group(1) == "ok  ")
        say(r["value"] == summ["release_latency_fs"], f"HOME_N-release: value {r['value']} fs = the script's "
            f"{summ['release_latency_fs']} fs (exact)")


def replay(a):
    recs = records(a.run)
    n = 0
    with open(a.script) as f:
        for ln in f:
            r = json.loads(ln)
            if r["outcome"] == "not_evaluable" and "bb assert replays" in r["msg"]:
                continue
            n += 1
            say(recs.get(r["name"]) == r, f"{r['name']}: bb assert gives the run's record")
    say(n > 0, f"{n} records replayed")


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("mode", choices=["e1", "e2", "e3", "replay"])
    ap.add_argument("run")
    ap.add_argument("script")
    ap.add_argument("--case")
    ap.add_argument("--park", action="store_true")
    ap.add_argument("--records", help="the assert records of `bb assert` (replayed), not the run's")
    a = ap.parse_args()
    global RECORDS
    RECORDS = a.records
    {"e1": e1, "e2": e2, "e3": e3, "replay": replay}[a.mode](a)
    if FAILS:
        print(f"check_asserts: FAIL ({len(FAILS)} comparisons, {a.run})")
        return 1
    print(f"check_asserts: PASS ({a.mode}, {a.run})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
