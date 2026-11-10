# The E2 demo: battery reading and LED shed

The second worked example (Phase 1 M8) runs on the same imported
dial-me-spinner board as [E1](./demo). The Zephyr firmware reads the
battery the way dial-me-spinner's `docs/firmware.md` §12.4 describes, and
sheds the LEDs when the battery runs low.

## What is simulated

- **The divider.** VSYS_SENSE is VSYS through R1 2.2 MΩ over R2 1 MΩ,
  with C5 100 nF, into AIN0 (P0.02).
- **The SAADC.** Gain 1/6 against the 0.6 V reference, 12 bits, 8×
  oversampling with BURST, and an acquisition time (TACQ) of 40 µs. The
  firmware computes VBAT_est = 0.979 × code × 2.8125 mV.
- **The policy.** Below 3.53 V the firmware sheds the LEDs with E1's off
  sequence. At 3.58 V or above it turns them back on. Every reading is a
  UART line, which `bb run` records as a log record.

Renode 1.17.0 has no nRF52 SAADC, so this is Breadboard's own model
(ADR-008 D7). The input **loads** VSYS_SENSE the way the silicon does. At
each acquisition, a 2.5 pF sample capacitor pre-charged to VDD shares its
charge with C5 and the pad. Because of that charge, the reading depends on
how often the firmware samples. dial-me-spinner's own SPICE suite measured
this effect, and E2 reproduces it inside a firmware run.

## Run it

```sh
make dms-vsense-test   # the short part, included in make test
make dms-vsense        # the full M8 gate, about 14 min
```

## What the gate checks

**The analog side.** Each loaded run is compared with the same run with
`load = "none"`, which reads the unloaded pad. The table gives the mean
sampling error of an 8-sample burst in steady state, on the suite's
worst-case bench (`bench/vsense.toml`):

| Interval | Co-simulation | Suite arithmetic, nominal parts | Suite at its tolerance corner |
| --- | --- | --- | --- |
| 1 s (`vsense.elf`) | 272.5 µV | 272.6 µV | 356.2 µV |
| 50 ms (`vsense_50ms.elf`) | 726.4 µV | 726.3 µV | 761.3 µV |
| C5 fitted at 10 nF, one burst | 2 676.6 µV | 2 675.0 µV | 3 474.3 µV |

The suite's own figures use a part-tolerance corner (C5 −23.5 %, R +1 %)
that a Phase 1 bench cannot set. So the check ties the co-simulation to
them through the suite's charge-sharing arithmetic:
- at nominal parts, the arithmetic must match every sample (within 1 µV
  plus 0.25 %) and the burst mean (within 3 µV);
- at the corner, the same arithmetic must reproduce the suite's figures
  within 2 µV.

**Firmware-visible.** Every code is exactly the one the arithmetic
predicts:
- 1067 at a 1 s interval;
- 1068 from the sixth burst at 50 ms, where the 0.83 LSB error crosses a
  rounding boundary;
- 1070, 3 LSB high, with C5 fitted at a tenth of its value.

**The shed** (`bench/sag.toml`). The battery's open-circuit voltage falls
10 mV/s behind 0.32 Ω. At the reading of code 1282 (VBAT_est 3.5299 V),
the firmware sheds the LEDs. LED_PWR_EN falls within one reading period of
the threshold crossing.

**The restore** (`bench/sag-recover.toml`). The battery dips to 3.58 V at
1 s and recovers to 3.72 V at 2 s. The LEDs are shed at the 1 s reading
and come back at the 2 s reading, in E1's on order: P0.06 goes high 30 ms
after LED_PWR_EN.

**Equivalence and determinism.** Every case gives the same `nets.csv`,
`nets.vcd` and non-grant records as its every-lattice-point reference
(`--lookahead off --adc-push every`). The same holds across other grant
sizes and engine orders. The sag run is identical over 10 runs.

## Simplifications

Compared with `firmware.md` §12.4, the E2 firmware leaves out production
calibration and the rested-OCV correction, and implements only the
"< 3.53 V" row of the policy. The firmware is in
`examples/dial-me-spinner/firmware/vsense/`. Both images are prebuilt, and
`make fw-rebuild` reproduces them from a fresh Zephyr v4.5.0 workspace.
Details are in ADR-008 Amendments 4–6.
