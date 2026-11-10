#!/usr/bin/env python3
"""Check a package's pins.toml against a KiCad symbol library (ADR-006 D4
lint: "every pin of the footprint is listed exactly once"; ADR-007 D3: the
KiCad libpart pin names must match).

    python3 scripts/check_pins_kicad.py [--composite] <package dir> <lib.kicad_sym> <symbol>
    python3 scripts/check_pins_kicad.py --print <lib.kicad_sym> <symbol>

--composite (Phase 1 M5, ADR-006 Amendment 3): KiCad's chip symbols name a
multi-function pin by its functions joined with "/" (MCU_Nordic:nRF52840's
"AIN0/P0.02", "P0.18/~{RESET}"). The package names the pin by its port name
(P0.02, the name bindings use) and lists the other functions in `alt`. With
--composite such a KiCad name matches when one of its components (overbar
markup ~{...} removed) is the package's name and every other component is in
the pin's alt list; it is reported as a composite match, not as identical.

Needs KiCad's symbol libraries (e.g. /usr/share/kicad/symbols from the
kicad package); not part of `make test`. Reads the library with a real
S-expression reader (AGENTS.md: never regex for KiCad files) and follows
`extends`. Compares pin numbers and names exactly; KiCad's electrical type
is printed next to the package's role for review.
"""
import sys
import tomllib


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


def kicad_pins(lib, name):
    tree = parse(tokens(open(lib, encoding="utf-8").read()))[0]
    syms = {e[1]: e for e in tree if isinstance(e, list) and e and e[0] == "symbol"}
    sym = syms[name]
    ext = [e for e in sym if isinstance(e, list) and e and e[0] == "extends"]
    if ext:
        sym = syms[ext[0][1]]
    pins = {}

    def walk(node):
        for e in node:
            if isinstance(e, list) and e:
                if e[0] == "pin":
                    etype = e[1]
                    num = [x for x in e if isinstance(x, list) and x and x[0] == "number"][0][1]
                    nm = [x for x in e if isinstance(x, list) and x and x[0] == "name"][0][1]
                    pins[num] = (nm, etype)
                else:
                    walk(e)

    walk(sym)
    return pins


def main():
    if sys.argv[1] == "--print":
        for num, (nm, et) in sorted(kicad_pins(sys.argv[2], sys.argv[3]).items(), key=lambda x: int(x[0]) if x[0].isdigit() else x[0]):
            print(num, nm, et)
        return 0
    args = sys.argv[1:]
    composite = args[:1] == ["--composite"]
    if composite:
        args = args[1:]
    pkg, lib, sym = args[:3]
    part = tomllib.load(open(f"{pkg}/part.toml", "rb"))
    pins = tomllib.load(open(f"{pkg}/{part['pins']['file']}", "rb"))["pin"]
    kp = kicad_pins(lib, sym)
    ours = {p["number"]: p for p in pins}
    bad = comp = 0
    for num in sorted(set(kp) | set(ours), key=lambda x: (0, int(x), "") if x.isdigit() else (1, 0, x)):
        k, o = kp.get(num), ours.get(num)
        if k is not None and o is not None and k[0] != o["name"] and composite and "/" in k[0]:
            parts = [c.replace("~{", "").replace("}", "") for c in k[0].split("/")]
            if o["name"] in parts and all(c in o.get("alt", []) for c in parts if c != o["name"]):
                comp += 1
                continue
        if k is None or o is None or k[0] != o["name"]:
            bad += 1
            print(f"MISMATCH pin {num}: kicad {k}, package {o and (o['name'], o['role'])}")
    print(f"{part['part']['id']}: {len(ours)} pins in pins.toml, {len(kp)} in KiCad {sym}; {bad} mismatches"
          + (f", {comp} composite-name matches" if composite else ""))
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
