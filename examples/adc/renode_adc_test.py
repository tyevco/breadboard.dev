#!/usr/bin/env python3
"""Renode-only ADC test for ADR-003 Part C (approver decision 4, Amendment 5).

Drives Renode 1.17.0's STM32F072 platform (`platforms/cpus/stm32f072.repl`,
ADC = `Analog.STM32F0_ADC`) running `firmware/build/adc.elf` through the
External Control API, the way the Renode engine drives the spike: paused
Renode, lattice-aligned `RUN_FOR`s (`L` = 10 us), and `ADC SetValue` sent
while Renode is paused at a lattice point (D6 "Delivery").

What it proves:

1. `ADC` instance lookup by name (`adc`), channel count 19, and `GetValue`
   reads back exactly the microvolts `SetValue` wrote.
2. `SetValue` changes the converted value: after every observation, the
   firmware's last code (TIM3 update ISR starts one conversion per ms) is
   the model's mapping of the value in effect at that conversion:
   code = round_half_even(floor(uV / 1000) / (3.3 * 1000) * 4095), *not*
   saturated at 4095 in ADC_DR. The profile pins the whole-millivolt
   truncation, the inclusive watchdog window and the missing saturation.
3. The analog watchdog (window [1241, 2482]) flags exactly the
   out-of-window conversions (the unclamped code is compared), and its ISR
   edges on PA5 land within 100 us after the conversion's TIM3 update.
   A value that is restored before the next conversion is never seen.
4. Determinism: two runs give byte-identical transcripts (status words at
   every observation plus every PA5 edge with its Renode time stamp).
5. Push equivalence (the step 9 reference): pushing `SetValue` with the
   current value at *every* lattice point, in one-cell `RUN_FOR`s, gives the
   same transcript as pushing only at the changes in long `RUN_FOR`s.
6. The hardware-trigger build (EXTSEL = TIM3_TRGO, TIM3 at 2 ms) converts at
   the repl's `externalEventFrequency` (1 kHz) instead: the model ignores
   EXTSEL. This is why the demo starts conversions from the TIM3 ISR.

Usage: renode_adc_test.py [--renode PATH] [--out DIR] [--keep]
Exit status 0 on success. Needs Renode 1.17.0 (`make setup`) and the
firmware (`make -C examples/adc firmware`).
"""

import argparse
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
PLATFORM = "platforms/cpus/stm32f072.repl"  # relative to the Renode install
MACHINE = "u1"
L_NS = 10_000  # lattice (Renode global quantum), ns
MS = 1_000_000

# --- External Control API (protocol 0), as engines/cpu-renode/src/protocol.rs
CMD_CHECK_VERSION, CMD_RUN_FOR, CMD_GET_TIME, CMD_GET_MACHINE = 0, 1, 2, 3
CMD_ADC, CMD_GPIO, CMD_SYSTEM_BUS = 4, 5, 6
T_REQUEST, T_EVENT, T_SUCCESS, T_INVALID, T_ERROR = 0, 1, 2, 3, 4
ADC_GET_COUNT, ADC_GET_VALUE, ADC_SET_VALUE = 0, 1, 2
GPIO_REGISTER_EVENT = 2
SYSBUS_READ, WIDTH_DW = 0, 4


class ProtocolError(Exception):
    pass


