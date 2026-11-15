#!/usr/bin/env python3
"""Phase 2 E4 (ADR-016 D2): the VS Code templates in docs/templates/vscode.

    vscode_templates.py --root R                      static checks (make vscode-templates)
    vscode_templates.py --root R --print-config-name  the launch configuration's name
    vscode_templates.py --root R --check-run MODE --dir D --golden G --vscode-rc RC --project P
                                                      one run of scripts/vscode_test.sh

The static checks: both files parse as VS Code's JSON with comments; the
launch configuration is cortex-debug, `request: attach`, `servertype:
external`, `gdbTarget` on localhost, `gdbPath: gdb-multiarch`,
`overrideAttachCommands: []` (cortex-debug's default for an external server
is `monitor halt`, which bb refuses), no `svdFile` and nothing that loads or
flashes; its preLaunchTask is the template's task, a background `bb run
--gdb <port>` with the same port as gdbTarget, whose endsPattern matches the
line bb prints when its proxy listens (taken from cli/src/main.rs, so a
change to that message fails here) and whose beginsPattern matches bb's
first stderr line.
"""
import argparse
import hashlib
import json
import re
import sys
from pathlib import Path

FAILS = []


def ok(msg):
    print(f"vscode-templates: PASS: {msg}", flush=True)


def bad(msg):
    print(f"vscode-templates: FAIL: {msg}", flush=True)
    FAILS.append(msg)


def check(cond, msg):
    (ok if cond else bad)(msg)
    return cond


def jsonc(text):
    """VS Code's JSON with comments: // and /* */ comments and trailing commas."""
    out, i, n = [], 0, len(text)
    while i < n:
        c = text[i]
        if c == '"':
            j = i + 1
            while j < n and text[j] != '"':
                j += 2 if text[j] == "\\" else 1
            out.append(text[i:j + 1])
            i = j + 1
        elif text.startswith("//", i):
            while i < n and text[i] != "\n":
                i += 1
        elif text.startswith("/*", i):
            i = text.index("*/", i + 2) + 2
        else:
            out.append(c)
            i += 1
    return json.loads(re.sub(r",(\s*[}\]])", r"\1", "".join(out)))


def templates(root):
    d = Path(root) / "docs/templates/vscode"
    return jsonc((d / "tasks.json").read_text()), jsonc((d / "launch.json").read_text())


# Keys that would make cortex-debug load, flash, reset or read peripherals.
FORBIDDEN = ("svdFile", "loadFiles", "preAttachCommands", "postAttachCommands", "preLaunchCommands",
             "overrideLaunchCommands", "postLaunchCommands", "postRestartCommands", "overrideRestartCommands",
             "runToEntryPoint", "runToMain", "breakAfterReset", "rtos", "liveWatch", "swoConfig", "rttConfig")


