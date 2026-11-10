# Phase 1 plan: a real board from KiCad (dial-me-spinner rev A)

- **Status:** Accepted (2026-11-09), with the approver decisions below
  (revised 2026-11-09 after two design reviews). M0 done (2026-11-09:
  inputs vendored with `SOURCE`, suite re-run committed and reproduced;
  see Review notes). M1 done (2026-11-09: the promise clamp, ADR-008
  Amendment 1). S1 done (2026-11-09) without the fallback: a view-local
  fix of the TPS22917 view runs E1 at `phase0` through the engine
  (ADR-006 Amendment 2; Review notes: S1). M3 done (2026-11-09: package format v0
  (ADR-006 Amendment 1), the Phase 0 examples through packages with their
  committed hashes, setup timed in a clean container; Review notes: M3).
  M2 done (2026-11-09: the nRF52840 platform and its gates, ADR-008
  Amendment 2; Review notes: M2). M4 done (2026-11-09: the KiCad importer,
  `board.bbm`, projects and `bb open`/`flash`/`run`/`probe`, ADR-007
  Amendment 1; gate (a) through ADR-007 D9's stated fallback, with its
  cause measured; Review notes: M4). M5 and M5b done (2026-11-09: the E1
  packages, the datasheet-fitted TPS22917 view as the committed default and
  the vendor-derived one local-only, gated against each other on the E1
  checks; ADR-006 Amendment 3; Review notes: M5 and M5b). M6 done
  (2026-11-09: E1 on the imported board with the Zephyr v4.5.0 firmware,
  both benches, `make dms-rail`, the local PWL cross-check within 0.5 mV,
  the cost-table row; ADR-008 Amendment 3, ADR-007 Amendment 3, ADR-006
  Amendment 4; Review notes: M6). M7 done **except two measurements**
  (2026-11-09), and **not published** (decision B). The README quickstart
  (`<CLONE-URL>` marked as a placeholder), `make demo`, and the E1
  cost-table row re-measured on an idle host. The timed stranger test from
  a local clone took 113.9 s in a clean container and 106.8 s in a fresh
  WSL distro, both with the apt mirror overridden, because Canonical's
  archive was unreachable from this host. Still open: the one human run
  (`docs/phase1-exit-checklist.md`), and a timed run with the image's own
  apt sources inside the budget (the post-M7 review's took 19 min 58 s
  while the archive crawled). Review notes: M7, and "Review notes: after
  M7". M8's first half done (2026-11-10): Breadboard's SAADC
  (`BbNrf52Saadc`, `nrf52840-bb@2`), the loading `adc_in`, the per-sample
  records, the UART lines and the VDD input, gated by `make
  nrf-saadc-model` (ADR-008 Amendment 4). M8 done (2026-11-10): E2 on
  dial-me-spinner, `make dms-vsense` (the battery reading at 1 s and 50 ms
  against the `load = "none"` reference, the codes, the C5 ÷ 10 fault, the
  LED shed on a battery-sag bench, every case equal to its
  every-lattice-point reference), the firmware `vsense.elf` /
  `vsense_50ms.elf` reproduced by `make fw-rebuild`, the cost-table row
  (ADR-008 Amendment 5; Review notes: M8). The post-M8 review's nine
  minor findings are fixed: the LED restore gated, grant partitions and
  `make pool-equiv` cover E2 (ADR-008 Amendment 6; "Review notes: after
  M8").
- **Decisions it depends on:** ADR-006 (component packages), ADR-007 (KiCad
  import and `board.bbm`), ADR-008 (nRF52840 target). All three are
  Accepted (2026-11-09) with the same approver decisions.
- **Replaces:** the design doc's Phase 1 definition ("worked example 1 from a
  real KiCad project": an invented STM32F103 blinky board) with the user's
  own board, **dial-me-spinner**, as the reference board (user-approved
  direction).
- **Evidence:** three research passes (board and SPICE suite, Renode 1.17.0
  nRF52840, KiCad import), measurements for this plan, two critiques, and
  re-measurements for this revision. Scratch work is in WSL under
  `~/bbscratch-p1-board`, `~/bbscratch-p1-nrf`, `~/bbscratch-p1-kicad`,
  `~/bbscratch-p1-design`, `~/bbscratch-p1-crit`, `~/bbscratch-crit-p1` and
  `~/bbscratch-p1-rev` (this revision). Neither repository was modified by
  any of the measurements.

## Approver decisions (2026-11-09)

Recorded after the human approver reviewed this plan and ADR-006, ADR-007
and ADR-008. The same three decisions are recorded in each ADR.

**A. Every recommended answer is accepted,** here (§7) and in the three
ADRs' own question lists:

1. **Numerics (Q1):** S1 as planned (a timeboxed view-local fix of the
   TPS22917 turn-off, before M5), with the datasheet-fitted view as the
   fallback.
2. **Licences (Q2):** Apache-2.0 for breadboard.dev and for first-party
   packages. `examples/dial-me-spinner/` vendors the reference `.net`,
   `.kicad_sch` and `.kicad_pro` files, and the migrated suite models.
3. **TI-derived TPS22917 view (Q3):** it stays **local-only**
   (`license = "LicenseRef-local-only"`, in a package path outside the
   repository, never committed). A datasheet-fitted TPS22917 view is the
   committed default. **M5b is therefore required**, not conditional; the
   exit estimate is the one with M5b (about 11.5 weeks).
4. **Solver options (Q5):** option sets are part of lattice
   qualification, `phase0` is the default for imported boards, and a bench
   may use another named set only where every in-scope view is qualified
   under it (ADR-006 D6).
5. **Exit scope (Q6):** E1 on one simulated board (the main board), at the
   nominal and cold benches. E2, GDB, E3 and system import follow.
6. **Design-doc deviations (Q7):** all accepted as recorded deviations.
7. **Canonical JSON and `confidence` (Q8):** Breadboard's integer-only
   canonical JSON, and `confidence` as an enum.
8. **nRF CPU speed (Q9):** `PerformanceInMips: 64`; busy-waits documented
   at about 2/3 of their requested time; rule timing only with
   `k_sleep`/RTC.
9. **Firmware location (Q10):** `examples/dial-me-spinner/firmware` in
   breadboard.dev.
10. **SAADC loading (ADR-008 Q4–Q6):** the loading `adc_in` design as
    recommended (ADR-008 approver decisions).

**B. Publication (replaces the recommended answer to Q4).** No GitHub
repository is created, nothing is pushed and no remote is added during
Phase 1's milestones. M7 runs the stranger test (§5) from a **local
clone** (a file path or `file://` URL) and stops there. The human decides
publication afterwards. The README gives the clone URL as a placeholder,
clearly marked to be filled in at publication.

**C. The reference revision changes to `6854d25`.** The reference board is
dial-me-spinner branch `kicad10-port`, commit `6854d25` ("Move the
toolchain and boards to KiCad 10.0.8, with no change to the design"; its
message says the netlists are connectivity-identical to the KiCad 9.0.9
ones). `dc96515` (KiCad 9, `main`) becomes the **secondary** import check:
it is not a gate for E1, but gate M4c still requires it to give the same
`board.bbm` if the connectivity is truly identical, and records the
differences if it is not. Every reference to `dc96515` as the reference
in this plan, the three ADRs, the M0/M4b/M4c gates and
`examples/dial-me-spinner/SOURCE` is updated, and the gate counts are
re-derived at `6854d25` (M0 below; they are unchanged: 201 parts, 142
nets, 657 pin nodes on the main board).

## 1. Goal

Phase 0 proved the seam: a hand-written netlist, one MCU pin, an RC or a
divider, byte-identical results. Phase 1 proves the **funnel** on a board
nobody wrote for Breadboard:

> Import a real KiCad project, resolve its parts to packages, load real
> firmware on the real MCU, and reproduce, in a co-simulation, a result
> that the board's own SPICE test suite established by hand: a firmware
> rule that protects the hardware.

The reference board is dial-me-spinner **rev A, pinned at commit `6854d25`**
(branch `kicad10-port`, KiCad 10.0.8; approver decision C). It descends
from `dc96515` (`main`, KiCad 9.0.9), the first pin, which stays as the
secondary import check (M4c). A pin is needed because:

- the working tree has uncommitted edits to `hardware/design/sheets/*.py`;
- branches move (`main`, `kicad10-port`, rev B work);
- rev B replaces the SSD1351 OLED.

The board has:

- an nRF52840 in a Raytac MDBT50Q-U1MV2 module;
- 201 parts and 142 nets on the main board, plus 21 parts and 17 nets on the
  base board (both re-derived at `6854d25`, M0);
- an ngspice test suite (`hardware/sim`). Its everyday subset gave 718
  passed and 44 expected failures in 3 min 24 s. Its decks already model
  each MCU output as a digital output plus glue.

That last point is exactly Breadboard's pin model, so the suite is a ready
cross-check. The suite commits no results at `6854d25` (`hardware/sim/results/`
is git-ignored), so M0 regenerates them (§4). Between `dc96515` and
`6854d25` the suite's models are unchanged; only `spicelib.py` (pin names
from `libparts`) and one line of `test_led_data.py` changed.

## 2. Worked examples (ranked)

All three come from the board's firmware rules and from suite tests. Each is
scored on two things: what Renode 1.17.0 can do without new peripheral
models, and how much new modelling it needs.

### E1 (rank 1, Phase 1's definition of done): LED rail sequencing (LSW-3, LED-DATA-V4, LSW-2)

Firmware drives LED_PWR_EN (P1.04, which turns on U2 TPS22917) and the LED
data pin P0.06, both as plain GPIO, following `docs/firmware.md` §6.4 of
dial-me-spinner:

- **On:** LED_PWR_EN high, then at least 10 ms with P0.06 low, then P0.06
  may go high.
- **Off:** P0.06 low, then LED_PWR_EN low, then at least 30 ms before the
  next turn-on.

The firmware times both waits with `k_sleep` (or the regulator's
`startup-delay-us`, which sleeps), never with `k_busy_wait`: busy-wait
loops do not run at their real speed in Renode (ADR-008 D1). A second
firmware build applies the deliberately wrong order: P0.06 is high while
VLED is off.

- **Analog side:**
  - U2 and its CT capacitor C7;
  - the VLED capacitance (C8, C9, C101–C148);
  - quick discharge through R4;
  - the data chain R40/R41/R43 into D101's input clamp;
  - the 48 LED supply loads.

  The suite has all of these models.
- **CPU side:** GPIO only. Renode's nRF52840 GPIO works through External
  Control (measured), so E1 needs **no new Renode peripheral model**. That
  is why it ranks first.
- **Power path: a bench, not the base board.** The suite drives VSYS
  through the base board's BQ25180 battery path (a behavioural BAT-FET
  model, a temperature-dependent pack resistance `RPACK`, the base-board
  VSYS capacitors and the link harness) and makes +3V3 an ideal LDO,
  `min(3.3, V(VSYS) − 44.2 mV)`. Phase 1 simulates only the main board, so
  E1 has two benches that stand in for that path with the suite's own
  constants (`test_load_switches.py` @6854d25, unchanged since `dc96515`):

  | Bench | VSYS source | Series R | Temp | +3V3 | VSYS load |
  | --- | --- | --- | --- | --- | --- |
  | `bench/nominal.toml` | 3.7 V | 0.319 Ω (RPACK 0.25 + BAT-FET 0.055 + harness 0.0142) | 25 °C | tracking supply, 3.3 V max, 44.2 mV drop | 50 mA |
  | `bench/cold.toml` | 3.0 V | 0.569 Ω (RPACK 0.50 + 0.055 + 0.0142) | −10 °C | same | 50 mA |

  The tracking supply and the DC load are the two bench additions this
  needs (ADR-007 D5). **Not reproduced:** the BQ25180 power-path dynamics,
  the base-board VSYS capacitance (omitting it should make VSYS sag more,
  which is pessimistic for VLED; M6 measures the size), and every
  tolerance, DC-bias and Monte Carlo corner (those need `pi`-rung views,
  Phase 2).
