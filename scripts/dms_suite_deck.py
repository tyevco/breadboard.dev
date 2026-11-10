"""Phase 1 plan M6, the local cross-check of E1 (plan section 2 E1, "Cross-check (local, M6)").

The suite's own `led_rail_sequence` deck, built by its functions at the reference commit (a `git archive`
export of dial-me-spinner 6854d25), with two replacements and nothing else:

  * the power path: the SAME bench supply as the co-simulation (the bench TOML of
    examples/dial-me-spinner/bench): the suite's ideal_vsys idiom (an ideal VSYS source) behind the
    bench's series resistance, its DC load, and its tracking +3V3 (min(volts, V(VSYS) - drop), behind the
    bench's rs), in place of the suite's vsys_network (BQ25180, pack, harness, base-board capacitors);
  * the two PWL control sources (VEN, VDIN): the run's own `pin_edge` records of LED_PWR_EN (U10.P1.04)
    and P0.06 (U10.P0.06), each edge a linear ramp of the bound pin model's rise or fall time.

The suite's pad models (LSW_NRF_GPIO), U2's view and the LED ring stay the suite's. Optionally
(--battery) the power path is left as the suite wrote it, which measures what the bench leaves out.

    deck    --src <export> --bench <bench.toml> --events <events.jsonl> --view <lib>:<subckt>
            [--battery] [--options phase0|suite_fallback] [--tran 10u] [--tmax 10u] --stop <s> --out <deck.cir>
    run     --deck <deck.cir> --out <csv> [--timeout s]    ngspice -b; VLED on the run's 10 us grid,
                                                          linear between solver points (one JSON line)
    compare --replay <csv> --run <nets.csv> [--limit-mv 5] [--label s]   max |dVLED| (one JSON line;
                                                          exit 1 over the limit when --gate)

The decks hold the suite's models and, with the vendor-derived view, TI-derived text, so they are written
outside the repository (scripts/dms_crosscheck.sh uses ~/.cache/bb-dms-crosscheck).
"""
from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
import time
import tomllib

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import dms_s1  # noqa: E402  (OPTION_SETS, _body, _Captured)
import dms_m5b  # noqa: E402  (shim: a view as the suite's LSW_TPS22917)

US = 1_000_000_000  # fs per us
EN, DIN = "U10.P1.04", "U10.P0.06"
SEQ_NODE = {EN: "n_seq_en", DIN: "n_seq_din"}
CORNER = {"cold": ("bat3.0", -10), "nominal": ("bat3.7", 25)}


def value(text: str, unit: str) -> float:
    """A bench value in the value grammar subset the benches use (3.0V, 0.5692, 50mA, 44.167mV, 10m)."""
    m = re.fullmatch(r"\s*([-+]?[0-9.]+(?:e[-+]?[0-9]+)?)\s*(m|u|n|k)?\s*" + unit + r"?\s*", text)
    if not m:
        raise SystemExit(f"dms_suite_deck: cannot read {text!r} as {unit}")
    return float(m.group(1)) * {None: 1.0, "m": 1e-3, "u": 1e-6, "n": 1e-9, "k": 1e3}[m.group(2)]


def bench_power(bench: dict):
    """(vsys volts, vsys rs, load amps, 3v3 volts, 3v3 drop, 3v3 rs, temp) from a bb-bench/0 table."""
    sup = {s["net"]: s for s in bench["supply"]}
    vs, p3 = sup["VSYS"], sup["+3V3"]
    load = sum(value(ld["amps"], "A") for ld in bench.get("load", []) if ld["net"] == "VSYS")
    return (value(vs["volts"], "V"), value(vs.get("rs", "0"), ""), load, value(p3["volts"], "V"),
            value(p3["track"]["drop"], "V"), value(p3.get("rs", "0"), ""), float(bench["temp_c"]))


def edges_of(events: str):
    out = {EN: [], DIN: []}
    with open(events) as f:
        for ln in f:
            if '"pin_edge"' in ln:
                e = json.loads(ln)
                if e["net"] in out:
                    out[e["net"]].append((e["t_fs"], e["level"]))
    return out


