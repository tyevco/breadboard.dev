#!/usr/bin/env python3
"""Renode-only tests of Breadboard's nRF52840 SAADC model (ADR-008 D7, D8).

Drives Renode 1.17.0 with Breadboard's nRF52840 (engines/cpu-renode/platforms/
nrf52840-bb.repl, nrf52840-bb@2: BbNrf52Saadc at 0x40007000, its host port
BbSaadcHostPort at 0x5FFF0100, the UART tap BbUartLog at 0x5FFF8000) through
the External Control API, paused, as the Renode engine does. The plugin
sources are compiled once with Renode's own compiler into an assembly cached
under --out. The shape is ADR-005's test plan 2 and 3 (examples/adc/
bb_adc_test.py).

Groups:

  registers  (nrf_saadc_idle.elf: WFI, no interrupt; the host drives the
             SAADC): reset values; ENABLE gating; the exact acquisition
             starts a_k, sample instants s_k = a_k + TACQ and DONE / END
             instants s_k + 2 us for every TACQ x RESOLUTION x OVERSAMPLE x
             BURST, computed independently here; an 8x BURST: one SAMPLE,
             8 back-to-back acquisitions, one result, 8 entries in the
             acquisition ring that feeds the #smp toggle; oversampling
             without BURST (one acquisition per SAMPLE, the result after the
             8th); round half up per sample and of the mean; every gain, the
             internal and the VDD/4 reference (VDD a second timed input),
             single-ended saturation, differential sign extension; PSELP =
             VDD and VDDHDIV5; sample-and-hold at a lattice point (a push at
             t is not seen by a sample at t, is at t + 1 ns); LIMITH/LIMITL at
             DONE; EasyDMA (PTR latched at START, MAXCNT, AMOUNT, END, a
             result without START dropped, STOP ends the buffer); SAMPLE while
             busy ignored; SAMPLERATE timer mode; CALIBRATEOFFSET; the IRQ
             line (NVIC pending bit of IRQ 7); the TACQ < 2 L refusal on a
             loaded input and no refusal at TACQ >= 2 L or unloaded; a late
             push sets the port's error word. Run twice: the transcripts must
             be byte-identical.
  firmware   (nrf_saadc_burst.elf): the RTC-paced 8x burst, as Zephyr's
             simple mode drives it: 8 acquisitions per burst at the D7
             instants, the END interrupt wakes the CPU (lookahead block:
             WFI exits), pushes between bursts wake nothing, the UARTE0
             lines reach the tap with their bytes and times.

Usage: bb_saadc_test.py [--renode PATH] [--out DIR] [--keep] [GROUP ...]
Exit status 0 on success.
"""

import argparse
import hashlib
import math
import os
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import time
from fractions import Fraction
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
sys.path.insert(0, str(ROOT / "examples/adc"))
from renode_adc_test import CMD_SYSTEM_BUS, Client, find_renode, WIDTH_DW  # noqa: E402

PLATFORMS = ROOT / "engines/cpu-renode/platforms"
PLUGIN_SOURCES = ["bb_lookahead.cs", "bb_stm32_timer.cs", "bb_stm32f0_adc.cs", "bb_nrf52_saadc.cs"]
OVERLAY = PLATFORMS / "nrf52840-bb.repl"
FW = ROOT / "engines/cpu-renode/tests/firmware/nrf52840/build"
MACHINE = "u1"
L_NS = 10_000
US, MS = 1_000, 1_000_000
SYSBUS_WRITE = 1

SA = 0x40007000
START, SAMPLE, STOP, CAL = SA + 0x000, SA + 0x004, SA + 0x008, SA + 0x00C
EV = {"STARTED": 0x100, "END": 0x104, "DONE": 0x108, "RESULTDONE": 0x10C, "CALIBRATEDONE": 0x110,
      "STOPPED": 0x114}
INTEN, INTENSET, STATUS, ENABLE = SA + 0x300, SA + 0x304, SA + 0x400, SA + 0x500
RESOLUTION, OVERSAMPLE, SAMPLERATE = SA + 0x5F0, SA + 0x5F4, SA + 0x5F8
PTR, MAXCNT, AMOUNT = SA + 0x62C, SA + 0x630, SA + 0x634
PORT = 0x5FFF0100
UARTLOG = 0x5FFF8000
LOOKAHEAD = 0x5FFF0000
NVIC_ISPR0 = 0xE000E200
BUF = 0x20010000