- **Checks.** The limits are the suite's limits, not values the suite
  measured:

  | Check | Limit (suite constant) | Benches |
  | --- | --- | --- |
  | LED-DATA-V4: VLED at the instant P0.06 first goes high | ≥ 2.67 V (`FW_VLED_FRAME_MIN`) | nominal, cold |
  | LSW-3: max(V(LED_DIN) − V(VLED)) over the run | ≤ 0.7 V (DI abs max) | nominal, cold |
  | LSW-2: VLED below 10 % of its on value after LED_PWR_EN falls | within 30 ms (`FW_LED_OFF`) | nominal, cold |
  | FW-6.4 (added after the M7 review): the rule's own timing from the `pin_edge` records | every P0.06 rise while LED_PWR_EN is high and ≥ 10 ms after its rise; every LED_PWR_EN rise ≥ 30 ms after the preceding fall | nominal, cold |
  | Wrong-order firmware | the LSW-3 check **fails**, and the message gives the excursion and the clamp current from the series-resistor drop | cold |

  The suite's measured values (VLED 2.9715 V at the P0.06 rise at the cold
  corner; 24 ms nominal QOD) are **reported** next to each result, never
  used as limits. The suite's 26.6 ms worst case is out of E1: it needs
  tolerance and DC-bias views.
- **Cross-check (local, M6).** `scripts/dms_suite_deck.py` builds the
  suite's own `led_rail_sequence` deck with the suite's functions, but with
  the power path replaced by the **same** bench supply (the suite's own
  `ideal_vsys` idiom plus the bench's series R and load) and the two PWL
  sources replaced by the run's `pin_edge` records. VLED from that deck and
  from the co-simulation must agree within 5 mV. The difference to the
  suite's original battery-network deck at the same corner is reported, not
  gated: it measures what the bench leaves out. The script needs the
  dial-me-spinner repository (it exports `6854d25` with `git archive`, as
  `scripts/dms_suite.sh` does), so it runs locally, not in the stranger
  demo.

### E2 (rank 2): battery reading, sample-rate error, low-battery LED shed (VSENSE, §12.4)

Firmware reads VSYS_SENSE (R1/R2 = 2.2 MΩ/1 MΩ with C5 = 100 nF) on AIN0 =
P0.02, using:

- gain 1/6;
- TACQ 40 µs;
- 12 bits with 8× oversampling (Zephyr's simple mode, which turns on BURST:
  one SAMPLE task runs 8 back-to-back acquisitions in hardware);
- VBAT_est = 0.979 × code × 2.8125 mV.

The bench ramps a battery source (open-circuit voltage plus 0.2–0.4 Ω) down
through the 3.53 V threshold. When VBAT_est drops below 3.53 V, the firmware
runs E1's off sequence ("LEDs off").

- **What is observable where.** One 12-bit LSB at the pin is
  3.6 V / 4096 = 879 µV. The suite's sample-rate effect (356 µV with 1 s
  bursts, 761 µV with 50 ms bursts, against a 439 µV half-LSB limit) is
  below one LSB and is measured on the analog hold node against an
  unloaded reference, at the suite's worst case (VSYS 3.0 V, CS pre-charged
  to VDD max). So E2 has two kinds of check:
  - **Analog-side** (not firmware-visible): the SAADC model records the
    value it sampled at each sample instant (V(hold), ADR-008 D7). A
    reference run of the same firmware with `load = "none"` records V(pad)
    unloaded. The check script compares the averaged per-sample error with
    356 µV (1 s) and 761 µV (50 ms) at the cold bench with VPRE = VDD
    (ADR-008 D7 default). The tolerance is set from M8's measurement.
  - **Firmware-visible:** codes match code = V/2.8125 mV within the model's
    stated error; the C5 ÷ 10 fault (about 3.47 mV, about 4 LSB) shows in
    the codes; LED_PWR_EN falls within one burst period after the
    threshold crossing.
- **Needs:**
  - a SAADC model (absent in Renode 1.17.0), with BURST;
  - an `adc_in` pin that **loads** the net: 2.5 pF sample capacitor plus
    3 pF pad, with one charge transfer per acquisition (ADR-008 D7).
    Without that load the sample-rate effect cannot appear.
  - UART log lines.

  All three are in ADR-008.
- **Out of E2 for Phase 1:** the SHIP write to the BQ25180 over I2C. That
  needs TWIM with EasyDMA (Renode has legacy TWI only), a BQ25180 bus model
  and the two-board system run.

### E3 (rank 3, stretch): detent homing H0–H5 (§5.4, MD-1/2/3, DRV5032)

PWM0 microstepping drives two DRV8837s and the stepper. A mechanism model
maps the microstep phase to carriage position and field, which drives the
DRV5032 HOME_N input.

- **Needs:**
  - a PWM model (absent);
  - TEMP and NVMC stand-ins (absent);
  - an averaged DRV8837/PWM analog view: switch level runs at about 1.75 s of
    wall time per ms of simulated time, so the 2.4 s homing would take about
    70 min;
  - a pin-level device for the mechanism and field.
- **It is the stretch** because it needs four new models before it shows
  anything.

**Not in Phase 1:**

- S4 (OLED start-up on a dead battery: brown-out reboot loop);
- S5 (battery sag, POFWARN, SHIP).

Both need a POWER/reset model driven by an analog rail, the BQ25180 I2C
model and the two-board system. They belong in Phase 2's `pi` fidelity.

## 3. Scope

**In (the exit, M0–M7):**

