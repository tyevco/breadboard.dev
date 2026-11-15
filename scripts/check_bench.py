#!/usr/bin/env python3
"""Milestone B1's checks on examples/bench's runs (ADR-010 D2-D4; Phase 2 plan
section 4, track B). Every expected value is computed here from the circuit
and the bench, not read from the run.

    check_bench.py stimulus <out> <bench dir>
    check_bench.py digital <pwl out> <playback out> <btn.csv>
    check_bench.py digital-csv <pwl out>          (print the CSV of the edges)
    check_bench.py trigger <out>
    check_bench.py set <out>
    check_bench.py same <a> <b> [all|nongrant]    (nets.csv, nets.vcd, records)

Exit status 0 when every check passes; each check prints one line.
"""
import csv
import json
import math
import pathlib
import sys
from fractions import Fraction

L = 10_000_000_000  # the lattice, 10 us in fs
MS = 10**12
failed = 0


def ok(cond, what):
    global failed
    print(f"{'ok  ' if cond else 'FAIL'} {what}")
    if not cond:
        failed += 1


def records(out):
    return [json.loads(l) for l in open(pathlib.Path(out) / "events.jsonl")]


def edges(out, net):
    return [(r["t_fs"], r["level"]) for r in records(out) if r["type"] == "pin_edge" and r["net"] == net]


def nets(out):
    rows = list(csv.reader(open(pathlib.Path(out) / "nets.csv")))
    head = rows[0]
    return {int(r[0]): dict(zip(head[1:], r[1:])) for r in rows[1:]}


def v(table, t_ms, net):
    return float(table[round(t_ms * MS)][net])


