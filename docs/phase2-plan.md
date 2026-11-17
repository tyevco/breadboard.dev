# Phase 2 plan: power integrity, benches with assertions, and a package ecosystem

- **Status:** Accepted (2026-11-15): the approver accepted all twelve
  questions of section 9 with their recommended answers, and set this
  build's scope (section 10). Proposed 2026-11-14, revised after design
  review round 1 the same day (see "Review notes" at the end). When it was
  accepted nothing in this plan or its ADRs was implemented, and no
  `spec/` file had been changed.
- **Technical exit measured (2026-11-17):** every milestone of this
  build's scope is merged, and section 5's items were measured on `main`
  ("Review notes: technical exit", at the end): items 1 and 3-6 pass
  (item 5e as an estimate from local job-equivalents, item 6 against local
  fixtures, both by decision); item 2 passes its stated checks, with W2's
  one-brown-out loop left to the approver (ADR-009 Amendment 9), as is one
  P0 spike gate broken by D-TPS (ADR-009 Amendment 10). The adoption exit
  is not attempted in this build (section 10).
- **Decisions it depends on (all Accepted 2026-11-15, ADR-014 behind its
  gate G0; written with this plan):**
  - ADR-009, `pi` fidelity;
  - ADR-010, bench v1, assertions, `bb test` and JUnit;
  - ADR-011, the package registry, signing and `bb-lock/1`;
  - ADR-012, the package-generation pipeline and the provenance and review policy;
  - ADR-013, project run modes (devices in `bb.toml`, single-MCU system runs, runs without a CPU target);
  - ADR-014, quiet grants with several producers;
  - ADR-015, Phase 2 hardening;
  - ADR-016, the waveform viewer and IDE integration.
- **Replaces:** the design doc's Phase 2 ("worked example 2" on an invented
  sensor board; 10 external projects; 500+ packages; a paper or talk). Its
  worked examples become the user's own board's power rules, and its exit
  criteria are restated below as measurable ones that one person plus
  agents can reach (section 5).
- **Evidence:** five research passes, all read-only on every repository:
  - the Phase 1 retrospective (`~/bbscratch-p2-retro`): the SAADC stall
    ablation, the E3 profile, the `.save` measurement, `/mnt/c` timings,
    CI timings;
  - `pi`, benches and `bb test` (`~/bbscratch-p2-pi`): a `pi` prototype
    run through the real `NgspiceEngine` (`deck_run`), and the nRF52840
    PS v1.1 and RT9080 datasheets;
  - the ecosystem (`~/bbscratch-p2-eco`): KiCad 10.0.8 library counts, the
    signing crates' dependency counts, wellen and cortex-debug against
    bb's outputs, and datasheet fetchability;
  - this plan's own checks against `examples/dial-me-spinner/suite/results.json`
    and the caliper docs;
  - **the review round's measurements** (`~/bbscratch-p2-rev`): the suite's
    own W1 and W2 decks, from a `git archive` export of `6854d25`, re-run
    with the suite's venv against the settable POFWARN thresholds, swept
    over the suite's static +3V3 load, and run at Breadboard's `phase0`
    option set (`w1_sweep.py`, `w1_phase0.py`, `floor_probe.py`). Section 2
    quotes them.

## 1. Goal

Phase 1 proved the **funnel**: import a real KiCad board, resolve its
parts, run real firmware, and reproduce a firmware rule that the board's
SPICE suite had established. Every Phase 1 result was at `fast`: ideal
rails, a bench in place of the power path, and checks written as Python
scripts.

Phase 2 makes three things true:

> 1. **Power is simulated, not assumed.** At `--fidelity pi` the rails
>    come from regulator and power-path views. Firmware state draws current
>    from them. Pins drive from the rail they sit on. A sagging rail can
>    raise POFWARN or reset the CPU. Phase 2 must reproduce, with the
>    firmware in the loop, a firmware power rule from dial-me-spinner's
>    suite that no `fast` run can show: the POF blanking of R8 (W1 below).
> 2. **A board's checks are data, not scripts.** A bench declares the
>    stimulus. An assertion declares the check. `bb test` runs the cases
>    and writes JUnit for CI.
> 3. **Other people can use it on their own boards.** Packages come from a
>    signed public registry. A reviewed pipeline produces them from
>    datasheets with checkable provenance. A stranger can import a board,
>    find most of its parts, and get a green `bb test`.

The first two are engineering, and agents can drive them to a gate. The
third depends partly on people outside the project. The definition of done
(section 5) therefore separates a **technical exit** from an **adoption
exit**.

## 2. Reference worked examples (ranked)

All of them come from dial-me-spinner's firmware rules (`docs/firmware.md`
§12) and from its suite at `6854d25`. Each one is cross-checked against an
independent reference: the suite's own result, re-derived in M0 of Phase 1
(`examples/dial-me-spinner/suite/results.json`), or for caliper the
datasheets.

**What the suite's decks are** (read for this revision, not assumed):

- +3V3 in the W1 and W2 decks is **not** the RT9080 model. It is an ideal
  LDO, `B3V3 n_p3v3 0 V = min(3.3, V(n_vsys) − I3V3·LDO_RDO)`, with the
  always-on +3V3 load `I3V3 = 50 mA` drawn from VSYS
  (`test_load_switches.py`, `vsys_network`).
- The BQ25180 is the suite's hand-built `load_switches__bq25180.lib`, an
  XSPICE `ilimit` block. It does not model BUVLO or the supplement gating,
  and a cell below BUVLO is `vbat = 0`.
- U2 is the TI-derived TPS22917 model, which stays local-only (ADR-006
  D9).
- The W1 reference ran at fallback level 1 (`reltol=1e-3`). At the suite's
  own `reltol=1e-4` it fails with `Timestep too small` on
  `b.xu2.x_u1_u2_u4.beout#branch`, a V-type B branch inside the TI model
  (the ADR-006 Amendment 2 mechanism). At `phase0` with trapezoidal
  integration it did not finish in 90 s. At `phase0` with Gear it
  converged in 5.7 s, within 25 µV and 2.2 µs of the reference (measured).

So every example has three parts:

- **Suite replica (local-only).** The suite's own deck, built through
  Breadboard's deck assembly and `deck_run`, must give `results.json`'s
  values. This checks the harness, not the models. It uses the
  TI-derived U2, so it runs only where `make dms-local-packages` has run,
  as `make dms-crosscheck` does.
- **Static-load mode.** Breadboard's `pi` views with the suite's load
  budget in place of the CPU's `supply_load` (a bench `[[load]] replace`,
  ADR-009 D4), and the suite's averaged LED-cue load on the ring. The
  firmware still runs. This is the like-for-like gate. Its tolerances are
  sized from P0's measured model difference, not asserted in advance.
- **Co-simulation.** The firmware's own state draws the current. This is
  reported, not gated, because the suite's 50 mA is a board budget that
  includes consumers no package models yet (the flash, the sensors, the
  OLED logic).

### W1 (rank 1, Phase 2's definition of done): POF blanking on a dead battery (R8), at the threshold the nRF52840 can be set to

USB power at ILIM 80 mA min, the cell below BUVLO, SYS_REG 4.312 V. The
firmware turns the LED charging cue on (LED_PWR_EN rises, U2 TPS22917
inrush). +3V3 dips. The firmware rule ignores POFWARN for 10 ms after
enabling a load. Without the rule a worst-case part sheds the cue on a dip
that recovers.

**The threshold, re-derived.** firmware.md sets POFWARN at 2.9 V, and the
suite's limit is 2.9 V + VPOFTOL 5 % = 3.045 V. The nRF52840 has no 2.9 V
code in normal-voltage mode: the highest is V28, 2.8 V (PS v1.1; MDK
`nrf52840_bitfields.h:8989`, `POWER_POFCON_THRESHOLD_V28`). Breadboard
refuses codes that do not exist (ADR-009 D6). So W1 runs at V28, and its
tolerance corners are 2.66 V, 2.80 V and 2.94 V. The suite's own W1 deck
against them (measured, `w1_sweep.py`):

| Static +3V3 load | VSYS min | +3V3 min | Below 3.045 V (after LED_PWR_EN) | Below 2.94 V (V28 + 5 %) | Below 2.80 V (V28) |
| --- | --- | --- | --- | --- | --- |
| 30 mA | 3.141 V | 3.115 V | never | never | never |
| 40 mA | 3.024 V | 2.989 V | 3.36-3.89 ms | never | never |
| **50 mA (the suite's)** | 2.9108 V | 2.8666 V | 3.22-5.82 ms | **3.30-4.44 ms** | never |
| 60 mA | 2.800 V | 2.747 V | from 3.12 ms, no recovery | from 3.18 ms, no recovery | from 3.33 ms, no recovery |

Three consequences, each now part of W1:

- At nominal V28, POFWARN never fires. The 10 ms rule and its 5 ms
  predecessor cannot be told apart there. **W1 gates at the VPOFTOL +5 %
  corner (2.94 V)**, the comparator that trips earliest, which is the case
  the rule exists for. The nominal and −5 % corners are reported.
- At 2.94 V the dip ends at 4.44 ms, so the original 5 ms blanking would
  have sufficed. The negative controls are therefore re-derived from the
  measured window (below), and "5 ms fails" is no longer claimed.
- The dip is a cliff in the load: 40 mA does not cross 2.94 V, 60 mA
  collapses. The firmware's own current is a few mA (ICPU0 3.3 mA), so a
  co-simulation with only the MCU modelled will not trip POFWARN at any
  corner. That is why the gated runs use the suite's budget (static-load
  mode with the firmware running), and the co-simulation is reported.

No other POF case in the suite crosses a settable threshold: the battery
floor turn-ons (`test_bat_pin_vs_buvlo_at_firmware_floor`) stay above
2.94 V, the closest being `U21+motor@-10` at 2.942 V (measured,
`floor_probe.py`). So W1 has no fallback scenario inside the suite; P0's
feasibility gate decides before P4 (below).

**The firmware's blanking.** The W1 firmware implements R8 by disabling
POFCON when it raises LED_PWR_EN and re-enabling it when the blanking
ends. The nRF52840 raises POFWARN on enable if VDD is already below the
threshold (PS v1.1; ADR-009 D6). A dip still in progress at the end of the
blanking therefore sheds the cue. This is the W1 firmware's choice, not a
statement of firmware.md.

- **Suite reference:**
  `test_u21_turn_on_dead_battery_usb_100ma_with_fw_rule[ILIM80min-SYSREG4.4-2pct]`
  (+3V3 min 2.8666 V, VSYS min 2.9108 V), and the re-derived window above.
- **Needs:**
  - the BQ25180 power path and the RT9080 as `pi` views (ADR-009 D2);
  - the base board in the deck: a single-MCU system run (ADR-013 D2);
  - the nRF52840 `supply_load` with CPU state outputs (ADR-009 D4);
  - rail-tracking `digital_out` (D5);
  - POFWARN from the rail (D6);
  - assertions (ADR-010);
  - the W1 firmware (milestone W-fw).