- **KiCad import (ADR-007):**
  - `kicad-cli sch export netlist` on a temporary copy, then a native
    S-expression parse;
  - KiCad 9 and 10;
  - part resolution (exact MPN, or a package's declared patterns);
  - deterministic, content-only `board.bbm`;
  - one simulated board per project, with qualified names (`main:U2`)
    specified now so that systems are additive later;
  - `bb open`, `bb flash`, `bb run`, `bb probe`.
- **Packages (ADR-006):**
  - `part.toml` v0 and `pins.toml`, with JSON Schemas and a versioning
    policy in `spec/`;
  - local package directories and `bb.lock`, which also pins the engine
    platform identity;
  - generic R/C/L packages;
  - packages for the two existing STM32 platforms;
  - `nordic/nrf52840-qiaa`, `raytac/mdbt50q-u1mv2`;
  - the E1 parts (TPS22917, WS2812C), migrated from `hardware/sim/models`
    under their provenance;
  - `bb pkg lint` and `bb pkg test` (lattice qualification through the real
    `NgspiceEngine`, per solver option set).
- **nRF52840 target (ADR-008):**
  - an offline platform overlay at 64 MIPS;
  - nRF pin names;
  - the engine overflow fix;
  - bare-metal gate firmware;
  - a Zephyr app for E1 (prebuilt, pinned, banner-free).
- **Fidelity:** `--fidelity fast` only, with a per-instance override of the
  analog view and the ladder fallback rule (ADR-006 D3).
- **A minimal bench (`bench.toml`):**
  - supplies on nets (with source resistance), including one tracking
    (LDO-like) supply;
  - DC loads;
  - temperature;
  - the analog scope;
  - PWL sources;
  - solver options (which must be a qualified set).
- **Setup for strangers:** `make setup SETUP=demo` installs `bb` on PATH,
  skips the ARM toolchain, and runs as root without sudo (M3).

**After the exit (Phase 1, M8–M11):** E2 (SAADC, loading `adc_in`, UART
log), GDB passthrough, the E3 stretch, and two-board system import (the base
board and `[[connection]]`, M11).

**Out (each named, so it is a decision rather than a gap):**

| Item | Why / where |
| --- | --- |
| Public registry, signing, `bb pkg publish` | Phase 2 (design doc). Packages are local directories plus a lockfile |
| PCB geometry, parasitics, gerbers | Nothing at `fast` consumes geometry. Phase 1 may parse `.kicad_pcb` only as an optional cross-check (ADR-007 D8) |
| `pi`/`si`/`em` rungs, rail-tracking pin outputs, brown-out reset from a rail | Phase 2 (S4, S5). Views may already declare `pi` |
| Python device API, the design doc's "5 example sensor packages" | Bus devices: Renode's nRF TWI drives no pins and has no External Control route; SPI has one, but its synchronous callbacks need a time-grant analysis (ADR-006 D7). Phase 2, with TWIM/SPIM models |
| BLE/RADIO, USBD, NFCT, QSPI, TWIM, SPIM3, I2S-to-pin, PWM (unless E3) | Absent or not faithful in Renode 1.17.0. Each is its own model and gate (ADR-008 D9 lists them) |
| RP2040 package | The design doc lists it. The Renode 1.17.0 RP2040 platform was not evaluated, and nothing on this board needs it. Deferred |
| `bench.yaml` with CSV playback, `bb test`, JUnit | Phase 2 (design doc). Phase 1 checks are scripts over the outputs, as in Phase 0 |
| Multi-MCU, multi-board **simulation** | Phase 3 (design doc). M11 imports the main/base system; only one board is ever simulated |
| Firmware images other than ELF | The schema takes a list of typed images now (ADR-007 D5); Phase 1 implements `elf` only |

## 4. Milestones (ordered; each is gated and committed after its gates)

Every milestone keeps `make test` green. Every new board or run mode gets
two things before it is on by default:

- an equivalence gate against its one-cell reference;
- a `make cost-table` row measured with the ADR-003 protocol.

These are AGENTS.md rules and they hold here unchanged.

| # | Milestone | Gate (must pass) | ADR |
| --- | --- | --- | --- |
| M0 | Approvals and inputs: ADR-006/007/008 accepted. Licence and publication decisions (§7, approver decisions A and B). Reference commit `6854d25` recorded (decision C; `dc96515` as the secondary import check). The reference KiCad inputs vendored. **The suite re-run at `6854d25`** from a `git archive` export, with pinned ngspice 42, its `results.json` committed under `examples/dial-me-spinner/suite/` with the ngspice version and the numeric fallback level each deck used | Approver sign-off. `examples/dial-me-spinner/SOURCE` names the commit and the sha256 of each imported file (`scripts/dms_source_check.sh`). The committed `results.json` reproduces on a second run (`make dms-suite-check`) | all |
| M1 | Engine fix: clamp the lookahead promise before `Time::from_ns` (`engines/cpu-renode/src/lib.rs:1509` panics past about 18 446 s) | A new unit test with a promise of `u64::MAX` ns returns a clamped grant. `make test`, `make lookahead-equiv` unchanged | 008 D6 |
| S1 | **Numerics spike (blocks M5, timeboxed to 1 week):** make the E1 deck pass the LED_PWR_EN fall and the QOD discharge through `NgspiceEngine` (lattice 10 µs, edges as digital glue) at Phase 0's option set, by a view-local fix of the TPS22917 turn-off and QOD path (§6 R1). The harness in `~/bbscratch-p1-rev/eng` is the starting point. If no view-local fix works in the timebox, take the R1 fallback | The E1 deck, through the engine, runs 120 ms at both benches at Phase 0's options with VLED within 1 mV of the suite's fallback-option result. A measured cost table (wall time per bench and option set) replaces §8's estimate. **Done (2026-11-09):** `make dms-s1` (18 gates), cost in §8; the gate as applied is in ADR-006 Amendment 2 item 4 | 006 D6 |
| M2 | nRF52840 platform `Nrf52840Bb` (offline overlay, `PerformanceInMips: 64`, host-port block). `PinSpec::nrf`. Bare-metal gate firmware (blinky, feedback, ppi, busy-wait) in `engines/cpu-renode/tests/firmware/nrf52840/` | `make nrf-gates`: blinky edge list identical at one-cell, 10 ms, plugin armed and lookahead on. The feedback board identical to its `--lookahead off` reference with no broken promise. Lookahead self-test status 0x3. `nrf-busywait` records the virtual time of a 10 ms nrfx busy-wait (expected about 6.67 ms, ADR-008 D1). `make pool-equiv` with the nRF configurations added | 008 D1–D4 |
| M3 | Package format v0. `spec/package.md` plus schemas, the value grammar and the schema versioning policy. Generic R/C/L. `st/stm32f103c8` and `st/stm32f072rb` packages. Platform identities. `bb pkg lint/test`. `bb.lock`. **Setup:** `make setup` installs `bb` into `~/.local/bin`, `SETUP=demo` skips the ARM toolchain, root runs without sudo | `examples/spike`, `feedback` and `adc` expressed through packages give the committed hashes (`tests/golden/*/outputs.sha256`) byte for byte. Schema validation of every `part.toml`/`pins.toml` in CI. `make setup SETUP=demo` timed in a clean `ubuntu:24.04` container (R7) | 006 |
| M4 | Importer (`importers/kicad`), `board.bbm`, `bb.toml`, name resolution, `bb open`/`flash`/`run`/`probe`. Existing `board.cir` files become a second front end of the same elaborated board | (a) `examples/adc` drawn in KiCad (new, in the repo), imported, gives `nets.csv`, `nets.vcd` and the non-grant records byte-identical to `board.cir`. (b) dial-me-spinner main at `6854d25` (the vendored netlist): 201 parts, 142 nets, 657 pin nodes (re-derived at M0); plus a small synthetic two-board fixture in which a shared bare ref (`U1`) is an error that lists `a:U1`, `b:U1`. (c) The committed KiCad 10.0.8 netlist at `6854d25`, fresh KiCad 10.0.8 exports (Linux and Windows `kicad-cli`) and the `.kicad_sch` route give a byte-identical `board.bbm`. Secondary (not a gate for E1): `dc96515`'s KiCad 9.0.9 netlist must give the same `board.bbm`; M0 measured the content the importer reads (ADR-007 D3) identical on the main board, so any difference is a bug to fix or, if it is a real connectivity difference, recorded. (d) Two imports give a byte-identical `board.bbm`. (e) A schematic newer than the supported versions, and a `.net` from an unsupported Eeschema major, are refused | 007 |
| M5 | E1 packages (`ti/tps22917`, `worldsemi/ws2812c-2020`, `nordic/nrf52840-qiaa` GPIO views, `raytac/mdbt50q-u1mv2`), migrated from `hardware/sim/models` with their provenance and S1's fix. Each view is **lattice-qualified** at Phase 0's option set | `bb pkg test` per package through `NgspiceEngine`: its vectors reproduce M0's committed `results.json` values within the stated tolerance | 006 D4–D6 |
| M5b | **Required** (approver decision A: Q3 keeps the TI-derived view local-only): a datasheet-fitted TPS22917 view, qualified, gated against the vendor-derived view locally, becomes the committed default | As M5, plus agreement with the vendor-derived view on the E1 checks within 10 mV / 1 ms | 006 D9 |
| M6 | E1 run: `examples/dial-me-spinner/` (`bb.toml`, `bench/nominal.toml`, `bench/cold.toml`, firmware sources and prebuilt ELFs). Check script `scripts/check_dms_rail.py` | The correct-order firmware passes every E1 check at both benches. The wrong-order firmware fails exactly the LSW-3 check (the script expects it to fail). **Board-level convergence:** the whole E1 deck completes 120 ms at each bench's option set, at both engine orders, in under 30 s. One-cell reference equivalence. 10-run determinism. Goldens in `tests/golden/dms-rail/`. Local cross-check: PWL replay into the suite-derived bench deck within 5 mV on VLED | 006, 007, 008 |
| M7 | Definition of done, packaged: README quickstart (clone URL as a marked placeholder), `make demo`, cost-table row. **Not published** (approver decision B): the stranger test runs from a local clone and stops; the human decides publication | §5, measured in a clean Ubuntu 24.04 container and on a clean WSL distro, cloning from a local path or `file://` URL | — |
| M8 | E2: `BbNrf52Saadc` model with BURST, loading `adc_in`, per-sample trace, UART-to-`log` records, VDD channel | `make nrf-saadc-model` (register and timing tests, the ADR-005 test-plan shape, including an 8× burst with one charge transfer per acquisition). `make dms-vsense`: the analog-side check against the `load = "none"` reference run (356 µV at 1 s and 761 µV at 50 ms, tolerance set from M8's measurement), the firmware-visible checks (codes, C5 ÷ 10 fault, LED shed timing). Equivalence with the every-lattice-point reference; no `stop:ngspice` | 008 D7–D8 |
| M9 | GDB passthrough (`bb run --gdb <port>`) | Prototype first (ADR-008 D10). Then a run with GDB attached, one breakpoint hit and continued, gives `nets.csv`, `nets.vcd` and non-grant records identical to the run without GDB. If that cannot hold, the mode is marked debug-only and its outputs carry a `debug` flag | 008 D10 |
| M10 | Stretch E3: PWM model, TEMP and NVMC stand-ins, the averaged DRV8837 view, the Rust device API, the mechanism and DRV5032 device | One full step at 446 PPS against `test_firmware_microstep_pwm`. HOME_N latency within 75 ms. Equivalence and determinism | 006 D7, 008 D9 |
| M11 | Systems: base board import, `[[connection]]`, the harness view | Base board: 21 parts, 17 nets. The mated system with the shared refs U1, U2, J1 and TP1 all resolved by qualified name; a bare shared ref is an error | 007 D5 |

Dependencies:

- M1 → M2 → M6.
- S1 → M5 (S1 can run in parallel with M1–M4).
- M3 → M4 → M5 → M5b → M6 → M7 (M5b required by decision A).
- M8 needs M6.
- M9 needs M2.
- M10 needs M6 and M8's model style.
- M11 needs M4.

## 5. Definition of done (the Phase 1 exit)

The design doc's exit is "a stranger can clone, install and run the
reference demo from the README in under 15 minutes". For this board:

1. On a clean Ubuntu 24.04 (container or fresh WSL distro), with no KiCad,
   no Zephyr SDK and no dial-me-spinner checkout, a person who has not
   worked on Breadboard follows the README and, **in under 15 minutes of
   wall time including `make setup`**, runs:

   ```sh
   git clone https://github.com/tyevco/breadboard.dev breadboard.dev && cd breadboard.dev   # published 2026-11-10; M7 used a local path
   make setup SETUP=demo                     # Renode, Rust, ngspice; installs bb into ~/.local/bin
   cd examples/dial-me-spinner
   bb open                                   # imports the pinned netlist, resolves parts, writes board.bbm
   bb flash --target U10 firmware/prebuilt/led_rail.elf
   bb run --for 120ms --bench bench/cold.toml --probe net:VLED --probe net:LED_DIN --out out/rail
   python3 ../../scripts/check_dms_rail.py out/rail      # PASS: LED-DATA-V4, LSW-3, LSW-2, FW-6.4
   ```

   `net:LED_DIN` resolves to `/LED ring/LED_DIN` as the unique leaf name
   (ADR-007 D7). They then repeat the run with
   `firmware/prebuilt/led_rail_wrong_order.elf` and see the LSW-3 check
   **fail**, with the DIN-over-VLED excursion and the clamp current in the
   message.
2. The same outputs are byte-identical across 10 runs and against the
   committed hashes, and `make test` (now including E1) is green.
3. `bb open` on the KiCad sources (with KiCad 10 installed) gives the
   **same** `board.bbm` as the committed netlist, and so does
   dial-me-spinner's KiCad 9 netlist (`dc96515`) (gate M4c; no gate runs
   KiCad 9's own `kicad-cli`, ADR-007 Amendment 4 item 3). Source paths,
   hashes and schematic versions live in `.bb/import.json`, not in
   `board.bbm`, so the identity holds by construction. A user with KiCad can
   therefore start from the schematic, and the stranger does not need
   KiCad.
4. Every analog view used in the run is listed with its provenance,
   confidence, qualification and the solver option set in
   `out/rail/run.json` (ADR-007 D6). A view whose provenance is `estimated`
   or `agent_generated` is flagged in `bb run`'s summary.

M7 measures it: a timed script in a clean container plus one human run,
both cloning from a local path or `file://` URL (approver decision B).
M3 measures `make setup` first, so a miss is known early (R7). The script
is `scripts/stranger_test.sh` (`make stranger-test-container`, `make
stranger-test-wsl`). It runs the README's own command block, line by line.
The human run follows `docs/phase1-exit-checklist.md`. The results are in
"Review notes: M7".

## 6. Risks

| # | Risk | Evidence | Mitigation |
| --- | --- | --- | --- |
| R1 | **The E1 deck does not get past the LED_PWR_EN fall through the engine, at any option set measured.** Breadboard's ngspice engine needs TMAX to divide the 10 µs lattice and clamps every step to the lattice (`engines/analog-ngspice/src/lib.rs`, `NgspiceConfig::lattice`). The options come from the deck (Phase 0: `.options RELTOL=1e-5 VNTOL=1e-8 ABSTOL=1e-13` in every `examples/*/board.cir`) | **Standalone** (ngspice 42 batch, the suite's `led_rail_sequence` cold deck, TMAX 10 µs, 7 runs in parallel at load 11, 120 s cap; `~/bbscratch-p1-rev`): Phase 0 options killed; Phase 0 + `rshunt`, + Gear and `rshunt`, and reltol 1e-4 + Gear + `rshunt` killed; Phase 0 + Gear 112.8 s; reltol 1e-4 85.4 s; the suite's fallback (reltol 1e-3, Gear, `rshunt` 1e12) 4.4 s. Every run that finished gives the same VLED (2.971511 V at 10.5 ms, 0.104326 V at 50.6 ms). **Through `NgspiceEngine`** (lattice 10 µs, edges as 100 ns digital glue, grants 10 µs–1 ms, 120–240 s caps): Phase 0 reaches the LED_PWR_EN fall (15.6 ms) in 1.5–3.3 s with the same VLED (2.971511 V at 10.5 ms) and stalls there; reltol 1e-4 and the suite fallback pass the fall and stall at 27.92 ms during QOD (VLED 0.914 V); Phase 0 + Gear stops at the LED_PWR_EN rise. The stall points do not depend on the grant size. The QOD deck at reltol 1e-5 also fails, at 0.1 ms (critique) | **S1**, timeboxed and before M5: a view-local fix of the TPS22917 turn-off path (the suite already replaced two numerically fragile vendor blocks), qualified through the engine. Option sets are part of qualification (ADR-006 D6), so a looser set is allowed only where it is shown not to change results. **Fallback** if no view-local fix works: a datasheet-fitted behavioural TPS22917 view (M5b), which also answers R2. The exit date moves by M5b's 1.5 weeks. **Resolved by S1 (2026-11-09):** the stalls are Newton failures on branch-current unknowns in the view's pass path under `ABSTOL=1e-13`. Restating TI's `RVAR` and `H1` without them runs E1 at `phase0` in about 4 s and changes no result by more than 0.07 mV (ADR-006 Amendment 2) |
| R2 | **Licences.** Neither repo has a LICENSE file. The TPS22917 and TPS61040 views derive from TI models (SLVMCE8, SLVMDA8) whose redistribution terms were not checked | `git ls-tree dc96515` and `6854d25` (root): no LICENSE; same for breadboard.dev | Decided (approver decision A): Apache-2.0; the TI-derived view stays local-only and M5b's datasheet-fitted view is the committed default. The cross-check then compares different models of U2, so its 5 mV gate applies to the local vendor-derived run only |
| R3 | **Reference board churn.** Uncommitted edits in the dial-me-spinner working tree, a `kicad10-port` branch, rev B | `git status` in dial-me-spinner; `6854d25` ("Move the toolchain and boards to KiCad 10.0.8, with no change to the design") on `kicad10-port` descends from `dc96515` and commits a `.net` from Eeschema 10.0.8 | Pin `6854d25` (decision C). Import from `git archive` of that commit, never from the working tree. The netlists and schematics are vendored (decision A). M4c also imports `dc96515`'s KiCad 9.0.9 netlist, which must give the same `board.bbm` (a free check of "no change to the design"; M0 found the importer's content identical on the main board) |
| R4 | **Renode nRF gaps grow every scenario.** SAADC, PWM, TWIM, SPIM3, QSPI, NVMC, TEMP, POWER are absent. TIMER SHORTS and BITMODE are missing. PPI tasks are deferred to the lattice | Renode research, `nrf52840.repl` | E1 needs none of them. E2 adds exactly one model (SAADC). Each further model is its own ADR-005-style step |
| R5 | **Static pin direction and no high-Z.** Renode's nRF GPIO keeps driving after a switch to input, and drive strength is ignored | Renode research §1 | Bindings in `bb.toml` fix direction and drive per pin. E1 uses fixed outputs. An end-of-run `PIN_CNF` audit of every pin on an analog-scope net warns when firmware's configuration differs from the bound model, or when an unbound pin is an output (ADR-008 D4) |
| R6 | **Firmware toolchain weight and image identity.** The Zephyr workspace plus SDK is about 2.0 GB; NCS is not installed; the workspace is v4.5.0-rc3. The boot banner embeds `git describe` of the Zephyr tree | Renode research §3; `strings zephyr.bin` shows `Booting Zephyr OS build 76280bce2a05` | Commit prebuilt ELFs; a rebuild job checks the **loadable image** hash. The example pins `BUILD_VERSION` and turns the boot banner off, and the rebuild gate uses a second, fresh workspace (ADR-008 D5) |
| R7 | **15 minutes not met.** The bb build is cheap (11.5 s clean release build, 24 cores); downloads dominate: Rust 1.99.1 about 617 MB, Renode portable 184 MB, the ARM toolchain about 930 MB installed. Today `make setup` uses sudo (absent in `ubuntu:24.04`), does not install `bb`, and always installs the ARM toolchain. breadboard.dev has no public remote | `scripts/setup.sh`; `git remote -v` empty; the GitHub URL returns 404 | M3: `SETUP=demo`, `bb` into `~/.local/bin`, a root path without sudo, and a timed clean-container run. M7: the timed run from a local clone; publication is the human's decision afterwards (approver decision B) |
| R8 | **KiCad format drift.** KiCad 11 (schematic 20261106) is already on the dev branch. Windows `kicad-cli` writes CRLF and orders some pins differently. `kicad-cli` writes `.kicad_prl` into the project | KiCad research | Version gates on the schematic and on the `.net`'s Eeschema major. Canonicalise before hashing. Always run on a temporary copy |
| R9 | **Whole-board decks are slow or fail to converge.** 201 parts, switching converters, motor drivers | Microstep deck: 1.75 s of wall time per simulated ms; R1 | The analog **scope** (ADR-007 D5) puts only the parts the scenario needs into ngspice. M6 adds a board-level convergence gate, because view-level qualification says nothing about the assembled deck |
| R10 | **Model disagreement inside one part.** The nRF GPIO output resistance is 200 Ω in one suite file and 800 Ω in another; BSS138 Vth is 1.2 V against 1.5 V | Board research finding 7 | Packages keep several views with their own provenance. `bb pkg lint` reports conflicts and never averages them |
| R11 | **Virtual CPU speed.** Renode's default is 100 MIPS; nrfx busy-waits count instructions as cycles | ADR-008 Context 3 | 64 MIPS in the overlay; rule timing with `k_sleep`/RTC only; the `nrf-busywait` gate documents the residual error |
| R12 | **Schema churn.** Five new schemas are public API for agents | — | A versioning policy (ADR-006 D11) and reserved Phase 2 keys, before M3 freezes v0 |

