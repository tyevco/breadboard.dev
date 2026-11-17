#!/usr/bin/env python3
"""Renode-only register and timing tests of Breadboard's nRF52840 POWER model (ADR-009 D6, G7).

BbNrf52Power (engines/cpu-renode/platforms/bb_nrf52_power.cs, platform nrf52840-bb@4, Phase 2 P4)
on the POWER/CLOCK base, in ADR-005's test-plan shape: a paused Renode with Breadboard's nRF52840
(nrf52840-bb.repl) and an idle gate firmware (nrf_saadc_idle.elf: WFI forever), driven over the
External Control API at lattice points (10 us lattice, serial execution) as the Renode engine
drives it. The register writes are the host's (the model treats a POFCON write the same from the
CPU or the host; a CPU write mid-cell and its "next lattice point" are the W-fw test's,
bb_wfw_test.py, on the W1 firmware). The CPU engine's view of the model is the state block
(bbState, bb_nrf52_state.cs), mapped and armed with the POWER bits (0x1F0) as the engine arms it:
its ring gives #pofen and #pofthr0..3 at their write times and the model's events (an
EVENTS_POFWARN raise, a refused write, a blocked NVMC erase). #pofwarn, the comparator's output, is
the model's GPIO input 0 on `clock`, set over External Control as the engine applies a digital_in.
The comparator itself (every threshold code's VPOF, VPOFTOL, VPOFHYST) is the package view
nordic/nrf52840-qiaa:pof, whose vectors `bb pkg test` runs (make packages).

Groups (every case starts from a fresh Renode; each case runs twice, byte-identical transcripts):
  reset     reset values: POFCON, EVENTS_POFWARN, INTEN.POFWARN, RESETREAS, GPREGRET(2), the host ID
  codes     every THRESHOLD code 4..15 with POF = 1: read back, the ring's #pofen/#pofthr bits at the
            write's time, no event with #pofwarn low; nrfx's second identical write changes nothing
  refused   codes 0..3 with POF = 1 and THRESHOLDVDDH != 0: refused (POFCON unchanged, error bit 0,
            a refused-write event naming the word); codes 0..3 with POF = 0 are accepted (stored)
  rise      #pofwarn rising with POF = 1 raises EVENTS_POFWARN at that lattice point (one event);
            with POF = 0 nothing; falling nothing; INTENSET.POFWARN pends IRQ 0 (NVIC ISPR0 bit 0)
            while the event is set, writing 0 to the event clears it
  enable    "already below at enable" (ADR-009 D6): POF 0 -> 1 with #pofwarn high raises at the next
            lattice point (the write is on one here: at once), a THRESHOLD change while enabled with
            #pofwarn high raises, a rewrite of the same value does not, a write that clears POF
            raises nothing
  regs      RESETREAS (the host preset, write 1 to clear), GPREGRET/GPREGRET2 (8 bits), TASKS_CONSTLAT
            and TASKS_LOWPWR (no-ops), the wrapped CLOCK (TASKS_HFCLKSTART -> EVENTS_HFCLKSTARTED,
            its INTENSET bit 0 pends IRQ 0 through the shared line)
  nvmc      POF blocks NVMC erases (ADR-009 D6): ERASEPAGE with POF = 1 and #pofwarn high is refused
            (the page keeps its data, a blocked-erase event); with #pofwarn low it erases

Usage: bb_power_test.py [--renode PATH] [--out DIR] [--jobs N] [--keep] [GROUP ...]
Exit status 0 on success.
"""

import argparse
import concurrent.futures
import hashlib
import os
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(ROOT / "examples/adc"))
sys.path.insert(0, str(ROOT / "examples/nrf-saadc"))
from renode_adc_test import CMD_GPIO, CMD_SYSTEM_BUS, WIDTH_DW, Client, find_renode  # noqa: E402

PLATFORMS = ROOT / "engines/cpu-renode/platforms"
PLUGIN_SOURCES = ["bb_lookahead.cs", "bb_stm32_timer.cs", "bb_stm32f0_adc.cs", "bb_nrf52_saadc.cs",
                  "bb_nrf52_pwm.cs", "bb_nrf52_state.cs", "bb_nrf52_power.cs"]
OVERLAY = PLATFORMS / "nrf52840-bb.repl"
FW = ROOT / "engines/cpu-renode/tests/firmware/nrf52840/build/nrf_saadc_idle.elf"
MACHINE = "u1"
L = 10_000
SYSBUS_WRITE = 1
GPIO_SET_STATE = 1

