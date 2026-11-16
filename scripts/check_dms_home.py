#!/usr/bin/env python3
"""Check an E3 run of examples/dial-me-spinner/e3 (Phase 1 plan section 2 E3, M10; ADR-008 Amendments 9-11).

Two kinds of run:

  --step <out> --board <board-step.cir>
      homing_step.elf on board-step.cir: H0, then one full step at 446 PPS (8 microsteps x 7 PWM periods),
      SETTLE, brake, sleep. Against the dial-me-spinner suite's test_firmware_microstep_pwm[VBAT3.7-25C]
      (examples/dial-me-spinner/suite/results.json at 6854d25), over the same window (the step's microstep 0
      to the end of its microstep 8, from the run's PWM edges): the PWM duty amplitude, the current-vector
      magnitude (its time average, from the coils' resistor drops), the thrust estimate, VSYS max, the VM
      ripple, the BAT pin minimum and the link current per contact. The run's step must start where the
      board's back-EMF does (to the fs), or the comparison is void.

  <out> --corner typ|worst [--start click|park]
      homing.elf on board.cir (typ, tS 50 ms), board-ts75.cir (worst, tS 75 ms) or board-park.cir (park:
      from inside the park zone, H1 runs): the phases H0-H5 of firmware.md section 5.4 in order with their
      step counts and the firmware's final microstep, HOME asserted after H4, the carriage at the park stop
      in H4 with at least 20 steps of overdrive, the final position 4 steps from the stop (+-2, the
      firmware.md error), the brake before sleep, and the HOME_N latency gate of ADR-008 Amendment 10: from
      the device's field crossing BOP (its `field above BOP` record, the first since HOME_N last rose) to the
      HOME_N fall, within 75 ms (typ) or the suite's 97.4 ms window (worst); for --start park also the
      release (BRP) latency of H1. The sample phase (seeded) and both latencies are printed.
      Since the M10 review (ADR-008 Amendment 12): the step rate of every phase, from the PWM edges (the
      run lengths of PWM periods per microstep must be firmware.md 5.3/5.4's schedule exactly: the device
      has no pull-out, so it would follow any rate); no park-stop contact before H4; H3's end 20-108 steps
      from the stop (the device's position at the BOP crossing less the microsteps played since); the
      overdrive counted from H4's microsteps after the contact; and the latency gate as a worst-phase
      bound: the device's start record must give the DRV5032FA's tS at the corner (50 or 75 ms), and
      tS + tACTIVE must be within the limit, besides the run's own (one seeded phase) latency.

Prints one `check_dms_home:` line per check and a JSON summary line; exit status 1 on a failure.
"""
import argparse
import csv
import json
import math
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
RESULTS = ROOT / "examples/dial-me-spinner/suite/results.json"
SUITE_TEST = "hardware/sim/tests/test_motor_drive.py::test_firmware_microstep_pwm[VBAT3.7-25C]"
FS = 1e-15
T_USTEP = 280_000_000_000            # 7 periods of 40 us
F_RATED = 0.78                       # suite F_RATED (>= 80 gf at 500 PPS)
F_NEED = 0.50                        # suite F_NEED (the 0.5 N load budget)
SHARE_WORST = (0.21 * 0.080 + 2 * 0.040) / (2 * 0.21 * 0.080 + 2 * 0.040 + 2 * 0.020)  # suite SHARE_WORST
R_LINK = 0.0484                      # the bench's link resistance (VSYS wires and contacts)
LATENCY_LIMIT = {"typ": 0.075, "worst": 0.075 + 10 / 446}   # ADR-008 Amendment 10 (97.42 ms)
TS = {"typ": 0.050, "worst": 0.075}

# Tolerances against the suite, set from M10's measurement (ADR-008 Amendment 11): the duty and the
# current vector differ because the firmware scales the duty with its own VSYS reading (3.645 V, read
# under the bench's 150 mA of other loads) where the suite assumed 3.7 V, and uses MD-3's 20.33 Ohm
# where the suite's case keeps the pre-MD-3 20.3 Ohm; the current per unit of duty (the drive chain:
# PWM edges, bridge, coil, back-EMF) is within 0.25 % of the suite's.
TOL = {"d_peak": 0.025, "ivec": 0.025, "ivec_per_d": 0.01, "thrust": 0.025, "vsys_max_v": 0.05}


