#!/usr/bin/env python3
"""Renode-only test of the W1 and W2 firmware's POWER and GPIO configuration (Phase 2 W-fw).

Runs each prebuilt image of examples/dial-me-spinner/firmware/prebuilt (w1_rule, w1_blank4,
w1_blank5, w1_blank_off, w2_rule, w2_old_rule) on Breadboard's nRF52840 (engines/cpu-renode/
platforms/nrf52840-bb.repl) through the External Control API, paused at every lattice point the
test acts on, as the Renode engine drives it (10 us lattice, serial execution). The pins are
observed as the engine observes them (External Control GPIO events), the UARTE0 console through
the tap BbUartLog (ADR-008 D8), and the POWER registers of Breadboard's POWER model BbNrf52Power
(ADR-009 D6, nrf52840-bb@4, Phase 2 P4) through power_journal.py, an observer that journals
every POWER access and changes none. Until P4 the test ran on a test double of those registers
(power_double.py, ADR-009 Amendment 7); P4 removed it, and the comparator input is now the
model's #pofwarn input (its GPIO input 0, set over External Control at lattice points as the
engine applies a digital_in), RESETREAS's preset its host register 0xF04 (plan section 4, W-fw
and P4 rows; ADR-009 Amendment 8).

Cases (every time relative to t_e, the LED_PWR_EN rise the test observes):

  W1, each blanking B (10, 4, 5 ms, off):
    config  no comparator input. Boot: P1.04, P0.06, P1.02 outputs driven low (PIN_CNF 0x3, P0.06
            H0H1 0x303, P1.00 the console's TX), POFCON = POF | V28 (0x1F) and INTENSET.POFWARN,
            all before t_e; with B > 0 the blanking starts within one lattice cell before t_e
            (POFCON.POF = 0, INTENCLR.POFWARN) and ends B to B + 10 us after t_e (POFCON 0x1F and
            INTENSET.POFWARN again); with B = 0 POFCON is never written after boot. P0.06 rises
            at least 10 ms after t_e and after the blanking; OLED_VCC_EN never rises; POFCON is
            only ever enabled at V28; no POFWARN; the console's boot line.
    window  the comparator input high from ceil_L(t_e + 3.255 ms) to ceil_L(t_e + 4.089 ms): the
            stress corner's measured 2.94 V window (ADR-009 Amendment 2), applied at lattice
            points as the engine applies a digital_in. 10 and 5 ms: the cue is held (no
            POFWARN, P0.06 rises). 4 ms (the window-midpoint control): the re-enable inside the
            window raises POFWARN ("already below at enable", D6) and the firmware sheds the
            cue within one lattice cell of the re-enable; off: POFWARN at the window's start
            sheds the cue within one lattice cell. A shed is P0.06 never high and LED_PWR_EN
            low, and the console line "w1: pofwarn n=1 cue=shed".
  W2, rule and old rule:
    config  RESETREAS preset 0 (what a brown-out leaves, D7): read once at boot, printed as 0;
            the same PIN_CNF; POFCON never written. Old rule: OLED_VCC_EN rises once at least
            1 ms after boot, LED_PWR_EN and P0.06 stay low. Rule: OLED_VCC_EN stays low,
            LED_PWR_EN rises, P0.06 at least 10 ms later.
    reason  RESETREAS preset 0x4 (SREQ): read, printed as 0x00000004 and cleared by writing the
            bits read (a read the test can see, so the brown-out case's 0 is not vacuous).

Every case runs twice; the two transcripts must be byte-identical.

Usage: bb_wfw_test.py [--renode PATH] [--fw DIR] [--out DIR] [--jobs N] [--keep] [IMAGE ...]
Exit status 0 on success.
"""

import argparse
import concurrent.futures
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
ROOT = HERE.parents[2]
sys.path.insert(0, str(ROOT / "examples/adc"))
sys.path.insert(0, str(ROOT / "examples/nrf-saadc"))
from renode_adc_test import CMD_GPIO, CMD_SYSTEM_BUS, WIDTH_DW, Client, find_renode  # noqa: E402
from bb_saadc_test import OVERLAY, Env, plugin_loader  # noqa: E402

