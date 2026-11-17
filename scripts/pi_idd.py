#!/usr/bin/env python3
"""Phase 2 milestone P3 (ADR-009 D4, gate G6; scripts/pi_idd.sh): the checks of an
examples/nrf-idd run.

    pi_idd.py idd <run-dir> [--radio AT,EVERY,FOR] [--expect-fail]
        The total VDD current of the nrf_duty.elf gate firmware against a hand
        calculation of the duty-weighted average, within 1 %:
          measured  (V(VSUP) - V(VDD)) / R1 (10 Ohm), nets.csv integrated
                    (trapezoid) from the 4th #run rise to the last one: whole RTC
                    periods, the start-up transient (C1 4.7 uF through 10 Ohm) gone;
          hand      ibase + d_run * idd.run + idd.hfxo [+ d_tx * idd.radio_tx],
                    d_run = 2 * SPIN instructions at 64 MIPS / (PERIOD / 32768 s)
                    (nrf_duty.c: SPIN 7812, PERIOD 32), HFXO on from boot, d_tx the
                    bench [[activity]] schedule's share of the same window; the
                    currents are packages/nordic/nrf52840-qiaa's [power] idd and the
                    idd view's ibase (PS v1.1 section 5.2).
        The duty the #run edges show is reported beside the designed one.
        --expect-fail: a negative control (the fast run, no supply_load): the
        check must fail.
    pi_idd.py static <run-dir> <amps>
        Static-load mode (bench/pi-static.toml): run.json's supply_load lists U1
        replaced by the bench load, no state output is in the events, and the
        current through R1 over the last 40 ms equals the load within 0.01 %.
    pi_idd.py grants <run-dir>   the number of grant records
"""
import csv
import json
import pathlib
import sys
import tomllib

ROOT = pathlib.Path(__file__).resolve().parent.parent
PART = ROOT / "packages/nordic/nrf52840-qiaa/part.toml"
R1 = 10.0
SPIN, PERIOD = 7812, 32
FS_PER_INSTR = 15_625_000           # 64 MIPS (nrf52840-bb@3)
PERIOD_FS = PERIOD * 10**15 / 32768
TOL = 0.01


def amps(v):
    """A current in the package value grammar (only the forms part.toml uses)."""
    for suf, k in (("mA", 1e-3), ("uA", 1e-6), ("nA", 1e-9), ("A", 1.0)):
        if v.endswith(suf):
            return float(v[: -len(suf)]) * k
    raise ValueError(v)


def table():
    p = tomllib.loads(PART.read_text())
    idd = {k: amps(v["typ"]) for k, v in p["power"]["idd"].items()}
    view = next(a for a in p["analog"] if a["name"] == "idd")
    return idd, amps(view["params"]["ibase"])


def events(d):
    return [json.loads(l) for l in open(d / "events.jsonl")]


def edges(ev, net):
    return [(e["t_fs"], e["level"]) for e in ev if e.get("type") == "pin_edge" and e.get("net") == net]


