#!/usr/bin/env python3
"""Phase 1 M9: the GDB passthrough gates (ADR-008 D10, Amendment 7).

`bb run --gdb` puts a GDB remote proxy in front of Renode's stub. This drives
a real GDB in batch mode against it, on the spike (STM32F103 blinky,
examples/spike) and on E1 (the dial-me-spinner Zephyr image led_rail.elf at
the cold bench, a copy of the project made by `scripts/dms_rail.sh prepare`).

    gdb_gate.py test --bb BB --gdb GDB --out DIR --work DIR   (make gdb-test, in make test)
    gdb_gate.py gate --bb BB --gdb GDB --out DIR --work DIR   (make gdb)

`test`: attach and detach without a stop give the committed hashes of a run
without GDB (all four files, spike and E1); a breakpoint hit and continued
marks the run (one `warn` record; run.json `debug: true` on E1), on both
boards; `monitor` is refused and changes nothing; a run without --gdb has
no debug key in run.json.

`gate` adds: attach in the middle of a running spike, reads and detach
(identical to the run without GDB); the scripted breakpoint session 20 times
(and with three single-steps, 20 times) byte-identical on the spike, 5 times
on E1; a 5 s hold at the breakpoint with the External Control timeout at 2 s
(no failure, outputs equal to no hold; the trace shows the RUN_FOR held open
with the CPU's time inside it and no grant boundary during the hold); reads
and steps at the breakpoint change nothing more than the breakpoint; Ctrl-C
then detach, and Ctrl-C then continue (identical to the run without GDB);
on E1, Ctrl-C then 700 single-steps then continue, and 1 500 single-steps
across grant boundaries and WFI, finish; monitor start/pause/halt refused;
a second debugger refused; GDB killed during a halt lets the run finish;
bb killed during a halt leaves no Renode; the listener is on 127.0.0.1 only;
--gdb with a Renode pool starts Renode fresh.
"""
import argparse
import hashlib
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FAILS = []


def ok(msg):
    print(f"gdb: PASS: {msg}", flush=True)


def bad(msg):
    print(f"gdb: FAIL: {msg}", flush=True)
    FAILS.append(msg)


def check(cond, msg):
    (ok if cond else bad)(msg)
    return cond


class Board:
    def __init__(self, name, args, elf, bp, golden):
        self.name, self.args, self.elf, self.bp, self.golden = name, args, elf, bp, golden


def boards(work):
    spike_elf = ROOT / "examples/spike/firmware/build/blinky.elf"
    proj = Path(work) / "proj-ok"
    return {
        "spike": Board(
            "spike",
            ["--netlist", str(ROOT / "examples/spike/board.cir"), "--firmware", str(spike_elf),
             "--target", "U1", "--probe", "net:LED_A", "--probe", "net:PA5"],
            spike_elf, "SysTick_Handler", ROOT / "tests/golden/spike/outputs.sha256"),
        "e1": Board(
            "e1",
            ["--project", str(proj), "--bench", str(proj / "bench/cold.toml")],
            proj / "firmware/prebuilt/led_rail.elf", "gpio_nrfx_port_set_bits_raw",
            ROOT / "tests/golden/dms-rail/ok-cold/outputs.sha256"),
    }


def hashes(d):
    d = Path(d)
    ev = (d / "events.jsonl").read_bytes()
    ng = b"".join(l for l in ev.splitlines(keepends=True) if b'"type":"grant"' not in l)
    h = lambda b: hashlib.sha256(b).hexdigest()
    return {
        "nets.csv": h((d / "nets.csv").read_bytes()),
        "nets.vcd": h((d / "nets.vcd").read_bytes()),
        "non-grant-records": h(ng),
        "events.jsonl": h(ev),
    }


def golden(path):
    out = {}
    for line in Path(path).read_text().splitlines():
        if line.strip():
            hx, name = line.split()
            out[name] = hx
    return out


def same(a, b, keys=("nets.csv", "nets.vcd", "non-grant-records")):
    """`a` equals `b` in `keys` (those `b` has)."""
    return bool(a) and all(a.get(k) == b.get(k) for k in keys if k in b)


def all_of(a, g):
    """`a` equals every hash of a golden file `g` (events.jsonl included)."""
    return bool(a) and all(a.get(k) == v for k, v in g.items())


def marks(d):
    recs = []
    for line in (Path(d) / "events.jsonl").read_text().splitlines():
        r = json.loads(line)
        if r.get("type") == "log" and r.get("level") == "warn" and "debug run: GDB" in r.get("msg", ""):
            recs.append(r)
    return recs


