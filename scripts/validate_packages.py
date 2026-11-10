#!/usr/bin/env python3
"""Schema validation of every package file and lockfile (ADR-006 G1, plan M3).

    python3 scripts/validate_packages.py [<root> ...]

Default roots: packages/ (first-party), spec/examples/packages/ (the
spec's example packages), examples/ (the bb.lock files and the projects)
and tests/fixtures/kicad/ (the import fixtures). Every part.toml, the
pins.toml it names, every test vector in its [tests] dir, every bb.lock,
every project file bb.toml and bench file (bench/*.toml) are converted from
TOML to JSON (tomllib), and every board.bbm is read as JSON with integers
only (a float is an error, spec/canonical-json.md); all are validated
against spec/schema/*.schema.json (JSON Schema 2020-12, python3-jsonschema).
mpn_patterns must compile as Python regular expressions (the resolver
matches them against the whole MPN). Before that, a self-test checks that
the validator rejects known-bad documents, so a broken schema load cannot
pass silently.

Exit status 0 when everything validates.
"""
import json
import pathlib
import re
import sys
import tomllib

try:
    import jsonschema
except ImportError:  # pragma: no cover
    sys.exit("validate_packages: python3-jsonschema is missing (make setup installs it)")

REPO = pathlib.Path(__file__).resolve().parent.parent
SCHEMAS = {
    name: json.loads((REPO / "spec" / "schema" / f"{name}.schema.json").read_text(encoding="utf-8"))
    for name in ("part", "pins", "lock", "pkgtest", "project", "bench", "board-bbm")
}
for s in SCHEMAS.values():
    jsonschema.Draft202012Validator.check_schema(s)
VALIDATORS = {n: jsonschema.Draft202012Validator(s) for n, s in SCHEMAS.items()}


def errors(kind, doc):
    return sorted(VALIDATORS[kind].iter_errors(doc), key=lambda e: list(e.absolute_path))


def self_test():
    bad = [
        ("part", {"schema": "bb-part/0", "part": {"id": "x/y", "version": "1", "kind": "soc", "license": "MIT"}, "pins": {"file": "p"}}),
        ("part", {"schema": "bb-part/0", "part": {"id": "x/y", "version": "1.0.0", "kind": "soc", "license": "MIT"}, "pins": {"file": "p"}, "thermal": {}}),
        ("part", {"schema": "bb-part/1", "part": {"id": "x/y", "version": "1.0.0", "kind": "soc", "license": "MIT"}, "pins": {"file": "p"}}),
        ("pins", {"schema": "bb-pins/0", "pin": [{"number": "1", "name": "A", "role": "gpio"}]}),
        ("pins", {"schema": "bb-pins/0", "pin": [{"number": "1", "name": "A", "role": "io"}], "defaults": {"digital_out": {"default": {"voh": "3.3V"}}}}),
        ("lock", {"schema": "bb-lock/0", "package": [{"id": "x/y", "version": "1.0.0", "path": ".", "tree": "sha256:00"}]}),
        ("pkgtest", {"schema": "bb-pkgtest/0", "view": "v", "circuit": "c.cir", "stop": "1ms", "ports": {}, "expect": []}),
        ("project", {"schema": "bb-project/0", "board": [{"id": "main", "netlist": "a.net", "kicad": "a.kicad_pro"}]}),
        ("project", {"schema": "bb-project/0", "board": [{"id": "main", "netlist": "a.net"}], "instances": {"J1": {"package": "none"}}}),
        ("bench", {"schema": "bb-bench/0", "supply": [{"net": "VSYS"}]}),
        ("board-bbm", {"schema": "bb-board/0", "boards": [{"id": "m", "ground": "GND"}], "nets": [{"name": "A", "board": "m", "spice": "a b", "nodes": []}], "instances": [], "platforms": [], "probes": []}),
    ]
    good = ("part", {"schema": "bb-part/0", "part": {"id": "x/y", "version": "1.0.0", "kind": "soc", "license": "MIT"}, "pins": {"file": "p"}})
    for kind, doc in bad:
        if not errors(kind, doc):
            sys.exit(f"validate_packages: self-test FAILED: the {kind} schema accepted {doc}")
    if errors(*good):
        sys.exit("validate_packages: self-test FAILED: the part schema rejected a minimal valid part")


def load(path):
    with open(path, "rb") as f:
        return tomllib.load(f)


def main(argv):
    self_test()
    roots = [pathlib.Path(a) for a in argv] or [
        REPO / "packages",
        REPO / "spec" / "examples" / "packages",
        REPO / "examples",
        REPO / "tests" / "fixtures" / "kicad",
    ]
    checked, failed = 0, 0

    def check(kind, path):
        nonlocal checked, failed
        checked += 1
        try:
            doc = load(path)
        except (OSError, tomllib.TOMLDecodeError) as e:
            failed += 1
            print(f"FAIL {path}: {e}")
            return None
        errs = errors(kind, doc)
        for e in errs:
            where = "/".join(str(p) for p in e.absolute_path) or "(top)"
            print(f"FAIL {path.relative_to(REPO) if path.is_relative_to(REPO) else path}: {where}: {e.message}")
        if errs:
            failed += 1
        return doc

    for root in roots:
        if not root.exists():
            continue
        for part_path in sorted(root.rglob("part.toml")):
            part = check("part", part_path)
            if part is None:
                continue
            d = part_path.parent
            pins = part.get("pins", {}).get("file")
            if pins:
                check("pins", d / pins)
            for p in part.get("part", {}).get("mpn_patterns", []):
                try:
                    re.compile(p)
                except re.error as e:
                    failed += 1
                    print(f"FAIL {part_path}: mpn_patterns {p!r}: {e}")
            tests = part.get("tests", {}).get("dir")
            if tests and (d / tests).is_dir():
                for t in sorted((d / tests).glob("*.toml")):
                    check("pkgtest", t)
        for lock in sorted(root.rglob("bb.lock")):
            check("lock", lock)
        for proj in sorted(root.rglob("bb.toml")):
            check("project", proj)
        for bench in sorted(root.rglob("bench/*.toml")):
            check("bench", bench)
        for bbm in sorted(root.rglob("board.bbm")):
            checked += 1
            try:
                doc = json.loads(bbm.read_text(encoding="utf-8"), parse_float=_no_float)
            except (OSError, ValueError) as e:
                failed += 1
                print(f"FAIL {bbm}: {e}")
                continue
            errs = errors("board-bbm", doc)
            for e in errs:
                where = "/".join(str(p) for p in e.absolute_path) or "(top)"
                print(f"FAIL {bbm}: {where}: {e.message}")
            if errs:
                failed += 1
    print(f"validate_packages: {checked} files, {failed} failed (schemas: spec/schema/*.schema.json, jsonschema {jsonschema_version()})")
    return 1 if failed or not checked else 0


def _no_float(text):
    raise ValueError(f"a float ({text}) in canonical JSON (spec/canonical-json.md: integers only)")


def jsonschema_version():
    try:
        from importlib.metadata import version

        return version("jsonschema")
    except Exception:  # pragma: no cover
        return "?"


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