class Checks:
    def __init__(self):
        self.fails = []
        self.summary = {}

    def __call__(self, ok, what):
        print(f"check_dms_home: {'ok  ' if ok else 'FAIL'} {what}")
        if not ok:
            self.fails.append(what)


def load(out):
    events = [json.loads(ln) for ln in (Path(out) / "events.jsonl").read_text().splitlines()]
    uart = [(e["t_fs"], e["msg"][len("uart0: "):]) for e in events
            if e["type"] == "log" and e["msg"].startswith("uart0: ")]
    dev = [(e["t_fs"], e["msg"]) for e in events if e["type"] == "log" and e["source"] == "devices"]
    edges = {}
    for e in events:
        if e["type"] == "pin_edge":
            edges.setdefault(e["net"], []).append((e["t_fs"], e["level"]))
    return events, uart, dev, edges


def suite_values():
    d = json.loads(RESULTS.read_text())
    return {c["check"]: c["value"] for c in d["checks"] if c["test"] == SUITE_TEST}


def near(got, want, tol):
    return abs(got - want) <= tol * abs(want)


def check_step(a, ck):
    events, uart, dev, edges = load(a.out)
    s = suite_values()
    lines = [m for _, m in uart]
    want = ["homing: H0 wake", "homing: H0 energise u=0", "homing: step test: one full step",
            "homing: SETTLE 20.16 ms at u=8", "homing: brake", "homing: step test done: u=8"]
    pos, ok = 0, True
    for w in want:
        k = next((i for i in range(pos, len(lines)) if lines[i].startswith(w)), None)
        ok = ok and k is not None
        pos = (k or pos) + 1
    ck(ok and not any("fail" in m for m in lines), f"the step test's phases in order ({len(lines)} UART lines)")
    vs = next((m for m in lines if m.startswith("homing: vsys=")), "")
    m = re.search(r"vsys=(\d+) uV .* r_eff=(\d+) uOhm d_run=(\d+)/640", vs)
    ck(m is not None, f"the firmware's reading and duty line: {vs!r}")
    vsys_uv, r_uohm, amp = (int(m.group(1)), int(m.group(2)), int(m.group(3))) if m else (1, 0, 0)
    want_amp = round(640 * 160000 * r_uohm / (vsys_uv * 1e6))
    ck(amp == min(640, want_amp), f"MD-3 duty d = 160 mA x {r_uohm / 1e6} Ohm / {vsys_uv / 1e6} V = "
       f"{amp}/640 (expected {want_amp})")
    d_peak = amp / 640
    runs, err = pwm_runs(edges)
    ck(runs is not None, f"the PWM edges on the PWM grid {err}".rstrip())
    check_schedule(runs, schedule(0, 0, step_test=True), ck,
                   "step rate from the PWM edges: the H0 hold 10.24 ms, one full step at 446.4 PPS (8 microsteps "
                   "of 7 periods, the last one with SETTLE 20.16 ms), the brake 3.12 ms")
    # The step: its microstep 1 begins at the first falling edge of MOT_B_IN2 after the hold (coil B is
    # braked at microstep 0 and gets its first drive part there); microstep 0 is the 280 us before.
    b2 = edges.get("U10.P0.11", [])
    t_ms1 = next((t for t, lv in b2 if lv == 0), None)
    ck(t_ms1 is not None, "the step's microstep 1 (MOT_B_IN2's first drive part)")
    t_s0 = (t_ms1 or 0) - T_USTEP
    t_end = t_s0 + 9 * T_USTEP
    emf = re.search(r"\(time > ([0-9.e-]+) && time < ([0-9.e-]+)\)", Path(a.board).read_text())
    emf_t0 = round(float(emf.group(1)) / FS) if emf else None
    ck(emf_t0 == t_s0, f"the step starts where the board's back-EMF does: t_s0 {t_s0} fs, board {emf_t0} fs")
    rows = list(csv.reader((Path(a.out) / "nets.csv").open()))
    ix = {n: i for i, n in enumerate(rows[0])}
    T, MAG, VS, VB, IL = [], [], [], [], []
    for r in rows[1:]:
        t = int(r[0])
        if t < t_s0 or t > t_end:
            continue
        ia = (float(r[ix["MOT_A_OUT1"]]) - float(r[ix["COILA_L"]])) / 20.0
        ib = (float(r[ix["MOT_B_OUT1"]]) - float(r[ix["COILB_L"]])) / 20.0
        T.append(t * FS)
        MAG.append(math.hypot(ia, ib))
        VS.append(float(r[ix["VSYS"]]))
        VB.append(float(r[ix["NB_VBAT"]]))
        IL.append((float(r[ix["NB_VSYS"]]) - float(r[ix["VSYS"]])) / R_LINK)
    ck(len(T) > 2000, f"{len(T)} samples in the step's window ({(t_end - t_s0) * FS * 1e3:.2f} ms)")
    ivec = sum((MAG[k] + MAG[k + 1]) / 2 * (T[k + 1] - T[k]) for k in range(len(T) - 1)) / (T[-1] - T[0])
    iref = F_RATED * s["µstep current-vector magnitude"] / (math.sqrt(2) * s["µstep thrust estimate (vs 2-phase-on rating at 446 PPS)"])
    f_micro = F_RATED * ivec / (math.sqrt(2) * iref)
    su_d, su_i = s["PWM duty amplitude d_peak"], s["µstep current-vector magnitude"]
    su_f = s["µstep thrust estimate (vs 2-phase-on rating at 446 PPS)"]
    rel = lambda x, y: f"{(x / y - 1) * 100:+.2f} %"  # noqa: E731
    ck(near(d_peak, su_d, TOL["d_peak"]), f"PWM duty amplitude d_peak {d_peak:.4f} vs the suite's {su_d:.4f} "
       f"({rel(d_peak, su_d)}, tolerance {TOL['d_peak'] * 100:.1f} %)")
    ck(near(ivec, su_i, TOL["ivec"]), f"current-vector magnitude {ivec * 1e3:.3f} mA vs {su_i * 1e3:.3f} mA "
       f"({rel(ivec, su_i)}, tolerance {TOL['ivec'] * 100:.1f} %)")
    ck(near(ivec / d_peak, su_i / su_d, TOL["ivec_per_d"]),
       f"current per unit duty {ivec / d_peak * 1e3:.3f} mA vs {su_i / su_d * 1e3:.3f} mA "
       f"({rel(ivec / d_peak, su_i / su_d)}, tolerance {TOL['ivec_per_d'] * 100:.1f} %)")
    ck(f_micro >= F_NEED and near(f_micro, su_f, TOL["thrust"]),
       f"thrust estimate {f_micro:.4f} N >= {F_NEED} N (the suite's limit) and vs {su_f:.4f} N "
       f"({rel(f_micro, su_f)}, tolerance {TOL['thrust'] * 100:.1f} %)")
    vmax, ripple = max(VS), max(VS) - min(VS)
    su_vmax = s["VM (VSYS) max, 25 kHz PWM"]
    ck(vmax <= 5.5 and abs(vmax - su_vmax) <= TOL["vsys_max_v"],
       f"VSYS max {vmax:.4f} V <= 5.5 V (BQ25180 abs max) and within {TOL['vsys_max_v'] * 1e3:.0f} mV of "
       f"the suite's {su_vmax:.4f} V")
    bmin, ilink = min(VB), max(IL) * SHARE_WORST
    ck(bmin >= 3.0, f"BAT pin min {bmin:.4f} V >= 3.0 V (VBUVLO; suite {s['BAT pin min, 25 kHz PWM']:.4f} V)")
    ck(ilink <= 1.0, f"link current per VSYS contact {ilink:.4f} A <= 1 A (suite "
       f"{s['link current per VSYS contact, 25 kHz PWM']:.4f} A)")
    print(f"check_dms_home: info VM ripple pk-pk {ripple * 1e3:.2f} mV (suite {s['VM ripple pk-pk, 25 kHz PWM'] * 1e3:.2f} mV,"
          f" info in both: the bench leaves out the link's 80 nH and samples at 1 us)")
    ck.summary = {"case": "step", "t_s0_fs": t_s0, "d_peak": d_peak, "ivec_a": ivec, "ivec_per_d": ivec / d_peak,
                  "thrust_n": f_micro, "vsys_max_v": vmax, "vm_ripple_v": ripple, "bat_min_v": bmin,
                  "link_a_per_contact": ilink, "suite": {"d_peak": su_d, "ivec_a": su_i, "thrust_n": su_f,
                                                          "vsys_max_v": su_vmax}}