class Client:
    def __init__(self, sock):
        self.s = sock
        self.next_id = 0
        self.events = []  # (descriptor, time_ns, state), emulation order
        self.request(CMD_CHECK_VERSION, struct.pack("<I", 0))

    def _send(self, mid, payload):
        self.s.sendall(struct.pack("<HI", mid, len(payload)) + payload)

    def _recv_exact(self, n):
        b = b""
        while len(b) < n:
            c = self.s.recv(n - len(b))
            if not c:
                raise ProtocolError("connection closed")
            b += c
        return b

    def request(self, cmd, data):
        mid = 0x8000 | self.next_id
        self.next_id = (self.next_id + 1) & 0x7FFF
        self._send(mid, struct.pack("<HB", cmd, T_REQUEST) + data)
        while True:
            rid, size = struct.unpack("<HI", self._recv_exact(6))
            body = self._recv_exact(size)
            rcmd, rtype = struct.unpack("<HB", body[:3])
            rdata = body[3:]
            # Classify by type, never by id bit 15 (ADR-003 D1.3).
            if rtype in (T_REQUEST, T_EVENT):
                if rtype != T_EVENT or rcmd != CMD_GPIO or len(rdata) < 13:
                    self._send(rid, struct.pack("<HB", rcmd, T_INVALID))
                    raise ProtocolError(f"unexpected server request {rcmd}/{rtype}")
                desc, t_ns, state = struct.unpack("<iQB", rdata[:13])
                self.events.append((desc, t_ns, state != 0))
                self._send(rid, struct.pack("<HB", rcmd, T_SUCCESS))
                continue
            if rid != mid or rcmd != cmd:
                raise ProtocolError(f"response {rid:#x}/{rcmd} for request {mid:#x}/{cmd}")
            if rtype == T_SUCCESS:
                return rdata
            msg = rdata.decode("utf-8", "replace").rstrip("\0")
            raise ProtocolError(f"command {cmd} failed (type {rtype}): {msg}")

    def get_machine(self, name):
        r = self.request(CMD_GET_MACHINE, struct.pack("<I", len(name)) + name.encode())
        (md,) = struct.unpack("<i", r)
        if md < 0:
            raise ProtocolError(f"no machine {name}")
        return md

    def instance(self, cmd, md, name):
        r = self.request(cmd, struct.pack("<iiI", -1, md, len(name)) + name.encode())
        (i,) = struct.unpack("<i", r)
        return i

    def adc_count(self, adc):
        return struct.unpack("<i", self.request(CMD_ADC, struct.pack("<iB", adc, ADC_GET_COUNT)))[0]

    def adc_get(self, adc, ch):
        r = self.request(CMD_ADC, struct.pack("<iBi", adc, ADC_GET_VALUE, ch))
        return struct.unpack("<I", r)[0]

    def adc_set(self, adc, ch, microvolts):
        self.request(CMD_ADC, struct.pack("<iBiI", adc, ADC_SET_VALUE, ch, microvolts))

    def gpio_register_event(self, gpio, pin, desc):
        self.request(CMD_GPIO, struct.pack("<ibii", gpio, GPIO_REGISTER_EVENT, pin, desc))

    def read_u32s(self, bus, addr, count):
        r = self.request(CMD_SYSTEM_BUS, struct.pack("<iBBQI", bus, SYSBUS_READ, WIDTH_DW, addr, count))
        return list(struct.unpack(f"<{count}I", r))

    def run_for(self, ns):
        assert ns > 0
        self.request(CMD_RUN_FOR, struct.pack("<Q", ns))

    def get_time(self):
        return struct.unpack("<Q", self.request(CMD_GET_TIME, struct.pack("<Q", 0)))[0]


def find_renode(arg):
    for c in (arg, os.environ.get("RENODE"), shutil.which("renode"),
              str(Path.home() / ".local/bin/renode")):
        if c and Path(c).is_file():
            return c
    sys.exit("renode not found (make setup, or --renode / RENODE)")