## 7. Questions for the approver (plan level)

The ten most important questions across the plan and the three ADRs, most
important first. Each ADR's own list has the rest. **All are answered**
(see Approver decisions at the top): every recommendation is accepted
except Q4, which decision B replaces, and the reference commit in Q2 is
now `6854d25` (decision C).

1. **Numerics (R1):** accept S1 (a timeboxed view-local fix of the TPS22917
   turn-off, before M5) with the datasheet-fitted view (M5b) as the
   fallback, and the exit date moving by 1.5 weeks if the fallback is
   taken? **Recommended: yes.**
2. **Licences:** Apache-2.0 for breadboard.dev and for first-party
   packages, and may `examples/dial-me-spinner/` vendor the `.net`,
   `.kicad_sch`/`.kicad_pro` files at the reference commit (`6854d25`
   since decision C) and the migrated suite models under it? **Recommended: yes to both** (otherwise the demo needs
   the dial-me-spinner repo to be public too).
3. **TI-derived TPS22917 view:** commit it, or keep it local and commit a
   datasheet-fitted default (M5b)? **Recommended: keep it local
   (`LicenseRef-local-only`) until TI's terms are read, and do M5b.** It
   also covers R1's fallback.
4. **Publication:** where and when is breadboard.dev published, so that the
   exit's `git clone` works? **Recommended: public GitHub
   `tyevco/breadboard.dev` at M7, after Q2.** (Not adopted: decision B,
   a local-clone stranger test, publication decided by the human
   afterwards.)
5. **Solver options:** options are part of lattice qualification; the
   default for imported boards is Phase 0's set; a bench may use another set
   only if every in-scope view is qualified under it, which requires its
   vectors to agree with the default set's results within tolerance
   (ADR-006 D6). **Recommended: yes.**
6. **Exit scope:** E1 alone (one simulated board, nominal and cold benches)
   is the exit; E2, GDB, E3 and system import (M11) follow. **Recommended:
   yes.** E2 in the exit would add about 3 weeks.
7. **Deviations from the design doc:** the `part.toml` restructure
   (top-level `[behavioral]`, `[[analog]]` views, reserved `idd`/`rails`/
   `ibis`); `board.bbm` split from `run.json`; no RP2040; a Rust-only device
   API; PCB parse optional; systems after the exit. **Recommended:
   accept all.**
8. **Canonical JSON and `confidence`:** Breadboard's own canonical JSON
   (sorted keys, arbitrary-precision integer literals, not RFC 8785), and
   `confidence` as an enum (`unchecked`, `self_checked`,
   `datasheet_checked`, `measured`). **Recommended: yes** (ADR-006 Q2,
   ADR-007 Q1).
9. **nRF CPU speed:** `PerformanceInMips: 64`, busy-waits documented as
   running at about 2/3 of their requested time, rule timing only with
   `k_sleep`/RTC. **Recommended: yes** (ADR-008 Q1).
10. **Firmware location:** E1/E2 firmware in
    `breadboard.dev/examples/dial-me-spinner/firmware` (dial-me-spinner
    stays untouched). **Recommended: yes** (ADR-008 Q2).

## 8. Cost and time estimate

Assumptions: one lead (the approver) plus coding agents, as in Phase 0. The
estimates are in agent-working weeks and are calibrated against Phase 0. For
example, ADR-005 (one peripheral model with gates) took about 2 days of
agent work. ADR-003 took about 2 days for 10 gated steps.

| Milestone | Estimate | Note |
| --- | --- | --- |
| M0 | 2–3 days of approver time, plus 1 day | Licences, publication, ADR review; the suite re-run |
| M1 | 0.5 day | One clamp plus a test |
| S1 | up to 1 week | Timeboxed; parallel with M1–M4 if a second agent is free |
| M2 | 1 week | Overlay, pins, 4 gate firmwares, pool-equiv |
| M3 | 2 weeks | Schema, grammar, versioning policy, lint/test through the engine, 5 packages, the three Phase 0 examples through packages, setup changes and the clean-container timing |
| M4 | 1.5 weeks | S-expression parser, resolver, name resolution, `board.bbm`, four CLI verbs, KiCad drawing of `examples/adc`. The base board moved to M11 |
| M5 | 1.5 weeks | S1 found the fix (ADR-006 Amendment 2) |
| M5b | +1.5 weeks, required | Decision A (Q3: the TI-derived view stays local) |
| M6 | 2 weeks | Zephyr board definition, two firmware builds, two benches, checks, goldens, suite-derived cross-check deck, convergence gate |
| M7 | 0.5 week | Clean-container timing from a local clone, README; no publication (decision B) |
| **Exit (M0–M7)** | **about 11.5 weeks (with M5b)** | The design doc allows months 2–4 (about 12 weeks). S1 needed no fallback, so M5b stays at its 1.5 weeks |
| M8 | 3 weeks | SAADC model with BURST (ADR-005 scale), pin-model change, reference runs, UART log |
| M9 | 1 week | Prototype-dependent |
| M10 | 3–4 weeks | Four models |
| M11 | 1 week | Base board, connections, harness view |

**Run cost of the demo (measured by S1).** `make dms-s1-cost` measured the
E1 deck: the suite's sequence deck with each bench's power path, two LED
rail on/off cycles, 120 ms. It ran through `NgspiceEngine` with
`deck_run`, lattice 10 µs, and the control edges as digital glue. The
protocol is ADR-003's: 5 interleaved rounds, medians. The host was **not
idle**: load median 7–9 from other agents' work, so the numbers are upper
bounds. Each row's minimum to maximum range is in
`out/dms-s1/cost.tsv`.

| Bench | View, option set | Grants | Solver points | Wall time (median) |
| --- | --- | --- | --- | --- |
| nominal | S1-fixed, `phase0` | 10 ms (12 grants) | 19 689 | 4.06 s |
| nominal | S1-fixed, `phase0` | 1 ms | 19 689 | 4.17 s |
| nominal | S1-fixed, `phase0` | 10 µs (one cell, 12 000 grants) | 19 689 | 4.50 s |
| nominal | S1-fixed, `suite_fallback` | 10 ms | 15 386 | 2.91 s |
| nominal | suite's view, `phase0` or `suite_fallback` | 10 ms | — | does not finish 120 ms in 120 s (R1) |
| cold | S1-fixed, `phase0` | 10 ms | 19 967 | 4.07 s |
| cold | S1-fixed, `phase0` | 1 ms | 19 967 | 4.21 s |
| cold | S1-fixed, `phase0` | 10 µs (one cell) | 19 967 | 6.20 s |
| cold | S1-fixed, `suite_fallback` | 10 ms | 15 438 | 2.95 s |
| cold | suite's view, `phase0` or `suite_fallback` | 10 ms | — | does not finish 120 ms in 120 s (R1) |

For scale, standalone `ngspice -b` on the same deck took 2.64 s / 2.76 s
(fixed view, `phase0`, `tran 20u`) and 1.90 s / 1.75 s (the suite's view
and options as the suite runs it), nominal / cold. All partitions give the
same samples (`make dms-s1`).

- Engine startup: about 1.5–1.75 s fresh for the nRF52840 (measured, at
  100 MIPS; M2 re-measures at 64), or about 0.1–0.4 s on a warm pool.
- ngspice: about 4.1 s for E1's 120 ms at `phase0` and 10 ms grants
  (above). The solver work dominates: the 12 000 one-cell handoffs add
  0.4–2 s.
- Grants: E1 is acyclic (no MCU input is bound), so 12 grants of 10 ms at
  about 7 ms each on the Renode side.

So a demo run is about 4–6 s of wall time, under the 10 s target. The
15-minute budget still goes almost entirely to `make setup`.

**CI:** `make test` grows by about 1–2 min:

- M2 gates: about 20 s;
- M3 equivalence reruns: about 40 s on the pool;
- E1 runs at two benches and determinism: about 60 s (S1 measured about
  4 s of ngspice per 120 ms run).

There is no new paid infrastructure. CI stays local-only, as now.

## Review notes (2026-11-09)

Two critiques of the first draft were resolved in this revision and in
ADR-006/007/008. Items not adopted as proposed:

- **"Replay into a suite-derived deck, or widen the 5 mV."** The first
  option is adopted (the suite-derived deck uses the same bench supply). The
  5 mV is kept for that comparison; the comparison against the suite's
  original battery network is reported only, not gated, because the two
  circuits differ by design.
- **"Take the base board and `[[connection]]` out of the exit."** Adopted
  (M11). The qualified-name rules are still specified now (ADR-007 D7), so
  M11 adds no schema change.
