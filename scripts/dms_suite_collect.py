#!/usr/bin/env python3
"""Assemble examples/dial-me-spinner/suite/results.json from one run of scripts/dms_suite.sh.

    dms_suite_collect.py --suite-results <export>/hardware/sim/results/results.json \
        --log <BB_DMS_LOG dir> --meta <meta.json> --out <results.json>

Inputs: the suite's own results.json (every check() with value and limits, in collection order),
the per-process records of scripts/dms_suite_plugin.py, and meta.json (commit, tool versions,
selection) written by dms_suite.sh. The output contains no wall times, paths, process ids, worker
counts or dates, so two runs of the same suite with the same tools give the same bytes.

Fallback level of a deck: the suite retries a deck with looser numerics only on "Timestep too
small" or a hang (spicelib.NUMERIC_FALLBACKS and test_load_switches.run(): reltol 1e-3, then also
Gear and rshunt 1e12). `fallback_level` is how many of those steps the deck took: 0 means it ran
with its own options, 1 with reltol 1e-3, 2 with reltol 1e-3 + Gear + rshunt 1e12. `options`
gives the absolute option lines of the attempt whose results the test used (the last one), and
`own_options` those of its first attempt (a deck may start on looser options by itself, e.g.
test_load_switches.py's QOD deck). `attempts` lists each attempt's result: ok, errors_reported
(ngspice finished but logged an error line; the test decides), timestep_too_small or timeout.
A deck's `deck` is the sha256 (16 hex) of its text without the option lines.
"""

from __future__ import annotations

import argparse
import glob
import json
import os
import sys


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--suite-results", required=True)
    ap.add_argument("--log", required=True)
    ap.add_argument("--meta", required=True)
    ap.add_argument("--out", required=True)
    a = ap.parse_args()

    with open(a.meta, encoding="utf-8") as f:
        meta = json.load(f)
    with open(a.suite_results, encoding="utf-8") as f:
        checks = json.load(f)

    parts = []
    for p in sorted(glob.glob(os.path.join(a.log, "*.json"))):
        with open(p, encoding="utf-8") as f:
            parts.append(json.load(f))
    items = next((p["items"] for p in parts if p.get("items")), [])
    controller = [p for p in parts if "outcomes" in p]
    if len(controller) != 1 or not items:
        print(f"dms-suite-collect: expected one controller record and a collection list in {a.log}",
              file=sys.stderr)
        return 1
    outcomes = controller[0]["outcomes"]
    index = {nid: i for i, nid in enumerate(items)}

    # The ngspice calls of one Deck object (one process, one `obj`) in call order, split into ladder
    # chains: a retry follows only a hang or a "Timestep too small" (both fallback ladders), so a call
    # after any other result starts a new chain (the test ran the same Deck again, e.g. after editing
    # it). A chain is one "deck" of results.json; its fallback level is its length minus one. Chains
    # are then sorted by content, so thread scheduling inside a test does not change the order.
    by_obj: dict = {}
    for p in parts:
        for c in p["calls"]:
            by_obj.setdefault((p["worker"], c["obj"]), []).append(c)
    decks = []
    for calls in by_obj.values():
        calls.sort(key=lambda c: c["seq"])
        chains = []
        for c in calls:
            if chains and chains[-1][-1]["result"] in ("timeout", "timestep_too_small") \
                    and c["test"] == chains[-1][-1]["test"]:
                chains[-1].append(c)
            else:
                chains.append([c])
        for ch in chains:
            decks.append({
                "test": ch[0]["test"] or "",
                "title": ch[0]["title"],
                "deck": ch[0]["deck"],
                "fallback_level": len(ch) - 1,
                "options": ch[-1]["options"],
                "own_options": ch[0]["options"],
                "attempts": [c["result"] for c in ch],
            })
    decks.sort(key=lambda d: (index.get(d["test"], len(index)), d["test"], d["title"], d["deck"],
                              d["own_options"], d["attempts"]))

    counts: dict = {}
    for nid in items:
        o = outcomes.get(nid, "not_run")
        if o != "not_run" or nid in outcomes:
            counts[o] = counts.get(o, 0) + 1
    deselected_or_unreported = [nid for nid in items if nid not in outcomes]
    levels: dict = {}
    for d in decks:
        levels[str(d["fallback_level"])] = levels.get(str(d["fallback_level"]), 0) + 1

    doc = {
        "schema": "bb-dms-suite/1",
        "reference": meta["reference"],
        "tools": meta["tools"],
        "run": meta["run"],
        "summary": {
            "outcomes": dict(sorted(counts.items())),
            "checks": len(checks),
            "checks_failed": sum(1 for c in checks if not c.get("ok")),
            "decks": len(decks),
            "ngspice_runs": sum(len(d["attempts"]) for d in decks),
            "decks_by_fallback_level": dict(sorted(levels.items())),
            "decks_with_timeouts": sum(1 for d in decks if "timeout" in d["attempts"]),
        },
        "fallback_levels": {
            "0": "the deck's own options (the suite's Deck default is 'option noacct', 'option reltol=1e-4'; tests add their own)",
            "1": "retry with 'option reltol=1e-3' (spicelib.NUMERIC_FALLBACKS[0])",
            "2": "retry with 'option reltol=1e-3', 'option method=gear', 'option rshunt=1e12' (NUMERIC_FALLBACKS[1])",
        },
        "outcomes": {nid: outcomes[nid] for nid in items if nid in outcomes},
        "decks": decks,
        "checks": checks,
    }
    if deselected_or_unreported:
        doc["summary"]["collected_without_outcome"] = len(deselected_or_unreported)
    os.makedirs(os.path.dirname(os.path.abspath(a.out)), exist_ok=True)
    with open(a.out, "w", encoding="utf-8") as f:
        f.write(json.dumps(doc, indent=1, ensure_ascii=False) + "\n")
    print(f"dms-suite-collect: {a.out}: {doc['summary']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