def pwl(edges, ramp_fs):
    pts, v = [(0, 0)], 0
    for t, lv in edges:
        pts.append((t, v))
        v = 1 if lv else 0
        pts.append((t + ramp_fs, v))
    return " ".join(f"{t}e-15 {lv}" for t, lv in pts)


def cmd_deck(a):
    sim = os.path.join(a.src, "hardware", "sim")
    sys.path.insert(0, sim)
    sys.path.insert(0, os.path.join(sim, "tests"))
    import spicelib  # noqa: E402
    import test_load_switches as tls  # noqa: E402

    bench = tomllib.load(open(a.bench, "rb"))
    name = os.path.basename(a.bench).split(".")[0]
    corner, temp = CORNER[name]
    v, rs, load, v3, drop, rs3, temp_c = bench_power(bench)
    assert temp_c == temp, (temp_c, temp)
    got = {}

    def fake_run(deck, need=(), timeout=90):
        got["text"] = deck.text()
        raise dms_s1._Captured()

    def bench_vsys(deck, main, base, corner="bat3.7", temp=25, i3v3=tls.I_3V3, **kw):
        # the suite's ideal_vsys idiom (VSYSI) behind the bench's series R, its load and its tracking +3V3
        deck += f"VSYSI n_vbench 0 {v!r}"
        deck += f"RBENCH n_vbench n_vsys {rs!r}"
        deck += main.passives(nets={"VSYS", "GND", "VSYS_SENSE"})
        deck += f"IBENCH n_vsys 0 DC {load!r}"
        if rs3 > 0:
            deck += f"B3V3 n_bench_3v3 0 V = min({v3!r}, V(n_vsys) - {drop!r})"
            deck += f"R3V3 n_bench_3v3 n_p3v3 {rs3!r}"
        else:
            deck += f"B3V3 n_p3v3 0 V = min({v3!r}, V(n_vsys) - {drop!r})"
        return deck

    lib, subckt = a.view.rsplit(":", 1)
    view_path = os.path.join(os.path.dirname(os.path.abspath(a.out)), "view." + os.path.basename(a.out) + ".lib")
    with open(view_path, "w", encoding="utf-8") as f:
        f.write(dms_m5b.shim(open(lib, encoding="utf-8").read(), subckt))
    tls.run, orig_lib, orig_vsys = fake_run, tls.LIB_SW, tls.vsys_network
    tls.LIB_SW = view_path
    if not a.battery:
        tls.vsys_network = bench_vsys
    try:
        tls.led_rail_sequence(spicelib.Design("main"), spicelib.Design("base"), corner, temp)
    except dms_s1._Captured:
        pass
    finally:
        tls.LIB_SW, tls.vsys_network = orig_lib, orig_vsys
    body = dms_s1._body(got["text"], ("VEN", "VDIN"))
    ed = edges_of(a.events)
    # each edge a ramp of the bound pin model's rise/fall time (examples/dial-me-spinner bb.toml: P1.04 set
    # S0S1, 13 ns; P0.06 set H0H1_max, 5 ns; nordic/nrf52840-qiaa pins.toml)
    body += f"VEN n_seq_en 0 PWL({pwl(ed[EN], 13_000_000)})\n"
    body += f"VDIN n_seq_din 0 PWL({pwl(ed[DIN], 5_000_000)})\n"
    body += dms_s1.OPTION_SETS[a.options] + "\n"
    body += (".control\nset filetype=ascii\nset wr_singlescale\nset wr_vecnames\n"
             f"tran {a.tran} {a.stop} 0 {a.tmax}\nwrdata __WAV__ v(n_vled)\nquit\n.endc\n.end\n")
    with open(a.out, "w", encoding="utf-8") as f:
        f.write(f"* dms_suite_deck: led_rail_sequence {corner} {temp}C, bench {name}"
                f"{' (the suite battery network)' if a.battery else ''}, view {subckt}, edges from the run\n" + body)
    print(json.dumps({"deck": a.out, "corner": corner, "temp": temp, "bench": name, "battery": a.battery,
                      "edges": {k: len(x) for k, x in ed.items()}}))


