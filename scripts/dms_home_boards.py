#!/usr/bin/env python3
"""Write (or check) the E3 board files of examples/dial-me-spinner/e3 (Phase 1 M10; ADR-006 Amendment 6,
ADR-008 Amendments 9 and 10).

E3 runs on hand-written board files (route A, approver decision 1, 2026-11-14): the detent sheet of the
dial-me-spinner main board at 6854d25 ("Detent actuator & home sensor": U40/U41 DRV8837, R70, C70, C72,
C74, C71, C73, C75, J40's coils, U42) with the nRF52840's pins, the mechanism and DRV5032 device M1
(dms/detent-mechanism) and, as the bench, the dial-me-spinner suite's own power path of
test_firmware_microstep_pwm[VBAT3.7-25C]. The cases differ only in M1's parameters and in the coils'
back-EMF, so one template here writes them all; `make dms-home-test` checks that the committed files are
this script's output (`--check`).

The DRV8837 bridge is the package view ti/drv8837:bridge, copied verbatim from
packages/ti/drv8837/analog/bridge.lib (a board file's SPICE body cannot name a package view; the copy is
checked byte for byte with the package's file, so it cannot drift).

    dms_home_boards.py [--check]
"""
import argparse
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "examples/dial-me-spinner/e3"
VIEW = ROOT / "packages/ti/drv8837/analog/bridge.lib"

# The step case's back-EMF: the rotor turns at 446 PPS from the start of the step's microstep 0 (the
# last 280 us of the H0 hold, as the suite's t_s0) to the end of microstep 8, with the suite's
# worst-case constants (KE_MAX 1.37 V per k step/s, load angle pi/2). T_S0 is the instant
# homing_step.elf starts that microstep (fs); scripts/check_dms_home.py requires the run's PWM edges to
# put it there exactly, so a firmware change that moves it fails the check instead of misaligning the
# EMF.
T_S0_FS = 11_972_875_000_000
T_USTEP_FS = 280_000_000_000          # 7 PWM periods of 40 us
STEP_EMF = dict(ke=1.37, pps=446, delta=1.5707963267949, t0=T_S0_FS, tstop=T_S0_FS + 9 * T_USTEP_FS)

# The worst corner (ADR-008 Amendments 10 and 12): the dial-me-spinner suite's HALL["FA"] corner of
# test_home_n_levels_and_latency, tS 75 ms (Table 6.5 max) with BOP 4.8 mT max and BRP 0.5 mT min
# (Table 6.6: "the corners that assert last / release last"), at its worst sample phase: the DRV5032's
# sampling window that ends 10 us before the device's field first passes BOP in H2 (at
# 552 221 562 500 000 fs in homing.elf's run at BOP 4.8 mT; the window starts tACTIVE = 40 us earlier, at
# phase + 7 tS), so HOME_N falls one full tS later, as late as the switch allows. scripts/check_dms_home.py
# prints the latency; a firmware change that moves the crossing moves it and the gate still bounds it.
WORST_PHASE_PS = 27_171_562_500
FA_LAST = "bop=4.8 brp=0.5"

CASES = {
    # name: (title, M1 parameters, back-EMF of the full analog sheet, or None: the homing boards)
    "board.cir": ("homing from the click end, DRV5032FA typical corner (tS 50 ms)", "s0=500 ts=50ms", None),
    "board-ts75.cir": ("homing from the click end, DRV5032FA worst corner (tS 75 ms, Table 6.5 max; BOP 4.8 mT "
                       "max, BRP 0.5 mT min, Table 6.6: the suite's HALL['FA']) at its worst sample phase",
                       f"s0=500 ts=75ms {FA_LAST} phase={WORST_PHASE_PS}ps", None),
    "board-park.cir": ("homing from inside the park zone (s0 = 150 steps: H1 runs), typical tS 50 ms, BOP/BRP "
                       "4.8/0.5 mT (the corner that asserts and releases last: H1 longest)",
                       f"s0=150 ts=50ms {FA_LAST}", None),
    "board-step.cir": ("one full step at 446 PPS (homing_step.elf), mid-stroke, with the bridges, the coils "
                       "and the suite's back-EMF", "s0=400 ts=50ms", STEP_EMF),
}

