# The E1 demo: LED rail sequencing

E1 is Phase 1's definition of done: the LED rail power sequencing of
dial-me-spinner rev A, imported from its KiCad netlist (201 parts, 142 nets).

## The circuit

- The firmware is prebuilt **Zephyr v4.5.0**, running on the **nRF52840**
  inside the Raytac module **U10**, in Renode.
- It switches the **TPS22917** load switch **U2** through `LED_PWR_EN`
  (U10 pin P1.04). U2 powers the 48-LED **WS2812C** ring's rail, **VLED**.
- It drives the ring's data line (U10 pin P0.06, `LED_DIN_MCU`, through R40
  into `LED_DIN`, the first LED's data input).
- ngspice solves the part of the board that the bench puts in scope:
  132 instances.

The two MCU pins are bound in `examples/dial-me-spinner/bb.toml` as
`digital_out` pins with the nRF52840 drive sets `S0S1` (LED_PWR_EN) and
`H0H1_max` (LED_DIN_MCU). Every other part comes from the imported netlist and
the packages.

## The benches

`examples/dial-me-spinner/bench/` holds two benches, `nominal.toml` and
`cold.toml`. The quickstart uses the cold bench. It stands in for the base
board's power path with the dial-me-spinner suite's own constants:

- `VSYS`: 3.0 V behind 0.5692 Ω, loaded by 50 mA;
- `+3V3`: 3.3 V tracking `VSYS` minus 44.167 mV (an ideal LDO), behind 10 mΩ;
- temperature −10 °C;
- an explicit analog scope (the nets VSYS, +3V3, VLED, the LED data chain and
  so on) and the solver option set `phase0`.

The bench file's comments say what it does not reproduce: the BQ25180
power-path dynamics, the base-board VSYS capacitance, tolerances, DC bias and
Monte Carlo corners.

## The checks

`scripts/check_dms_rail.py` tests three rules from the board's own SPICE suite,
and the firmware rule's own timing:

| Check | Rule |
| --- | --- |
| **LED-DATA-V4** | VLED is at least 2.67 V whenever data starts. |
| **LSW-3** | DIN never goes more than 0.7 V above VLED. |
| **LSW-2** | VLED falls below 10 % within 30 ms of switch-off. |
| **FW-6.4** | Data starts at least 10 ms after LED_PWR_EN rises, and LED_PWR_EN stays low at least 30 ms before it rises again. |

## Correct order: all four pass

The correct-order firmware follows dial-me-spinner's `docs/firmware.md` 6.4. It
raises LED_PWR_EN before it sends data, and it takes the data line low before
it drops LED_PWR_EN. In this Zephyr image, data starts 30 ms after LED_PWR_EN,
not the 10 ms that 6.4 gives (ADR-008 Amendment 3); FW-6.4 only requires at
least 10 ms.

```text
check_dms_rail: PASS: LED-DATA-V4: VLED 2.971510 V at 30.190000 ms (P0.06 rose 30.060 ms after LED_PWR_EN), ...
check_dms_rail: PASS: LSW-3: max(V(LED_DIN) - V(VLED)) = 0.320308 V at 30.400 ms ...
check_dms_rail: PASS: LSW-2: LED_PWR_EN fell at 35.230000 ms from VLED 2.971510 V: below 10 % after 24.420 ms; ...
check_dms_rail: PASS: FW-6.4: P0.06 rose 30.060 ms after LED_PWR_EN; ...; LED_PWR_EN off for 35.026 ms before rising again ...
check_dms_rail: PASS: LED-DATA-V4, LSW-3, LSW-2, FW-6.4
```

## Wrong order: LSW-3 fails

The wrong-order firmware (`led_rail_wrong_order.elf`) reverses the switch-off.
It drops LED_PWR_EN first and keeps the data line high while VLED discharges.
Its waits are unchanged, so FW-6.4 passes. LSW-3 fails, and the message gives
the excursion and the clamp current it drives into the first LED's DIN. The
script exits 1:

```text
check_dms_rail: FAIL: LSW-3: max(V(LED_DIN) - V(VLED)) = 0.792868 V at 45.260 ms (VLED 1.486643 V, LED_DIN 2.279511 V;
    DIN clamp current 4.083 mA from the R40 drop ...) (limit <= 0.7 V)
check_dms_rail: FAIL: LED-DATA-V4, LSW-3, LSW-2, FW-6.4 (failed: LSW-3)
```

That is the point of the demo: a firmware ordering bug shows up as a
violation of an analog rule on the board's own circuit.

![VLED and LED_DIN over 120 ms for the correct-order and the wrong-order firmware, and a zoom on the wrong order's switch-off where LED_DIN rises 0.793 V above VLED at 45.26 ms](/plots/e1-rail.light.svg){.plot-light}
![VLED and LED_DIN over 120 ms for the correct-order and the wrong-order firmware, and a zoom on the wrong order's switch-off where LED_DIN rises 0.793 V above VLED at 45.26 ms](/plots/e1-rail.dark.svg){.plot-dark}

*The two runs of `make demo` (`scripts/demo.sh`): `bb run --for 120ms
--bench bench/cold.toml --probe net:VLED --probe net:LED_DIN` with
`led_rail.elf` (top) and `led_rail_wrong_order.elf` (middle; bottom, 33–48 ms).
The traces are `nets.csv` (10 µs samples); the LED_PWR_EN edges are the runs'
`pin_edge` records for P1.04. Shaded: LED_DIN above VLED + 0.7 V. The wrong
order's second switch-off (115.39 ms) heads the same way until the run ends at
120 ms. Drawn by `make docs-plots`.*

## How it is gated

- `make dms-rail-test` (part of `make test`): both firmwares at both benches
  at `bb`'s defaults, the checks, the committed one-cell hashes, the VLED
  snapshots, a clean `PIN_CNF` audit, one analog-first run byte-identical,
  and two FW-6.4 negative controls.
- `make dms-rail` (the M6 gate, CI job `dms-rail`, about 3 min fresh): the
  above, plus every case's one-cell reference (`--max-quantum 10us`), the
  board-level convergence gate (120 ms in under 30 s at both engine orders),
  the audit's negative controls and 10-run determinism.
- `make fw-rebuild` (CI job `fw-rebuild`): rebuilds the committed E1 images in
  a fresh west workspace (Zephyr v4.5.0, SDK 1.0.1) and requires the image
  hashes of `firmware/prebuilt/SHA256SUMS`.

## About the TPS22917 model

The committed TPS22917 view is **datasheet-fitted**. A vendor-derived view
exists only as a local, never-committed package (the TI model's
redistribution terms were not checked; Phase 1 approver decision A3). The
plan's M5b gate compares the two on the E1 checks locally (VLED at each P0.06
rise and the LSW-3 maximum within 10 mV, LSW-2 within 1 ms, the wrong-order
excursion within 10 mV; `make dms-m5b`). `bb run` flags the committed view as `agent_generated`;
see [Fidelity and honesty](../concepts/fidelity).