def static(root):
    tasks, launch = templates(root)
    cfgs = launch.get("configurations", [])
    check(len(cfgs) == 1, "launch.json: one configuration")
    c = cfgs[0] if cfgs else {}
    check(c.get("type") == "cortex-debug" and c.get("request") == "attach" and c.get("servertype") == "external",
          "launch.json: cortex-debug, request attach, servertype external (bb is the GDB server)")
    m = re.fullmatch(r"localhost:(\d+)", c.get("gdbTarget", ""))
    check(bool(m), f"launch.json: gdbTarget on localhost ({c.get('gdbTarget')})")
    check(c.get("gdbPath") == "gdb-multiarch", "launch.json: gdbPath gdb-multiarch")
    check(c.get("overrideAttachCommands") == [],
          "launch.json: overrideAttachCommands [] (cortex-debug's default `monitor halt` is refused by bb)")
    bad_keys = [k for k in FORBIDDEN if k in c]
    check(not bad_keys, f"launch.json: no svdFile, no load, no extra GDB commands ({bad_keys or 'none'})")
    check(str(c.get("executable", "")).endswith(".elf"), "launch.json: executable is an ELF")
    ts = [t for t in tasks.get("tasks", []) if t.get("label") == c.get("preLaunchTask")]
    check(len(ts) == 1, f"launch.json: preLaunchTask names the task in tasks.json ({c.get('preLaunchTask')})")
    t = ts[0] if ts else {}
    args = t.get("args", [])
    check(t.get("command") == "bb" and args[:1] == ["run"] and "--gdb" in args,
          f"tasks.json: the task runs `bb run --gdb` ({t.get('command')} {' '.join(args)})")
    port = args[args.index("--gdb") + 1] if "--gdb" in args and args.index("--gdb") + 1 < len(args) else None
    check(bool(m) and port == m.group(1), f"tasks.json: --gdb {port} is gdbTarget's port")
    check(t.get("isBackground") is True, "tasks.json: a background task (the run outlives the attach)")
    bg = (t.get("problemMatcher") or {}).get("background") or {}
    src = (Path(root) / "cli/src/main.rs").read_text()
    lm = re.search(r'"bb run: GDB: listening on 127\.0\.0\.1:\{port\} \(`target remote :\{port\}`\); \{\}"', src)
    line = f"bb run: GDB: listening on 127.0.0.1:{port} (`target remote :{port}`); the CPU waits at reset"
    check(bool(lm) and bool(re.search(bg.get("endsPattern", "^$"), line)),
          f"tasks.json: endsPattern {bg.get('endsPattern')!r} matches bb's listening line (cli/src/main.rs)")
    # VS Code 1.145.3's background matcher (`processLineInternal`: `tryBegin(line)
    # || tryFinish(line)`) does not test a line that matched beginsPattern against
    # endsPattern, so the listening line must not match beginsPattern (measured:
    # the attach never started when it did). `activeOnStart` (not "activeBegins",
    # which VS Code ignores) makes the task active before any line.
    check(bg.get("activeOnStart") is True and set(bg) == {"activeOnStart", "beginsPattern", "endsPattern"}
          and not re.search(bg["beginsPattern"], line)
          and not re.search(bg["beginsPattern"], line),
          "tasks.json: the background is active from the start and beginsPattern does not swallow "
          "the listening line")
    pat = ((t.get("problemMatcher") or {}).get("pattern") or {})
    check("file" in pat and "message" in pat, "tasks.json: the problem pattern has a file and a message")
    return c, t


def hashes(d):
    d = Path(d)
    ev = (d / "events.jsonl").read_bytes()
    ng = b"".join(l for l in ev.splitlines(keepends=True) if b'"type":"grant"' not in l)
    h = lambda b: hashlib.sha256(b).hexdigest()
    return {"nets.csv": h((d / "nets.csv").read_bytes()), "nets.vcd": h((d / "nets.vcd").read_bytes()),
            "non-grant-records": h(ng), "events.jsonl": h(ev)}


def golden(path):
    return {name: hx for hx, name in (l.split() for l in Path(path).read_text().splitlines() if l.strip())}


def marks(d):
    out = []
    for line in (Path(d) / "events.jsonl").read_text().splitlines():
        r = json.loads(line)
        if r.get("type") == "log" and r.get("level") == "warn" and "debug run: GDB" in r.get("msg", ""):
            out.append(r)
    return out