P = 0x40000000
TASKS_HFCLKSTART, TASKS_CONSTLAT, TASKS_LOWPWR = P + 0x000, P + 0x078, P + 0x07C
EV_HFCLKSTARTED, EV_POFWARN = P + 0x100, P + 0x108
INTENSET, INTENCLR, RESETREAS, POFCON = P + 0x304, P + 0x308, P + 0x400, P + 0x510
GPREGRET, GPREGRET2 = P + 0x51C, P + 0x520
HOST_ID, HOST_RESETREAS, HOST_RAISES, HOST_ERROR, HOST_INPUT, HOST_POFCON = (
    P + 0xF00, P + 0xF04, P + 0xF08, P + 0xF0C, P + 0xF10, P + 0xF14)
NVIC_ISPR0 = 0xE000E200
ST = 0x5FFE0000
NVMC = 0x4001E000
STATES = {4: "#pofen", 5: "#pofthr0", 6: "#pofthr1", 7: "#pofthr2", 8: "#pofthr3"}
EVENTS = {0x10: "raise", 0x11: "refused", 0x12: "nvmc_blocked"}


def plugin_loader(workdir, dll):
    srcs = ", ".join(f'r"{PLATFORMS / s}"' for s in PLUGIN_SOURCES)
    p = workdir / "load.py"
    p.write_text(f'''import System
from Antmicro.Renode.Utilities import TypeManager, AdHocCompiler
DLL = r"{dll}"
try:
    if not (System.IO.File.Exists(DLL) and TypeManager.Instance.ScanFile(DLL)):
        out = AdHocCompiler().Compile(System.Array[System.String]([{srcs}]))
        System.Reflection.Assembly.LoadFrom(out)
        if not TypeManager.Instance.ScanFile(out):
            raise Exception("ScanFile failed")
        System.IO.File.Copy(out, DLL + ".tmp", True)
        System.IO.File.Move(DLL + ".tmp", DLL, True)
except Exception as e:
    f = open(r"{workdir / 'plugin.err'}", "w")
    f.write("bb-power-test: plugin load failed: " + str(e))
    f.close()
    System.Environment.Exit(3)
''')
    return p


def plugin_dll(renode):
    key = hashlib.sha256()
    for src in PLUGIN_SOURCES:
        key.update(src.encode() + b"\0" + (PLATFORMS / src).read_bytes() + b"\0")
    key.update(str(Path(renode).resolve()).encode())
    base = os.environ.get("BB_CACHE_DIR") or str(Path.home() / ".cache/bb")
    cache = Path(base) / "power-test-plugins"
    cache.mkdir(parents=True, exist_ok=True)
    return cache / f"bb_plugins-{key.hexdigest()[:16]}.dll"


