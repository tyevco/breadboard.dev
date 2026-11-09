#!/usr/bin/env python3
"""ADR-006 gate G3: Breadboard's KiCad value parser against the
dial-me-spinner suite's own (`spicelib.parse_value`, `parse_tol`) on every
distinct passive value of the main board.

    python3 scripts/dms_values.py --bb <bb> [--repo <dial-me-spinner>] [--write]

1. Reads the vendored main-board netlist
   (examples/dial-me-spinner/kicad/main/dial-me-spinner-main.net, 6854d25)
   with an S-expression reader and collects the value of every generic
   passive (footprint library Resistor_*, Capacitor_*, Inductor_*, or a
   R/C/L reference on a custom footprint): the distinct (kind, value) pairs.
2. With --repo, takes `parse_value` and `parse_tol` from
   `git show 6854d25:hardware/sim/spicelib.py` (read only; the functions are
   extracted with `ast` and run alone, so nothing else of the suite is
   imported) and computes the suite's parse of each pair; with --write it
   writes them to tests/fixtures/values/dms-main.json, else it requires the
   committed fixture to equal them. Without --repo the committed fixture is
   the reference.
3. Runs `bb pkg value --kicad <K> <text>...` and requires, for every pair,
   bb's exact integer value to equal the suite's float scaled to the same
   sub-unit and rounded (`M` is mega in KiCad values: 1M, 2.2M), the
   tolerance to equal the suite's explicit one, and the emitted SPICE number
   to carry no letter suffix.
"""
import argparse
import ast
import json
import pathlib
import subprocess
import sys

REPO = pathlib.Path(__file__).resolve().parent.parent
NET = REPO / "examples/dial-me-spinner/kicad/main/dial-me-spinner-main.net"
FIXTURE = REPO / "tests/fixtures/values/dms-main.json"
REF = "6854d25e0fde9b5d60da07b2c2b6183fd36dfc5a"
SUBEXP = {"R": 6, "C": 18, "L": 15}  # uohm, aF, fH (ADR-007 D6)


def tokens(s):
    i, n = 0, len(s)
    while i < n:
        c = s[i]
        if c in "()":
            yield c
            i += 1
        elif c.isspace():
            i += 1
        elif c == '"':
            j, buf = i + 1, []
            while s[j] != '"':
                if s[j] == "\\":
                    buf.append(s[j + 1])
                    j += 2
                else:
                    buf.append(s[j])
                    j += 1
            yield ("s", "".join(buf))
            i = j + 1
        else:
            j = i
            while j < n and s[j] not in '() \t\r\n"':
                j += 1
            yield ("a", s[i:j])
            i = j


def parse(it):
    out = []
    for t in it:
        if t == "(":
            out.append(parse(it))
        elif t == ")":
            return out
        else:
            out.append(t[1])
    return out


def find(node, key):
    for e in node:
        if isinstance(e, list) and e and e[0] == key:
            return e
    return None


def board_values():
    tree = parse(tokens(NET.read_text(encoding="utf-8")))[0]
    pairs = set()
    for c in find(tree, "components")[1:]:
        if not (isinstance(c, list) and c and c[0] == "comp"):
            continue
        ref = find(c, "ref")[1]
        value = find(c, "value")[1]
        fp = (find(c, "footprint") or [None, ""])[1]
        lib = fp.split(":")[0]
        kind = {"Resistor_": "R", "Capacitor_": "C", "Inductor_": "L"}
        k = next((v for p, v in kind.items() if lib.startswith(p)), None)
        if k is None and ref[:1] in "RCL" and ref[1:2].isdigit() and ":" in fp and not lib.startswith(("TestPoint", "MountingHole")):
            k = ref[0]
        if k:
            pairs.add((k, value))
    return sorted(pairs)


def spicelib_functions(repo):
    src = subprocess.run(
        ["git", "-C", repo, "show", f"{REF}:hardware/sim/spicelib.py"], check=True, capture_output=True, text=True
    ).stdout
    mod = ast.parse(src)
    keep = []
    for node in mod.body:
        if isinstance(node, ast.Assign) and any(getattr(t, "id", "") in ("_SI", "DEFAULT_TOL") for t in node.targets):
            keep.append(node)
        if isinstance(node, ast.FunctionDef) and node.name in ("parse_value", "parse_tol"):
            keep.append(node)
    ns = {"re": __import__("re")}
    exec(compile(ast.Module(body=keep, type_ignores=[]), "spicelib.py@6854d25", "exec"), ns)
    return ns["parse_value"], ns["parse_tol"]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bb", required=True)
    ap.add_argument("--repo")
    ap.add_argument("--write", action="store_true")
    a = ap.parse_args()
    pairs = board_values()
    print(f"dms_values: {len(pairs)} distinct (kind, value) pairs of generic passives on the main board")
    if a.repo:
        pv, pt = spicelib_functions(a.repo)
        ref = []
        for k, text in pairs:
            v = pv(text)
            explicit_tol = "%" in text
            ref.append(
                {
                    "kind": k,
                    "text": text,
                    "spicelib_value": repr(v),
                    "subunits": round(v * 10 ** SUBEXP[k]),
                    "tolerance_ppm": round(pt(text, k) * 1e6) if explicit_tol else None,
                }
            )
        doc = {"source": f"dial-me-spinner {REF}: hardware/sim/spicelib.py parse_value/parse_tol on {NET.relative_to(REPO)}", "values": ref}
        text = json.dumps(doc, indent=1, ensure_ascii=False) + "\n"
        if a.write:
            FIXTURE.parent.mkdir(parents=True, exist_ok=True)
            FIXTURE.write_text(text, encoding="utf-8")
            print(f"dms_values: wrote {FIXTURE.relative_to(REPO)}")
        elif FIXTURE.read_text(encoding="utf-8") != text:
            sys.exit(f"dms_values: FAIL: {FIXTURE.relative_to(REPO)} differs from spicelib's parse at {REF[:7]} (rerun with --write and review)")
    doc = json.loads(FIXTURE.read_text(encoding="utf-8"))
    ref = doc["values"]
    if [(r["kind"], r["text"]) for r in ref] != [list(p) if False else (p[0], p[1]) for p in pairs]:
        sys.exit("dms_values: FAIL: the fixture's values are not the board's distinct passive values")
    bad = 0
    for k in "RCL":
        rows = [r for r in ref if r["kind"] == k]
        if not rows:
            continue
        out = subprocess.run([a.bb, "pkg", "value", "--kicad", k] + [r["text"] for r in rows], capture_output=True, text=True)
        got = [json.loads(l) for l in out.stdout.splitlines()]
        for r, g in zip(rows, got):
            problems = []
            if "error" in g:
                problems.append(g["error"])
            else:
                if g["value"] != r["subunits"]:
                    problems.append(f"value {g['value']} != spicelib {r['subunits']} ({r['spicelib_value']})")
                if r["tolerance_ppm"] is not None and g["tolerance_ppm"] != r["tolerance_ppm"]:
                    problems.append(f"tolerance {g['tolerance_ppm']} != spicelib {r['tolerance_ppm']} ppm")
                if any(ch.isalpha() and ch != "e" for ch in g["spice"]):
                    problems.append(f"SPICE number {g['spice']!r} has a letter suffix")
            status = "ok  " if not problems else "FAIL"
            bad += bool(problems)
            print(f"{status} {k} {r['text']!r:16} -> {g.get('value')} {g.get('unit', '')} (spicelib {r['spicelib_value']}; SPICE {g.get('spice')}) {'; '.join(problems)}")
    print(f"dms_values: {len(ref)} values, {bad} mismatches")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