TACQ = [3000, 5000, 10000, 15000, 20000, 40000]
RES_BITS = [8, 10, 12, 14]
GAIN = [Fraction(1, 6), Fraction(1, 5), Fraction(1, 4), Fraction(1, 3), Fraction(1, 2), Fraction(1),
        Fraction(2), Fraction(4)]
TCONV = 2000


def ch(n, reg):
    return SA + 0x510 + 16 * n + {"PSELP": 0, "PSELN": 4, "CONFIG": 8, "LIMIT": 12}[reg]


def config(gain=0, refvdd=False, tacq=5, diff=False, burst=False):
    return gain << 8 | int(refvdd) << 12 | tacq << 16 | int(diff) << 20 | int(burst) << 24


def code(uv, gain=0, res=2, ref_uv=600_000, diff=False):
    """ADR-008 D7: round_half_up(V * G / REF * 2^(RES - m)), saturated."""
    bits = RES_BITS[res]
    x = Fraction(uv) * GAIN[gain] / ref_uv * 2 ** (bits - int(diff))
    c = math.floor(x + Fraction(1, 2))
    lo, hi = (-(2 ** (bits - 1)), 2 ** (bits - 1) - 1) if diff else (0, 2 ** bits - 1)
    return max(lo, min(hi, c))


def mean(codes):
    return math.floor(Fraction(sum(codes), len(codes)) + Fraction(1, 2))


class Failures(list):
    def check(self, cond, msg):
        if not cond:
            self.append(msg)
            print("  FAIL: " + msg)
        return cond