class Session:
    """A paused Renode with Breadboard's nRF52840, the idle firmware and the state block armed."""

    def __init__(self, renode, dll, keep=False):
        self.dir = Path(tempfile.mkdtemp(prefix="bb-power-"))
        self.keep = keep
        with socket.socket() as s:
            s.bind(("127.0.0.1", 0))
            port = s.getsockname()[1]
        (self.dir / "platform.repl").write_text(OVERLAY.read_text())
        lines = [f"include @{plugin_loader(self.dir, dll)}", f'mach create "{MACHINE}"',
                 f"machine LoadPlatformDescription @{self.dir / 'platform.repl'}",
                 f"sysbus LoadELF @{FW}",
                 'machine LoadPlatformDescriptionFromString "bbState: Miscellaneous.BbNrf52State @ sysbus '
                 f'{ST:#x}"', "sysbus.bbState Arm 0x1F0",
                 "emulation SetSeed 0", f'emulation SetGlobalQuantum "{L / 1e9:.8f}"',
                 "emulation SetGlobalSerialExecution true", "emulation SetGlobalAdvanceImmediately true",
                 "logLevel 2", f'emulation CreateExternalControlServer "bb" {port}']
        (self.dir / "session.resc").write_text("\n".join(lines) + "\n")
        self.log = self.dir / "renode.log"
        with open(self.log, "wb") as log:
            self.proc = subprocess.Popen(
                [renode, "--disable-xwt", "--plain", "--port", "-1", "session.resc"],
                cwd=self.dir, stdin=subprocess.DEVNULL, stdout=log, stderr=log)
        deadline = time.monotonic() + 180
        while True:
            if self.proc.poll() is not None:
                self.fail(f"renode exited during startup ({self.proc.returncode})")
            s = socket.socket()
            s.settimeout(0.5)
            try:
                s.connect(("127.0.0.1", port))
                if s.getsockname() == s.getpeername():
                    raise OSError("self-connect")
                break
            except OSError:
                s.close()
                if time.monotonic() > deadline:
                    self.fail("External Control server did not come up")
                time.sleep(0.05)
        s.settimeout(120)
        s.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        self.c = Client(s)
        md = self.c.get_machine(MACHINE)
        self.bus = self.c.instance(CMD_SYSTEM_BUS, md, "sysbus")
        self.port = self.c.instance(CMD_GPIO, md, "clock")
        self.now = 0
        self.seen = 0

    def r(self, addr):
        return self.c.read_u32s(self.bus, addr, 1)[0]

    def w(self, addr, *vals):
        data = struct.pack(f"<{len(vals)}I", *vals)
        self.c.request(CMD_SYSTEM_BUS, struct.pack("<iBBQI", self.bus, SYSBUS_WRITE, WIDTH_DW,
                                                   addr, len(vals)) + data)

    def pofwarn(self, level):
        self.c.request(CMD_GPIO, struct.pack("<ibi", self.port, GPIO_SET_STATE, 0) + bytes([level]))

    def run(self, ns=L):
        self.c.run_for(ns)
        self.now += ns

    def ring(self):
        """New ring entries: [(t_ns, name, level_or_value)]."""
        n = self.r(ST + 0x008)
        out = []
        for k in range(self.seen, n):
            j = k % 2048
            lo, hi, s, v = (self.r(ST + 0x1000 + 16 * j + 4 * i) for i in range(4))
            t = (lo | hi << 32) // 1_000_000
            state = s & 0xFF
            if state in EVENTS:
                out.append((t, EVENTS[state], v))
            else:
                out.append((t, STATES.get(state, f"state{state}"), s >> 8 & 1))
        self.seen = n
        self.w(ST + 0x00C, n)
        return out

    def tail(self):
        try:
            return "\n".join(self.log.read_text(errors="replace").splitlines()[-30:])
        except OSError:
            return ""

    def fail(self, msg):
        t = self.tail()
        self.close()
        raise RuntimeError(f"{msg}\n--- renode log (tail) ---\n{t}")

    def close(self):
        if self.proc.poll() is None:
            self.proc.kill()
        self.proc.wait()
        if not self.keep:
            shutil.rmtree(self.dir, ignore_errors=True)


def thr_bits(code):
    return {f"#pofthr{k}": code >> k & 1 for k in range(4)}