- **Checks:**

  | Check | Limit | Reference |
  | --- | --- | --- |
  | Suite replica (local-only) | every W1 value of `results.json` within 1 mV and 10 µs | suite |
  | P0 feasibility at the **stress corner** (D-W1): static-load mode on the `pi` views at the +5 % corner, 55 mA static +3V3 load, RT9080 worst | the stress load is the smallest whole mA with +3V3 below 2.94 V for at least 0.5 ms; +3V3 below 2.94 V for at least 0.5 ms, recovered at least 2 ms before the 10 ms blanking ends (P0: 0.83 ms, 3.255-4.089 ms) | measured window |
  | **Margin check** at the suite's 50 mA (D-W1): static-load mode, RT9080 worst, +5 % corner | no POFWARN: +3V3 never below 2.94 V (P0: min 2.9640 V, margin +24.0 mV); +3V3 min and its margin reported at the nominal and the halved (DC-bias stand-in) +3V3 capacitance (P0: halved −21.2 mV, a 0.38 ms dip inside the blanking). With the firmware in the loop (P4), reported: the blanking-off build's `pofwarn` count (expected 0; the rule build masks the blanking) | ADR-009 Amendment 2 |
  | Static-load mode at the stress corner: +3V3 min, VSYS min, the 2.94 V window's start and end | within the stress corner's own tolerances, fixed from P0's measured difference at 55 mA: 200 mV, 200 mV, 50 µs, 5 ms. The replica is near its cliff there, so the window's end is bounded, not discriminated | suite at 55 mA |
  | Static-load load sweep: 30, 50, 55 and 60 mA | no crossing; no crossing (the margin check: the suite crosses and recovers here, the one verdict that differs, recorded); cross and recover; collapse | table above |
  | Rule firmware (10 ms), static-load at the stress corner, +5 % corner | the cue stays on; **`pofwarn` fired at least once** (so the pass is not vacuous); no `reset` record; +3V3 ≥ 2.7 V (flash VCC min) after 10 ms | firmware.md R8 |
  | Negative control: blanking that ends inside the window (the whole ms nearest its midpoint: 4 ms for the suite's window, and 4 ms for P0's stress-corner window, 89 µs before its end) | fails exactly `cue-held`, the shed caused by the re-enable rule | measured window |
  | Negative control: blanking off | fails exactly `cue-held`; POFWARN within one lattice cell of the window's start | — |
  | 5 ms blanking | reported (the suite predicts a pass) | — |
  | Nominal and −5 % corners, rule firmware, stress corner | reported; `pofwarn` count expected 0 | table above |
  | Co-simulation: `supply_load` only, and `supply_load` plus a bench remainder load up to the 50 mA budget | reported; partition independence and determinism gated | — |

- **The stress corner (approver decision D-W1, 2026-11-15, round 2b;
  ADR-009 Amendment 2).** At the suite's 50 mA, P0's static-load deck
  never crosses 2.94 V. Its +3V3 min is 2.9640 V at the RT9080 worst
  corner, because of the board's 37.6 µF on +3V3, which the suite's ideal
  LDO does not have. So W1's firmware-in-the-loop checks run at the
  smallest whole mA that gives at least 0.5 ms below 2.94 V: 55 mA, at
  the RT9080 worst corner. The stress corner has its own tolerances, and
  the suite's 50 mA becomes a gated margin check. Every W1 report and
  `run.json` labels the stress corner: the bench's name and its
  `[[load]]` (`bench/w1-stress.toml`), and the report's suite names.
- **Deviation, recorded:** the firmware cannot set ILIM and SYS_REG over
  I2C, because Phase 2 has no TWIM bus device (section 3 Out). The bench
  sets them, and the report says so.
- **Finding for the user (not a gate).** firmware.md's 2.9 V cannot be
  set. At V28 the suite's own deck says the 10 ms rule holds with margin
  and 5 ms would have sufficed at the worst corner. Whether firmware.md
  changes is the user's decision, because dial-me-spinner is read-only
  here (Q12).

### W2 (rank 2, conditional): the S4 brown-out reboot loop

USB at ILIM 100 mA with a dead cell. Under the old rule the firmware
enables the OLED boost. VSYS collapses, +3V3 falls below VBOR, the CPU
resets, the boot sequence enables the OLED again, and the loop repeats.

- **Suite reference:** the strict xfail `test_u21_turn_on_dead_battery_usb_100ma`
  (VSYS min 1.4614 V; +3V3 min 1.4172 V against the 1.57 V limit) and the
  passing `..._with_fw_rule[ILIM100-SYSREG4.5]` (+3V3 min 3.2224 V, VSYS
  min 3.2666 V, OLED_VIN held off).
- **Measured for this revision:** in the suite's deck the collapse does not
  depend on the +3V3 load. From 5 mA to 50 mA, VSYS falls to
  1.461-1.481 V and +3V3 crosses VBOR,ON max (1.63 V) at 3.83-4.09 ms.
  The Phase 2 prototype's 1.639 V came from Breadboard's provisional views
  over 60 ms, so a co-simulated crossing is not yet shown.
- **Needs:** W1, plus:
  - BOR and reset from the rail, and the `reset` record (ADR-009 D7);
  - tristate pins during reset (D5);
  - a TPS61040 averaged surge view;
  - the W2 firmware (old rule and rule; W-fw).
- **Checks:**
  - static-load mode against both suite cases, with P0's tolerances;
  - old-rule firmware: at least 2 `reset` records in 200 ms, RESETREAS
    read as 0 after each one (a brown-out leaves no reset reason), and the
    enables fall through their pull-downs during reset;
  - rule firmware: no `reset` record, and +3V3 ≥ 2.7 V.
- **Conditional.** W2 counts in the technical exit only after P5's
  prototype shows both a co-simulated crossing of VBOR with the P1 views
  and a working machine reset under External Control, fresh and pooled
  (ADR-009 D7). If the reset does not work, the exit criterion is the
  fallback stated in section 5: the first brown-out ends the run with a
  `reset` record (`phase = "stop"`), and the deviation is recorded.

### W3 (rank 3, stretch): aligned LED PWM against the peak budget (PI-2), and caliper analog-only

- **dial-me-spinner PI-C.** Suite reference:
  - `test_firmware_capped_sync_pwm_transient[worst-full@3.70]`: VLED min
    3.1885 V, which fails the 3.3 V limit;
  - with the PI-2 rule (`..._with_fw_rule[worst-full@3.70-as-scenario]`,
    0.51 A aligned peak): 3.3018 V, which passes.

  Gates:
  - static-load mode within P0-style measured tolerances of both;
  - the table firmware passes;
  - the uncapped firmware fails exactly the VLED check.

  This needs the averaged-against-switched load gate of ADR-009 D9.