class Session:
    """A paused Renode with Breadboard's nRF52840 (nrf52840-bb@2)."""

    def __init__(self, renode, elf, dll, lookahead=False, keep=False):
        self.dir = Path(tempfile.mkdtemp(prefix="bb-saadc-"))
        self.keep = keep
        with socket.socket() as s:
            s.bind(("127.0.0.1", 0))
            port = s.getsockname()[1]
        (self.dir / "platform.repl").write_text(OVERLAY.read_text())
        lines = [f"include @{plugin_loader(self.dir, dll)}", f'mach create "{MACHINE}"',
                 f"machine LoadPlatformDescription @{self.dir / 'platform.repl'}"]
        if lookahead:
            lines.append('machine LoadPlatformDescriptionFromString '
                         f'"bbLookahead: Miscellaneous.BbLookahead @ sysbus {LOOKAHEAD:#x}"')
        lines += [f"sysbus LoadELF @{elf}", "emulation SetSeed 0",
                  f'emulation SetGlobalQuantum "{L_NS / 1e9:.8f}"',
                  "emulation SetGlobalSerialExecution true",
                  "emulation SetGlobalAdvanceImmediately true"]
        if lookahead:
            lines.append("sysbus.bbLookahead Arm")
        lines += ["logLevel 2", f'emulation CreateExternalControlServer "bb" {port}']
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
        self.md = self.c.get_machine(MACHINE)
        self.bus = self.c.instance(CMD_SYSTEM_BUS, self.md, "sysbus")
        self.now = 0
        self.acq_seen = 0
        self.smp_seen = 0

    def r(self, addr, n=1):
        v = self.c.read_u32s(self.bus, addr, n)
        return v if n > 1 else v[0]

    def w(self, addr, *vals):
        data = struct.pack(f"<{len(vals)}I", *vals)
        self.c.request(CMD_SYSTEM_BUS, struct.pack("<iBBQI", self.bus, SYSBUS_WRITE, WIDTH_DW,
                                                   addr, len(vals)) + data)

    def run(self, ns):
        if ns > 0:
            self.c.run_for(ns)
            self.now += ns

    def run_to(self, t):
        assert t >= self.now, (t, self.now)
        self.run(t - self.now)

    def push(self, entries):
        """Timed inputs [(t_ns, input, uV)], one batch (ADR-005 D5.1)."""
        words = [0xBA7C0000 | len(entries)]
        for t, i, uv in entries:
            words += [t & 0xFFFFFFFF, (t >> 32) | (i << 16), uv]
        for k in range(0, len(words), 900):
            self.w(PORT + 0x1000, *words[k:k + 900])

    def ev(self, name):
        return self.r(SA + EV[name])

    def clear_events(self):
        for off in EV.values():
            self.w(SA + off, 0)
        for n in range(8):
            self.w(SA + 0x118 + 8 * n, 0)
            self.w(SA + 0x11C + 8 * n, 0)

    def acquisitions(self):
        """New acquisition-ring entries since the last call: [(a_ns, input, channel)]."""
        n = self.r(PORT + 0x2C)
        out = []
        for k in range(self.acq_seen, n):
            lo, hi, info, _ = self.r(PORT + 0x2000 + 16 * (k % 256), 4)
            out.append((lo | hi << 32, info & 0xFF, info >> 8 & 0xFF))
        self.acq_seen = n
        return out

    def samples(self):
        """New sample-ring entries: [(s_ns, input, channel, uV, code)]."""
        n = self.r(PORT + 0x30)
        out = []
        for k in range(self.smp_seen, n):
            lo, hi, uv, info = self.r(PORT + 0x3000 + 16 * (k % 256), 4)
            c = info & 0xFFFF
            out.append((lo | hi << 32, info >> 16 & 0xFF, info >> 24, uv, c - 0x10000 if c & 0x8000 else c))
        self.smp_seen = n
        return out

    def result(self, i):
        w = self.r(BUF + 4 * (i // 2))
        v = (w >> (16 * (i % 2))) & 0xFFFF
        return v - 0x10000 if v & 0x8000 else v

    def error(self):
        return self.r(PORT + 4, 4)

    def fail(self, msg):
        t = self.tail()
        self.close()
        sys.exit(f"FAIL: {msg}\n--- renode log (tail) ---\n{t}")

    def tail(self):
        err = self.dir / "plugin.err"
        try:
            text = "\n".join(self.log.read_text(errors="replace").splitlines()[-30:])
            return text + ("\n" + err.read_text() if err.is_file() else "")
        except OSError:
            return ""

    def close(self):
        if self.proc.poll() is None:
            self.proc.kill()
        self.proc.wait()
        if not self.keep:
            shutil.rmtree(self.dir, ignore_errors=True)


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
    f.write("bb-saadc-test: plugin load failed: " + str(e))
    f.close()
    System.Environment.Exit(3)
''')
    return p


# --- groups -----------------------------------------------------------------

def setup_single(s, cfg, pselp=1, res=2, ovs=0, maxcnt=1, start=True):
    s.w(ENABLE, 1)
    for n in range(8):
        s.w(ch(n, "PSELP"), 0)
    s.w(ch(0, "CONFIG"), cfg)
    s.w(ch(0, "PSELP"), pselp)
    s.w(RESOLUTION, res)
    s.w(OVERSAMPLE, ovs)
    s.w(PTR, BUF)
    s.w(MAXCNT, maxcnt)
    s.clear_events()
    if start:
        s.w(START, 1)


def group_registers(env, f):
    print("registers: reset values, timing, transfer, EasyDMA, events, refusal")
    out = []
    s = env.session(FW / "nrf_saadc_idle.elf")
    try:
        # Reset values (PS v1.1 6.23.14).
        got = {"CONFIG0": s.r(ch(0, "CONFIG")), "LIMIT0": s.r(ch(0, "LIMIT")), "RESOLUTION": s.r(RESOLUTION),
               "OVERSAMPLE": s.r(OVERSAMPLE), "STATUS": s.r(STATUS), "ENABLE": s.r(ENABLE),
               "ID": s.r(PORT), "UARTLOG_ID": s.r(UARTLOG)}
        want = {"CONFIG0": 0x00020000, "LIMIT0": 0x7FFF8000, "RESOLUTION": 1, "OVERSAMPLE": 0, "STATUS": 0,
                "ENABLE": 0, "ID": 0x50534242, "UARTLOG_ID": 0x4C554242}
        f.check(got == want, f"reset values {got}, expected {want}")
        out.append(f"reset {sorted(got.items())}")
        # Trace and acquisition rings on AIN0..7 and VDD; no refusal (L = 0).
        s.w(PORT + 0x84, 0x3FF)
        s.w(PORT + 0x88, 0x3FF)
        # ENABLE = 0: tasks ignored.
        s.w(ch(0, "PSELP"), 1)
        s.w(SAMPLE, 1)
        s.run(100 * US)
        f.check(not s.acquisitions() and s.ev("DONE") == 0, "SAMPLE with ENABLE = 0 was not ignored")

        # Timing: TACQ x RESOLUTION x OVERSAMPLE x BURST.
        cases = 0
        for tacq in range(6):
            for res in range(4):
                for ovs in (0, 1, 3):
                    for burst in (False, True):
                        s.run_to((s.now // L_NS + 1) * L_NS + 3 * US)
                        setup_single(s, config(tacq=tacq, burst=burst), res=res, ovs=ovs)
                        s.acquisitions()
                        s.samples()
                        t0 = s.now
                        n = 2 ** ovs
                        per_sample = 1 if burst or ovs == 0 else n
                        exp_a = []
                        a = t0
                        for k in range(n):
                            if k > 0 and not burst:
                                # One SAMPLE per acquisition: the host
                                # triggers it 1 us after the previous DONE.
                                a = exp_a[-1] + TACQ[tacq] + TCONV + 1 * US
                            elif k > 0:
                                a = exp_a[-1] + TACQ[tacq] + TCONV
                            exp_a.append(a)
                        s.w(SAMPLE, 1)
                        for k in range(1, per_sample):
                            s.run_to(exp_a[k])
                            f.check(s.r(STATUS) == 0, f"STATUS busy at a new SAMPLE (tacq {tacq})")
                            s.w(SAMPLE, 1)
                        done = exp_a[-1] + TACQ[tacq] + TCONV
                        s.run_to(done - 1)
                        end_early = s.ev("END")
                        s.run_to(done)
                        acq = s.acquisitions()
                        smp = s.samples()
                        exp_s = [x + TACQ[tacq] for x in exp_a]
                        ok = ([x[0] for x in acq] == exp_a and [x[0] for x in smp] == exp_s
                              and end_early == 0 and s.ev("END") == 1 and s.ev("RESULTDONE") == 1
                              and s.r(AMOUNT) == 1)
                        f.check(ok, f"timing tacq={TACQ[tacq]} res={res} ovs={ovs} burst={burst}: "
                                f"a {[x[0] - t0 for x in acq]} expected {[x - t0 for x in exp_a]}, "
                                f"s {[x[0] - t0 for x in smp]}, END {end_early}->{s.ev('END')}")
                        out.append(f"timing tacq={TACQ[tacq]} res={res} ovs={ovs} burst={int(burst)} "
                                   f"a={[x[0] - t0 for x in acq]} s={[x[0] - t0 for x in smp]} "
                                   f"end={done - t0}")
                        cases += 1
        print(f"  {cases} TACQ x RESOLUTION x OVERSAMPLE x BURST cases: a_k, s_k = a_k + TACQ, "
              "a_k+1 = s_k + 2 us (BURST), END at s_last + 2 us")

        # The 8x BURST of E2: one SAMPLE, 8 acquisitions, one result, the
        # mean of 8 rounded codes rounded half up.
        s.run_to((s.now // L_NS + 1) * L_NS)
        setup_single(s, config(tacq=5, burst=True), res=2, ovs=3)
        s.acquisitions()
        s.samples()
        t0 = s.now
        # Codes 1138 1138 1139 1139 1140 1140 1141 1141: mean 1139.5 -> 1140.
        vals = [999_800 + 440 * k for k in range(8)]
        exp_s = [t0 + 40 * US + k * 42 * US for k in range(8)]
        s.push([(x - 1, 0, v) for x, v in zip(exp_s, vals)])
        s.w(SAMPLE, 1)
        s.run_to(exp_s[-1] + TCONV)
        acq, smp = s.acquisitions(), s.samples()
        codes = [code(v) for v in vals]
        f.check(len(acq) == 8 and [x[0] for x in acq] == [x - 40 * US for x in exp_s],
                f"8x burst: {len(acq)} acquisitions at {[x[0] - t0 for x in acq]}")
        f.check([x[3] for x in smp] == vals and [x[4] for x in smp] == codes,
                f"8x burst samples {smp}, expected uV {vals} codes {codes}")
        f.check(s.result(0) == mean(codes) and s.r(PORT + 0x18) >= 1,
                f"8x burst result {s.result(0)}, expected mean {mean(codes)}")
        out.append(f"burst8 acq={[x[0] - t0 for x in acq]} codes={codes} result={s.result(0)}")
        print(f"  8x BURST: one SAMPLE -> 8 acquisitions ({[x[0] - t0 for x in acq]} ns), one "
              f"result {s.result(0)} = round_half_up(mean {codes})")

        # Transfer: gains, references, saturation, differential, VDD inputs.
        for gain in range(8):
            for refvdd in (False, True):
                for uv in (0, 1, 299_999, 300_000, 1_234_567, 3_600_000):
                    s.run_to((s.now // L_NS + 1) * L_NS)
                    setup_single(s, config(gain=gain, refvdd=refvdd, tacq=0), res=3)
                    vdd = 3_000_000
                    s.push([(s.now, 0, uv), (s.now, 8, vdd)])
                    s.samples()
                    s.w(SAMPLE, 1)
                    s.run(5 * US)
                    want = code(uv, gain, 3, Fraction(vdd, 4) if refvdd else 600_000)
                    got = s.result(0)
                    f.check(got == want, f"code gain={gain} refvdd={refvdd} uv={uv}: {got}, expected {want}")
                    out.append(f"code gain={gain} refvdd={int(refvdd)} uv={uv} -> {got}")
        # Differential: AIN0 - AIN1, negative codes sign-extended.
        for p_uv, n_uv in ((1_000_000, 1_500_000), (2_000_000, 100_000), (0, 3_000_000)):
            s.run_to((s.now // L_NS + 1) * L_NS)
            setup_single(s, config(gain=0, tacq=0, diff=True), res=2, start=False)
            s.w(ch(0, "PSELN"), 2)
            s.w(START, 1)
            s.push([(s.now, 0, p_uv), (s.now, 1, n_uv)])
            s.w(SAMPLE, 1)
            s.run(5 * US)
            want = code(p_uv - n_uv, 0, 2, 600_000, diff=True)
            f.check(s.result(0) == want, f"differential {p_uv}-{n_uv}: {s.result(0)}, expected {want}")
            out.append(f"diff {p_uv}-{n_uv} -> {s.result(0)}")
            s.w(ch(0, "PSELN"), 0)
        # PSELP = VDD (input 8) and VDDHDIV5 (input 9 / 5).
        for pselp, inp, uv, conv in ((9, 8, 2_950_000, lambda u: u), (0x0D, 9, 4_200_000, lambda u: (2 * u + 5) // 10)):
            s.run_to((s.now // L_NS + 1) * L_NS)
            setup_single(s, config(tacq=0), pselp=pselp, res=2)
            s.push([(s.now, inp, uv)])
            s.w(SAMPLE, 1)
            s.run(5 * US)
            want = code(conv(uv))
            f.check(s.result(0) == want, f"PSELP {pselp:#x}: {s.result(0)}, expected {want}")
            out.append(f"pselp {pselp:#x} uv={uv} -> {s.result(0)}")
        print("  transfer: 8 gains x {internal, VDD/4} x 6 inputs, differential, PSELP = VDD, VDDHDIV5")

        # Sample-and-hold at a lattice point: push at T; a sample at T reads
        # the old value, at T + 1 ns the new one.
        for off, want_uv in ((0, 500_000), (1, 700_000)):
            T = (s.now // L_NS + 2) * L_NS
            setup_single(s, config(tacq=0), res=2)
            s.push([(T - L_NS, 0, 500_000), (T, 0, 700_000)])
            s.samples()
            s.run_to(T + off - 3000)
            s.w(SAMPLE, 1)
            s.run(10 * US)
            smp = s.samples()
            f.check(smp and smp[0][0] == T + off and smp[0][3] == want_uv,
                    f"sample-and-hold: a sample at T+{off} read {smp}, expected {want_uv} uV")
            out.append(f"hold T+{off} -> {smp[0][3] if smp else None}")
        print("  sample-and-hold: a push at lattice T is not seen at s = T, is at s = T + 1 ns")

        # LIMIT events at DONE.
        s.run_to((s.now // L_NS + 1) * L_NS)
        setup_single(s, config(tacq=0), res=2)
        s.w(ch(0, "LIMIT"), (2000 << 16) | 100)
        s.push([(s.now, 0, 2_000_000)])  # code 2276 >= 2000
        s.w(SAMPLE, 1)
        s.run_to(s.now + 3 * US + TCONV - 1)
        before = (s.r(SA + 0x118), s.r(SA + 0x11C))
        s.run(1)
        after = (s.r(SA + 0x118), s.r(SA + 0x11C))
        f.check(before == (0, 0) and after == (1, 0), f"LIMITH: before DONE {before}, at DONE {after}")
        s.w(START, 1)
        s.clear_events()
        s.push([(s.now, 0, 10_000)])  # code 17 <= 100
        s.w(SAMPLE, 1)
        s.run(5 * US)
        f.check((s.r(SA + 0x118), s.r(SA + 0x11C)) == (0, 1), "LIMITL not set for a code below LOW")
        s.w(ch(0, "LIMIT"), 0x7FFF8000)
        out.append(f"limit {before} {after}")
        print("  LIMITH / LIMITL set at DONE")

        # EasyDMA: PTR latched at START; MAXCNT; AMOUNT; END; no START.
        s.run_to((s.now // L_NS + 1) * L_NS)
        setup_single(s, config(tacq=0), res=2, maxcnt=3)
        s.w(PTR, BUF + 0x100)  # after START: must not move the buffer
        s.w(BUF, 0, 0)
        s.w(BUF + 0x100, 0, 0)
        for k in range(3):
            s.push([(s.now, 0, 600_000 * (k + 1) // 2)])
            s.w(SAMPLE, 1)
            s.run(5 * US)
            f.check(s.r(AMOUNT) == k + 1 and s.ev("END") == int(k == 2), f"EasyDMA after result {k}: AMOUNT "
                    f"{s.r(AMOUNT)}, END {s.ev('END')}")
        res3 = [s.result(k) for k in range(3)]
        f.check(res3 == [code(600_000 * (k + 1) // 2) for k in range(3)] and s.r(BUF + 0x100) == 0,
                f"EasyDMA results {res3} at the START-latched PTR")
        dw = s.r(PORT + 0x28)
        s.w(SAMPLE, 1)
        s.run(5 * US)
        f.check(s.r(PORT + 0x28) == dw, "a result after END (no START) was written")
        out.append(f"dma {res3}")
        # STOP while started: END then STOPPED.
        setup_single(s, config(tacq=5, burst=True), res=2, ovs=3, maxcnt=4)
        s.w(SAMPLE, 1)
        s.run(50 * US)
        s.w(STOP, 1)
        f.check(s.ev("END") == 1 and s.ev("STOPPED") == 1 and s.r(STATUS) == 0 and s.r(AMOUNT) == 0,
                "STOP while started: END, STOPPED, not busy, AMOUNT 0")
        acq_before = s.r(PORT + 0x20)
        s.run(400 * US)
        f.check(s.r(PORT + 0x20) == acq_before, "acquisitions went on after STOP")
        print("  EasyDMA: PTR latched at START, MAXCNT, AMOUNT, END; nothing written without START; "
              "STOP ends the buffer")

        # SAMPLE while busy is ignored.
        s.run_to((s.now // L_NS + 1) * L_NS)
        setup_single(s, config(tacq=5), res=2)
        s.acquisitions()
        s.w(SAMPLE, 1)
        ign = s.r(PORT + 0x24)
        s.run(10 * US)
        s.w(SAMPLE, 1)
        s.run(40 * US)
        f.check(s.r(PORT + 0x24) == ign + 1 and len(s.acquisitions()) == 1, "SAMPLE while busy not ignored")
        print("  SAMPLE while busy ignored")

        # SAMPLERATE timer mode: CC = 801 at 16 MHz -> a_k = a_0 + ceil(k * 50062.5 ns).
        s.run_to((s.now // L_NS + 1) * L_NS)
        setup_single(s, config(tacq=0), res=2, maxcnt=100)
        s.w(SAMPLERATE, 1 << 12 | 801)
        s.acquisitions()
        t0 = s.now
        s.w(SAMPLE, 1)
        s.run(260 * US)
        s.w(STOP, 1)
        s.w(SAMPLERATE, 0)
        got = [x[0] - t0 for x in s.acquisitions()]
        want = [math.ceil(Fraction(k * 801 * 125, 2)) for k in range(6)]
        f.check(got == want, f"timer mode starts {got}, expected {want}")
        out.append(f"timers {got}")
        print(f"  SAMPLERATE timers: starts {got} ns")

        # CALIBRATEOFFSET.
        s.clear_events()
        s.w(CAL, 1)
        s.run(TCONV - 1)
        c0 = s.ev("CALIBRATEDONE")
        s.run(1)
        f.check(c0 == 0 and s.ev("CALIBRATEDONE") == 1, "CALIBRATEDONE not exactly 2 us after the task")

        # IRQ 7: END with INTEN sets the NVIC pending bit.
        s.w(0xE000E280, 1 << 7)  # ICPR
        s.w(INTEN, 0)
        setup_single(s, config(tacq=0), res=2)
        s.w(SAMPLE, 1)
        s.run(5 * US)
        p0 = s.r(NVIC_ISPR0) >> 7 & 1
        s.w(INTENSET, 1 << 1)  # END
        p1 = s.r(NVIC_ISPR0) >> 7 & 1
        s.w(INTEN, 0)
        f.check(p0 == 0 and p1 == 1, f"IRQ 7 pending: without INTEN {p0}, with INTEN.END {p1}")
        print("  IRQ 7 (SAADC) pends on END with INTEN.END")

        # The TACQ >= 2 L refusal (ADR-008 D7) on a loaded input.
        s.w(PORT + 0x80, L_NS)
        s.w(PORT + 0x84, 1)  # AIN0 loaded
        for tacq, loaded, want in ((4, True, 0), (2, False, 0), (2, True, 1 << 5)):
            s.w(PORT + 4, 0)
            s.w(PORT + 0x84, 1 if loaded else 0)
            setup_single(s, config(tacq=tacq), res=2)
            s.w(SAMPLE, 1)
            s.run(50 * US)
            err = s.error()
            f.check(err[0] == want, f"refusal TACQ {TACQ[tacq]} loaded={loaded}: error {err[0]:#x}")
            if want:
                f.check(err[3] == 0 and s.r(PORT + 0x34) == TACQ[tacq],
                        f"refusal detail: input {err[3]}, TACQ {s.r(PORT + 0x34)}")
            out.append(f"refusal tacq={TACQ[tacq]} loaded={int(loaded)} err={err[0]:#x}")
        s.w(PORT + 4, 0)
        s.w(PORT + 0x80, 0)
        print("  refusal: TACQ 10 us < 2 L on a loaded input sets error bit 5; TACQ 20 us or unloaded do not")

        # A late push (before a sample already taken) sets error bit 0.
        setup_single(s, config(tacq=0), res=2)
        s.w(SAMPLE, 1)
        s.run(10 * US)
        s.push([(s.now - 8 * US, 0, 1)])
        f.check(s.error()[0] & 1 == 1, "a late push did not set the error word")
        s.w(PORT + 4, 0)
        print("  host port: a late push sets the error word")
        out.append(f"warnings {s.tail().count('bb_nrf52_saadc')}")
    finally:
        s.close()
    return "\n".join(out) + "\n"


def group_firmware(env, f):
    print("firmware: nrf_saadc_burst.elf, RTC-paced 8x BURST, VDD, UARTE0 lines")
    s = env.session(FW / "nrf_saadc_burst.elf", lookahead=True)
    out = []
    try:
        s.w(PORT + 0x84, 0x1)       # AIN0 loaded: the acquisition ring
        s.w(PORT + 0x88, 0x101)     # AIN0 and VDD traced
        s.w(PORT + 0x80, L_NS)
        s.push([(0, 0, 1_234_000), (0, 8, 3_000_000)])
        exits0 = s.r(LOOKAHEAD, 8)[5]
        s.run(12 * MS)
        exits_before = s.r(LOOKAHEAD, 8)[5]
        # A push between bursts must not wake the CPU.
        s.push([(12 * MS, 0, 1_500_000)])
        s.run(2 * MS)
        exits_after = s.r(LOOKAHEAD, 8)[5]
        s.run(6 * MS)
        acq = s.acquisitions()
        smp = s.samples()
        bursts = []
        for a, inp, _ in acq:
            if not bursts or a - bursts[-1][-1] > 50 * US:
                bursts.append([])
            bursts[-1].append(a)
        f.check(len(bursts) >= 3 and all(len(b) == 8 for b in bursts),
                f"bursts of AIN0 acquisitions: {[len(b) for b in bursts]}, expected 8 each")
        f.check(all(b[k + 1] - b[k] == 42 * US for b in bursts for k in range(7)),
                "burst acquisitions not 42 us apart (TACQ 40 us + tCONV 2 us)")
        ain = [x for x in smp if x[1] == 0]
        vdd = [x for x in smp if x[1] == 8]
        f.check(len(vdd) == len(bursts) and all(x[3] == 3_000_000 for x in vdd),
                f"VDD samples {len(vdd)} for {len(bursts)} bursts")
        f.check(exits_after == exits_before, f"a push between bursts woke the CPU ({exits_before} -> {exits_after})")
        # The UART tap: one line per burst, the codes as the model converted them.
        n = s.r(UARTLOG + 4)
        raw = []
        for k in range(n):
            lo, hi = s.r(UARTLOG + 0x100 + 8 * (k % 1024), 2)
            raw.append((lo | (hi & 0xFFFF) << 32, hi >> 16 & 0xFF, hi >> 24))
        text = bytes(b for _, _, b in raw).decode()
        lines = text.splitlines()
        want = []
        for k, b in enumerate(bursts):
            c = mean([x[4] for x in ain if b[0] <= x[0] <= b[-1] + 40 * US])
            want.append(f"b={k} ain0={c} vdd={code(3_000_000)}")
        f.check(lines[:len(want)] == want, f"UART lines {lines}, expected {want}")
        f.check(all(raw[k][0] <= raw[k + 1][0] for k in range(len(raw) - 1)) and all(u == 0 for _, u, _ in raw),
                "UART tap times not monotonic or not UARTE0")
        out.append(f"bursts {[[a for a in b] for b in bursts]}")
        out.append(f"lines {lines}")
        out.append(f"wfi exits {exits0} {exits_before} {exits_after}")
        print(f"  {len(bursts)} bursts of 8 acquisitions 42 us apart, first at {bursts[0][0]} ns; "
              f"UART lines {lines}; WFI exits unchanged across a push ({exits_before})")
    finally:
        s.close()
    return "\n".join(out) + "\n"


GROUPS = {"registers": group_registers, "firmware": group_firmware}


class Env:
    def __init__(self, renode, out, keep):
        self.renode, self.keep = renode, keep
        key = hashlib.sha256()
        for src in PLUGIN_SOURCES:
            key.update(src.encode() + b"\0" + (PLATFORMS / src).read_bytes() + b"\0")
        key.update(str(Path(renode).resolve()).encode())
        # Off /mnt/c (AGENTS.md: slow, and Renode reads the assembly).
        base = os.environ.get("BB_CACHE_DIR") or str(Path.home() / ".cache/bb")
        cache = Path(base) / "saadc-test-plugins"
        cache.mkdir(parents=True, exist_ok=True)
        self.dll = cache / f"bb_plugins-{key.hexdigest()[:16]}.dll"

    def session(self, elf, lookahead=False):
        return Session(self.renode, elf, self.dll, lookahead, self.keep)


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--renode")
    ap.add_argument("--out", default=str(ROOT / "out/nrf-saadc-model"))
    ap.add_argument("--keep", action="store_true")
    ap.add_argument("groups", nargs="*", default=list(GROUPS))
    a = ap.parse_args()
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    env = Env(find_renode(a.renode), out, a.keep)
    for e in ("nrf_saadc_idle.elf", "nrf_saadc_burst.elf"):
        if not (FW / e).is_file():
            sys.exit(f"{FW / e} missing: make firmware")
    f = Failures()
    t0 = time.monotonic()
    for g in a.groups:
        text = GROUPS[g](env, f)
        (out / f"{g}.txt").write_text(text)
        if g == "registers":
            again = GROUPS[g](env, Failures())
            f.check(again == text, "registers: two runs differ")
            print("  determinism: two runs byte-identical")
    print(f"wall time {time.monotonic() - t0:.1f} s; transcripts in {out}")
    if f:
        print(f"bb-saadc-test: FAIL ({len(f)} failures)")
        return 1
    print("bb-saadc-test: PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
