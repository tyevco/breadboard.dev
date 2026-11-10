#!/usr/bin/env python3
"""E2's checks (Phase 1 plan section 2 E2, M8; ADR-008 D7-D8, Amendment 5) on the runs of
scripts/dms_vsense.sh: the battery reading of dial-me-spinner docs/firmware.md section 12.4 through
Breadboard's SAADC with the loading adc_in on VSYS_SENSE, and the low-battery LED shed.

Usage: check_dms_vsense.py <out-root> --suite <results.json> [--json <file>] [--cases a,b,...]
                           [--suffix -ref]
<out-root>/<case><suffix> are the run directories (s1, s1-none, s50, s50-none, c5, c5-none, sag,
rec);
--cases limits the checks to those present in the list (make test runs s50, s50-none, c5, c5-none).

Every run (RUN):
  - no grant stopped early (`stop:`, e.g. stop:ngspice at a push): the input is sampled;
  - the firmware's readings: one UART line "vsense: n=<k> code=<c> vbat_est=<uV> uV leds=<..>" per
    8x burst, the bursts one period apart (1 s or 50 ms on RTC deadlines: within one 32 768 Hz tick),
    8 traced samples per burst 42 us apart (TACQ 40 us + tCONV 2 us; loaded runs: each sample at its
    #smp toggle + TACQ, one toggle per acquisition, alternating levels; load = "none": no toggle);
  - the codes, exactly: each sample's code is round_half_up(uV x 4096 / 3.6 V) and the burst's code is
    round_half_up(mean of its 8 codes) (ADR-008 Amendment 4), VBAT_est = code x 2.8125 mV x 0.979 as
    the firmware's integer arithmetic gives it.

The analog side (ANALOG; s1 against s1-none, s50 against s50-none, c5 against c5-none):
  - the error of every sample, V(hold) of the loaded run minus V(pad) of the load = "none" run at the
    same instant (the instants are the same: the firmware's timing does not depend on the codes at
    VSYS 3.0 V), against the charge-sharing arithmetic of the dial-me-spinner suite
    (hardware/sim/tests/test_vsys_sense.py, _sample: CS pre-charged to VPRE, shared with C5 + CPAD at
    each acquisition, C5 recovering through R1 || R2) at this bench's nominal parts and the run's own
    acquisition instants, with saadc_se's one simplification (CS stays on the pad between
    acquisitions, ADR-008 D7): within TOL_SAMPLE_UV per sample and TOL_MEAN_UV for the steady-state
    burst mean (the last burst; the suite's measure: |mean of the 8 errors|);
  - that arithmetic at the suite's own corner (C5 -10 % x -15 % X7R, R1/R2 +1 %, VSYS 3.0 V, VPRE =
    VDD max 3.366 V, the suite's burst count and switched CS) reproduces the suite's figures in
    results.json: 356.16 uV (1 s), 761.27 uV (50 ms), 3.4743 mV (C5 / 10, one burst), within
    TOL_SUITE_UV. So the co-simulation is tied to the suite's figures through one model that has to
    match both; the suite's corner itself needs tolerance views (Phase 2) and nominal parts give
    smaller figures (reported);
  - saadc_se's simplification measured (ADR-008 D7: "M8 measures the difference against the suite's
    switched model"): the same arithmetic with CS switched off the pad between acquisitions, at the
    run's instants and the bench's parts, against the connected one; the difference of the
    steady-state burst means and the worst per-sample difference are reported (not a limit).

Firmware-visible (FW):
  - the codes follow code = V / 2.8125 mV (VSYS) to within the rounding and the sampling error: every
    burst's code equals the code the model predicts from the unloaded pad and the modelled errors, and
    |code - VSYS / 2.8125 mV| < 1;
  - C5 / 10 shows in the codes: the c5 bursts' codes exceed the nominal ones (s1-none/c5-none read the
    unloaded pad) by the model's shift, at least 2 LSB;
  - the LED shed (sag): the LEDs come on at boot (LED_PWR_EN rise, P0.06 high at least 10 ms later),
    the first burst whose VBAT_est < 3.53 V (code <= 1282) sheds them (P0.06 low no later than
    LED_PWR_EN low, "leds off" logged, no later edge), every earlier burst has code >= 1283, and
    LED_PWR_EN falls within one burst period after the threshold crossing, the first instant at
    which V(VSYS_SENSE) plus the modelled mean sampling error falls below the code boundary 1282.5 LSB;
  - the restore (rec, ADR-008 Amendment 6): the LEDs come on at boot, the first reading with code <=
    1282 sheds them (E1's off order), every reading until the first one with VBAT_est >= 3.58 V is
    "leds=off", that reading restores them (LED_PWR_EN rises between its last sample and its line, no
    sooner than 30 ms after the shed, P0.06 high at least 10 ms after LED_PWR_EN: E1's on order), and
    they stay on: LED_PWR_EN and P0.06 edges exactly [1, 0, 1], one "leds off" and one "leds on" line.
"""
import argparse
import bisect
import csv
import json
import math
import sys
from fractions import Fraction