- **caliper (analog-only, optional).** ESP32-S3 Wi-Fi TX bursts played
  back as a current profile through the BQ25180 battery path and an
  averaged TPS63802, from a cold 135 mAh cell (ADR-013 D3: no CPU target,
  because Renode cannot run the ESP32-S3).
  - Checks: VBAT against BUVLO (3.0 V for 60 µs) and IBAT_OCP.
  - caliper has **no SPICE suite** (`docs/simulation.md`: "Not written
    yet"). The references are therefore datasheet figures and a hand
    calculation, and the report says so.
  - The 355 mA TX peak is the figure caliper's `docs/electronics.md`
    quotes from the ESP32-S3 datasheet. It must be verified against that
    datasheet before it is used as a gate input.

**Not in Phase 2:** S5 (SHIP over I2C), anything that needs the firmware
to configure the BQ25180, RF behaviour, and the two-MCU system.

## 3. Scope

**In:**

| Area | What | ADR |
| --- | --- | --- |
| `pi` fidelity | The fidelity dial at `pi`. Regulator and power-path views (`richtek/rt9080` restated from DS9080-09, `ti/bq25180`, `ti/tps61040` averaged, `ti/tps63802` averaged). `pi` declarations on the existing views that are adequate there. A battery bench element. The `supply_load` view kind and `[power] idd`. CPU state outputs. Rail-tracking and tristate `digital_out` at `pi`, with a high-Z leakage. The `power` pin model and package limits as assertions. POFWARN and BOR from the rail on the nRF52840 (`BbNrf52Power`). The `reset` record. Named corners | 009 |
| Benches and tests | `bb-bench/1`: timed actions, CSV playback through an `EXTERNAL` source, current profiles, environment signals, faults, per-probe rates, the `switch` view kind. The bench engine. A closed set of assertions, evaluated deterministically. `assert` v2. `bb-test/0` and `bb test`. JUnit. `bb capture import` (sigrok `.sr`, Saleae binary to CSV) | 010 |
| Run modes | Devices in `bb.toml` (Phase 1 route B, so E3 runs on the project). Single-MCU system runs (both dial-me-spinner boards in one deck). Runs without a CPU target (caliper) | 013 |
| Schema epoch | Every Phase 2 schema major (`bb-part/1`, `bb-pins/1`, `bb-lock/1`, `bb-project/1`, `bb-board/1`, `bb-bench/1`, event stream v2) specified and landed together, with one relock (milestone S) | 009-013 |
| Registry | A static, git-hosted public index. Deterministic archives keyed by tree hash. Minisign-signed snapshots. GitHub attestations. `bb-lock/1` with `source`, `vendor_file` and engine-neutral platform keys. Yanking. Namespace routing for private registries. `bb pkg publish`, `yank`, and `init --from-svd/--from-ibis` | 011 |
| Pipeline | Documents with hashes, claims with page and quote, figure and fit claims, a mechanical claim checker run by the maintainer, a staging channel, a pipeline bot identity, `reviewed_by` from PR approvals, and tiers T0/T1. The first 200 packages, ranked by MPN usage | 012 |
| Kernel | Quiet grants with several producers (E3 from one-cell grants to long grants), behind a measured prototype; digital-only nets | 014 |
| Hardening | An ngspice progress watchdog. Engine-owned `.save`. B-expression lint. Operating-point fail-fast. External Control through a reverse connection. Platform hashes scoped to what a platform loads. The package-path trap. The Renode pin policy. Removal of `--adc-model stock`. The debug-run sentence in `spec/time-grant.md`. A nightly gate tier | 015 |
| Viewer and IDE | A docs page for Surfer, Vaporview and GTKWave. VS Code `tasks.json` and `launch.json` templates for cortex-debug, copied by hand. A small optional web viewer that overlays Breadboard's records | 016 |

**Out of Phase 2** (each named, so it is a decision):

| Item | Where / why |
| --- | --- |
| `si` and `em` rungs, IBIS buffer I/V, extraction, gerbers, PCB geometry | Phase 3 (design doc). The IBIS **scaffolding** (`bb pkg init --from-ibis` emitting pin facts) is in |
| Switching-converter views at `pi` | Measured: about 1.5 h per simulated second for a 2 MHz converter. `pi` views are averaged; switching belongs to `si` or to short windows |
| Multi-MCU simulation, QEMU (ESP32-S3, application processors), Verilator | Phase 3. caliper's CPU is out; its analog-only run is in. New public formats use engine-neutral names so a QEMU backend needs no new lock major (ADR-011 D5) |
| Bus devices (TWIM/EasyDMA, SPIM3), the BQ25180 I2C model, the Python device API, WASM devices | Phase 3 candidates. The Python round trip measured 85.5 µs, 20-25 % of a one-cell grant. WASM needs a sandbox ADR. Registry packages stay **data-only** in Phase 2 (ADR-011 D9) |
| A faithful radio | Renode's nRF radio is not faithful (ADR-008). TX current comes from bench-scheduled activity, labelled as such (ADR-009 D4) |
| Seeded Monte Carlo over tolerances | Named corners are in (ADR-009 D8). Seeded Monte Carlo is a Phase 3 candidate once corners have a gate |
| Long grants for a CPU that is awake and has edges both into and out of the analog engine (`pi` state outputs, the loading SAADC view) with a device beside it | ADR-014 scope. It needs a "sample horizon" promise from the CPU, which is a Phase 3 candidate |
| A Python-free replacement for every `make` gate script | The dms-* checks are ported to `bb test` files (ADR-010 D10). `check_feedback.py` and `check_adc.py` make analytic predictions no assertion kind expresses, and stay scripts |
| A VS Code extension with its own debug type, `bb open --vscode` | Templates copied by hand first (ADR-016). An extension only if users ask for one |
| A sigrok/PulseView live bridge | PulseView cannot open a 1 fs-timescale VCD sensibly (libsigrok's VCD input sets samplerate = 1/timescale). The useful direction, captures into benches, is in (ADR-010) |
| RP2040, STM32 power models, NVMC write timing, `BbNrf52Ppi` / `BbNrf52Timer` | No Phase 2 example needs them. STM32 idd tables wait for a verified datasheet |
| An enterprise or private-registry service | Private registries work as static servers (ADR-011). No hosted service |

## 4. Milestones (ordered; each is gated and committed after its gates)

The Phase 1 rules hold unchanged:

- `make test` stays green;
- every new board or run mode gets an equivalence gate against its
  one-cell reference before it is on by default, and a `make cost-table`
  row;
- `spec/` changes only with its accepted ADR.

Three rules are new (section 7):

- **Gate budgets.** Every milestone states the wall-time budget of its
  `make test` additions and of its full gate. A gate over its budget goes
  to the nightly tier (ADR-015 D11), not into `make test`.
- **Consistency checks are named as such.** When a "reference" builds the
  same partition as the run it checks, the gate is called a consistency
  check, not an equivalence (the E3 lesson, ADR-008 Amendment 12).
- **No vacuous passes.** A negative control must show that the mechanism
  it controls acted (W1's `pofwarn` count is the pattern).

### Track H: hardening and debt (first, because it makes every later gate cheaper or safe)

| # | Milestone | Gate (must pass) | Budget | ADR |
| --- | --- | --- | --- | --- |
| H0 | **Process and baseline.** One `git worktree` per track on ext4 (`~/src/bb-<track>`), each with its own `CARGO_TARGET_DIR`, through `make worktree TRACK=<name>`. A host lock (`flock ~/.cache/bb/measure.lock`) taken by every perf, cost and startup target. The gate-budget table in AGENTS.md. CI caches the Renode tarball and the apt packages. **The runtime baseline:** `make check` on an idle ext4 worktree, and each CI job's wall time, measured N = 3 | The baseline committed in the budget table; two concurrent tracks build without crossing artifacts (a test that builds both and compares crate hashes); cost targets refuse to run while another one holds the lock | — | — |
| H1 | **ngspice robustness.** A deterministic progress watchdog (an error, not a hang). Engine-owned `.save` of exactly the vectors the engine reads. B-expression lint (function table and arity). Operating-point fail-fast | The SAADC stall repro (`e3/board.cir` with the loading view, 100 ms) ends in `NoProgress:ngspice` at 77.43 ms within the budget, with bounded memory. Every committed hash unchanged (`make test`, `make dms-home`, `make dms-vsense`). E3 `typ` peak RSS with `.save` measured over the full 2.7 s first, then gated at that figure + 20 % (from 1.5 GB). A three-argument `min()` in a view is refused by `bb pkg lint` (it segfaults libngspice 42) | +30 s in `make test` | 015 D1-D4 |
| H2 | **SAADC loading-view stall**, a timeboxed spike (1 week) like Phase 1's S1. Candidates in order: VPRE as a per-acquisition value from the engine; qualification under a Gear option set; a view-local restatement of `Brst` | E3's homing with the loading view (`homing_step.elf`, 300 ms) completes. E2's `make dms-vsense` checks still pass. Changed E2 hashes are re-recorded with the reason and the per-sample difference (≤ 1 µV, or justified). If no candidate works in the timebox, the fallback: the view is qualified only for periodic sampling, and `bb run` refuses it on a board where the firmware stops sampling (recorded) (as built, under approver decision D-SNAP, section 10: the cause was the engine's lattice clamp, not the view; the **lattice snap** in `NgspiceEngine`, with `bb run --ngspice-lattice-snap off` to reproduce the stall. `make ngspice-stall`: H1's 77.43 ms stall with the snap off; with it the repro completes 300 ms at 5, 10 and 20 µs and 1 s, and the committed E3 board 1 s (with the snap off it stalls at 502.47 ms). E3 and six of E2's eight cases byte-identical; E2's `sag` and `rec` (on P1's views) and the references of `feedback`, `feedback-open`, `nrf-feedback`, the ADC boards, `adc-kicad`, E1 and `bench` re-recorded, at most 1 µV per sample, two `digital_in` edges moved by 0.285 and 0.284 ns; ADR-015 Amendment 4) | — | 015 D1, 008 |
| H3 | **External Control exposure.** A reverse-connection External Control client: `bb` listens on loopback and Renode connects out (`CreateExternalControlClient`); `bb` admits only the connection whose owning process is its own Renode child. Renode 1.17.0's server has no accept hook (verified in `ExternalControlSocket.cs`) | A local foreign client is refused N/N (N = 20) against both the Renode side (nothing listens) and `bb`'s listener. `make ec-reconnect`, `make ec-long` and `make pool-equiv` unchanged | +10 s | 015 D5 |
| H4 | **Lock churn and the package-path trap.** A platform hash covers the `.repl` and the plugin sources whose types that platform loads, plus the shared ones. `bb open` and `bb run` warn when a lock's `path` differs from the resolved path | Editing `bb_nrf52_saadc.cs` changes no STM32 lock. A modified package beside a board copy is no longer ignored silently. The relock lands in S's commit (as built: `make lock-scope`, `make plugin-scope`, `cli/tests/lock.rs`; ADR-015 Amendment 3) | — | 015 D6-D7 |
| H5 | **Multi-producer quiet grants** (E3 from 270 000 grants). **G0 first** (one week, before the rest is costed): E3 `typ` measured three ways, the committed run, the digital-only MOT nets at one cell (the model change alone), and D1-D4 on a branch | G0: the protocol gain over the model change ≥ 3× in simulation time, or ADR-014 is withdrawn and `dms-home` stays nightly. Then `make dms-home`: every case equal to its one-cell reference except grant records (a true equivalence: the partitions differ); E3 `typ` ≥ 3× faster in simulation than the model change alone, and at most 5 000 grants (from 270 000); no promise broken. A new gate `make multi-producer` on a small synthetic board, in `make test`. The project route (R1) measured and reported (as built, ADR-014 Amendment 1: G0 passed, `make dms-home-g0`, the protocol gain 32.3x in E3 `typ`'s simulation time, the model change alone 1.11x; `typ` 433 grants at bb's defaults; D4's re-recorded `typ`, `worst` and `park` hashes differ only by the MOT_NSLEEP column, every other sample and record identical; `make multi-producer` with the kernel part in `bbdev/tests/multi_producer.rs`; `dms/mechanism@2` gives `@1`'s results on every case, G2; the order of a quiet grant without a producer breaks ties by the static plan order, found by `make device-smoke`; `board-glued.cir` keeps `make ngspice-stall`'s pre-H5 deck) | +30 s (`multi-producer`) | 014 |
| H6 | **Gate tiers and CI runtime.** A nightly CI tier (ADR-015 D11): the `fresh` job (`make test POOL=0`), `ec-long`, `dms-home`, `dms-vsense`, the model-test suites over budget. Split `cargo-test`; cache `make setup`; drop `--adc-model stock` with its references (ADR-015 D9) | Each move's saving measured against H0's baseline and recorded; the nightly tier green twice; section 5 item 5's targets (as built: `.github/workflows/nightly.yml` and `make nightly` run `make test POOL=0`, `ec-long`, `ngspice-stall`, the full `dms-home` and `dms-vsense`; no model-test suite is over its budget, so none moved; CI's `cargo-test` job beside `test`, which runs `make test TEST_SKIP=cargo-test`; the bb-setup action caches the installed Rust, Renode and uv; `--adc-model stock` removed. `make check` 475 s idle against H0's 470 s, target 710 s; the CI jobs by their local equivalents, `make ci-jobs-local`, the slowest `dms-rail` at 528 s under load; the nightly tier green twice locally, 1 775 s and 1 627 s; GitHub confirms on the first push; ADR-015 Amendment 5) | — | 015 D9, D11 |

H0 comes first. H1 and H3 can run in parallel with everything else. H2
needs H1, because the watchdog makes the spike's runs bounded. H5 needs H1
(memory) and is independent of the P track.

### Track S: the schema epoch (one milestone, after H4)

| # | Milestone | Gate | Budget | ADR |
| --- | --- | --- | --- | --- |
| S | **Every Phase 2 schema major, in one reviewed commit.** `bb-part/1` (ADR-009 `power`, `rails`, `supply_load`, `[[analog.corners]]`, the `switch` view kind; ADR-012 `[[document]]`, `claims`, `upstream_license`), `bb-pins/1` (`limits`, `rail`), `bb-lock/1` (ADR-011, engine-neutral platform keys), `bb-project/1` and `bb-board/1` (ADR-013), `bb-bench/1` (ADR-010), event stream v2 (ADR-009/010). The JSON schemas, `bb pkg migrate` for each, and the first-party view renames to `<ns>_<name>_` prefixes (ADR-011 D11). One `make locks`. The `bb-lock/1` migration writes every platform entry with ADR-015 D6's scoped hash, which is H4's relock (H4 keeps the legacy all-sources entries while they match; ADR-015 Amendment 3) | Every committed file migrated gives its committed hashes (decks byte-identical, so every output hash holds); `make packages` and `make test` green; the new keys are accepted but inert until their milestone uses them (as built: decks byte-identical up to the renames, `make schema-epoch`; keys a run would not honour as written are refused, naming their milestone; ADR-011 Amendment 1) | — | 009-013 |

Tracks develop against the epoch's draft schemas in their worktrees, so S
is not on their critical path. S must merge before the first commit that
writes a `/1` file to `main`, and before anything is published (E1). A key
found to be missing after S uses a key that S reserved, or waits for
Phase 3.

### Track B: benches, assertions, `bb test`

| # | Milestone | Gate | Budget | ADR |
| --- | --- | --- | --- | --- |
| B1 | `bb-bench/1` and the bench engine. Phase 1 benches migrate mechanically (S). Timed actions, CSV playback (an `EXTERNAL` source with a cursor), current profiles, faults, the `switch` view kind | Every Phase 1 bench migrated gives its committed hashes byte for byte, since a migrated bench builds the same deck. Playback of a 1 s, 100 ksps CSV in ≤ 2 s of added wall time (inline PWL measured 103 s). A digital playback into an MCU is applied at `ceil_L`, with the same edge times as an equivalent `[[pwl]]`. Partition independence at 10 µs, 1 ms and 10 ms | +40 s | 010 D1-D4 |
| B2 | Assertions and `assert` v2: the closed kinds, analog monitors at solver points, windows | E1's four checks, E2's codes, the shed timing and E3's HOME_N latency written as assertions give **the same verdicts and the same integer values** as `check_dms_rail.py`, `check_dms_vsense.py` and the E3 checks on the committed runs. Every Phase 1 negative control fails exactly as before. The verdicts do not depend on the partition (as built: `make assert-test`, `make assert-gate`, `scripts/check_asserts.py`; the values exact where the scripts and the assertions read the same data, and within stated bounds where the scripts read samples and the assertions solver points: an extremum or a crossing between samples, ADR-010 Amendment 3 A3.6) | +20 s | 010 D5-D7 |
| B3 | `bb-test/0`, `bb test`, `test.json`, JUnit | The `dms-*-test` targets re-expressed as `bb test` files with the same outcomes. `test.json` byte-identical across 10 runs. `junit.xml` read by a GitHub reporter action in CI (checked in the prototype) and valid against the subset schema committed in `spec/` (as built: `examples/dial-me-spinner/tests/{e1,e2,e3}.toml` run by `make dms-rail-test`, `dms-vsense-test` and `dms-home-test`, E2's one-edit variants as committed projects `e2/{none,c5,c5-none}`, a case's image by `bb run --image`; what is not a kind stays a script on `bb test`'s outputs; `make bb-test-gate`: bb test's semantics and negative controls, G9, 10 runs of each file with `test.json` byte-identical, the tenth fresh, `spec/schema/junit-subset.xsd` and `test-result.schema.json`; the reporter is dorny/test-reporter v3.2.0's java-junit, its own parser run locally on every `junit.xml`, its CI step unrun until a push; `make bb-test-cost` for the budget; ADR-010 Amendment 5) | +0 s (replaces scripts) | 010 D8-D9 |
| B4 | `bb capture import` (sigrok `.sr`, Saleae binary v0 to canonical CSV) | Round trip of a synthetic capture, exact to fs after the documented rounding. A real logic capture of the user's choice drives a bench | — | 010 D3 |

B1 can start at once. B2 needs B1. B3 needs B2. B4 is parallel and part of
the adoption exit.

### Track R: run modes

| # | Milestone | Gate | Budget | ADR |
| --- | --- | --- | --- | --- |
| R1 | Devices in `bb.toml` (`[[device]]`, Phase 1 route B) | E3's `step` and `typ` cases on the dial-me-spinner project equal the board-file runs, byte for byte after the documented name mapping (or their own one-cell references and goldens, if the deck's names differ). `make pool-equiv` with the entry. The project's `[analog] nets` leaves the MOT nets out, so H5's long grants apply (reported) | +15 s | 013 D1 |
| R2 | Single-MCU system runs (`simulate = ["main", "base"]`) | E1 on the system project with the bench's power path replaced by the base board's views, cross-checked against the suite's `led_rail_sequence` battery-network deck (VLED within 10 mV; that deck is the reference E1 could only report). Board-level convergence: 120 ms in < 60 s at both engine orders. Equivalence and determinism (as built: `examples/dial-me-spinner/e1-system`, `make dms-e1-system` and `dms-e1-system-test`, `cli/tests/system_run.rs`; a `[[battery]]` at `base:VBAT`, `ti/bq25180` at `suite_compat`, the base capacitors and the harness in the deck; +3V3 stays the suite's ideal LDO, because the reference has it and the RT9080 view's operating point fails at the nominal corner in a `bb run`, an open finding for P4; VLED within 3.164 / 3.219 mV of the battery-network deck at cold / nominal, the cell's R0 doubled fails it at 49.2 mV; 71 against 12 000 grants; ADR-013 Amendment 3) | +30 s | 013 D2 |
| R3 | Runs without a CPU target | The caliper power path, or a synthetic board until caliper's packages exist: 2 s analog-only with playback, partition independent, deterministic, and `run.json` records `targets: []` | +10 s | 013 D3 |

R1 can start at once. R2 needs B1 (the base board's battery is a bench
element) and the P1 views. R3 needs B1 and is part of the adoption exit
(section 5).

### Track P: `pi` fidelity (the definition of done)

| # | Milestone | Gate | Budget | ADR |
| --- | --- | --- | --- | --- |
| P0 | **Numerics and feasibility spike** (1 week, timeboxed, like Phase 1's S1). The W1 and W2 suite replicas through `deck_run`; the first RT9080, BQ25180 and TPS61040 `pi` views in static-load mode at `phase0` and at the suite's fallback set | The suite replicas reproduce `results.json` (1 mV, 10 µs). The option set at which the static-load decks converge is named and qualified (expected: `phase0` with Gear, as measured on the suite deck). The model difference between the `pi` views and the suite is measured and **the static-load tolerances are fixed from it**. W1's feasibility row (section 2) passes at the labelled stress corner the approver chose (D-W1, ADR-009 Amendment 2: 55 mA static +3V3 load at the RT9080 worst corner, the smallest whole mA with 0.5 ms below 2.94 V), with the stress corner's own tolerances fixed from the measured difference at 55 mA; the margin check at the suite's 50 mA passes (no POFWARN at the +5 % corner), with +3V3 min and its margin reported at the nominal and the halved +3V3 capacitance | — (spike) | 009 D2 |
| P1 | `bb-part/1` keys in use (S): `power`, view `rails` and `supply_load`. The `pi` views (`richtek/rt9080` restated, `ti/bq25180` with its `suite_compat` corner, `ti/tps61040`), a battery element, view lint rules, the fidelity dial. **`pi` declarations on the existing views** that are adequate there (generic R/C/L, TPS22917, WS2812C), re-qualified per ADR-006 D6 | Every view qualified with its operating point solved at every qualification corner. **W1 and W2 static-load decks** against the suite with P0's tolerances, and W1's load sweep. The decks are assembled from the package views and run through `NgspiceEngine` with `deck_run`, because the project route needs R2. Every W1 assertion reports `rung: pi`. `fast` runs unchanged byte for byte | +30 s | 009 D1-D3 |
| P2 | Rail-tracking and tristate `digital_out` at `pi`, with the high-Z leakage; `oe` from the `PIN_CNF` direction (or the fallback, ADR-009 D5) | At `fast` every committed hash unchanged. At `pi`: the operating point of every Phase 1 board solves with every `oe = 0`; E1 cold: LSW-3 re-reported with the documented pessimism removed; LED_PWR_EN's high level under a stated load against the PS VOH-versus-IOH figure. Partition independence (as built: `make pi-glue`, `make pi-glue-test` in `make test`, `make pi-oe-cost`; `oe` is `PIN_CNF.DIR` on the nRF52840, exact and within the 5 %, and the fallback `oe = 1` on the STM32 parts, which have no direction hook. `bb run --operating-point`: twenty Phase 1 decks solve at `pi` with every `oe = 0` (board files with a stand-in rail; benches made `pi` with `main:U1` excluded). E1 cold at `pi` (`bench/cold-pi.toml`): LSW-3 0.000 V against 0.320 V at `fast`, the wrong order still fails exactly LSW-3 (0.777 V); LED_PWR_EN 2.8217 V at VDD 2.9273 V while sourcing 0.528 mA (`bench/cold-pi-ioh.toml`: the PS's 0.5 mA VOH,SD condition plus R3), within VDD − 0.4 V; one-cell, 1 ms and default grants equal, `tests/golden/pi-glue`; ADR-009 Amendment 5) | +15 s | 009 D5 |
| P3 | `supply_load`, `[power] idd` for `nordic/nrf52840-qiaa`, CPU state outputs `#run`, `#hfxo`, `#nvmc`, `#saadc`, bench `[[activity]]` for radio TX | Renode-only test: `#run` edges at the WFI instants of a gate firmware, exact to fs. The total VDD current of a gate firmware with a known WFI duty equals a hand calculation of the duty-weighted average, within 1 %. Lookahead: the nRF feedback board with state outputs still takes at most 2× its grants. Equivalence against one-cell (as built: `make pi-idd`, `make pi-idd-test` in `make test`, on `examples/nrf-idd`; the state outputs from `bb_nrf52_state.cs`, mapped only when bound; Renode 1.17.0 hooks a WFI entry a slice late, so `#run` falls at the instruction count's instant. `nrf_state_outputs_exact`: every rise at an END wake equals the SAADC's END exactly and every fall is exactly 9 or 7 instructions after its wake, 119 cycles; `nrf_duty.elf` 0.977976 mA against the hand 0.977300 mA, +0.069 %, and with a bench-scheduled radio −0.158 %; the nRF feedback firmware 120 grants with the state outputs against 120 without; one-cell, 1 ms and default equal, `tests/golden/pi-idd`; `#nvmc` draws nothing, no PS figure; P2's `pi` references re-recorded, U10's load moves +3V3 by at most 0.032 mV; ADR-009 Amendment 6) | +30 s | 009 D4 |
| W-fw | **The W1 and W2 firmware.** Zephyr v4.5.0 apps in `examples/dial-me-spinner/firmware/`: the W1 app with POFCON at V28 and the blanking as a build option (10 ms rule, the window-midpoint control, 5 ms, off); the W2 old-rule and rule apps. Prebuilt images, `SHA256SUMS`, `PROVENANCE` | `make fw-rebuild` reproduces every image hash in a fresh west workspace. Each app's POWER and GPIO configuration checked by a Renode-only test on the P4 model (as built: `w1_cue` gives `w1_rule`, `w1_blank4`, `w1_blank5`, `w1_blank_off` from `W1_BLANK_MS`, the blanking on TIMER1 from the LED_PWR_EN edge; `w2_oled` gives `w2_rule`, `w2_old_rule`; `make dms-wfw-test`, `bb_wfw_test.py`, runs on a POWER test double, `power_double.py`, because P4's model comes after W-fw: PIN_CNF, POFCON at V28, each blanking's start and end, and the cue held (10, 5 ms) or shed (4 ms, off) with the stress window as the comparator input; P4 re-runs it on `BbNrf52Power`; ADR-009 Amendment 7) | — | 008 D5 |
| P4 | POFWARN: `BbNrf52Power` (POFCON, EVENTS_POFWARN, the IRQ), the comparator view, `#pofwarn` | Register and timing tests (ADR-005 test-plan shape): every threshold code, hysteresis, the "already below at enable" rule, non-existent codes refused. W-fw's Renode-only test (`make dms-wfw-test`) on the model in place of its test double (ADR-009 Amendment 7). **W1 with the firmware in the loop and its negative controls** at the stress corner, and the margin check (section 2). `make pool-equiv` entry (as built: `BbNrf52Power` wraps the sealed `NRF_CLOCK` on `nrf52840-bb@4`, whose `.repl` inlines the stock entries; `#pofen`/`#pofthr0..3` and the model's events through the state block; the view `nordic/nrf52840-qiaa:pof`, kind `pof_comparator`, its output linear in VDD and the hysteresis in `#pofwarn`'s `digital_in` thresholds, glue rule 13; `make nrf-power-model` and `make dms-w1-test` in `make test`, `make dms-w1` the gate on `examples/dial-me-spinner/w1`; the rule holds the cue, both controls fail exactly `cue-held`, the margin check has no POFWARN; the anti-vacuity check is `pof-dip` with the paired control's evidence, because the W-fw firmware blanks by disabling POFCON, so no POFWARN can fire inside the blanking; static-load mode's load collapses below 0.5 V at `pi`, ADR-013 Amendment 3's operating-point finding; ADR-009 Amendment 8) | +40 s | 009 D6 |
| P5 | BOR and reset, the `reset` record, RESETREAS, GPIO high-Z during reset | Prototype first (one week): Renode 1.17.0 machine reset under External Control, fresh and pooled (risk R3), and a co-simulated crossing of VBOR with the P1 views. **W2**, or its fallback (section 5). Pool equivalence, and a pool process is never reused after a reset that failed | +40 s | 009 D7 |
| P6 | (stretch) W3: averaged vs switched PWM load gate, PI-C; caliper analog-only | Section 2 W3 | — | 009 D9, 013 D3 |

