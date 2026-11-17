#!/usr/bin/env python3
"""Phase 2 H6 (ADR-015 D11, Amendment 5): a local structural check of the CI workflows.

CI runs only on a push, which an agent never does here, and actionlint is not installed, so a
workflow change is first checked by this script:

- every file under .github/workflows/ and .github/actions/*/action.yml parses as YAML;
- a workflow has `on` and `jobs`; each job has `runs-on` and a non-empty `steps` list, any
  `needs` names a job of the same file, and `timeout-minutes` (if any) is a positive integer;
- each step has exactly one of `uses` and `run`; a local action (`uses: ./...`) exists with an
  action.yml; a composite action's steps each have `shell` with `run`;
- every `${{ ... }}` expression is closed, every `matrix.<key>` names a key of the job's matrix,
  every `steps.<id>` names an earlier step's id, and every `inputs.<name>` an input of the action;
- every `make <target>` in a `run:` step (the matrix values substituted) names a target the
  Makefile defines (from `make -qp`).

It is not actionlint: it does not type-check expressions or know every action's inputs. The
first push confirms the workflows on GitHub (AGENTS.md "Gate budgets").

    python3 scripts/check_workflows.py [<file> ...]      (default: every workflow and action)
"""
import itertools
import os
import re
import subprocess
import sys

import yaml

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXPR = re.compile(r"\$\{\{(.*?)\}\}", re.S)
MAKE = re.compile(r"(?:^|[;&|(]\s*|\s)make\s+([^\n;&|)]*)")