def idd(d, radio, expect_fail):
    rows = list(csv.reader(open(d / "nets.csv")))
    h = rows[0]
    s, v = h.index("VSUP"), h.index("VDD")
    t = [int(r[0]) for r in rows[1:]]
    cur = [(float(r[s]) - float(r[v])) / R1 for r in rows[1:]]
    ev = events(d)
    run = edges(ev, "U1.#run")
    rise = [x for x, l in run if l == 1]
    fall = [x for x, l in run if l == 0]
    hfxo = edges(ev, "U1.#hfxo")
    if len(rise) < 10:
        print(f"pi-idd: {d.name}: {len(rise)} #run rises (the supply_load's #run is not bound?)")
        if expect_fail:
            # The negative control's own verdict: the measurement path below must still fail.
            a, b = t[len(t) // 10], t[-1]
        else:
            return False
    else:
        a, b = rise[3], rise[-1]
    num = 0.0
    for k in range(len(t) - 1):
        lo, hi = max(t[k], a), min(t[k + 1], b)
        if hi > lo:
            num += (cur[k] + cur[k + 1]) / 2 * (hi - lo)
    meas = num / (b - a)
    high = 0
    for r in rise:
        if a <= r < b:
            f = min(x for x in fall if x > r)
            high += min(f, b) - r
    idd_t, ibase = table()
    d_run = 2 * SPIN * FS_PER_INSTR / PERIOD_FS
    hand = ibase + d_run * idd_t["run"] + idd_t["hfxo"]
    terms = [f"{ibase * 1e6:.2f} uA", f"{d_run:.6f} x {idd_t['run'] * 1e3:g} mA", f"{idd_t['hfxo'] * 1e3:g} mA (HFXO)"]
    d_tx = 0.0
    if radio:
        at, every, dur = radio
        on, x = 0, at
        while x < b:
            on += max(0, min(x + dur, b) - max(x, a))
            x += every
        d_tx = on / (b - a)
        hand += d_tx * idd_t["radio_tx"]
        terms.append(f"{d_tx:.6f} x {idd_t['radio_tx'] * 1e3:g} mA (radio_tx)")
    err = (meas - hand) / hand
    ok = abs(err) <= TOL
    if hfxo[:1] and not expect_fail:
        ok &= hfxo[0][1] == 1 and len(hfxo) == 1   # started at boot, never stopped
    periods = len([r for r in rise if a <= r < b])
    res = {"window_ms": [a / 1e12, b / 1e12], "periods": periods, "measured_mA": round(meas * 1e3, 6),
           "hand_mA": round(hand * 1e3, 6), "error_pct": round(err * 100, 4),
           "duty_designed": round(d_run, 6), "duty_run_edges": round(high / (b - a), 6) if rise else None,
           "radio_tx_share": round(d_tx, 6)}
    print(f"pi-idd: {d.name}: measured {meas * 1e3:.6f} mA (VSUP - VDD over R1, {periods} RTC periods, "
          f"{a / 1e12:.3f}-{b / 1e12:.3f} ms); hand {hand * 1e3:.6f} mA = {' + '.join(terms)}; "
          f"error {err * 100:+.3f} % (limit 1 %); duty by the #run edges "
          f"{(high / (b - a)) if rise else float('nan'):.6f} against {d_run:.6f} designed")
    print("json: " + json.dumps(res, sort_keys=True))
    if expect_fail:
        return not ok
    return ok


def static(d, load):
    sl = json.load(open(d / "run.json"))["supply_load"]
    replaced = sl == [{"package": "nordic/nrf52840-qiaa", "ref": "main:U1",
                       "replaced_by": "[[load]] on VDD", "view": "idd"}]
    states = [e for e in events(d) if e.get("type") == "pin_edge" and "#" in e.get("net", "")
              and not e["net"].endswith("#oe")]
    rows = list(csv.reader(open(d / "nets.csv")))
    h = rows[0]
    s, v = h.index("VSUP"), h.index("VDD")
    t_end = int(rows[-1][0])
    cur = [(float(r[s]) - float(r[v])) / R1 for r in rows[1:] if int(r[0]) >= t_end - 40 * 10**12]
    meas = sum(cur) / len(cur)
    err = (meas - load) / load
    ok = replaced and not states and abs(err) <= 1e-4
    print(f"pi-idd: {d.name}: run.json supply_load {'replaced by the bench load' if replaced else sl}; "
          f"{len(states)} state-output edges; current through R1 {meas * 1e3:.6f} mA against the load "
          f"{load * 1e3:.6f} mA ({err * 100:+.4f} %, limit 0.01 %)")
    return ok


def main():
    cmd, d = sys.argv[1], pathlib.Path(sys.argv[2])
    if cmd == "grants":
        print(sum(1 for e in events(d) if e.get("type") == "grant"))
        return
    if cmd == "static":
        sys.exit(0 if static(d, amps(sys.argv[3])) else 1)
    radio = None
    if "--radio" in sys.argv:
        from decimal import Decimal
        parts = sys.argv[sys.argv.index("--radio") + 1].split(",")
        radio = tuple(int(Decimal(p) * 10**15) for p in parts)
    ok = idd(d, radio, "--expect-fail" in sys.argv)
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
