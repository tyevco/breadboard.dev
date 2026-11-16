#!/usr/bin/env python3
"""Milestone S's deck gate: two elaborated decks are byte-identical up to the
first-party view renames of the schema epoch (ADR-011 D11).

    python3 scripts/schema_epoch_decks.py <old deck.cir> <new deck.cir>   # exit 0 when equal

The old deck's names are renamed (whole words, without case, as SPICE reads
them) by RENAMES, except a name the new deck still holds (a board file's own
inline copy of a definition, e.g. examples/dial-me-spinner/e3/board-step.cir's
DRV8837_BRIDGE, is board text, not a package view). Both decks are cut into
units: a `.subckt ... .ends` block or a `.model`/`.func`/`.param` line with its
`+` continuations is a definition, every other line a unit of its own. The
non-definition units must be equal in order, and the definitions equal as a
sorted list: the deck emits view definitions in sorted (kind, name) order
(spec/package.md section 3), so a rename may move a definition block, and
nothing else may change. Prints one line: `equal`, `equal up to the renames
(<n> renamed definitions)`, or the first difference.
"""
import re
import sys

# The schema epoch's renames (packages/, ADR-011 D11): old name -> new name.
RENAMES = {
    "bb_generic_resistor": "GENERIC_RESISTOR_IDEAL",
    "bb_generic_capacitor": "GENERIC_CAPACITOR_IDEAL",
    "bb_generic_inductor": "GENERIC_INDUCTOR_IDEAL",
    "NRF_SAADC_SE": "NORDIC_NRF52840_QIAA_SAADC_SE",
    "DRV8837_BRIDGE": "TI_DRV8837_BRIDGE",
    "drv8837_bridge_body": "TI_DRV8837_BRIDGE_BODY",
    "TPS22917_LSW_FIT": "TI_TPS22917_LSW_FIT",
    "LSW_WS2812C": "WORLDSEMI_WS2812C_2020_LSW",
}


def word(name):
    return re.compile(r"(?<![A-Za-z0-9_])" + re.escape(name) + r"(?![A-Za-z0-9_])", re.IGNORECASE)


def units(text):
    """[(is_definition, text)] of a deck."""
    lines = text.split("\n")
    out, i = [], 0
    while i < len(lines):
        ln = lines[i]
        kw = ln.strip().split(None, 1)[0].lower() if ln.strip() else ""
        if kw == ".subckt":
            depth, j = 0, i + 1
            while j < len(lines):
                k = lines[j].strip().split(None, 1)[0].lower() if lines[j].strip() else ""
                if k == ".subckt":
                    depth += 1
                elif k == ".ends":
                    if depth == 0:
                        break
                    depth -= 1
                j += 1
            out.append((True, "\n".join(lines[i:j + 1])))
            i = j + 1
        elif kw in (".model", ".func", ".param"):
            j = i + 1
            while j < len(lines) and lines[j].lstrip().startswith("+"):
                j += 1
            out.append((True, "\n".join(lines[i:j])))
            i = j
        else:
            out.append((False, ln))
            i += 1
    return out


def main(argv):
    old = open(argv[1], encoding="utf-8").read()
    new = open(argv[2], encoding="utf-8").read()
    if old == new:
        print("equal")
        return 0
    renamed = old
    applied = []
    for a, b in RENAMES.items():
        if word(a).search(new):
            continue  # the new deck still holds it: board text, not a renamed view
        if word(a).search(renamed):
            renamed = word(a).sub(b, renamed)
            applied.append(a)
    uo, un = units(renamed), units(new)
    ro = [t for d, t in uo if not d]
    rn = [t for d, t in un if not d]
    if ro != rn:
        for k, (x, y) in enumerate(zip(ro, rn)):
            if x != y:
                print(f"DIFFERENT: non-definition line {k}: old {x!r} new {y!r}")
                return 1
        print(f"DIFFERENT: {len(ro)} non-definition lines in the old deck, {len(rn)} in the new")
        return 1
    do = sorted(t for d, t in uo if d)
    dn = sorted(t for d, t in un if d)
    if do != dn:
        extra = sorted(set(do) ^ set(dn))
        print(f"DIFFERENT: the definitions differ: {extra[0][:200]!r}")
        return 1
    n = sum(1 for d, t in un if d and any(word(RENAMES[a]).search(t.split("\n", 1)[0]) for a in applied))
    print(f"equal up to the renames ({n} renamed definitions; {', '.join(applied)})")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
