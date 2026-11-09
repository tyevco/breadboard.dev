#!/usr/bin/env python3
"""Check an `examples/adc` run against the firmware's conversions, computed
from the circuit's own waveform.

    check_adc.py <out-dir> [--model bb|stock] [--period-us 999] [--drive-updates 50]

--model bb (the default; ADR-005 D7, Breadboard's ADC model,
examples/adc/firmware/build/adc_board.elf): TIM3 TRGO triggers conversion k
at T_k = T_1 + (k - 1) * 999 us (Renode's ARR-tick period; T_1 follows from
the start-up). With CKMODE = PCLK/2 (4 MHz), SMP = 000 and 12 bits the model
samples at s_k = T_k + 1063 ns and ends the conversion at c_k = T_k + 4750 ns
(ADR-005 D4). The value converted is ADC_IN at the latest lattice point
strictly before s_k (ADR-005 D5.2), as code12 = min(4095, round_half_up(uV *
4095 / 3.3 V)). The CPU sleeps until the EOC interrupt at c_k; its ISR sets
ALARM (PA5) on the analog watchdog (code outside [1241, 2482]) and releases
it on the next in-window code, copies bits 4 and 8 to PA6/PA7 and toggles
PA4 every 50th conversion. Renode stamps each write with the sync point
that opens its slice: c_k (the EOC limit), or the next lattice point (+1 fs)
when the write comes after it. T_1 follows from the first ALARM edge
(conversion 1 converts 0 V). With --require-no-stops (lookahead runs) no
grant may end with `stop:ngspice`: the ADC pin is a sampled input and is not
watched (ADR-005 D3).

--model stock (ADR-003 plan step 9, `bb run --adc-model stock`,
adc_board_isr.elf): the firmware converts ADC channel 1 once per TIM3
update. On Renode 1.17.0 TIM3 updates every
999 us (ARR ticks, ADR-003 Amendment 5 fact 5), so conversion k happens at
the TIM3 limit T_k = k * 999 us. Its ISRs then write, in the same slice:

- PA4 (DRIVE) toggles at every 50th update, before the conversion;
- the analog watchdog drives PA5 (ALARM) high on a code outside
  [1241, 2482], and an in-window code drives it low again;
- PA6 and PA7 take bits 4 and 8 of the code.

The ADC sees the value pushed at the last lattice point at or before the
conversion (ADR-003 D6: ngspice pushes the delivered whole millivolts at
the lattice points where they change, and Renode sets them while paused
there). nets.csv, sampled at the lattice (100 kHz for L = 10 us), holds
ADC_IN at every lattice point, in microvolts rounded like the delivered
value, so the expected code of every conversion follows from it with the
conversion of Renode's STM32_ADC_Common (Amendment 5 fact 1):
code = round_half_even(floor(uV / 1000) / 3300 * 4095), the delivered
value clamped to [0, 3.3 V].

The check: the edges of PA4..PA7 are exactly those the firmware model
predicts from the codes, conversion by conversion, at their Renode stamps
(measured, pinned here): the TIM3 ISR's PA4 write at T_k (T_k + 1 fs when
T_k is on the lattice: lattice +1 fs rule), the ADC ISR's writes at T_k plus
a constant offset, the synced state in which STM32_ADC_Common completes the
conversion (180 ns; taken from conversion 1, whose code 0 raises ALARM).

It also reports which grant ended at each push that moved ADC_IN across the
watchdog window (the analog-watchdog wakes): `stop:ngspice` is a quiet grant,
promised past the push, that the code-change watch cut there (ADR-003 D6).
--require-stops fails the check unless there is at least one.
"""
import argparse
import csv
import json
import sys

FS_PER_US = 1_000_000_000
VREF_UV = 3_300_000
BITS = 12
AWD_LT, AWD_HT = 1241, 2482
PINS = ["U1.PA4", "U1.PA5", "U1.PA6", "U1.PA7"]


def code_of(uv):
    uv = min(max(uv, 0), VREF_UV)
    mv = uv // 1000
    return round(mv / (VREF_UV / 1000) * ((1 << BITS) - 1))  # half to even


