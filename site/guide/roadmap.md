# Status and roadmap

This page separates what exists in the repository from what is planned. The
plans are `docs/phase1-plan.md` and `docs/phase2-plan.md`; their status
lines and review notes are the authority. Nothing on this page under
"Planned" is implemented.

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

## Phase 2: the technical exit

Phase 2 (`docs/phase2-plan.md`, ADR-009 to ADR-016) is power integrity,
benches with assertions and a package ecosystem. This build's scope is
merged, and its technical exit was measured on 2026-11-17 ("Review notes:
technical exit" in the plan):

- **Hardening (H0–H6):** track worktrees and a measurement lock; an
  ngspice progress watchdog and engine-owned `.save`; the lattice snap
  that fixed the SAADC stall; a reverse-connection External Control client
  that refuses foreign clients; scoped platform hashes; quiet grants with
  several producers (E3's homing in 433 grants instead of 270 000); a
  nightly CI tier. `make check` takes 478 s on an idle ext4 worktree.
- **The schema epoch (S)** and **benches, assertions and `bb test`
  (B1–B3)**: `bb-bench/1` with CSV playback and faults, assertions
  evaluated at solver points, `bb test` files with `test.json` and JUnit
  ([Testing](./testing)). E1–E3 and W1–W2 are `bb test` files.
- **Run modes (R1–R2):** devices in `bb.toml`, and the main and base
  boards simulated together in one deck.
- **The `pi` rung (P0–P5, W-fw):** power views for the RT9080, BQ25180,
  TPS61040 and the battery, rail-tracking tristate pins, supply currents
  from the CPU's states, POFWARN and brown-out reset from the rail
  ([Fidelity](../concepts/fidelity)). **W1**, POF blanking on a dead
  battery, passes with the firmware in the loop at a labelled stress corner
  and is the README quickstart's last line; **W2**, the brown-out reboot,
  passes: the old rule browns out (once in 200 ms; on these models the
  second boot completes, so the loop does not repeat), the rule does not.
  The nRF52840's supply current follows the regulator the firmware runs on
  (the LDO by default).
- **The registry (E1)** and **the viewer and VS Code templates (E4)**: the
  registry, signing and `bb-lock/1` work end to end against local
  fixtures; publishing it is a human step that has not been taken.

Not started in this build: `bb capture import` (B4), runs without a CPU
target (R3), W3 (P6), the package pipeline and the first 200 packages
(E2, E3) and the adoption work (E5).

Moved to Phase 3 by the approver (2026-11-17): bench environment signals
(`[[env]]`) and per-probe rates (`[probes]`), the bench engine's lockfile
entry, and `bb pkg init --from-svd/--from-ibis`. `bb` refuses each of them
with a message that says Phase 3.

## Out of Phase 1

Several of these rows were Phase 2 work and are now done (above): the
registry and signing (against local fixtures), the `pi` rung with
rail-tracking pins and brown-out reset, devices in imported projects,
bench files, `bb test` and JUnit, and several boards with one MCU in one
deck.

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
| Multi-MCU and multi-board simulation | Multi-MCU: Phase 3. Several boards with one MCU in one deck: done in Phase 2 (R2, a single-MCU system run; `examples/dial-me-spinner/e1-system` and `w1`). |
| Firmware images other than ELF | The schema already takes a list of typed images; only `elf` is implemented. |
| PCB geometry, parasitics, gerbers | Nothing at `fast` fidelity consumes geometry. |
| RP2040 package | Deferred; the Renode RP2040 platform was not evaluated. |