def cmd_run(a):
    import numpy as np
    work = tempfile.mkdtemp(prefix="bb-dms-xc-")
    wav = os.path.join(work, "w.txt")
    deck = os.path.join(work, "deck.cir")
    with open(deck, "w") as f:
        f.write(open(a.deck).read().replace("__WAV__", wav))
    t0 = time.monotonic()
    status = "ok"
    try:
        r = subprocess.run(["ngspice", "-b", deck], capture_output=True, text=True, timeout=a.timeout, cwd=work)
        if r.returncode:
            status = f"exit {r.returncode}"
        if re.search(r"(?i)timestep too small", r.stdout + r.stderr):
            status = "timestep too small"
    except subprocess.TimeoutExpired:
        status = f"timeout {a.timeout:g} s"
    res = {"deck": a.deck, "status": status, "wall_s": round(time.monotonic() - t0, 3)}
    if status == "ok" and os.path.exists(wav):
        d = np.loadtxt(wav, skiprows=1, ndmin=2)
        t, v = d[:, 0], d[:, 1]
        stop = int(round(t[-1] * 1e15))
        grid = np.arange(0, stop + 1, 10 * US)
        vv = np.interp(grid * 1e-15, t, v)
        with open(a.out, "w") as f:
            f.write("t_fs,VLED\n")
            for g, x in zip(grid, vv):
                f.write(f"{g},{x:.9e}\n")
        res.update(reached_fs=stop, solver_points=len(t))
    print(json.dumps(res))
    return 0 if status == "ok" else 1


def _load(path, col):
    import numpy as np
    with open(path) as f:
        head = f.readline().strip().split(",")
    d = np.loadtxt(path, delimiter=",", skiprows=1, ndmin=2)
    return d[:, 0].astype(np.int64), d[:, head.index(col)]


def cmd_compare(a):
    import numpy as np
    tr, vr = _load(a.replay, "VLED")
    tc, vc = _load(a.run, "VLED")
    n = min(len(tr), len(tc))
    assert (tr[:n] == tc[:n]).all(), "the two grids differ"
    dv = np.abs(vr[:n] - vc[:n])
    k = int(dv.argmax())
    res = {"label": a.label, "max_dVLED_mV": float(dv[k]) * 1e3, "at_ms": float(tc[k]) / 1e12,
           "VLED_run_V": float(vc[k]), "VLED_replay_V": float(vr[k]), "samples": n,
           "limit_mV": a.limit_mv if a.gate else None}
    ok = (not a.gate) or res["max_dVLED_mV"] <= a.limit_mv
    res["pass"] = ok if a.gate else None
    print(json.dumps(res))
    return 0 if ok else 1


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    d = sub.add_parser("deck")
    d.add_argument("--src", required=True)
    d.add_argument("--bench", required=True)
    d.add_argument("--events", required=True)
    d.add_argument("--view", required=True)
    d.add_argument("--battery", action="store_true")
    d.add_argument("--options", default="phase0", choices=sorted(dms_s1.OPTION_SETS))
    d.add_argument("--tran", default="10u")
    d.add_argument("--tmax", default="10u")
    d.add_argument("--stop", default="120m")
    d.add_argument("--out", required=True)
    r = sub.add_parser("run")
    r.add_argument("--deck", required=True)
    r.add_argument("--out", required=True)
    r.add_argument("--timeout", type=float, default=300)
    c = sub.add_parser("compare")
    c.add_argument("--replay", required=True)
    c.add_argument("--run", required=True)
    c.add_argument("--limit-mv", type=float, default=5.0)
    c.add_argument("--gate", action="store_true")
    c.add_argument("--label", default="")
    a = ap.parse_args()
    return {"deck": cmd_deck, "run": cmd_run, "compare": cmd_compare}[a.cmd](a) or 0


if __name__ == "__main__":
    sys.exit(main())