def run_json(d):
    p = Path(d) / "run.json"
    return json.loads(p.read_text()) if p.exists() else None


def children(pid):
    out = []
    for p in Path("/proc").iterdir():
        if p.name.isdigit():
            try:
                fields = (p / "stat").read_text().rsplit(")", 1)[1].split()
                if int(fields[1]) == pid:
                    out.append(int(p.name))
            except OSError:
                pass
    return out


def alive(pid):
    try:
        st = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[0]
        return st != "Z"
    except OSError:
        return False


class Run:
    """One `bb run`, with --gdb unless gdb is False."""

    def __init__(self, a, board, out, extra=(), gdb=True, wait=True, dur=None, env=None):
        self.a, self.board, self.out = a, board, Path(out)
        if self.out.exists():
            shutil.rmtree(self.out)
        self.out.parent.mkdir(parents=True, exist_ok=True)
        cmd = [a.bb, "run", *board.args, "--for", dur or ("120ms" if board.name == "e1" else "500ms"),
               "--out", str(self.out), *extra]
        if gdb:
            cmd += ["--gdb", "0"] + ([] if wait else ["--gdb-wait", "off"])
        e = dict(os.environ)
        e["BB_GDB_TRACE"] = str(self.out) + ".trace"
        e.update(env or {})
        self.stdout = open(str(self.out) + ".log", "w")
        self.p = subprocess.Popen(cmd, stdout=self.stdout, stderr=subprocess.PIPE, text=True, env=e)
        self.port = None
        self.err = []
        self.got_port = threading.Event()
        threading.Thread(target=self._read_err, daemon=True).start()
        if gdb and not self.got_port.wait(120):
            self.p.kill()
            raise RuntimeError(f"{out}: no GDB port: {self.err}")

    def _read_err(self):
        for line in self.p.stderr:
            self.err.append(line)
            m = re.search(r"listening on 127\.0\.0\.1:(\d+)", line)
            if m:
                self.port = int(m.group(1))
                self.got_port.set()
        self.got_port.set()

    def gdb(self, lines, name="gdb", interrupt_after=None, timeout=600):
        script = Path(f"{self.out}.{name}.gdb")
        script.write_text("\n".join(["set pagination off", "set confirm off", "set remotetimeout 60",
                                     f"target remote :{self.port}", *lines]) + "\n")
        log = open(f"{self.out}.{name}.log", "w")
        g = subprocess.Popen([self.a.gdb, "-nx", "-batch", "-x", str(script), str(self.board.elf)],
                             stdout=log, stderr=subprocess.STDOUT)
        if interrupt_after is not None:
            time.sleep(interrupt_after)
            g.send_signal(signal.SIGINT)
        try:
            g.wait(timeout)
        except subprocess.TimeoutExpired:
            g.kill()
            g.wait()
        log.close()
        return Path(f"{self.out}.{name}.log").read_text()

    def finish(self, timeout=600):
        try:
            rc = self.p.wait(timeout)
        except subprocess.TimeoutExpired:
            self.p.kill()
            rc = self.p.wait()
        self.stdout.close()
        self.log = Path(str(self.out) + ".log").read_text() + "".join(self.err)
        self.rc = rc
        self.h = hashes(self.out) if rc == 0 else {}
        return self

    @property
    def renode_pid(self):
        m = re.search(r"started fresh \(pid (\d+)\)", self.log)
        return int(m.group(1)) if m else None


BP = lambda b: [f"break {b.bp}", "continue", 'printf "HIT1 pc=%#x\\n", $pc',
                "continue", 'printf "HIT2 pc=%#x\\n", $pc', "delete", "detach"]


def scripted(a, b, out, lines, **kw):
    r = Run(a, b, out, **kw)
    glog = r.gdb(lines)
    r.finish()
    r.glog = glog
    return r


def plain(a, b, out, **kw):
    return Run(a, b, out, gdb=False, **kw).finish()