class Renode:
    """A paused Renode with one machine, the ELF and an External Control server."""

    def __init__(self, renode, elf, keep=False):
        self.dir = Path(tempfile.mkdtemp(prefix="bb-adc-"))
        self.keep = keep
        with socket.socket() as s:
            s.bind(("127.0.0.1", 0))
            port = s.getsockname()[1]
        script = (
            f'mach create "{MACHINE}"\n'
            f"machine LoadPlatformDescription @{PLATFORM}\n"
            f"sysbus LoadELF @{elf}\n"
            "emulation SetSeed 0\n"
            f'emulation SetGlobalQuantum "{L_NS / 1e9:.8f}"\n'
            "emulation SetGlobalSerialExecution true\n"
            "emulation SetGlobalAdvanceImmediately true\n"
            "logLevel 2\n"
            f'emulation CreateExternalControlServer "bb" {port}\n'
        )
        (self.dir / "session.resc").write_text(script)
        self.log = self.dir / "renode.log"
        with open(self.log, "wb") as log:
            self.proc = subprocess.Popen(
                [renode, "--disable-xwt", "--plain", "--port", "-1", "session.resc"],
                cwd=self.dir, stdin=subprocess.DEVNULL, stdout=log, stderr=log)
        deadline = time.monotonic() + 120
        while True:
            if self.proc.poll() is not None:
                self.fail(f"renode exited during startup ({self.proc.returncode})")
            s = socket.socket()
            s.settimeout(0.5)
            try:
                s.connect(("127.0.0.1", port))
                if s.getsockname() == s.getpeername():  # TCP self-connect
                    raise OSError("self-connect")
                break
            except OSError:
                s.close()
                if time.monotonic() > deadline:
                    self.fail("External Control server did not come up")
                time.sleep(0.2)
        s.settimeout(120)
        s.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        self.c = Client(s)

    def tail(self):
        try:
            return "\n".join(self.log.read_text(errors="replace").splitlines()[-30:])
        except OSError:
            return ""

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


# --- the scenario -----------------------------------------------------------
VREF_MV = 3.3 * 1000
BITS = 12
AWD_LT, AWD_HT = 1241, 2482
STATUS_ADDR, STATUS_WORDS, MAGIC = 0x20000000, 10, 0x42424144
ADC_CHANNEL = 1
ALARM_PORT, ALARM_PIN, ALARM_DESC = "gpioPortA", 5, 1

# (lattice time ns, microvolts on channel 1). All changes are at k ms + 250
# or + 750 us: conversions happen within the first 100 us of each ms.
PROFILE = [
    (0, 1_500_000),               # 1.5 V: 1861, inside
    (3 * MS + 250_000, 600_000),  # 0.6 V: 745 < LT -> AWD, ALARM high
    (6 * MS + 250_000, 1_650_000),  # 2048, inside -> ALARM low
    (7 * MS + 250_000, 0),        # glitch, restored before the 8 ms conversion
    (7 * MS + 750_000, 1_650_000),
    (9 * MS + 250_000, 2_500_000),  # 3102 > HT -> AWD
    (12 * MS + 250_000, 2_000_999),  # 2000 mV (truncated) -> 2482 = HT, inside
    (15 * MS + 250_000, 999_999),  # 999 mV -> 1240 < LT -> AWD
    (18 * MS + 250_000, 1_000_000),  # 1241 = LT, inside
    (21 * MS + 250_000, 3_400_000),  # above vref: 4219, not saturated; AWD
    (24 * MS + 250_000, 0),       # 0 -> AWD
    (27 * MS + 250_000, 1_500_000),  # inside
]
END_OBS = 30  # observations at k ms + 500 us, k = 0 .. END_OBS
OBS = [k * MS + 500_000 for k in range(END_OBS + 1)]
# Renode 1.17.0's STM32_Timer sets its LimitTimer limit to ARR, so an update
# comes every ARR ticks, not ARR + 1 as on silicon (RM0091): the firmware's
# ARR = 999 at 1 MHz gives 999 us. Conversion k is at k * PERIOD + delta, with
# delta (CEN write plus ISR latency) the same for every k. Over 30 ms the
# conversions stay within 30 us before k ms, far from the changes at + 250 us.
TIM3_PERIOD_NS = 999_000
CONV_WINDOW_NS = 100_000


