#!/usr/bin/env python3
"""Write E2's assertion files (milestone B2; ADR-010 D10, Amendment 3) from check_dms_vsense.py's JSON.

    dms_asserts.py <check_dms_vsense.json> <dir>

One file per case, <dir>/<case>.toml (s1, s1-none, s50, s50-none, c5, c5-none, sag, rec), for
`bb run ... --assert`. A check that needs the suite's charge-sharing arithmetic stays in the
script (no expression language, ADR-010 D5); what the assertions carry is its result for this
bench, as exact integers:

  s1, s50, c5          every reading's burst code equals the code the script's model predicts from
                       the unloaded pad and the run's acquisition instants; the number of readings
  s1-none, s50-none,   every reading's code equals the unloaded pad's code; the number of readings
  c5-none
  sag                  the LED shed: every reading before LED_PWR_EN falls >= 1283 and LEDs on, the
                       first one after it <= 1282, none after it with the LEDs on, LED_PWR_EN one
                       rise and one fall, one "leds off" line; and the shed timing: LED_PWR_EN
                       falls within one burst period (1 s) after V(VSYS_SENSE) crosses below the
                       code boundary 1282.5 LSB less the script's modelled mean sampling error,
                       written as a whole-uV threshold (rounded up, so the comparison of a uV-
                       rounded sample with it is the script's)
  rec                  the shed and the restore: before the fall >= 1283, the shed reading <= 1282,
                       no LEDs-on reading until the "leds on" line, the restoring reading's
                       VBAT_est >= 3.58 V and none with the LEDs off after it, LED_PWR_EN rises
                       twice and falls once, off at least 30 ms, one "leds off" and one "leds on"

make assert-gate regenerates them from its own runs and requires the committed files byte for
byte. Needs only the standard library.
"""
import json
import math
import os
import sys

HEAD = """# examples/dial-me-spinner/asserts/e2/{case}.toml: E2's {what} (Phase 1 plan section 2 E2, ADR-008
# Amendments 4-6) as assertions (ADR-010 D5, D10; Amendment 3), for `bb run ... --assert` on the
# case {case} of scripts/dms_vsense.sh. Written by scripts/dms_asserts.py from check_dms_vsense.py's
# results on the committed runs; make assert-gate rewrites it from its own runs and requires this
# file byte for byte. Do not edit by hand.
schema = "bb-bench/1"
"""

READ = '^uart0: vsense: n='


def block(**kv):
    out = ["", "[[assert]]"]
    for k, v in kv.items():
        if isinstance(v, bool):
            out.append(f"{k} = {'true' if v else 'false'}")
        elif isinstance(v, int):
            out.append(f"{k} = {v}")
        elif isinstance(v, dict):
            inner = ", ".join(f'{a} = {b}' if isinstance(b, int) else f'{a} = "{b}"' for a, b in v.items())
            out.append(f"{k} = {{ {inner} }}")
        else:
            out.append(f'{k} = "{v}"')
    return "\n".join(out) + "\n"


def codes_file(case, codes, why):
    t = HEAD.format(case=case, what="reading codes")
    t += f"# Each reading's burst code: {why}.\n"
    t += block(name="E2-readings", kind="log", source="U10", match=READ, min=len(codes), max=len(codes),
               msg="the number of readings")
    for k, c in enumerate(codes):
        t += block(name=f"E2-code-n{k}", kind="log", source="U10", match=f"{READ}{k} ", field="code", min=c,
                   max=c, msg=f"reading {k}'s code")
    return t


EN_FALL = {"edge": "U10.P1.04", "level": 0}
EN_RISE = {"edge": "U10.P1.04", "level": 1}