HEADER = """\
* examples/dial-me-spinner/e3/{name}: E3, {title}.
* Written by scripts/dms_home_boards.py (do not edit; `make dms-home-test` checks it). Phase 1 M10,
* plan section 2 E3; ADR-006 Amendment 6 (device parts in board files), ADR-008 Amendments 9 and 10.
*
* The detent sheet of the dial-me-spinner main board at 6854d25 ("Detent actuator & home sensor",
* examples/dial-me-spinner/kicad/main/dial-me-spinner-main.net): U40 and U41 (DRV8837, coil A and coil
* B), the shared nSLEEP with R70 100 kOhm, the VM capacitors C70, C72 (100 nF) and C74 (10 uF), J40's
* two coils, U42's supply capacitors (C71, C73, C75 on +3V3) and the nRF52840's pins (what of the
* sheet's analog side this file carries: below):
*   P0.26 MOT_A_IN1, P0.05 MOT_A_IN2, P0.07 MOT_B_IN1, P0.11 MOT_B_IN2  PWM0 channels 0-3 (digital_out,
*         the suite's S0S1 bound: 800 Ohm; the PWM model's edges at their exact instants, ADR-008 D9)
*   P1.15 MOT_NSLEEP (digital_out), P1.14 HOME_N (digital_in, reads M1.HOME_N directly)
*   P0.02 VSYS_SENSE (adc_in, load=none: E2's SAADC channel, below; R1 2.2 MOhm, R2 1 MOhm, C5 100 nF)
*   VDD   +3V3 (adc_in: the SAADC's VDD input and VPRE of the pin_load view)
* U10 is the Raytac module's chip, bound directly (nordic/nrf52840-qiaa, as examples/nrf-saadc): a module
* pad has no VDD adc_in in a board file.
*
* M1 (dms/detent-mechanism, device dms/mechanism@1) is the lead screw, carriage, magnet and U42
* DRV5032FA as one device: it reads the five driver commands and drives HOME_N (push-pull, low = magnet
* present). Its parameters here: {params} (the rest at their defaults: die 2.6 mm, BOP/BRP 3/1.5 mT
* unless set, phase seeded from the run seed and "M1").
*
{analog_text}
*
* The VSYS reading binds P0.02 with load=none (an ideal input): the firmware reads VSYS once, at idle,
* before H0, so the SAADC's charge sharing (E2's subject, a few hundred uV) does not reach the duty; with
* the loading view (saadc_se) after that one acquisition, ngspice stalled 28-40 ms into the quiet
* stretch after the move at phase0 (found in M10; recorded in ADR-008 Amendment 11).
*
* The bench (the main board alone, as E1 and E2): the suite's own power path of
* test_firmware_microstep_pwm[VBAT3.7-25C] (test_motor_drive.py power() at 6854d25): the cell 3.7 V
* behind RPACK 0.2 Ohm (a Norton source), the base board's VBAT (C3) and VSYS (C2, C4) capacitors,
* the BQ25180's battery-mode BAT-FET (RON_BAT max 90 mOhm, the only element of BQ25180_PATH with
* usb = 0), the 12-way link (28 AWG 80 mm + two aged JST contacts: 48.4 mOhm VSYS, 32.3 mOhm GND), the
* main board's VSYS capacitors (C1, C2, C6, C41, C70, C72, C74), the 150 mA of other VSYS loads, and
* +3V3 from an ideal 3.3 V behind 50 mOhm (a Norton source) with the main board's +3V3 capacitors. Every
* VSYS capacitor at the suite's worst-case value at 3.7 V (Samsung DC bias minus the MPN tolerance) with
* 10 mOhm ESR. Left out: the link's 80 nH (a branch-current unknown in series with milliohms stalls
* phase0, AGENTS.md; its corner is 4 MHz, the suite's comment: "no effect at the 25 kHz PWM or
* slower").
"""

PINS = """\
*bb part U10 package=nordic/nrf52840-qiaa
*bb pin U10.P0.26 net=MOT_A_IN1 model=digital_out set=S0S1_bound voh=3.3
*bb pin U10.P0.05 net=MOT_A_IN2 model=digital_out set=S0S1_bound voh=3.3
*bb pin U10.P0.07 net=MOT_B_IN1 model=digital_out set=S0S1_bound voh=3.3
*bb pin U10.P0.11 net=MOT_B_IN2 model=digital_out set=S0S1_bound voh=3.3
*bb pin U10.P1.15 net=MOT_NSLEEP model=digital_out set=S0S1_bound voh=3.3
*bb pin U10.P1.14 net=HOME_N model=digital_in vih=2.31 vil=0.99
*bb pin U10.P0.02 net=VSYS_SENSE model=adc_in vref=3.6 load=none
*bb pin U10.VDD net=P3V3 model=adc_in vref=3.6
*bb part M1 package=dms/detent-mechanism {params}
*bb pin M1.IN_A1 net=MOT_A_IN1 model=digital_in
*bb pin M1.IN_A2 net=MOT_A_IN2 model=digital_in
*bb pin M1.IN_B1 net=MOT_B_IN1 model=digital_in
*bb pin M1.IN_B2 net=MOT_B_IN2 model=digital_in
*bb pin M1.NSLEEP net=MOT_NSLEEP model=digital_in
*bb pin M1.HOME_N net=HOME_N model=digital_out
"""

