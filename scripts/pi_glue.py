#!/usr/bin/env python3
"""Phase 2 milestone P2 (ADR-009 D5): the reports of the pi digital_out glue on E1's cold bench.

    pi_glue.py lsw3 <run>   LSW-3 (max V(LED_DIN) - V(VLED), every 10 us sample) over the run and
                            while P0.06 is high; P0.06's (LED_DIN_MCU) high level against +3V3
    pi_glue.py rail <run>   the oe edges (U10.P1.04#oe, U10.P0.06#oe) and each output's level before
                            them (high-Z) and after (driving), against +3V3
    pi_glue.py ioh <run>    LED_PWR_EN's high level under bench/cold-pi-ioh.toml's 0.5 mA load:
                            VOH, VDD (+3V3), the pin current, the PS-VOH-SD assert record

Each prints one line per finding and a JSON summary line (`json: {...}`); with --expect <key>=<value>
pairs it exits 1 when a check fails. Needs only the standard library.
"""
import csv
import json
import sys

FS_MS = 1e12


def load(run):
    rows = list(csv.reader(open(f"{run}/nets.csv")))
    head = rows[0]
    cols = {n: i for i, n in enumerate(head)}
    data = [[int(r[0])] + [float(x) for x in r[1:]] for r in rows[1:]]
    recs = [json.loads(line) for line in open(f"{run}/events.jsonl")]
    return cols, data, recs


def edges(recs, net):
    return [(r["t_fs"], r["level"]) for r in recs if r.get("type") == "pin_edge" and r["net"] == net]


def high_windows(recs, net, t_end):
    out, start = [], None
    for t, lv in edges(recs, net):
        if lv == 1 and start is None:
            start = t
        elif lv == 0 and start is not None:
            out.append((start, t))
            start = None
    if start is not None:
        out.append((start, t_end))
    return out


def col(cols, *names):
    for n in names:
        if n in cols:
            return cols[n]
    raise SystemExit(f"pi_glue.py: none of {names} in nets.csv ({list(cols)})")


def lsw3(run):
    cols, data, recs = load(run)
    din, vled = col(cols, "/LED ring/LED_DIN", "LED_DIN"), col(cols, "VLED")
    mcu, v33 = col(cols, "LED_DIN_MCU"), col(cols, "+3V3")
    t_end = data[-1][0]
    best = max(data, key=lambda r: r[din] - r[vled])
    wins = high_windows(recs, "U10.P0.06", t_end)
    # inside each P0.06 high window, 100 us after the edge (the ramp and R40 settled)
    inside = [r for r in data if any(a + 100_000_000_000 <= r[0] < b for a, b in wins)]
    hi = max(inside, key=lambda r: r[din] - r[vled]) if inside else None
    gap = max((abs(r[v33] - r[mcu]) for r in inside), default=None)
    print(f"lsw3: max V(LED_DIN) - V(VLED) over the run {best[din] - best[vled]:.6f} V at "
          f"{best[0] / FS_MS:.6f} ms")
    if hi:
        print(f"lsw3: while P0.06 is high ({len(wins)} windows): max {hi[din] - hi[vled]:.6f} V at "
              f"{hi[0] / FS_MS:.6f} ms (LED_DIN {hi[din]:.6f} V, VLED {hi[vled]:.6f} V, LED_DIN_MCU "
              f"{hi[mcu]:.6f} V, +3V3 {hi[v33]:.6f} V); |V(+3V3) - V(LED_DIN_MCU)| <= {gap:.6f} V")
    return {
        "lsw3_V": round(best[din] - best[vled], 6),
        "lsw3_at_ms": best[0] / FS_MS,
        "lsw3_p006_high_V": None if hi is None else round(hi[din] - hi[vled], 6),
        "p006_high_minus_rail_max_V": None if gap is None else round(gap, 6),
        "p006_high_windows": len(wins),
    }


def rail(run):
    cols, data, recs = load(run)
    v33 = col(cols, "+3V3")
    out = {}
    for pin, net in (("P1.04", "LED_PWR_EN"), ("P0.06", "LED_DIN_MCU")):
        oe = edges(recs, f"U10.{pin}#oe")
        lv = edges(recs, f"U10.{pin}")
        c = col(cols, net)
        first_oe = oe[0][0] if oe else None
        before = [r[c] for r in data if first_oe is not None and r[0] < first_oe]
        rise = next((t for t, x in lv if x == 1), None)
        after = [r for r in data if rise is not None and rise + 100_000_000_000 <= r[0] < rise + 1_000_000_000_000]
        drop = max((r[v33] - r[c] for r in after), default=None)
        print(f"rail: U10.{pin}#oe edges {[(t / FS_MS, x) for t, x in oe]}; {net} before the first "
              f"oe rise: max {max(before, default=0.0):.6f} V ({len(before)} samples, high-Z); "
              f"0.1-1 ms after its first rise: V(+3V3) - V({net}) <= "
              f"{'n/a' if drop is None else f'{drop:.6f} V'}")
        out[pin] = {
            "oe_edges_fs": [[t, x] for t, x in oe],
            "before_oe_max_V": max(before, default=0.0),
            "high_drop_from_rail_max_V": drop,
        }
    return out


def ioh(run, zout=200.0):
    cols, data, recs = load(run)
    en, v33 = col(cols, "LED_PWR_EN"), col(cols, "+3V3")
    win = [r for r in data if 6 * 10**12 <= r[0] <= 29 * 10**12]
    worst = min(win, key=lambda r: r[en] - r[v33])
    cur = (worst[v33] - worst[en]) / zout
    a = [r for r in recs if r.get("type") == "assert" and r.get("name") == "PS-VOH-SD"]
    verdict = a[0]["outcome"] if a else None
    print(f"ioh: VOH {worst[en]:.6f} V at VDD (+3V3) {worst[v33]:.6f} V, {worst[0] / FS_MS:.3f} ms: "
          f"VOH - VDD = {worst[en] - worst[v33]:.6f} V (limit >= -0.4 V, nRF52840 PS v1.1 6.9.3.1 "
          f"VOH,SD at 0.5 mA); the pin sources (VDD - VOH) / zout {zout:g} Ohm = {cur * 1e3:.4f} mA "
          f"(bench 0.5 mA + R3 100 kOhm + U2 ON); assert PS-VOH-SD: {verdict}")
    return {
        "voh_V": worst[en],
        "vdd_V": worst[v33],
        "voh_minus_vdd_V": round(worst[en] - worst[v33], 6),
        "ioh_mA": round(cur * 1e3, 4),
        "assert": verdict,
    }


def main():
    kind, run = sys.argv[1], sys.argv[2]
    res = {"lsw3": lsw3, "rail": rail, "ioh": ioh}[kind](run)
    print("json: " + json.dumps(res, sort_keys=True))
    bad = 0
    for e in sys.argv[3:]:
        if "<=" in e:
            k, v = e.split("<=")
            ok = res[k] is not None and res[k] <= float(v)
            rel = "<="
        elif ">=" in e:
            k, v = e.split(">=")
            ok = res[k] is not None and res[k] >= float(v)
            rel = ">="
        else:
            k, v = e.split("=")
            ok = str(res[k]) == v
            rel = "=="
        print(f"check {k} {rel} {v}: {'ok' if ok else 'FAIL'} ({res[k]})")
        bad |= not ok
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