P0 can start at once. P1 needs P0's tolerances. Its static-load checks are
scripts until B2 lands, then they are ported. R2 repeats them on the system
project. P2 can run beside P1. P3 needs P2. W-fw can start when P0 has
fixed the control blanking, and needs P4's model for its Renode test. P4
needs P3, R2, B2 and W-fw. P5 needs P4. W1 is P4's gate and W2 is P5's.

### Track E: ecosystem

| # | Milestone | Gate | Budget | ADR |
| --- | --- | --- | --- | --- |
| E0 | Approver decisions, round 2 (section 9: Q9-Q11) | Sign-off (**given 2026-11-15**, with round 1; section 10) | — | 011, 012 |
| E1 | The registry repository and its CI. `bb-lock/1` (from S), fetch in `bb open`, minisign verification, yanking, namespace routing, `vendor_file`, reading every published major | End to end: publish, resolve, offline run. A tampered archive is refused. A rolled-back snapshot is refused. A yanked version is skipped on a new resolve and kept, with a warning, in an existing lock. A routed namespace never falls through to the public registry (dependency-confusion test). Deterministic archives: the tree hash is recomputed after extraction. **`make test` and `make packages` pass with the network disabled** (as built, against local fixtures only: `make registry-test` in `make test`, `make registry-gate` with G7 on the local registry repository `~/src/bb-registry`, `make test-offline` for G8; the vendor file is declared by the reserved `[behavioral] svd`; the human's publishing step is `docs/registry-publish.md`; ADR-011 Amendment 2) | +20 s (local registry fixture) | 011 |
| E2 | The pipeline: `[[document]]`, `claims.toml` with printed, figure and fit claims, the claim checker and its signed result, the staging channel, the bot identity, `reviewed_by` from approvals | Negative controls: a changed number, a wrong page, a quote that is absent, a wrong document hash, a changed digitised point and a changed fit residual each give `unverified`. A view cannot be published as `datasheet_checked` with one `unverified` claim. An agent PR approved by the bot identity is refused. 10 pilot packages from dial-me-spinner and caliper MPNs | — | 012 |
| E3 | The first 200 packages, ranked by MPN usage (ADR-012 D2), with T0 pins-only for most of them | 200 in the index, all CI-verified. ≥ 50 with `reviewed_by` from a PR approval. 0 `datasheet_checked` without passing claims. Coverage ≥ 90 % of the distinct MPNs on dial-me-spinner and caliper | — | 012 |
| E4 | Viewer and IDE: the docs page, the VS Code templates; optional site viewer | The templates attach to `bb run --gdb` in VS Code with cortex-debug (prototype tested with gdb-multiarch only). The run stays byte-identical when nothing stops it | — | 016 |
| E5 | Adoption: a candidate survey against the supported targets, then 5 external public boards, a stranger `pi` demo, the preprint | Section 5 | — | — |

E1 needs E0 and S. E2 can start with E1, against a local registry fixture.
E3 needs E2 and **human review time** (section 8). E4 is independent. E5
needs B3, R3 and E3.

### Dependency summary

```
H0 ─┬─ H1 ─┬─ H2
    │      └─ H5 (G0 first)
    ├─ H3, H6                                  (parallel)
    ├─ H4 ─ S ──────────────────────────────┐  (S before any /1 file on main, and before E1)
    ├─ B1 ─ B2 ─ B3 ─────────────────────┐  │
    │    └─ R3 ──────────────────────────┤  │
    ├─ R1                                │  │
    ├─ P0 ─ P1 ─ P2 ─ P3 ─ P4 ─ P5 ─ P6  │  │
    │    │    └─ R2 (needs B1) ┘ (P4 needs R2, B2, W-fw)
    │    └─ W-fw ──────────┘             │  │
    └─ E0 (round 2) ─ E1 ─ E2 ─ E3 ──────┴──┴─ E5
       E4 (parallel)
```

Up to five tracks run in parallel (H, B/R, P, E, and one reviewer), each in
its own worktree (H0). They merge to `main` serially, and every merge
re-runs `make check` on the merged tree.

## 5. Definition of done (the Phase 2 exit)

### Technical exit (agent-reachable; this is the gate for Phase 3)

1. **W1 passes at `--fidelity pi` through `bb test`.** On the
   dial-me-spinner system project, at V28 and the VPOFTOL +5 % corner, in
   static-load mode at the labelled stress corner (55 mA static +3V3 load,
   RT9080 worst; D-W1, ADR-009 Amendment 2), with the firmware in the
   loop:
   - the static-load checks match the suite within the stress corner's
     tolerances, and the suite replica passes where the local-only
     package exists;
   - the margin check at the suite's 50 mA passes (no POFWARN at the +5 %
     corner), and +3V3 min and its margin are reported at the nominal and
     the halved +3V3 capacitance;
   - every W1 report and `run.json` labels the stress corner;
   - the rule firmware keeps the cue, and `pofwarn` fired;
   - both negative controls fail exactly `cue-held`;
   - the co-simulation is reported;
   - JUnit is published by CI;
   - the run equals its one-cell reference except grant records, and is
     byte-identical across 10 runs.