def ceil_l(t):
    return -(-t // L) * L


def copies(out, src, dst, what):
    """The firmware's EXTI ISR copies src's level to dst (which starts low) at each src edge: dst
    changes in (ceil_L(t), ceil_L(t) + L] whenever the copied level differs (the MCU applies an
    input at ceil_L, the ISR runs in the next slice), and never otherwise."""
    s, d = edges(out, src), edges(out, dst)
    want, level = [], 0
    for t, lv in s:
        if lv != level:
            want.append((t, lv))
            level = lv
    good = len(want) == len(d) and all(
        lv_s == lv_d and ceil_l(ts) < td <= ceil_l(ts) + L for (ts, lv_s), (td, lv_d) in zip(want, d)
    )
    ok(good and len(s) > 0, f"{what}: {len(s)} {src} edges; the {len(want)} that change {dst} do so in (ceil_L(t), ceil_L(t) + L]")


def rc(v0, vinf, tau, t):
    return vinf + (v0 - vinf) * math.exp(-t / tau)


def par(a, b):
    return a * b / (a + b)


# Circuit constants (examples/bench/make_netlist.py and its packages).
VDD, R1, C1, CIN = 3.3, 10e3, 100e-9, 5e-12
RON_SW1, ROFF = 0.05, 100e6
RON_SW2, R4, C3 = 1e3, 1e6, 1e-6
RS_VSW = 0.01
VIH, VIL = 2.0, 0.8


def stimulus(out, bench_dir):
    n = nets(out)
    run = json.load(open(pathlib.Path(out) / "run.json"))
    # ADR-010 G8 (the switch view kind): SW2 open, RC_N is the divider of VSW by roff and R4.
    v_open = VDD * R4 / (R4 + ROFF)
    ok(abs(v(n, 10, "RC_N") - v_open) <= 5e-6, f"G8 open: RC_N at 10 ms {v(n, 10, 'RC_N'):.6f} V = V x R4/(R4 + roff) = {v_open:.6f} V (5 uV)")
    # Pressed at 20 ms: the analytic RC settling of ron (1 kOhm) into C3 (with R4).
    rs = RON_SW2 + RS_VSW
    vinf, tau = VDD * R4 / (R4 + rs), C3 * par(rs, R4)
    worst = max(abs(v(n, t, "RC_N") - rc(v_open, vinf, tau, (t - 20) * 1e-3)) for t in (20.1, 20.5, 21, 22, 25, 30))
    ok(worst <= 1e-3, f"G8 pressed: RC_N at 20.1-30 ms within {worst * 1e6:.0f} uV of {vinf:.6f} V + exp(-t / {tau * 1e3:.6f} ms) (1 mV)")
    v60 = rc(v_open, vinf, tau, 40e-3)
    tau_off = C3 * par(R4, ROFF)
    worst = max(abs(v(n, t, "RC_N") - rc(v60, v_open, tau_off, (t - 60) * 1e-3)) for t in (61, 70, 100, 200))
    ok(worst <= 1e-3, f"G8 released at 60 ms: RC_N decays with tau {tau_off:.6f} s, within {worst * 1e6:.0f} uV (1 mV)")
    # The current profile: 20 mA bursts of 5 ms every 40 ms from 30 ms, through +3V3's 1 Ohm.
    worst = max(abs(v(n, t, "+3V3") - (VDD - 0.020)) for t in (31, 34, 71, 74))
    ok(worst <= 10e-6, f"profile: +3V3 = 3.280 V during the bursts (within {worst * 1e6:.1f} uV, 10 uV)")
    worst = max(abs(v(n, t, "+3V3") - VDD) for t in (29, 36, 50, 69, 76))
    ok(worst <= 10e-6, f"profile: +3V3 = 3.3 V between them (within {worst * 1e6:.1f} uV)")
    # The playback: AIN is an ideal source, so it equals the CSV's linear interpolation.
    pts = [(int(Fraction(t) * 10**15), float(x)) for t, x in
           (l.split(",") for l in (pathlib.Path(bench_dir) / "../data/ain.csv").read_text().split("\n")[1:] if l)]
    def interp(t):
        for (t0, v0), (t1, v1) in zip(pts, pts[1:]):
            if t0 <= t <= t1:
                return v0 + (v1 - v0) * (t - t0) / (t1 - t0)
        return pts[-1][1]
    worst = max(abs(float(row["AIN"]) - interp(t)) for t, row in n.items())
    ok(worst <= 1e-6, f"playback: AIN equals data/ain.csv at all {len(n)} samples (within {worst * 1e6:.2f} uV, 1 uV)")
    # The short: AFILT is held at ground from 260 ms for 10 ms (50 mOhm against R3's 1 kOhm).
    worst = max(abs(v(n, t, "AFILT")) for t in (261, 265, 269.9))
    ok(worst <= 1e-3, f"fault short: AFILT <= {worst * 1e3:.3f} mV during 260-270 ms (1 mV)")
    # After the short, AFILT is back on its periodic response (the triangle's period is 20 ms).
    d = abs(v(n, 279, "AFILT") - v(n, 299, "AFILT"))
    ok(d <= 1e-3, f"fault short: AFILT recovers after 270 ms (279 ms and 299 ms within {d * 1e6:.0f} uV, 1 mV)")
    # The set: VSW at 2.5 V from 250 ms for 20 ms, RC_N follows through SW2 (pressed 240-290);
    # VSW's 10 mOhm carries SW2's few mA (tens of uV).
    worst = max(abs(v(n, t, "VSW") - 2.5) for t in (251, 260, 269.9))
    ok(worst <= 50e-6, f"set: VSW = 2.5 V during 250-270 ms (within {worst * 1e6:.1f} uV, 50 uV)")
    ok(abs(v(n, 269.9, "RC_N") - 2.5 * R4 / (R4 + rs)) <= 1e-3, f"set: RC_N follows VSW through SW2 ({v(n, 269.9, 'RC_N'):.6f} V)")
    ok(abs(v(n, 280, "VSW") - VDD) <= 50e-6, "set: VSW back at 3.3 V after 270 ms")
    # SW1 at 60 ms for 5 ms (C1 still on BTN): the release charges BTN through R1 into C1 + cin.
    pb0 = edges(out, "U1.PB0")
    tau = (C1 + CIN) * par(R1, ROFF)
    vinf = VDD * ROFF / (ROFF + R1)
    t_rise = 65 * MS + round(tau * math.log(vinf / (vinf - VIH)) * 1e15)
    rise = [t for t, l in pb0 if l == 1 and 65 * MS < t < 70 * MS]
    ok(len(rise) == 1 and abs(rise[0] - t_rise) <= 5 * 10**9,
       f"press 60-65 ms: PB0 rises {rise[0] / MS if rise else None} ms, the RC through R1 into C1 gives {t_rise / MS:.6f} ms (5 us)")
    fall = [t for t, l in pb0 if l == 0 and 60 * MS <= t < 61 * MS]
    ok(len(fall) == 1 and fall[0] - 60 * MS <= 100_000_000, f"press 60 ms: PB0 falls within 100 ns ({(fall[0] - 60 * MS) / 1e6 if fall else None} ns)")
    # The timed press 120-180 ms with bounce (offsets in run.json); C1 opens at 150 ms, so after
    # the release at 180 ms every bounce toggle reaches PB0 at once (R1 into cin, tau 50 ns).
    b = {x["phase"]: x["offsets_fs"] for x in run["bench"]["bounce"]}
    ok(len(b.get("press", [])) == 6 and len(b.get("release", [])) == 6 and run["bench"]["seed"] == 7,
       "bounce: 3 bounces (6 toggles) at the press and at the release, seed 7, recorded in run.json")
    ok(all(0 < o < 2 * MS for o in b["press"] + b["release"]), "bounce: every toggle inside the view's bounce_max (2 ms)")
    ctl = [180 * MS] + [180 * MS + o for o in b["release"]]
    after = [(t, l) for t, l in pb0 if 180 * MS <= t <= 183 * MS]
    good = len(after) == len(ctl) and all(0 < t - c <= 2_000_000_000 and l == (1 if k % 2 == 0 else 0)
                                          for k, ((t, l), c) in enumerate(zip(after, ctl)))
    ok(good, f"fault open + bounce: {len(after)} PB0 edges after the release, each within 2 us of its contact change ({len(ctl)} changes)")
    copies(out, "U1.PB0", "U1.PA6", "MCU")
    ok("data" in run["bench"] and len(run["bench"]["data"]) == 2 and "engine" not in run["bench"],
       "run.json: both CSV files hashed; no bench engine (every change is a table)")


def digital_csv(pwl_out):
    """The [[pwl]] run's PB0 edges as canonical CSV (t in seconds, exact): PB0 starts high."""
    e = edges(pwl_out, "U1.PB0")
    lines = ["t,PB0", "0,1"]
    for t, l in e:
        s = f"{t // 10**15}.{t % 10**15:015d}".rstrip("0").rstrip(".")
        lines.append(f"{s},{l}")
    return "\n".join(lines) + "\n"


def digital(pwl_out, play_out, btn_csv):
    want = digital_csv(pwl_out)
    ok(pathlib.Path(btn_csv).read_text() == want, f"data/btn.csv holds the [[pwl]] run's PB0 edge times ({len(edges(pwl_out, 'U1.PB0'))} edges)")
    a, b = edges(pwl_out, "U1.PB0"), edges(play_out, "U1.PB0")
    ok(a == b and len(a) == 8, f"digital playback: the {len(b)} PB0 edges equal the [[pwl]] run's, to the fs (8 edges)")
    drivers = {r["driver"] for r in records(play_out) if r["type"] == "pin_edge" and r["net"] == "U1.PB0"}
    ok(drivers == {"bench.U1.PB0"}, f"digital playback: PB0 is driven by the bench engine ({sorted(drivers)})")
    ok(edges(pwl_out, "U1.PA6") == edges(play_out, "U1.PA6"), "digital playback: PA6's edges equal the [[pwl]] run's")
    copies(pwl_out, "U1.PB0", "U1.PA6", "[[pwl]]")
    copies(play_out, "U1.PB0", "U1.PA6", "playback")


def trigger(out):
    n = nets(out)
    pa5 = edges(out, "U1.PA5")
    tr = next(t for t, l in pa5 if l == 1)
    ok(tr == 50 * MS + 1, f"trigger: PA5 first rises at {tr} fs (50 ms + 1 fs, a write in the slice after 50 ms)")
    pb0 = edges(out, "U1.PB0")
    press = tr + 5 * MS
    fall = [t for t, l in pb0 if l == 0]
    ok(bool(fall) and press < fall[0] <= press + 100_000_000,
       f"trigger: SW1 pressed at PA5 + 5 ms: PB0 falls {(fall[0] - press) / 1e6 if fall else None} ns after it (100 ns)")
    tau = (C1 + CIN) * par(R1, ROFF)
    vinf = VDD * ROFF / (ROFF + R1)
    t_rise = press + 3 * MS + round(tau * math.log(vinf / (vinf - VIH)) * 1e15)
    rise = [t for t, l in pb0 if l == 1]
    ok(bool(rise) and abs(rise[0] - t_rise) <= 5 * 10**9, f"trigger: released 3 ms later, PB0 rises at {rise[0] / MS if rise else None} ms ({t_rise / MS:.6f} ms, 5 us)")
    # VSW's 10 mOhm carries SW2's few mA (tens of uV).
    worst = max(abs(v(n, t, "VSW") - 2.0) for t in (50.1, 55, 59.9))
    ok(worst <= 50e-6, f"trigger: VSW = 2.0 V from PA5's rise for 10 ms (within {worst * 1e6:.1f} uV, 50 uV)")
    ok(abs(v(n, 49.9, "VSW") - VDD) <= 50e-6 and abs(v(n, 60.1, "VSW") - VDD) <= 50e-6, "trigger: VSW at 3.3 V before and after")
    ok(abs(v(n, 59.9, "RC_N") - 2.0 * R4 / (R4 + RON_SW2 + RS_VSW)) <= 1e-3, "trigger: RC_N follows VSW through SW2 (pressed 45-65 ms)")
    run = json.load(open(pathlib.Path(out) / "run.json"))
    ok(run["bench"].get("engine", {}).get("id") == "bench@1", "run.json: the bench engine bench@1 and its content hash")
    copies(out, "U1.PB0", "U1.PA6", "MCU")


def set_(out):
    pb0 = edges(out, "U1.PB0")
    tr = next(t for t, l in edges(out, "U1.PA5") if l == 1)
    want = [(30 * MS, 1), (40 * MS, 0), (tr + 5 * MS, 1), (tr + 6 * MS, 0)]
    ok(pb0 == want, f"set: PB0 edges {pb0} = 30 ms, 40 ms, PA5 + 5 ms, PA5 + 6 ms")
    copies(out, "U1.PB0", "U1.PA6", "MCU")


def same(a, b, mode="nongrant"):
    pa, pb = pathlib.Path(a), pathlib.Path(b)
    for f in ("nets.csv", "nets.vcd"):
        ok((pa / f).read_bytes() == (pb / f).read_bytes(), f"{f} identical: {a} {b}")
    if mode == "all":
        ok((pa / "events.jsonl").read_bytes() == (pb / "events.jsonl").read_bytes(), f"events.jsonl identical: {a} {b}")
    else:
        g = lambda p: [l for l in (p / "events.jsonl").read_text().splitlines() if '"type":"grant"' not in l]
        ok(g(pa) == g(pb), f"non-grant records identical: {a} {b}")


def main(argv):
    cmd = argv[0]
    if cmd == "stimulus":
        stimulus(argv[1], argv[2])
    elif cmd == "digital":
        digital(argv[1], argv[2], argv[3])
    elif cmd == "digital-csv":
        sys.stdout.write(digital_csv(argv[1]))
        return 0
    elif cmd == "trigger":
        trigger(argv[1])
    elif cmd == "set":
        set_(argv[1])
    elif cmd == "same":
        same(*argv[1:])
    else:
        sys.exit(__doc__)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
