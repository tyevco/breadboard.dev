#!/usr/bin/env python3
"""Phase 2 B3 (ADR-010 D8, Amendment 5): E2's variant projects for `bb test`.

examples/dial-me-spinner/tests/e2.toml runs make dms-vsense-test's four cases as a bb test file. Two
of them need a project that differs from examples/dial-me-spinner by one edit each, which
scripts/dms_vsense.sh makes in scratch copies (and keeps making for make dms-vsense):

  e2/none      load = "none" on the VSYS_SENSE binding (the unloaded reference, ADR-008 D7)
  e2/c5        C5 fitted one decade low (100n -> 10n in the main board's netlist: the suite's
               test_fault_c5_decade_low_detected_at_fw_tacq)
  e2/c5-none   both

This script writes each variant's bb.toml from the project's own bb.toml (every binding and probe
kept; the netlist path changed; no [targets]: the bb test case gives the image), and e2/c5's netlist
from the vendored one with exactly that one edit. `bb open` then writes their board.bbm and bb.lock
(make locks). `--check` requires the committed files to be this script's output byte for byte.

  scripts/dms_e2_projects.py [--check]
"""
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PROJ = ROOT / "examples/dial-me-spinner"
NET = "kicad/main/dial-me-spinner-main.net"
C5_NET = "dial-me-spinner-main-c5.net"

BIND = 'pin = "main:U10.P0.02"\nmodel = "adc_in"\nnet = "VSYS_SENSE"\n'
C5_OLD = '(ref "C5")\n\t\t\t(value "100n")'
C5_NEW = '(ref "C5")\n\t\t\t(value "10n")'

VARIANTS = {
    # name: (netlist path from the variant's directory, load none, what)
    "none": (f"../../{NET}", True, 'load = "none" on the VSYS_SENSE binding (the unloaded reference)'),
    "c5": (C5_NET, False, "C5 fitted one decade low (10 nF; the suite's C5 / 10 fault)"),
    "c5-none": (f"../c5/{C5_NET}", True,
                'C5 at 10 nF (../c5\'s netlist) and load = "none" on the VSYS_SENSE binding'),
}


def project_toml(name: str) -> str:
    net, none, what = VARIANTS[name]
    src = (PROJ / "bb.toml").read_text()
    body = src[src.index('schema = "bb-project/1"'):]
    # The [targets] table and the comment above it: the bb test case gives the image.
    t0 = body.index("# Written by `bb flash --target U10")
    t1 = body.index("\n\n", body.index('[targets."main:U10"]')) + 2
    body = body[:t0] + body[t1:]
    old = f'netlist = "{NET}"'
    assert body.count(old) == 1, "the main board's netlist line"
    body = body.replace(old, f'netlist = "{net}"')
    old = 'name = "dial-me-spinner rev A"'
    assert body.count(old) == 1, "the project name"
    body = body.replace(old, f'name = "dial-me-spinner rev A: E2 {name}"')
    if none:
        assert body.count(BIND) == 1, "the VSYS_SENSE binding"
        body = body.replace(BIND, BIND + 'load = "none"\n')
    head = (
        f"# examples/dial-me-spinner/e2/{name}/bb.toml: WRITTEN by scripts/dms_e2_projects.py from\n"
        "# ../../bb.toml (do not edit; `scripts/dms_e2_projects.py --check` in make dms-vsense-test).\n"
        f"# E2's variant {name} for bb test (tests/e2.toml; Phase 2 B3, ADR-010 Amendment 5): {what}.\n"
        "# No [targets]: each bb test case gives its image (--image).\n"
    )
    return head + body


def c5_netlist() -> str:
    s = (PROJ / NET).read_text()
    assert s.count(C5_OLD) == 1, "C5 100n in the netlist"
    return s.replace(C5_OLD, C5_NEW)


def outputs():
    out = {f"e2/{n}/bb.toml": project_toml(n) for n in VARIANTS}
    out[f"e2/c5/{C5_NET}"] = c5_netlist()
    return out


def main() -> int:
    check = sys.argv[1:] == ["--check"]
    if sys.argv[1:] not in ([], ["--check"]):
        print(__doc__)
        return 2
    bad = 0
    for rel, text in outputs().items():
        p = PROJ / rel
        if check:
            if not p.is_file() or p.read_text() != text:
                print(f"dms_e2_projects: {rel} is not this script's output")
                bad = 1
        else:
            p.parent.mkdir(parents=True, exist_ok=True)
            if not p.is_file() or p.read_text() != text:
                p.write_text(text)
                print(f"dms_e2_projects: wrote {rel}")
    if check and not bad:
        print(f"dms_e2_projects: the {len(VARIANTS)} E2 variant projects are this script's output")
    return bad


if __name__ == "__main__":
    sys.exit(main())