def run_group(renode, dll, group, keep):
    s = Session(renode, dll, keep)
    out, fails = [f"group {group}"], []

    def check(cond, msg):
        if not cond:
            fails.append(f"{group}: {msg}")
        return cond

    def note(line):
        out.append(line)

    try:
        s.run()  # boot to the idle WFI, one cell
        if group == "reset":
            vals = {n: s.r(a) for n, a in (("POFCON", POFCON), ("EVENTS_POFWARN", EV_POFWARN),
                                            ("INTENSET", INTENSET), ("RESETREAS", RESETREAS),
                                            ("GPREGRET", GPREGRET), ("GPREGRET2", GPREGRET2),
                                            ("ID", HOST_ID), ("ERROR", HOST_ERROR))}
            note(f"reset {vals}")
            check(vals["ID"] == 0x57504242, f"host ID {vals['ID']:#x}")
            check(all(vals[k] == 0 for k in ("POFCON", "EVENTS_POFWARN", "RESETREAS", "GPREGRET",
                                             "GPREGRET2", "ERROR")), f"reset values {vals}")
            check(vals["INTENSET"] & 0x4 == 0, "INTEN.POFWARN set after reset")
            check(s.ring() == [], "ring entries without a write")
        elif group == "codes":
            prev = 0
            for code in range(4, 16):
                word = 1 | code << 1
                t = s.now
                s.w(POFCON, word)
                s.w(POFCON, word)  # nrfx's second, identical write
                got = s.r(POFCON)
                ring = s.ring()
                note(f"code {code}: POFCON {got:#x}, ring {ring}")
                check(got == word, f"code {code}: POFCON reads {got:#x}")
                want = []
                if prev == 0:
                    want.append((t, "#pofen", 1))
                for k in range(4):
                    if (prev >> 1 >> k & 1) != (code >> k & 1):
                        want.append((t, f"#pofthr{k}", code >> k & 1))
                check(sorted(ring) == sorted(want), f"code {code}: ring {ring}, want {want}")
                check(s.r(EV_POFWARN) == 0 and s.r(HOST_RAISES) == 0, f"code {code}: an event with #pofwarn low")
                prev = word
                s.run()
        elif group == "refused":
            s.w(POFCON, 0x1F)
            s.ring()
            for code in range(0, 4):
                word = 1 | code << 1
                s.w(POFCON, word)
                ring = s.ring()
                got, err = s.r(POFCON), s.r(HOST_ERROR)
                note(f"code {code} with POF: POFCON {got:#x}, error {err:#x}, ring {ring}")
                check(got == 0x1F, f"code {code}: refused write took effect ({got:#x})")
                check(err & 1 == 1, f"code {code}: error bit 0 not set ({err:#x})")
                check(ring == [(s.now, "refused", word)], f"code {code}: ring {ring}")
                s.w(HOST_ERROR, 0)
            s.w(POFCON, 0x11F)  # THRESHOLDVDDH 1 (V28 in the high-voltage mode)
            ring = s.ring()
            got = s.r(POFCON)
            note(f"THRESHOLDVDDH 1: POFCON {got:#x}, ring {ring}")
            check(got == 0x1F and ring == [(s.now, "refused", 0x11F)], f"VDDH refusal: {got:#x} {ring}")
            s.w(HOST_ERROR, 0)
            for code in range(0, 4):  # stored without POF (nrfx's disable writes a code too)
                s.w(POFCON, code << 1)
                got, err = s.r(POFCON), s.r(HOST_ERROR)
                check(got == code << 1 and err == 0, f"code {code} without POF: {got:#x}, error {err:#x}")
            note(f"codes 0..3 without POF accepted; ring {s.ring()}")
        elif group == "rise":
            s.w(INTENSET, 0x4)
            s.w(POFCON, 0x1F)
            s.ring()
            s.run()
            t = s.now
            s.pofwarn(1)
            ev, ispr, raises = s.r(EV_POFWARN), s.r(NVIC_ISPR0), s.r(HOST_RAISES)
            ring = s.ring()
            note(f"rise with POF: EVENTS_POFWARN {ev}, ISPR0 {ispr:#x}, raises {raises}, ring {ring}")
            check(ev == 1 and raises == 1, f"no event on a rising #pofwarn ({ev}, {raises})")
            check(ispr & 1 == 1, f"IRQ 0 not pending ({ispr:#x})")
            check(ring == [(t, "raise", 0x1F)], f"ring {ring}")
            s.w(EV_POFWARN, 0)
            s.run()
            s.pofwarn(0)
            s.run()
            check(s.r(EV_POFWARN) == 0 and s.r(HOST_RAISES) == 1, "an event on a falling #pofwarn")
            s.w(POFCON, 0x1E)  # POF off, V28 kept
            s.run()
            s.pofwarn(1)
            check(s.r(EV_POFWARN) == 0 and s.r(HOST_RAISES) == 1, "an event with POF = 0")
            note(f"falling and POF = 0: no event; ring {s.ring()}")
            s.w(INTENCLR, 0x4)
            check(s.r(INTENSET) & 0x4 == 0, "INTENCLR.POFWARN")
        elif group == "enable":
            s.w(INTENSET, 0x4)
            s.w(POFCON, 0x1E)  # V28, POF off
            s.run()
            s.pofwarn(1)
            s.run()
            s.ring()
            t = s.now
            s.w(POFCON, 0x1F)  # enable with #pofwarn high: at the next lattice point (here: now)
            s.w(POFCON, 0x1F)  # the identical second write: nothing
            ring = s.ring()
            note(f"enable with #pofwarn high: ring {ring}")
            check([x for x in ring if x[1] == "raise"] == [(t, "raise", 0x1F)], f"enable-while-below: {ring}")
            s.w(EV_POFWARN, 0)
            s.run()
            t = s.now
            s.w(POFCON, 0x1D)  # V27 while enabled: a reconfiguration
            ring = s.ring()
            note(f"threshold change with #pofwarn high: ring {ring}")
            check([x for x in ring if x[1] == "raise"] == [(t, "raise", 0x1D)], f"reconfiguration: {ring}")
            s.w(EV_POFWARN, 0)
            s.run()
            s.w(POFCON, 0x1D)
            s.w(POFCON, 0x1C)  # POF off
            ring = s.ring()
            check(not [x for x in ring if x[1] == "raise"], f"rewrite or disable raised: {ring}")
            check(s.r(HOST_RAISES) == 2, f"raises {s.r(HOST_RAISES)}, want 2")
            note(f"rewrite and disable: no event; raises {s.r(HOST_RAISES)}")
        elif group == "regs":
            s.w(HOST_RESETREAS, 0x5)
            a = s.r(RESETREAS)
            s.w(RESETREAS, 0x4)
            b = s.r(RESETREAS)
            s.w(GPREGRET, 0x1A5)
            s.w(GPREGRET2, 0xFFFF)
            g1, g2 = s.r(GPREGRET), s.r(GPREGRET2)
            s.w(TASKS_CONSTLAT, 1)
            s.w(TASKS_LOWPWR, 1)
            s.w(INTENSET, 0x1)
            s.w(TASKS_HFCLKSTART, 1)
            s.run()
            hf, ispr, inten = s.r(EV_HFCLKSTARTED), s.r(NVIC_ISPR0), s.r(INTENSET)
            note(f"RESETREAS {a:#x} -> {b:#x}; GPREGRET {g1:#x}, GPREGRET2 {g2:#x}; "
                 f"HFCLKSTARTED {hf}, ISPR0 {ispr:#x}, INTENSET {inten:#x}")
            check(a == 0x5 and b == 0x1, f"RESETREAS {a:#x} -> {b:#x}")
            check(g1 == 0xA5 and g2 == 0xFF, f"GPREGRET {g1:#x} {g2:#x}")
            check(hf == 1, f"the wrapped CLOCK: EVENTS_HFCLKSTARTED {hf}")
            check(ispr & 1 == 1 and inten & 0x5 == 0x1, f"CLOCK's interrupt: ISPR0 {ispr:#x}, INTENSET {inten:#x}")
            check(s.r(POFCON) == 0 and s.r(EV_POFWARN) == 0, "TASKS_CONSTLAT/LOWPWR changed POWER")
        elif group == "nvmc":
            page = 0x80000
            s.w(page, 0x12345678)  # the flash is memory: a word write by the host
            s.w(NVMC + 0x504, 2)  # CONFIG = EEN
            s.w(POFCON, 0x1F)
            s.pofwarn(1)
            s.ring()
            t = s.now
            s.w(NVMC + 0x508, page)  # ERASEPAGE while POF blocks it
            ring = s.ring()
            kept = s.r(page)
            note(f"ERASEPAGE with POF and #pofwarn high: word {kept:#x}, ring {ring}")
            check(kept == 0x12345678, f"the blocked erase erased ({kept:#x})")
            check([x for x in ring if x[1] == "nvmc_blocked"] == [(t, "nvmc_blocked", 0x508)], f"ring {ring}")
            s.run()
            s.pofwarn(0)
            s.w(NVMC + 0x508, page)
            s.run(100 * 1_000_000)  # tERASEPAGE 85 ms
            erased = s.r(page)
            note(f"ERASEPAGE with #pofwarn low: word {erased:#x}")
            check(erased == 0xFFFFFFFF, f"the allowed erase did not erase ({erased:#x})")
    finally:
        s.close()
    return "\n".join(out) + "\n", fails