2. **W2, if P5's prototype passes:** the same way, with at least 2 `reset`
   records. **Otherwise** the fallback: the old-rule firmware's first
   brown-out ends the run with a `reset` record (`phase = "stop"`) within
   one lattice cell of the static-load VBOR crossing, the rule firmware
   has none, and the deviation is recorded.
3. **The dms-* boards' checks run as `bb test` files** (E1, E2 and E3 of
   Phase 1), with the same verdicts and values as `check_dms_rail.py`,
   `check_dms_vsense.py` and the E3 checks (B2/B3). `check_feedback.py`
   and `check_adc.py` stay scripts. `fast` results are unchanged byte for
   byte.
4. **A stranger `pi` demo:** the README quickstart gains a W1 step. The
   stranger test (`make stranger-test-container`) stays under 15 minutes.
5. **Hardening and runtime:**
   - no known unbounded hang (H1);
   - External Control refuses foreign clients (H3);
   - ADR-014 accepted with its G3 met, or withdrawn by G0 with
     `dms-home` in the nightly tier (H5: G0 passed and G3 met, ADR-014
     Amendment 1);
   - `make check` on an idle ext4 worktree ≤ H0's baseline + 4 minutes,
     and the slowest CI PR job ≤ 15 minutes, with everything moved to
     the nightly tier named and its saving measured (H6). Section 8
     shows the arithmetic.
6. **Registry live:**
   - signed snapshots and `bb-lock/1`;
   - every first-party package published through it;
   - `bb open` resolves dial-me-spinner from the registry with a cold
     cache, and offline with a warm one;
   - `make test` passes with the network disabled.

### Adoption exit (needs people outside the project; tracked, and a miss is reported, not hidden)

The design doc's numbers, adapted:

| Design doc | Phase 2 exit | Why the change |
| --- | --- | --- |
| 10 external projects simulating their own boards | **5 public boards not designed by the user**, each imported with `bb open`, each with at least one green `bb test` assertion in CI, listed on the docs site; **at least 2** run by someone other than the user (a public issue, PR or discussion that quotes the `run.json` hash). A board counts with a supported CPU target (STM32F103C8, STM32F072, nRF52840) **or as a no-target run** (R3) whose assertions are on its analog side. caliper counts as an internal sixth board. E5 starts with a candidate survey: the boards are chosen against the supported targets before the number is committed to | One person plus agents can port 5 boards. Whether others run them is not in the project's control, so the number is small and evidenced. The most-used MCUs (ESP32-S3, RP2040) are out of Phase 2, so no-target runs are what make 5 reachable |
| 500+ packages | **≥ 200 packages** in the public index, all CI-verified; **≥ 50 human-reviewed** (`reviewed_by` from an approval); **0** `datasheet_checked` without hashed documents and passing claims; **≥ 90 %** of the distinct MPNs on dial-me-spinner, caliper and the 5 external boards resolve to a package (generic packages count) | KiCad's library cannot rank parts: 7 of 22 808 symbols carry an MPN. Review is the bottleneck (section 8). Coverage of real boards is the useful measure; a raw count invites unreviewed volume |
| A published paper or talk with reproducible results | **One public preprint** (arXiv, or a JOSS submission) whose every figure is rebuilt by a make target from a tagged commit, as `make docs-plots-check` does today. A talk submission is optional | A preprint has no deadline that the plan cannot control. FOSDEM 2027's devroom calls opened around 27 October 2026 |

The adoption exit also carries R3, B4 and E2-E5. P6 (W3) is a stretch and
in neither exit.

Phase 3 may start on the technical exit. The adoption exit is reported at
every milestone review.

## 6. Risks

