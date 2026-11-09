#!/usr/bin/env python3
"""Renode-only tests of Breadboard's STM32F072 ADC model (ADR-005).

Drives Renode 1.17.0 with Breadboard's STM32F072 overlay
(engines/cpu-renode/platforms/stm32f072-bb.repl: BbStm32Timer with TRGO for
TIM1/2/3/15, BbStm32F0Adc and its host port BbAdcHostPort) through the
External Control API, paused, as the Renode engine does. The plugin sources
are compiled once with Renode's own compiler into one assembly cached under
--out (like the engine's cache).

Groups (ADR-005 "Test plan"):

  pwm        TIM3 CH1 PWM on PA6 (tim3_pwm.elf) through the overlay's
             restated alternate-function wiring: the same PA6 edges as on
             the stock platform (step 1).
  trgo       Test plan 1: host-programmed timers, TRGO rising edges (seen by
             the ADC's trigger inputs, the port's edge ring) at every update
             instant with UIE = 0 and 1, none with UDIS, one per EGR.UG with
             MMS = 000 and 010, MMS read back, MMS = 100 gives none.
  registers  Test plan 2 (adc_idle.elf, the host drives the ADC): reset
             values, ADRDY, ADCAL duration and DR[6:0], the exact sample and
             end-of-conversion instants of every SMP x RES x CKMODE (against
             the D4 formulas computed here with fractions), sample-and-hold
             at a lattice point, code12 incl. saturation, RES and alignment,
             AWD per RM0091 Tables 47-48, EOC cleared by a DR read, OVR with
             OVRMOD 0/1, WAIT, CONT back to back, DISCEN, SCANDIR, ADSTP,
             EXTEN rising/falling/both from TIM3 TRGO, a trigger during a
             conversion ignored, ADSTART kept in hardware-trigger single
             mode, DMA requests, the port (firmware access ignored, a late
             push sets the error word); the transcript of two runs is
             byte-identical.
  firmware   Test plan 3 (adc_board.elf): TIM3-TRGO-triggered conversions at
             exactly s_k = T_k + 1063 ns and c_k = T_k + 4750 ns, the AWD
             interrupt drives PA5 (ALARM) at c_k of exactly the out-of-window
             conversions (also after a SetValue crossing), pushes between
             conversions never wake the CPU (the lookahead block's WFI-exit
             counter), and the timed-input path and the SetValue path give
             byte-identical transcripts.

Usage: bb_adc_test.py [--renode PATH] [--out DIR] [--keep] [GROUP ...]
Exit status 0 on success.
"""

import argparse
import hashlib
import math
import socket
import struct
import subprocess
import sys
import tempfile
import time
import shutil
from fractions import Fraction
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
sys.path.insert(0, str(HERE))
from renode_adc_test import (  # noqa: E402
    CMD_ADC, CMD_GPIO, CMD_SYSTEM_BUS, Client, find_renode, WIDTH_DW,
)

PLATFORMS = ROOT / "engines/cpu-renode/platforms"
PLUGIN_SOURCES = ["bb_lookahead.cs", "bb_stm32_timer.cs", "bb_stm32f0_adc.cs"]
OVERLAY = PLATFORMS / "stm32f072-bb.repl"
STOCK = "platforms/cpus/stm32f072.repl"
FW = HERE / "firmware/build"
MACHINE = "u1"
L_NS = 10_000
US, MS = 1_000, 1_000_000
SYSBUS_WRITE = 1

ADC = 0x40012400
ISR, IER, CR, CFGR1, CFGR2, SMPR, TR, CHSELR, DR, CCR = (
    ADC + o for o in (0x00, 0x04, 0x08, 0x0C, 0x10, 0x14, 0x20, 0x28, 0x40, 0x308))
PORT = 0x5FFF0100
LOOKAHEAD = 0x5FFF0000
TIM = {1: 0x40012C00, 2: 0x40000000, 3: 0x40000400, 15: 0x40014000}
TRG = {1: 0, 2: 2, 3: 3, 15: 4}  # RM0091 Table 45
STATUS = 0x20000000

VREF_UV = 3_300_000
ADRDY, EOSMP, EOC, EOSEQ, OVR, AWD = 1, 2, 4, 8, 16, 128
ADEN, ADDIS, ADSTART, ADSTP, ADCAL = 1, 2, 4, 16, 1 << 31


def code12(uv):
    """ADR-005 D5.2 (approver decision 2): round half up, saturating."""
    return min(4095, math.floor(Fraction(uv * 4095, VREF_UV) + Fraction(1, 2)))


def adc_cycle_ns(ckmode):
    return {0: Fraction(10**9, 14_000_000), 1: Fraction(2 * 10**9, 8_000_000),
            2: Fraction(4 * 10**9, 8_000_000)}[ckmode]