BENCH = """\
* ---- bench: the suite's power path at VBAT3.7, 25 C (node NB_GND is the base board's ground)
BCELL NB_GND NB_VBAT I = (3.7 - V(NB_VBAT,NB_GND))/0.2
RBC3_ESR NB_VBAT NB_VBAT_C3 0.01
CBC3 NB_VBAT_C3 NB_GND 4.7u
RBATFET NB_VBAT NB_VSYS 0.09
RBC2_ESR NB_VSYS NB_VSYS_C2 0.01
CBC2 NB_VSYS_C2 NB_GND 4.88u
RBC4_ESR NB_VSYS NB_VSYS_C4 0.01
CBC4 NB_VSYS_C4 NB_GND 4.88u
RLINK NB_VSYS VSYS 0.0484
RLGND 0 NB_GND 0.0322667
IOTHER VSYS 0 DC 0.15
B3V3 0 P3V3 I = (3.3 - V(P3V3))/0.05
* ---- main board VSYS capacitors (worst case at 3.7 V) and the +3V3 capacitors
RC1_ESR VSYS VSYS_C1 0.01
CC1 VSYS_C1 0 11.0176u
RC2_ESR VSYS VSYS_C2 0.01
CC2 VSYS_C2 0 4.88u
RC6_ESR VSYS VSYS_C6 0.01
CC6 VSYS_C6 0 4.88u
RC41_ESR VSYS VSYS_C41 0.01
CC41 VSYS_C41 0 4.88u
RC70_ESR VSYS VSYS_C70 0.01
CC70 VSYS_C70 0 85.374n
RC72_ESR VSYS VSYS_C72 0.01
CC72 VSYS_C72 0 85.374n
RC74_ESR VSYS VSYS_C74 0.01
CC74 VSYS_C74 0 5.7906u
"""

P3V3_CAPS = [("C3", "10u"), ("C4", "100n"), ("C10", "10u"), ("C11", "100n"), ("C15", "100n"), ("C33", "1u"),
             ("C35", "4.7u"), ("C36", "100n"), ("C50", "100n"), ("C51", "1u"), ("C52", "100n"), ("C53", "10u"),
             ("C54", "100n"), ("C71", "100n"), ("C73", "100n"), ("C75", "100n")]

READING = """\
* ---- the VSYS reading (power sheet): R1 2.2 MOhm over R2 1 MOhm, C5 100 nF
R1 VSYS VSYS_SENSE 2.2Meg
R2 VSYS_SENSE 0 1Meg
C5 VSYS_SENSE 0 100n
"""

SHEET = """\
* ---- the detent sheet: U40 (coil A), U41 (coil B), R70, J40's coils
XU40 VSYS P3V3 MOT_A_IN1 MOT_A_IN2 MOT_NSLEEP MOT_A_OUT1 MOT_A_OUT2 0 TI_DRV8837_BRIDGE params: rds=0.33 temp_c=25 twake=30u
XU41 VSYS P3V3 MOT_B_IN1 MOT_B_IN2 MOT_NSLEEP MOT_B_OUT1 MOT_B_OUT2 0 TI_DRV8837_BRIDGE params: rds=0.33 temp_c=25 twake=30u
R70 MOT_NSLEEP 0 100k
RCOILA MOT_A_OUT1 COILA_L 20
RCOILB MOT_B_OUT1 COILB_L 20
"""

# The homing boards: the detent sheet's input side only (the DRV8837 inputs' 100 kOhm pull-downs, RPD of
# SLVSBA4F Table 6.5, and R70 on the shared nSLEEP).
INPUTS = """\
* ---- the detent sheet's input side: U40/U41's RPD (100 kOhm, SLVSBA4F Table 6.5) on IN1, IN2 and the
*      shared nSLEEP (two in parallel), and R70
RPD_A1 MOT_A_IN1 0 100k
RPD_A2 MOT_A_IN2 0 100k
RPD_B1 MOT_B_IN1 0 100k
RPD_B2 MOT_B_IN2 0 100k
RPD_NS MOT_NSLEEP 0 50k
R70 MOT_NSLEEP 0 100k
"""