| # | Risk | Evidence | Mitigation |
| --- | --- | --- | --- |
| R1 | **`pi` decks stall or fail their operating point** | A flat B-source region made the DC operating point fail after 31 s; the SAADC view stalls in quiet stretches; Phase 1's S1 and M6 stalls; **the suite's own W1 deck fails at `reltol=1e-4` and does not finish at `phase0` with trapezoidal integration** (measured) | P0's numerics spike before any tolerance is fixed. View rules plus lint (ADR-009 D2): a conductance at the operating point (also in the high-Z state), B conductances and no ammeters, averaged switchers. Operating points at every corner in `bb pkg test`. The H1 watchdog turns a stall into an error. A board-level convergence gate per worked example |
| R2 | **idd fidelity, and loads no package models** | Datasheet typ figures are state-resolved, CoreMark-based and taken at 3 V on the DC/DC. Scaling over VDD is not verified. W1's dip needs about 45-55 mA on +3V3 (measured cliff); the nRF52840 alone draws a few mA | The gated runs use the suite's budget (static-load mode). Co-simulation is reported, with the remainder load labelled. idd confidence tops out at `self_checked` plus `reviewed_by` unless ADR-012's figure claims pass. The upgrade path is a `measured` table (for example a PPK2) |
| R3 | **Renode reset under External Control is unknown** | Not verified; pooled and fresh behaviour may differ | P5 starts with a prototype. The fallback exit criterion is stated in section 5 item 2 |
| R4 | **POWER and CLOCK share one peripheral** | Renode's `NRF_CLOCK` sits at the POWER/CLOCK base (0x40000000) and has no POFCON, RESETREAS or GPREGRET (checked on master) | `BbNrf52Power` wraps or replaces it in Breadboard's overlay (a platform major bump). Prototype it first in P4 |
| R5 | **A busy CPU in a feedback loop takes one-cell grants** | ADR-003 D5. idd and POF make CPU and ngspice a loop. ADR-014 does not help a CPU that is awake with edges both ways (its scope) | A sleeping CPU's promise covers its state outputs. W1/W2 are mostly sleep plus boot. A cost row per example; the budget is in the gate |
| R6 | **Licences block the registry or the views** | KiCad libraries are CC-BY-SA-4.0 (not OSI). ST SVDs come under an EULA. ADI IBIS files are per-model. TI models stay local-only. dial-me-spinner has no LICENSE | Data allowlist plus `vendor_file` fetch-at-use (ADR-011 D7). The RT9080 view is restated from the datasheet, not migrated, so it needs no grant. Datasheet-fitted views by default |
| R7 | **Agent-written packages are subtly wrong** | Phase 1: TPS22917's `refs` named a datasheet that was never opened (ADR-006 Amendment 5). The implementer's checks were too weak on E1 and E3 | Mechanical claim checks; `reviewed_by` only from approvals of a bot-authored PR; a reviewer agent writes the negative controls (section 7) |
| R8 | **Review hours** | Estimate: 15-30 min per pins-only package, 1-3 h per analog view, so 50-150 h for 50 reviews | Tiers. T0 first. The review queue is ordered by coverage gain. The exit counts reviewed packages, not packages |
| R9 | **No external users appear** | The repository has had none | The adoption exit is separate and reported honestly. Port 5 public boards ourselves with their authors' consent; announce through KiCad and Zephyr channels |
| R10 | **Renode 1.17.0 stays the pin while upstream moves** | 1.17.0 (2026-09-07) is the latest release. 4 342 lines of plugin C# reflect on internals | Stay pinned. Any upgrade needs an ADR and the lookahead and ADC gates (AGENTS.md). Evaluate when a release fixes the EC leak (ADR-015 D8) |
| R11 | **Host contention and `/mnt/c` distort measurements and gates** | Cost rows at load 4-12; `bb run --elaborate-only` 1.9-4.8 s on `/mnt/c` against 14-16 ms on ext4 | H0: ext4 worktrees, a measurement lock and a measured baseline |
| R12 | **An in-process ngspice crash takes `bb` down** | A three-argument `min` segfaults libngspice 42 | Lint for registry views (H1). An out-of-process ngspice for untrusted views stays deferred (ADR-015) |
| R13 | **Documentation volume** | 15 486 lines of ADRs and specs; ADR-008 is 2 827 lines | Each Phase 2 ADR opens with a one-screen summary. An "as implemented" amendment is at most about 150 lines, and the details live in the spec (section 7) |
| R14 | **Approver load** | Phase 1 was bound by the approver's rounds; the eight ADRs opened 36 questions | 12 consolidated questions in two rounds (section 9); every other ADR question has a recorded default that applies unless the approver objects |
| R15 | **Schema churn strands registry content** | Five majors change across milestones; spec §10 reads only two majors | Milestone S lands them together; registry content is read at every published major (ADR-011 D12) |

## 7. Process changes learned from Phase 1

1. **One worktree per track, on ext4, with its own `CARGO_TARGET_DIR`.**
   - Phase 1's M11 had to move to a separate worktree, because M10's
     uncommitted package changes made every lock stale.
   - Two checkouts sharing a target directory link each other's crates.
   - `/mnt/c` multiplies file-heavy steps by 30 to 300 times.
   - The Windows checkout stays the place where commits land.
2. **Gate runtime budgets and two tiers.**
   - The PR tier (`make check`, CI on a PR) has a budget relative to H0's
     measured baseline (section 5 item 5). A gate over its budget goes to
     the nightly tier.
   - A gate that overruns its budget by more than 25 % is a review
     finding.
   - Phase 1's `make check` doubled from 7 to 14 min without anyone
     deciding it.
3. **Measurements hold a host lock.** No cost row, perf gate or startup
   gate runs while another one does. Phase 1's rows at load 4-12 had to be
   measured again.
4. **The reviewer writes the negative controls, and each one proves its
   mechanism acted.** In Phase 1 the implementer's checks were
   systematically too weak:
   - E1's FW-6.4 accepted a 5 ms wait;
   - a firmware stepping 7 times too fast and the wrong DRV5032 variant
     both passed every E3 check.

   This review found the same pattern in this plan's first draft: W1's
   negative controls could not fail at the threshold the hardware has.
   Every milestone review now adds at least one negative control the
   implementer did not write, with a check that the controlled event
   occurred.
5. **Provenance review for agent-written models** (ADR-012). An agent may
   write a view, but only a person's approval of a bot-authored PR sets
   `reviewed_by`, and only the claim checker allows `datasheet_checked`.
6. **Consistency is not equivalence.** A gate whose "reference" builds the
   same partition as the run it checks is called a consistency check.
7. **Shorter ADRs.**
   - A summary first.
   - Amendments record decisions and deviations; the details go into the
     spec.
   - A stale row is fixed in the same commit that makes it stale.
   - A spec statement that contradicts the code is a bug. For example,
     ADR-006 D3 says the TPS22917 `lsw` view carries `["fast", "pi"]`,
     while `packages/ti/tps22917/part.toml` declares `["fast"]`. P1's
     re-qualification fixes it.
8. **Lock churn is designed out, not reviewed away** (H4, S). In Phase 1,
   11 of 83 commits touched `bb.lock`.
9. **Batch pushes stay gated locally.** CI is a second line of defence. Its
   first-run results are reported at each milestone.
10. **A reference is read before it is trusted.** The suite's W1 deck uses
    an ideal LDO, not the RT9080 model, and ran at a loosened option set.
    Every Phase 2 cross-check states which models and options its
    reference used.

## 8. Cost and time estimate

**Units.** Agent-weeks on Phase 1's plan scale. Phase 1's plan estimated
M0-M11 at about 21 agent-weeks. They were delivered in 6 calendar days
(2026-11-09 to 2026-11-14, `git log`), with several agents in parallel. So
agent effort was not what bound Phase 1. Three things did:

- gate wall time;
- host contention;
- the approver's review rounds.

The same holds here, and two of Phase 2's constraints cannot be
parallelised at all: human package review and external users.

| Milestone | Agent-weeks | Exit | Note |
| --- | --- | --- | --- |
| H0 | 0.5 | technical | scripts, CI cache, AGENTS.md, the baseline |
| H1 | 1 | technical | watchdog, `.save`, lint, op fail-fast |
| H2 | ≤ 1 | technical | timeboxed |
| H3 | 1.5-2 | technical | a new reverse-connection EC client, its reconnect logic re-proven by `ec-long` and `pool-equiv` |
| H4 | 0.5 | technical | one hash rule; the relock lands in S |
| H5 | 0.5 (G0) + 2-3 | technical | the 2-3 only if G0 passes |
| H6 | 0.5 | technical | the nightly tier, measured savings |
| S | 1 | technical | every schema major, migrations, renames, one relock |
| B1 | 2 | technical | schema use, engine, playback, the `switch` kind |
| B2 | 2 | technical | monitors in ngspice, windows, porting the dms-* checks |
| B3 | 1 | technical | `bb test`, JUnit |
| R1 | 1 | technical | |
| R2 | 1.5 | technical | bigger deck, convergence |
| P0 | 1 | technical | timeboxed spike |
| P1 | 3.5 | technical | four views, battery, lint, static-load gates, `pi` declarations on existing views |
| P2 | 1 | technical | glue, leakage, `oe`, the `fast` identity gate |
| P3 | 2 | technical | WFI/clock hooks, idd table, lookahead check |
| W-fw | 1.5 | technical | W1 app in four builds, two W2 apps, prebuilt, `fw-rebuild` |
| P4 | 2 | technical | power model, comparator, W1 |
| P5 | 2-3 | technical | reset semantics, pool, W2; prototype-dependent |
| E1 | 2.5 | technical | registry repo, CI, client, all-majors read |
| **Technical exit** | **31.5-34** | | H 7.5-9, S 1, B1-B3 5, R1-R2 2.5, P0-P5 and W-fw 13-14, E1 2.5 |
| R3 | 0.5 | adoption | |
| B4 | 0.5 | adoption | |
| E2 | 2.5 | adoption | claims with figure and fit kinds, signed results, bot identity |
| E3 | 4 (agents) **+ 50-150 h human review** | adoption | 200 packages |
| E4 | 0.5-1.5 | adoption | templates; viewer optional |
| E5 | 2 (agents) + calendar | adoption | candidate survey, porting 5 boards, the preprint |
| **Adoption exit** | **+10-11 agent-weeks, 50-150 h of review, and calendar time for outside users** | | |
| P6 | 1.5 | stretch | W3; in neither exit |

**Calendar projection (uncertain).** If Phase 1's ratio held, the
technical exit would take three to five calendar weeks with four or five
parallel tracks. That includes **one calendar week for each approver
round** (section 9): round 1 before H, P0 and B1 start; round 2 before E1.
The approver's answer to Q1 after P0 can add a third, short round.

The adoption exit cannot be projected from agent effort. Package review is
the critical path. At 10 hours of review a week it needs 5 to 15 weeks.
The design doc allows months 5-10 for Phase 2 (about 24 weeks), which
covers it.

**Run cost.**

- **W1:** the prototype ran E1-sized `pi` decks at 3.7-4.6 s per 120 ms
  with the motor averaged, and 18 s with 25 kHz PWM resolved. The suite's
  W1 deck took 5.7 s at `phase0` with Gear over 15.2 ms. W1 at its
  15-120 ms should be under 20 s per case.