FS = 10**15
US = 10**9  # fs per us
TACQ_FS, TCONV_FS = 40 * US, 2 * US
LSB_UV = 3_600_000 / 4096           # 878.90625 uV at the pin
CS, CPAD = 2.5e-12, 3e-12           # saadc_se and the suite (PS v1.1)
R1, R2, C5 = 2.2e6, 1e6, 100e-9     # the board (VSYS_SENSE)
VSYS_BENCH, VDD_BENCH = 3.0, 3.366  # bench/vsense.toml
TICK_FS = FS // 32768 + 1           # one RTC tick, rounded up
SHED_CODE = 1282                    # VBAT_est < 3.53 V <=> code <= 1282 (firmware integer arithmetic)
RESTORE_UV = 3_580_000              # VBAT_est >= 3.58 V restores the LEDs (50 mV of hysteresis)
LED_OFF_MIN_FS = 30 * 10**12        # and not sooner than 30 ms after the shed
# Tolerances, set from M8's measurement (ADR-008 Amendment 5; the measured worst cases are printed).
# Measured (2026-11-10): per sample 0.53 uV (s1), 0.62 uV (s50), 5.51 uV (c5, 0.2 % of its 2.7 mV);
# steady-state burst mean 0.13 / 0.04 / 1.65 uV; the arithmetic at the suite's corner against
# results.json 0.009 / 0.015 / 0.85 uV. Each tolerance is about twice the worst measured value.
TOL_SAMPLE_UV, TOL_SAMPLE_REL = 1.0, 0.0025  # per sample: 1 uV + 0.25 % of the modelled error
TOL_MEAN_UV = 3                     # co-simulation vs the arithmetic, the steady-state burst mean
TOL_SUITE_UV = 2                    # the arithmetic at the suite's corner vs results.json
SUITE_CHECKS = {  # (test, check) of results.json -> the case
    "s1": ("test_sample_interval", "info: burst every 1 s, |8x average error|"),
    "s50": ("test_sample_interval", "info: burst every 0.05 s, |8x average error|"),
    "c5": ("test_fault_c5_decade_low_detected_at_fw_tacq", "fault C5 / 10, tACQ 40 µs: |8x average sampling error|"),
}
PERIOD_FS = {"s1": FS, "s1-none": FS, "c5": FS, "c5-none": FS, "sag": FS, "rec": FS, "s50": FS // 20, "s50-none": FS // 20}


def code_of(uv):
    return math.floor(Fraction(uv) * 4096 / 3_600_000 + Fraction(1, 2))


def mean_code(codes):
    return math.floor(Fraction(sum(codes), len(codes)) + Fraction(1, 2))


def vbat_est_uv(code):
    """The firmware's integer arithmetic (firmware/vsense/src/main.c, vbat_est_pv)."""
    pv = code * 878906250 * 32 // 10 * 979 // 1000
    return pv // 1_000_000


def load(d):
    recs = [json.loads(l) for l in open(f"{d}/events.jsonl")]
    samples, smp, lines, edges = [], [], [], []
    for r in recs:
        if r["type"] == "log" and r["msg"].startswith("saadc sample: P0.02"):
            uv = int(r["msg"].split(") ")[1].split(" uV")[0])
            samples.append((r["t_fs"], uv, int(r["msg"].rsplit("code ", 1)[1])))
        elif r["type"] == "log" and r["msg"].startswith("uart0: "):
            lines.append((r["t_fs"], r["msg"][len("uart0: "):]))
        elif r["type"] == "pin_edge" and r["net"] == "U10.P0.02#smp":
            smp.append(r)
        elif r["type"] == "pin_edge":
            edges.append(r)
    stops = [r for r in recs if r["type"] == "grant" and r.get("limited_by", "").startswith("stop:")]
    return dict(samples=samples, smp=smp, lines=lines, edges=edges, stops=stops, dir=d)


def readings(lines):
    out = []
    for t, m in lines:
        if m.startswith("vsense: n="):
            f = dict(kv.split("=") for kv in m.split()[1:] if "=" in kv)
            out.append(dict(t=t, n=int(f["n"]), code=int(f["code"]), est=int(f["vbat_est"]), leds=f["leds"]))
    return out


def model_errors(acq_s, meas_s, vth, rth, cext, vpre, connected=True):
    """Pad voltage minus vth at each meas instant (seconds): one charge transfer per acquisition start,
    relaxation to vth through rth. connected: CS stays on the pad between acquisitions (saadc_se);
    else it is switched off (the suite's NRF52840_SAADC_SE)."""
    cp = cext + CPAD
    tau_on = rth * (cp + CS)
    tau_off = tau_on if connected else rth * cp
    ev = sorted([(t, 0) for t in acq_s] + [(t, 1) for t in meas_s])
    v, t0, out, sampling = vth, 0.0, [], False
    for t, k in ev:
        tau = tau_on if sampling or connected else tau_off
        v = vth + (v - vth) * math.exp(-(t - t0) / tau)
        t0 = t
        if k == 0:
            v = (cp * v + CS * vpre) / (cp + CS)
            sampling = True
        else:
            out.append(v - vth)
            sampling = False
    return out


def suite_corner(interval, c5_scale_div=1.0, bursts=None):
    """test_vsys_sense.py's _sample at the suite's worst case: VSYS 3.0 V, VPRE = VDD max, C5 at
    (1 - 10 %) x (1 - 15 %), R1/R2 +1 %, 8x bursts (TACQ 40 us + 2 us) every `interval` from 20 us,
    max(2, ceil(6 tau / interval) + 1) bursts (or `bursts`); the mean error of the last burst."""
    r1, r2, c5 = R1 * 1.01, R2 * 1.01, C5 / c5_scale_div * 0.9 * 0.85
    rth = r1 * r2 / (r1 + r2)
    tau = rth * c5
    nb = bursts or max(2, math.ceil(6 * tau / interval) + 1)
    acq, meas = [], []
    for b in range(nb):
        for k in range(8):
            ts = 20e-6 + b * interval + k * 42e-6
            acq.append(ts)
            meas.append(ts + 40e-6 - 2e-9)
    e = model_errors(acq, meas, 3.0 * r2 / (r1 + r2), rth, c5, VDD_BENCH, connected=False)
    return sum(e[-8:]) / 8


class Checker:
    def __init__(self):
        self.fails, self.notes, self.results = [], [], {}

    def check(self, ok, what, msg):
        if not ok:
            self.fails.append(f"{what}: {msg}")
        return ok

    def note(self, what, msg):
        self.notes.append(f"{what}: {msg}")


def bursts_of(run, c, case):
    s = run["samples"]
    c.check(len(s) % 8 == 0 and s, case, f"{len(s)} samples, not whole 8x bursts")
    return [s[i:i + 8] for i in range(0, len(s) - len(s) % 8, 8)]


def check_run(c, case, run):
    loaded = not case.endswith("-none")
    c.check(not run["stops"], case, f"{len(run['stops'])} grants stopped early (stop:)")
    bursts = bursts_of(run, c, case)
    rd = readings(run["lines"])
    c.check(len(rd) == len(bursts), case, f"{len(rd)} readings for {len(bursts)} bursts")
    if loaded:
        smp = run["smp"]
        c.check(len(smp) == len(run["samples"]), case, f"{len(smp)} #smp toggles for {len(run['samples'])} samples")
        c.check([r["level"] for r in smp] == [(k + 1) % 2 for k in range(len(smp))], case,
                "#smp levels do not alternate from 1")
        # A toggle on a lattice point is reported 1 fs after it (ADR-008 Amendment 4).
        acq = [r["t_fs"] - r["t_fs"] % 1_000_000 for r in smp]
        c.check([s[0] for s in run["samples"]] == [a + TACQ_FS for a in acq][:len(run["samples"])], case,
                "samples not at their #smp toggle + TACQ")
    else:
        c.check(not run["smp"], case, f"{len(run['smp'])} #smp toggles with load = none")
    per = PERIOD_FS[case]
    for b, sb in enumerate(bursts):
        st = [x[0] for x in sb]
        c.check(all(st[k + 1] - st[k] == TACQ_FS + TCONV_FS for k in range(7)), case,
                f"burst {b}: samples not 42 us apart")
        for t, uv, code in sb:
            c.check(code == code_of(uv), case, f"sample at {t}: code {code}, expected {code_of(uv)} for {uv} uV")
        # The first reading runs at boot, the others on absolute RTC deadlines one period apart
        # (within one 32 768 Hz tick: 50 ms is 1638.4 ticks).
        if b == 1:
            gap = sb[0][0] - bursts[0][0][0]
            c.check(0 < gap <= per, case, f"burst 1: {gap} fs after the boot reading, period {per}")
        elif b > 1:
            gap = sb[0][0] - bursts[b - 1][0][0]
            c.check(abs(gap - per) <= TICK_FS, case, f"burst {b}: {gap} fs after the previous, period {per}")
        if b < len(rd):
            r = rd[b]
            want = mean_code([x[2] for x in sb])
            c.check(r["n"] == b and r["code"] == want, case, f"burst {b}: reading {r}, expected code {want}")
            c.check(r["est"] == vbat_est_uv(r["code"]), case,
                    f"burst {b}: vbat_est {r['est']} uV, the firmware's arithmetic gives {vbat_est_uv(r['code'])}")
            c.check(r["t"] > sb[-1][0], case, f"burst {b}: reading logged before its last sample")
    return bursts, rd


def check_analog(c, case, run, ref, suite):
    """A loaded case against its load = "none" reference."""
    bl, bn = bursts_of(run, c, case), bursts_of(ref, c, case + "-none")
    if not c.check(len(bl) == len(bn) and bl, case, f"{len(bl)} bursts, the reference {len(bn)}"):
        return None
    tl = [x[0] for b in bl for x in b]
    c.check(tl == [x[0] for b in bn for x in b], case, "sample instants differ from the load = none run")
    vth_uv = bn[0][0][1]
    c.check(all(x[1] == vth_uv for b in bn for x in b), case + "-none",
            f"the unloaded pad moves: {sorted({x[1] for b in bn for x in b})[:5]} uV")
    cext = C5 / 10 if case.startswith("c5") else C5
    acq = [(r["t_fs"] - r["t_fs"] % 1_000_000) / FS for r in run["smp"]]
    model = model_errors(acq, [t / FS for t in tl], vth_uv * 1e-6, R1 * R2 / (R1 + R2), cext, VDD_BENCH)
    errs = [x[1] - y[1] for b, n in zip(bl, bn) for x, y in zip(b, n)]
    worst = max(abs(e - m * 1e6) for e, m in zip(errs, model))
    for k, (e, m) in enumerate(zip(errs, model)):
        tol = TOL_SAMPLE_UV + TOL_SAMPLE_REL * abs(m * 1e6)
        c.check(abs(e - m * 1e6) <= tol, case,
                f"sample {k}: error {e} uV, the arithmetic {m * 1e6:.2f} uV (tol {tol:.2f} uV)")
    mean = sum(errs[-8:]) / 8
    mmean = sum(model[-8:]) / 8 * 1e6
    c.check(abs(mean - mmean) <= TOL_MEAN_UV, case,
            f"steady-state burst mean {mean:.2f} uV, the arithmetic {mmean:.2f} uV (tol {TOL_MEAN_UV})")
    # D7's simplification against the suite's switched CS, same instants and parts (reported).
    sw = model_errors(acq, [t / FS for t in tl], vth_uv * 1e-6, R1 * R2 / (R1 + R2), cext, VDD_BENCH,
                      connected=False)
    sw_mean = sum(sw[-8:]) / 8 * 1e6
    sw_worst = max(abs(m - x) for m, x in zip(model, sw)) * 1e6
    c.note(case, f"connected CS (saadc_se) minus switched CS (the suite): steady-state burst mean "
                 f"{mmean - sw_mean:+.3f} uV, worst sample {sw_worst:.3f} uV")
    out = dict(mean_uv=mean, model_uv=mmean, worst_sample_uv=worst, vth_uv=vth_uv,
               switched_model_uv=sw_mean, connected_minus_switched_uv=mmean - sw_mean,
               connected_vs_switched_worst_sample_uv=sw_worst,
               burst_means_uv=[sum(errs[8 * k:8 * k + 8]) / 8 for k in range(len(bl))])
    base = case.split("-")[0]
    if base in SUITE_CHECKS:
        t, name = SUITE_CHECKS[base]
        sv = next(x["value"] for x in suite["checks"] if x["test"].endswith("::" + t) and x["check"] == name) * 1e6
        sc = suite_corner(1.0 if base == "c5" else (1.0 if base == "s1" else 0.05),
                          10.0 if base == "c5" else 1.0, 1 if base == "c5" else None) * 1e6
        c.check(abs(sc - sv) <= TOL_SUITE_UV, case,
                f"the arithmetic at the suite's corner gives {sc:.2f} uV, results.json {sv:.2f} uV (tol {TOL_SUITE_UV})")
        out.update(suite_uv=sv, suite_corner_model_uv=sc)
        c.note(case, f"steady-state mean sampling error {mean:.1f} uV (co-simulation, V(hold) - V(pad) of load = "
                     f"none), {mmean:.1f} uV (the suite's arithmetic at this bench's nominal parts, max sample "
                     f"difference {worst:.2f} uV); the suite's {sv:.1f} uV at its corner (C5 min, R +1 %), "
                     f"which the same arithmetic gives as {sc:.2f} uV; half an LSB is {LSB_UV / 2:.1f} uV")
    return out


def check_fw_codes(c, case, run, ref, an):
    """Codes against V / 2.8125 mV: the predicted code from the unloaded pad plus the modelled errors."""
    bl, bn = bursts_of(run, c, case), bursts_of(ref, c, case + "-none")
    rd = readings(run["lines"])
    vth_uv = an["vth_uv"]
    acq = [(r["t_fs"] - r["t_fs"] % 1_000_000) / FS for r in run["smp"]]
    cext = C5 / 10 if case.startswith("c5") else C5
    model = model_errors(acq, [x[0] / FS for b in bl for x in b], vth_uv * 1e-6, R1 * R2 / (R1 + R2), cext, VDD_BENCH)
    worst = 0.0
    for b, r in enumerate(rd):
        pred = mean_code([code_of(round(vth_uv + model[8 * b + k] * 1e6)) for k in range(8)])
        c.check(r["code"] == pred, case, f"burst {b}: code {r['code']}, the model predicts {pred}")
        # code = V / 2.8125 mV within the model's stated error: the burst's mean sampling error, and
        # the rounding of each sample and of the mean (at most 1 LSB together).
        ideal = VSYS_BENCH * 1e6 / 2812.5
        err_lsb = sum(model[8 * b:8 * b + 8]) / 8 * 1e6 / LSB_UV
        worst = max(worst, abs(r["code"] - ideal - err_lsb))
        c.check(abs(r["code"] - ideal - err_lsb) <= 1, case,
                f"burst {b}: code {r['code']} vs VSYS / 2.8125 mV = {ideal:.3f} + the sampling error {err_lsb:.3f} LSB")
    c.note(case, f"codes {[r['code'] for r in rd]} (VSYS / 2.8125 mV = {VSYS_BENCH * 1e6 / 2812.5:.3f}; unloaded "
                 f"{code_of(vth_uv)}), each the model's prediction; at most {worst:.2f} LSB from V / 2.8125 mV plus "
                 f"the modelled sampling error")
    return worst


def nets_csv(d, col):
    ts, vs = [], []
    with open(f"{d}/nets.csv") as f:
        rd = csv.reader(f)
        head = next(rd)
        i = head.index(col)
        for row in rd:
            ts.append(int(row[0]))
            vs.append(float(row[i]))
    return ts, vs


def check_sag(c, run):
    case = "sag"
    rd = readings(run["lines"])
    edges = run["edges"]
    en = [r for r in edges if r["net"] == "U10.P1.04"]
    din = [r for r in edges if r["net"] == "U10.P0.06"]
    shed = next((k for k, r in enumerate(rd) if r["code"] <= SHED_CODE), None)
    if not c.check(shed is not None and shed > 0, case, f"no shed after the first reading: {[r['code'] for r in rd]}"):
        return {}
    c.check(all(r["code"] > SHED_CODE and r["leds"] == "on" for r in rd[:shed]), case,
            f"a reading before the shed is low or LEDs off: {rd[:shed]}")
    c.check(all(r["leds"] == "off" for r in rd[shed:]), case, "LEDs back on after the shed")
    c.check([r["level"] for r in en] == [1, 0], case, f"LED_PWR_EN edges {[(r['t_fs'], r['level']) for r in en]}")
    c.check([r["level"] for r in din] == [1, 0], case, f"P0.06 edges {[(r['t_fs'], r['level']) for r in din]}")
    if len(en) != 2 or len(din) != 2:
        return {}
    c.check(din[0]["t_fs"] - en[0]["t_fs"] >= 10 * 10**12, case, "P0.06 high less than 10 ms after LED_PWR_EN (FW-6.4)")
    c.check(din[1]["t_fs"] <= en[1]["t_fs"], case, "LED_PWR_EN fell before P0.06 (the off order, firmware.md 6.4)")
    off = [t for t, m in run["lines"] if m.startswith("vsense: leds off")]
    c.check(len(off) == 1, case, f"{len(off)} 'leds off' lines")
    bursts = bursts_of(run, c, case)
    t_fall = en[1]["t_fs"]
    c.check(bursts[shed][-1][0] < t_fall < rd[shed]["t"], case, "LED_PWR_EN fall not between the shed burst and its line")
    # The threshold crossing: V(VSYS_SENSE) + the modelled mean sampling error below the code boundary.
    ts, vs = nets_csv(run["dir"], "VSYS_SENSE")
    _, vsys = nets_csv(run["dir"], "VSYS")
    boundary = (SHED_CODE + 0.5) * LSB_UV * 1e-6
    k = bisect.bisect_left(ts, bursts[shed - 1][-1][0] + 2 * TCONV_FS)
    v_at = vs[k]
    acq = [(r["t_fs"] - r["t_fs"] % 1_000_000) / FS for r in run["smp"][8 * shed:8 * shed + 8]]
    acq = [a - acq[0] for a in acq]
    e = sum(model_errors(acq, [a + TACQ_FS / FS for a in acq], v_at, R1 * R2 / (R1 + R2), C5,
                         min(3.3, vsys[k] - 0.044167))) / 8
    t_x = next((t for t, v in zip(ts, vs) if v + e < boundary), None)
    if not c.check(t_x is not None, case, "no threshold crossing in the run"):
        return {}
    c.check(bursts[shed - 1][-1][0] < t_x < bursts[shed][0][0], case,
            f"the crossing {t_x} fs is not between the last burst before the shed and the shed burst")
    c.check(0 < t_fall - t_x <= PERIOD_FS[case], case,
            f"LED_PWR_EN fell {(t_fall - t_x) / 1e12:.3f} ms after the crossing (limit one period, 1000 ms)")
    t_an = next((t for t, v in zip(ts, vsys) if 0.979 * v < 3.53), None)
    c.note(case, f"LEDs on at boot (LED_PWR_EN {en[0]['t_fs'] / 1e12:.3f} ms, P0.06 {din[0]['t_fs'] / 1e12:.3f} ms); "
                 f"readings {[r['code'] for r in rd]}; shed at reading {shed} (code {rd[shed]['code']}, VBAT_est "
                 f"{rd[shed]['est']} uV): LED_PWR_EN low at {t_fall / 1e12:.3f} ms, "
                 f"{(t_fall - t_x) / 1e12:.3f} ms after the threshold crossing at {t_x / 1e12:.1f} ms "
                 f"(V(VSYS_SENSE) + {e * 1e6:.0f} uV < {boundary * 1e6:.0f} uV); 0.979 x V(VSYS) < 3.53 V from "
                 f"{t_an / 1e12 if t_an else float('nan'):.1f} ms")
    return dict(shed_reading=shed, t_fall_fs=t_fall, t_cross_fs=t_x, after_cross_ms=(t_fall - t_x) / 1e12,
                t_analog_cross_fs=t_an, codes=[r["code"] for r in rd])


def check_rec(c, run):
    """The shed and the restore (bench/sag-recover.toml)."""
    case = "rec"
    rd = readings(run["lines"])
    en = [r for r in run["edges"] if r["net"] == "U10.P1.04"]
    din = [r for r in run["edges"] if r["net"] == "U10.P0.06"]
    codes = [r["code"] for r in rd]
    shed = next((k for k, r in enumerate(rd) if r["code"] <= SHED_CODE), None)
    if not c.check(shed is not None and shed > 0, case, f"no shed after the first reading: {codes}"):
        return {}
    rest = next((k for k in range(shed + 1, len(rd)) if vbat_est_uv(rd[k]["code"]) >= RESTORE_UV), None)
    if not c.check(rest is not None, case, f"no reading at VBAT_est >= 3.58 V after the shed: {codes}"):
        return {}
    c.check(all(r["code"] > SHED_CODE and r["leds"] == "on" for r in rd[:shed]), case,
            f"a reading before the shed is low or LEDs off: {rd[:shed]}")
    c.check(all(r["leds"] == "off" for r in rd[shed:rest]), case,
            f"LEDs on between the shed and the restore: {rd[shed:rest]}")
    c.check(all(r["leds"] == "on" and r["code"] > SHED_CODE for r in rd[rest:]), case,
            f"LEDs not on (or shed again) from the restore: {rd[rest:]}")
    c.check([r["level"] for r in en] == [1, 0, 1], case, f"LED_PWR_EN edges {[(r['t_fs'], r['level']) for r in en]}")
    c.check([r["level"] for r in din] == [1, 0, 1], case, f"P0.06 edges {[(r['t_fs'], r['level']) for r in din]}")
    off = [t for t, m in run["lines"] if m.startswith("vsense: leds off")]
    on = [t for t, m in run["lines"] if m.startswith("vsense: leds on")]
    c.check(len(off) == 1 and len(on) == 1, case, f"{len(off)} 'leds off' and {len(on)} 'leds on' lines")
    if len(en) != 3 or len(din) != 3:
        return {}
    for i, what in ((0, "boot"), (2, "restore")):
        c.check(din[i]["t_fs"] - en[i]["t_fs"] >= 10 * 10**12, case,
                f"{what}: P0.06 high less than 10 ms after LED_PWR_EN (FW-6.4)")
    c.check(din[1]["t_fs"] <= en[1]["t_fs"], case, "LED_PWR_EN fell before P0.06 (the off order, firmware.md 6.4)")
    bursts = bursts_of(run, c, case)
    c.check(bursts[shed][-1][0] < en[1]["t_fs"] < rd[shed]["t"], case,
            "LED_PWR_EN fall not between the shed burst and its line")
    c.check(bursts[rest][-1][0] < en[2]["t_fs"] < rd[rest]["t"], case,
            "LED_PWR_EN rise not between the restore burst and its line")
    c.check(en[2]["t_fs"] - en[1]["t_fs"] >= LED_OFF_MIN_FS, case, "restored sooner than 30 ms after the shed")
    c.note(case, f"readings {codes}; shed at reading {shed} (VBAT_est {rd[shed]['est']} uV < 3.53 V, LED_PWR_EN low "
                 f"at {en[1]['t_fs'] / 1e12:.3f} ms); restored at reading {rest} (VBAT_est {rd[rest]['est']} uV >= "
                 f"3.58 V, LED_PWR_EN high at {en[2]['t_fs'] / 1e12:.3f} ms, "
                 f"{(en[2]['t_fs'] - en[1]['t_fs']) / 1e12:.3f} ms after the shed; P0.06 high "
                 f"{(din[2]['t_fs'] - en[2]['t_fs']) / 1e12:.3f} ms later)")
    return dict(shed_reading=shed, restore_reading=rest, codes=codes, t_shed_fs=en[1]["t_fs"],
                t_restore_fs=en[2]["t_fs"], p006_after_en_ms=(din[2]["t_fs"] - en[2]["t_fs"]) / 1e12)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("--suite", required=True)
    ap.add_argument("--json")
    ap.add_argument("--cases", default="s1,s1-none,s50,s50-none,c5,c5-none,sag,rec")
    ap.add_argument("--suffix", default="")
    a = ap.parse_args()
    cases = a.cases.split(",")
    suite = json.load(open(a.suite))
    c = Checker()
    runs = {k: load(f"{a.out}/{k}{a.suffix}") for k in cases}
    for k, r in runs.items():
        check_run(c, k, r)
    res = {}
    for base in ("s1", "s50", "c5"):
        if base in runs and base + "-none" in runs:
            an = check_analog(c, base, runs[base], runs[base + "-none"], suite)
            if an:
                an["code_worst_vs_ideal_lsb"] = check_fw_codes(c, base, runs[base], runs[base + "-none"], an)
                res[base] = an
    if "c5" in res:
        nominal = res["s1"] if "s1" in res else None
        c5c = [r["code"] for r in readings(runs["c5"]["lines"])]
        ref = [code_of(res["c5"]["vth_uv"])] * len(c5c)
        shift = [x - y for x, y in zip(c5c, ref)]
        c.check(all(s >= 2 for s in shift), "c5", f"C5 / 10 moves the codes by {shift} LSB, not >= 2")
        if nominal:
            s1c = [r["code"] for r in readings(runs["s1"]["lines"])][:len(c5c)]
            c.check(all(x > y for x, y in zip(c5c, s1c)), "c5", f"codes {c5c} not above the nominal {s1c}")
        c.note("c5", f"C5 / 10: codes {c5c} against {ref[0]} unloaded (+{min(shift)} LSB); mean error "
                     f"{res['c5']['mean_uv']:.0f} uV")
        res["c5"]["code_shift_lsb"] = shift
    if "sag" in runs:
        res["sag"] = check_sag(c, runs["sag"])
    if "rec" in runs:
        res["rec"] = check_rec(c, runs["rec"])
    for n in c.notes:
        print("check_dms_vsense: " + n)
    for f in c.fails:
        print("check_dms_vsense: FAIL: " + f)
    if a.json:
        with open(a.json, "w") as f:
            json.dump(dict(cases=cases, failed=c.fails, results=res), f, indent=1, sort_keys=True)
    if c.fails:
        sys.exit(1)
    print(f"check_dms_vsense: {', '.join(k + a.suffix for k in cases)}: PASS")


if __name__ == "__main__":
    main()
