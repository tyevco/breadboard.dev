# ADR-008: nRF52840 as a CPU target (Renode platform, peripheral models, firmware, GDB)

- **Status:** Accepted (2026-11-09), with the approver decisions below
  (revised 2026-11-09 after two design reviews, see Review notes).
  Written before implementation because D7
  changes the `adc_in` pin model (`spec/pin-model.md`) and D6 fixes a
  lookahead bug in the Renode engine. D6 implemented (M1, 2026-11-09,
  Amendment 1). D1-D4 implemented (M2, 2026-11-09, Amendment 2). D5's
  example firmware, the image hash and D4 for projects implemented (M6,
  2026-11-09, Amendment 3). D7 and D8 implemented, M8's first half (the
  model, the loading `adc_in`, the UART lines; 2026-11-10, Amendment 4).
  E2 on dial-me-spinner, M8's second half (the firmware, the project
  bindings, `saadc_se` qualified, `make dms-vsense`; 2026-11-10,
  Amendment 5). The post-M8 review's fixes (2026-11-10, Amendment 6).
  D10 prototyped and implemented, M9 (2026-11-13, Amendment 7): a
  breakpoint stop changes results, so `bb run --gdb` is a marked debug
  mode, and a session without a stop inside a slice is byte-identical.
- **Part of:** the Phase 1 plan (`docs/phase1-plan.md`): M1, M2, M6, M8, M9,
  and M10 for D9.
- **Builds on:**
  - ADR-003: the lattice, traces, lookahead, boundary delivery inside a
    loop.
  - ADR-004: startup and pool rules.
  - ADR-005: an in-repo peripheral model, timed inputs, the host port and
    the **sampled-input** rule. Its D1.6 already records that the core
    carries over to the nRF52840 SAADC.
  - ADR-006: platform identities (D8), `pin_load` views (D4).
- **Normative changes when implemented:**
  - `spec/pin-model.md`: `adc_in` generalised and the loading option (D7);
  - `spec/time-grant.md`: none;
  - `spec/event-stream.md`: none (UART lines, PIN_CNF findings and SAADC
    sample records use the existing `log` record).
- **Evidence:**
  - **Renode 1.17.0:** the installed `platforms/cpus/nrf52840.repl` (read
    for this ADR; it sets no `PerformanceInMips`, while e.g.
    `ambiq-apollo4.repl` sets 96) and the sources at `~/bbscratch-src/renode`
    (tag `v1.17.0`; `BaseCPU.cs` defaults `PerformanceInMips = 100`).
  - **Prototypes and measurements** in `~/bbscratch-p1-nrf`: a harness on
    the real `RenodeEngine`; bare-metal and Zephyr firmware;
    `out/perf/results.tsv`.
  - **Measurements** in `~/bbscratch-p1-design` (Zephyr build
    reproducibility, GDB-server sources) and the critiques' reading of the
    Zephyr and nrfx sources.
  - **Nordic:** the nRF52840 Product Specification (SAADC chapter online,
    PS v1.1 §6.23 as cited by the dial-me-spinner suite) and the SVD.
  - **Zephyr** (`~/zephyrproject`, v4.5.0-rc3): `drivers/adc/adc_nrfx_saadc.c`,
    `modules/hal/nordic/nrfx/drivers/src/nrfx_saadc.c`,
    `modules/hal/nordic/nrfx/lib/nrfx_coredep.h`,
    `soc/nordic/nrf52/soc.c`, and `nrf52840.dtsi`.

## Approver decisions (2026-11-09)

Recorded after the human approver reviewed the Phase 1 plan and ADR-006,
ADR-007 and ADR-008 (the same decisions A–C are in each of them).

- **A. Every recommended answer is accepted.** For this ADR's open
  questions:
  1. **CPU speed (D1):** `PerformanceInMips: 64`; busy-waits documented at
     about 0.67× of their requested time; rule timing in example firmware
     only with `k_sleep`/RTC/TIMER.
  2. **Firmware location (D5):** `breadboard.dev/examples/dial-me-spinner/firmware`;
     dial-me-spinner stays untouched.
  3. **Zephyr pin:** the latest release tag at M6, with its matching SDK.
  4. **The loading `adc_in` (D7):** the pin-model change, the `#smp`
     toggle with the in-view pre-charge, and the TACQ ≥ 2·L refusal with an
     explicit `load = "none"` escape (no automatic fallback).
  5. **SAADC timing and rounding:** tCONV fixed at 2 µs; the sample instant
     at the end of TACQ; start latency 0 after SAMPLE; `round_half_up`
     before averaging.
  6. **VPRE default (D7):** VDD.
  7. **PIN_CNF audit (D4):** `warn`, with open-drain against `digital_out`
     as `error`.
  8. **GDB (M9):** after the exit, prototype first.
- **B. Publication:** nothing is published, pushed or given a remote
  during Phase 1's milestones; M7 tests from a local clone and the human
  decides publication afterwards (plan, Approver decisions B).
- **C. Reference revision:** the reference board is dial-me-spinner
  `6854d25` (branch `kicad10-port`, KiCad 10.0.8) instead of `dc96515`.
  The board, its pins and its firmware rules are the same at both
  commits (ADR-007 Context 12), so nothing in this ADR changes; the E1
  firmware cites `docs/firmware.md` at `6854d25`.

## Context

1. **What Renode 1.17.0 provides.**
   - One CPU file (`cortex-m4`, 1 MB flash at 0, 256 KB RAM at 0x20000000)
     and one board file (the nRF52840-DK).
   - The CPU file ends with `sysbus: init: ApplySVD @https://dl.antmicro.com/…`,
     which downloads the SVD on every load. The download also changes what
     unimplemented registers read: the SVD reset value instead of 0.
   - Peripherals present: CLOCK, RTC0–2, TIMER0–4, PPI, EGU0–5, GPIO P0/P1,
     GPIOTE, UARTE0/1, legacy TWI0/1, legacy SPI2, RADIO, RNG, ECB, WDT,
     I2S, PDM.
   - **Absent:** SAADC, PWM0–3, QSPI, NFCT, USBD (a source exists, but no
     platform uses it), SPIM3, POWER (POFWARN, RESETREAS), NVMC, CCM/AAR,
     TEMP, COMP/LPCOMP, TWIM.
   - No serial peripheral drives a pin: pin-select registers are ignored.
   - **CPU speed:** the stock file sets no `PerformanceInMips`, so the CPU
     runs at Renode's default of 100 MIPS, against the part's 64 MHz.
2. **Faithfulness gaps that matter** (sources read, behaviour measured):
   - **GPIO:**
     - DRIVE is stored but has no effect;
     - a pin switched back to input keeps driving its last level;
     - `LATCH` is a stub;
     - P1 is modelled with 32 pins (silicon has 16).
   - **TIMER:** `SHORTS` and `BITMODE` are unimplemented, so the counter is
     64-bit and never wraps. Measured: COMPARE0→CLEAR is ignored.
   - **PPI:** every task is deferred with `ExecuteInNearestSyncedState`.
     Measured: a TIMER1 compare at 1.003 ms toggled a GPIOTE pin at
     1.010 ms + 1 fs (silicon takes about one clock). While such a task is
     queued, lookahead gives no promise.
   - **RTC:** no OVRFLW event. Zephyr's `nrf_rtc_timer.c` keeps an
     `overflow_cnt`, so runs past 512 s are at risk.
   - **GPIOTE:** three bugs (Renode research §1).
   - **Busy-waits:** Zephyr on nRF52 uses `arch_busy_wait` →
     `nrfx_coredep_delay_us` (`soc/nordic/nrf52/soc.c`), a loop of
     `SUBS r0, #3; BHI` over `time_us × 64` assumed cycles
     (`nrfx_coredep.h`): 2 instructions per 3 assumed cycles. Renode counts
     instructions, so a busy-wait lasts `2·64/(3·MIPS)` of its requested
     time: 0.43× at 100 MIPS, 0.67× at 64 MIPS. `k_sleep` and the
     regulator's `startup-delay-us` (which sleeps) are timer-based and
     unaffected.
3. **The engine works on this CPU** (measured with the real `RenodeEngine`,
   at the default 100 MIPS):
   - The lookahead self-test passes: status 0x3, one TranslationCPU, one
     NVIC, all reflected fields present.
   - **Blinky (RTC0):** edges at 49.987940 / 99.975730 / 149.963520 ms,
     identical at one-cell grants, 10 ms grants, plugin armed, and
     lookahead on (61 grants against 50 000 per 500 ms).
   - **Feedback (GPIOTE IN):** identical edges with lookahead, 42–60 grants
     against 30 000.
   - **Zephyr blinky and a Zephyr FPU app** (`CONFIG_FPU`, FPU sharing) run
     unmodified on the offline platform, with identical edges with and
     without lookahead.
   - **Startup and per-grant cost** (N = 5, medians, noisy load):

     | Firmware | Fresh startup | One-cell grant | 10 ms grant |
     | --- | --- | --- | --- |
     | STM32F103 (reference) | 2.03 s | 0.45 ms | 6.6 ms |
     | nRF52840 bare-metal blinky | 1.75 s | 0.36 ms | 6.8 ms |
     | Zephyr blinky | 1.55 s | 0.31 ms | 6.8 ms |
   - **External Control:** output events and `SET_STATE` on `gpio0` work.
   - **0x5FFF0000 is free** for the lookahead block.
4. **An engine bug.** `engines/cpu-renode/src/lib.rs` (`quiet_until`, the
   line `let until = Time::from_ns(p_ns);`, about line 1509) panics with
   "Time overflow" when the promise is more than about 18 446 s away (u64
   femtoseconds). An nRF TIMER after its compare (64-bit, no wrap) triggers
   it at once, as would an RTC with prescaler 4095.
5. **Firmware toolchains.**
   - The pinned `arm-none-eabi-gcc` 13.2.1 builds bare-metal nRF programs
     (`hal_nordic`'s `nrf52840.h`, `cmsis_6`, a 60-line `startup.c`, a
     13-line linker script) in under 1 s, at 480–572 bytes.
   - The user's Zephyr workspace (`~/zephyrproject`) is Zephyr v4.5.0-rc3,
     a shallow clone without tags, with only `cmsis`, `cmsis_6` and
     `hal_nordic`, plus SDK 1.0.1. It is about 2.0 GB, and a build takes
     27–45 s.
   - NCS is not installed.
   - **Reproducibility (measured):** two pristine `west build -p always`
     runs of `samples/basic/blinky` for `nrf52840dk/nrf52840`, in different
     build directories of **one** workspace, gave
     - an **identical `zephyr.bin`** (sha256 `0bf1ea0f3205…`);
     - **different `zephyr.elf`** (first difference at byte 26118: debug
       information carrying the build path).
   - **But the image embeds workspace metadata:** the boot banner is
     `*** Booting Zephyr OS build 76280bce2a05 ***`, the `git describe` of
     the Zephyr tree. A workspace with tags would print `v4.x.y`, and a
     dirty tree adds `-dirty`, so the same SHA from another workspace can
     give another image.