- **`make test` arithmetic.** The milestone budgets add about 360 s (H1
  30, H3 10, H5 30, B1 40, B2 20, R1 15, R2 30, R3 10, P1 30, P2 15, P3
  30, P4 40, P5 40, E1 20). H5 does not offset them: `make test` runs only
  `dms-home-test`, on `board-step.cir`, which keeps its pull-downs. The
  offsets are named instead (H6): the CI `fresh` job (13:58, the slowest
  job) moves to nightly; `adc-stock` and its references go (ADR-015 D9);
  `cargo-test` is split. Each saving is measured against H0's baseline
  before the exit target is checked. The target is therefore the baseline
  + 4 minutes, not a fixed 10, and if the named savings fall short, more
  gates move to nightly by name rather than the target moving.
- No paid infrastructure. The registry is on GitHub Pages, and Actions
  minutes on public repositories cost nothing (`billable.UBUNTU.total_ms
  = 0` on the latest run). Cloudflare R2's free tier is the fallback.

## 9. Questions for the approver (consolidated)

Twelve questions, most important first, in two rounds. Every other
question in the ADRs now has a **recorded default** that applies unless
the approver objects (each ADR lists them).

**Round 1 (before H, P0 and B1 start):**

1. **W1 at the threshold that exists.** Gate W1 at V28 with the VPOFTOL
   +5 % corner (2.94 V), with the firmware in the loop in static-load mode
   (the suite's 50 mA budget), and negative controls derived from the
   measured window (blanking off, blanking ending inside the window);
   co-simulation reported. If P0's feasibility row fails on the `pi`
   views: raise the static load to the smallest whole mA at which the
   deck crosses 2.94 V for 0.5 ms, labelled a stress corner.
   **Recommended: yes**, with that fallback.
2. **The exit split, and what is conditional.** The technical exit is
   Phase 3's gate; the adoption exit is tracked and reported. W2 counts
   fully only if P5's prototype passes, else its run-ending fallback.
   E3's runtime is not an exit item (ADR-014 is accepted or withdrawn by
   G0). **Recommended: yes.**
3. **The schema epoch** (milestone S): every Phase 2 schema major in one
   reviewed commit with one relock, before any `/1` file lands and before
   the registry publishes. **Recommended: yes.**
4. **ADR-014 behind G0**, with one threshold (≥ 3× protocol gain over the
   model change), and E3's homing boards' MOT nets made digital-only (a
   recorded model change that re-records their hashes). Its scope excludes
   awake CPUs with edges both ways. **Recommended: yes.**
5. **Single-MCU system runs in Phase 2** (ADR-013 D2). W1 needs the base
   board's power path. **Recommended: yes**, with the bench fallback
   recorded in case R2 slips.
6. **Benches and checks.** TOML `bb-bench/1` (not `bench.yaml`), a closed
   set of assertion kinds extended by amendment, and only the dms-* checks
   ported (`check_feedback.py` and `check_adc.py` stay scripts).
   **Recommended: yes.**
7. **External Control in Phase 2**: build the reverse-connection client
   (H3, 1.5-2 agent-weeks), not the accept hook (which 1.17.0 does not
   have). If H3 slips, the exposure stays documented as in ADR-004.
   **Recommended: build it**, because the registry brings people who run
   `bb` on shared machines.
8. **Runtime targets relative to a measured baseline**, with a nightly
   tier that takes the `fresh` job, `ec-long`, `dms-home` and `dms-vsense`.
   **Recommended: yes.**

**Round 2 (before E1):**

9. **Registry basics:** the licence policy (OSI, plus CC0-1.0, CC-BY-4.0
   and CC-BY-SA-4.0 for data-only files; vendor files fetched at use), one
   new crate `minisign-verify` (0 dependencies), `curl` as the fetcher,
   GitHub Pages hosting, and the search order with first-party before
   the public registry. **Recommended: yes.**
10. **Review identity:** agent PRs are authored by a dedicated pipeline
    bot account; the user approves them; claim checks run on the
    maintainer's machine and are committed as a signed result that CI
    verifies; no self-hosted runner ever runs registry PRs.
    **Recommended: yes.**
11. **Figure and fit claims:** values read from datasheet figures or
    fitted to them become claims with digitised points, a fit record and
    a reviewer sign-off; without them a `pi` view tops out at
    `self_checked` plus `reviewed_by`. **Recommended: yes.**

**For the user (dial-me-spinner, read-only here):**

12. **The POFWARN finding.** firmware.md's 2.9 V cannot be set (V28 is the
    highest code). At V28 the suite's own deck shows the 10 ms rule holds
    and 5 ms would have sufficed at the worst corner. Report it to the
    user as a dial-me-spinner issue; Breadboard refuses codes that do not
    exist. **Recommended: yes.**

## 10. Approver decisions (2026-11-15)

Both rounds were answered together. **Every question of section 9 was
accepted with its recommended answer.** No objection was made to any
ADR's recorded defaults, so they apply as written (each ADR's "Approver
decisions" section lists them).

| Q | Decision | ADR |
| --- | --- | --- |
| 1 | **Accepted.** W1 is gated at V28 and the VPOFTOL +5 % corner (2.94 V), firmware in the loop, in static-load mode (the suite's 50 mA budget), with the two window-derived negative controls; the co-simulation is reported. The stress-corner fallback is accepted as stated. Invoking it stays the approver's call on P0's report (P0 row): an implementer whose feasibility row fails stops and reports | 009 |
| 2 | **Accepted.** The technical exit is Phase 3's gate; the adoption exit is tracked and reported. W2 counts fully only if P5's prototype passes, otherwise its run-ending fallback (section 5 item 2). E3's runtime is not an exit item | 009, 014 |
| 3 | **Accepted.** The schema epoch (milestone S): every Phase 2 schema major in one reviewed commit with one relock, merged before any `/1` file lands on `main` and before anything is published | 009-013 |
| 4 | **Accepted.** ADR-014 behind G0, with the one threshold (≥ 3× protocol gain over the model change in simulation time), E3's homing boards' MOT nets made digital-only as a recorded model change, and the scope excluding awake CPUs with edges both ways | 014 |
| 5 | **Accepted.** Single-MCU system runs in Phase 2, with the bench fallback recorded if R2 slips | 013 |
| 6 | **Accepted.** TOML `bb-bench/1`, a closed set of assertion kinds extended by amendment, and only the dms-* checks ported | 010 |
| 7 | **Accepted.** The reverse-connection External Control client is built in Phase 2 (H3); if H3 slips, the exposure stays documented as in ADR-004 | 015 |
| 8 | **Accepted.** Runtime targets relative to H0's measured baseline, and a nightly tier with the `fresh` job, `ec-long`, `dms-home` and `dms-vsense` | 015 |
| 9 | **Accepted** (round 2). The licence policy, `minisign-verify`, `curl`, GitHub Pages hosting, and the search order with first-party before the public registry | 011 |
| 10 | **Accepted** (round 2). A pipeline bot identity, claim checks run on the maintainer's machine and committed as a signed result that CI verifies, no self-hosted runner for registry PRs | 012 |
| 11 | **Accepted** (round 2). Figure and fit claims, and the confidence ceiling without them | 012 |
| 12 | **Accepted.** The POFWARN finding is reported to the user as a dial-me-spinner issue; Breadboard refuses threshold codes that do not exist | 009 |

**E0 is signed off** by these answers, so E1 is not waiting on round 2.

**Round 2b (2026-11-15), on P0's report: D-W1.** P0's feasibility row
failed at 50 mA, and the approver invoked Q1's fallback with a margin
check added:

- W1 is gated at a labelled stress corner: 55 mA static +3V3 load at the
  RT9080 worst corner, the smallest whole mA giving at least 0.5 ms below
  2.94 V (0.83 ms, 3.255-4.089 ms);
- its tolerances are its own, fixed from P0's measured model difference
  at that load (the replica is near its cliff there);
- a gated margin check at the suite's 50 mA: no POFWARN at the +5 %
  corner, with +3V3 min and its margin reported at the nominal and the
  halved (DC-bias stand-in) +3V3 capacitance;
- every W1 report and `run.json` labels the stress corner.

This is ADR-009 Amendment 2, and it is in section 2 (W1), section 4 (P0)
and section 5 (item 1).

**Round 3 (2026-11-16), on H2's report: D-SNAP.** H2's spike found the
SAADC stall's cause in `NgspiceEngine`'s lattice clamp (a sub-femtosecond
step after TMAX steps drift off the lattice, then a trapezoidal Newton
limit cycle), and stopped at an approval point because the fix changes
committed references. The approver approved the **lattice snap** as H2:

- `bb run --ngspice-lattice-snap off` and a `run.json` key (only when
  off) keep the stall reproducible for H1's gate; the `spec/project.md`
  line for the key is approved;
- `make ngspice-stall` adds H2's completion runs (300 ms at 5, 10 and
  20 µs, and a 1 s E3 run that completes);
- the changed references are re-recorded with this reason and their
  measured per-sample differences (at most 1 µV per sample), with the
  0.285 ns edge recorded.

This is ADR-015 Amendment 4, and it is in section 4 (the H2 row).

**This build's scope.** Hardening and the technical exit:

- **In:** H0-H6, S, B1-B3, R1-R2, P0-P5, W-fw, E1 and E4.
- **Out of this build** (not started): B4, R3, P6, E2, E3 and E5. They
  need adoption work, the pipeline bot account or human review time. The
  adoption exit is therefore not attempted in this build; it is still
  reported.

**No publishing in this build.** Nothing is pushed, no remote is changed,
and no GitHub repository, Pages site, release, secret or setting is
created or touched. E1's registry is built and tested against local
fixtures only: a local git repository or `file://` registry, and a local
static HTTP server. Technical exit item 6 is met against those fixtures.
Its "live" part, the public registry repository with its Pages site and
signing key, is the human's publishing step afterwards.

**What stays with the approver.** A fallback that this plan reserves for
the approver is not taken by an implementer: Q1's stress corner, if P0's
feasibility row fails, is the approver's choice before P4 starts. The
same holds for a `spec/`, protocol or schema change beyond the accepted
ADRs. Each is reported with the measurement that triggers it. The rules
the approver accepted with a fixed outcome are applied as written and
reported: ADR-014 is withdrawn if G0 misses 3×, and W2 falls back to its
run-ending criterion if P5's prototype fails.

## Review notes (design review round 1, 2026-11-14)

Two critiques reviewed the first draft. Every blocker and major point was
adopted, most of them changed by a measurement made for this revision.

**Adopted:**

- **W1 could not discriminate (blocker, both critiques).** Re-derived at
  V28 from the suite's own deck (section 2's table). The +5 % corner is
  gated; the 5 ms control is replaced by a window-derived one; an
  anti-vacuity check (`pofwarn` fired) is added; a feasibility row in P0
  decides before P4; the static-load sweep shows why co-simulation is
  reported rather than gated.
- **W2 conditional** and its fallback stated in section 5. The load
  sweep shows the suite's collapse does not depend on the +3V3 load, so
  the open question is Breadboard's views and Renode's reset, both in
  P5's prototype.
- **ADR-014's premise** (glue is unconditional; the remaining graph stays
  cyclic): D4 rewritten as a model change with digital-only nets, G0
  measures the model change and the protocol separately, one threshold,
  the scope stated, and the exit item made "accepted or withdrawn".