- **The 26.6 ms QOD worst case** is dropped from E1 rather than
  re-scoped: it depends on tolerance and DC-bias views that are `pi` work.

## Review notes: M0 (2026-11-09)

- **Inputs.** `examples/dial-me-spinner/kicad/` holds the main and base
  projects at `6854d25` (netlists, `.kicad_pro`, every sheet,
  `sym-lib-table`, the project symbol library) and `kicad9-dc96515/` the two
  KiCad 9.0.9 netlists of the secondary check. `SOURCE` gives both commit
  ids and each file's sha256 and `commit:path`; `make dms-source` checks
  them, and against `git show` when the repository is present. A fresh
  `kicad-cli` 10.0.8 export of the vendored schematics matches the vendored
  netlists on every component, net, node and `libpart`, so the set is
  complete for the `kicad=` route.
- **Suite.** `make dms-suite-check` re-runs the suite at `6854d25` from a
  `git archive` export (no KiCad needed; ngspice 42+ds-3build1) and requires
  the committed `examples/dial-me-spinner/suite/results.json` byte for
  byte. 718 passed, 44 xfailed, 4 465 checks, 2 318 decks: 2 314 at
  fallback level 0, 4 at level 1 (all in `test_load_switches.py`, including
  `test_u2_qod_discharge[usb5.0]`), none at level 2, no timeouts. Three runs
  (12, 12 and 5 jobs) gave the same bytes.
- **What "the suite" means here.** The committed run is the everyday subset,
  `-m "not slow"`, the one the reference commit gates on and the one §1
  cites. The tests marked `slow` (Monte Carlo and sweeps) are not in `results.json`;
  `scripts/dms_suite.sh --select ''` runs them if a package vector ever
  needs one.
- **Reproducibility limit.** A retry after a hang depends on wall time (90 s
  per attempt in `test_load_switches.py`), so a heavily loaded host could
  move a deck to a higher fallback level; `summary.decks_with_timeouts`
  shows it. None occurred in M0's runs.

## Review notes: M1 (2026-11-09)

- **The fix.** `quiet_until` in the Renode engine clamps a promise to
  `MAX_PROMISE_NS` (18 446 744 073 709 ns, the last whole nanosecond
  `Time` holds) before converting it, instead of panicking in
  `Time::from_ns` (ADR-008 D6, Amendment 1).
- **Gate.** The new Renode test drives the real engine with a promise of
  `u64::MAX` ns (`Fault::StretchPromise { ns: u64::MAX }`). It panicked
  with "Time overflow" before the fix. It now gets the clamped promise,
  the grant that relies on it is capped at 10 ms, and the audit catches
  the stretch. A unit test covers the clamp and a `bbcore` test covers a
  `Time::MAX` promise in the scheduler. `make test` (pool and `POOL=0`) and
  `make lookahead-equiv` (pool and `POOL=0`) are green and unchanged. A
  build before the fix and one after it give byte-identical `events.jsonl`,
  grant records included, on the feedback and ADC boards.
- **Deviations (recorded in ADR-008 Amendment 1).** The engine does not
  know `t_end`, so the clamp is to the type's limit only; the kernel
  already takes the minimum with `t_end` and `--max-quantum`. D6's nRF
  gate firmware (a TIMER compare followed by a long sleep) moves to M2's
  `make nrf-gates`, because it needs M2's platform.

## Review notes: S1 (2026-11-09)

- **Result.** S1 is done inside its timebox, and the R1 fallback was not
  needed. The suite's TPS22917 view stalls through the engine because of a
  Newton convergence problem, not because of the step size. Under
  `ABSTOL=1e-13`, TI's `RVAR` and `H1` blocks put branch-current unknowns
  in series with milliohm paths. Restating those two blocks without
  branch unknowns, with the same I-V, is the whole fix (ADR-006
  Amendment 2). `make dms-s1` passes its 18 gates:
  - 120 ms at `phase0` on both benches and on the suite's two E1 corners;
  - grants of 10 µs, 1 ms and 10 ms give byte-identical samples;
  - VLED at the E1 check instants within 0.0004 mV of the suite's
    `suite_fallback` run, and the LSW-2 time within 0.03 µs of it;
  - VLED at every 10 µs within 0.59 mV of the converged run;
  - the fix changes the converged result by at most 0.07 mV;
  - the suite corners match M0's `results.json`.
- **Gate as applied (an amendment, not a silent change).** "VLED within
  1 mV of the suite's fallback-option result" is gated at the instants the
  suite measures, against the suite's run, and over the whole waveform,
  against the converged run of the suite's view at its fallback options.
  The reason: the suite's `suite_fallback` run moves by up to 2.4 mV on the
  turn-on ramp's knee under a 1-ulp change of its edge times, and lies
  1.3–5.5 mV from the converged solution there (ADR-006 Amendment 2
  item 4).
- **The E1 deck used** is the suite's own `led_rail_sequence` deck, built
  by its functions at `6854d25`, with each bench's power path. It runs two
  on/off cycles to 120 ms, so the second turn-on and its P0.06 rise are
  covered (the unfixed view also stalled there).
- **Local-only.** The fixed view and every deck are TI-derived. They are
  generated in `~/.cache/bb-dms-s1` from a `git archive` export, and the
  repository holds only the generator (`scripts/dms_s1.py`, which carries
  the two replacement lines), the runner (`scripts/dms_s1.sh`) and the
  engine example `engines/analog-ngspice/examples/deck_run.rs`. Fixed view
  sha256 `061285e8bb854529fc2c77ec122bcea58e49eaecaf9e9c30f5881b5d2fa4a4ef`;
  M5's local `ti/tps22917` package takes it from there.
- **For M5 and M5b.** The view's own suite vectors (U2 QOD at three
  corners, Table 7.6 at 3.6 V and 5 V) also run at `phase0` with the fix
  and reproduce `results.json` within 4 ns, so M5's qualification at
  (10 µs, `phase0`) should hold. M5b stays required (decision A3) with its
  gate unchanged. Its comparison with the vendor-derived view can now run
  both views at `phase0` through the engine, locally.
- **Cost.** §8's estimate is replaced by the measured table. The host was
  loaded (load 7–9) during the measurement, so the medians are upper
  bounds. A re-measurement on an idle host (`make dms-s1-cost`) can only
  lower them.

## Review notes: M3 (2026-11-09)

- **What exists.** `spec/package.md` (normative) and four JSON Schemas in
  `spec/schema/` (`part`, `pins`, `lock`, `pkgtest`); the `bbpkg` crate (the
  value grammar and integer units, a TOML reader, `part.toml`/`pins.toml`,
  lint, view files and deck assembly, test vectors through `NgspiceEngine`,
  tree hashes and `bb.lock`, schema versions); `bb pkg lint|test|migrate|
  tree|value`; `bb open <board.cir>` (writes `bb.lock`); five first-party
  packages (`generic/resistor`, `generic/capacitor`, `generic/inductor`,
  `st/stm32f103c8`, `st/stm32f072rb`); the platform identities
  `stm32f103-offline@1` and `stm32f072-bb@1` (`cli/src/platforms.rs`).
  ADR-006 Amendment 1 records what the implementation settled.
- **Gate: the Phase 0 examples through packages.** `examples/spike`,
  `feedback`, `feedback-open` and `adc` (both boards) now name their MCU as
  `*bb part U1 package=st/...` and take every pin parameter from the
  package's `pins.toml`, with a `bb.lock` beside them. With no golden
  update, `make test` (on the pool and with `POOL=0`), `make partition`,
  `feedback-open`, `lookahead-inert`, `lookahead-equiv`, `adc-equiv` (pool
  and `POOL=0`) and `make pool-equiv` give the committed hashes in
  `tests/golden/*/` byte for byte. A unit test also requires the parsed
  pins and pin models of every example to equal its Phase 0 directives, bit
  for bit.
- **Gate: schema validation in CI.** `make packages` (in `make test`, and a
  CI step of its own) validates every `part.toml`, `pins.toml`, test vector
  and `bb.lock` (22 files: the five packages, the two spec format examples,
  the four example locks) against the schemas with `python3-jsonschema`
  4.10.3, after a self-test that the schemas reject known-bad documents;
  then `bb pkg lint` (clean), `bb pkg test` (every vector at one cell and
  1 ms, identical) and `bb open --check` on every example.
