# Status and roadmap

This page separates what exists in the repository from what is planned. The
plan is `docs/phase1-plan.md`; its status lines and review notes are the
authority. Nothing on this page under "Planned" is implemented.

## Done

- **Phase 0**, the co-simulation spike: Renode and ngspice under the
  time-grant protocol, with the speed-ups of ADR-003 (grant lattice, edge
  traces, CPU lookahead, ADC pushes), ADR-004 (engine startup and the warm
  Renode pool) and ADR-005 (Breadboard's own STM32F072 ADC model and sampled
  inputs).
- **Phase 1, M0–M7**: the reference inputs vendored and the board's SPICE
  suite re-run (M0); the lookahead promise clamp (M1); the TPS22917 view's
  numerics fix (S1); the nRF52840 platform and its gates (M2); package format
  v0 (M3); the KiCad importer, `board.bbm`, projects and
  `bb open`/`flash`/`run`/`probe` (M4); the E1 packages and the
  datasheet-fitted TPS22917 view (M5, M5b); E1 on the imported board (M6);
  the README quickstart, `make demo` and the stranger test (M7).
- **Phase 1, M8 (E2)**: Breadboard's own nRF52840 SAADC model, a loading
  `adc_in`, per-sample records, UART log records and the VDD channel; the
  battery reading and low-battery LED shed on dial-me-spinner
  ([the E2 demo](./demo-e2)).
- **Phase 1, M9 (GDB passthrough)**: `bb run --gdb <port>`
  ([Debugging with GDB](./debugging)). Attaching, halting, reading and
  detaching leave the outputs byte-identical. A breakpoint stop changes
  them, so such a run is marked as a debug run (ADR-008 D10's fallback).
- **Phase 1, M10 (E3, the stretch)**: Breadboard's nRF52840 PWM, TEMP and
  NVMC models with the PWM engine path; the Rust device API and device
  parts in board files (ADR-006 Amendment 6); the mechanism and DRV5032
  device `dms/mechanism@1`; the ideal-switch `ti/drv8837` view; the Zephyr
  homing firmware; the detent homing H0–H5 on dial-me-spinner with the
  HOME_N latency gate ([the E3 demo](./demo-e3)).
- **Phase 1, M11 (systems)**: the base board imported next to the main
  board (`examples/dial-me-spinner/system`), `[[connection]]` between
  them through a harness view, and qualified names across the two boards
  ([Projects and benches](../reference/project#systems-connection)). One
  board is still simulated.

The stranger test has also been run against the published repository with
the image's default apt sources: the README took 68.2 s against the
15-minute limit. The one remaining exit item is the human run
(`docs/phase1-exit-checklist.md`).

Every Phase 1 milestone (M0–M11) is done.

## Out of Phase 1

The plan names these as decisions, not gaps ("Scope", `docs/phase1-plan.md`):

| Item | Where it goes |
| --- | --- |
| Public package registry, signing, `bb pkg publish` | Phase 2. Packages are local directories plus a lockfile. |
| `pi`/`si`/`em` fidelity rungs, rail-tracking pin outputs, brown-out reset from a rail | Phase 2. Views may already declare `pi`. |
| Devices in imported projects (route B: a `[[device]]` section with pin-to-net bindings in `bb.toml`) | Phase 2 (approver decision 2, 2026-11-14). Phase 1's device parts exist only in hand-written board files, which is how E3 runs. |
| Python device API, bus devices (TWIM/SPIM) | Phase 2. |
| An averaged DRV8837 view, and a lookahead for a CPU fed by two engines | Not needed for E3's gates; the homing runs take one-cell grants and leave the bridges' power stage out of ngspice (ADR-008 Amendment 11). |
| BLE/RADIO, USBD, NFCT, QSPI, TWIM, SPIM3, I2S-to-pin, PWM (unless E3) | Absent or not faithful in Renode 1.17.0; each is its own model and gate. |
| Bench files with CSV playback, `bb test`, JUnit | Phase 2. Phase 1 checks are scripts over the outputs. |
| Multi-MCU and multi-board simulation | Phase 3. |
| Firmware images other than ELF | The schema already takes a list of typed images; only `elf` is implemented. |
| PCB geometry, parasitics, gerbers | Nothing at `fast` fidelity consumes geometry. |
| RP2040 package | Deferred; the Renode RP2040 platform was not evaluated. |