GROUPS = ["reset", "codes", "refused", "rise", "enable", "regs", "nvmc"]


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--renode")
    ap.add_argument("--out", default=str(ROOT / "out/nrf-power-model"))
    ap.add_argument("--jobs", type=int, default=int(os.environ.get("POWER_JOBS", "4")))
    ap.add_argument("--keep", action="store_true")
    ap.add_argument("groups", nargs="*", default=GROUPS)
    a = ap.parse_args()
    if not FW.is_file():
        sys.exit(f"{FW} missing (make firmware)")
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    renode = find_renode(a.renode)
    dll = plugin_dll(renode)
    jobs = [(g, rep) for g in a.groups for rep in (0, 1)]
    t0 = time.monotonic()
    results = {jobs[0]: run_group(renode, dll, jobs[0][0], a.keep)}  # compiles the plugin if cold
    with concurrent.futures.ThreadPoolExecutor(max_workers=max(1, a.jobs)) as ex:
        futs = {ex.submit(run_group, renode, dll, g, a.keep): (g, rep) for g, rep in jobs[1:]}
        for f in concurrent.futures.as_completed(futs):
            results[futs[f]] = f.result()
    failures = []
    for g in a.groups:
        (t0_, f0), (t1_, _) = results[(g, 0)], results[(g, 1)]
        failures += f0
        if t0_ != t1_:
            failures.append(f"{g}: two runs differ")
        (out / f"{g}.txt").write_text(t0_)
        print(f"{g}: {'FAIL' if f0 or t0_ != t1_ else 'ok'}")
    for msg in failures:
        print(f"  FAIL: {msg}")
    print(f"wall time {time.monotonic() - t0:.1f} s; {len(jobs)} runs; transcripts in {out}")
    if failures:
        print(f"bb-power-test: FAIL ({len(failures)} failures)")
        return 1
    print("bb-power-test: PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