- **External Control has no accept hook in 1.17.0** (verified in
  `ExternalControlSocket.cs`): the reverse client is the design, costed at
  1.5-2 weeks.
- **High-Z glue had no DC path** (both critiques): a leakage conductance,
  a consistent `oe` source and an operating-point gate (ADR-009 D5, P2).
- **The suite's models differ from the views**: the suite replica, the
  restated RT9080, the BQ25180 `suite_compat` corner, and tolerances from
  P0's measured difference.
- **Numerics:** P0 is the S1-style spike; the measurement already shows
  the suite deck needs Gear at `phase0`.
- **Runtime arithmetic:** baseline-relative targets, named nightly moves,
  H5 removed from the offsets.
- **Figure-derived and fitted values:** claim kinds in ADR-012, and the
  confidence ceiling stated in R2.
- **Schema epoch, registry majors, search order, honesty rung,
  check scripts, review identity:** milestone S, ADR-011 D4/D12, ADR-009
  D1 and P1, ADR-010 D10, ADR-012 D7/D10.
- **Minor points:** estimate totals and the W-fw milestone; R3, B4 and P6
  assigned; tautological gates replaced (P3, P2); the `switch` view kind;
  `bb open --vscode` dropped; `file://` registries; engine-neutral names;
  view-name prefixes made an error; the adoption exit's no-target rule
  and candidate survey; approver rounds staged and budgeted.

**Not adopted, and why:**

- **"Extend ADR-014 D1 so a sampled-only back edge does not count for
  acyclicity."** No ordering makes that cycle safe without a new promise:
  the CPU would have to promise when it next samples, and Breadboard's
  SAADC starts a sample when firmware writes a task, which a running CPU
  can do at any instruction. That "sample horizon" is recorded as a
  Phase 3 candidate in ADR-014 instead.
- **"Compile one plugin assembly per platform at run time"** (ADR-015
  D6). It costs a cold compile per platform for a residual that the
  subset test plus a stated residual covers (ADR-015 D6).
- **"wellen is not light"** (ADR-016). Its 11 dependencies stay out of the
  workspace: the check is a separate crate built only in the docs job. The
  critique's real point, that the gate had nothing to load, is adopted.
- **"Pick another worked example if W1 cannot discriminate."** Measured:
  no other suite POF case crosses a settable threshold (`floor_probe.py`).
  W1 stays, at the +5 % corner, with Q1's stress-corner fallback.

## Review notes: technical exit (2026-11-17)

Every milestone of this build's scope (section 10) is merged. The
technical exit (section 5) was measured item by item on `main` at
f91d025, in the ext4 worktree `~/src/bb-p` (on `main`'s commit) and, for
the stranger test, from a local clone of the main checkout. Three gaps
were found and closed in this round, each with its own commit: W1 and W2
did not run through `bb test` and the quickstart had no W1 step (items 1,
2, 4; ADR-009 Amendment 10), and the registry's G7 failed (item 6; ADR-011
Amendment 3). `make check` on the result (the registry fix and the
relock): green pooled (497 s) and with `POOL=0` (597 s); `make
registry-gate` PASS; the docs site builds (`npm run docs:build`). Nothing
was pushed or published.

| Item | Verdict | Evidence |
| --- | --- | --- |
| 1. W1 at `pi` through `bb test` | **PASS**, with Amendment 8's recorded deviation (the anti-vacuity check is `pof-dip` with the paired control, not "`pofwarn` fired") | `make dms-w1-test` is `tests/w1.toml` through `bb test` (6 cases: `rule` passes cue-held, pof-dip, flash-vcc, no-reset at rung `pi`; `blank4` and `off` fail exactly `cue-held`; `margin`, `margin-off`; analog-first `same_as`); `make dms-w1` PASS (37 checks, 0 failures: every gated case and both co-simulations equal their one-cell references except grant records at bb's defaults and 1 ms, the references equal `tests/golden/dms-w1`, analog-first, 10 runs byte-identical); static-load against the suite at 55 mA: +3V3 min +109.6 mV, VSYS +101.0 mV, window start +18.8 µs (tolerances 200 mV, 200 mV, 50 µs); margin at 50 mA: +3V3 min 2.962615 V (+22.6 mV), no POFWARN, both builds; halved capacitance reported (2.9189 V, −21.1 mV, `make pi-spike`); `run.json`'s bench and every `check_dms_w1.py` line name the stress corner; the suite replica (local-only TPS22917) passes every replica gate of `make pi-spike` (see finding A); `make bb-test-gate`: 10 runs, `test.json` byte-identical, `junit.xml` read by the reporter's own parser. W1 is converged in the lattice (Amendment 10 item 3). "JUnit is published by CI": the step reads `out/dms-*-test/junit.xml`, which now includes W1's; it runs only on a push |
| 2. W2 (P5's prototype passed, so in full) | **PASS on its stated checks; the loop's verdict is the approver's** | `tests/w2.toml` through `bb test` (old: brownout ≥ 2 `reset` records, boots, resetreas-zero, oled-en-in-reset; rule: no-reset, vcc, oled-off); `make dms-w2` PASS (23 checks: VBOR crossings at 1.57/1.60/1.63 V within 500 µs of the suite, the rule's minima within 20 / 10 mV, partition independence, goldens, analog-first, 10 runs); 10 `bb test` runs byte-identical. **needs approval:** one brown-out writes two `reset` records (hold, release), so "at least 2" passes literally while the loop does not repeat in 200 ms (ADR-009 Amendment 9) |
| 3. dms-* checks as `bb test` files; `fast` unchanged | **PASS** | `make dms-rail-test`, `dms-vsense-test`, `dms-home-test` (in `make test`) run `tests/e1.toml`, `e2.toml`, `e3.toml`; `make assert-gate` PASS (83 checks, every negative control failing exactly as before); `make bb-test-gate` PASS (e1-e3, w1, w2: 10 runs each, `test.json` byte-identical, the tenth fresh). Every `fast` golden changed since aaebe43 was changed by an approved, recorded decision: H2 (D-SNAP, ADR-015 Amendment 4), P1 (D-TPS, ADR-009 Amendment 4: E1's and E2 sag/rec's hashes, the e1-rail plots), H5 (Q4's D4 model change, ADR-014 Amendment 1: E3 typ/worst/park), H6 (Q8/ADR-015 D9: `adc-stock` removed); the spike's goldens are unchanged; `make test` re-checks every committed hash |
| 4. Stranger `pi` demo | **PASS** | README quickstart gains `bb test tests/w1.toml --out out/w1`; `make stranger-test-container` from a local clone (`file:///repo`, the main checkout read-only) at f91d025: **504.6 s** against 900 s (apt 21.3 s, clone 404.5 s from the Windows-mounted checkout, `make setup SETUP=demo` 64.6 s, every `bb` line under 6 s, the W1 line 5.9 s, exit 0); both E1 runs equal their committed hashes |
| 5a. No unbounded hang (H1) | **PASS** | `make ngspice-stall` PASS (NoProgress at 77.43 ms in 23.8 s, 38 MB; H2's completion runs; the 502.47 ms negative control); a long quiet E3 run, `homing.elf` on `e3/board.cir` for 30 s (2.7 s of homing, then 27.3 s of the parked board with every engine quiet): completed in 50.4 s wall (47.7 s simulation), 3 175 grants (3 003 quiet), 0 promises broken, bb's peak RSS 137 MB (335 MB for the process tree) |
| 5b. External Control refuses foreign clients (H3) | **PASS** | `make ec-exposure`: 20/20 starts (4 fresh, 16 leased) refused their foreign client and every reconnect's, none read a byte, Renode listened on no port |
| 5c. ADR-014 built, G3 met | **PASS** | `make dms-home-g0` (N = 3, idle, lock): the protocol gain **27.8x** in simulation time (d4 123.25 s / proto 4.43 s; H5 measured 32.3x), proto equal to d4 except grant records; `make dms-home` PASS (`typ` 433 grants ≤ 5 000, 270 000 one-cell; peak RSS 31 MB; negative controls) |
| 5d. `make check` ≤ H0 + 4 min | **PASS** | `make baseline` (N = 3 + warm-up, idle ext4 worktree, lock) at f91d025: **478 s** (478, 481, 478; warm-up 491 s) against H0's 470 s and the target **710 s**; green 4 of 4. `make check POOL=0` 725 s, green |
| 5e. Slowest CI PR job ≤ 15 min | **PASS (estimate)** | `make ci-jobs-local` with `fw-rebuild` (lock, all 8 jobs green, setup aside): `fw-rebuild` 642 s (a fresh west workspace over this host's network; H0's CI ran 7 images in 1:41 plus 1:12 setup, so 13 images estimate about 5 min on CI), `dms-rail` 376 s, `test` 343 s, `cargo-test` 152 s, `feedback` 103 s, `adc` 42 s, `partition` 20 s, `verify-every` 6 s. With H0's CI setup medians the worst is `fw-rebuild` at 642 + 72 s = 11.9 min, under 15; CI confirms on the first push. Moved to nightly by H6, with its saving: the `fresh` job (11:30 CI median), `ec-long`, `ngspice-stall`, the full `dms-home` and `dms-vsense` (ADR-015 Amendment 5) |
| 6. Registry (against local fixtures, decision "No publishing") | **PASS after a fix** | `make registry-gate` first FAILED G7 (17 checks): `raytac/mdbt50q-u1mv2` changed in P4 without a version bump, and `bb registry check` re-linted already-published versions against platforms this bb has retired. Fixed (ADR-011 Amendment 3: 0.1.1 and `make locks`; published versions not re-linted for platform/device availability, with a check and its control). Then `make registry-gate` PASS: 106/106 and G7 (every first-party package, 15, in the local registry repository at serial 2, signed; dial-me-spinner's E1 with a cold cache from the HTTPS fixture and offline with a warm one, equal to `tests/golden/dms-rail/ok-cold`; tampering, rollback, yanking, the routed namespace in `registry-test`). `make test-offline` PASS (`make test` and `make packages` with no route, 955 s). The live registry is the human's publishing step (`docs/registry-publish.md`) |

**Findings that are not exit items** (ADR-009 Amendment 10):

- **A. `make pi-spike` fails one of 37 gates since P1** (the W2-old `pi`
  deck at 10 µs against its TMAX 1 µs run: 14.8 mV against 2 mV), caused
  by D-TPS's TPS22917 restatement (shown by restoring the pre-P1 view
  alone). **needs approval:** re-base P0's tolerance or pin the spike's
  view; nothing was changed.
- **B. W2 old's collapse is about 14 mV off at the 10 µs lattice** in the
  gated run too (converged at 2 µs); no W1 or W2 verdict moves, and W1
  is converged to 0.04 mV.

**Not shown here, by decision:** GitHub runs (the CI jobs, the JUnit
reporter step, `nightly.yml`), the public registry and its Pages site;
the adoption exit (B4, R3, P6, E2, E3, E5 not started).
