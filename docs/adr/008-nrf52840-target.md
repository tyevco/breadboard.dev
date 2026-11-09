# ADR-008: nRF52840 as a CPU target (Renode platform, peripheral models, firmware, GDB)

- **Status:** Accepted (2026-11-09), with the approver decisions below
  (revised 2026-11-09 after two design reviews, see Review notes).
  Written before implementation because D7
  changes the `adc_in` pin model (`spec/pin-model.md`) and D6 fixes a
  lookahead bug in the Renode engine.
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
| `make nrf-gates` | M2 | D3, including `nrf-busywait` (D1) |
| Promise-clamp unit test | M1 | D6 |
| `make pool-equiv` with nRF configurations | M2 | AGENTS.md rule |
| `make dms-rail` | M6 | E1: one-cell equivalence, determinism, goldens. The PIN_CNF audit (D4) is clean for the correct firmware |
| `make nrf-saadc-model` | M8 | Register map; TACQ × RESOLUTION × OVERSAMPLE × BURST acquisition and DONE instants; an 8× burst gives 8 `smp` toggles and 8 charge transfers at **both** engine orders; EasyDMA PTR/MAXCNT/AMOUNT; LIMIT events; the VDD reference; host-port timed inputs; the TACQ < 2·L refusal and its message |
| `la_saadc` in `lookahead.rs` | M8 | A sampled input that never wakes the CPU before `q`, with a burst |
| `make dms-vsense` | M8 | E2: analog-side and firmware-visible checks (plan §2), equivalence to the every-lattice-point reference, `--engine-order` independence, no `stop:ngspice` |
| GDB equivalence | M9 | D10 |
| `fw-rebuild` (CI job where a Zephyr workspace can be created) | M6 | Image hashes of ELFs rebuilt in a fresh workspace equal `SHA256SUMS` |
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
