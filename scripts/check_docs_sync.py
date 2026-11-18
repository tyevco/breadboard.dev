#!/usr/bin/env python3
"""make docs-sync (in make test): the docs that copy the code stay in step with it.

    python3 scripts/check_docs_sync.py <bb>

1. The quickstart page (site/guide/quickstart.md, its first ```sh block) is the README's quickstart
   block (between the stranger-test markers), line for line: the page says it is a verbatim copy.
2. Every step of the Makefile's TEST_STEPS is named in site/guide/testing.md ("What make test
   runs", as **`<step>`**) and in the README's "What `make test` checks".
3. The CLI reference (site/reference/cli.md) has a section for every subcommand `bb --help` lists,
   names every option of `bb run`'s usage, and every `bb pkg` and `bb registry` subcommand.
4. The Phase 3 deferrals (approver decision D-close 7): the roadmap's paragraph names as refused
   exactly what bb refuses (`[[env]]`, `[probes]`, `bb pkg init`; `bb pkg init --from-svd` exits
   non-zero naming Phase 3), and says the bench engine's lock entry is not written rather than
   refused (no committed bb.lock has one; run.json records bench@1).

Exit status 0 when all hold; each miss is printed.
"""
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
fails = []


def check(cond, text):
    print(f"docs-sync: {'PASS' if cond else 'FAIL'}: {text}")
    if not cond:
        fails.append(text)


def help_text(bb, *args):
    p = subprocess.run([bb, *args, "--help"], capture_output=True, text=True)
    return p.stdout + p.stderr


def main(bb):
    readme = (ROOT / "README.md").read_text(encoding="utf-8")
    # 1. The quickstart block.
    m = re.search(r"<!-- stranger-test:begin -->\n```sh\n(.*?)```\n<!-- stranger-test:end -->", readme, re.S)
    qs = (ROOT / "site/guide/quickstart.md").read_text(encoding="utf-8")
    q = re.search(r"```sh\n(.*?)```", qs, re.S)
    check(m and q and m.group(1) == q.group(1),
          "site/guide/quickstart.md's first sh block is the README's quickstart block, verbatim")
    if m and q and m.group(1) != q.group(1):
        a, b = m.group(1).splitlines(), q.group(1).splitlines()
        for line in a:
            if line not in b:
                print(f"    only in the README: {line}")
        for line in b:
            if line not in a:
                print(f"    only in quickstart.md: {line}")
    # 2. make test's steps.
    mk = (ROOT / "Makefile").read_text(encoding="utf-8")
    s = re.search(r"^TEST_STEPS := (.*?)(?<!\\)\n", mk, re.S | re.M)
    steps = s.group(1).replace("\\\n", " ").split() if s else []
    check(len(steps) > 30, f"TEST_STEPS has {len(steps)} steps")
    testing = (ROOT / "site/guide/testing.md").read_text(encoding="utf-8")
    sec = re.search(r"## What `make test` runs\n(.*?)\n## ", testing, re.S)
    sec = sec.group(1) if sec else ""
    miss = [x for x in steps if f"**`{x}`**" not in sec]
    check(not miss, f"site/guide/testing.md names every make test step{f' (missing: {miss})' if miss else ''}")
    rsec = re.search(r"## What `make test` checks\n(.*?)\n## ", readme, re.S)
    rsec = rsec.group(1) if rsec else ""
    alias = {"cargo-test": "cargo test --workspace"}
    miss = [x for x in steps if not re.search(rf"(?<![\w-]){re.escape(alias.get(x, x))}(?![\w-])", rsec)]
    check(not miss, f"the README's 'What make test checks' names every make test step{f' (missing: {miss})' if miss else ''}")
    # 3. The CLI reference.
    cli = (ROOT / "site/reference/cli.md").read_text(encoding="utf-8")
    top = help_text(bb)
    usage = top.split("\n\n")[0]
    subs = sorted(set(re.findall(r"^\s*(?:usage: )?bb (\w+)", usage, re.M)))
    check(len(subs) >= 9, f"bb --help lists {len(subs)} subcommands: {subs}")
    miss = [x for x in subs if f"## `bb {x}`" not in cli]
    check(not miss, f"site/reference/cli.md has a section per subcommand{f' (missing: {miss})' if miss else ''}")
    run_usage = "\n".join(line for line in usage.splitlines() if not re.match(r"\s*bb (?!run)\w", line.replace("usage: ", "")))
    opts = sorted(set(re.findall(r"(--[a-z][a-z0-9-]*)", run_usage)))
    miss = [o for o in opts if f"`{o}" not in cli]
    check(not miss, f"site/reference/cli.md documents every bb run option ({len(opts)}){f' (missing: {miss})' if miss else ''}")
    for sub in ("pkg", "registry"):
        names = sorted(set(re.findall(rf"bb {sub} (\w+)", help_text(bb, sub))))
        miss = [n for n in names if f"bb {sub} {n}" not in cli]
        check(names and not miss, f"site/reference/cli.md names every bb {sub} subcommand {names}"
              f"{f' (missing: {miss})' if miss else ''}")
    # 4. The Phase 3 deferrals.
    p = subprocess.run([bb, "pkg", "init", "--from-svd", "x.svd"], capture_output=True, text=True)
    check(p.returncode != 0 and "Phase 3" in p.stdout + p.stderr,
          "bb pkg init --from-svd is refused, naming Phase 3")
    locks = subprocess.run(["git", "-C", str(ROOT), "ls-files", "*bb.lock"], capture_output=True,
                           text=True).stdout.split()
    benchy = [x for x in locks if "bench@" in (ROOT / x).read_text(encoding="utf-8")]
    check(locks and not benchy, f"no committed bb.lock ({len(locks)}) has a bench engine entry"
          f"{f' (found in: {benchy})' if benchy else ''}")
    road = (ROOT / "site/guide/roadmap.md").read_text(encoding="utf-8")
    para = re.search(r"Moved to Phase 3 by the approver.*?(?:\n\n|$)", road, re.S)
    para = " ".join(para.group(0).split()) if para else ""
    refused = re.search(r"`bb` refuses (.*?)\.(?:\s|$)", para)
    refused = refused.group(1) if refused else ""
    check(all(k in refused for k in ("`[[env]]`", "`[probes]`", "`bb pkg init`"))
          and "lock" not in refused and "each of them" not in refused
          and re.search(r"lock entry is not written", para),
          "site/guide/roadmap.md's Phase 3 paragraph: bb refuses [[env]], [probes] and bb pkg init; "
          "the bench engine's lock entry is not written")
    print(f"docs-sync: {'PASS' if not fails else 'FAIL'}")
    return 0 if not fails else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1]))