MACHINE = "u1"
L = 10_000                      # the lattice (Renode global quantum), ns
US, MS = 1_000, 1_000_000
SYSBUS_WRITE = 1
GPIO_SET_STATE = 1
UARTLOG = 0x5FFF8000
POWER = 0x40000000
CTRL_RESETREAS = POWER + 0xF04  # BbNrf52Power's host register: the RESETREAS preset
POF_RAISES = POWER + 0xF08      # its count of EVENTS_POFWARN raises
POFCON_V28 = 0x1F               # POF | THRESHOLD 15 (V28) << 1 (nrf52840_bitfields.h)
INT_POFWARN = 0x4
WINDOW = (3255 * US, 4089 * US)  # the stress corner's 2.94 V window after LED_PWR_EN (Amendment 2)
RUN_NS = 30 * MS
# (port, pin) -> name, the pins W1 and W2 drive (firmware.md section 1.3)
PINS = {(1, 4): "LED_PWR_EN", (0, 6): "LED_DIN_MCU", (1, 2): "OLED_VCC_EN"}
# PIN_CNF after boot: outputs with the input buffer disconnected (DIR | INPUT), S0S1, and H0H1 on
# P0.06 (the board DTS's NRF_GPIO_DRIVE_H0H1); P1.00 the console's TX
PIN_CNF_WANT = {(1, 4): 0x3, (0, 6): 0x303, (1, 2): 0x3, (1, 0): 0x3}
W1 = {"w1_rule": 10, "w1_blank4": 4, "w1_blank5": 5, "w1_blank_off": 0}
W2 = {"w2_rule": False, "w2_old_rule": True}