- **ADR-006's M3 gates.** G1 as above. G2 as above. G3: `make dms-values`
  and `bbpkg/tests/values.rs`, 29 of 29 equal to `spicelib.parse_value` at
  `6854d25` (after Amendment 1's qualifiers). G5: `cli/tests/lock.rs`
  (refused by `bb open`, `--check` and `bb run`; `--update-lock` rewrites
  one line). G7: `bbpkg/src/spice.rs` and `bbpkg/tests/lint.rs`. G9: the
  synthetic `bb-part/1` registry tests (Amendment 1 item 9).
- **Gate: setup.** `make setup` now builds `bb` and installs it into
  `~/.local/bin`; `SETUP=demo` skips the ARM toolchain, uv and
  `python3-jsonschema`; as root it runs apt without sudo.
  **Timed in a clean `ubuntu:24.04` container** (podman 4.9.3, installed
  in WSL for this; image `d9b8c34d61a9`; as root, nothing preinstalled; the
  source a `git archive` of commit `590c455`; `make setup-container`): `apt
  install make` 5.5 s, `make setup SETUP=demo` 57.6 s (apt packages,
  rustup and Rust 1.99.1, Renode 1.17.0 portable, the release build of
  `bb` in 22 s, `bb` into `~/.local/bin`), then `make doctor SETUP=demo`
  and the installed `bb pkg lint` passing: **63.6 s in total**. A second
  run, which also ran `bb pkg test` with the container's ngspice, took
  66.0 s. Installed: 617 MB Rust toolchain, 184 MB Renode. R7's 15-minute
  budget is therefore not at risk from setup on this host's network; M7
  repeats the measurement from a local clone, with the demo's own steps.
- **No new board or run mode,** so no new equivalence gate or cost-table
  row: the examples are the same boards with byte-identical outputs. `bb
  run` now resolves packages and checks the lock before it starts any
  engine; measured at 22-174 ms (5 runs, load 23) up to the firmware check
  for the spike, from `/mnt/c`.

## Review notes: M2 (2026-11-09)

- **What exists.** Breadboard's nRF52840, `nrf52840-bb@1`
  (`engines/cpu-renode/platforms/nrf52840-bb.repl`: no SVD download, 64
  MIPS), `PinSpec::nrf`, `bb run` boards with `platform=nrf52840`, six
  bare-metal gate firmwares and a prebuilt Zephyr gate image in
  `engines/cpu-renode/tests/firmware/nrf52840/`, `examples/nrf-feedback`,
  and the D4 `PIN_CNF` audit (end of run, and mid-run through Renode's
  peripheral write hooks). Details and numbers: ADR-008 Amendment 2.
- **The M2 row's gate.** `make nrf-gates` passes on the warm pool and with
  `POOL=0`: the blinky edge list is identical at one-cell grants, 10 ms
  grants, plugin armed and lookahead on; the feedback board equals its
  `--lookahead off` reference with no broken promise; the lookahead
  self-test status is exactly 0x3; `nrf-busywait` records 6.660 ms for a
  10 ms nrfx busy-wait (expected about 6.67 ms). `make pool-equiv` with
  three nRF configurations passes. The four firmwares the row names
  (blinky, feedback, ppi, busy-wait) are there, plus D6's
  TIMER-compare-then-long-sleep firmware moved from M1, the `PIN_CNF`
  firmware and ADR-008 D3's `nrf-zephyr`.
- **Cost.** `make test` grows by `make nrf-feedback` (its one-cell
  reference is 500 ms of 10 µs grants, about 40 s on this host) and by the
  `nrf52840.rs` engine tests inside `cargo-test` (about 50 s, in parallel
  with the other engine tests): more than §8's "about 20 s", mostly one-cell
  reference runs. The cost-table row: ADR-008 Amendment 2.
- **Deviations** (ADR-008 Amendment 2): the feedback gate is a board run
  through `bb run` (as the plan's row says) as well as engine tests; the
  Zephyr gate image is built from the developer workspace (v4.5.0-rc3)
  until M6 pins a release; `bb run` audits bound pins only, with no drive
  set, until packages (M5) and the importer (M4/M6) supply the set and the
  unbound pins. The platform identity `nrf52840-bb@1` is a row of M3's
  `cli/src/platforms.rs`; no package names it until M5.

## Review notes: M4 (2026-11-09)

- **What exists.** The crate `importers/kicad` (an S-expression reader, the
  netlist model and its version gates, the schematic gates, the
  `kicad-cli` runner on a temporary copy of the project, Linux or the
  Windows build from WSL); `bbcore::board` and `bbcore::cjson` (`board.bbm`
  and the integer-only canonical JSON); in `bb` the project file and bench,
  names, resolution (D4, with `mpn_patterns` matched by `bbpkg::pattern`),
  elaboration, and `bb open` (projects, `.kicad_pro`, `.net`), `bb flash`,
  `bb run --project` (read-only; writes `run.json` and `deck.cir`;
  `--elaborate-only`) and `bb probe`. Normative: `spec/board-bbm.md`,
  `spec/canonical-json.md`, `spec/project.md` and the `board-bbm`,
  `project` and `bench` schemas (validated in `make packages`), and the
  ADR-007 amendments of `spec/pin-model.md` and `spec/event-stream.md`.
  Examples: `examples/adc/kicad` (the ADC board drawn in KiCad, with the
  local package `example/led-red`) and `examples/dial-me-spinner/bb.toml`
  with its committed `board.bbm` and `bb.lock`. ADR-007 Amendment 1
  records what the implementation settled.
- **Gate (a), `make adc-kicad` (in `make test`).** The KiCad drawing of
  `examples/adc`, imported and run as a project, gives board.cir's
  committed non-grant records (every `pin_edge` and log line) byte for
  byte; `nets.csv` and `nets.vcd` differ in 2 of 180 006 analog samples,
  by 1 µV. So byte identity with the hand-written hashes is **not** met,
  and the gate as applied is ADR-007 D9's stated fallback (byte identity
  between the `.net` and `kicad-cli` routes: yes; within the golden
  tolerance of 1 mV: 1 µV; an identical `pin_edge` list: yes). The cause
  was measured, not assumed, and the script checks it: board.cir writes C1
  as `470n`, which ngspice reads as 470 × 10^-9, one ulp above the
  correctly rounded 4.7e-7 that the generated deck passes through a subckt
  parameter; board.cir with `4.7e-7` gives the KiCad run's files byte for
  byte, and the element order does not matter. The project run mode has
  its own one-cell reference and committed hashes
  (`tests/golden/adc-kicad`), a `make pool-equiv` configuration and a cost
  row (README). `bb run` changes no file of the project.
- **Gates (b)-(e), `make kicad-import` (KiCad 10.0.8 in WSL and on
  Windows; about 20 s) and `cli/tests/kicad_import.rs` (in `make
  test`).** (b) Main board at `6854d25`: 201 parts, 142 nets, 657 pin
  nodes; the synthetic two-board fixture refuses a bare `U1` in
  `[instances]`, `[[bind]]` and `--probe` with both candidates listed
  (`a:U1 (RT9080-33GJ5), b:U1 (BQ25180YBGR)`), resolves `a:U1`, a leaf net
  within the simulated board, and qualified test-point aliases. (c) One
  `board.bbm` from the committed netlist, a fresh Linux `kicad-cli` export
  (as a `.net` and through `kicad =`), the Windows `kicad-cli.exe` (through
  `kicad =` and as a CRLF `.net`) and a partial KiCad 10 re-save (three
  sheets at 20260306, four at 20250114). Secondary (decision C):
  `dc96515`'s KiCad 9.0.9 netlist gives the **identical** `board.bbm`; no
  difference to record. (d) Two imports byte-identical (`board.bbm`,
  `bb.lock`, `.bb/resolution.json`, `.bb/unresolved.toml`); the KiCad
  sources unchanged (no `.kicad_prl`). (e) Refused: a sheet stamped
  20261106, a `.net` from Eeschema 11, a stacked pin `[1-4]`. ADR-007's own
  M4f (resolution counts: 118 generic, 12 no-model, 70 MPN parts
  unresolved, J11 the only part with no key; `RT9080-33GJ5` never binds to
  a package that lists only `RT9080`) and M4g (canonical JSON) pass too.
- **Deviations (ADR-007 Amendment 1).** Gate (a) through D9's fallback
  (item 1); `bb-board/0`, not `/1` (item 3); DNP and `exclude_from_sim`
  set status `excluded` without changing the stage, so D4's counts hold and
  a DNP part is never simulated (item 5); the pin-name rule skips passive
  packages (item 7); `bb run` checks currency by re-importing (or from the
  netlist cached in `.bb/` after hashing every schematic) instead of
  running `kicad-cli` per run (item 8); `bb open <board.cir>` still writes
  only `bb.lock`, and the shared type of the two front ends is the run plan
  (item 9); the bench of D5 is implemented now but gated at M6 (item 13);
  the PCB cross-check (D8) is deferred (decision A7).
- **Cost.** Simulation of the KiCad project equals the board file's
  (README cost table: 1.13 s against 1.09 s of simulation, 2.64 s against
  2.62 s in total, defaults, fresh, load 1.6-2.2). The front end (import,
  lock and currency check, elaboration) takes under 20 ms with the packages
  off `/mnt/c`, 0.2-1.2 s with the first-party packages on `/mnt/c`.

## Review notes: M5 and M5b (2026-11-09)

- **What exists.** Four first-party packages: `ti/tps22917` (the committed
  default view `lsw`, **datasheet-fitted**: breadboard.dev's own behavioural
  model from SLVSDW8B Tables 7.3, 7.5 and 7.6, M5b), `worldsemi/ws2812c-2020`
  (the suite's pixel view, copied unchanged), `nordic/nrf52840-qiaa` (74
  pins, the GPIO pin models as named PIN_CNF-drive and PS-column sets, the
  Renode platform `nrf52840-bb@1`) and the module `raytac/mdbt50q-u1mv2` (61
  pads mapped onto the chip). The vendor-derived TPS22917 view is a
  **LOCAL-ONLY** package with the same id, view name, ports and parameters,
  built by `make dms-local-packages` into the git-ignored `local-packages/`
  from a `git archive` export (the S1 fix and the `GND` -> `GNDP` rename);
  nothing TI-derived is committed, and `make packages` checks that
  (`scripts/check_local_only.sh`). Details: ADR-006 Amendment 3.
- **Gate M5** (`bb pkg test` per package through `NgspiceEngine`, vectors
  reproduce `results.json` within the stated tolerance; each view qualified
  at (10 us, `phase0`)). The vectors are the suite's own decks, cut around
  the part under test by `scripts/dms_packages.py`, with `results.json`'s
  values (`make dms-vectors-check` regenerates them byte for byte).
  - Vendor-derived `lsw` (local): 9 vectors, recorded at (10 us, `phase0`):
    Table 7.6 crossings within 0.5 ns, RON and QOD within 1 uV, QOD
    discharge times within 10 ns of `results.json`; gate G8 (the rename)
    byte-identical over 18 vector runs.
  - Datasheet-fitted `lsw` (committed): the same 9 vectors, recorded at
    (10 us, `phase0`): Table 7.6 within 0.5 % of the datasheet (tON 3.818 /
    4.020 ms, tR 1.613 / 1.815 ms), RON 90 / 80 mOhm, QOD 150 Ohm exactly;
    against `results.json` within the stated 0.2 ms / 3 mV / 0.5 mV / 1 ms
    (measured 0.12-0.16 ms, 2.7 / 0.85 mV, 0.23 mV, within 0.56 ms).
  - `pixel`: the suite's phantom operating point at bat4.2 25 C and usb5.0
    60 C, U2 stood in as its off-state QOD resistance; VLED and DIN - VDD
    within 5 uV of `results.json`; recorded at (10 us, `phase0`).
  - Every vector gives identical samples at one-cell and 1 ms grants. `make
    packages` (in `make test`) runs them all.
- **Gate M5b** (`make dms-m5b`): the E1 deck through `NgspiceEngine` at
  `phase0` on both benches with each view: VLED at each P0.06 rise within
  0.001 mV, max(DIN - VLED) within 0.0001 mV, the LSW-2 time within 0.555 ms
  (nominal) and 0.356 ms (cold), the same E1 verdicts, and in a wrong-order
  run the LSW-3 excursion within 0.5 mV with LSW-3 failing for both. 20 of 20
  gates; the table is in ADR-006 Amendment 3 item 4.
- **Deviations** (ADR-006 Amendment 3 item 5): the nRF52840's GPIO "views"
  of D9's table are pin parameter sets, not analog views (a circuit view of
  the pad's driver would be a second driver on the net); the bare chip's
  KiCad composite pin names (`AIN0/P0.02`) are matched by the pin checker,
  not yet by the importer (no reference board places the bare chip); module
  semantics are new in `spec/package.md` section 4, and binding a module pad
  in `bb run` is M6's; the WS2812C vectors stand U2 in as its off state; the
  QOD crossing levels are fixed from the suite's own `vled0`.
- **M4f** changes with the package set (ADR-007 Amendment 2): 51 parts by
  MPN, 19 unresolved (70 at M4); `make kicad-import` passes with the
  restated counts, and the committed `board.bbm` and `bb.lock` are
  regenerated.
- **Cost.** `make packages` takes about 22 s (6 s of it the new vectors);
  `make dms-local-packages` about 25 s; `make dms-m5b` about 40 s on 8 jobs.
  M5 adds no board or run mode; M6 adds E1's equivalence gate and cost row.

## Review notes: M6 (2026-11-09)

- **What exists.** `examples/dial-me-spinner` runs E1. The new pieces:
  - `bb.toml` gains the target U10, `[[bind]]` P1.04 (`S0S1`) and P0.06
    (`H0H1_max`), and `[probes]`;
  - `bench/nominal.toml` and `bench/cold.toml`;
  - `firmware/`: a pinned west manifest (Zephyr v4.5.0, SDK 1.0.1), the
    out-of-tree board `dial_me_spinner/nrf52840`, the `led_rail` app with
    its one Kconfig switch, and `prebuilt/` (both stripped ELFs,
    `SHA256SUMS` of their image hashes, `PROVENANCE`);
  - `scripts/check_dms_rail.py`, `scripts/dms_rail.sh` (`make dms-rail`,
    `make dms-rail-test` in `make test`), `scripts/dms_zephyr.sh` (`make
    fw-rebuild`, `make dms-firmware`, `make setup SETUP=zephyr`),
    `scripts/dms_suite_deck.py` and `scripts/dms_crosscheck.sh` (`make
    dms-crosscheck`, local);
  - `tests/golden/dms-rail/`.

  In `bb`: module targets and pad bindings through the pinmap, the ELF
  image hash (`bb flash`, `bb run`, `--hash-only`), the full `PIN_CNF`
  audit list for projects, and Norton bench supplies. Details: ADR-008
  Amendment 3, ADR-007 Amendment 3, ADR-006 Amendment 4.
- **The M6 row's gate, item by item.**
  - Both benches pass with the correct order. LED-DATA-V4: 3.684020 V
    (nominal) and 2.971510 V (cold; the suite's 2.971511 V). LSW-3: 0.000 V
    and 0.320 V. LSW-2: 24.420 ms at both (the suite at bat3.0: 24.059 ms).
  - The wrong order fails exactly LSW-3 at both benches: 0.728 V (3.64 mA
    clamp current) and 0.793 V (4.08 mA). `check_dms_rail.py
    --expect-fail LSW-3` passes only then.
  - Board-level convergence: every run completes 120 ms at `phase0`, at
    both engine orders, in 3.0-6.2 s of wall time (limit 30 s). The gate
    found a real stall first: the bench's ideal tracking +3V3 across the
    rail's 17 capacitors. Bench supplies with `rs` are now Norton sources
    (ADR-007 Amendment 3 item 4).
  - One-cell equivalence: all four cases equal their `--max-quantum 10us`
    references except grant records.
  - 10-run determinism: byte-identical.
  - Goldens in `tests/golden/dms-rail/`.
  - The local PWL cross-check: VLED within 0.223 mV (nominal) and 0.489 mV
    (cold) of the co-simulation with the vendor-derived view (gate 5 mV).
    With the committed fitted view the figures are 0.850 / 1.039 mV, and
    the suite's battery-network deck differs from the bench deck by 1.53 /
    2.43 mV (both reported only).
  - Also: analog-first byte-identical on every case, the `PIN_CNF` audit
    clean with two negative controls that fire, `fw-rebuild` PASS from a
    fresh workspace, and `make pool-equiv` with three E1 configurations.
- **Gate runs** (2026-11-09, on `78a9674` plus M6, load 2-12 from other
  agents' work):
  - `make check` on the pool: green (7 min 53 s), now including `make
    dms-rail-test`;
  - `make test POOL=0`: green (8 min 40 s, every Renode fresh);
  - `make dms-rail`: PASS on the pool (2 min 15 s) and with `POOL=0`
    (2 min 47 s), 42 gate lines each and none failing;
  - `make pool-equiv`: PASS, 19 configurations, 38 pooled runs from one
    Renode process;
  - `make fw-rebuild` and `make dms-crosscheck`: PASS.
- **A firmware finding** (ADR-008 Amendment 3). Zephyr v4.5.0's
  `regulator_enable()` waits `off-on-delay-us` (30 ms), not
  `startup-delay-us` (10 ms), and nothing enforces the off time before a
  turn-on. P0.06 therefore rises 30.06 ms after LED_PWR_EN. That still
  meets LED-DATA-V4, but it is not what firmware.md 6.4 says.
- **Deviations.**
  - The 10 ms wait is 30 ms in the image (above).
  - P0.06 is a GPIO with H0H1 drive standing in for the I2S.
  - The firmware cycles forever, so LSW-2 is evaluated on the one complete
    off sequence of a 120 ms run, and the second fall is reported as not
    evaluable.
  - The E1 benches give +3V3 10 mOhm of series resistance (the Norton
    form).
  - A pin's `voh` is the bench's +3V3 ceiling (3.3 V), not the cold rail
    (2.93 V). Rail-tracking outputs are Phase 2; this is pessimistic for
    LSW-3 and does not move VLED.
  - `fw-rebuild` is a make target and a CI job definition; CI itself is
    not run (no remote, approver decision B).
- **Cost.** The README row (ADR-003 protocol, N = 5): 120 ms takes 9.21 s
  of simulation one-cell (12 000 grants) and 1.87 s at bb's defaults (12
  grants, 4.9x). Engine startup is 1.36 s fresh and 0.07 s pooled. The
  host was loaded (3.9-11.6), so these are upper bounds.

## Review notes: M7 (2026-11-09)

- **What exists.**
  - **The README quickstart** is at the top of `README.md`. Its command
    block sits between `<!-- stranger-test:begin -->` and `<!--
    stranger-test:end -->`. It is §5's sequence plus three things a clean
    Ubuntu needs: the `apt-get install git make ca-certificates` line, the
    `export PATH="$HOME/.local/bin:$PATH"` line (root's `.profile` in
    `ubuntu:24.04` does not add `~/.local/bin`), and the wrong-order run
    written out. `<CLONE-URL>` is marked as a placeholder to be filled in
    at publication; until then, users clone from a local path or a
    `file://` URL. The README also shows the expected PASS and FAIL output
    and explains what each step writes.
  - **`make demo`** (`scripts/demo.sh`, about 15 s) runs the README's
    commands on a copy of the project, with every Renode started fresh. It
    then checks both runs against `tests/golden/dms-rail/{ok,wrong}-cold`
    (§5 item 2) and checks `run.json`'s list of analog views (§5 item 4:
    132 instances, `phase0`, each with its provenance, confidence and
    qualification).
  - **`make stranger-test-container` and `make stranger-test-wsl`**
    (`scripts/stranger_test.sh`) run the README block at `HEAD` line by
    line, as root, from `$HOME`, timed, against a 900 s limit. `<CLONE-URL>`
    becomes the local repository. Then they check the committed hashes and
    run `make demo`.
  - **`docs/phase1-exit-checklist.md`** gives the exact steps for the one
    human run, and what the human does if they decide to publish.
- **The cost-table row** (README): E1 re-measured with `make
  cost-table-dms-rail` on an idle host (N = 5, interleaved, medians; fresh
  at load 0.7–1.1, pool at load 1.0–1.4). One-cell: 6.34 s of simulation,
  7.78 s in total fresh. Defaults: 1.83 s (3.5×), 3.29 s in total fresh,
  1.95 s pooled; engine startup 1.42 s fresh, 0.08 s pooled. Every B run
  was identical to A except for grant records. M6's figures were measured
  at load 3.9–11.6 (9.21 s and 1.87 s of simulation).
- **The stranger test (§5 item 1), measured.** Both runs started from a
  clean Ubuntu 24.04: image `docker.io/library/ubuntu:24.04` `d9b8c34d61a9`
  (24.04.5), with no git, make, Rust, Renode, ngspice, `bb`, KiCad, west,
  ARM toolchain or python3 beforehand. Both cloned commit `1d9bd64`, ran
  as root, on a 24-CPU host. Times in seconds:

  | Step | Container | Fresh WSL distro |
  | --- | --- | --- |
  | `apt-get install git make ca-certificates` | 13.2 | 21.7 |
  | `git clone` (local) | 44.6 | 24.5 |
  | `make setup SETUP=demo` | 48.8 | 52.8 |
  | `bb open`, `bb flash` | under 0.1 each | under 0.1 each |
  | `bb run`, correct order | 3.6 | 3.7 |
  | `check_dms_rail.py` | 0.1 (PASS) | 0.1 (PASS) |
  | `bb run`, wrong order | 3.0 | 3.1 |
  | `check_dms_rail.py`, wrong order | 0.1 (FAIL: exactly LSW-3, exit 1) | 0.1 (the same) |
  | **README total** | **113.9 (1 min 54 s)** | **106.8 (1 min 47 s)** |
  | `make demo` afterwards | 6.8, PASS | 6.4, PASS |

  The two environments:
  - **Container:** rootless podman 4.9.3, with the repository mounted
    read-only and cloned from `file:///repo`.
  - **Fresh WSL 2 distro:** the same image's root filesystem
    (`podman export`), imported with `wsl --import` into
    `%TEMP%\bb-stranger-wsl-<pid>` (WSL 3.0.3.0). It cloned the plain path
    `/mnt/c/source/tyevco/breadboard.dev`, and was unregistered and its
    folder deleted afterwards (checked: `wsl -l` lists only `Ubuntu-24.04`).

  In both, the two runs' `nets.csv`, `nets.vcd` and non-grant records
  equal the committed hashes, and no KiCad, west or ARM toolchain was
  installed. Installed: 617 MB of Rust and 184 MB of Renode. The 15-minute
  budget is met with about 13 minutes to spare. The clone (from `/mnt/c`,
  1 438 loose objects) and `make setup` take most of the time.
- **Deviation: the apt mirror.** Both timed runs used `--apt-mirror
  http://mirrors.kernel.org/ubuntu/`. Before the timer starts, this points
  the image's apt sources at that mirror; the README is unchanged and
  `timing.txt` records it. The reason: Canonical's archive was
  unreachable from this host. The earlier attempts with the image's own
  sources (`archive.ubuntu.com`, `security.ubuntu.com`):
  1. At `fbcfc1d`, step 1 failed after 16.6 s: `security.ubuntu.com`
     returned 404 for perl updates that its index already listed (a
     mirror mid-sync).
  2. Step 1 took 508 s (apt at 87 kB/s), then `make setup` failed after
     98 s: "File has unexpected size (2176865 != 2176869). Mirror sync in
     progress?". `setup.sh` gave up on the first failed index, so commit
     `1d9bd64` makes it retry each download and the whole apt command, three
     times each, 30 s apart.
  3. From about 21:10 to past 21:42 (PST), every IPv4 address of
     `archive.ubuntu.com` and `security.ubuntu.com` timed out or crawled.
     Probes every 3 min mostly got 0 B/s, and never more than 46 kB/s. Over the same period `mirrors.kernel.org`,
     `static.rust-lang.org` (28 MB/s) and GitHub answered normally.

  So the measured 107–114 s holds for a working Ubuntu mirror. A stranger
  whose mirror is down or syncing is stopped in the README's first line,
  before any Breadboard code runs. `setup.sh` now rides out a sync, but not
  an outage. The unmodified run (the image's own sources) still needs to
  be repeated once Canonical's archive is reachable: `make
  stranger-test-container` and `make stranger-test-wsl` with no
  `STRANGER_ARGS`.
- **Other deviations.**
  - The README block adds the prerequisite, `PATH` and wrong-order lines
    to §5's sequence (above).
  - The fresh WSL distro is built from the `ubuntu:24.04` OCI image, not
    the Microsoft Store image. That avoids another download, and the OCI
    image is the stricter of the two: it has no git, python3 or sudo.
  - The container is rootless. Inside a rootful container, root does not
    own the bind-mounted repository, so git 2.43's safe-directory check
    refuses to clone it ("dubious ownership"). A real URL does not have
    this problem.
  - The "one human run" of this section is not done; it is the user's, by
    `docs/phase1-exit-checklist.md`.
- **The other §5 items, re-run at `1d9bd64`** (load 0.3–3.6):
  - item 2: `make test` is green on the pool (7 min 12 s) and with
    `POOL=0` (9 min 25 s). `make dms-rail` passes on the pool (2 min 23 s,
    42 PASS lines, including 10-run determinism) and with `POOL=0` (2 min 33
    s, 42 PASS lines, none failing).
  - item 3: `make kicad-import` passes (56 s).
  - item 4: `make demo` checks it, and `bb run` notes the
    `agent_generated` ideal R and C.
- **Not done (decision B).** No remote, no push, no repository. Before
  publication the tree also needs the Apache-2.0 `LICENSE` file (decision
  A2; packages already declare `license = "Apache-2.0"`), and `<CLONE-URL>`
  must be filled in. Both are listed in the exit checklist's step 6.

## Review notes: after M7 (2026-11-09)

A review of the finished Phase 1 raised 15 findings (1 major, 14 minor).
Each was re-checked before anything changed. What became of each:

| # | Finding | Outcome |
| --- | --- | --- |
| 1 | (major) With `ubuntu:24.04`'s own apt sources the README block took 19 min 58 s (apt at 63 kB/s, `security.ubuntu.com` timing out); with another mirror, 71 s. The README had no way out | **Fixed (docs).** The README states that the 15 minutes assume a reachable Ubuntu mirror and gives the one-line `sed` that points apt at another mirror (checked on the image's `ubuntu.sources`). `setup.sh` prints that hint when apt fails after its retries or takes over 300 s. Re-probed at 22:14 PST: `archive.ubuntu.com` 1.09 MB/s, `security.ubuntu.com` still timing out after 20 s. The timed run with the image's own sources inside the budget is **still owed** (status line) |
| 2 | The datasheet-fitted TPS22917 view is singular with QOD floating; `bb` stopped with an empty reason | **Fixed** (ADR-006 Amendment 5 item 1): `R_QODL QOD VOUT 1G`, vector `qod_open_3.6` (failed before, passes after), package 0.2.0, `NgspiceEngine` reports ngspice's warnings when it stops without an error. E1 moved by at most 1 µV (2 µV on VSYS); the dms-rail hashes were rewritten from fresh one-cell references, non-grant records unchanged |
| 3 | E1 never gated the firmware rule's timing (a 5 ms wait passed) | **Fixed** (ADR-007 Amendment 4 item 1): check FW-6.4 in `check_dms_rail.py`, with two negative controls in `make dms-rail-test` |
| 4 | The fitted view's provenance named the wrong source for several figures; SLVSDW8B was never opened | **Fixed** (Amendment 5 item 2): each figure's real source is named; `confidence` is `self_checked` until someone checks SLVSDW8B |
| 5 | dial-me-spinner's HEAD is `claude/revb-bench` (`8663b6e`), not `kicad10-port` at `6854d25` | **Not a bug in breadboard.dev.** Its reflog shows the checkout at 12:28:55, about 2 h before the first Phase 1 commit; no breadboard script writes there, and every script reads it by commit id (`git archive`/`git show 6854d25`; no script reads its working tree). The human should confirm the checkout was theirs |
| 6 | ADR-006's promised three complete spec example packages differ from what exists | **Fixed (amendment)**: Amendment 5 item 4 records the deliverable as built |
| 7 | The per-instance analog view override is untested | **Fixed**: a `select_view` unit test and a project test (`[instances."main:D1"] analog`) (ADR-007 Amendment 4 item 2) |
| 8 | ADR-007 D2's variants and jumper-pin-group refusals are untested | **Fixed**: importer unit test on synthetic projects |
| 9 | Bench `[[pwl]]` elaboration is untested | **Fixed**: deck-text test (ideal and `rs`) |
| 10 | `check_local_only.sh` passed vacuously outside git | **Fixed** (Amendment 5 item 6), with `scripts/check_local_only_test.sh` in `make packages` |
| 11 | The migrated WS2812C model is not in `SOURCE` | **Fixed** (Amendment 5 item 5): listed, and `make dms-source` requires every package `origin` to be |
| 12 | The fitted view said `source = "datasheet"` though an agent wrote it, so §5 item 4's flag never fired | **Fixed** (Amendment 5 item 3, `spec/package.md` section 2): `agent_generated`; `bb run` flags it |
| 13 | §5 item 3 claimed KiCad 9 or 10 installed; no gate runs KiCad 9's `kicad-cli` | **Fixed (wording)**: §5 item 3 and the README say KiCad 10, and that the KiCad 9 netlist imports identically (ADR-007 Amendment 4 item 3) |
| 14 | M7 "done" while the human run and an unmodified-mirror run are open; no `LICENSE` | **Fixed / still owed**: the status line now says M7 is done except those two measurements; `LICENSE` (the Apache-2.0 text, sha256 `cfc7749b...`, decision A2) is in the tree. The human run and the unmodified run remain the human's and the network's |
| 15 | AGENTS.md's `pool-equiv` row is stale; `dms_s1.py` says Amendment 1 for the S1 fix | **Fixed**, except the two comment lines `dms_s1.py` writes *into* the local-only view: kept, so the recorded sha256s of that view stay valid (Amendment 5 item 7) |

- **Gates, after the fixes** (load 0.1-3.7): `make check` on the pool
  (7 min 33 s) and with `POOL=0` (8 min 59 s), `make dms-rail` on the pool
  (2 min 36 s, 44 PASS lines) and with `POOL=0` (2 min 52 s, 44 PASS
  lines, none failing), `make dms-m5b` (20/20),
  `make dms-vectors-check`, `make dms-source` with `DMS_REPO` (20 files,
  1 package origin), `make demo`. `git status` is clean after the commit.
  At that commit (`aba44e9`), `make stranger-test-container` with
  `--apt-mirror http://mirrors.edge.kernel.org/ubuntu/` passed: the README
  block in 119.4 s (limit 900 s), the wrong order failing exactly LSW-3
  with FW-6.4 passing, both runs equal to the rewritten hashes, and `make
  demo` inside the container.
- **Observed once, not caused here:** in the first pooled `make check`,
  `cli/tests/jit_profile.rs` found no JIT profile after the gather run
  (Renode's profile is written on `SIGTERM`); it passed three times in a
  row on its own and in the full re-run. Its cause was not investigated;
  the profile affects JIT timing only, never an output (ADR-004
  Amendment 6).

## Review notes: M8 (2026-11-10)

M8 was done in two halves. The first half (`e2a7de7`, `311d2e8`; ADR-008
Amendment 4) is the SAADC model, the loading `adc_in`, the per-sample
records, the UART lines and the VDD input, gated by `make
nrf-saadc-model`. The second half is E2 itself (ADR-008 Amendment 5).

- **What exists.**
  - `examples/dial-me-spinner/firmware/vsense/`: the battery reading and
    the LED shed of firmware.md 12.4, built as `prebuilt/vsense.elf` (one
    reading per second) and `vsense_50ms.elf`. `make fw-rebuild` reproduces
    all four images.
  - `bb.toml` binds VSYS_SENSE (AIN0, the loading input, per-sample
    records) and U10's VDD.
  - `bench/vsense.toml` (the suite's sampling worst case) and
    `bench/sag.toml` (a battery ramping down through the shed).
  - `scripts/dms_vsense.sh` and `scripts/check_dms_vsense.py`, run by `make
    dms-vsense` and, in `make test`, `make dms-vsense-test`. Goldens are in
    `tests/golden/dms-vsense/`.
  - The `nordic/nrf52840-qiaa` test vectors qualify `saadc_se`.
  - In `bb`: the project `[[bind]]` takes `load` and `trace_samples`; `bb
    pkg test` runs `pin_load` views; `bb run` checks a `pin_load` view's
    qualification; and a module pad must offer the model it is bound with.
- **The M8 row's gate, item by item** (numbers in ADR-008 Amendment 5).
  - **The analog side against the `load = "none"` reference.** At 1 s the
    co-simulation gives 272.5 uV, and the suite's arithmetic at the
    bench's nominal parts gives 272.6 uV. At 50 ms it is 726.4 against
    726.3 uV. With C5 / 10 it is 2676.6 against 2675.0 uV. At the suite's
    corner, the same arithmetic gives 356.15, 761.26 and 3475.11 uV,
    against the suite's 356.16, 761.27 and 3474.26 uV. The tolerances are
    set from this measurement: per sample, 1 uV plus 0.25 % (worst
    measured 5.5 uV, on c5); 3 uV for the burst mean; 2 uV against the
    suite.
  - **Firmware-visible.** Every code is the predicted one. 50 ms moves the
    reading from 1067 to 1068 once C5 has settled. C5 / 10 reads 3 LSB high.
    On the sag bench, LED_PWR_EN falls 154.4 ms after the threshold
    crossing (the limit is one 1 s period). Every reading before the shed
    stays at or above code 1283.
  - **Equivalence with the every-lattice-point reference:** seven cases,
    each equal except grant records; the references equal the committed
    hashes. There is no `stop:` grant, so no `stop:ngspice`, and no
    promise is broken.
  - **Also:** analog-first is byte-identical, and the TACQ refusal works
    on the project path.
  - **10-run determinism (sag):** byte-identical.
  - **Goldens:** `tests/golden/dms-vsense/` (7 cases).
  - **Cost-table row:** in the README (`make cost-table-dms-vsense`).
- **Deviations** (ADR-008 Amendment 5).
  - The suite's figures are at a tolerance corner (C5 -23.5 %, R +1 %),
    which a Phase 1 bench cannot set. So they are reached through the
    suite's arithmetic, which must match both the co-simulation and the
    figures.
  - The suite's own worst case (VSYS 3.0 V, VPRE = VDD max 3.366 V)
    replaces the plan's "cold bench" (VDD 2.93 V).
  - The shed timing is measured from the firmware-visible crossing.
  - The off order is checked as "not after" (equal stamps).
  - The firmware stores no calibration and makes no rested-OCV correction.
- **E1 changed slightly.** With the SAADC bindings in `bb.toml`, E1's
  board is cyclic (lookahead on). VSYS_SENSE carries CPAD and CS. VLED
  moves by at most 1 uV and VSYS by 9 uV (nominal bench), and non-grant
  records are unchanged. `nets.vcd` gains `U10.P0.02#smp`. The dms-rail
  hashes were rewritten from fresh references, `make dms-rail` passes, and
  the E1 cost-table row was measured again.
- **Gate runs** (2026-11-10, on `ec759de`; load at start and end):
  - `make dms-vsense POOL=0`: PASS (13 min 44 s; load 1.0, then 4.8 from
    its own parallel runs).
  - `make check POOL=0`: green (10 min 31 s, load 1.5-1.6, 207 cargo
    tests).
  - `make check` on the pool: green (8 min 53 s, load 0.6-1.6).
  - `make dms-rail POOL=0`: PASS (2 min 49 s).
  - `make fw-rebuild`: PASS.
- **Not run for this milestone:** `make pool-equiv` has no E2
  configuration. Pooled against fresh is covered by `make
  dms-vsense-test`, which passes against the same committed hashes in both
  `make check` runs. (Closed after the review: `make pool-equiv` now has
  E2; see "Review notes: after M8".)

## Review notes: after M8 (2026-11-10)

A review of M8 raised nine minor findings and no blocker or major one.
Each was re-checked before anything changed (ADR-008 Amendment 6). What
became of each:

| # | Finding | Outcome |
| --- | --- | --- |
| 1 | Scan mode takes exactly the sum of TACQ + tCONV; the PS says less, and the header did not call it a deviation | **Fixed (documented).** Listed as a deviation in the model's header and in Amendment 4's not-modelled list. The PS gives no figure for the overlap, so it is not modelled. E2 uses one channel |
| 2 | The PS's "VDDHDIV5 needs TACQ >= 10 us" was neither enforced nor warned | **Fixed.** A one-time warning, like `SAMPLERATE.CC < 80`. `bb_saadc_test.py registers` checks for no warning at 10 us and exactly one, naming 5 us, at 5 us |
| 3 | No case reached the restore half of the LED policy, and `check_dms_vsense.py` failed any run whose LEDs came back | **Fixed.** New case `rec` (`bench/sag-recover.toml`) and `check_rec`: shed at 1269, restore at 1314 (VBAT_est 3.618 V >= 3.58 V), 1 s after the shed, P0.06 30.065 ms after LED_PWR_EN; one-cell reference equal except grants; new golden `rec`. The sag run fails `check_rec` (negative control) |
| 4 | `make dms-vsense` did not vary the grant partition | **Fixed.** s50 at `--max-quantum 1ms` and at `3ms --verify every`, each equal to the committed hashes except grants, with no broken promise |
| 5 | The package did not document the TACQ >= 2 L refusal (D7) | **Fixed.** `pins.toml`'s `adc_in` comment; `make locks` |
| 6 | The package did not document that UARTE pins give no edges (D8) | **Fixed.** `pins.toml` header |
| 7 | `run.json` recorded `load = "none"` only by leaving an entry out | **Fixed.** An `analog` entry with `kind = "pin_load"`, `view = "none"` under the bound pin (`spec/project.md` section 6, as D7 already required). `dms_vsense.sh` checks it on every case, in `make test` too |
| 8 | D7's promised connected-vs-switched CS difference was never reported | **Fixed.** `check_dms_vsense.py` prints it: +0.000 / +0.016 / +0.000 uV on the burst mean (s1 / s50 / c5) |
| 9 | E2 was not in `make pool-equiv`, though `make check` runs it pooled | **Fixed.** s50 at bb's defaults and as its reference are in the list, with `dms-vsense-prepare`; `make pool-equiv` passes (below) |

- **Gates, after the fixes** (2026-11-10, run one after another; load
  at start and end): `make nrf-saadc-model` PASS (25 s); `make dms-vsense
  POOL=0` PASS (15 min 56 s, 42 PASS lines, load 1.1-4.9); `make dms-rail
  POOL=0` PASS (2 min 47 s, 44 PASS lines); `make check POOL=0` green
  (10 min 30 s, 207 cargo tests); `make check` on the pool green (9 min
  20 s); `make pool-equiv` PASS (23 configurations, 46 leased runs,
  20 min 47 s). Every committed golden is unchanged; `rec` is new. Not
  pushed.

