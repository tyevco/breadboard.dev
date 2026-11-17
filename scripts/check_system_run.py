#!/usr/bin/env python3
"""Phase 2 R2 (ADR-013 D2): that a run of examples/dial-me-spinner/e1-system is a system run, so a
pass of its other checks is not vacuous (plan section 7 rule 4).

    python3 scripts/check_system_run.py <out> [--json <file>]

<out> is a `bb run --project examples/dial-me-spinner/e1-system ...` directory. Checks, from its
run.json, deck.cir and nets.csv:

  simulated   run.json records simulated = ["main", "base"]
  base-path   the base board's power path is in the deck: base:U1 (ti/bq25180, view pp, at its
              suite_compat corner) and the base board's VSYS and VBAT capacitors (C2, C3, C4);
              the bench has no VSYS supply (the cell is a [[battery]] at base:VBAT)
  harness     the [[connection]] main:J1 = base:J3 is in run.json's analog list (kind harness) and
              in the deck between main and base nodes, its ground wires on node 0 and base_GND
  grounds     the base board's elements return to base_GND, not node 0 (each board its own
              ground): base:U1's ground port and the battery
  current     the power path carries the load: base:VBAT below the cell's OCV and base:VSYS above
              main:VSYS (the harness drop) at every sample while VLED is above 2.5 V

Exit status 0 when every check passes. Standard library only.
"""
from __future__ import annotations

import argparse
import csv
import json
import os
import re
import sys


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("out")
    ap.add_argument("--json", default=None)
    a = ap.parse_args()
    run = json.load(open(os.path.join(a.out, "run.json")))
    deck = open(os.path.join(a.out, "deck.cir")).read()
    res = {}

    def report(name, ok, msg):
        res[name] = {"pass": bool(ok), "msg": msg}
        print(f"check_system_run: {'PASS' if ok else 'FAIL'}: {name}: {msg}")

    report("simulated", run.get("simulated") == ["main", "base"], f"run.json simulated = {run.get('simulated')}")

    an = {x["ref"]: x for x in run.get("analog", [])}
    u1 = an.get("base:U1", {})
    caps = [r for r in ("base:C2", "base:C3", "base:C4") if r in an]
    bench = run.get("bench", {}).get("text", "")
    vsys_supply = re.search(r'\[\[supply\]\]\s*\nnet\s*=\s*"(main:)?VSYS"', bench)
    battery = re.search(r'\[\[battery\]\]\s*\nnet\s*=\s*"base:VBAT"', bench)
    ok = (u1.get("package") == "ti/bq25180" and u1.get("view") == "pp" and u1.get("corner") == "suite_compat"
          and len(caps) == 3 and not vsys_supply and battery)
    report("base-path", ok, f"base:U1 {u1.get('package')}:{u1.get('view')} at {u1.get('corner')}; "
           f"capacitors {', '.join(caps)}; bench VSYS supply {'present' if vsys_supply else 'none'}; "
           f"[[battery]] at base:VBAT {'yes' if battery else 'no'}")

    h = an.get("main:J1=base:J3", {})
    m = re.search(r"^X_conn1_main_J1_base_J3 (.*) dms_link_harness_wires", deck, re.M)
    nodes = m.group(1).split() if m else []
    # ports A1..A12 then B1..B12; pins 1, 2 VSYS, 3, 4, 8 GND
    ok = (h.get("kind") == "harness" and len(nodes) == 24
          and nodes[0] == nodes[1] == "main_VSYS" and nodes[12] == nodes[13] == "base_VSYS"
          and nodes[2] == nodes[3] == nodes[7] == "0" and nodes[14] == nodes[15] == nodes[19] == "base_GND")
    report("harness", ok, f"{h.get('package')}:{h.get('view')} kind {h.get('kind')}; "
           f"VSYS ends {nodes[0:2] if nodes else '-'} / {nodes[12:14] if nodes else '-'}, "
           f"GND ends {[nodes[i] for i in (2, 3, 7)] if nodes else '-'} / {[nodes[i] for i in (14, 15, 19)] if nodes else '-'}")

    bq = re.search(r"^Xbase_U1 (\S+) (\S+) (\S+) (\S+) TI_BQ25180_PP_PI", deck, re.M)
    bat = re.search(r"^I_bench_bat0 (\S+) base_VBAT ", deck, re.M)
    r0 = re.search(r"^R_bench_bat0_0 base_VBAT (\S+) ", deck, re.M)
    ok = bool(bq and bq.group(4) == "base_GND" and bat and bat.group(1) == "base_GND" and r0
              and r0.group(1) == "base_GND")
    report("grounds", ok, f"base:U1 GND port {bq.group(4) if bq else '-'}, battery returns to "
           f"{bat.group(1) if bat else '-'} / {r0.group(1) if r0 else '-'}")

    with open(os.path.join(a.out, "nets.csv"), newline="") as f:
        r = csv.reader(f)
        head = next(r)
        rows = [[float(x) for x in row] for row in r]
    col = {n: i for i, n in enumerate(head)}
    m = re.search(r'ocv\s*=\s*"([^"]+)"', bench)
    data = {d["file"]: d for d in run.get("bench", {}).get("data", [])}
    on = [x for x in rows if x[col["main:VLED"]] > 2.5]
    drop = [x[col["base:VSYS"]] - x[col["main:VSYS"]] for x in on]
    sag = [x[col["base:VBAT"]] for x in on]
    ocv_v = {"cell-3v0.csv": 3.0, "cell-3v7.csv": 3.7}.get(m.group(1) if m else "", None)
    ok = bool(on) and ocv_v is not None and min(drop) > 0 and max(sag) < ocv_v and m.group(1) in data
    report("current", ok, f"{len(on)} samples with VLED > 2.5 V: base:VSYS - main:VSYS "
           f"{min(drop) * 1e3 if on else 0:.3f}..{max(drop) * 1e3 if on else 0:.3f} mV, base:VBAT "
           f"{min(sag) if on else 0:.6f}..{max(sag) if on else 0:.6f} V below the cell's OCV {ocv_v} V")

    failed = sorted(k for k, v in res.items() if not v["pass"])
    print(f"check_system_run: {'PASS' if not failed else 'FAIL'}: {', '.join(res)}"
          + (f" (failed: {', '.join(failed)})" if failed else ""))
    if a.json:
        with open(a.json, "w") as f:
            json.dump({"checks": res, "failed": failed}, f, indent=1, sort_keys=True)
            f.write("\n")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