ANALOG_FULL = """\
* The analog side of this board is the whole detent sheet: U40 and U41 as the package view
* ti/drv8837:bridge (ideal switches, rDS(on) 0.33 Ohm, the suite's RDS_MAX; twake 30 us), fed by the PWM
* edges at their exact instants through the pins' glue, and J40's coils, 20 Ohm + 4.3 mH each
* (motor_drive__stepper.lib r25 and lph at 25 C), {emf_text}"""

ANALOG_INPUTS = """\
* The analog side of this board is the detent sheet's input side (U40/U41's input pull-downs and R70)
* and the bench; the bridges' power stage and J40's coils are on board-step.cir only. With them, ngspice
* takes about 0.4 s of wall time per simulated ms while the PWM runs (the ideal-switch view at phase0:
* about 60 solver points per edge, 100 000 edges per second; measured in M10, ADR-008 Amendment 11),
* about 20 minutes for one homing, and it keeps every solver point in memory. The homing checks read
* the device, the firmware and the pins, none of which the analog side feeds back into (the device
* reads the driver commands, ADR-008 Amendment 9), so the homing runs leave the power stage out."""


def emf_lines(emf):
    w =2 * 3.14159265358979 * emf["pps"] / 4
    e = emf["ke"] * emf["pps"] / 1000
    t0, t1 = emf["t0"] / 1e15, emf["tstop"] / 1e15
    gate = f"(time > {t0!r} && time < {t1!r})"
    ph = f"({w!r}*(time - {t0!r}) - {emf['delta']!r})"
    return (f"LCOILA COILA_L COILA_E 4.3m\nLCOILB COILB_L COILB_E 4.3m\n"
            f"BEMFA COILA_E MOT_A_OUT2 V = {gate} ? {e!r}*(-sin{ph}) : 0\n"
            f"BEMFB COILB_E MOT_B_OUT2 V = {gate} ? {e!r}*cos{ph} : 0\n")


def emf_text(emf):
    return ("with the suite's back-EMF of a rotor turning at 446 PPS from the step's\n* microstep 0 "
            f"(t0 = {emf['t0']} fs, from homing_step.elf's PWM edges) to the end of its microstep 8 "
            f"({emf['tstop']} fs):\n* ke 1.37 V per k step/s (KE_MAX), load angle pi/2, "
            "motor_drive__stepper.lib's PMSTEP_PHASE (phase A -E sin, phase B +E cos).")


def board(name):
    title, params, emf = CASES[name]
    full = emf is not None
    analog = ANALOG_FULL.format(emf_text=emf_text(emf)) if full else ANALOG_INPUTS
    out = HEADER.format(name=name, title=title, params=params, analog_text=analog)
    out += "*\n" + PINS.format(params=params) + "\n* Circuit body. Node 0 is the main board's GND.\n"
    out += BENCH
    for ref, v in P3V3_CAPS:
        out += f"R{ref}_ESR P3V3 P3V3_{ref} 0.01\nC{ref} P3V3_{ref} 0 {v}\n"
    out += READING
    if full:
        out += SHEET + emf_lines(emf)
        out += "\n* ---- ti/drv8837 view bridge (packages/ti/drv8837/analog/bridge.lib, verbatim)\n"
        out += VIEW.read_text()
        out += "\n* phase0 (spec/package.md section 8), the set ti/drv8837:bridge is qualified at.\n"
    else:
        out += INPUTS
        out += "\n* phase0 (spec/package.md section 8).\n"
    out += ".options RELTOL=1e-5 VNTOL=1e-8 ABSTOL=1e-13\n.end\n"
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--check", action="store_true")
    a = ap.parse_args()
    bad = []
    for name in CASES:
        text = board(name)
        p = OUT / name
        if a.check:
            if not p.is_file() or p.read_text() != text:
                bad.append(name)
        else:
            OUT.mkdir(parents=True, exist_ok=True)
            p.write_text(text)
            print(f"dms_home_boards: wrote {p.relative_to(ROOT)}")
    if bad:
        print(f"dms_home_boards: FAIL: {', '.join(bad)} differ from scripts/dms_home_boards.py's output "
              f"(run it and review the diff)")
        return 1
    if a.check:
        print(f"dms_home_boards: PASS: {len(CASES)} board files are the generator's output, the bridge view "
              f"verbatim from {VIEW.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