def test(a, bs):
    o = Path(a.out)
    for b in bs.values():
        g = golden(b.golden)
        r = scripted(a, b, o / f"{b.name}-attach", ["detach"])
        check(r.rc == 0 and all_of(r.h, g),
              f"{b.name}: GDB attached at reset and detached without a stop: all four outputs equal the "
              f"committed hashes of the run without GDB ({b.golden.relative_to(ROOT)})")
        check(r.rc == 0 and not marks(r.out) and "no stop inside a slice" in r.log,
              f"{b.name}: no debug mark when nothing can have changed")
        r = scripted(a, b, o / f"{b.name}-bp", BP(b))
        hit = r.glog.count("Breakpoint 1,") == 2
        m = marks(r.out) if r.rc == 0 else []
        check(r.rc == 0 and hit and len(m) == 1,
              f"{b.name}: a breakpoint hit twice and continued: the run finishes and carries exactly one "
              f"debug mark ({m[0]['msg'][:60] + '...' if m else 'none'})")
        if b.name == "e1":
            rj = run_json(r.out) or {}
            check(rj.get("debug") is True and rj.get("options", {}).get("gdb") is True
                  and rj["options"].get("lookahead") is False,
                  "e1: run.json of a --gdb project run: debug true, options.gdb true, lookahead off")
            p = plain(a, b, o / "e1-plain")
            rj = run_json(p.out) or {}
            check(p.rc == 0 and "debug" not in rj and "gdb" not in rj.get("options", {}) and not marks(p.out)
                  and same(p.h, g),
                  "e1: a run without --gdb: no debug key, no gdb option, no mark, the committed hashes")
    b = bs["spike"]
    r = scripted(a, b, o / "spike-monitor", ["monitor start", "detach"])
    check(r.rc == 0 and "monitor commands are refused" in r.glog and same(r.h, golden(b.golden)),
          "spike: monitor refused with its message, the run unchanged")


def par(n, fn, items):
    with ThreadPoolExecutor(n) as ex:
        return list(ex.map(fn, items))


def trace_hold(path, hold):
    """The hold at the first breakpoint: no RUN_FOR boundary, the CPU's time inside the open RUN_FOR."""
    lines = [l for l in Path(path).read_text().splitlines() if re.match(r"\s*\d+\.\d+ ", l)]
    t_stop = i_stop = None
    for i, l in enumerate(lines):
        if " ->gdb T05" in l:
            t_stop, i_stop = float(l.split()[0]), i
            break
    if i_stop is None:
        return False, "no T05 in the trace"
    for l in lines[i_stop:]:
        if re.search(r" gdb-> (c|s|vCont;\S*) \[", l):  # the first resume after the hold
            t_c = float(l.split()[0])
            break
    else:
        return False, "no continue after the stop"
    window = [l for l in lines[i_stop:] if float(l.split()[0]) <= t_c]
    engine = [l for l in window if " engine " in l]
    m = re.search(r"run_for=Some\((\d+)\)", lines[i_stop])
    g = int(m.group(1)) if m else None
    after = [l for l in lines if float(l.split()[0]) > t_c and " cpu-state " in l]
    st = re.search(r"t_ns: (\d+)", after[0]) if after else None
    t_cpu = int(st.group(1)) if st else None
    good = (not engine and t_c - t_stop >= hold and g is not None and t_cpu is not None and t_cpu < g)
    return good, (f"held {t_c - t_stop:.2f} s; RUN_FOR boundaries during the hold: {len(engine)}; "
                  f"CPU time {t_cpu} ns < RUN_FOR end {g} ns")