6. **The firmware plan's SAADC use, and what Zephyr actually does**
   (dial-me-spinner `docs/firmware.md` §12.4):
   - AIN0 = P0.02 behind 687 kΩ with 100 nF;
   - gain 1/6, 0.6 V reference, 12 bits, 8× oversampling, TACQ 40 µs;
   - VSYS = code × 2.8125 mV.

   **Zephyr's `adc_read` path** (`adc_nrfx_saadc.c`, lines 550–570): only a
   single-channel sequence with `interval_us > 0` and no callback uses
   nrfx **advanced mode** with the SAADC's internal timer and
   `BURST_DISABLED`; there nrfx busy-polls DONE between SAMPLEs. Every
   other read, including the RTC-paced bursts the firmware plan uses, goes
   through `nrfx_saadc_simple_mode_set`, and nrfx turns BURST on whenever
   oversampling is on (`nrfx_saadc.c:524–525`: "Burst is implicitly
   enabled if oversampling is enabled"). So one SAMPLE task runs **8
   back-to-back acquisitions in hardware**, each TACQ followed by tCONV,
   with no CPU action between them. Zephyr's `ADC_ACQ_TIME_DEFAULT` maps to
   10 µs (`adc_nrfx_saadc.c:77–78`).

   The suite's SAADC model (`vsys_sense__nrf52840_saadc.lib`, from PS v1.1)
   has:
   - CS 2.5 pF and CPAD 3 pF;
   - an assumed switch resistance of 5 kΩ, checked against the PS
     acquisition-time table;
   - a pre-charge VPRE, an assumption because "the PS does not say what CS
     holds between samples", applied **before every acquisition**
     (`_burst_waves`: "n back-to-back acquisitions (tACQ + tCONV each). RST
     pre-charges CSAMPLE to VPRE just before every acquisition");
   - TCONV "< 2 µs" (PS v1.1 §6.23.10.1), so the gaps between acquisitions
     are about 2 µs, far below the 10 µs lattice.

   Measured by the suite: because C5 supplies the sampling charge, the
   averaged error depends on the **sample rate** (356 µV for 1 s bursts,
   761 µV for 50 ms bursts, at VSYS 3.0 V with CS pre-charged to VDD max,
   against the unloaded pin voltage), not on TACQ. Both are below one
   12-bit LSB at the pin (3.6 V/4096 = 879 µV). Breadboard's `adc_in` is
   ideal and loads nothing (`spec/pin-model.md`, "No glue: the input is
   ideal"), so it cannot show this.
7. **Sub-lattice features collapse when ngspice runs first.** Inside a
   loop, a consumer before its producer in the plan order receives every
   transition of a sub-cell pulse at `ceil_L(t)` (ADR-003, boundary
   delivery inside a loop). A 2 µs low gap between two acquisition windows
   then has zero width in ngspice, and the pre-charge between them, which
   is the charge draw that produces E2's effect, disappears.
8. **GDB.** Renode's `machine StartGdbServer port autostartEmulation`
   defaults `autostartEmulation = true` (`Machine.cs:395`).
   `GdbStub.SetupTerminal` then starts the emulation when a client
   connects (`GdbStub.cs:107`). That is free-running, which breaks the
   invariant "no engine free-runs". Nothing in the repo uses GDB today.

## Decision

### D1. Platform: `nrf52840-bb@1`, an overlay in the repo, at 64 MIPS

`engines/cpu-renode/platforms/nrf52840-bb.repl`:

- **The base:** `using "platforms/cpus/nrf52840.repl"`, with a replacement
  `sysbus: init:` block. It drops `ApplySVD` and keeps the stock `Tag`s
  (FICR, DEVICEADDRTYPE, DEVICEADDR[0], PSELRESET). This is the same
  technique as `stm32f103-offline.repl`, already prototyped in
  `~/bbscratch-p1-nrf/plat/nrf52840-offline.repl`.
- **CPU speed:** `cpu: PerformanceInMips: 64`, the part's clock, so
  instruction-count timing is right for code that averages about one
  instruction per cycle. Busy-wait loops still run at about 0.67× of their
  requested time (Context 2), which no single MIPS value fixes without
  slowing all other code. Therefore:
  - **example firmware times every rule with `k_sleep`, the RTC or a
    TIMER,** never with `k_busy_wait` or nrfx delays; the E1 and E2
    sources are reviewed for this;
  - the gate `nrf-busywait` records the virtual time of `k_busy_wait(10000)`
    (expected about 6.67 ms) and of a 10 ms `k_sleep` (expected 10 ms to
    within one RTC tick), and the package documents both.
- **Phase 1 (M2):** no peripheral is replaced. The lookahead block is
  mapped at 0x5FFF0000 on cyclic boards, as on the STM32s.
- **M8:** adds `bbSaadc: Analog.BbNrf52Saadc @ sysbus 0x40007000` (D7) and
  its host port `@ 0x5FFF0100`. It also wires the nRF52840 SAADC interrupt
  (IRQ 7, from the SVD) and GPIO VDD sensing as D7 describes.
- **Identity (ADR-006 D8):** the platform is `nrf52840-bb@1`; its content
  hash covers the overlay and every plugin source it loads. M8's SAADC
  model makes it `@2`.
- **`Platform::needs_plugins()`** is true once any Breadboard type is named.
  The overlay is compiled into the same cached plugin assembly (ADR-005 D6:
  one assembly, one `ScanFile`).
- **The pool:** before `nrf52840-bb` runs on the pool, its configurations
  are added to `make pool-equiv` (AGENTS.md rule).

The CPU type stays `cortex-m4`. Hard-float code runs on it (measured), and
the platform needs no `cortex-m4f` variant. Context 3's costs were measured
at 100 MIPS; M2 re-measures them at 64.

### D2. Pins

- **`PinSpec::nrf("P0.13", dir)`** maps `P<port>.<n>` to `gpio<port>`,
  index `n`. It accepts P0.00–P0.31 and P1.00–P1.15 and rejects P1.16 and
  above, even though Renode models 32 pins.
- **In the package** (`nordic/nrf52840-qiaa`, ADR-006), the `adc` channels
  are:

  | Channel | Pin |
  | --- | --- |
  | AIN0–AIN3 | P0.02–P0.05 |
  | AIN4–AIN7 | P0.28–P0.31 |

  The STM32 table in `cli/src/board.rs` (`adc_channel`) becomes a lookup in
  the package.
- **The board pin syntax** is `<instance>.<pin>` (ADR-007 D7), split at the
  first `.` after the instance, so `main:U10.P0.06` and `U10.P0.06` parse.
  Glue names follow ADR-007 (`V_U10_P0_06`).

### D3. Lookahead plugin compatibility

- **No change to the plugin.** Its assumptions hold on this CPU (Context 3).
- **The new gate, `make nrf-gates`:**
  - `nrf-blinky`: the edge list is identical at one-cell, 10 ms, plugin
    armed, and lookahead on;
  - `nrf-feedback`: GPIOTE IN feedback, with and without lookahead,
    byte-identical outputs and no `PromiseBroken`;
  - `nrf-zephyr`: Zephyr blinky with and without lookahead;
  - `nrf-busywait`: D1.
- **Gate firmware** lives in `engines/cpu-renode/tests/firmware/nrf52840/`
  and runs as `lookahead.rs`-style tests.
- **Stock PPI's deferred tasks** are deterministic, but they add up to one
  lattice cell of latency and block promises while queued (Context 2).
  - That costs speed, not correctness, so Phase 1 keeps stock PPI.
  - A Breadboard PPI that runs tasks immediately is required before any
    sampled-input argument relies on a PPI-triggered SAMPLE (D7), and
    before any example uses PPI timing (E3, the encoder chain).
  - Its own gate: one PPI hop measured at the event time, not at
    `ceil_L`.

### D4. Pin-model honesty on nRF GPIO

Renode ignores DRIVE, and pin direction is static in Breadboard. So:

- The project binding fixes the model and its parameter set per pin, for
  example `set = "H0H1"` (ADR-007 D5).
- **End-of-run audit.** After the last grant, the Renode engine reads
  `PIN_CNF[n]` (sysbus read, while paused) of every pin whose net is in the
  analog scope, bound or not, and logs a `warn` record when:
  - DIR disagrees with the bound model;
  - DRIVE disagrees with the bound set (S0S1, H0H1, H0S1, S0H1; open-drain
    S0D1/H0D1 against `digital_out` is an error-level `log`);
  - an **unbound** pin is configured as an output. Its net gets no glue
    (ADR-007 D5), so a forgotten `[[bind]]` would otherwise leave the net
    undriven without a word.

  This read happens after the last grant, so it changes no result.
- **Mid-run changes.** If Renode 1.17.0's peripheral write hooks can
  observe `PIN_CNF` writes on the GPIO peripheral (checked in M2), each
  write to an audited pin is recorded with the write's own virtual time,
  which does not depend on the grant partition. Otherwise the audit stays
  end-of-run only, and the package documents that a DIR or DRIVE change
  that is later reverted is not seen.

### D5. Firmware strategy and toolchain

**Decision: two kinds of firmware, each pinned.**

1. **Gate firmware is bare metal.** It builds with the already-pinned
   `arm-none-eabi-gcc` 13.2.1 against vendored headers:
   - nrfx MDK `nrf52840.h`, `nrf52840_bitfields.h` and `system_nrf52840`
     (BSD-3-Clause);
   - CMSIS-Core (Apache-2.0).

   It lives in `engines/cpu-renode/tests/firmware/nrf52840/`, with a
   startup file and linker script written for Breadboard. `make firmware`
   builds it in under a second per program. This is the STM32 pattern, and
   it keeps model gates independent of an RTOS.
2. **Example firmware is a Zephyr application** with an out-of-tree board,
   `dial_me_spinner/nrf52840`. The board is a subset transcribed from the
   firmware plan's §3.1 devicetree draft:
   - pins;
   - `regulator-fixed` VLED with `startup-delay-us = 10000`;
   - `led-din-gpios`.

   Builds:
   - E1: `led_rail.elf` and `led_rail_wrong_order.elf` (one Kconfig switch).
   - E2: `vsense.elf`, with the sample interval as a Kconfig choice.

   **Pinned by a west manifest** with exact SHAs:
   - Zephyr, `hal_nordic`, `cmsis` and `cmsis_6`;
   - the Zephyr SDK version (1.0.1 unless the approver picks otherwise).

   **Image identity does not depend on the workspace:** the app sets
   `CONFIG_BOOT_BANNER=n` and a fixed `BUILD_VERSION` (in its
   `CMakeLists.txt`, before `find_package(Zephyr)`), so neither `git
   describe` nor a dirty tree reaches the image (Context 5).

   **Prebuilt ELFs are committed** in
   `examples/dial-me-spinner/firmware/prebuilt/` with a `SHA256SUMS` of
   their **loadable images**, so `make test` and the demo never need the
   about 2 GB SDK.
   - **The image hash** (kind `elf`) is sha256 over the PT_LOAD segments
     (address, length, bytes) plus the entry point, in address order. It is
     what `bb flash` records in `[targets."<ref>"] images` (ADR-007 D5) and
     `run.json` reports. The whole ELF is not hashed, because it differs
     between identical builds (Context 5). Other image kinds define their
     own hash when implemented.
   - **A separate CI job, `fw-rebuild`,** runs only where a workspace
     exists. It creates a **fresh** workspace from the manifest (a second
     `west init` into a new directory, not the developer's), rebuilds, and
     requires identical image hashes.

**Why Zephyr, and why not NCS or bare metal, for the examples.**

- **Against bare metal:** the firmware plan targets nRF Connect SDK and
  Zephyr. The Zephyr regulator, GPIO and ADC drivers express exactly the
  rules under test (`startup-delay-us`, the ADC API's
  `acquisition_time` and `oversampling`). A Zephyr app is therefore much
  closer to the real firmware than a hand-written loop, while needing
  nothing that Renode lacks for E1, and only the SAADC for E2.
- **Against NCS in Phase 1:** it is not installed. Its value (the
  SoftDevice Controller, the NFC libraries) needs RADIO plus CCM/AAR and
  NFCT, which are absent or unverified, so Phase 1 stubs BLE out.
- **Against v4.5.0-rc3 as the pin:** the user's workspace is an rc. The
  manifest should pin a **release** tag's SHA (approver question 3).

**Test firmware location (approver question 2).** Proposed:
`breadboard.dev/examples/dial-me-spinner/firmware/`. This leaves the
dial-me-spinner repository untouched, which is a hard rule for this design
work. It keeps the example self-contained, so the stranger demo needs no
second repository. It also makes clear that this firmware is a test
fixture written to exercise one rule, not the product firmware.

### D6. Engine fix: clamp the promise (M1)

- **The fix:** in `quiet_until`, clamp `p_ns` to `Time::MAX.as_ns()` (or to
  the run's `t_end`, whichever is smaller) before `Time::from_ns`. The
  grant rule already caps every grant at `--max-quantum`, so a clamped
  promise changes no grant.
- **Gate:** a unit test with `Fault::StretchPromise { ns: u64::MAX }`, and
  an nRF gate firmware whose TIMER compare is followed by a long sleep.
- **Applies to:** both platforms. The STM32 path cannot reach the bug
  today, but the fix is platform-independent.

### D7. SAADC model (`BbNrf52Saadc`) and a loading `adc_in` (M8, for E2)

**The model.** It is C# in `engines/cpu-renode/platforms/bb_nrf52_saadc.cs`,
in ADR-005's style: timed inputs, a host port, `LimitTimer`s on a 1 GHz
clock, and the causality error word.

- **Registers** (SVD; PS SAADC chapter):
  - tasks: START, SAMPLE, STOP, CALIBRATEOFFSET;
  - events: STARTED, END, DONE, RESULTDONE, CALIBRATEDONE, STOPPED and
    CH[n].LIMITH/LIMITL, with INTEN/INTENSET/INTENCLR;
  - CH[n].PSELP/PSELN/CONFIG (gain, reference INTERNAL or VDD1_4, TACQ,
    MODE, BURST);
  - CH[n].LIMIT, RESOLUTION, OVERSAMPLE, SAMPLERATE (task mode, and the
    internal-timer mode that Zephyr's advanced path uses);
  - RESULT.PTR/MAXCNT/AMOUNT, through EasyDMA writes to system RAM. PTR is
    latched at START, and END fires when MAXCNT results are written.
- **Events reach PPI** through the stock PPI's event inputs, as on other
  nRF peripherals.
- **Timing** (approver question 5 fixes the numbers). Every instant below
  is the deadline of one of the model's own `LimitTimer`s:
  - Acquisition `k` starts at `a_k` and its sample instant is
    `s_k = a_k + TACQ` (the end of TACQ); `a_0` is the SAMPLE task (start
    latency 0).
  - DONE comes at `s_k + tCONV`, with tCONV = 2 µs. PS v1.1 §6.23.10.1 says
    "< 2 µs", so the bound is taken as the value, like ADR-005's fixed
    latency.
  - **BURST** (set explicitly, or implied by nrfx when oversampling): one
    SAMPLE task runs `2^OVERSAMPLE` acquisitions back to back,
    `a_{k+1} = s_k + tCONV`, with no CPU action. Without BURST, each
    acquisition needs its own SAMPLE.
  - RESULTDONE comes after `2^OVERSAMPLE` samples. The average is computed
    at the configured resolution, before averaging, per the PS.
- **Transfer:** `RESULT = (V_P − V_N) · GAIN / REF · 2^(RES − m)` with m = 1
  differential and m = 0 single-ended. The result is sign-extended to
  16 bits.
  - Rounding is `round_half_up` (approver question 5; the PS does not say).
  - With `REF = VDD1_4`, VDD is a **second sampled input**, pushed from the
    `+3V3` net through a VDD binding of the package's `power_in` pin.
- **The push key changes for this model:** raw µV on every change. Gain,
  reference and resolution are run-time choices, so ADR-005's `code12`-style
  key does not exist here. Because the pin is a sampled input, pushes cost
  batch size only, not grants (ADR-005 D3).
- **Per-sample record** (binding option `trace_samples = true`, used by
  E2): at each `s_k` the model emits a `log` record (`source = "renode"`,
  level `debug`) with the channel and the µV value it sampled. That is the
  analog-side observable for E2 (plan §2), stamped with an exact virtual
  time, so it is partition-independent.

**The sampled-input argument (ADR-005 D3), restated for SAADC.**

- **The model reads an input only at a sample instant `s_k`,** which is the
  deadline of its own `LimitTimer`.
- **An acquisition can start in only three ways:**
  - **A CPU write of SAMPLE.** The CPU is awake, so there is no promise.
    This covers Zephyr's advanced mode (which busy-polls DONE between
    SAMPLEs) and the first acquisition of a simple-mode burst.
  - **The model's own timer:** the next acquisition of a burst
    (`s_k + tCONV`), or the internal-timer mode's period. Both are clock
    limits of the model, which `quiet_until` already reports, as for
    ADR-005's timer-triggered conversions.
  - **A PPI-triggered SAMPLE.** Valid only with a Breadboard PPI that runs
    the task at the event instant (D3), because the event is itself a clock
    limit (RTC or TIMER compare).

  So `q <= s` for every future sample.
- **LIMITH/LIMITL fire at DONE**, which is after `s`. A push therefore
  never wakes the CPU before `q`, and the audit (`PromiseBroken`) stays the
  safety net.
- **`RenodeEngine::input_is_sampled`** is true for an `adc_in` bound to
  `BbNrf52Saadc`. A gate firmware like `la_adc` (`lookahead.rs`) pins this,
  including a burst.

**The loading `adc_in`: one charge transfer per acquisition.** The ideal
`adc_in` cannot show E2's main effect (Context 6), so `adc_in` gains an
optional **sampling load**, a `pin_load` view of the package (ADR-006 D4).
Context 7 rules out driving it with an acquisition window and a separate
pre-charge pulse: their 2 µs gaps collapse when ngspice runs first. So the
CPU engine sends **one edge per acquisition**, and the view does the rest:

```
   pad (net) ── CPAD to 0                                   (always)
      │
   Bacq: I = (V(pad) − V(hold)) / RSW                       (hold follows the pad through RSW)
      │
   hold ── CS to 0 ;  Brst: hold → VPRE for t_pre after every edge of smp
                      (edge detector inside the view: smp XOR smp delayed by an internal RC)
```

- **`smp` is a toggle:** `<ref>.<pin>#smp` changes level at every
  acquisition start `a_k`, in either direction. It is one internal digital
  output of the CPU engine, driven through a rule-1 `EXTERNAL` source into
  a generated glue net `__smp_<pin>`, with exactly one driver, so the
  pin-boundary contract holds. A toggle cannot be lost by late delivery as
  a pulse can: two toggles could only cancel if they fell in one lattice
  cell, and the TACQ rule below makes acquisition starts at least
  `TACQ + tCONV > 2·L` apart.
- **The view pre-charges `hold` to VPRE** for a short internal `t_pre`
  (tens of ns, a view parameter) at each `smp` edge, then lets it settle
  through RSW towards the pad. This is the suite's "RST pre-charges CSAMPLE
  to VPRE just before every acquisition", moved inside the view so it
  happens once per acquisition however the edge is delivered. The charge
  drawn from the pad per acquisition, `CS·(VPRE − V(pad))`, is what makes
  the error depend on the sample rate.
- **Simplification, bounded:** in the suite, CS is disconnected between
  acquisitions; here `hold` stays connected through RSW until the next
  edge. That adds CS (2.5 pF) in parallel with the pad between samples:
  25 ppm of C5 (100 nF), 250 ppm in the C5 ÷ 10 fault. M8 measures the
  difference against the suite's switched model.
- **VPRE default:** VDD of the bound supply (the suite's worst case, and
  the case behind the 356/761 µV figures), as the view parameter `vpre`;
  `vpre = "0V"` is the alternative. Approver question 6.
- **The value pushed to the model is V(hold),** the `observe` port, not
  V(pad). With loading, the converter sees the charge-shared, partially
  settled sample cap, as silicon does.
- **The window-skew bound.** An `smp` edge reaches ngspice either at its
  exact time (Renode advanced first in that cell) or at the next lattice
  point (ADR-002 D4.4). The transfer therefore starts up to `L` late, and
  the converter reads the latest push strictly before `s_k`, which is up to
  `L` old. Reading a settled V(hold) needs at least one lattice point
  strictly between the latest possible transfer start and `s_k`.
  - The model refuses (error word, run fails) a loaded channel with
    TACQ < 2·L, where L is a constructor parameter set from
    `--renode-quantum`.
  - E2's 40 µs with L = 10 µs qualifies.
  - The error message names both remedies: `--renode-quantum 1us` (for
    TACQ ≥ 2 µs), or an explicit `load = "none"` on the binding, which
    `run.json` records. Zephyr's default TACQ (10 µs) at L = 10 µs is
    refused, and the package documentation says so.
- **Why the slow effect is unaffected by the skew:** E2's dependence on the
  sample rate is the cumulative charge drawn from C5 (τ about 69 ms), one
  transfer per acquisition. It is resolved by ngspice whatever the sub-L
  skew of each transfer. Gate M8 measures how close the result is to the
  suite's 356/761 µV and sets the tolerance from that.
- **Is `adc_in` still a sampled input with loading?** Yes. The value is
  still read only at `s_k`. `smp` is an output that changes only at the
  model's own limits or at CPU writes, both covered by `quiet_until` as
  ordinary output changes.
- **On a cyclic board** (loading always makes one: Renode → ngspice →
  Renode), the grants around each acquisition are one-cell. That costs
  about 5 cells per acquisition: about 40 per 8× burst, or about 800 grants
  per second of simulation at 20 Hz bursts. This is acceptable for E2 and
  measured at M8.

**Rejected for SAADC:**

- **An averaged conductance** `1/(f_sample · CS)` in place of the switched
  cap. Its value depends on the firmware's sample rate, which is the very
  thing under test.
- **Separate `#acq` and `#rst` pulses** (the first draft). Their 2 µs gaps
  collapse when ngspice runs first (Context 7), which removes the charge
  draw that E2 measures.
- **Forcing Renode-first order for loaded pins.** Results would then
  depend on the engine order, which every board must not (the
  `--engine-order` gate).
- **Asking ngspice to evaluate V(hold) at exactly `s`.** ngspice advances
  before Renode in the plan order, so `s` is not known when ngspice
  crosses it. It would need a new request in the time-grant protocol.

### D8. UART log lines (M8)

The Renode engine attaches to UARTE0 (and any UART the package names) and
turns each line into a `log` record:

- **Source:** `renode`.
- **Level:** `info`.
- **`t_fs`:** the virtual time of the byte that ended the line, clamped to
  the grant start (`spec/event-stream.md`).

UARTE pin selects are placeholders in Renode, so there is no TX pin edge.
That is documented in the package. This record type exists already; only
the producer is new. Determinism: the bytes and their virtual times are a
function of the firmware and the inputs.

### D9. Peripheral models after E2 (listed so scope is explicit)

| Model | Needed by | Note |
| --- | --- | --- |
| `BbNrf52Ppi`: immediate tasks | SAADC through PPI, E3, encoder chain | D3 |
| `BbNrf52Timer`: SHORTS, BITMODE, counter-mode events | E3 (PWM seq timing), encoder chain | Context 2 |
| `BbNrf52Pwm`: drives `gpioN.Connections[n]` from PSEL at run time | E3, piezo | Pins selected at run time. An edge at a limit gets an exact timestamp through External Control's GPIO hooks |
| TEMP and NVMC stand-ins | E3 (MD-3 duty, NVS) | NVMC at least READY and erase/write timing |
| POWER: RESETREAS, POFWARN fed by a rail, SYSTEMOFF | S4, S5 (Phase 2) | Needs a "rail-driven reset" pin-model ADR |
| TWIM with EasyDMA, SPIM3 | Bus devices (Phase 2) | Also a route for bus devices (ADR-006 D7) |
| RTC OVRFLW | Runs past 512 s | Small; before any long soak |

Each is its own ADR-005-style step: register tests, timing tests and an
equivalence gate. Any that claims sampled inputs needs the D7 argument and
a `la_*` gate. Each bumps the platform major (D1).

### D10. GDB stub passthrough (M9, gated, prototype first)

- **The mode:** `bb run --gdb <port>` (default off) issues
  `machine StartGdbServer <port> false` before the first grant. Passing
  `autostartEmulation = false` is mandatory (Context 8).
- **While GDB is attached:**
  - Lookahead is forced off for that engine. A debugger-halted CPU breaks
    the plugin's WFI-based assumptions.
  - The External Control client's request timeouts are suspended, because
    a halted CPU keeps a `RunFor` open for an unbounded wall time.
- **To be measured by the prototype before implementation:**
  1. Does a breakpoint hit inside a `RunFor` leave virtual time and the
     sync-point sequence unchanged after `continue`?
  2. Does single-stepping?
  3. Does a GDB memory read while paused at a grant boundary perturb
     anything?
- **The gate:** a run with GDB attached, one breakpoint hit and continued,
  must give `nets.csv`, `nets.vcd` and the non-grant records identical to
  the run without GDB.
- **If a step fails:** that operation is documented as perturbing, and
  `run.json` gets `"debug": true`. Such runs are never compared against
  goldens.
- **Outcome (Amendment 7):** a breakpoint stop ends Renode's time slice
  early and changes results, so this fallback applies. Attaching, halting
  at a slice boundary, reads and detaching are byte-identical and gated.
  The debugger talks to a proxy in `bb`, not to Renode's stub directly.

## Exact `spec/` changes (described, not applied)

1. **`spec/pin-model.md`, `adc_in`:**
   - The parameters become `adc_in{model, vmax, load}`:
     - `model` is `stm32f0` (today's `vref`/`bits` behaviour; `vref` and
       `bits` keep their meaning for it), `stm32-stock` or `nrf_saadc`;
     - `vmax` is the clamp, `[0, vmax]` (for nRF, the VDD of the bound
       supply);
     - `load` is `none` (today's "no glue") or a `pin_load` view name.
   - The push-key list gains `nrf_saadc`: the key is raw µV, delivered as
     raw µV.
   - The channel-mapping sentence moves to the package (D2), with the
     nRF52840 table as its second example.
   - A new paragraph, **Loading**, states D7's glue, the `#smp` toggle
     output (one edge per acquisition), the in-view pre-charge, V(hold) as
     the pushed value, and the TACQ ≥ 2·L rule.
   - A new **glue rule 8:** "an `adc_in` with `load` gets the `pin_load`
     view's subckt on the pad, one internal `EXTERNAL` source driven by the
     CPU engine's `#smp` trace, and pushes the view's `observe` node".
   - The sampled-input sentence names `BbNrf52Saadc` as a second model
     that qualifies.
2. **`spec/time-grant.md`:** no change. The sampled-input definition
   (ADR-005) is generic, and D7 satisfies it as stated.
3. **`spec/event-stream.md`:** no change. UART lines use `log` (D8),
   PIN_CNF audit findings are `log` records with level `warn` or `error`
   (D4), and SAADC sample records are `log` records with level `debug`
   (D7).

## Determinism impact

- **The overlay** removes the network dependency (the SVD download) and
  the SVD-dependent register reads. Runs depend only on the repo and the
  Renode install. The fixed MIPS is part of the platform identity.
- **New models are virtual-time only:** limit timers on fixed clocks, no
  wall clock, no host randomness, and timed inputs as in ADR-005.
- **The loading `adc_in`** adds one output trace and one analog node per
  loaded pin. A toggle cannot be lost by late delivery, so the number of
  charge transfers does not depend on the engine order. It is gated, like
  every speed-up, by byte identity with the one-cell, every-lattice-point
  reference, and by `--engine-order` independence.
- **The firmware image hash** (D5) makes "the same firmware" checkable
  across rebuilds and workspaces.
- **The PIN_CNF audit** reads after the last grant, or records writes at
  their own virtual time, so it never depends on the grant partition.
- **GDB mode** is either proved non-perturbing by its gate, or marked
  `debug` and excluded from comparisons.

## Gates

| Gate | Milestone | Content |
| --- | --- | --- |
| `make nrf-gates` | M2 | D3, including `nrf-busywait` (D1), and D6's TIMER-compare-then-long-sleep firmware (Amendment 1); as implemented also `nrf-ppi`, the D4 `PIN_CNF` audit and the board-level `make nrf-feedback` (Amendment 2) |
| Promise-clamp unit test | M1 | D6 |
| `make pool-equiv` with nRF configurations | M2 | AGENTS.md rule |
| `make dms-rail` | M6 | E1: one-cell equivalence, determinism, goldens. The PIN_CNF audit (D4) is clean for the correct firmware; as implemented also both engine orders, the board-level convergence gate and two negative controls of the audit (Amendment 3) |
| `make nrf-saadc-model` | M8 | Register map; TACQ × RESOLUTION × OVERSAMPLE × BURST acquisition and DONE instants; an 8× burst gives 8 `smp` toggles and 8 charge transfers at **both** engine orders; EasyDMA PTR/MAXCNT/AMOUNT; LIMIT events; the VDD reference; host-port timed inputs; the TACQ < 2·L refusal and its message |
| `la_saadc` in `lookahead.rs` | M8 | A sampled input that never wakes the CPU before `q`, with a burst |
| `make dms-vsense` | M8 | E2: analog-side and firmware-visible checks (plan §2), equivalence to the every-lattice-point reference, `--engine-order` independence, no `stop:ngspice` |
| GDB equivalence | M9 | D10; as implemented `make gdb-test` (in `make test`) and `make gdb`: no stop inside a slice gives the run without GDB byte for byte, a breakpoint marks the run (Amendment 7) |
| `fw-rebuild` (CI job where a Zephyr workspace can be created) | M6 | Image hashes of ELFs rebuilt in a fresh workspace equal `SHA256SUMS` (`make fw-rebuild`, Amendment 3) |
| `make cost-table` row for `dms-rail` | M6 | The ADR-003 protocol |
| `make cost-table` row for `dms-vsense` | M8 | The ADR-003 protocol |

## Rejected alternatives

- **The stock platform with `ApplySVD`.** It needs the network on every
  run, and register reads differ with the SVD.
- **Keeping Renode's default 100 MIPS.** Every instruction-timed path runs
  1.56× too fast; 64 is the part's clock.
- **A MIPS value chosen to make busy-waits exact (about 43).** It would
  make all other code 1.5× too slow to fix one loop that example firmware
  must not use for rule timing anyway.
- **Zephyr for gate firmware.** Model gates should not depend on a 2 GB SDK
  or on driver behaviour. Bare metal pins exactly what is tested.
- **Bare metal for the example firmware.** It would test a rewrite of the
  rules, not their Zephyr expression (D5).
- **Committing ELFs and checking whole-ELF hashes.** The measured debug-path
  differences make that a false failure (Context 5).
- **Fixing GPIO DRIVE inside Renode to change electrical behaviour.** Drive
  strength is an electrical property, so it belongs in the pin model. The
  CPU model only needs to report the configuration, which D4 audits.
- **Waiting for upstream Renode models.** ADR-005 already chose in-repo
  models for this reason, and Renode stays pinned at 1.17.0.

## Open questions for the approver (consolidated, with recommendations)

1. **CPU speed** (D1): `PerformanceInMips: 64`, busy-waits documented at
   about 0.67× of their requested time, and rule timing in example
   firmware only with `k_sleep`/RTC/TIMER. **Recommended: accept.**
2. **Firmware location** (D5): `breadboard.dev/examples/dial-me-spinner/firmware`,
   or in dial-me-spinner with prebuilt ELFs copied over? **Recommended:
   breadboard.dev** (dial-me-spinner stays untouched; the demo needs one
   repo).
3. **The Zephyr pin:** a release tag's SHA, or the workspace's
   v4.5.0-rc3? And keep SDK 1.0.1? **Recommended: the latest release tag
   at M6, with its matching SDK.**
4. **The loading `adc_in`** (D7): accept the pin-model change, the `#smp`
   toggle with the in-view pre-charge, and the TACQ ≥ 2·L refusal with an
   explicit `load = "none"` escape (no automatic fallback)?
   **Recommended: accept.**
5. **SAADC timing and rounding:** tCONV fixed at 2 µs; the sample instant
   at the end of TACQ; start latency 0 after SAMPLE; `round_half_up`
   before averaging. **Recommended: accept all four** (the PS gives only
   the bound and no rounding rule; M8 documents them).
6. **VPRE default** (D7): VDD (the suite's worst case and the basis of its
   356/761 µV figures) or 0 V? **Recommended: VDD.**
7. **PIN_CNF audit** (D4) as `warn`, or a run failure when DRIVE
   mismatches? **Recommended: `warn`,** with open-drain against
   `digital_out` as `error`.
8. **GDB in Phase 1** (M9), or deferred to Phase 2 with the VS Code
   debugger work? **Recommended: keep M9 after the exit, prototype first.**

## Review notes (2026-11-09)

Two critiques of the first draft. Adopted: the corrected Zephyr SAADC
paths and BURST (Context 6, D7), the collapse of sub-lattice features
(Context 7) and the toggle-plus-in-view-pre-charge design, the CPU speed
(D1) and the busy-wait gate, the workspace-independent image (D5), the
extended PIN_CNF audit (D4), `set` as the one key name, and platform
identity. Not adopted as proposed:

- **"Fall back to an unloaded `adc_in` with a warning when TACQ < 2·L."**
  Rejected: an automatic change of physics, even with a warning, is the
  silent default the honesty rule forbids, and it would hide exactly the
  effect E2 tests. The refusal stays, with an error that names both
  remedies, one of which is an explicit `load = "none"`.
- **"Sample PIN_CNF at each grant boundary."** Rejected: grant boundaries
  depend on the partition, so the records would differ between lookahead
  on and off. D4 uses write hooks stamped with the write's own virtual
  time instead, if Renode supports them, and the end-of-run audit
  otherwise.
- **"Require Renode-first order for loaded pins."** Rejected in favour of
  the toggle design, which needs no order (D7, Rejected for SAADC).

## Amendments

### Amendment 1: D6 as implemented (M1, 2026-11-09)

- **The fix.** `engines/cpu-renode/src/lib.rs`: `MAX_PROMISE_NS =
  Time::MAX.as_fs() / FS_PER_NS` (18 446 744 073 709 ns, the last whole
  nanosecond `Time` holds) and `promise_time(p_ns)`, which `quiet_inner`
  now uses in place of `Time::from_ns(p_ns)`. A promise further away is
  clamped to it; a promise in range is unchanged, bit for bit. A shorter
  promise is always honest, and the audit compares against the clamped
  value, so nothing else in the engine changes.
- **Deviation from the D6 text ("or to the run's `t_end`, whichever is
  smaller").** The engine does not know `t_end`: `bbcore::Engine` has no
  such call, and adding one would change the kernel interface for no
  effect. The kernel already takes the minimum of every promise with
  `t_end`, `--max-quantum`, aligned samples and the other engines' bounds
  before it rounds to the lattice (`Scheduler::query_quiet`), so a clamp to
  the type's limit gives the same grants. No protocol or `spec/` change.
- **Gate (the plan's M1 row), three tests:**
  - `bb-cpu-renode` unit test `promise_is_clamped_to_time_range`:
    `promise_time(u64::MAX)` and `promise_time(MAX_PROMISE_NS + 1)` give
    `Time::from_ns(MAX_PROMISE_NS)`; in-range promises are exact; a
    lookahead snapshot whose clock limit is 20 000 s away (an nRF TIMER
    after its compare) gives a valid `promise_ns` and a clamped `Time`.
  - `bb-cpu-renode` Renode test `promise_of_u64_max_ns_is_clamped`
    (`tests/lookahead.rs`): the real engine on `la_silent` with
    `Fault::StretchPromise { ns: u64::MAX }`, so the promise is `u64::MAX`
    ns. **Before the fix it panicked** ("Time overflow: value does not fit
    in u64 femtoseconds", `bbcore/src/time.rs:37`); after it, the promise
    is `Time::from_ns(MAX_PROMISE_NS)`, the grant that relied on it is
    capped at the maximum quantum (`(20 µs, 10.02 ms]`), and the audit
    raises `PromiseBroken` inside that grant.
  - `bbcore` scheduler test `a_promise_of_time_max_is_capped_by_max_quantum`
    (`tests/lookahead.rs`): a mock CPU with no timer wakes promises
    `Time::MAX` on the feedback board; every grant is at most
    `max_quantum` (10 ms), some are limited by it, the run ends exactly at
    `t_end`, and every non-grant record equals the `--lookahead off`
    run's.
- **Gate results** (2026-11-09, on `f7a6f8d` plus the change, load 4–11):
  `make check` green on the pool (304 s; the three tests above included),
  `make test POOL=0` green (353 s), `make lookahead-equiv` PASS on the pool
  and with `POOL=0` (73 s each; committed one-cell hashes, no promise
  broken). The clamp moves no grant on today's boards: `bb run` built
  from `f7a6f8d` and from the change give byte-identical `events.jsonl`,
  **grant records included**, plus `nets.csv` and `nets.vcd`, on
  `examples/feedback` at `--max-quantum` 10 ms (117 grants) and 1 ms (562)
  and on `examples/adc` (1 209).
- **Moved to M2:** D6's second gate item, "an nRF gate firmware whose
  TIMER compare is followed by a long sleep", needs the nRF52840 platform
  and gate-firmware build that M2 adds. It becomes part of `make
  nrf-gates` (M2). Until then the clamp is exercised by the injected
  `u64::MAX` promise on the STM32F103, which reaches the same line.
- **Observed, not changed:** when a quiet grant holds both an output
  change and an earlier silent WFI exit, `PromiseBroken` names the output
  change, because the audit checks output changes first (the test above:
  PA5's toggle at 10 ms + 1 fs, while the SysTick wake was at about
  1 ms). The run fails either way; only the `at` in the message is not
  the earliest violation. Left as is, since M1 changes nothing outside
  the clamp.

### Amendment 2: D1-D4 as implemented (M2, 2026-11-09)

- **D1, the platform.** `engines/cpu-renode/platforms/nrf52840-bb.repl`
  (`NRF52840_BB_REPL`, `Platform::Nrf52840Bb`, `RenodeConfig::nrf52840`):
  `using "platforms/cpus/nrf52840.repl"`, `cpu: PerformanceInMips: 64`, and
  a replacement `sysbus: init:` block with the four stock `Tag`s and no
  `ApplySVD`. It names no Breadboard type, so `needs_plugins()` stays false
  and the plugin assembly (and its cache key) is unchanged; the lookahead
  block is mapped at 0x5FFF0000 as on the STM32s. The JIT startup profile
  is keyed by the overlay's text like the other platforms. `bb run` takes
  `*bb part U1 engine=renode platform=nrf52840`.
- **D2, pins.** `PinSpec::nrf("P0.13", dir)` -> `gpio0` pin 13, and
  `bb_cpu_renode::nrf_pin`: P0.00-P0.31 and P1.00-P1.15, one or two digits
  after the dot (`P0.6` = `P0.06`, the name kept as written); P1.16-P1.31,
  P2.x and malformed names are refused (unit test `nrf_pin_names`). The
  board-file pin `U1.P0.13` splits at the first `.` (`board.rs` test
  `nrf52840_pins`); the generated ngspice source is `V_bb_U1_P0_13`. The
  package's AIN table belongs to `nordic/nrf52840-qiaa` (M5); until the
  SAADC model (M8) `bb run` refuses `adc_in` on this platform.
- **D3, the gates.** Bare-metal gate firmware in
  `engines/cpu-renode/tests/firmware/nrf52840/` (`nrf_blinky`,
  `nrf_feedback`, `nrf_ppi`, `nrf_busywait`, `nrf_timer_sleep`,
  `nrf_pincnf`; 448-580 bytes each), built by `make firmware` with the
  pinned arm-none-eabi-gcc 13.2.1 against headers vendored unmodified by
  `scripts/nrf_vendor_headers.sh` into `vendor/` with their sha256s and
  upstream commits in `vendor/SOURCE`: the nrfx MDK's `nrf52840.h`,
  `nrf52840_bitfields.h` and `system_nrf52840.h` (hal_nordic `c708b9fe`,
  BSD-3-Clause) and CMSIS-Core's `core_cm4.h` and the five headers it
  includes (CMSIS_6 `1c1840af`, Apache-2.0, with its `LICENSE`).
  `engines/cpu-renode/tests/nrf52840.rs` drives the real engine with the
  D5 grant rule, as `lookahead.rs` does. `make nrf-gates` runs it and then
  the board-level `make nrf-feedback`. Results (2026-11-09; identical on
  the warm pool and with `POOL=0`):
  - **nrf-blinky:** 6 edges in 300 ms, each within one cell of `k · 1638 /
    32768 s`, identical at one-cell grants (30 000), 10 ms grants (30),
    plugin armed (30 000) and lookahead on (45 grants, 37 promises, all
    honest). The lookahead self-test status is exactly 0x3 (armed, passed)
    on this CPU.
  - **nrf-feedback** (`examples/nrf-feedback`, examples/feedback's
    circuit and pin models on the nRF52840): lookahead on at 1 ms (565
    grants) and 10 ms (120 grants; at most 200 required) gives `nets.csv`,
    `nets.vcd` and the non-grant records of the one-cell `--lookahead off`
    reference (50 000 grants, the committed
    `tests/golden/nrf-feedback/outputs.sha256`), 0 promises broken; the
    reference passes `check_feedback.py --nrf` (9 P0.11 transitions within
    8.8 ns of the analytic RC crossings, every P0.15 edge 1 fs after
    `ceil_L`); `--engine-order analog-first` is byte-identical, grant
    records included. In `make test`.
  - **nrf-ppi:** TIMER1 COMPARE0 -> PPI -> GPIOTE OUT toggles P0.14 at
    `k` ms + 1 fs: with the compare on a lattice point, stock PPI's
    deferred task lands in the same cell (lattice rule 3), and the ISR's
    P0.16 follows 125 ns later. Identical in all four modes; with
    lookahead 60 grants against 2 000. The one-cell latency of an
    off-lattice compare (Context 2) is not exercised by a 1 MHz timer on
    the 10 µs lattice and stays a D9 item.
  - **nrf-busywait (D1):** the nrfx busy-wait (the `SUBS r0,#3; BHI` loop
    of `nrfx_coredep.h`, `time_us · 64` assumed cycles) for 10 000 µs
    takes **6.660 ms** of virtual time (0.666 of the request, against the
    predicted 2·64/(3·64) = 0.667); a 328-tick RTC sleep 10.010 ms
    (328/32768 s = 10.0098 ms, within one tick). Zephyr (below):
    `k_busy_wait(10000)` 6.670 ms, `k_msleep(10)` 10.050 ms. The
    `nordic/nrf52840-qiaa` package (M5) must document both figures.
  - **D6's TIMER-compare-then-long-sleep (moved from M1):** after TIMER0's
    compare at 1 ms the next limit is about 2^64 timer ticks away; 12 of
    13 promises in 120 ms are the clamped `MAX_PROMISE_NS`, every grant is
    at most 10 ms (17 grants), and the edges equal the reference's.
  - **nrf-zephyr:** a Zephyr image (GPIO only; no console, UART, logging
    or boot banner; `BUILD_VERSION` fixed) runs unmodified with identical
    edges at one-cell grants, 10 ms grants, plugin armed and lookahead on
    (742 grants against 30 000).
- **D4, the `PIN_CNF` audit.** `RenodeConfig::pin_cnf_audit` lists the
  audited pins with what their binding expects (`PinCnfExpect`:
  `DigitalOut { drive }`, `DigitalIn`, `Unbound`), and
  `pin_cnf_check` applies D4's rules (DIR against the model; DRIVE against
  the set; an open drive (`D0`/`D1`) against `digital_out` is `error`; an
  unbound output on an analog-scope net is `warn`).
  - **End of run:** `end_of_run` reads `PIN_CNF[n]` of every audited pin
    (a pure `SYSTEM_BUS` read after the last grant) and publishes the
    findings through `RenodeEngine::pin_cnf_findings()`; `bb run` writes
    each as a `log` record at `t_end` (source the part, level `warn` or
    `error`) and on stderr.
  - **Mid-run (D4's condition checked: Renode 1.17.0's hooks do observe
    the writes).** `sysbus SetHookBeforePeripheralWrite` on each audited
    port, restricted to DIR/DIRSET/DIRCLR and `PIN_CNF` (offsets
    0x14-0x27F, so OUT/OUTSET/OUTCLR writes never run Python), appends
    `<t> <port> <offset> <value>` to a file in the engine's workdir and
    returns the value unchanged. `t` is the time source's elapsed virtual
    time, the same clock and lattice rule as GPIO events, measured
    identical for one 25 ms `RunFor` and for 25 of 1 ms. After each
    advance the engine reads the new lines, tracks each audited pin's
    configuration, and logs a `PIN_CNF write:` record (through
    `drain_logs`, at the write's own time) whenever a pin starts to
    disagree with its binding or disagrees differently; it is silent when
    a pin returns to agreement. Gate `nrf_pin_cnf_audit_findings`: the
    five boot-time and ISR-time disagreements of `nrf_pincnf.c` are
    logged at the same times (1 fs, and 10.010125 ms for a DIRSET in an
    RTC ISR that the next ISR reverts) at one-cell grants, 10 ms grants
    and lookahead on; the end-of-run findings are the four that remain;
    edges are identical with and without the audit. The committed
    `nrf-feedback` hashes were made before the hooks existed and still
    match with them.
  - **What a board file can express.** A Phase 0 board file names no
    drive set and no unbound pins, so `bb run` audits the bound pins only,
    with `drive: None` (only open drain is checked against `digital_out`).
    The drive set comes with package bindings (`set=`, M5) and the unbound
    analog-scope pins with the importer's netlist (M4/M6); the engine API
    already takes both.
- **The pool (D1, AGENTS.md rule).** `make pool-equiv` gains
  `nrf-feedback` (lookahead on), `nrf-feedback-off` and
  `nrf-feedback-noplugin` (no lookahead block). PASS: 15 configurations,
  2 rounds in alternating order, all 30 pooled runs leased from one Renode
  process and byte-identical to their fresh runs and to the committed
  hashes. The no-plugin run's `events.jsonl` equals the reference's, grant
  records included, so that configuration is checked on every file
  (`all`). Run twice: on M2 alone, and on the committed tree (6 min).
- **Platform identity (ADR-006 D8).** `nrf52840-bb@1` is a row of M3's
  `cli/src/platforms.rs` (board name `nrf52840`, its content hash over the
  overlay and every plugin source); no package names it until M5's
  `nordic/nrf52840-qiaa`.
- **Not done here, and why.**
  - **The Zephyr gate image's identity** is the sha256 of its loadable
    bytes (`objcopy -O binary` of the committed, debug-stripped ELF) in
    `zephyr/prebuilt/PROVENANCE`, built from the developer workspace
    (Zephyr `76280bce`, v4.5.0-rc3, SDK 1.0.1) because the release pin is
    M6's decision (approver answer 3). `scripts/nrf_zephyr_gate.sh`
    rebuilds it in a scratch directory and requires the same image (two
    builds in different directories gave a byte-identical stripped ELF).
    D5's PT_LOAD image hash comes with `bb flash` (M6).
- **Costs at 64 MIPS** (Context 3 re-measured, `make nrf-costs`: fresh
  Renode, interleaved with the STM32F103 blinky, N = 5, medians, load
  3.4-10): the nRF52840 blinky starts in 1.36 s (STM32F103 1.36 s), a
  one-cell grant costs 0.287 ms (0.297 ms) and a 10 ms grant 4.91 ms
  (4.65 ms). Lower than Context 3's figures (taken at a noisier load), and
  no worse than the STM32F103: the lower MIPS costs nothing measurable.
  The `make cost-table` row (`examples/nrf-feedback`, README): one-cell
  reference 13.93 s of simulation for 500 ms (50 000 grants), lookahead on
  0.52 s (120 grants, 27x); engine startup 1.41 s fresh, 0.05 s from the
  pool.
- **`make test` cost.** `make nrf-feedback` (its one-cell reference runs
  15-40 s depending on load) and the `nrf52840.rs` engine tests inside
  `cargo-test` (about 50 s, in parallel with the other engine tests).
- **Gate results on the committed tree** (M2 on top of S1 and M3,
  `fe80368`, in a separate worktree with its own target directory): `make
  check` on the pool 4 min 12 s, `make test POOL=0` 4 min 58 s (every
  Renode fresh), `make nrf-gates` 51 s on the pool and 1 min 1 s with
  `POOL=0`, `make lookahead-equiv` PASS on the pool and with `POOL=0`,
  `make pool-equiv` PASS (above). The same gates had passed on M2 alone
  (on `f517d43`) before the rebase.

### Amendment 3: D5's example firmware, the image hash and D4 for projects (M6, 2026-11-09)

- **The pin (approver answer 3).** Zephyr **v4.5.0**, the latest release
  tag on 2026-11-09 (`git ls-remote`: commit
  `41d10b59730406bbab4ca9f1fbbe9e96aafb4966`, tag object `d6e5dc6a`), with
  its matching SDK **1.0.1** (v4.5.0's `SDK_VERSION`;
  `arm-zephyr-eabi-gcc` 14.3.0). The west manifest
  `examples/dial-me-spinner/firmware/west.yml` pins Zephyr and the three
  modules an nRF52840 build needs by exact SHA, the ones v4.5.0's own
  `west.yml` pins: `hal_nordic` `c708b9fe` (the commit M2 vendored the MDK
  headers from), `cmsis` `512cc7e8`, `cmsis_6` `1c1840af`. west 1.5.0 and
  the build's Python packages are pinned too (`firmware/requirements.lock`,
  a `pip -c` constraints file). `scripts/dms_zephyr.sh` installs the SDK
  (the minimal bundle plus the `arm-zephyr-eabi` toolchain, each tarball
  checked against a sha256 in the script) into
  `~/.local/opt/zephyr-sdk-1.0.1`, makes workspaces from the manifest
  (`west init -l` on a copy in `<ws>/manifest`, `west update --narrow`),
  builds, and never touches a developer's workspace. `make setup
  SETUP=zephyr` (optional) installs the host tools and the SDK.
- **The board** `dial_me_spinner/nrf52840`
  (`firmware/boards/dial/dial_me_spinner`, hardware model v2,
  `SOC_NRF52840_QIAA`) is the subset of dial-me-spinner `docs/firmware.md`
  section 3.1 at `6854d25` that E1 uses: the `regulator-fixed` VLED on
  `&gpio1 4` (LED_PWR_EN) with `startup-delay-us = 10000` and
  `off-on-delay-us = 30000`, and `board-signals` (`dial,board-signals`, its
  binding in `firmware/dts/bindings`) with `led-din-gpios` on `&gpio0 6`.
  One addition: `led-din-gpios` carries `NRF_GPIO_DRIVE_H0H1`. The product
  drives P0.06 from the I2S with its pinctrl state's H0H1 drive
  (firmware.md 1.3: "H0H1 drive, required"); here the GPIO stands in for
  the I2S's first data edge, so it gets the same drive. LED_PWR_EN keeps
  `PIN_CNF`'s reset drive, S0S1, as firmware.md 1.3 says ("Output").
- **The application** `firmware/led_rail` writes firmware.md 6.4 with the
  APIs it names. On: `regulator_enable(vled)`, then P0.06 high (standing in
  for the first frame; Renode's I2S drives no pin). Off:
  `gpio_pin_configure_dt(&led_din, GPIO_OUTPUT_LOW)`, then
  `regulator_disable(vled)`, then `k_msleep(35)`. 5 ms of "frames" between;
  the cycle repeats forever. Every wait is a `k_sleep` on the RTC system
  timer (D1). `CONFIG_LED_RAIL_WRONG_ORDER` (`wrong_order.conf`, the one
  switch) swaps the off order: LED_PWR_EN low first, P0.06 low 10 ms later
  (`LED_RAIL_WRONG_DIN_MS`), so DIN is high while U2's quick output
  discharge pulls VLED down. `BUILD_VERSION` is fixed (`bb-dms-led-rail-1`)
  and the boot banner, console, UART, printk and logging are off. 18 408 B
  of flash and 4 608 B of RAM.
- **A finding about Zephyr v4.5.0's regulator framework**
  (`drivers/regulator/regulator_common.c` and `regulator_fixed.c`, read for
  this amendment). `regulator_enable()` sleeps for **`off-on-delay-us`**
  after it enables the regulator; `startup-delay-us` is applied only to a
  regulator enabled at boot (`regulator-boot-on`). firmware.md 6.4 expects
  `regulator_enable` to hold P0.06 low for the 10 ms of `startup-delay-us`,
  and `off-on-delay-us` to give the QOD its 30 ms before the next turn-on.
  With this DTS the enable waits 30 ms (P0.06 rises 30.06 ms after
  LED_PWR_EN in every run), so LED-DATA-V4 still holds, with margin; but
  nothing in the regulator enforces the off time before a turn-on: that is
  the firmware's own `k_msleep`. Recorded here for the firmware's authors;
  dial-me-spinner is not modified (it is read-only for this work).
- **Image identity (D5).** The image hash is implemented as specified
  (`cli/src/image.rs`; the byte stream is in `spec/project.md` section 1):
  SHA-256 over a fixed prefix, each `PT_LOAD` segment with file content
  (`p_paddr`, `p_filesz`, its bytes) in load-address order, and the entry
  point. `bb flash` records it (at M4 it recorded the whole file's sha256),
  `bb run` checks it and copies it into `run.json`, and `bb flash
  --hash-only` prints it. The prebuilt ELFs in `firmware/prebuilt/` are
  stripped of debug information (it carries the build path) and listed in
  `SHA256SUMS` by image hash: `led_rail.elf` `5b02dd56...`,
  `led_rail_wrong_order.elf` `382756c5...`. `PROVENANCE` gives the commits,
  the SDK and compiler, west, and a sha256 over the firmware sources.
- **`fw-rebuild`** (`make fw-rebuild`, `scripts/dms_zephyr.sh check`, and a
  CI job): a second `west init` into a new temporary directory, `west
  update`, both builds; the image hashes of the rebuilt ELFs, of
  `SHA256SUMS` and of the committed ELFs must all be equal. **PASS**
  (2026-11-09): both images identical from the fresh workspace.
- **D4 for projects.** For a project, `bb run` now gives the Renode engine
  the full audit list (`cli/src/elab.rs`): every bound pin with the drive
  its parameter set names, and every unbound GPIO of the target whose net
  is in the bench's analog scope. The drive of a set is read from its name
  (`spec/package.md` section 4: `H0H1_max` is H0H1, `default` is S0S1); a
  package schema key was not needed for one platform. On E1 the audit is
  clean for both firmwares at both benches (P1.04 bound S0S1, P0.06 bound
  H0H1_max, the unbound GPIO P0.02 on VSYS_SENSE never an output). Its two
  negative controls in `make dms-rail` fire: P0.06 bound with `set =
  "S0S1"` gives `PIN_CNF write: U10.P0.06 is bound with drive S0S1 but the
  firmware configured PIN_CNF[6] = 0x00000303 (output, drive H0H1)`, and
  P0.06 left unbound gives `PIN_CNF write: U10.P0.06 is not bound, but the
  firmware configured it as an output and its net is in the analog scope
  (it gets no glue, so nothing drives it): ...`.
- **D2 on a module.** U10 is the Raytac module: a binding names the module
  pad (`main:U10.P0.06`), and the module's pinmap gives the chip pin, its
  pin model and the Renode pin (ADR-007 Amendment 3).
- **E1 results** (`make dms-rail`). The edges: LED_PWR_EN up at 0.130 ms
  (after boot), P0.06 up at 30.190 ms, P0.06 down at 35.221 ms,
  LED_PWR_EN down at 35.230 ms, the second cycle from 70.256 ms. The checks
  of `scripts/check_dms_rail.py`, with the suite's limits:

  | Run | LED-DATA-V4 (>= 2.67 V) | LSW-3 (<= 0.7 V) | LSW-2 (<= 30 ms) |
  | --- | --- | --- | --- |
  | correct order, nominal | 3.684020 V at both P0.06 rises | 0.000 V | 24.420 ms |
  | correct order, cold | 2.971510 / 2.971511 V (the suite at bat3.0 -10 C: 2.971511 V) | 0.320 V | 24.420 ms (the suite: 24.059 ms) |
  | wrong order, nominal | 3.684020 V | **0.728 V, FAIL** at 45.26 ms, DIN clamp current 3.64 mA | 26.017 ms |
  | wrong order, cold | 2.971510 V | **0.793 V, FAIL** at 45.26 ms, DIN clamp current 4.08 mA | 27.114 ms |

  The second LED_PWR_EN fall of each run (105.35 ms; 115.39 ms in the
  wrong order) has less than 30 ms of run after it and is reported as not
  evaluable, not as passed. The cold correct-order LSW-3 value (0.320 V,
  against the suite's -5.5e-10 V) comes from the pin model's fixed `voh =
  "VDD"`: 3.3 V, the bench's +3V3 ceiling, while the suite's pad follows
  its VDD node (+3V3 is 2.93 V at the cold bench). Rail-tracking pin
  outputs are Phase 2 (plan section 3, Out). The difference is pessimistic
  for LSW-3 and does not move VLED (the cross-check, ADR-007 Amendment 3).
- **Deviations.** (1) The 10 ms wait of firmware.md 6.4 is 30 ms in the
  image, for the regulator-framework reason above. (2) P0.06 is driven by
  GPIO with H0H1 drive in place of the I2S (above). (3) The firmware
  repeats its cycle, so a 120 ms run holds one complete off sequence, and
  LSW-2 is evaluated on that one. (4) `fw-rebuild` is a make target and a
  CI job; it ran here, but CI is not run (the repository has no remote).

### Amendment 4: D7 and D8 as implemented, M8's first half (2026-11-10)

M8 is split in two. This half is the model and the pin model: the SAADC,
its host port, the loading `adc_in`, the per-sample records, the UART lines
and the VDD input, wired into `nrf52840-bb@2` and the Renode engine, with
the gate `make nrf-saadc-model`. The second half is E2 itself (`vsense.elf`,
the dial-me-spinner project's `[[bind]]` of `VSYS_SENSE` and VDD, the
benches, `make dms-vsense` and its cost-table row).

- **The model** (`engines/cpu-renode/platforms/bb_nrf52_saadc.cs`,
  `Analog.BbNrf52Saadc` at `0x40007000`, `-> nvic@7`; in `PLUGIN_SOURCES`,
  so every Renode platform's content hash and the plugin cache key change).
  As D7, with these implementation choices:
  - One one-shot 1 GHz `LimitTimer` for every pending event (sample, DONE,
    the internal timer's tick, CALIBRATEDONE), armed for the earliest of a
    time-ordered list, as `BbStm32F0Adc`. The next acquisition of a burst
    or scan is started in the DONE handler, so a limit is always armed at
    or before the next sample instant (D7's sampled-input argument).
  - Scan mode: one SAMPLE converts every channel with `PSELP != NC`, in
    channel order, back to back (`a` of the next channel = DONE of the
    previous). A SAMPLE while a scan runs (`STATUS` busy) is ignored and
    counted. Oversampling without BURST takes one acquisition per SAMPLE
    and the result follows the `2^OVERSAMPLE`-th (nrfx's
    `oversampling_without_burst`). `SAMPLERATE.MODE = Timers`: after
    SAMPLE a scan starts at `a_0 + ceil(k * CC * 62.5 ns)`, never
    accumulated, until STOP.
  - **Rounding** (approver decision A5 fixes it per sample): each sample's
    code is `round_half_up` at the configured resolution, saturated
    (single-ended `[0, 2^RES - 1]`, differential `[-2^(RES-1),
    2^(RES-1) - 1]`, sign-extended to 16 bits); the oversampled result is
    the mean of the codes, **also rounded half up** (the PS gives no rule
    for either; recorded here as the model's choice).
  - **EasyDMA:** `RESULT.PTR` and `MAXCNT` are latched at START (STARTED at
    once); each result is a 16-bit system-bus write at `PTR + 2 * AMOUNT`;
    END when `AMOUNT` reaches `MAXCNT`, after which results are dropped
    (counted, a warning) until the next START. STOP cancels every pending
    acquisition, ends a started buffer (END, as nrfx_saadc.c says: "the
    STOP task also triggers the END event") and sets STOPPED.
  - LIMITH/LIMITL are set at the DONE that completes a result, for
    `result >= HIGH` and `result <= LOW`.
  - **Inputs** of the host port: 0-7 are AIN0-7, 8 is VDD (`PSELP = VDD`,
    and the reference of `REFSEL = VDD1_4`, read at the sample instant like
    any input), 9 is VDDH (`PSELP = VDDHDIV5` converts VDDH/5, rounded half
    up). Without a timed input an input reads its constructor default (AIN
    0 V, VDD and VDDH 3.3 V) or its `IADC` channel source.
  - CALIBRATEOFFSET sets CALIBRATEDONE after `calibrationNs` (2 us, an
    assumption: the PS gives no figure) and changes no result.
  - Not modelled (a warning once each): offset and gain errors, the
    RESP/RESN ladders, PPI event outputs (a PPI-triggered SAMPLE needs the
    Breadboard PPI of D3 first), oversampling over several channels (each
    channel averages its own samples; the PS calls it undefined), and
    (added by Amendment 6) the overlap of a scan's channels that the PS
    states without a figure.
- **The host port** (`Miscellaneous.BbSaadcHostPort` at `0x5FFF0100`, ID
  `BBSP`): ADR-005's layout where the meaning is shared (the batch window
  at `0x1000`, the per-input latest value at `0x100 + 4i`, the error word
  and the first error at `0x004-0x010`), so the engine's timed-input
  batches and its `Verify` cross-check are unchanged. New: error bit 5 (a
  refused acquisition, with its TACQ at `0x034`); the configuration words
  `0x080` (L in ns), `0x084` (loaded inputs), `0x088` (traced inputs),
  which the engine writes once at startup; an acquisition ring of the
  loaded inputs (`0x2000`, 256 entries) and a sample ring of the traced
  inputs (`0x3000`, 256 entries: `s`, input, channel, uV, code). Its
  GPIO output 0 toggles at every ring entry and error bit; the engine
  registers an External Control GPIO event on it and reads the port only
  in an advance that had such an event, so a board pays no per-grant read
  for the SAADC. **Deviation from D7's text:** L reaches the model through the host port,
  not as a constructor parameter, so the overlay (and the platform
  identity) does not depend on `--renode-quantum`.
- **The loading `adc_in`, as D7 specifies,** with two refinements:
  - The `#smp` toggle is **not a GPIO**: after each advance the engine reads
    the acquisition ring and reports each start `a_k` as a toggle of the
    output `<pin>#smp` (`PinDirection::AdcSample`), stamped with the lattice
    rule of GPIO edges (exact; 1 fs after a lattice point; a start at the
    grant end carried into the next grant). A GPIO line set from the
    model's timer callback would carry the time source's slice-start stamp
    instead of `a_k`. The toggles merge with the GPIO edges in time order;
    the lookahead audit sees them as output changes.
  - The view (`packages/nordic/nrf52840-qiaa/analog/saadc_se.lib`,
    `pin_load`, ports pad / ctrl `smp` / observe `hold` / pin `VDD`):
    the edge detector `d = |V(smp) - V(smp delayed by tpre)|` opens the
    sample switch while `d > 0.3` (about 24 ns) and pre-charges CS to
    `VPRE = vpre_vdd * V(VDD)` through 100 Ohm while `d > 0.5` (about
    14 ns), so the pad never sees VPRE through the switch. A first version
    with one window for both drew 0.37 pC more per acquisition (3.5 mV on
    the test pad) through the half-open switch at the window's edges; the
    nested windows give the analytic charge sharing to 1 uV. Every element
    is a B conductance (no branch-current unknown, AGENTS.md).
  - Glue (`spec/pin-model.md` rule 9; rule 8 was taken by ADR-007): the
    view's instance `X_bbload_<part>_<pin>`, the toggle's rule-1 source
    `V_bb_<part>_<pin>_smp` (0 V / 1 V, 1 ns edges, no `zout`) on
    `__smp_<part>_<pin>`, the hold node `__hold_<part>_<pin>`, which the
    analog engine pushes, and the VDD port on the net of the VDD binding.
  - **The VDD binding:** an `adc_in` on the package's supply pin `VDD`
    (role `power_in`, no pin model in `pins.toml`) is the SAADC's VDD input
    with the binding's own parameters; `spec/package.md` section 4 states
    this one exception to the role rule, as D7 describes it.
  - **Parameters:** `vref` is the clamp of the pushed value (D7's `vmax`;
    the key name is kept so the package schema needs no change), `bits`
    documents only, `model = "nrf_saadc"`, `load = "<pin_load view>"` or
    `"none"`. The push key is raw uV (`AdcKey::Microvolts`). The
    `nordic/nrf52840-qiaa` package gives the eight AIN pins `adc_in` with
    `adc = { channel = "AIN<n>" }` and a default set `vref = "VDD"`,
    `bits = "12"`, `model = "nrf_saadc"`, `load = "saadc_se"`.
  - `trace_samples = true` is a binding option of the board-file grammar:
    each sample becomes a `debug` log record (`saadc sample: <pin>
    (AIN<n>, CH<m>) <uV> uV, code <c>`) at `s_k`.
  - **The refusal** (approver decision A4): the engine fails the run on
    error bit 5 with "the SAADC acquired the loaded input <pin> (AIN<n>) at
    <t> ns with TACQ <x> us < 2 L = <y> us ... Remedies: --renode-quantum
    <TACQ/2>, or load = "none" on the binding". There is no fallback.
- **D8, the UART lines.** `Miscellaneous.BbUartLog` (`0x5FFF8000`, in the
  overlay) subscribes to `CharReceived` of `uart0` and `uart1` and records
  each byte with the instruction-accurate time of the register write that
  sent it (`cpu.SyncTime()`, as the ADC models do), in a 1 024-entry ring,
  and toggles its GPIO output 0 at every `\n` and every 512th byte. After
  an advance with such a toggle (an External Control GPIO event) the
  engine reads the new bytes and turns each line
  (ended by `\n`; `\r` dropped; other non-printable bytes `\xNN`; split at
  1 024 bytes) into an `info` log record `uart<n>: <text>` at the time of
  its `\n`. `bb run --uart-log on|off` (default on, nRF52840 only). Cost:
  none measurable without UART output (`examples/nrf-feedback
  --lookahead off`, 50 000 grants, three interleaved pairs at load 2.8-9:
  15.01 s on against 14.89 s off, medians, outputs byte-identical). A
  first version read the tap (and the SAADC port) after every advance;
  the GPIO events replaced that before any gate result recorded here. A
  ring overflow in one advance (more than 1 024 bytes, or 256 acquisitions
  or samples) fails the run with a message to lower `--max-quantum`;
  nothing is dropped silently.
- **Platform identity:** `nrf52840-bb@2` (`cli/src/platforms.rs`); the
  overlay names Breadboard types, so `Platform::needs_plugins()` is true
  for it and the plugin assembly loads before it on every nRF52840 run.
  `make locks`: every `bb.lock` (the plugin sources are in every
  platform's hash), `examples/adc/kicad/board.bbm` (the platform content)
  and `examples/dial-me-spinner/board.bbm` (the platform id and content,
  and the module package's tree) changed; no other field.
- **Not done in this half, and why.** The project path (`bb.toml`
  `[[bind]]` with `load`, `trace_samples` and the VDD input) has no example
  yet: the board-file path is the one gated here, and the project path
  goes through the same `run()` glue but is exercised first by E2.
  `saadc_se` has no test vectors, so it is not lattice-qualified
  (`bb pkg lint` warns); `bb pkg test` has no `pin_load` harness yet. The
  board gate's analytic charge-sharing check is its qualification here;
  E2's project run must qualify it (or pass `--allow-unqualified`)
  before `make dms-vsense`. The `la_saadc` gate of the Gates table is
  covered by `bb_saadc_test.py firmware` (WFI exits unchanged across a
  push batch) and by the board gate's lookahead runs (no broken promise
  with pushes inside promised windows, a burst included), not as a
  separate `lookahead.rs` test. M8's measurement against the suite's
  356/761 uV and its tolerance belong to `make dms-vsense`.
- **Gate `make nrf-saadc-model`** (in `make test`, about 25 s):
  - `examples/nrf-saadc/bb_saadc_test.py registers` (`nrf_saadc_idle.elf`,
    the host drives the SAADC; run twice, transcripts byte-identical):
    reset values; ENABLE gating; **144/144** TACQ x RESOLUTION x
    {OVERSAMPLE 1, 2, 8} x BURST cases give `a_k`, `s_k = a_k + TACQ`,
    `a_(k+1) = s_k + 2 us` (BURST) and END exactly at `s_last + 2 us`
    (not 1 ns earlier); the 8x BURST: one SAMPLE, 8 acquisitions 42 us
    apart, codes 1138 1138 1139 1139 1140 1140 1141 1141, result 1140
    (the mean 1139.5 rounded half up); 8 gains x {0.6 V, VDD/4} x 6
    inputs, differential sign extension, PSELP = VDD and VDDHDIV5; a push
    at lattice `T` is not seen at `s = T`, is at `T + 1 ns`; LIMITH/LIMITL
    at DONE and not 1 ns before; EasyDMA (PTR latched, MAXCNT 3, AMOUNT,
    END, nothing written without START, STOP ends the buffer); SAMPLE
    while busy ignored; timer mode `CC = 801` starts at 0, 50 063,
    100 125, ... ns; CALIBRATEDONE at 2 us; IRQ 7 pends on END with
    INTEN.END only; the refusal at TACQ 10 us on a loaded input with
    L = 10 us (error bit 5, TACQ 10 000 ns reported), none at 20 us or
    unloaded; a late push sets error bit 0.
  - `firmware` (`nrf_saadc_burst.elf`, lookahead block armed): 3 bursts of
    8 acquisitions 42 us apart in 20 ms, the first at 5 005 250 ns; the
    UART tap's lines `b=<k> ain0=<mean code> vdd=3413`; the lookahead
    block's WFI-exit counter unchanged across a push batch.
  - `scripts/nrf_saadc_board.sh` on `examples/nrf-saadc` (30 ms, 5 bursts):
    40 `#smp` toggles, 40 samples, each within 1 uV of the charge-sharing
    model (one transfer per acquisition, the 1 TOhm leak included; one
    transfer more or fewer is 20-40 mV away), every code and UART line as
    predicted, no `stop:`; the one-cell every-lattice-point reference
    (3 000 grants) gives the committed `tests/golden/nrf-saadc/
    outputs.sha256`; one cell with pushes on change, lookahead at 1 ms
    (233 grants) and 10 ms (209 grants, 0 promises broken) and
    `--engine-order analog-first` (byte-identical to mcu-first, grant
    records included) equal it in `nets.csv`, `nets.vcd` and the non-grant
    records; `--renode-quantum 25us` fails with "TACQ 40 us < 2 L = 50 us
    ... --renode-quantum 20us, or load = "none""; the same board with
    `load=none` at 25 us runs, with no toggle and every sample 1 200 000 uV.
  - One burst sample at a lattice point (the SAMPLE write at exactly
    20.02 ms in burst 4): its toggle is reported at 20.02 ms + 1 fs, as a
    GPIO write there would be, and its transfer is still exact.
- **Other gates** (2026-11-10, on `8efb0b5` plus this change; load noted
  as `/proc/loadavg` at start and end): `make check POOL=0` green
  (9 min 21 s, load 2.2-1.4; 207 cargo tests), `make check` on the pool
  green (7 min 35 s, load 1.1-1.5), `make nrf-gates POOL=0` PASS
  (1 min 27 s; every committed nRF hash unchanged on `nrf52840-bb@2`),
  `make pool-equiv` PASS (21 configurations with `nrf-saadc` and
  `nrf-saadc-ref` added, 2 rounds, every pooled run leased and identical
  to its fresh run and the committed hashes; 11 min 47 s). Every committed
  golden is unchanged (spike, feedback, ADC, `nrf-feedback`, `dms-rail`,
  `adc-kicad`): the plugin assembly grew and every platform's content hash
  changed, results did not.
- **Cost-table row** (`make cost-table-nrf-saadc`, the ADR-003 protocol,
  N = 5, interleaved, medians; README): `examples/nrf-saadc`, 30 ms, one
  cell (`--lookahead off`) 3 000 grants and 1.55 s of simulation against
  bb's defaults 209 grants and 0.30 s (5.2x), fresh, load 0.7-0.9; on a
  one-slot pool 1.15 s against 0.17 s (6.8x), every run leased. D7's
  estimate of about 40 grants per 8x burst holds (209 for 5 bursts,
  including the RTC wakes and the UART writes).

### Amendment 5: E2 on dial-me-spinner, M8's second half (2026-11-10)

E2 (plan section 2) runs on the imported main board: the firmware reads
VSYS_SENSE through `BbNrf52Saadc` with the loading `adc_in`, and sheds the
LEDs at low battery. The gate is `make dms-vsense`
(`scripts/dms_vsense.sh`, `scripts/check_dms_vsense.py`), with its short
part `make dms-vsense-test` in `make test`.

- **The firmware** (`examples/dial-me-spinner/firmware/vsense/`; D5's
  workspace, toolchain and image hash). It is firmware.md 12.4 with the
  Zephyr ADC API:
  - `adc_read_dt` on channel 0 (gain 1/6, the internal reference, TACQ
    40 us, AIN0, 12 bits, `oversampling = 3`). nrfx's simple mode sets
    BURST, so each reading is one SAMPLE task and 8 acquisitions 42 us apart.
  - VBAT_est = 0.979 x code x 2.8125 mV, in exact integer arithmetic.
    Below 3.53 V the LEDs are shed with E1's off sequence (P0.06 low, then
    `regulator_disable`). They come back at 3.58 V (50 mV of hysteresis),
    and not sooner than 30 ms after the shed.
  - Each reading prints `vsense: n=<k> code=<c> vbat_est=<uV> uV
    leds=<on|off>` on UARTE0 (TX on P1.00, 1 Mbaud), a D8 log record.
  - The first reading is at boot; the rest are on absolute RTC deadlines
    (`k_sleep(K_TIMEOUT_ABS_MS)`). `vsense.elf` reads every 1 s
    (firmware.md 2.2's power_thread tick). `vsense_50ms.elf` reads every 50
    ms, the suite's shortest burst interval. One Kconfig switch
    (`VSENSE_PERIOD_MS`, `fast.conf`) separates the two.
  - The SAADC channel, the `vsys-sense` divider node and UARTE0 are in the
    application's `app.overlay`, not in the board file. So `led_rail.elf`
    and `led_rail_wrong_order.elf` build byte-identically.
  - Image hashes: `vsense.elf` `9a0c6a06...`, `vsense_50ms.elf`
    `f8555d2e...` (`firmware/prebuilt/SHA256SUMS`). `make fw-rebuild` from
    a fresh workspace gives all four committed hashes (181 s, 2026-11-10).
  - **Deviations from firmware.md 12.4:** no production calibration
    (`cal/vsys_gain` = 1), no rested-OCV correction (`I_est x 0.25 Ohm`),
    and only the "< 3.53 V" row of the policy. The plan's E2 compares
    0.979 x code x 2.8125 mV with the threshold.
- **The project path** (Amendment 4 left it to this half):
  - `bb.toml` binds `main:U10.P0.02` (`adc_in`, net VSYS_SENSE,
    `trace_samples = true`, the package set's `load = "saadc_se"`) and
    `main:U10.VDD` (the SAADC's VDD input, `vref = "3.6V"`).
  - A project `[[bind]]` now takes `load` (a string) and `trace_samples`
    (a TOML boolean), as a board file's `*bb pin` does.
  - The VDD input's keys are read in the package value grammar
    (`"3.6V"`) when the part is a package part. The board file's bare
    `vref=3.6` still parses the same way.
  - On a module target, the pad must list the model it is bound with
    (`bb run` checks it; the VDD input is the one exception).
    `raytac/mdbt50q-u1mv2` gives its eight AIN pads `adc_in` with the chip
    pin's channel.
  - `spec/project.md` sections 1 and 6 and `spec/schema/project.schema.json`
    say this.
- **`saadc_se` is lattice-qualified** (Amendment 4: "E2's project run must
  qualify it").
  - `bb pkg test` accepts `pin_load` views. A vector names the typed ports
    by role: `pad`, the signals `smp` and `hold`, and the pin `VDD`
    (`spec/package.md` section 8).
  - `packages/nordic/nrf52840-qiaa/tests/` holds two vectors, each 8 `smp`
    edges with `V(hold)` after every transfer against the closed-form
    charge sharing: `charge_share` (the `nrf-saadc` pad: 1.2 V through
    1 TOhm, 100 pF, VDD 3.0 V) and `vsense_burst` (one 8x burst on the
    dial-me-spinner divider at VSYS 3.0 V, VDD 3.366 V).
  - All 16 values are within 1 uV (tolerance 3 uV), and both partitions
    are identical. The view is recorded as qualified at (10 us, `phase0`),
    and its `confidence` is now `self_checked`.
  - `bb run` on a project now checks the `pin_load` view of every loading
    `adc_in` against qualification, like an instance's view. It records
    the view in `run.json` `analog` under the bound pin, with `kind =
    "pin_load"`, and flags its `agent_generated` provenance.
- **The benches.**
  - `bench/vsense.toml` is the suite's worst case for the sampling error
    (`test_sample_interval`): VSYS an ideal 3.0 V, +3V3 3.366 V (VDD max,
    so VPRE = VDD max), and E1's analog scope.
  - `bench/sag.toml` ramps the battery's open-circuit voltage down 10 mV/s
    from 3.660 V, as a `[[pwl]]` behind 0.3192 Ohm, with E1's 50 mA
    +3V3 load and tracking +3V3.
- **Gate `make dms-vsense`.** Seven cases, each run in a project copy:
  - s1: `vsense.elf`, 2.1 s, three bursts;
  - s50: `vsense_50ms.elf`, 650 ms, 13 bursts;
  - c5: `vsense.elf` with C5 at 10 nF (the suite's fault; the netlist
    edited in the copy), 1.1 s;
  - the three `-none` twins of these, with `load = "none"` on the binding;
  - sag: `vsense.elf` on the sag bench, 4.3 s.

  Every case runs at bb's defaults and as its every-lattice-point
  reference (`--lookahead off --adc-push every`; 210 000, 65 000, 110 000
  and 430 000 one-cell grants against 377, 732, 227 and 731 at the
  defaults). The results:
  - **Analog side.** Per sample, V(hold) of the loaded run minus V(pad) of
    its `-none` twin at the same instant is compared with the suite's
    charge-sharing arithmetic (`_sample`) at the bench's nominal parts and
    the run's own acquisition instants, with saadc_se's one simplification
    (CS on the pad between acquisitions). Steady-state burst means:

    | Case | Co-simulation | Arithmetic | Worst sample | Suite (results.json) | Arithmetic at the suite's corner |
    | --- | --- | --- | --- | --- | --- |
    | s1 | 272.5 uV | 272.6 uV | 0.53 uV | 356.16 uV | 356.15 uV |
    | s50 | 726.4 uV | 726.3 uV | 0.62 uV | 761.27 uV | 761.26 uV |
    | c5 | 2676.6 uV | 2675.0 uV | 5.51 uV | 3474.26 uV | 3475.11 uV |

    **The tolerances are set from this measurement**, at about twice the
    worst value. Per sample the limit is 1 uV + 0.25 % of the modelled
    error; for the burst mean it is 3 uV. The arithmetic at the suite's
    corner must match results.json within 2 uV.
  - **Firmware-visible.**
    - Every reading's code equals the arithmetic's prediction:
      round_half_up of the mean of the 8 rounded sample codes.
    - Every code is within 1 LSB of VSYS / 2.8125 mV plus the modelled
      error. s1 reads 1067 (ideal 1066.67). s50 reads 1067, then 1068 from
      its sixth burst: its 726 uV error (0.83 LSB) crosses a rounding
      boundary, so at 50 ms the sample-rate effect does reach the codes.
    - C5 / 10 reads 1070: 3 LSB above the unloaded 1067. The plan's "about
      4 LSB" is the suite's corner.
    - The LED shed (sag). The LEDs come on at boot (code 1296; P0.06 high
      30.08 ms after LED_PWR_EN). The readings then go 1293, 1289 and
      1286. Reading 4 is 1282 (VBAT_est 3 529 906 uV < 3.53 V), and
      LED_PWR_EN falls at 4000.480 ms. That is 154.4 ms after the
      threshold crossing (3846.1 ms), within the 1 s burst period. P0.06
      falls with the same stamp, and no edge follows.
  - **Equivalence.** Each default run equals its reference in `nets.csv`,
    `nets.vcd` and every non-grant record. The references equal
    `tests/golden/dms-vsense/<case>/outputs.sha256`. There is no `stop:`
    grant (none at a push, so no `stop:ngspice`), and no lookahead promise
    is broken.
  - **Also:** `--engine-order analog-first` is byte-identical on s50. On
    the project path, `--renode-quantum 25us` fails with "TACQ 40 us < 2 L
    = 50 us ... --renode-quantum 20us ... load = "none"". The sag run is
    byte-identical over 10 runs.
- **Deviations from the plan's text** (closest correct gate; no limit
  loosened):
  1. **The suite's figures are at a tolerance corner a Phase 1 bench
     cannot set.** The 356/761 uV and 3.47 mV figures need C5 at -23.5 %
     (-10 % tolerance and -15 % X7R) and R1/R2 at +1 %. Bench parts are
     nominal, and tolerance views are Phase 2 (plan section 2, E1). The gate
     therefore checks two things. The co-simulation must match the
     suite's own arithmetic at the bench's parts. That arithmetic must
     reproduce the suite's figures at the suite's corner. One model ties
     the two. The co-simulation is within 5.5 uV of the arithmetic on every
     sample and within 1.7 uV on the burst means, and the arithmetic is
     within 0.9 uV of the suite's figures. The nominal-part figures are
     smaller (272.5 / 726.4 uV).
  2. **The suite's worst case, not the cold bench.** The plan says "at the
     cold bench with VPRE = VDD". The cold bench's VDD is 2.93 V (its
     tracking +3V3), not the 3.366 V VDD max behind the suite's figures.
     `bench/vsense.toml` takes the suite's own case instead: VSYS 3.0 V,
     VPRE = VDD max.
  3. **"Within one burst period after the threshold crossing"** is
     measured from the firmware-visible crossing: the first instant at
     which V(VSYS_SENSE) plus the modelled mean sampling error is below
     the code boundary 1282.5 LSB. The crossing of 0.979 x V(VSYS) through
     3.53 V (3830.5 ms) is reported. The two differ by the half-LSB
     rounding and the sampling error.
  4. **The off order is checked as "P0.06 falls no later than
     LED_PWR_EN".** Here both edges carry the same stamp, as in E1's second
     cycle (Renode stamps GPIO writes made close together in one block with
     one time).
- **Effect on E1.** E1's runs now carry the two SAADC bindings, so the
  board is cyclic (U10 -> ngspice -> U10) and lookahead is on. E1's
  firmware never starts the SAADC, so the binding adds CPAD + CS to
  VSYS_SENSE and nothing else.
  - Measured against the one-cell references without the bindings: VLED
    moves by at most 1 uV and VSYS by 9 uV at the nominal bench, and
    nothing moves at the cold bench. Non-grant records are unchanged.
  - `nets.vcd` gains the variable `U10.P0.02#smp`, and the grant records
    differ.
  - `tests/golden/dms-rail` was rewritten from fresh one-cell references.
  - `make dms-rail` (fresh) passes with the same values as before (44 PASS
    lines; every run converges in under 3.6 s).
  - The E1 cost-table row was measured again (README).
- **Gate runs** (2026-11-10, on `311d2e8` plus this change, committed as
  `ec759de`; load as `/proc/loadavg` at start and end):
  - `make dms-vsense POOL=0`: PASS, 27 PASS lines and none failing
    (13 min 44 s; load 1.0 at the start and 4.8 at the end, from its own
    parallel runs).
  - `make check POOL=0`: green (10 min 31 s, load 1.5-1.6, 207 cargo
    tests). It now includes `make dms-vsense-test`, which took 57 s fresh.
  - `make check` on the pool: green (8 min 53 s, load 0.6-1.6, 92 leases).
  - `make dms-rail POOL=0`: PASS (2 min 49 s, load 1.1-2.5, 44 PASS lines).
  - `make fw-rebuild`: PASS.
  - `bb pkg test`: 19 vector runs, all passed.

  An earlier run of the gate printed the same check output. It ran before
  the goldens were written, and its refusal case at first stopped on the
  views' qualification at L = 25 us. It now names them with
  `--allow-unqualified`, and the refusal comes from the SAADC.
- **Cost-table row** (`make cost-table-dms-vsense`, the ADR-003 protocol,
  N = 5, interleaved, medians; README; also in `make cost-table`). The
  case is s50: 650 ms, 13 bursts.
  - The every-lattice-point reference takes 65 000 grants and 39.51 s of
    simulation. bb's defaults take 732 grants and 12.85 s (3.1×). Fresh,
    load 1.0-1.2.
  - On a one-slot pool, the same runs take 39.78 s and 12.72 s, with
    startup 0.07 s instead of 1.6 s. Load 1.0-1.4, every run leased.
  - The board's analog deck (E1's scope, TMAX = L) sets the floor, so D7's
    estimate of about 40 grants per burst becomes about 56 here. That
    count includes the RTC wake and the UART line of each reading.
  - E1's row was measured again (`make cost-table-dms-rail`, now cyclic):
    one cell 12 000 grants and 6.83 s; defaults 71 grants and 1.87 s
    (3.7×). Fresh, load 1.2-1.5; pooled 6.54 s and 1.59 s, load 0.9-1.3.

### Amendment 6: the post-M8 review (2026-11-10)

A review of M8 raised nine minor findings. Each was re-checked first.
Nothing here changes a result already recorded: every committed golden is
unchanged, and the one new golden is the new case's.

1. **Scan mode: no overlap between channels** (D7, Amendment 4). The PS
   says "the time it takes to sample all channels is less than the sum of
   the conversion time of all enabled channels", but gives no figure.
   `BbNrf52Saadc` starts the next channel's acquisition at the previous
   channel's DONE, so a scan takes exactly the sum of `TACQ + tCONV`. This
   is now listed as a deviation in the model's header and in Amendment 4's
   not-modelled list. A multi-channel scan's later samples are late by the
   unknown overlap. E2 uses one channel, so E2 is not affected. Modelling
   the overlap needs a source for its size.
2. **VDDHDIV5 with TACQ < 10 us** now gives a warning once ("VDDHDIV5 as
   input with TACQ <x> us: the PS requires 10 us or longer; converted as
   is"), as `SAMPLERATE.CC < 80` already did. The PS limit is "When using
   VDDHDIV5 as input, the acquisition time must be 10 us or longer". The
   check covers PSELP and, in differential mode, PSELN. The conversion is
   unchanged. `bb_saadc_test.py registers` converts VDDHDIV5 at TACQ
   10 us (no warning) and then at 5 us, and requires exactly one warning,
   naming 5 us.
3. **The restore half of the LED policy is now gated.** It was untested,
   and `check_dms_vsense.py` failed any run in which the LEDs came back.
   The new case `rec` (`bench/sag-recover.toml`, `vsense.elf`, 2.3 s) dips
   the battery's OCV to 3.580 V at 1 s and brings it back to 3.720 V at
   2 s. `check_dms_vsense.py`'s `check_rec` requires:
   - the shed at the first reading with code <= 1282 (E1's off order);
   - `leds=off` until the first reading with VBAT_est >= 3.58 V (the
     firmware's integer arithmetic);
   - the restore at that reading, with LED_PWR_EN rising between its last
     sample and its line, no sooner than 30 ms after the shed, and P0.06
     high at least 10 ms after LED_PWR_EN (E1's on order);
   - no further shed;
   - LED_PWR_EN and P0.06 edges exactly `[1, 0, 1]`, and one `leds off`
     and one `leds on` line.

   Readings: 1296, 1269 (VBAT_est 3 494 112 uV, shed: LED_PWR_EN low at
   1000.480 ms), 1314 (3 618 016 uV, restored: LED_PWR_EN high at
   2000.480 ms, P0.06 high 30.065 ms later). The `sag` run fails
   `check_rec` (no reading reaches 3.58 V: the negative control). `rec`
   runs at bb's defaults and as its every-lattice-point reference
   (230 000 one-cell grants), equal except grant records. The reference
   is the new golden `tests/golden/dms-vsense/rec/outputs.sha256`.
   `check_sag` is unchanged: the sag bench never recovers, so LEDs coming
   back there is still a failure.
4. **Grant partitions in the gate.** `make dms-vsense` now also runs s50 at
   `--max-quantum 1ms` and at `--max-quantum 3ms --verify every`. Both
   must equal the committed one-cell hashes except grant records, with no
   broken promise.
5. **The package now says what D7 promised:** `nordic/nrf52840-qiaa`
   `pins.toml`, in the `adc_in` defaults comment. A loaded input needs
   TACQ >= 2 L. Zephyr's default TACQ (10 us) at L = 10 us is refused. The
   remedies are `--renode-quantum <TACQ/2>` or `load = "none"`.
6. **D8's UARTE note in the package.** The `pins.toml` header now says
   that UARTE pin selects are placeholders in Renode, so a UART gives no
   TX or RX pin edge. Its output reaches `events.jsonl` only as D8's
   `uart<n>:` log records. Items 5 and 6 change the package's tree hash
   (`make locks`: `examples/dial-me-spinner/bb.lock` and
   `examples/nrf-saadc/bb.lock`). Item 2 changes the plugin sources, and so
   every platform's content hash: every `bb.lock`,
   `examples/adc/kicad/board.bbm` and `examples/dial-me-spinner/board.bbm`.
   No other field changed.
7. **`run.json` records an explicit `load = "none"`** (D7: "an explicit
   `load = "none"` on the binding, which `run.json` records"). Before, it
   was recorded only by the absence of the `pin_load` entry. Now `analog`
   carries `{"kind": "pin_load", "ref": "<board>:<ref>.<pin>", "package",
   "version", "view": "none"}` for such a binding (`cli/src/elab.rs`;
   `spec/project.md` section 6). A board file (Phase 0 path) writes no
   `run.json`. `scripts/dms_vsense.sh` checks the record on every case, in
   `make dms-vsense-test` too: `saadc_se` on the loaded cases, `none` on
   the `-none` cases.
8. **Connected against switched CS, measured** (D7: "M8 measures the
   difference against the suite's switched model"). `check_dms_vsense.py`
   now evaluates the suite's arithmetic both ways, at the run's
   acquisition instants and the bench's parts, and prints the difference.
   Connected minus switched, steady-state burst mean: +0.000 uV (s1),
   +0.016 uV (s50), +0.000 uV (c5). Worst single sample: 0.000, 0.016 and
   0.001 uV. D7 put the extra capacitance at 25 ppm of C5 (250 ppm for
   C5 / 10). Its effect on the error is far smaller: between acquisitions
   it only lengthens the pad's recovery time constant by that fraction,
   and the pre-charge at each acquisition is the same either way. It is
   reported, not a limit.
9. **`make pool-equiv` has E2.** AGENTS.md requires a new board on the pool
   to be in that list. It now runs s50 at bb's defaults (non-grant hashes)
   and as its every-lattice-point reference (all hashes) against
   `tests/golden/dms-vsense/s50/outputs.sha256`, with
   `dms-vsense-prepare` as a prerequisite. That closes the gap Amendment
   5's gate list left open ("Not run for this milestone").

- **Gate runs** (2026-11-10, on `c18ba59` plus this change; load as
  `/proc/loadavg` at start and end; run one after another):
  - `make nrf-saadc-model`: PASS (25 s, load 0.8-2.7), with the new
    VDDHDIV5 check in both `registers` runs.
  - `make dms-vsense POOL=0`: PASS, 42 PASS lines and none failing
    (15 min 56 s; load 1.1, then 4.9 from its own parallel runs). The
    new lines: `rec` and its reference (230 000 grants against 456 at
    the defaults), the eight run.json load records, s50 at 1 ms (1 317
    grants) and at 3 ms with `--verify every` (888 grants).
  - `make dms-rail POOL=0`: PASS (2 min 47 s, load 4.9-2.1, 44 PASS
    lines, none failing).
  - `make check POOL=0`: green (10 min 30 s, load 2.1-1.6, 207 cargo
    tests).
  - `make check` on the pool: green (9 min 20 s, load 1.6-1.4; 93
    leases, 1 discarded: the `nrf-saadc` refusal run, which fails by
    design).
  - `make pool-equiv`: PASS (23 configurations with `dms-vsense` and
    `dms-vsense-ref` added, 2 rounds, 46 pooled runs from one Renode
    process, each leased and identical to its fresh run and the committed
    hashes; 20 min 47 s, load 1.4-1.3).
  - `make locks` changed only the hashes named in item 6. Every committed
    golden is unchanged. `tests/golden/dms-vsense/rec` is new.

### Amendment 7: D10 as prototyped and implemented (M9, 2026-11-13)

D10 asked for a prototype first. It was run on Renode 1.17.0 with
`arm-zephyr-eabi-gdb` 16.2 (Zephyr SDK 1.0.1), on the spike (STM32F103
blinky, 500 ms) and on E1 (`led_rail.elf`, cold bench, 120 ms), reading
the v1.17.0 sources. Then `bb run --gdb` was built on what it found.

#### What the prototype measured

- **Nothing changes** when GDB attaches, halts the CPU at a slice
  boundary, reads registers or memory and detaches. That held at reset, 3
  of 3 times in the middle of a 5 s spike run, and on E1, all four output
  files byte-identical.
- **A breakpoint stop changes results.** A breakpoint (or any CPU hook)
  ends the CPU's time slice at the halt (`TranslationCPU.cs` 1478-1494,
  then `ReportBackAndBreak`, `TimeHandle.cs` 373). The master time source
  then jumps to the halt instant, and `GPIOPort.cs` 97 stamps a later pin
  change of that slice with it.
  - Spike, a breakpoint at `SysTick_Handler` hit twice: `nets.csv` is
    unchanged; two `pin_edge` stamps move, from 50 ms + 1 fs to
    50 ms + 50 ns and from 100 ms + 1 fs to 100 ms + 40 ns.
  - E1, a breakpoint at `gpio_nrfx_port_set_bits_raw`: the `U10.P1.04`
    rise moves from 130 µs + 1 fs to 137.625 µs; `nets.csv` differs from
    1.82 ms on in 787 of 12 001 rows (VLED by up to 12.29 mV, VSYS by up to
    4.36 mV).
  - A breakpoint at the spike's `main` (140 ns in) shifted every later
    edge by 40 to 150 ns: the slice sequence stays shifted for the rest of
    the run.
- **What does not matter:** how long the CPU is held (5 s holds gave the
  same outputs as none), lookahead on or off and the grant partition
  (E1's breakpoint run was byte-identical at all three), register and
  memory reads at the breakpoint, and three single-steps after it.
- **Renode's own stub is not safe to hand to a debugger.**
  - A step calls `emulation.StartAll()` whenever the emulation is not
    marked started (`BaseCPU.cs` 107-121), which is true during and
    between `RUN_FOR`s. Inside a `RUN_FOR` that is harmless (and costs a
    100 ms sleep). Between two, it starts the free-running time loop.
  - Ctrl-C pauses the CPU in the middle of a slice (`GdbStub.cs` 131-146);
    1 of 3 such runs hung with the `RUN_FOR` never returning.
  - `monitor` reaches every Renode command (`MonitorCommand.cs`),
    `start`, `pause` and OpenOCD's `halt` and `reset init` included.
  - Its step-wait stop replies race the resume packets
    (`MultithreadContinueCommand.cs` says so): 6 of 22 scripted runs lost
    sync, GDB took a park reply for the reply to its `c`, removed its
    breakpoint and the run changed.
  - It listens on every interface (`SocketServerProvider.cs` 38).

D10's three questions: (1) a breakpoint inside a `RUN_FOR` stops virtual
time at the hit and wall-clock waiting changes nothing, but the slice is
cut, so the stamps and sometimes the firmware's timing change; (2)
single-steps after a breakpoint add no further change, measured on both
boards; (3) reads while halted change nothing.

**D10's gate therefore cannot hold for a breakpoint stop**, and D10's own
fallback applies: the mode is debug-only and its outputs carry a mark.
What D10 does hold: a session with no stop inside a slice gives the run
without GDB byte for byte, and that is gated.

#### As implemented

- **The command line.** `bb run --gdb <port>` (0: a free port, printed)
  and `--gdb-wait on|off`. With `on` (the default) the CPU waits at reset
  until a debugger attaches and continues or detaches, like QEMU's `-S`.
  With `off` the run starts at once and a debugger may attach at any time.
  `--gdb` forces lookahead off (D10; `--gdb --lookahead on` is refused),
  always starts Renode fresh (never from the pool, never with the JIT
  startup profile: the stub belongs to the process), and removes the
  External Control read timeout, because a halted CPU holds a `RUN_FOR`
  open.
- **The proxy** (`engines/cpu-renode/src/gdb.rs`). This replaces D10's
  literal "issues `machine StartGdbServer <port> false`" on the user's
  port. Renode's stub starts on an internal port (`autostartEmulation`
  false, as D10 requires). The engine connects to it right after startup,
  before the first `RUN_FOR`, which takes the stub's only client slot.
  The debugger talks to `bb` on `127.0.0.1:<port>`. The engine and the
  kernel are unchanged: the grant boundaries, `ceil_L` and the traces are
  what they are without GDB. A halted CPU keeps the `RUN_FOR` of `(p, g]`
  open with virtual time frozen at the halt instant in `(p, g]`. The
  engine tells the proxy before and after each `RUN_FOR`. The rules:
  - **R1, the resume barrier.** A `c` or `s` reaches Renode only when a
    state read says the CPU is parked inside a running `RUN_FOR` that
    cannot end without it. A Python monitor command reads the CPU's time
    handle, `SourceSideActive` (the time source is running) and
    `TotalElapsedTime` (written by the CPU thread before it parks, so not
    racy), and `IsHalted`. The resume is forwarded only when the source is
    active, the CPU is not asleep in WFI, and its time is before the
    `RUN_FOR`'s end `g`. Otherwise it is held, and the read repeats (after
    a park reply, at the next `RUN_FOR`, or every 2 ms while one is open).
    So a step never reaches Renode outside a running `RUN_FOR` (no
    `StartAll`, no free-run), and a `c` never resumes a paused CPU between
    two `RUN_FOR`s.

    This differs from the prototype's proposal ("only after Renode has
    sent its step-wait stop reply inside the `RUN_FOR`"), because
    implementing it measured two cases where that reply does not come or
    is not enough:
    - A step can execute the last instruction of a `RUN_FOR`'s time (a
      WFI step on E1 skipped 125 ns to the slice end). The `RUN_FOR` then
      ends, and in the next one the CPU is parked **without** sending a
      park reply. Measured with a monitor-driven `RunFor` harness on the
      spike and in a 1 500-step E1 session, which hung until R1 read the
      state instead.
    - A CPU asleep in WFI in single-step mode is halted: Renode's time
      flows without it until it wakes, so a step there would be ignored
      ("Ignoring stepping on a halted CPU") or race the end of the
      `RUN_FOR`.
  - **R2, stop replies.** A step's reply is the proxy's own `S05`, sent
    when Renode's `Step` has returned. A `qC` sent after the `s` is
    answered only then, because `Step` blocks the stub's receive thread.
    Renode's two `S05`s per step are never forwarded. After a `c` the
    debugger gets the first `T` (breakpoint or watchpoint), or an `S02`
    when a requested halt has parked the CPU. The proxy answers `?` itself,
    once the CPU is parked.
  - **R3, Ctrl-C and attach.** Neither reaches Renode as a pause. The
    proxy sends `sysbus.cpu ExecutionMode SingleStep` between two
    `RUN_FOR`s (the engine waits for its reply), so the CPU parks at the
    start of the next one: a slice boundary. The prototype proposed
    switching the mode at once. In a monitor-driven run the first such
    command took 0.4 s, longer than the `RUN_FOR` it raced, so where it
    took effect depended on host timing. Applied between `RUN_FOR`s, with
    the engine waiting for its reply, it takes effect at a boundary fixed
    by the grants, at most one grant of virtual time after the Ctrl-C.
  - **R4, detach.** `D`, `k` or a closed connection remove the debugger's
    breakpoints and watchpoints (the proxy tracks every accepted `Z`) and
    continue the CPU. The proxy keeps Renode's connection. Another
    debugger may attach later; a second one at the same time is refused.
  - **R5, refused packets.** `monitor` (`qRcmd`, with a message and
    `E01`), reverse execution (`bc`, `bs`), `vRun`, continue or step with
    an address or a signal, and Renode's TRACE32 packets. `vCont` is
    answered by the proxy (`vCont;c;C;s;S`) and mapped onto `c` and `s`
    (one CPU, all-stop: any step action is a step). Renode's own `vCont`
    takes a different path (`TryHandleBlockingExecution`, with
    `DelayGrant` and time skips), so it is never forwarded. The proxy also
    edits Renode's `qSupported` reply: it drops `ReverseStep+` and
    `ReverseContinue+` and adds `vContSupported+`. Without that feature
    GDB must assume the target cannot single-step. `arm-zephyr-eabi-gdb`
    16.2 used `s` anyway, but `gdb-multiarch` 15.1 (Ubuntu 24.04, the CI
    GDB) stepped in software, with a temporary breakpoint and a `c` per
    step. Its steps then let time flow, and a `stepi 700` ran the E1 run to
    its end (measured in the gate before the fix).
- **The debug mark.** The proxy records each operation that can change
  results:
  - a breakpoint or watchpoint stop, and inserting a breakpoint or
    watchpoint (conservative: an inserted hook that never fires was not
    measured);
  - a single-step;
  - a register or memory write;
  - a read in the peripheral or system range (`0x40000000`-`0x5FFFFFFF`,
    `0xE0000000` and up), since a register read can have side effects
    (SysTick's `COUNTFLAG` clears on read).

  The first of them becomes one `warn` log record from the CPU engine,
  stamped at the start of the `RUN_FOR` in (or before) which it happened:
  "debug run: GDB: <what> in (p ns, g ns]: ... must not be compared
  against references (bb run --gdb, ADR-008 D10)". This holds for both
  front ends: a board-file run has no `run.json`, and its mark is this
  record. A project run's `run.json` has `"debug": true` for every
  `--gdb` run, with `options.gdb` and `options.gdb_wait`
  (`spec/project.md` section 6, the edit D10 implies; the key is absent
  without `--gdb`). The summary line names the first operation.

  This differs from the prototype's proposal (a `warn` record at t = 0 on
  every `--gdb` board-file run). Marking at the first operation that can
  change results keeps a session that only attached, read and detached
  byte-identical to the run without GDB, which is the part of D10's gate
  that does hold, and gated. And it says where the run stopped being
  comparable.
- **`spec/` changes:**
  - `spec/project.md` section 6: the `debug` key (D10).
  - `spec/time-grant.md`: unchanged. Its rule that an engine must not
    depend on wall-clock time holds for every unmarked run, since those
    are byte-identical to runs without GDB (gated). A marked run can
    depend on when a person interrupted it: Ctrl-C itself changes
    nothing, but a breakpoint set after it is anchored there. D10 and
    "Determinism impact" already put a marked run outside every
    comparison. The prototype suggested one exempting sentence in
    `time-grant.md`. It is left to the approver (open question below),
    not applied.
  - `spec/event-stream.md`: unchanged (the mark is a `log` record).
- **Setup.** `make setup` installs `gdb-multiarch` (Ubuntu 24.04: GDB
  15.1). The gates find `gdb-multiarch`, `arm-none-eabi-gdb` or the Zephyr
  SDK's `arm-zephyr-eabi-gdb`, in that order (`GDB=` overrides).
  `make doctor` reports it.

#### Gates

- **`make gdb-test`** (in `make test`): attach at reset and detach without
  a stop give all four files of the committed hashes of the run without
  GDB (`tests/golden/spike/outputs.sha256`;
  `tests/golden/dms-rail/ok-cold/outputs.sha256`, whose `events.jsonl`,
  grant records included, is E1's one-cell reference, and `--gdb` runs E1
  in one-cell grants). No mark. A breakpoint hit twice and continued
  finishes with exactly one mark, on both boards. E1's `run.json` has
  `debug: true`, `options.gdb` and lookahead off. A run without `--gdb`
  has no `debug` key, no `gdb` option, no mark and the committed hashes.
  `monitor` is refused and changes nothing.
- **`make gdb`** (CI job `dms-rail`) adds:
  - attach in the middle of a running 5 s spike run (no wait, at 1.0, 1.4
    and 1.8 s), read registers and RAM, detach: identical to the run
    without GDB;
  - the scripted breakpoint session 21 times on the spike, the same with
    three single-steps 20 times, and 5 times on E1: each byte-identical
    across repetitions (all four files). This is the case that lost sync
    6 of 22 times in the prototype: GDB's continue follows the attach at
    once, and continuing from a breakpoint steps off it first;
  - single-steps and register and RAM reads at the breakpoint change
    nothing more than the breakpoint;
  - a 5 s hold at the breakpoint with the External Control timeout at
    2 s (`BB_RENODE_IO_TIMEOUT`, a test knob): no failure, outputs equal
    to the session without the hold. The proxy trace shows no `RUN_FOR`
    boundary during the hold and the CPU's time inside the open `RUN_FOR`
    (the run neither free-runs nor passes its grant while halted);
  - Ctrl-C then detach, and Ctrl-C then continue: the runs finish,
    identical to the run without GDB, no mark;
  - on E1, Ctrl-C then 700 single-steps then continue, and 1 500
    single-steps after the breakpoint, across grant boundaries and a WFI:
    both finish. The 1 500 steps change nothing more than the
    breakpoint;
  - `monitor start`, `pause`, `halt` and `emulation RunFor` refused, the
    run unchanged; a second debugger refused while one is attached;
  - GDB killed during a halt: the run finishes. `bb` killed during a
    halt: its Renode exits too (no orphan);
  - the listener is bound to `127.0.0.1` only;
  - `--gdb` with `BB_RENODE_POOL` set never asks the pool and starts
    Renode fresh (control: the same environment makes a run without
    `--gdb` ask it).
- **Not a cost-table row.** `--gdb` is never on by default and its runs
  are not results. Its cost, attached at reset and detached at once,
  medians of 3 runs, load 1.7-2.4 (not the ADR-003 protocol):
  - spike, 500 ms: 1.83 s without GDB and 2.29 s with it (engine startup
    1.44 s and 1.70 s, simulation 0.38 s and 0.56 s);
  - E1, 120 ms, against its one-cell run (`--lookahead off`, the grants
    `--gdb` forces): 8.12 s and 9.41 s (simulation 6.49 s and 7.60 s,
    12 000 grants, about 0.09 ms more per `RUN_FOR` for the two proxy
    notifications).

  Against E1's default run (lookahead on, about 4 s) the forced one-cell
  grants are most of the cost.

- **Gate runs** (2026-11-13/14): `make gdb` PASS with `gdb-multiarch`
  15.1 (5 min 3 s) and with `arm-zephyr-eabi-gdb` 16.2 (4 min 37 s), 33
  PASS lines each; `make check` green fresh (`POOL=0`, 10 min 47 s) and on
  the pool (9 min 17 s); `make pool-equiv` PASS (23 configurations, 46
  leased runs). No committed golden changed.

#### Residual risks

- **Renode's internal stub port** listens on every interface from the
  `StartGdbServer` line until the engine connects, a fraction of a second
  at startup, on a random port. A remote client that connected first would
  hold the stub's only slot, and the run would not start.
- **Not measured:** watchpoints (forwarded and marked), holds of minutes
  or more (no timeout is left that could fire), and multi-CPU boards
  (Phase 1 has one MCU).
- **A halted CPU asleep in WFI does not stop time.** After Ctrl-C the
  CPU parks at the next slice boundary at which it is awake, so a Ctrl-C
  during a long sleep halts at the wake, not at once. Renode's time flows
  meanwhile, inside the grants, as it does for a sleeping CPU.

#### Open question for the approver

- **`spec/time-grant.md`.** Add one sentence exempting a marked debug run
  from the wall-clock rule? **Recommended: yes**, with this text: "A
  debug run (`bb run --gdb`) whose outputs carry the debug mark
  (ADR-008 D10, Amendment 7) is exempt: it may depend on when a debugger
  acts, and it is never compared against a reference."