def check_run(a):
    _, task = static(a.root)
    d = Path(a.dir)
    mode = a.check_run
    res = json.loads((d / "result.json").read_text()) if (d / "result.json").exists() else {}
    log = (d / "bb.log").read_text() if (d / "bb.log").exists() else ""
    argv = (d / "bb.argv").read_text().splitlines() if (d / "bb.argv").exists() else []
    if not check(a.vscode_rc == 0 and res.get("ok") is True,
                 f"{mode}: VS Code ran the test and exited 0 (rc {a.vscode_rc}; {res.get('error', 'no error')[:300]})"):
        for s in res.get("steps", []):
            print(f"  {s['ms']:>7} ms  {s['msg']}")
        return
    for s in res["steps"]:
        print(f"  {s['ms']:>7} ms  {s['msg']}")
    check(argv == task["args"],
          f"{mode}: VS Code ran the template's task: bb {' '.join(argv)}")
    check(res.get("task", {}).get("exitCode") == 0, f"{mode}: the run (the task) exited 0")
    check("GDB: listening on 127.0.0.1:" in log and "monitor commands are refused" not in log,
          f"{mode}: bb's proxy listened, and no monitor command reached it")
    dap = [json.loads(l) for l in (d / "result.json.dap.jsonl").read_text().splitlines()]
    att = [x["m"] for x in dap if x.get("to") == "adapter" and x["m"].get("command") == "attach"]
    aa = att[0].get("arguments", {}) if att else {}
    check(len(att) == 1 and aa.get("servertype") == "external" and aa.get("overrideAttachCommands") == []
          and "svdFile" not in aa,
          f"{mode}: cortex-debug got one attach request with the template's settings")
    regs = [v for vs in res.get("scopes", {}).values() for v in vs]
    check(any(v.startswith("pc=") for v in regs) and res.get("ram_at_sp") not in (None, ""),
          f"{mode}: read the core registers ({len(regs)} values, e.g. "
          f"{next((v for v in regs if v.startswith('pc=')), 'no pc')}) and RAM at $sp ({res.get('ram_at_sp')})")
    run = d / "run"
    if not check((run / "events.jsonl").exists(), f"{mode}: the run's outputs exist ({run})"):
        return
    h, g, mk = hashes(run), golden(a.golden), marks(run)
    rj = json.loads((run / "run.json").read_text()) if (run / "run.json").exists() else {}
    if mode == "attach":
        check("debugger sessions 1, no stop inside a slice and no write" in log,
              "attach: bb reports one debugger session, no stop inside a slice and no write")
        check(not mk, "attach: no debug-run mark")
        check(all(h.get(k) == v for k, v in g.items()),
              f"attach: all four outputs equal the committed hashes of the run without GDB "
              f"({Path(a.golden).name} of tests/golden/dms-rail/ok-cold)")
    else:
        check(res.get("break_frame", "").startswith(a.break_at if hasattr(a, "break_at") else "gpio_nrfx"),
              f"break: cortex-debug stopped at the breakpoint ({res.get('break_frame')})")
        check(len(mk) == 1, f"break: exactly one debug-run mark ({mk[0]['msg'][:70] + '...' if mk else 'none'})")
        check(rj.get("debug") is True, "break: run.json debug true")
        diff = [k for k in ("nets.csv", "nets.vcd", "non-grant-records") if h.get(k) != g.get(k)]
        check(bool(diff), f"break: the outputs differ from the golden ({', '.join(diff) or 'none'}), "
                          f"so the attach case's comparison can fail")
    proj = Path(a.project)
    check(not (proj / ".vscode").exists() or sorted(p.name for p in (proj / ".vscode").iterdir())
          == ["launch.json", "tasks.json"], f"{mode}: VS Code added nothing to the project's .vscode/")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--root", required=True)
    ap.add_argument("--print-config-name", action="store_true")
    ap.add_argument("--check-run")
    ap.add_argument("--dir")
    ap.add_argument("--golden")
    ap.add_argument("--vscode-rc", type=int)
    ap.add_argument("--project")
    a = ap.parse_args()
    if a.print_config_name:
        print(templates(a.root)[1]["configurations"][0]["name"])
        return 0
    if a.check_run:
        check_run(a)
    else:
        static(a.root)
    print(f"vscode-templates: {'FAIL' if FAILS else 'PASS'}", flush=True)
    return 1 if FAILS else 0


if __name__ == "__main__":
    sys.exit(main())