PCLK_NS = Fraction(10**9, 8_000_000)
SMPL = [Fraction(x) for x in ("1.5", "7.5", "13.5", "28.5", "41.5", "55.5", "71.5", "239.5")]
SAR = [Fraction(x) for x in ("12.5", "11.5", "9.5", "7.5")]


def instants(t0, ckmode, res, smp, i=0):
    """D4: sample and EOC instants of conversion i of a chain triggered at t0
    (back to back), from RM0091 13.4 and the datasheet (tlatr, WLATENCY)."""
    cyc = adc_cycle_ns(ckmode)
    lat = Fraction(11, 4) if ckmode == 1 else Fraction(21, 8)
    w = {0: Fraction(3, 2) * cyc + 2 * PCLK_NS, 1: Fraction(9, 2) * PCLK_NS,
         2: Fraction(17, 2) * PCLK_NS}[ckmode]
    a = (lat + i * (SMPL[smp] + SAR[res])) * cyc
    s = t0 + math.ceil(a + SMPL[smp] * cyc)
    c = t0 + math.ceil(a + (SMPL[smp] + SAR[res]) * cyc + w)
    return s, c


class Failures(list):
    def check(self, cond, msg):
        if not cond:
            self.append(msg)
            print("  FAIL: " + msg)
        return cond


class Session:
    """A paused Renode with Breadboard's (or the stock) STM32F072."""

    def __init__(self, renode, elf, dll, platform="bb", lookahead=False, keep=False, repl_edit=None):
        self.dir = Path(tempfile.mkdtemp(prefix="bb-adc5-"))
        self.keep = keep
        with socket.socket() as s:
            s.bind(("127.0.0.1", 0))
            port = s.getsockname()[1]
        lines = []
        if platform == "bb":
            text = OVERLAY.read_text()
            if repl_edit:
                text = repl_edit(text)
            (self.dir / "platform.repl").write_text(text)
            lines.append(f"include @{plugin_loader(self.dir, dll)}")
            repl = self.dir / "platform.repl"
        else:
            repl = STOCK
        lines += [f'mach create "{MACHINE}"', f"machine LoadPlatformDescription @{repl}"]
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

    # --- bus and time ---
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

    # --- the host port ---
    def push(self, entries):
        """Timed inputs [(t_ns, channel, uV)], one batch (ADR-005 D5.1)."""
        words = [0xBA7C0000 | len(entries)]
        for t, ch, uv in entries:
            words += [t & 0xFFFFFFFF, (t >> 32) | (ch << 16), uv]
        for k in range(0, len(words), 900):
            self.w(PORT + 0x1000, *words[k:k + 900])

    def conversions(self):
        return self.r(PORT + 0x18)

    def ring(self, k):
        w = self.r(PORT + 0x200 + 32 * (k % 64), 8)
        return {"T": w[0] | w[1] << 32, "s": w[2] | w[3] << 32, "c": w[4] | w[5] << 32,
                "ch": w[6] & 0xFF, "awd": bool(w[6] >> 8 & 1), "ovr": bool(w[6] >> 9 & 1),
                "eoseq": bool(w[6] >> 10 & 1), "code": w[7] & 0xFFFF, "dr": w[7] >> 16}

    def edges(self):
        """Trigger-input edges recorded since reset: [(t_ns, input, rising)]."""
        n = self.r(PORT + 0x2C)
        out = []
        for k in range(max(0, n - 64), n):
            lo, hi = self.r(PORT + 0xA00 + 8 * (k % 64), 2)
            v = lo | hi << 32
            out.append((v & ((1 << 56) - 1), (v >> 56) & 0x7F, bool(v >> 63)))
        return out

    def error(self):
        return self.r(PORT + 4)

    def tail(self):
        try:
            return "\n".join(self.log.read_text(errors="replace").splitlines()[-30:])
        except OSError:
            return ""

    def log_text(self):
        return self.log.read_text(errors="replace")

    def fail(self, msg):
        t = self.tail()
        self.close()
        sys.exit(f"FAIL: {msg}\n--- renode log (tail) ---\n{t}")

    def close(self):
        if self.proc.poll() is None:
            self.proc.kill()
        self.proc.wait()
        if not self.keep:
            shutil.rmtree(self.dir, ignore_errors=True)


def plugin_loader(workdir, dll):
    """A loader like the engine's: ScanFile the cached assembly, else compile
    every plugin source into it."""
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
    print("bb-adc-test: plugin load failed: " + str(e))
    System.Environment.Exit(3)