def model_code(uv):
    """Renode 1.17.0 STM32_ADC_Common: GetSampleFromChannel and
    MillivoltsToSample. `Sample.Voltage / 1000` is uint / int in C#, so whole
    millivolts; Math.Round is round-half-to-even (Python's round()); the
    arithmetic is IEEE double in both. ClampSample clamps to the width of
    the DATA field, which is 16 bits, so a 12-bit ADC_DR is *not* saturated
    at 4095 (real silicon saturates; the producer must clamp to [0, vref]).
    Returns (sample compared by the watchdog, ADC_DR)."""
    mv = float(uv // 1000)
    sample = int(round((mv / VREF_MV) * ((1 << BITS) - 1)))
    return sample, min(sample, (1 << 16) - 1)


def value_at(t):
    v = None
    for tc, uv in PROFILE:
        if tc <= t:
            v = uv
    return v


def expected():
    """Status at each observation, the ALARM edges by conversion index."""
    conv = awd = alarm = 0
    code = 0
    out, edges = [], []
    for k in range(END_OBS + 1):
        if k >= 1:  # conversion k: [k * PERIOD, k * PERIOD + CONV_WINDOW]
            sample, code = model_code(value_at(k * TIM3_PERIOD_NS + CONV_WINDOW_NS))
            conv += 1
            if sample < AWD_LT or sample > AWD_HT:
                awd += 1
                if not alarm:
                    edges.append((k, True))
                alarm = 1
            elif alarm:
                edges.append((k, False))
                alarm = 0
        out.append((conv, code, awd, alarm))
    return out, edges


def run_scenario(renode, elf, mode, keep):
    """mode "changes": RUN_FOR to the next change or observation, SetValue at
    the changes only. mode "every-cell": one-cell RUN_FORs and SetValue with
    the current value at every lattice point (the step 9 reference)."""
    r = Renode(renode, elf, keep)
    try:
        c = r.c
        md = c.get_machine(MACHINE)
        adc = c.instance(CMD_ADC, md, "adc")
        gpio = c.instance(CMD_GPIO, md, ALARM_PORT)
        bus = c.instance(CMD_SYSTEM_BUS, md, "sysbus")
        count = c.adc_count(adc)
        if count != 19:
            r.fail(f"ADC channel count {count}, expected 19 (STM32F0_ADC)")
        c.gpio_register_event(gpio, ALARM_PIN, ALARM_DESC)

        changes = dict(PROFILE)
        if mode == "changes":
            points = sorted(set(changes) | set(OBS))
        else:
            points = list(range(0, OBS[-1] + 1, L_NS))
        obs = set(OBS)
        lines = []
        now = 0
        for t in points:
            assert t % L_NS == 0
            if t > now:
                c.run_for(t - now)
                now = t
                for desc, t_ns, state in c.events:
                    lines.append(f"edge {t_ns} {int(state)}")
                c.events.clear()
            if mode == "every-cell" or t in changes:
                uv = value_at(t)
                c.adc_set(adc, ADC_CHANNEL, uv)
                back = c.adc_get(adc, ADC_CHANNEL)
                if back != uv:
                    r.fail(f"GetValue {back} uV after SetValue {uv} uV at {t} ns")
            if t in obs:
                st = c.read_u32s(bus, STATUS_ADDR, STATUS_WORDS)
                lines.append("obs {} {}".format(t, " ".join(str(w) for w in st)))
        t_end = c.get_time()
        if t_end != now:
            r.fail(f"Renode time {t_end} ns, expected {now} ns")
        return "\n".join(lines) + "\n"
    finally:
        r.close()


def check_semantics(transcript):
    exp, exp_edges = expected()
    problems = []
    obs = [l.split() for l in transcript.splitlines() if l.startswith("obs ")]
    edges = [tuple(map(int, l.split()[1:])) for l in transcript.splitlines() if l.startswith("edge ")]
    if len(obs) != len(exp):
        return [f"{len(obs)} observations, expected {len(exp)}"]
    for k, (o, e) in enumerate(zip(obs, exp)):
        w = list(map(int, o[2:]))
        magic, conv, code, awd, alarm, tim, lt, ht, period, hw = w
        if (magic, lt, ht, period, hw) != (MAGIC, AWD_LT, AWD_HT, 1000, 0):
            problems.append(f"obs {k}: bad header {w}")
        if (conv, code, awd, alarm) != e:
            problems.append(f"obs {k} at {o[1]} ns: (conversions, code, awd, alarm) = "
                            f"{(conv, code, awd, alarm)}, expected {e}")
        if tim != conv:
            problems.append(f"obs {k}: {tim} TIM3 updates but {conv} conversions")
    if len(edges) != len(exp_edges):
        problems.append(f"ALARM edges {edges}, expected at conversions {exp_edges}")
    else:
        deltas = set()
        for (t_ns, state), (k, s) in zip(edges, exp_edges):
            lo = k * TIM3_PERIOD_NS
            deltas.add(t_ns - lo)
            if bool(state) != s or not (lo <= t_ns <= lo + CONV_WINDOW_NS):
                problems.append(f"ALARM edge {state} at {t_ns} ns, expected {int(s)} in "
                                f"[{lo}, {lo + CONV_WINDOW_NS}] ns (conversion {k})")
        if len(deltas) > 1:
            problems.append(f"ALARM edges not at one fixed offset after the TIM3 updates: {deltas}")
        elif deltas:
            print(f"ALARM edges at conversion k * {TIM3_PERIOD_NS} ns + {deltas.pop()} ns")
    return problems


def run_hwtrig(renode, elf, keep):
    r = Renode(renode, elf, keep)
    try:
        c = r.c
        md = c.get_machine(MACHINE)
        adc = c.instance(CMD_ADC, md, "adc")
        bus = c.instance(CMD_SYSTEM_BUS, md, "sysbus")
        c.adc_set(adc, ADC_CHANNEL, 1_500_000)
        c.run_for(20 * MS + 500_000)
        return c.read_u32s(bus, STATUS_ADDR, STATUS_WORDS)
    finally:
        r.close()


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--renode")
    ap.add_argument("--out", default="out/adc")
    ap.add_argument("--keep", action="store_true", help="keep the Renode work dirs")
    a = ap.parse_args()
    renode = find_renode(a.renode)
    elf = HERE / "firmware/build/adc.elf"
    elf_hw = HERE / "firmware/build/adc_hwtrig.elf"
    for e in (elf, elf_hw):
        if not e.is_file():
            sys.exit(f"{e} missing: make -C examples/adc firmware")
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)

    failed = []
    t0 = time.monotonic()
    runs = {}
    for name, mode in (("changes-1", "changes"), ("changes-2", "changes"),
                       ("every-cell", "every-cell")):
        runs[name] = run_scenario(renode, elf, mode, a.keep)
        (out / f"{name}.txt").write_text(runs[name])
        digest = hashlib.sha256(runs[name].encode()).hexdigest()
        print(f"{name:11s} {digest}  ({len(runs[name].splitlines())} lines)")

    probs = check_semantics(runs["changes-1"])
    for p in probs:
        print("  " + p)
    if probs:
        failed.append("SetValue/conversion/watchdog semantics")
    if runs["changes-1"] != runs["changes-2"]:
        failed.append("determinism: changes-1 != changes-2")
    if runs["changes-1"] != runs["every-cell"]:
        failed.append("push equivalence: changes != every-cell")

    edges = [l for l in runs["changes-1"].splitlines() if l.startswith("edge ")]
    print(f"ALARM edges: {' '.join(l.split()[1] + ':' + l.split()[2] for l in edges)}")
    last = runs["changes-1"].splitlines()[-1].split()
    print(f"final status: conversions={last[3]} code={last[4]} awd={last[5]} alarm={last[6]}")

    hw = run_hwtrig(renode, elf_hw, a.keep)
    magic, conv, code, awd, alarm, tim, lt, ht, period, hwflag = hw
    print(f"hw-trigger build, 20.5 ms: TIM3 updates={tim} (ARR 1999 at 1 MHz), "
          f"conversions={conv}, code={code}")
    if (magic, hwflag, period) != (MAGIC, 1, 2000) or tim != 10 or conv != 20 or code != 1861:
        failed.append(f"hw-trigger build: status {hw}, expected 10 TIM3 updates, 20 "
                      "conversions (externalEventFrequency 1 kHz, EXTSEL ignored), code 1861")

    print(f"wall time {time.monotonic() - t0:.1f} s; transcripts in {out}")
    if failed:
        for f in failed:
            print("FAIL: " + f)
        return 1
    print("renode-adc: PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