PERIOD = 40_000_000_000                # one PWM period: COUNTERTOP 640 at 16 MHz (25 kHz)
TICK = 62_500_000                      # one PWM clock tick (16 MHz)
IN_PINS = ("U10.P0.26", "U10.P0.05", "U10.P0.07", "U10.P0.11")   # MOT_A_IN1, A_IN2, B_IN1, B_IN2
PERIODS = {"H1": 10, "H2a": 10, "H2": 7, "H3": 7, "H4": 39, "back": 10}  # REFRESH + 1 (firmware.md 5.3)
PPS = {k: 25_000 / (8 * v) for k, v in PERIODS.items()}  # 312.5, 446.4, 80.1 PPS
HOLD_H0, SETTLE, BRAKE = 8 * 32, 8 * 63, 7 * 11 + 1      # periods: 10.24 ms, 20.16 ms, 3.12 ms


def pwm_runs(edges):
    """The PWM0 schedule from the four IN pins' pin_edge records: (t_start_fs, periods, duties) for each
    run of PWM periods with the same high time on every channel (one microstep, or a hold), from the
    first edge (the first period's start: a COMP 0 channel rises there) to the last fall (STOP returns
    the pins to their GPIO level, low). Every edge must lie on the PWM grid (a period start, or a whole
    number of 16 MHz ticks after it, + 1 fs for the lattice rule), or the result is None."""
    chans = [edges.get(n, []) for n in IN_PINS]
    ts = [t for ch in chans for t, _ in ch]
    if not ts:
        return None, "no PWM edges"
    t0 = min(ts)
    t_stop = max(t for ch in chans for t, lv in ch if lv == 0)
    n = -(-(t_stop - t0) // PERIOD)
    high = [[0] * n for _ in chans]
    for c, ch in enumerate(chans):
        lv, since = 0, t0
        for t, l in ch + [(t_stop, 0)]:
            off = (t - t0) % PERIOD % TICK
            if t < t_stop and off > 1 and TICK - off > 1:
                return None, f"{IN_PINS[c]} edge at {t} fs is off the PWM grid"
            if lv == 1:
                a = since
                while a < t:
                    k = (a - t0) // PERIOD
                    b = min(t, t0 + (k + 1) * PERIOD)
                    high[c][k] += b - a
                    a = b
            lv, since = l, t
    runs = []
    for k in range(n):
        d = tuple(round(high[c][k] / TICK) for c in range(len(chans)))
        if runs and runs[-1][2] == d:
            runs[-1][1] += 1
        else:
            runs.append([t0 + k * PERIOD, 1, d])
    return runs, ""


def schedule(n1, n2, step_test=False):
    """firmware.md 5.4's schedule in PWM periods per run, and each run's phase: the H0 hold, H1 (313
    PPS), H2 (4 steps at 313 PPS, then 446), H3 (199 steps at 446), H4 (128 steps at 80), the back-off
    (4 steps at 313), SETTLE (at the back-off's last microstep, so one run with it), the brake."""
    if step_test:
        return [(HOLD_H0, "H0")] + [(7, "step")] * 7 + [(7 + SETTLE, "SETTLE")] + [(BRAKE, "brake")]
    s = [(HOLD_H0, "H0")] + [(PERIODS["H1"], "H1")] * (8 * n1)
    s += [(PERIODS["H2a"], "H2")] * (8 * min(4, n2)) + [(PERIODS["H2"], "H2")] * (8 * max(0, n2 - 4))
    s += [(PERIODS["H3"], "H3")] * (8 * 199) + [(PERIODS["H4"], "H4")] * (8 * 128)
    s += [(PERIODS["back"], "back")] * (8 * 4 - 1) + [(PERIODS["back"] + SETTLE, "SETTLE"), (BRAKE, "brake")]
    return s


def check_schedule(runs, want, ck, what):
    """The run lengths equal the schedule; returns {phase: (t_start, t_end)} of the measured runs."""
    got = [r[1] for r in runs] if runs else []
    bad = next((k for k in range(max(len(got), len(want)))
                if k >= len(got) or k >= len(want) or got[k] != want[k][0]), None)
    spans = {}
    for r, (_, ph) in zip(runs or [], want):
        a, b = spans.get(ph, (r[0], 0))
        spans[ph] = (a, r[0] + r[1] * PERIOD)
    if bad is None:
        ck(True, f"{what}: {len(got)} PWM runs exactly as firmware.md 5.3/5.4 schedules them")
    else:
        g = f"{got[bad]} periods ({25_000 / (8 * got[bad]):.1f} PPS) at {runs[bad][0] * FS:.6f} s"             if bad < len(got) else "no run"
        w = f"{want[bad][0]} periods ({want[bad][1]})" if bad < len(want) else "no run"
        ck(False, f"{what}: PWM run {bad} of {len(got)} (schedule {len(want)}) is {g}, the schedule has {w}")
    return spans, bad is None


def level_at(edges, t):
    lv = None
    for te, l in edges:
        if te > t:
            break
        lv = l
    return lv


def latency(dev, edges_home, t_from, t_to, up):
    """(t_cross, t_edge) of HOME_N's first fall (up: rise) in [t_from, t_to], and the first BOP (BRP)
    crossing record since HOME_N last changed before it."""
    want_level = 1 if up else 0
    t_edge = next((t for t, lv in edges_home if t_from <= t <= t_to and lv == want_level), None)
    if t_edge is None:
        return None, None
    prev = max((t for t, _ in edges_home if t < t_edge), default=0)
    tag = "field below BRP" if up else "field above BOP"
    t_cross = next((t for t, m in dev if prev <= t <= t_edge and m.startswith("M1: " + tag)), None)
    return t_cross, t_edge


def check_home(a, ck):
    events, uart, dev, edges = load(a.out)
    lines = [m for _, m in uart]
    at = {m: t for t, m in reversed(uart)}
    ck(not any("failed" in m or "FAULT" in m for m in lines), "no failed attempt, no FAULT")
    park = a.start == "park"
    seq = ["homing: vsys=", "homing: H0 wake: nSLEEP high (attempt 1)", "homing: H0 energise u=0",
           "homing: H1 back-off: HOME asserted" if park else "homing: H1 skipped: HOME released",
           *(["homing: H1 done: HOME released after"] if park else []),
           "homing: H2 done: HOME asserted after", "homing: H3 done", "homing: H5 confirm: HOME asserted",
           "homing: SETTLE", "homing: brake", "homing: IDLE"]
    found, pos = [], 0
    for w in seq:
        k = next((i for i in range(pos, len(lines)) if lines[i].startswith(w)), None)
        found.append(k)
        pos = (k if k is not None else pos) + 1
    ck(None not in found, f"H0-H5 in order: {' -> '.join(w.split(': ', 1)[1].split(' ')[0] for w in seq[1:])}")
    get = lambda pre: next((m for m in lines if m.startswith(pre)), "")  # noqa: E731
    n1 = int(re.search(r"after (\d+) steps", get("homing: H1 done")).group(1)) if park else 0
    m2 = re.search(r"after (\d+) steps", get("homing: H2 done"))
    n2 = int(m2.group(1)) if m2 else -1
    mi = re.search(r"u=(-?\d+)", get("homing: IDLE"))
    u_end = int(mi.group(1)) if mi else 0
    ck(0 < n2 <= 540 and (not park or 0 < n1 <= 377), f"step counts: H1 {n1} (limit 377), H2 {n2} (limit 540), "
       f"H3 199, H4 128, back-off 4")
    ck(u_end == 8 * (n1 - n2 - 199 - 128 + 4), f"the firmware's final microstep u={u_end} = 8 x (H1 {n1} - H2 {n2} "
       f"- H3 199 - H4 128 + 4)")
    home = edges.get("M1.HOME_N", [])
    t_h5 = at.get(get("homing: H5 confirm"))
    t_idle = at.get(get("homing: IDLE"))
    ck(t_h5 is not None and level_at(home, t_h5) == 0, "HOME asserted after H4 (M1.HOME_N low when H5 reads it)")
    # The step rate of every phase, from the PWM edges (the device has no pull-out or maximum speed, a
    # stated limit of dms/mechanism@1, so a wrong rate would move the rotor all the same: the check, not
    # the device, has to catch it; ADR-008 Amendment 12).
    runs, err = pwm_runs(edges)
    ck(runs is not None, f"the PWM edges on the PWM grid {err}".rstrip())
    spans, _ = check_schedule(runs, schedule(n1, max(n2, 0)), ck,
                              "step rate from the PWM edges: H0 hold 10.24 ms, " + ("H1 at 312.5 PPS, " if park else "")
                              + "H2 4 steps at 312.5 PPS then 446.4, H3 199 at 446.4, H4 128 at 80.1, back-off 4 at "
                              "312.5, SETTLE 20.16 ms, brake 3.12 ms")
    t_h4, t_h4_end = spans.get("H4", (None, None))
    stops = [t for t, m in dev if m == "M1: carriage at the park stop"]
    early = [t for t in stops if t_h4 is None or t < t_h4]
    ck(t_h4 is not None and not early, f"no park-stop contact before H4 (H4 from {(t_h4 or 0) * FS:.6f} s; first contact "
       f"at {(stops[0] if stops else 0) * FS:.6f} s)")
    # H3's end, 20-108 steps from the stop (firmware.md 5.4): the device's position at the BOP crossing that
    # ended H2, less the microsteps the PWM played between it and H4's first one (the rotor follows the field
    # while the carriage is off the stops).
    t_cross, t_edge = latency(dev, home, at.get(get("homing: H1 done" if park else "homing: H1 skipped")) or 0,
                              t_idle or 0, up=False)
    rec = next((m for t, m in dev if t == t_cross and m.startswith("M1: field above BOP")), "")
    ms = re.search(r"at s=(-?[0-9.]+)", rec)
    n_after = sum(1 for r in runs or [] if t_cross is not None and t_h4 is not None and t_cross < r[0] < t_h4)
    s_h3 = float(ms.group(1)) - n_after / 8 if ms and t_h4 is not None else float("nan")
    ck(20 <= s_h3 <= 108, f"H3 ends {s_h3:.3f} steps from the park stop (20-108, firmware.md 5.4: the BOP crossing "
       f"at s={ms.group(1) if ms else '?'} less {n_after} microsteps)")
    t_stop = next((t for t in stops if t_h4 is not None and t >= t_h4), None)
    over = sum(1 for r in runs or [] if t_stop and t_h4 <= r[0] < t_h4_end and r[0] >= t_stop) / 8
    slips = sum(1 for t, m in dev if m.startswith("M1: carriage left the park stop") and t_stop and t_stop < t < t_h4_end)
    ck(t_stop is not None and over >= 20, f"H4 reaches the park stop at {(t_stop or 0) * FS:.4f} s and overdrives "
       f"{over:.3f} steps (>= 20; H4's microsteps from the PWM edges after the contact); the rotor slips {slips} times")
    sl = [(t, m) for t, m in dev if m.startswith("M1: nSLEEP low at s=")]
    s_end = float(re.search(r"s=(-?[0-9.]+)", sl[-1][1]).group(1)) if sl else -1
    ck(2.0 <= s_end <= 6.0, f"final position s={s_end:.3f} steps from the park stop (x_park = stop - 4 steps, "
       f"+-2: firmware.md 5.4)")
    ins = [edges.get(n, []) for n in IN_PINS]
    ns = edges.get("U10.P1.15", [])
    t_sleep = next((t for t, lv in reversed(ns) if lv == 0), None)
    t_brake = max((max((t for t, lv in e if lv == 1), default=0) for e in ins), default=0)
    t_low = max((max((t for t, lv in e if lv == 0), default=0) for e in ins), default=0)
    ck(t_sleep is not None and all(level_at(e, t_sleep - 1) == 0 for e in ins) and t_low <= t_sleep
       and (t_low - t_brake) * FS >= 3e-3,
       f"brake {(t_low - t_brake) * FS * 1e3:.2f} ms (>= 3 ms, IN1 = IN2 = 1), then coast and nSLEEP low at "
       f"{(t_sleep or 0) * FS:.4f} s")
    # The HOME_N latency gate (ADR-008 Amendments 10 and 12): the device must be the DRV5032FA at the
    # corner (its start record's tS: 50 ms typical, 75 ms the Table 6.5 maximum), the worst sample phase's
    # latency tS + tACTIVE must be within the limit, and so must the run's own latency (one sample phase).
    start = next((m for _, m in dev if m.startswith("M1: DRV5032 tS")), "")
    mp = re.search(r"first window at (\d+) fs", start)
    phase_fs = int(mp.group(1)) if mp else None
    mt = re.search(r"tS ([0-9.]+) ms, tactive ([0-9.]+) us, BOP ([0-9.]+) mT, BRP ([0-9.]+) mT, die ([0-9.]+) mm", start)
    ts_dev, tact = (float(mt.group(1)) * 1e-3, float(mt.group(2)) * 1e-6) if mt else (float("nan"), float("nan"))
    lim = LATENCY_LIMIT[a.corner]
    lat = (t_edge - t_cross) * FS if t_cross and t_edge else None
    bound = ts_dev + tact
    ck(mt is not None and abs(ts_dev - TS[a.corner]) < 1e-9 and bound <= lim and lat is not None
       and 0 <= lat <= bound + 1e-12 and lat <= lim,
       f"HOME_N assert latency {lat * 1e3 if lat is not None else float('nan'):.3f} ms (field above BOP at "
       f"{(t_cross or 0) * FS:.6f} s, HOME_N low at {(t_edge or 0) * FS:.6f} s) <= {lim * 1e3:.1f} ms, and the "
       f"worst phase's tS + tACTIVE = {bound * 1e3:.2f} ms <= {lim * 1e3:.1f} ms; the device's tS "
       f"{ts_dev * 1e3:.0f} ms = the DRV5032FA's at the {a.corner} corner, {TS[a.corner] * 1e3:.0f} ms; "
       f"sample phase {phase_fs} fs; BOP {mt.group(3) if mt else '?'} mT, BRP {mt.group(4) if mt else '?'} mT, "
       f"die {mt.group(5) if mt else '?'} mm")
    rel_lat = None
    if park:
        t_c2, t_e2 = latency(dev, home, at.get(get("homing: H0 energise")) or 0,
                             at.get(get("homing: H2 done")) or 0, up=True)
        rel_lat = (t_e2 - t_c2) * FS if t_c2 and t_e2 else None
        ck(rel_lat is not None and 0 <= rel_lat <= min(bound + 1e-12, lim),
           f"HOME_N release latency {rel_lat * 1e3 if rel_lat is not None else float('nan'):.3f} ms in H1 (field "
           f"below BRP at {(t_c2 or 0) * FS:.6f} s) <= {lim * 1e3:.1f} ms")
    ck.summary = {"case": f"home-{a.corner}-{a.start}", "h1_steps": n1, "h2_steps": n2, "u_end": u_end,
                  "s_end": s_end, "s_h3_end": s_h3, "overdrive_steps": over, "ts_s": ts_dev, "slips": slips, "sample_phase_fs": phase_fs,
                  "assert_latency_s": lat, "release_latency_s": rel_lat, "limit_s": lim,
                  "idle_s": (t_idle or 0) * FS,
                  # exact fs, exported for the assertions (ADR-010 Amendment 3); the checks are unchanged
                  "assert_latency_fs": t_edge - t_cross if t_cross and t_edge else None,
                  "release_latency_fs": t_e2 - t_c2 if park and t_c2 and t_e2 else None,
                  "ts_fs": round(ts_dev * 1e15) if mt else None, "tactive_fs": round(tact * 1e15) if mt else None}


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("out")
    ap.add_argument("--step", action="store_true")
    ap.add_argument("--board")
    ap.add_argument("--corner", choices=("typ", "worst"), default="typ")
    ap.add_argument("--start", choices=("click", "park"), default="click")
    a = ap.parse_args()
    ck = Checks()
    if a.step:
        check_step(a, ck)
    else:
        check_home(a, ck)
    print("check_dms_home: summary " + json.dumps(ck.summary, sort_keys=True))
    if ck.fails:
        print(f"check_dms_home: FAIL ({len(ck.fails)} of the checks)")
        return 1
    print(f"check_dms_home: PASS ({a.out})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
