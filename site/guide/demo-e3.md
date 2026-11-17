# The E3 demo: detent homing

The third worked example (Phase 1 M10, the plan's stretch) homes
dial-me-spinner's detent actuator the way its `docs/firmware.md` §5.4
describes. Zephyr firmware microsteps a micro linear stepper through two
DRV8837 H-bridges with the nRF52840's PWM, and a DRV5032 Hall switch tells
it when the carriage's magnet is near the park end.

## What is simulated

- **The firmware** (`examples/dial-me-spinner/firmware/homing`): PWM0
  through nrfx at 25 kHz, 8 microsteps per full step with a sine
  amplitude, slow decay, and two ping-pong sequences that the SEQEND
  interrupt refills one full step ahead. The amplitude follows the MD-3
  rule from the nRF's die temperature (TEMP) and an idle VSYS reading
  (the SAADC, as in [E2](./demo-e2)). The procedure is H0 (wake and
  energise) to H5 (confirm HOME, back off 4 steps), then SETTLE, brake
  and sleep. Every phase is a UART line, which `bb run` records as a log
  record.
- **The nRF52840's PWM, TEMP and NVMC.** Renode 1.17.0 has none of them on
  the nRF52840, so these are Breadboard's own models (ADR-008 Amendment 9).
  The Renode engine reads every PWM pin change from the model's ring and
  reports it at its exact instant, in half-nanoseconds, so ngspice and the
  mechanism see each 25 kHz edge.
- **The mechanism and the DRV5032** are one *device*, `dms/mechanism@1`
  (ADR-006 D7): a small Rust model in `bb`, placed in the board file as a
  part (`*bb part M1 package=dms/detent-mechanism`). It reads the bridges'
  five inputs, filters each coil's commanded polarity through L/R, lets the
  rotor follow the field angle, moves the carriage 0.015 mm per full step
  between the park stop and the click end, looks up the magnet's field at
  the switch in `firmware.md`'s table, and drives HOME_N from a DRV5032
  that samples every tS from a seeded phase.
- **The bridges and coils** (`ti/drv8837`'s ideal-switch view, 20 Ω +
  4.3 mH coils, the suite's back-EMF) are in ngspice on the one-step board.
  The homing boards carry only the bridges' inputs, because the full sheet
  costs about 0.4 s of wall time per simulated millisecond while the PWM
  runs.

E3's gates run on hand-written board files in
`examples/dial-me-spinner/e3`, because the mechanism is not on the
schematic. Since Phase 2 (R1, ADR-013 D1) a device can also be a
`[[device]]` of an imported project (`bb.toml`; a `virtual` one has no
schematic symbol): `examples/dial-me-spinner/e3/home` and `e3/step` are
E3's homing and its step case as projects of the imported main board
(`make dms-home-project`; see [Projects](../reference/project)).

## Run it

```sh
make dms-home-test   # the one-step case, included in make test (about 40 s)
make dms-home        # the full M10 gate (about 45 min on 5 jobs)
```

## What the gate checks

**One full step at 446 PPS** against the suite's
`test_firmware_microstep_pwm[VBAT3.7-25C]`, over the same window (the
step's microstep 0 to the end of its microstep 8), with the suite's power
path as the bench:

| Quantity | Co-simulation | Suite | Difference |
| --- | --- | --- | --- |
| PWM duty amplitude | 0.8922 (571/640) | 0.8778 | +1.63 % |
| Current-vector magnitude (mean) | 128.47 mA | 126.68 mA | +1.42 % |
| Current per unit of duty | 143.99 mA | 144.31 mA | −0.22 % |
| Thrust estimate | 0.5255 N | 0.5182 N | +1.42 % (limit ≥ 0.5 N) |
| VSYS max | 3.606 V | 3.622 V | −16 mV (limit 5.5 V) |

The firmware scales its duty with its own VSYS reading (3.645 V under the
bench's 150 mA of other loads, where the suite assumes 3.7 V) and with
MD-3's 20.33 Ω, so its duty is 1.6 % higher. Per unit of duty, the drive
chain (PWM edges, bridge, coil, back-EMF) is within 0.25 % of the suite.

![Coil A and coil B currents through one full step at 446 PPS, with their 25 kHz ripple: coil A falls from 156 mA to 0 and coil B rises to 128 mA over 8 microsteps, and the current-vector magnitude averages 128.47 mA against the suite's 126.68 mA](/plots/e3-step.light.svg){.plot-light}
![Coil A and coil B currents through one full step at 446 PPS, with their 25 kHz ripple: coil A falls from 156 mA to 0 and coil B rises to 128 mA over 8 microsteps, and the current-vector magnitude averages 128.47 mA against the suite's 126.68 mA](/plots/e3-step.dark.svg){.plot-dark}

*The `step` case of `make dms-home` (`homing_step.elf`, `board-step.cir`,
45 ms, 1 MHz samples). Each coil's current is its 20 Ω resistor's drop in
`nets.csv`. Drawn by `make docs-plots`.*

**The homing procedure** from the click end. H2 approaches at 446 PPS
until HOME asserts (236 steps). H3 runs in 199 steps. H4 stalls into the
park stop at 80 PPS and half current for 128 steps: the carriage reaches
the stop after about 65 of them, and the rotor slips 16 times over the
remaining 63 steps of overdrive. H5 reads HOME asserted and backs off 4 steps. The
carriage ends 5 steps from the stop, within the ±1–2 steps that
`firmware.md` gives for the stall. The bridges brake for 3.12 ms before
they sleep. A third board starts inside the park zone, so that H1 (the
back-off until HOME releases) runs too.

**The HOME_N latency** (ADR-008 Amendment 10), from the device's field
passing BOP to the HOME_N edge:

| Corner | Sample phase | Latency | Gate |
| --- | --- | --- | --- |
| Typical, tS 50 ms | seeded (41.58 ms) | 44.30 ms | ≤ 75 ms |
| Worst: tS 75 ms, BOP 4.8 mT, BRP 0.5 mT (the suite's DRV5032FA corner) | the worst phase (a sample ends 10 µs before the crossing) | 74.99 ms | ≤ 97.4 ms (the suite's edge window) |
| Typical tS, from the park zone, BOP/BRP 4.8/0.5 mT | seeded | 46.29 ms (and 48.56 ms on release in H1) | ≤ 75 ms |

The gate also reads the device's own tS from its start record: it must be
the DRV5032FA's at the corner, and tS + tACTIVE (the latency of the worst
sample phase) must be within the limit, so the bound does not rest on the
one seeded phase a run uses. A DRV5032FB (tS 200 ms) fails it even though
its seeded phase puts HOME_N at the same instant as the FA's.

**The step rate** of every phase is measured on the PWM edges: the run
lengths of PWM periods per microstep must be exactly what firmware.md
schedules (7 periods at 446 PPS, 10 at 313, 39 at 80). The device has no
pull-out, so a firmware that stepped seven times too fast would still
"home" on it; the gate's negative control, `homing_fast.elf`, does exactly
that and fails the step-rate check, the "no park-stop contact before H4"
check and the "H3 ends 20-108 steps from the stop" check.

![The homing timeline: nSLEEP high from 1 ms to 2.62 s, HOME_N falling at 0.54 s during H2, the park stop reached at 1.80 s in H4; below, a zoom on the HOME_N latency of 44.30 ms from the field passing BOP at 497.317 ms to HOME_N falling at 541.618 ms](/plots/e3-homing.light.svg){.plot-light}
![The homing timeline: nSLEEP high from 1 ms to 2.62 s, HOME_N falling at 0.54 s during H2, the park stop reached at 1.80 s in H4; below, a zoom on the HOME_N latency of 44.30 ms from the field passing BOP at 497.317 ms to HOME_N falling at 541.618 ms](/plots/e3-homing.dark.svg){.plot-dark}

*The `typ` case of `make dms-home` (`homing.elf`, `board.cir`, 2700 ms,
10 kHz samples). The phase markers are the firmware's UART lines; the BOP
crossing and the park stop are the device's own log records. Drawn by
`make docs-plots`.*

**Equivalence and determinism.** Each case gives the same `nets.csv`,
`nets.vcd` and event records as its one-cell reference
(`--lookahead off`) and with the other engine order, and the typical
homing is identical over 10 runs. These boards have two engines feeding
the CPU (the device's HOME_N and ngspice's VSYS reading), and the kernel's
lookahead allows one producer, so every E3 run uses one-cell grants: about
270 000 grants and 4.5 minutes for a homing. bb's defaults, `--lookahead
off` and `--max-quantum 1ms` therefore build the same partition here, and
their equality says nothing about partition independence; that is gated
for devices by `bbdev`'s scheduler tests and `make device-smoke`.

## Simplifications

- The device follows the commanded polarity, not the coil current that
  ngspice computes. It has no thrust, load or rotor inertia, so a stall in
  the loaded zone cannot happen.
- The homing boards leave the bridges' power stage and the coils out of
  ngspice, as described above. The one-step board has the whole sheet.
- The firmware uses the unlearned HOME edges (no factory test 19), switches
  from 446 to 80 PPS at a full-step boundary without a ramp, and does not
  write the carriage state to NVS.
- The VSYS reading uses an ideal SAADC input (`load=none`). The firmware
  reads once at idle, so the charge sharing that E2 studies does not reach
  the duty.

The firmware's two images are prebuilt, and `make fw-rebuild` reproduces
them from a fresh Zephyr v4.5.0 workspace. Details are in ADR-008
Amendments 9–11 and ADR-006 Amendments 6–7.