def make_targets():
    out = subprocess.run(
        ["make", "-qp", "--no-print-directory"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        env={k: v for k, v in os.environ.items() if k not in ("MAKEFLAGS", "MFLAGS")},
    ).stdout
    targets = set()
    for line in out.splitlines():
        m = re.match(r"^([A-Za-z0-9_./%-]+)\s*:(?!=)", line)
        if m:
            targets.add(m.group(1))
    return targets


def expressions(text):
    """The expressions in text; an unclosed `${{` is an error."""
    if text.count("${{") != len(EXPR.findall(text)):
        raise ValueError(f"unclosed ${{{{ in {text!r}")
    return [e.strip() for e in EXPR.findall(text)]


def walk_strings(node):
    if isinstance(node, str):
        yield node
    elif isinstance(node, dict):
        for k, v in node.items():
            yield from walk_strings(k)
            yield from walk_strings(v)
    elif isinstance(node, list):
        for v in node:
            yield from walk_strings(v)


def matrix_rows(job):
    """Every combination of the job's matrix (include rows only, or the product)."""
    m = (job.get("strategy") or {}).get("matrix")
    if not m:
        return [{}], set()
    if isinstance(m, str):
        return [{}], None  # a computed matrix: keys unknown
    axes = {k: v for k, v in m.items() if k not in ("include", "exclude")}
    keys = set(axes)
    rows = [dict(zip(axes, combo)) for combo in itertools.product(*axes.values())] if axes else []
    for inc in m.get("include", []) or []:
        keys |= set(inc)
        rows.append(dict(inc))
    return rows or [{}], keys


def check_steps(errors, where, steps, targets, rows, mkeys, composite=False, inputs=None):
    if not isinstance(steps, list) or not steps:
        errors.append(f"{where}: no steps")
        return
    ids = set()
    for i, s in enumerate(steps):
        sw = f"{where} step {i + 1} ({s.get('name') or s.get('uses') or s.get('id') or '?'})"
        if ("uses" in s) == ("run" in s):
            errors.append(f"{sw}: needs exactly one of uses and run")
        if composite and "run" in s and "shell" not in s:
            errors.append(f"{sw}: a composite action's run step needs shell")
        uses = s.get("uses", "")
        if uses.startswith("./"):
            p = os.path.join(ROOT, uses[2:])
            if not (os.path.isfile(os.path.join(p, "action.yml")) or os.path.isfile(os.path.join(p, "action.yaml"))):
                errors.append(f"{sw}: local action {uses} has no action.yml")
        elif uses and "@" not in uses:
            errors.append(f"{sw}: {uses} is not pinned to a ref")
        for text in walk_strings(s):
            try:
                exprs = expressions(text)
            except ValueError as e:
                errors.append(f"{sw}: {e}")
                continue
            for e in exprs:
                for k in re.findall(r"\bmatrix\.([A-Za-z0-9_-]+)", e):
                    if mkeys is not None and k not in mkeys:
                        errors.append(f"{sw}: matrix.{k} is not a matrix key")
                for k in re.findall(r"\bsteps\.([A-Za-z0-9_-]+)", e):
                    if k not in ids:
                        errors.append(f"{sw}: steps.{k} names no earlier step id")
                for k in re.findall(r"\binputs\.([A-Za-z0-9_-]+)", e):
                    if inputs is not None and k not in inputs:
                        errors.append(f"{sw}: inputs.{k} is not an input")
        if "run" in s:
            for row in rows:
                run = EXPR.sub(
                    lambda m: str(row.get(m.group(1).strip()[len("matrix."):], m.group(0)))
                    if m.group(1).strip().startswith("matrix.")
                    else "$EXPR",  # any other expression: a value, never a target
                    s["run"],
                )
                for args in MAKE.findall(run):
                    for tok in args.split():
                        if tok.startswith("-") or "=" in tok or tok.startswith("$") or tok.startswith("#"):
                            if tok.startswith("#"):
                                break
                            continue
                        if tok not in targets:
                            errors.append(f"{sw}: make {tok}: no such Makefile target")
        if "id" in s:
            ids.add(s["id"])


def check_workflow(path, targets, errors):
    doc = yaml.safe_load(open(path))
    # YAML 1.1 reads the key `on` as True.
    if not isinstance(doc, dict) or ("on" not in doc and True not in doc) or "jobs" not in doc:
        errors.append(f"{path}: needs on and jobs")
        return 0
    jobs = doc["jobs"]
    for name, job in jobs.items():
        where = f"{path} job {name}"
        if "runs-on" not in job:
            errors.append(f"{where}: no runs-on")
        needs = job.get("needs", [])
        for n in [needs] if isinstance(needs, str) else needs:
            if n not in jobs:
                errors.append(f"{where}: needs {n}, not a job")
        t = job.get("timeout-minutes")
        if t is not None and not (isinstance(t, int) and t > 0):
            errors.append(f"{where}: timeout-minutes {t!r}")
        rows, mkeys = matrix_rows(job)
        for text in walk_strings({k: v for k, v in job.items() if k != "steps"}):
            for e in expressions(text):
                for k in re.findall(r"\bmatrix\.([A-Za-z0-9_-]+)", e):
                    if mkeys is not None and k not in mkeys:
                        errors.append(f"{where}: matrix.{k} is not a matrix key")
        check_steps(errors, where, job.get("steps"), targets, rows, mkeys)
    return len(jobs)


def check_action(path, targets, errors):
    doc = yaml.safe_load(open(path))
    runs = doc.get("runs") or {}
    if runs.get("using") != "composite":
        errors.append(f"{path}: runs.using is not composite")
        return 0
    inputs = set((doc.get("inputs") or {}).keys())
    check_steps(errors, path, runs.get("steps"), targets, [{}], set(), composite=True, inputs=inputs)
    return len(runs.get("steps") or [])


def main(argv):
    files = argv[1:]
    if not files:
        wf = os.path.join(ROOT, ".github", "workflows")
        files = sorted(os.path.join(wf, f) for f in os.listdir(wf) if f.endswith((".yml", ".yaml")))
        act = os.path.join(ROOT, ".github", "actions")
        if os.path.isdir(act):
            for d in sorted(os.listdir(act)):
                for n in ("action.yml", "action.yaml"):
                    if os.path.isfile(os.path.join(act, d, n)):
                        files.append(os.path.join(act, d, n))
    targets = make_targets()
    errors = []
    for f in files:
        try:
            if os.path.basename(f) in ("action.yml", "action.yaml"):
                n = check_action(f, targets, errors)
                print(f"check-workflows: {os.path.relpath(f, ROOT)}: {n} steps")
            else:
                n = check_workflow(f, targets, errors)
                print(f"check-workflows: {os.path.relpath(f, ROOT)}: {n} jobs")
        except (yaml.YAMLError, ValueError) as e:
            errors.append(f"{f}: {e}")
    for e in errors:
        print(f"check-workflows: {e}", file=sys.stderr)
    if errors:
        print(f"check-workflows: FAIL ({len(errors)} problem(s))", file=sys.stderr)
        return 1
    print(f"check-workflows: PASS ({len(files)} files)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