def sag_file(s):
    thr = math.ceil(s["boundary_uv"] - s["e_uv"])
    t = HEAD.format(case="sag", what="LED shed and its timing")
    t += block(name="E2-before-shed", kind="log", source="U10", match=READ, field="code", until=EN_FALL, min=1283,
               msg="every reading before the shed above the threshold code (VBAT_est >= 3.53 V)")
    t += block(name="E2-shed", kind="log", source="U10", match=READ, field="code", after=EN_FALL,
               until={"log": READ, "source": "U10"}, max=1282, msg="the shedding reading at or below 1282")
    t += block(name="E2-on-before", kind="log", source="U10", match="leds=off$", until=EN_FALL, present=False,
               msg="the LEDs on in every reading before the shed")
    t += block(name="E2-off-after", kind="log", source="U10", match="leds=on$", after=EN_FALL, present=False,
               msg="no reading with the LEDs on after the shed")
    t += block(name="E2-en-rises", kind="count", net="U10.P1.04", level=1, min=1, max=1,
               msg="LED_PWR_EN rises once (boot)")
    t += block(name="E2-en-falls", kind="count", net="U10.P1.04", level=0, min=1, max=1,
               msg="LED_PWR_EN falls once (the shed)")
    t += block(name="E2-leds-off-line", kind="log", source="U10", match="^uart0: vsense: leds off", min=1, max=1,
               msg="one leds off line")
    t += (f"\n# The shed timing: the boundary {s['boundary_uv']:.6f} uV (1282.5 LSB) less the modelled mean\n"
          f"# sampling error {s['e_uv']:.6f} uV, rounded up to {thr} uV.\n")
    t += block(name="E2-shed-timing", kind="edge_timing",
               start={"cross": "VSYS_SENSE", "below": f"{thr}uV"}, end=EN_FALL, pair="first", min="1fs", max="1s",
               msg="LED_PWR_EN falls within one burst period after the threshold crossing")
    return t


def rec_file():
    t = HEAD.format(case="rec", what="LED shed and restore")
    on_line = {"log": "^uart0: vsense: leds on", "source": "U10"}
    t += block(name="E2-before-shed", kind="log", source="U10", match=READ, field="code", until=EN_FALL, min=1283,
               msg="every reading before the shed above the threshold code")
    t += block(name="E2-shed", kind="log", source="U10", match=READ, field="code", after=EN_FALL,
               until={"log": READ, "source": "U10"}, max=1282, msg="the shedding reading at or below 1282")
    t += block(name="E2-off-between", kind="log", source="U10", match="leds=on$", after=EN_FALL, until=on_line,
               present=False, msg="the LEDs off in every reading between the shed and the restore")
    t += block(name="E2-restore", kind="log", source="U10", match=READ, field="vbat_est", after=on_line,
               until={"log": READ, "source": "U10"}, min="3.58V", msg="the restoring reading's VBAT_est >= 3.58 V")
    t += block(name="E2-on-after", kind="log", source="U10", match="leds=off$", after=on_line, present=False,
               msg="the LEDs on in every reading after the restore")
    t += block(name="E2-en-rises", kind="count", net="U10.P1.04", level=1, min=2, max=2,
               msg="LED_PWR_EN rises twice (boot, restore)")
    t += block(name="E2-en-falls", kind="count", net="U10.P1.04", level=0, min=1, max=1,
               msg="LED_PWR_EN falls once (the shed)")
    t += block(name="E2-off-time", kind="edge_timing", start=EN_FALL, end=EN_RISE, min="30ms",
               msg="restored no sooner than 30 ms after the shed")
    t += block(name="E2-leds-off-line", kind="log", source="U10", match="^uart0: vsense: leds off", min=1, max=1,
               msg="one leds off line")
    t += block(name="E2-leds-on-line", kind="log", source="U10", match="^uart0: vsense: leds on", min=1, max=1,
               msg="one leds on line")
    return t


def main():
    js, d = sys.argv[1], sys.argv[2]
    res = json.load(open(js))["results"]
    os.makedirs(d, exist_ok=True)
    files = {}
    for case in ("s1", "s50", "c5"):
        files[case] = codes_file(case, res[case]["pred_codes"],
                                 "the code check_dms_vsense.py's model predicts from the unloaded pad and the "
                                 "run's acquisition instants (the suite's charge-sharing arithmetic)")
        vth = res[case]["vth_uv"]
        unloaded = math.floor(vth * 4096 / 3_600_000 + 0.5)
        files[f"{case}-none"] = codes_file(f"{case}-none", [unloaded] * len(res["readings"][f"{case}-none"]),
                                           f"the unloaded pad's code (V(pad) {vth} uV)")
    files["sag"] = sag_file(res["sag"])
    files["rec"] = rec_file()
    for case, text in files.items():
        with open(os.path.join(d, f"{case}.toml"), "w") as f:
            f.write(text)
    print(f"dms_asserts: wrote {len(files)} files in {d}")


if __name__ == "__main__":
    main()