def gate(a, bs):
    test(a, bs)
    o = Path(a.out)
    sp, e1 = bs["spike"], bs["e1"]
    gs = golden(sp.golden)
    bp1 = scripted(a, sp, o / "spike-bp-ref", BP(sp))

    # Attach mid-run (no wait), read, detach: identical to the 5 s run without GDB.
    base5 = plain(a, sp, o / "spike-plain-5s", dur="5s")
    for k in range(3):
        r = Run(a, sp, o / f"spike-mid-{k}", wait=False, dur="5s")
        time.sleep(1.0 + 0.4 * k)
        glog = r.gdb(['printf "ATTACH pc=%#x\\n", $pc', "info registers", "x/8wx $sp", "x/4wx 0x20000000",
                      "detach"])
        r.finish()
        check(r.rc == 0 and "ATTACH" in glog and same(r.h, base5.h) and not marks(r.out),
              f"spike: attach at {1.0 + 0.4 * k:.1f} s into a running 5 s run, read registers and RAM, "
              "detach: identical to the run without GDB, no mark")

    # The scripted breakpoint session, 20 times; with three steps, 20 times.
    reps = par(a.jobs, lambda k: scripted(a, sp, o / f"spike-bp-{k}", BP(sp)), range(a.reps))
    check(all(r.rc == 0 for r in reps) and len({tuple(sorted(r.h.items())) for r in reps}) == 1
          and reps[0].h == bp1.h,
          f"spike: the breakpoint session {a.reps + 1} times byte-identical (all four files)")
    step = [f"break {sp.bp}", "continue", "stepi", "stepi", "stepi", "continue", "delete", "detach"]
    reps = par(a.jobs, lambda k: scripted(a, sp, o / f"spike-bpstep-{k}", step), range(a.reps))
    check(all(r.rc == 0 for r in reps) and len({tuple(sorted(r.h.items())) for r in reps}) == 1,
          f"spike: the breakpoint session with three single-steps {a.reps} times byte-identical")
    check(same(reps[0].h, bp1.h),
          "spike: single-steps at the breakpoint change nothing more than the breakpoint")
    r = scripted(a, sp, o / "spike-bpread", [f"break {sp.bp}", "continue", "info registers", "x/16wx $sp",
                                             "bt", "x/4wx 0x20000000", "continue", "delete", "detach"])
    check(r.rc == 0 and same(r.h, bp1.h), "spike: register and RAM reads at the breakpoint change nothing")

    # A hold longer than the External Control timeout.
    hold = 5
    r = scripted(a, sp, o / "spike-hold", [f"break {sp.bp}", "continue", f"shell sleep {hold}", "continue",
                                           "delete", "detach"], env={"BB_RENODE_IO_TIMEOUT": "2"})
    check(r.rc == 0 and same(r.h, bp1.h),
          f"spike: a {hold} s hold at the breakpoint with the External Control timeout at 2 s: no failure, "
          "outputs equal to the session without the hold")
    good, why = trace_hold(str(r.out) + ".trace", hold)
    check(good, f"spike: during the hold the RUN_FOR stayed open and no grant advanced ({why})")

    # Ctrl-C, then detach or continue.
    for then in ("detach", "continue"):
        r = Run(a, sp, o / f"spike-intr-{then}", dur="5s")
        glog = r.gdb(["continue", 'printf "INTR pc=%#x\\n", $pc', then], interrupt_after=1.5)
        r.finish()
        check(r.rc == 0 and "SIGINT" in glog and same(r.h, base5.h) and not marks(r.out),
              f"spike: Ctrl-C then {then}: the run finishes, identical to the run without GDB, no mark")

    # monitor start/pause/halt (and a RunFor) at reset and at a breakpoint.
    for m in ("start", "pause", "halt", 'emulation RunFor "1"'):
        r = scripted(a, sp, o / f"spike-monitor-{m.split()[0]}", [f"monitor {m}", "detach"])
        check(r.rc == 0 and "monitor commands are refused" in r.glog and all_of(r.h, gs),
              f"spike: monitor {m} at reset: refused, the run unchanged")
    r = scripted(a, sp, o / "spike-monitor-bp", [f"break {sp.bp}", "continue", "monitor start", "detach"])
    check(r.rc == 0 and "monitor commands are refused" in r.glog and marks(r.out),
          "spike: monitor start at a breakpoint: refused, the run finishes")

    # A second debugger while one holds the CPU.
    r = Run(a, sp, o / "spike-second")
    first = threading.Thread(target=lambda: r.gdb([f"break {sp.bp}", "continue", "shell sleep 3", "delete",
                                                    "continue"], name="first"))
    first.start()
    time.sleep(1.5)
    second = r.gdb(['printf "SECOND\\n"'], name="second", timeout=60)
    first.join()
    r.finish()
    check(r.rc == 0 and "SECOND" not in second and "refused" in r.log,
          "spike: a second debugger is refused while one is attached; the first finishes the run")

    # GDB killed during a halt: the run finishes; bb killed during a halt: no Renode left.
    r = Run(a, sp, o / "spike-gdbkill")
    gl = Path(str(r.out) + ".kill.gdb")
    gl.write_text(f"set confirm off\ntarget remote :{r.port}\nbreak {sp.bp}\ncontinue\nshell sleep 60\n")
    g = subprocess.Popen([a.gdb, "-nx", "-batch", "-x", str(gl), str(sp.elf)], stdout=subprocess.DEVNULL,
                         stderr=subprocess.DEVNULL)
    time.sleep(4)
    g.kill()
    g.wait()
    r.finish(timeout=120)
    check(r.rc == 0 and marks(r.out), "spike: GDB killed while halted at a breakpoint: the run finishes "
          "(breakpoints removed, CPU continued)")
    r = Run(a, sp, o / "spike-bbkill")
    gl = Path(str(r.out) + ".kill.gdb")
    gl.write_text(f"set confirm off\ntarget remote :{r.port}\nbreak {sp.bp}\ncontinue\nshell sleep 30\n")
    g = subprocess.Popen([a.gdb, "-nx", "-batch", "-x", str(gl), str(sp.elf)], stdout=subprocess.DEVNULL,
                         stderr=subprocess.DEVNULL)
    time.sleep(4)
    kids = children(r.p.pid)
    r.p.kill()
    r.p.wait()
    deadline = time.time() + 15
    while time.time() < deadline and any(alive(k) for k in kids):
        time.sleep(0.2)
    g.kill()
    g.wait()
    check(kids and not any(alive(k) for k in kids),
          f"spike: bb killed while the CPU is halted: its Renode ({kids}) exits too (no orphan)")

    # The listener: 127.0.0.1 only.
    r = Run(a, sp, o / "spike-listen")
    ss = subprocess.run(["ss", "-ltnH"], capture_output=True, text=True).stdout
    mine = [l.split()[3] for l in ss.splitlines() if l.split()[3].endswith(f":{r.port}")]
    r.gdb(["detach"])
    r.finish()
    check(mine == [f"127.0.0.1:{r.port}"], f"spike: the GDB listener is bound to 127.0.0.1 only ({mine})")

    # A Renode pool is ignored: --gdb starts Renode fresh. The control: a
    # run without --gdb in the same environment leases from the pool.
    pdir = Path(f"/tmp/bb-gdb-gate-pool-{os.getpid()}")
    pdir.mkdir(mode=0o700, exist_ok=True)
    st = subprocess.run([a.bb, "pool", "start", "--slots", "1", "--dir", str(pdir), "--idle-exit", "300s"],
                        capture_output=True, text=True, timeout=120)
    m = re.search(r"BB_RENODE_POOL=(\S+)", st.stdout + st.stderr)
    if not m:
        bad(f"spike: could not start a Renode pool: {st.stdout} {st.stderr}")
    else:
        env = {"BB_RENODE_POOL": m.group(1).strip("'\"")}
        try:
            c = plain(a, sp, o / "spike-pool-control", env=env)
            r = scripted(a, sp, o / "spike-pool", ["detach"], env=env)
            check(r.rc == 0 and "started fresh" in r.log and all_of(r.h, gs) and "leased" in c.log,
                  "spike: --gdb with a Renode pool starts Renode fresh, identical to the run without GDB "
                  "(control: a run without --gdb leases from the pool)")
        finally:
            subprocess.run([a.bb, "pool", "stop", "--socket", env["BB_RENODE_POOL"]], capture_output=True)
    shutil.rmtree(pdir, ignore_errors=True)

    # E1: the breakpoint session 5 times; Ctrl-C, steps, continue; 1 500 steps.
    reps = par(min(a.jobs, 5), lambda k: scripted(a, e1, o / f"e1-bp-{k}", BP(e1)), range(a.e1_reps))
    check(all(r.rc == 0 for r in reps) and len({tuple(sorted(r.h.items())) for r in reps}) == 1,
          f"e1: the breakpoint session {a.e1_reps} times byte-identical")
    r = Run(a, e1, o / "e1-intr")
    glog = r.gdb(["continue", 'printf "INTR pc=%#x\\n", $pc', "stepi 700", 'printf "STEP pc=%#x\\n", $pc',
                  "continue"], interrupt_after=5)
    r.finish()
    check(r.rc == 0 and "SIGINT" in glog and "STEP pc" in glog and "exited normally" in glog,
          "e1: Ctrl-C, 700 single-steps, continue: the run finishes")
    r = scripted(a, e1, o / "e1-manystep", [f"break {e1.bp}", "continue", "stepi 1500",
                                             'printf "STEP pc=%#x\\n", $pc', "delete", "detach"])
    bounds = Path(str(r.out) + ".trace").read_text().count("engine before")
    check(r.rc == 0 and "STEP pc" in r.glog and same(r.h, reps[0].h),
          f"e1: 1 500 single-steps after the breakpoint, across grant boundaries and WFI: the run finishes, "
          f"identical to the breakpoint session ({bounds} RUN_FORs)")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("mode", choices=["test", "gate"])
    ap.add_argument("--bb", required=True)
    ap.add_argument("--gdb", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--work", required=True)
    ap.add_argument("--jobs", type=int, default=4)
    ap.add_argument("--reps", type=int, default=20)
    ap.add_argument("--e1-reps", type=int, default=5)
    a = ap.parse_args()
    v = subprocess.run([a.gdb, "--version"], capture_output=True, text=True).stdout.splitlines()
    print(f"gdb: using {a.gdb} ({v[0] if v else '?'})", flush=True)
    Path(a.out).mkdir(parents=True, exist_ok=True)
    bs = boards(a.work)
    t0 = time.time()
    (test if a.mode == "test" else gate)(a, bs)
    print(f"gdb: {a.mode}: {'FAILED: ' + str(len(FAILS)) if FAILS else 'all passed'} "
          f"({time.time() - t0:.0f} s)", flush=True)
    sys.exit(1 if FAILS else 0)


if __name__ == "__main__":
    main()