''')
    return p


# --- groups -----------------------------------------------------------------

def group_pwm(env, f):
    print("pwm: TIM3 CH1 PWM on PA6, stock platform vs Breadboard's overlay")
    out = {}
    for plat in ("stock", "bb"):
        s = env.session(FW / "tim3_pwm.elf", platform=plat)
        try:
            gpio = s.c.instance(CMD_GPIO, s.md, "gpioPortA")
            s.c.gpio_register_event(gpio, 6, 0)
            s.run(MS)
            out[plat] = [(t, st) for _, t, st in s.c.events]
        finally:
            s.close()
    f.check(len(out["bb"]) >= 18, f"PA6: {len(out['bb'])} edges in 1 ms, expected a 100 us PWM")
    f.check(out["bb"] == out["stock"], "PA6 PWM edges differ between the overlay and the stock platform")
    print(f"  PA6: {len(out['bb'])} edges, first {out['bb'][:4]}; identical to the stock platform")
    return f"pwm {out['bb']}\n"


def timer_start(s, n, mms, uie=False, udis=False, arr=99):
    b = TIM[n]
    s.w(b + 0x28, 9)        # PSC: 10 MHz / 10 = 1 MHz
    s.w(b + 0x2C, arr)      # ARR: an update every ARR ticks on Renode
    s.w(b + 0x04, mms << 4)
    s.w(b + 0x0C, 1 if uie else 0)
    s.w(b + 0x24, 0)        # CNT: a restart counts a full period
    s.w(b + 0x00, 1 | (2 if udis else 0))


def group_trgo(env, f):
    print("trgo: BbStm32Timer TRGO (test plan 1)")
    lines = []
    for n in (3, 1, 2, 15):
        for uie in (False, True):
            s = env.session(FW / "adc_idle.elf")
            try:
                s.run(L_NS)
                t0 = s.now
                timer_start(s, n, 2, uie=uie)
                f.check(s.r(TIM[n] + 4) >> 4 & 7 == 2, f"TIM{n}: MMS does not read back 010")
                s.run(MS)
                rising = [t for t, i, r in s.edges() if r and i == TRG[n]]
                want = [t0 + k * 99 * US for k in range(1, 11)]
                f.check(rising == want, f"TIM{n} UIE={int(uie)}: TRGO at {rising}, expected {want}")
                lines.append(f"trgo tim{n} uie={int(uie)} {rising}")
            finally:
                s.close()
    print("  TIM1/2/3/15 MMS=010: TRGO at t0 + k * 99 us exactly, with UIE 0 and 1")
    s = env.session(FW / "adc_idle.elf")
    try:
        s.run(L_NS)
        timer_start(s, 3, 2, udis=True)
        s.run(MS)
        f.check(not [e for e in s.edges() if e[1] == 3], "UDIS: TRGO pulsed")
        s.w(TIM[3] + 0x00, 0)
        for mms in (0, 2):
            s.w(TIM[3] + 0x04, mms << 4)
            n0 = len(s.edges())
            s.w(TIM[3] + 0x14, 1)  # EGR.UG
            e = s.edges()[n0:]
            f.check(len(e) == 2 and e[0][2] and not e[1][2] and e[0][0] == s.now,
                    f"MMS={mms:03b}: EGR.UG gave edges {e}, expected one pulse at {s.now}")
        s.w(TIM[3] + 0x04, 4 << 4)
        f.check(s.r(TIM[3] + 4) >> 4 & 7 == 4, "MMS = 100 does not read back")
        n0 = len(s.edges())
        timer_start(s, 3, 4)
        s.run(MS)
        f.check(len(s.edges()) == n0, "MMS = 100 pulsed TRGO")
        f.check("MMS = 100 is not modelled" in s.log_text(), "no warning for MMS = 100")
        lines.append("trgo udis/ug/mms ok")
    finally:
        s.close()
    print("  UDIS: none; EGR.UG: one pulse with MMS 000 and 010; MMS 100: none, one warning")
    return "\n".join(lines) + "\n"


def enable(s, ckmode=0, cfgr1=0, smp=0, ch=1 << 1):
    """ADC off, configure, on (host writes while paused)."""
    if s.r(CR) & ADEN:
        if s.r(CR) & ADSTART:
            s.w(CR, ADSTP)
        s.w(CR, ADDIS)
    s.w(CFGR2, ckmode << 30)
    s.w(CFGR1, cfgr1)
    s.w(SMPR, smp)
    s.w(CHSELR, ch)
    s.w(ISR, 0xFF)
    s.w(CR, ADEN)


def convert_sw(s, settle=300 * US):
    """One software-triggered sequence at the current time; returns the new
    ring entries."""
    k0 = s.conversions()
    s.w(CR, ADSTART)
    s.run(settle)
    return [s.ring(k) for k in range(k0, s.conversions())]


def group_registers(env, f):
    print("registers: BbStm32F0Adc register level (test plan 2)")
    lines = []

    def note(x):
        lines.append(x)

    s = env.session(FW / "adc_idle.elf", repl_edit=lambda t: t.replace(
        "    referenceVoltage: 3.3\n", "    referenceVoltage: 3.3\n    calibrationFactor: 0x55\n", 1))
    try:
        regs = [s.r(a) for a in (ISR, IER, CR, CFGR1, CFGR2, SMPR, TR, CHSELR, DR, CCR)]
        f.check(regs == [0, 0, 0, 0, 0, 0, 0x0FFF0000, 0, 0, 0], f"reset values {regs}")
        f.check(s.r(PORT) == 0x50414242, "port ID")
        note(f"reset {regs}")
        # ADCAL: 83 cycles of the 14 MHz asynchronous clock, then DR[6:0].
        s.run(L_NS)
        t0 = s.now
        s.w(CR, ADCAL)
        cal = math.ceil(83 * adc_cycle_ns(0))
        s.run(cal - 1)
        f.check(s.r(CR) & ADCAL, f"ADCAL cleared before {cal} ns")
        s.run(1)
        f.check(not s.r(CR) & ADCAL and s.r(DR) == 0x55,
                f"ADCAL at t0 + {cal} ns: CR {s.r(CR):#x}, DR {s.r(DR):#x} (calibration factor 0x55)")
        note(f"adcal {s.now - t0}")
        s.run_to((s.now // L_NS + 1) * L_NS)
        s.w(CR, ADEN)
        f.check(s.r(ISR) & ADRDY, "ADRDY not set after ADEN")
        # Timing of every SMP x RES x CKMODE (D4), software trigger.
        s.push([(0, 1, 1_000_000)])
        bad = 0
        for ck in (0, 1, 2):
            for res in range(4):
                for smp in range(8):
                    enable(s, ckmode=ck, cfgr1=res << 3, smp=smp)
                    s.run_to((s.now // L_NS + 1) * L_NS)
                    t0 = s.now
                    e = convert_sw(s)
                    want = instants(t0, ck, res, smp)
                    if len(e) != 1 or (e[0]["s"], e[0]["c"]) != want or e[0]["T"] != t0:
                        bad += 1
                        f.check(False, f"CKMODE={ck} RES={res} SMP={smp}: {e}, expected (s, c) = {want}")
                    note(f"timing {ck} {res} {smp} {e[0]['s'] - t0} {e[0]['c'] - t0}")
                    s.r(DR)
        print(f"  timing: {96 - bad}/96 SMP x RES x CKMODE combinations at the D4 instants")
        # Sample-and-hold at a lattice point: a push at lattice T is seen by
        # a sample at T + 1 ns, not by one at T (strictly earlier entries).
        enable(s, ckmode=1)
        for ds, want_uv in ((0, 1_000_000), (1, 2_000_000)):
            T = (s.now // L_NS + 3) * L_NS
            s.push([(T - L_NS, 1, 1_000_000), (T, 1, 2_000_000)])
            s.run_to(T - 1063 + ds)  # s = T + ds (CKMODE 01, SMP 000)
            e = convert_sw(s, 30 * US)
            f.check(e and e[0]["s"] == T + ds and e[0]["code"] == code12(want_uv),
                    f"sample at T + {ds} ns: {e}, expected code {code12(want_uv)}")
            s.r(DR)
        note("hold ok")
        print("  sample-and-hold: a push at lattice T is seen from T + 1 ns on, not at T")
        # code12: rounding half up, saturation; RES and ALIGN (Figure 37).
        for uv in (0, 1, 402, 403, 329_999, 330_000, 1_650_000, 3_299_999, 3_300_000, 3_400_000, 4_000_000):
            s.push([(s.now, 1, uv)])
            s.run(L_NS)
            e = convert_sw(s, 30 * US)
            f.check(e and e[0]["code"] == code12(uv) and e[0]["dr"] == code12(uv),
                    f"{uv} uV: {e}, expected code {code12(uv)}")
            s.r(DR)
            note(f"code {uv} {e[0]['code']}")
        uv = 2_531_087
        for res in range(4):
            for align in (0, 1):
                enable(s, ckmode=1, cfgr1=res << 3 | align << 5)
                s.push([(s.now, 1, uv)])
                s.run(L_NS)
                e = convert_sw(s, 30 * US)
                bits = 12 - 2 * res
                d = code12(uv) >> (12 - bits)
                if align:
                    d <<= 2 if bits == 6 else 16 - bits
                f.check(e and e[0]["dr"] == d, f"RES={res} ALIGN={align}: DR {e and e[0]['dr']:#x}, expected {d:#x}")
                s.r(DR)
        print("  code12: round half up, saturated at 4095; RES and ALIGN per Figure 37")
        # AWD: Tables 47-48.
        LT, HT = 1240, 2482
        s.w(TR, HT << 16 | LT)
        cases = [
            # (cfgr1, uV on ch1, uV on ch2, expected AWD per conversion)
            (1 << 23 | 1 << 22 | 1 << 26, 3_000_000, 3_000_000, [True, False]),   # single ch1
            (1 << 23 | 1 << 22 | 2 << 26, 3_000_000, 3_000_000, [False, True]),   # single ch2
            (1 << 23, 1_500_000, 500_000, [False, True]),                         # all channels
            (1 << 22 | 1 << 26, 3_000_000, 3_000_000, [False, False]),            # AWDEN = 0
            (1 << 23 | 1 << 22 | 1 << 26 | 2 << 3, 1_002_800, 0, [True, False]),  # 8 bits: raw < LT
            (1 << 23 | 1 << 22 | 1 << 26, 1_002_800, 0, [False, False]),          # 12 bits: inside
            (1 << 23 | 1 << 22 | 1 << 26 | 1 << 5, 3_000_000, 0, [True, False]),  # left aligned
        ]
        for cfg, u1, u2, want in cases:
            enable(s, ckmode=1, cfgr1=cfg, ch=0b110)
            s.push([(s.now, 1, u1), (s.now, 2, u2)])
            s.run(L_NS)
            s.w(ISR, 0xFF)
            e = convert_sw(s, 30 * US)
            got = [x["awd"] for x in e]
            f.check(got == want and bool(s.r(ISR) & AWD) == any(want),
                    f"AWD cfgr1={cfg:#x} ({u1}, {u2}) uV: per conversion {got}, expected {want}")
            s.w(ISR, 0xFF)
            s.r(DR)
        print("  AWD: single channel, all channels, AWDEN = 0, 8-bit raw compare, left alignment")
        # EOC cleared by a DR read; OVR with OVRMOD 0 and 1.
        enable(s, ckmode=1)
        s.push([(s.now, 1, 1_000_000)])
        s.run(L_NS)
        convert_sw(s, 30 * US)
        f.check(s.r(ISR) & EOC, "EOC not set")
        first = s.r(DR)
        f.check(not s.r(ISR) & EOC, "a DR read does not clear EOC")
        for ovrmod in (0, 1):
            enable(s, ckmode=1, cfgr1=ovrmod << 12)
            s.push([(s.now, 1, 1_000_000)])
            s.run(L_NS)
            convert_sw(s, 30 * US)
            s.push([(s.now, 1, 2_000_000)])
            s.run(L_NS)
            convert_sw(s, 30 * US)
            want = code12(2_000_000) if ovrmod else code12(1_000_000)
            isr = s.r(ISR)
            f.check(isr & OVR and s.r(DR) == want, f"OVRMOD={ovrmod}: ISR {isr:#x}, DR (expected {want})")
        note(f"eoc ovr {first}")
        print("  EOC cleared by a DR read; OVR keeps DR (OVRMOD 0) or overwrites it (1)")
        # CONT: back to back; ADSTP stops it.
        s.push([(s.now, 1, 1_000_000)])
        enable(s, ckmode=1, cfgr1=1 << 13 | 1 << 12, ch=1 << 1)
        s.run_to((s.now // L_NS + 1) * L_NS)
        t0 = s.now
        k0 = s.conversions()
        s.w(CR, ADSTART)
        s.run(40 * US)
        e = [s.ring(k) for k in range(k0, s.conversions())]
        want = [instants(t0, 1, 0, 0, i) for i in range(len(e))]
        f.check(len(e) >= 9 and [(x["s"], x["c"]) for x in e] == want,
                f"CONT: {[(x['s'] - t0, x['c'] - t0) for x in e]}, expected {[(a - t0, b - t0) for a, b in want]}")
        f.check(s.r(CR) & ADSTART, "CONT cleared ADSTART")
        s.w(CR, ADSTP)
        n = s.conversions()
        s.run(40 * US)
        f.check(s.conversions() == n and not s.r(CR) & (ADSTART | ADSTP), "ADSTP did not stop CONT")
        note(f"cont {len(e)}")
        print(f"  CONT: {len(e)} conversions back to back at the D4 instants; ADSTP stops it")
        # WAIT: the next conversion starts when DR is read.
        enable(s, ckmode=1, cfgr1=1 << 13 | 1 << 14, ch=1 << 1)
        s.run_to((s.now // L_NS + 1) * L_NS)
        k0 = s.conversions()
        s.w(CR, ADSTART)
        s.run(30 * US)
        f.check(s.conversions() - k0 == 1, f"WAIT: {s.conversions() - k0} conversions before the DR read")
        t_read = s.now
        s.r(DR)
        s.run(30 * US)
        e = [s.ring(k) for k in range(k0, s.conversions())]
        f.check(len(e) == 2 and e[1]["s"] == t_read + math.ceil(SMPL[0] * adc_cycle_ns(1)),
                f"WAIT: after the read at {t_read}: {e}")
        s.w(CR, ADSTP)
        print("  WAIT: the next conversion starts at the DR read, then waits again")
        # DISCEN with SCANDIR: one channel per ADSTART, descending.
        s.push([(s.now, 0, 100_000), (s.now, 2, 200_000), (s.now, 5, 300_000)])
        enable(s, ckmode=1, cfgr1=1 << 16 | 1 << 2, ch=0b100101)
        order = []
        for _ in range(4):
            e = convert_sw(s, 30 * US)
            order += [(x["ch"], x["eoseq"]) for x in e]
            s.r(DR)
        f.check(order == [(5, False), (2, False), (0, True), (5, False)],
                f"DISCEN + SCANDIR: {order}")
        # Ascending, whole sequence per trigger, back to back.
        enable(s, ckmode=1, ch=0b100101)
        s.run_to((s.now // L_NS + 1) * L_NS)
        t0 = s.now
        e = convert_sw(s, 30 * US)
        f.check([x["ch"] for x in e] == [0, 2, 5] and [x["s"] for x in e] ==
                [instants(t0, 1, 0, 0, i)[0] for i in range(3)] and e[-1]["eoseq"]
                and not s.r(CR) & ADSTART,
                f"sequence: {[(x['ch'], x['s'] - t0) for x in e]}")
        note(f"disc {order}")
        print("  DISCEN: one channel per trigger; SCANDIR: descending; a sequence back to back")
        # Hardware trigger from TIM3 TRGO: rising, falling, both; triggers
        # during a conversion ignored; ADSTART kept in single mode.
        for exten, name in ((1, "rising"), (2, "falling"), (3, "both")):
            s.push([(s.now, 1, 1_000_000)])
            enable(s, ckmode=1, cfgr1=exten << 10 | 3 << 6, ch=1 << 1)
            s.w(ISR, 0xFF)
            s.run_to((s.now // L_NS + 1) * L_NS)
            k0 = s.conversions()
            acc0 = s.r(PORT + 0x20)
            ign0 = s.r(PORT + 0x24)
            s.w(CR, ADSTART)
            t0 = s.now
            timer_start(s, 3, 2)
            s.run(MS)
            s.w(TIM[3], 0)
            e = [s.ring(k) for k in range(k0, s.conversions())]
            trg = [t0 + k * 99 * US for k in range(1, 11)]
            acc = s.r(PORT + 0x20) - acc0
            ign = s.r(PORT + 0x24) - ign0
            want_T = trg
            f.check([x["T"] for x in e] == want_T and all(
                (x["s"], x["c"]) == instants(x["T"], 1, 0, 0) for x in e),
                f"EXTEN {name}: conversions at {[x['T'] - t0 for x in e]}")
            # With "both", the falling half of each blink arrives while the
            # conversion of its rising half runs: ignored.
            f.check((acc, ign) == ((10, 10) if exten == 3 else (10, 0)),
                    f"EXTEN {name}: {acc} triggers accepted, {ign} ignored")
            f.check(s.r(CR) & ADSTART, f"EXTEN {name}: ADSTART cleared in hardware-trigger single mode")
            s.w(CR, ADSTP)
            s.r(DR)
            note(f"exten {exten} {acc} {ign}")
        print("  EXTEN rising/falling/both from TIM3 TRGO: conversions at each update; "
              "triggers during a conversion ignored; ADSTART kept")
        # DMA: one request per EOC with DMAEN; DMA1 channel 1 (circular,
        # 32-bit, memory increment) copies ADC_DR to RAM, which clears EOC.
        s.push([(s.now, 1, 1_000_000), (s.now, 2, 2_000_000)])
        s.run(L_NS)
        enable(s, ckmode=1, cfgr1=1 | 2, ch=0b110)
        DMA_CH1 = 0x40020008
        s.w(DMA_CH1 + 0x8, DR)              # CPAR1
        s.w(DMA_CH1 + 0xC, 0x20001000)      # CMAR1
        s.w(DMA_CH1 + 0x4, 4)               # CNDTR1
        s.w(DMA_CH1, 1 | 1 << 5 | 1 << 7 | 2 << 8 | 2 << 10)  # EN CIRC MINC 32/32
        d0 = s.r(PORT + 0x28)
        e = convert_sw(s, 30 * US)
        mem = s.r(0x20001000, 2)
        f.check(s.r(PORT + 0x28) - d0 == 2 and mem == [x["dr"] for x in e] and not s.r(ISR) & OVR,
                f"DMA: {s.r(PORT + 0x28) - d0} requests, RAM {mem}, conversions {[x['dr'] for x in e]}")
        s.w(DMA_CH1, 0)
        # The port: a late push sets the error word and is ignored.
        enable(s, ckmode=1)
        convert_sw(s, 30 * US)
        last_s = s.ring(s.conversions() - 1)["s"]
        f.check(s.error() == 0, f"port error {s.error():#x} before the late push")
        s.push([(last_s - 1, 1, 123)])
        f.check(s.error() & 1 and s.r(PORT + 0x10) == 1, f"late push: error word {s.error():#x}")
        s.w(PORT + 4, 0)
        print("  DMA: one request per conversion; port: a late push sets the error word")
        note(f"end {s.now} {s.conversions()}")
    finally:
        s.close()
    return "\n".join(lines) + "\n"


# Firmware profile: (lattice ns, uV on channel 1); changes at k ms + 500 us,
# far from the conversions (T_k + 1063 ns .. T_k + 4750 ns, T_k = k * 999 us).
PROFILE = [
    (0, 1_500_000), (3 * MS + 500 * US, 600_000), (6 * MS + 500 * US, 1_650_000),
    (7 * MS + 500 * US, 0), (7 * MS + 800 * US, 1_650_000), (9 * MS + 500 * US, 2_500_000),
    (12 * MS + 500 * US, 2_001_500), (15 * MS + 500 * US, 999_999), (18 * MS + 500 * US, 1_000_000),
    (21 * MS + 500 * US, 3_400_000), (24 * MS + 500 * US, 0), (27 * MS + 500 * US, 1_500_000),
]
FW_END = 31 * MS
TIM3_PERIOD = 999 * US
AWD_LT, AWD_HT = 1241, 2482


def value_at(t):
    v = None
    for tc, uv in PROFILE:
        if tc < t:
            v = uv
    return v


def run_firmware(env, mode, keep=False):
    """mode "timed": pushes through the port at their lattice points, in one
    batch per 1 ms RUN_FOR; mode "setvalue": RUN_FOR split at each change,
    ADC SetValue while paused there."""
    s = env.session(FW / "adc_board.elf", lookahead=True)
    try:
        gpio = s.c.instance(CMD_GPIO, s.md, "gpioPortA")
        for pin in (4, 5, 6, 7):
            s.c.gpio_register_event(gpio, pin, pin)
        adc = s.c.instance(CMD_ADC, s.md, "bbAdc")
        lines, wakes = [], []
        changes = dict(PROFILE)
        if mode == "timed":
            points = list(range(0, FW_END + 1, MS))
        else:
            points = sorted(set(changes) | set(range(0, FW_END + 1, MS)))
        for t in points:
            s.run_to(t)
            if mode == "timed":
                # Every push of the coming 1 ms RUN_FOR, ahead of time.
                batch = [(tc, 1, uv) for tc, uv in PROFILE if t <= tc < t + MS]
                if batch:
                    s.push(batch)
                    wakes.append(t)
            elif t in changes:
                s.c.adc_set(adc, 1, changes[t])
            for desc, t_ns, state in s.c.events:
                lines.append(f"edge PA{desc} {t_ns} {int(state)}")
            s.c.events.clear()
        n = s.conversions()
        ring = [s.ring(k) for k in range(max(0, n - 64), n)]
        trg = [e for e in s.edges() if e[1] == 3 and e[2]]
        lines += [f"conv {x['T']} {x['s']} {x['c']} {x['code']} {int(x['awd'])}" for x in ring]
        lines.append(f"conversions {n} error {s.error()}")
        # WFI exits since Arm: one per ADC interrupt (EOC, with AWD) if
        # nothing else wakes the CPU, i.e. no push does.
        exits = s.r(LOOKAHEAD, 8)[5]
        return "\n".join(lines) + "\n", ring, trg, n, (len(wakes), exits)
    finally:
        s.close()


def group_firmware(env, f):
    print("firmware: adc_board.elf on the model (test plan 3)")
    out = {}
    for mode in ("timed", "setvalue"):
        out[mode] = run_firmware(env, mode)
    text, ring, trg, n, wakes = out["timed"]
    f.check(n == FW_END // TIM3_PERIOD, f"{n} conversions in 31 ms, expected {FW_END // TIM3_PERIOD}")
    f.check([x["T"] for x in ring[-len(trg):]] == [t for t, _, _ in trg][-len(ring):],
            "conversion trigger instants are not the TRGO instants")
    bad = [x for x in ring if x["s"] != x["T"] + 1063 or x["c"] != x["T"] + 4750]
    f.check(not bad, f"conversions not at T_k + 1063 / T_k + 4750 ns: {bad[:3]}")
    periods = {b["T"] - a["T"] for a, b in zip(ring, ring[1:])}
    f.check(periods == {TIM3_PERIOD}, f"TRGO periods {periods}, expected {TIM3_PERIOD} ns")
    # Codes and the ALARM edges from the profile.
    alarm, want_edges = 0, []
    for x in ring:
        uv = value_at(x["s"])
        c = code12(uv)
        f.check(x["code"] == c, f"conversion at {x['T']}: code {x['code']}, expected {c} ({uv} uV)")
        out_of = c < AWD_LT or c > AWD_HT
        f.check(x["awd"] == out_of, f"conversion at {x['T']}: AWD {x['awd']}, code {c}")
        if out_of and not alarm:
            want_edges.append((x["c"], 1))
            alarm = 1
        elif not out_of and alarm:
            want_edges.append((x["c"], 0))
            alarm = 0
    pa5 = [tuple(map(int, l.split()[2:])) for l in text.splitlines() if l.startswith("edge PA5 ")]
    # Renode stamps a GPIO write with the sync point that opens its slice:
    # c_k (the EOC limit woke the CPU there), or the next lattice point when
    # the ISR's write comes after it.
    f.check(len(pa5) == len(want_edges) and all(
        lv == wl and t in (c, -(-c // L_NS) * L_NS) for (t, lv), (c, wl) in zip(pa5, want_edges)),
        f"ALARM edges {pa5}, expected at {want_edges} (or the next lattice point)")
    batches, exits = wakes
    f.check(exits == n, f"{exits} WFI exits for {n} conversions ({batches} push batches)")
    f.check(text == out["setvalue"][0], "timed inputs and SetValue give different transcripts")
    print(f"  {n} conversions at T_k + 1063 / + 4750 ns (T_k: the TRGO instants, 999 us apart); "
          f"ALARM edges at c_k "
          f"of the out-of-window conversions {[t for t, _ in pa5]}; {batches} push batches and "
          f"{exits} WFI exits (one per conversion: no push woke the CPU); SetValue path identical")
    # The AWD interrupt after a SetValue crossing: at the EOC of the first
    # conversion sampled after the change to 0.6 V at 3.5 ms.
    sv = [tuple(map(int, l.split()[2:])) for l in out["setvalue"][0].splitlines() if l.startswith("edge PA5 ")]
    first = next(x for x in ring if x["s"] > 3 * MS + 500 * US)
    f.check(sv and sv[0] == (first["c"], 1),
            f"SetValue crossing at 3.5 ms: first ALARM edge {sv[:1]}, expected ({first['c']}, 1)")
    print(f"  SetValue crossing at 3.5 ms: AWD interrupt (ALARM high) at {first['c']} ns, the EOC of "
          f"the first conversion after it (T = {first['T']} ns)")
    return text


GROUPS = {"pwm": group_pwm, "trgo": group_trgo, "registers": group_registers, "firmware": group_firmware}


class Env:
    def __init__(self, renode, out, keep):
        self.renode, self.keep = renode, keep
        key = hashlib.sha256()
        for src in PLUGIN_SOURCES:
            key.update(src.encode() + b"\0" + (PLATFORMS / src).read_bytes() + b"\0")
        key.update(str(Path(renode).resolve()).encode())
        cache = out / "plugins"
        cache.mkdir(parents=True, exist_ok=True)
        self.dll = cache / f"bb_plugins-{key.hexdigest()[:16]}.dll"

    def session(self, elf, platform="bb", lookahead=False, repl_edit=None):
        return Session(self.renode, elf, self.dll, platform, lookahead, self.keep, repl_edit)


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--renode")
    ap.add_argument("--out", default=str(ROOT / "out/adc-model"))
    ap.add_argument("--keep", action="store_true")
    ap.add_argument("groups", nargs="*", default=list(GROUPS))
    a = ap.parse_args()
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    env = Env(find_renode(a.renode), out, a.keep)
    for e in ("adc_idle.elf", "tim3_pwm.elf", "adc_board.elf"):
        if not (FW / e).is_file():
            sys.exit(f"{FW / e} missing: make -C examples/adc firmware")
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
        print(f"bb-adc-test: FAIL ({len(f)} failures)")
        return 1
    print("bb-adc-test: PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