def parse_uv(s):
    neg = s.startswith("-")
    whole, _, frac = s.lstrip("-").partition(".")
    v = int(whole) * 1_000_000 + int(frac.ljust(6, "0")[:6])
    return -v if neg else v


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("--period-us", type=int, default=999)
    ap.add_argument("--drive-updates", type=int, default=50)
    ap.add_argument("--lattice-us", type=int, default=10)
    ap.add_argument(
        "--require-stops",
        action="store_true",
        help="fail unless some window crossing ended a quiet grant (stop:ngspice)",
    )
    ap.add_argument("--model", choices=["bb", "stock"], default="bb")
    ap.add_argument(
        "--require-no-stops",
        action="store_true",
        help="fail if a grant ended with stop:ngspice (bb model: nothing is watched)",
    )
    a = ap.parse_args()
    period = a.period_us * FS_PER_US
    lat = a.lattice_us * FS_PER_US

    with open(f"{a.out}/nets.csv") as f:
        rows = list(csv.DictReader(f))
    if "ADC_IN" not in rows[0]:
        sys.exit("check-adc: nets.csv has no ADC_IN column (probe net:ADC_IN)")
    adc = {int(r["t_fs"]): parse_uv(r["ADC_IN"]) for r in rows}
    t_end = max(adc)
    if any(t % lat for t in adc) or len(adc) != t_end // lat + 1:
        sys.exit("check-adc: nets.csv must be sampled at the lattice (--sample-rate 100kHz)")

    edges = []
    grant_end = {}
    with open(f"{a.out}/events.jsonl") as f:
        for line in f:
            e = json.loads(line)
            if e["type"] == "pin_edge" and e["net"] in PINS:
                edges.append((e["t_fs"], e["net"], e["level"]))
            elif e["type"] == "grant":
                grant_end[e["until_fs"]] = e["limited_by"]
    if not edges:
        sys.exit("check-adc: FAIL: no edges")
    if a.model == "bb":
        check_bb(a, adc, t_end, lat, period, edges, grant_end)
        return

    # The ISR offset: conversion 1 (code 0, ADC_IN at 0 V) raises ALARM.
    first = next(e for e in edges if e[1] == "U1.PA5")
    offset = first[0] - period
    if not 0 < offset < lat:
        sys.exit(f"check-adc: FAIL: first ALARM edge at {first[0]} fs is not conversion 1")

    def tim_stamp(k):
        # The TIM3 ISR runs in the slice the TIM3 limit opens.
        t = k * period
        return t + 1 if t % lat == 0 else t

    def adc_stamp(k):
        # The ADC ISR runs in the slice of the synced state the conversion
        # completes in (STM32_ADC_Common: ExecuteInNearestSyncedState).
        return k * period + offset

    # Model the firmware conversion by conversion.
    level = {p: 0 for p in PINS}
    alarm = 0
    expect = []
    k = 1
    ambiguous = 0
    crossings = []
    while k * period <= t_end - lat:
        t = k * period
        s = adc_stamp(k)
        if s > t_end:
            break
        out = []
        if k % a.drive_updates == 0:
            level["U1.PA4"] ^= 1
            expect.append((tim_stamp(k), "U1.PA4", level["U1.PA4"]))
        code = code_of(adc[t // lat * lat])
        if t % lat == 0 and code_of(adc[t - lat]) != code:
            ambiguous += 1
        if code < AWD_LT or code > AWD_HT:
            if not alarm:
                out.append(("U1.PA5", 1))
            alarm = 1
        elif alarm:
            out.append(("U1.PA5", 0))
            alarm = 0
        for pin, bit in (("U1.PA6", 4), ("U1.PA7", 8)):
            v = (code >> bit) & 1
            if v != level[pin]:
                level[pin] = v
                out.append((pin, v))
        for pin, v in out:
            expect.append((s, pin, v))
            if pin == "U1.PA5":
                # The push that moved ADC_IN across the window: the last
                # lattice point up to this conversion where the in-window
                # state changed.
                p = t // lat * lat
                inside = lambda q: AWD_LT <= code_of(adc[q]) <= AWD_HT  # noqa: E731
                while p >= lat and inside(p - lat) == inside(p):
                    p -= lat
                crossings.append(p)
        k += 1
    conversions = k - 1

    got = sorted(edges, key=lambda e: (e[0], PINS.index(e[1])))
    expect.sort(key=lambda e: (e[0], PINS.index(e[1])))
    # Edges after the last modelled conversion (its ISR inside the last
    # cell) are not modelled.
    horizon = adc_stamp(conversions)
    got = [e for e in got if e[0] <= horizon]
    if got != expect:
        for i, (g, x) in enumerate(zip(got, expect)):
            if g != x:
                print(f"check-adc: FAIL: edge {i}: got {g}, expected {x}")
                break
        else:
            print(f"check-adc: FAIL: {len(got)} edges, expected {len(expect)}")
        sys.exit(1)
    counts = {p: sum(1 for e in got if e[1] == p) for p in PINS}
    # Which grant ended at each window crossing (the analog-watchdog wakes):
    # `stop:ngspice` means a quiet grant, promised past the crossing, was
    # cut there by the code-change watch (ADR-003 D6).
    ends = {}
    for c in crossings:
        lim = grant_end.get(c, "inside a grant")
        ends[lim] = ends.get(lim, 0) + 1
    print(
        f"check-adc: PASS: {conversions} conversions (ISR offset {offset} fs), "
        f"{len(got)} edges as the firmware model predicts from ADC_IN "
        f"(PA4 {counts['U1.PA4']}, ALARM {counts['U1.PA5']}, bit 4 {counts['U1.PA6']}, "
        f"bit 8 {counts['U1.PA7']}); {ambiguous} on-lattice conversions where the "
        f"previous cell's value would differ"
    )
    print(
        "check-adc: the pushes that moved ADC_IN across the watchdog window end: "
        + ", ".join(f"{n} {k}" for k, n in sorted(ends.items()))
    )
    if a.require_stops and not ends.get("stop:ngspice"):
        sys.exit("check-adc: FAIL: no window crossing ended a quiet grant (stop:ngspice)")


def code12(uv):
    uv = min(max(uv, 0), VREF_UV)
    return min(4095, (2 * uv * 4095 + VREF_UV) // (2 * VREF_UV))


FS_PER_NS = 1_000_000
SAMPLE_NS = 1063  # ADR-005 D4: T + ceil(2.75 + 1.5 cycles at 4 MHz)
EOC_NS = 4750  # T + ceil(2.75 + 1.5 + 12.5 cycles + 4.5 PCLK at 8 MHz)
ISR_US = 3  # an ISR write lands within this of c_k


def check_bb(a, adc, t_end, lat, period, edges, grant_end):
    first = next(e for e in edges if e[1] == "U1.PA5")
    if first[2] != 1 or first[0] % FS_PER_NS:
        sys.exit(f"check-adc: FAIL: first ALARM edge {first} is not conversion 1's EOC")
    t1 = first[0] - EOC_NS * FS_PER_NS
    level = {p: 0 for p in PINS}
    alarm = 0
    expect = []  # (c_k, pin, level)
    k = 1
    on_lattice = 0
    crossings = 0
    while True:
        t = t1 + (k - 1) * period
        s = t + SAMPLE_NS * FS_PER_NS
        c = t + EOC_NS * FS_PER_NS
        if c + ISR_US * FS_PER_US + lat > t_end:
            break
        q = (s - 1) // lat * lat  # the latest lattice point strictly before s
        if s % lat == 0:
            on_lattice += 1
        code = code12(adc[q])
        out = []
        if code < AWD_LT or code > AWD_HT:
            if not alarm:
                out.append(("U1.PA5", 1))
                crossings += 1
            alarm = 1
        for pin, bit in (("U1.PA6", 4), ("U1.PA7", 8)):
            v = (code >> bit) & 1
            if v != level[pin]:
                level[pin] = v
                out.append((pin, v))
        if k % a.drive_updates == 0:
            level["U1.PA4"] ^= 1
            out.append(("U1.PA4", level["U1.PA4"]))
        if alarm and AWD_LT <= code <= AWD_HT:
            out.append(("U1.PA5", 0))
            alarm = 0
            crossings += 1
        expect += [(c, pin, v) for pin, v in out]
        k += 1
    conversions = k - 1
    # Compare conversion by conversion: every edge of an ISR belongs to the
    # conversion whose EOC c_k is at most ISR_US before it.
    c1 = t1 + EOC_NS * FS_PER_NS
    horizon = c1 + (conversions - 1) * period + ISR_US * FS_PER_US
    got = [e for e in edges if e[0] <= horizon]
    by_conv = {}
    for t, pin, v in got:
        k = (t - c1) // period
        if t < c1 or t - (c1 + k * period) > ISR_US * FS_PER_US:
            sys.exit(f"check-adc: FAIL: edge {(t, pin, v)} is not within {ISR_US} us after an EOC")
        by_conv.setdefault(k, []).append((t, pin, v))
    want = {}
    for c, pin, v in expect:
        want.setdefault((c - c1) // period, []).append((pin, v))
    late = 0
    for k in sorted(set(by_conv) | set(want)):
        c = c1 + k * period
        nxt = -(-c // lat) * lat + 1
        g = by_conv.get(k, [])
        bad = [e for e in g if e[0] != c and e[0] != nxt]
        if sorted((p, v) for _, p, v in g) != sorted(want.get(k, [])) or bad:
            sys.exit(f"check-adc: FAIL: conversion {k + 1} (c_k = {c} fs): edges {g}, expected "
                     f"{want.get(k, [])} at c_k (or {nxt} fs)")
        late += sum(1 for e in g if e[0] != c)
    stops = sum(1 for v in grant_end.values() if v == "stop:ngspice")
    counts = {p: sum(1 for e in got if e[1] == p) for p in PINS}
    print(
        f"check-adc: PASS: {conversions} conversions (model bb: T_1 = {t1} fs, s_k = T_k + "
        f"{SAMPLE_NS} ns, c_k = T_k + {EOC_NS} ns), {len(got)} edges as the firmware model "
        f"predicts from ADC_IN (PA4 {counts['U1.PA4']}, ALARM {counts['U1.PA5']}, bit 4 "
        f"{counts['U1.PA6']}, bit 8 {counts['U1.PA7']}; {late} at the next lattice point); "
        f"{on_lattice} sample instants on a lattice point; {crossings} watchdog window "
        f"crossings, each reported at the EOC of the first conversion after it"
    )
    print(f"check-adc: {stops} grants ended with stop:ngspice")
    if a.require_no_stops and stops:
        sys.exit("check-adc: FAIL: a quiet grant stopped at an ADC push (stop:ngspice), but the "
                 "pin is a sampled input")


if __name__ == "__main__":
    main()