def ceil_l(t):
    return -(-t // L) * L


class Session:
    """A paused Renode with Breadboard's nRF52840, the image, the POWER journal and the pins."""

    def __init__(self, renode, elf, dll, keep=False):
        self.dir = Path(tempfile.mkdtemp(prefix="bb-wfw-"))
        self.keep = keep
        self.journal = self.dir / "power.journal"
        self.journal.write_text("")
        with socket.socket() as s:
            s.bind(("127.0.0.1", 0))
            port = s.getsockname()[1]
        (self.dir / "platform.repl").write_text(OVERLAY.read_text())
        (self.dir / "journal.py").write_text(
            f"exec(open(r'{HERE / 'power_journal.py'}').read())\ninstall(r'{self.journal}')\n")
        get = "__import__('System').AppDomain.CurrentDomain.GetData('bb_power_journal')"
        lines = [f"include @{plugin_loader(self.dir, dll)}", f'mach create "{MACHINE}"',
                 f"machine LoadPlatformDescription @{self.dir / 'platform.repl'}",
                 f"sysbus LoadELF @{elf}",
                 f"include @{self.dir / 'journal.py'}",
                 f'sysbus SetHookBeforePeripheralWrite sysbus.clock "{get}.write(machine, offset, value)"',
                 f'sysbus SetHookAfterPeripheralRead sysbus.clock "value = {get}.read(machine, offset, value)"',
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
        self.desc = {}
        for (port_, pin), name in sorted(PINS.items()):
            gpio = self.c.instance(CMD_GPIO, md, f"gpio{port_}")
            d = len(self.desc) + 1
            self.desc[d] = name
            self.c.gpio_register_event(gpio, pin, d)
        self.power_port = self.c.instance(CMD_GPIO, md, "clock")
        self.now = 0

    def pofwarn(self, level):
        """The comparator input (BbNrf52Power's #pofwarn, its GPIO input 0), set while paused."""
        self.c.request(CMD_GPIO, struct.pack("<ibi", self.power_port, GPIO_SET_STATE, 0) + bytes([level]))

    def r(self, addr, n=1):
        v = self.c.read_u32s(self.bus, addr, n)
        return v if n > 1 else v[0]

    def w(self, addr, *vals):
        data = struct.pack(f"<{len(vals)}I", *vals)
        self.c.request(CMD_SYSTEM_BUS, struct.pack("<iBBQI", self.bus, SYSBUS_WRITE, WIDTH_DW,
                                                   addr, len(vals)) + data)

    def run_to(self, t):
        assert t >= self.now and t % L == 0, (t, self.now)
        if t > self.now:
            self.c.run_for(t - self.now)
            self.now = t

    def edges(self):
        """[(t_ns, name, level)] in emulation order."""
        return [(t, self.desc[d], lvl) for d, t, lvl in self.c.events]

    def run_until_edge(self, name, limit):
        while not any(n == name and lvl for _, n, lvl in self.edges()):
            if self.now >= limit:
                return None
            self.run_to(self.now + L)
        return next(t for t, n, lvl in self.edges() if n == name and lvl)

    def power(self):
        """The POWER journal: [(t_ns, kind, offset, value)]."""
        out = []
        for line in self.journal.read_text().splitlines():
            t, k, off, v = line.split()
            out.append((int(t), k, int(off, 16), int(v, 16)))
        return out

    def uart(self):
        n = self.r(UARTLOG + 4)
        raw = []
        for k in range(n):
            lo, hi = self.r(UARTLOG + 0x100 + 8 * (k % 1024), 2)
            raw.append((lo | (hi & 0xFFFF) << 32, hi >> 16 & 0xFF, hi >> 24))
        return raw

    def pin_cnf(self):
        """Every PIN_CNF word: {(port, pin): value}."""
        out = {}
        for port_, base, n in ((0, 0x50000000, 32), (1, 0x50000300, 16)):
            for pin, v in enumerate(self.r(base + 0x700, n)):
                out[(port_, pin)] = v
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


# --- one case ---------------------------------------------------------------

def run_case(renode, dll, fw, image, case, keep):
    """Run one case; return (transcript, [failure messages])."""
    s = Session(renode, fw / f"{image}.elf", dll, keep)
    fails, out = [], []

    def check(cond, msg):
        if not cond:
            fails.append(f"{image}/{case}: {msg}")
        return cond

    try:
        if case == "reason":
            s.w(CTRL_RESETREAS, 0x4)
        t_e = None
        if image in W1:
            t_e = s.run_until_edge("LED_PWR_EN", 5 * MS)
            if not check(t_e is not None, "LED_PWR_EN did not rise within 5 ms"):
                return "", fails
            if case == "window":
                t_on, t_off = ceil_l(t_e + WINDOW[0]), ceil_l(t_e + WINDOW[1])
                s.run_to(t_on)
                s.pofwarn(1)
                s.run_to(t_off)
                s.pofwarn(0)
        s.run_to(RUN_NS)
        edges, power, uart, cnf = s.edges(), s.power(), s.uart(), s.pin_cnf()
        raises = s.r(POF_RAISES)
    finally:
        s.close()

    text = bytes(b for _, _, b in uart).decode(errors="replace")
    lines = text.splitlines()
    out.append(f"case {image}/{case}")
    out += [f"edge {t} {n} {int(lvl)}" for t, n, lvl in edges]
    out += [f"power {t} {k} {off:#x} {v:#x}" for t, k, off, v in power]
    out += [f"uart {t} {line!r}" for t, line in zip(uart_line_times(uart), lines)]
    out += [f"pin_cnf P{p}.{n:02d} {v:#x}" for (p, n), v in sorted(cnf.items())]

    out.append(f"pofwarn raises {raises}")
    rises = {n: [t for t, m, lvl in edges if m == n and lvl] for n in PINS.values()}
    falls = {n: [t for t, m, lvl in edges if m == n and not lvl] for n in PINS.values()}
    writes = [(t, off, v) for t, k, off, v in power if k == "W"]
    # nrfx writes POFCON twice per call (nrf_power_pofcon_set, then nrf_power_pofcon_vddh_set's
    # read-modify-write of THRESHOLDVDDH, which stays at its reset value): one entry per call.
    # BbNrf52Power times a POFCON write after cpu.SyncTime() (ADR-009 D6), so the journal's clock
    # (the machine's local time) moves between the two writes of one call, by the instructions
    # between them (on the test double both had the slice's time): the second is the same value
    # less than one lattice cell after the first.
    pofcon = []
    for t, off, v in writes:
        if off == 0x510 and not (pofcon and pofcon[-1][1] == v and t - pofcon[-1][0] < L):
            pofcon.append((t, v))

    # GPIO configuration: the three enables and the console's TX, every other pin at reset.
    reset = {v for k, v in cnf.items() if k not in PIN_CNF_WANT}
    check(all(cnf[k] == v for k, v in PIN_CNF_WANT.items()),
          "PIN_CNF " + ", ".join(f"P{p}.{n:02d}={cnf[(p, n)]:#x} (want {v:#x})"
                                 for (p, n), v in PIN_CNF_WANT.items() if cnf[(p, n)] != v))
    check(reset == {0x2}, f"other PIN_CNF values {sorted(hex(v) for v in reset)}, expected only 0x2")
    # POFCON is only ever enabled at V28.
    check(all(v == POFCON_V28 for _, v in pofcon if v & 1), f"POFCON enabled other than V28: {pofcon}")

    if image in W1:
        blank = W1[image] * MS
        check(lines[:1] == [f"w1: boot pofcon=V28 blank={W1[image]} ms"], f"console {lines}")
        en = [t for t, v in pofcon if v == POFCON_V28 and t < t_e]
        ie = [t for t, off, v in writes if off == 0x304 and v & INT_POFWARN and t < t_e]
        check(len(en) == 1 and len(ie) == 1 and en[0] <= ie[0],
              f"boot: POFCON 0x1F then INTENSET.POFWARN once each before t_e = {t_e}: {pofcon}, {ie}")
        after = [(t, v) for t, v in pofcon if t >= en[0]] if en else []
        if blank:
            dis = [t for t, v in after if not v & 1]
            check(len(dis) == 1 and 0 <= t_e - dis[0] <= L,
                  f"blanking start: POFCON.POF = 0 within one lattice cell before t_e = {t_e}: {after}")
            clr = [t for t, off, v in writes if off == 0x308 and v & INT_POFWARN]
            check(len(clr) == 1 and clr[0] <= t_e, f"INTENCLR.POFWARN {clr}, t_e {t_e}")
            re = [t for t, v in after if v == POFCON_V28 and t > t_e]
            check(len(re) == 1 and blank <= re[0] - t_e <= blank + L,
                  f"blanking end: POFCON 0x1F {[t - t_e for t in re]} ns after t_e, want "
                  f"[{blank}, {blank + L}]")
            t_r = re[0] if re else None
            out.append(f"blanking {W1[image]} ms: POF off {t_e - dis[0] if dis else None} ns before "
                       f"LED_PWR_EN, on {t_r - t_e if t_r else None} ns after")
        else:
            check(after == [(en[0], POFCON_V28)] if en else False,
                  f"blanking off: POFCON written after boot: {after}")
            check(not [t for t, off, v in writes if off == 0x308 and v & INT_POFWARN],
                  "blanking off: INTENCLR.POFWARN written")
            t_r = None
        check(rises["OLED_VCC_EN"] == [], f"OLED_VCC_EN rose: {rises['OLED_VCC_EN']}")
        check(rises["LED_PWR_EN"] == [t_e], f"LED_PWR_EN rises {rises['LED_PWR_EN']}")
        shed_expected = case == "window" and W1[image] in (0, 4)
        reads = [t for t, k, off, v in power if k == "R" and off == 0x108 and v]
        if not shed_expected:
            din = rises["LED_DIN_MCU"]
            check(len(din) == 1 and din[0] - t_e >= 10 * MS and (t_r is None or din[0] > t_r),
                  f"P0.06 rises {[t - t_e for t in din]} ns after t_e, want one >= 10 ms and after "
                  "the blanking")
            check(falls["LED_PWR_EN"] == [], f"cue shed: LED_PWR_EN falls {falls['LED_PWR_EN']}")
            check(not reads and not any("pofwarn" in x for x in lines),
                  f"POFWARN handled (EVENTS_POFWARN reads {reads}, console {lines})")
            check(raises == 0, f"the model raised EVENTS_POFWARN {raises} times, want 0")
        else:
            t_on = ceil_l(t_e + WINDOW[0])
            ref = t_r if W1[image] else t_on
            f = falls["LED_PWR_EN"]
            check(len(f) == 1 and 0 <= f[0] - ref <= L,
                  f"cue not shed within one lattice cell of {'the re-enable' if W1[image] else 'the window start'} "
                  f"({ref}): LED_PWR_EN falls {f}")
            check(f[0] < ceil_l(t_e + WINDOW[1]) if f else False, "shed after the window's end")
            check(rises["LED_DIN_MCU"] == [], f"P0.06 rose after a shed: {rises['LED_DIN_MCU']}")
            check(lines[1:] == ["w1: pofwarn n=1 cue=shed"], f"console {lines}")
            check(raises == 1, f"the model raised EVENTS_POFWARN {raises} times, want 1")
            out.append(f"shed {f[0] - t_e if f else None} ns after LED_PWR_EN")
    else:
        old = W2[image]
        reas = 0x4 if case == "reason" else 0
        rr = [(t, v) for t, k, off, v in power if k == "R" and off == 0x400]
        check(len(rr) == 1 and rr[0][1] == reas, f"RESETREAS reads {rr}, want one read of {reas:#x}")
        wr = [v for t, off, v in writes if off == 0x400]
        check(wr == [reas], f"RESETREAS writes {wr}, want [{reas:#x}] (clear the bits read)")
        check(lines == [f"w2: boot resetreas={reas:#010x} rule={'old' if old else 'new'}"], f"console {lines}")
        check(pofcon == [], f"POFCON written: {pofcon}")
        check(raises == 0, f"the model raised EVENTS_POFWARN {raises} times, want 0")
        boot = rr[0][0] if rr else 0
        if old:
            o = rises["OLED_VCC_EN"]
            check(len(o) == 1 and o[0] - boot >= 1 * MS, f"OLED_VCC_EN rises {o}, boot {boot}")
            check(rises["LED_PWR_EN"] == [] and rises["LED_DIN_MCU"] == [],
                  f"old rule drove the cue: {rises}")
        else:
            check(rises["OLED_VCC_EN"] == [], f"rule enabled the OLED: {rises['OLED_VCC_EN']}")
            e = rises["LED_PWR_EN"]
            d = rises["LED_DIN_MCU"]
            check(len(e) == 1 and e[0] - boot >= 1 * MS and len(d) == 1 and d[0] - e[0] >= 10 * MS,
                  f"rule cue: LED_PWR_EN {e}, P0.06 {d}, boot {boot}")
        for n in PINS.values():
            check(falls[n] == [], f"{n} fell: {falls[n]}")
    return "\n".join(out) + "\n", fails


def uart_line_times(raw):
    """The time of each line's last byte (the newline)."""
    return [t for t, _, b in raw if b == 0x0A]


def cases_of(image):
    return ["config", "window"] if image in W1 else ["config", "reason"]


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--renode")
    ap.add_argument("--fw", default=str(HERE / "prebuilt"))
    ap.add_argument("--out", default=str(ROOT / "out/dms-wfw-test"))
    ap.add_argument("--jobs", type=int, default=int(os.environ.get("WFW_JOBS", "4")))
    ap.add_argument("--keep", action="store_true")
    ap.add_argument("images", nargs="*", default=list(W1) + list(W2))
    a = ap.parse_args()
    fw, out = Path(a.fw), Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    for im in a.images:
        if im not in W1 and im not in W2:
            sys.exit(f"unknown image {im}")
        if not (fw / f"{im}.elf").is_file():
            sys.exit(f"{fw / im}.elf missing")
    renode = find_renode(a.renode)
    env = Env(renode, out, a.keep)
    jobs = [(im, case, rep) for im in a.images for case in cases_of(im) for rep in (0, 1)]
    t0 = time.monotonic()
    results = {}
    # Each case's first run alone compiles the plugin assembly if the cache is cold.
    first = jobs[0]
    results[first] = run_case(renode, env.dll, fw, first[0], first[1], a.keep)
    with concurrent.futures.ThreadPoolExecutor(max_workers=max(1, a.jobs)) as ex:
        futs = {ex.submit(run_case, renode, env.dll, fw, im, case, a.keep): (im, case, rep)
                for im, case, rep in jobs[1:]}
        for f in concurrent.futures.as_completed(futs):
            results[futs[f]] = f.result()
    failures = []
    for im in a.images:
        for case in cases_of(im):
            (t0_, f0), (t1_, f1) = results[(im, case, 0)], results[(im, case, 1)]
            failures += f0
            if t0_ != t1_:
                failures.append(f"{im}/{case}: two runs differ")
            (out / f"{im}.{case}.txt").write_text(t0_)
            summary = [x for x in t0_.splitlines() if x.startswith(("blanking", "shed"))]
            print(f"{im}/{case}: {'FAIL' if f0 or t0_ != t1_ else 'ok'}"
                  + (f" ({'; '.join(summary)})" if summary else ""))
    for msg in failures:
        print(f"  FAIL: {msg}")
    print(f"wall time {time.monotonic() - t0:.1f} s; {len(jobs)} runs; transcripts in {out}")
    if failures:
        print(f"bb-wfw-test: FAIL ({len(failures)} failures)")
        return 1
    print("bb-wfw-test: PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
