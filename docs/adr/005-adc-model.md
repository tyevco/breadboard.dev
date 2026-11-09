# ADR-005: An in-repo STM32 ADC peripheral model (hardware triggers, conversion timing, sampled inputs)

- **Status:** Accepted (2026-11-08), with the approver decisions below,
  including the protocol change of D3 in its generic form. Written before
  implementation because it changes the kernel time-grant protocol (the
  quiet-grant watch, "D3" below) and `spec/` (`time-grant.md`,
  `pin-model.md`).
- **Amends:** ADR-003 D6 (the code-change watch on `adc_in` pins) and
  Amendment 10 (the whole-millivolt push key); supersedes ADR-003
  approver decision 4 ("prefer an existing Renode ADC model") for the
  STM32F072 board, by the user-approved goal "write our own ADC peripheral
  model so ADC boards are realistic and lookahead becomes effective on
  them".
- **Normative changes when implemented:** `spec/time-grant.md` (definition
  of `quiet_until`, a new **sampled input** definition, the early-stop
  watch, Advance step 1, "What engines MUST NOT do"), `spec/pin-model.md`
  (`adc_in`: push key, delivery, conversion mapping). `spec/event-stream.md`
  does not change.
- **Evidence:** read in the Renode 1.17.0 sources (`~/bbscratch-src/renode`,
  tag `v1.17.0`, infrastructure submodule `066a7f13`), checked against the
  installed Renode 1.17.0 by a prototype, and in ST's documents:
  RM0091 Rev 9 (DocID018940, January 2017; ADC chapter 13, TIM2/TIM3
  chapter 18), the STM32F072 datasheet DocID025004 Rev 2 (Table 58, ADC
  characteristics) and RM0008 Rev 6 (ADC chapter 10, `ADC_CR2`). All
  scratch work lives in `~/bbscratch-adc` (measurement scripts `m1.sh`,
  `m2.py`, `count_pushes.py`; prototype `proto/`), outside the repo.

## Approver decisions (2026-11-08)

Recorded after the human approver reviewed this ADR and ADR-004.

1. **The ADR is approved, including its protocol change in the generic
   form (open question 5):** `Engine::input_is_sampled` and the "sampled
   input" definitions in `spec/time-grant.md` and `spec/pin-model.md`,
   the amendments to ADR-003 D6 and Amendment 10, and superseding ADR-003
   approver decision 4 for the STM32F072 board.
2. **Transfer function (open question 1):** `round_half_up(uV · 4095 /
   Vref)`, saturating at 4095, as proposed.
3. **`CKMODE = 00` latency (open question 2):** a fixed 2.625 ADC cycles,
   no jitter, as proposed.
4. **Stock path (open question 3):** kept for now behind
   `--adc-model stock` (with `make adc-renode`, the Amendment 5 test).
5. **Timer period (open question 4):** the `STM32_Timer` `ARR`-tick
   period and fixed 10 MHz clock stay as they are in `BbStm32Timer`, and
   are documented.
6. **DMA one-shot (D1.4):** `DMACFG = 0` is treated as circular, with a
   warning, as proposed.
7. **`TIM1_CC4` (TRG1, D1.3):** unsupported, with a warning, as proposed.
8. **STM32F1 front-end (open question 6, step 7):** deferred until an
   F103 board needs an ADC. D1.6 records whether the design generalises to
   the nRF52840 SAADC (it does, for the core; see there).

## Context

### Where the ADC board's time goes today

`examples/adc` (300 ms) takes 6 344 grants with lookahead against 30 000
one-cell grants: 2.1× (README cost table, Amendment 11). Measured again
for this ADR (single runs, load average 4–5, so read ratios, not seconds):

| Run | Grants | `limited_by` | Simulation |
| --- | --- | --- | --- |
| `examples/adc`, defaults (lookahead on) | 6 344 | 5 140 `stop:ngspice`, 903 `loop_quantum`, 300 `engine:U1`, 1 `engine:ngspice` | 10.34 s |
| `board-open.cir`, defaults (10 ms grants, acyclic) | 30 | `max_quantum` | 3.89 s |

ADC_IN changes its push key (whole millivolts) at 5 353 lattice points in
300 ms (`count_pushes.py` over `nets.csv`), in 144 of the 300 milliseconds,
up to 100 per millisecond. Two costs follow from that, and neither is the
one the task statement names:

1. **The code-change watch (ADR-003 D6) stops a quiet grant at every push.**
   5 140 of the 6 344 grants are `stop:ngspice`. The firmware does not
   convert at those instants; it converts once per TIM3 update. In a flat
   stretch (no pushes) the run already takes exactly 4 grants per
   conversion: a quiet grant to the TIM3 limit, then three one-cell grants
   (measured in 40–50 ms: 41 grants; the pattern is `engine:U1` then three
   `loop_quantum`).
2. **Every push splits a Renode `RUN_FOR`** (D2: `SetValue` while paused on
   the push's lattice point). The open board makes only 30 grants, but its
   3.89 s of simulation are mostly the 5 353 splits: one `RUN_FOR` of 10 µs
   plus one `ADC SetValue` costs 0.50–0.62 ms (`m2.py`, 1000 each, two
   repetitions), against about 2 ms of wall per simulated ms for long
   grants.

The task statement says the 1 kHz own clock of Renode's `STM32F0_ADC`
keeps the CPU from being quiet. That is true only for a board that uses
`EXTEN` or `CONT` (the Amendment 5 `adc_hwtrig.elf` build). The step-9
board starts each conversion from the TIM3 ISR with `ADSTART`, so the
model's own clock never runs there; its CPU wakes once per TIM3 update
because of its ISR, which a hardware-triggered board would not need. What
limits lookahead on `examples/adc` is items 1 and 2.

### Renode 1.17.0's STM32F0 ADC: confirmed limitations

`platforms/cpus/stm32f0.repl` declares `adc: Analog.STM32F0_ADC @ sysbus
0x40012400` (`referenceVoltage: 3.3`, `externalEventFrequency: 1000`,
`dmaPeripheral: dma`, `dmaChannel: 1`, IRQ `nvic@12`). `STM32F0_ADC` is
`STM32_ADC_Common` with one watchdog, 19 channels, `SamplingTime.OneForAll`,
`hasChannelSelect`, `hasScanDirection` and no prescaler. In
`STM32_ADC_Common.cs` (1.17.0):

1. **Own conversion clock.** The constructor takes
   `machine.ObtainManagedThread(StartSampling, externalEventFrequency)`
   (line 123). `ADSTART` (lines 685–698) starts that thread when
   `externalTrigger || CONT`, otherwise it converts at once
   (`StartSampling` in the register write). So continuous mode converts
   at 1 kHz instead of back to back, and a triggered board converts at
   1 kHz whatever the trigger does (Amendment 5 gate 5: 20 conversions
   against 10 TIM3 updates).
2. **Trigger source ignored.** `EXTSEL` is a tag of **two** bits at bit 6
   (`.WithTag("EXTSEL", 6, 2)`, line 553); RM0091 13.12.4 puts
   `EXTSEL[2:0]` at bits 8:6. `EXTEN` only sets `externalTrigger = val > 0`
   (lines 555–560): no edge selection. Renode logged it in the prototype:
   `Unhandled bits: [6-7] when writing value 0x4C004C0. Tags: EXTSEL (0x3)`.
3. **No timer trigger output.** `STM32_Timer` has no TRGO. `CR2.MMS` is a
   tag of two bits at bit 4 (`STM32_Timer.cs` line 174; RM0091 18.4.2: bits
   6:4), and tags are not stored (`PeripheralRegister.Write` updates only
   registered fields), so MMS reads back 0. Its GPIO outputs are `IRQ`,
   `UpdateInterrupt` (level `UIF & UIE`, line 761), `TriggerInterrupt`,
   `CommutationInterrupt`, `CaptureCompareInterrupt`, `BreakInterrupt` and
   the four channel outputs; none is TRGO. Prototype log:
   `Unhandled write to offset 0x4. Unhandled bits: [5] when writing value
   0x20. Tags: Master mode selection (MMS) (0x2)`.
4. **No conversion time.** `SMP` is a tag (line 1014), `CKMODE` has "no
   actual logic" (lines 641, 645). A conversion samples, sets `EOSMP`,
   `EOC`, `AWD` and `EOSEQ` inside the `ADSTART` write (or the thread
   tick): zero sampling and SAR time, zero trigger latency. Amendment 10
   measured the ADC ISR at `T_k + 180 ns`, which is ISR latency only.
5. **Watchdog.** Evaluated only at a conversion (lines 322–332): flag set
   when the converted `sample` is outside `[LT, HT]`, for the channel
   `AWDCH` when `AWDSGL`, else every channel. Deviations from RM0091 13.8
   Table 47: the compared value is the *aligned* sample (left alignment
   shifts it, line 455–458) and, at 10/8/6 bits, the reduced-resolution
   code; silicon compares the raw 12-bit left-aligned data before
   alignment. A `SetValue` never sets `AWD` or wakes the CPU.
6. **Channel values.** `IADC` children are `ADCDefaultChannelSource`s at
   `NumberRegistrationPoint(0..18)`, named `adc-channelN`
   (`IADC.cs`, `RegisterDefaultChildren`). External Control `ADC SetValue`
   (`ExternalControl/ADC.cs` lines 52–56) calls
   `IADCExtensions.SetVoltage(uV, channel)`, which replaces the default
   source's `Sample`. The conversion reads `Sample.Voltage / 1000` (a
   `uint` division: whole millivolts, line 440) and maps
   `Math.Round(mV / (vref·1000) · (2^bits − 1))` (round half even, IEEE
   double, line 454). `ClampSample` clamps to the 16-bit DATA field, not to
   4095 (lines 286–299). All of this is Amendment 5 fact 1–3.
7. **Other gaps.** `DISCEN` and `DISCNUM` tagged (line 576–577); `ADCAL`
   tagged, so calibration takes no time; `ADRDY` is set in the `ADEN`
   write; `AUTOFF` tagged; `ADSTART` is cleared at the end of every
   sequence, also in hardware-trigger single mode (RM0091 13.4.9: it must
   stay set there); DMA one-shot mode unsupported (warning). The next
   channel of a sequence and the `EOC` write go through
   `ExecuteInNearestSyncedState` (lines 473, 765). A queued synced action
   makes the lookahead plugin refuse any promise (`delayedActions`,
   ADR-003 D5).
8. **The other STM32 ADCs do not help.** `STM32G0/L5/H7/WBA/F3_ADC` are
   the same `STM32_ADC_Common` with other feature flags. `STM32_ADC` (F4)
   is not an `IADC` and has no watchdog or triggers (Amendment 5).
   `platforms/cpus/stm32f103.repl` has no ADC at all, and the Renode tree
   has no SAADC model for the nRF52840 (`nrf52840.repl`: no ADC entry; no
   `SAADC` in `Peripherals/`).

### What the platform tools allow (checked by prototype)

- **A repl overlay cannot redeclare a variable's type.**
  `CreationDriver.ProcessVariableDeclarations` raises
  `VariableAlreadyDeclared` for a second typed entry of `adc`. It can,
  however, cancel a registration (`adc: @none`, `TryRegisterFromEntry`
  line 1043–1047) and its interrupt (`-> none`), and declare a new
  variable at the same address.
- **Prototype** (`~/bbscratch-adc/proto`, run against the installed Renode
  1.17.0, firmware `adc_hwtrig.elf`): an overlay that `using`s the stock
  `stm32f072.repl`, unregisters `timer3` and `adc`, and declares
  `bbTimer3: Timers.BbStm32Timer` and `bbAdc: Analog.STM32F0_ADC` at the
  stock addresses loads cleanly. `BbStm32Timer` is a 60-line subclass of
  `STM32_Timer` compiled by `include @….cs` *before*
  `LoadPlatformDescription`; it re-implements `IDoubleWordPeripheral` to
  keep `MMS[2:0]` and adds a `TRGO` GPIO pulsed from `LimitReached`. A
  probe connected to `TRGO` recorded exactly 10 rising edges in 20.5 ms at
  k · 1 999 000 ns (Renode's `ARR` period, Amendment 5 fact 5), the TIM3
  ISR still ran 10 times through `nvic@16`, and External Control found the
  ADC by its new name `bbAdc` (19 channels, `SetValue` works).
- **Batched host writes.** External Control `SYSTEM_BUS` write takes a
  count (`SystemBus.cs` `PerformWrite`, one bus write per dword): one
  request of 400 dwords cost 0.43–1.17 ms, one of 1 dword 0.11–0.13 ms,
  `ADC SetValue` 0.10–0.12 ms, `RUN_FOR` 10 µs 0.49–0.87 ms (`m2.py`,
  load average 3.7–4.2).
- **Clock resolution.** `TimeInterval.TicksPerNanosecond = 1`. A
  `ClockEntry` reaches its limit at the first whole tick at or after it
  (`BaseClockSource`, `wholeTicksToLimit += 1` on a fractional remainder)
  and drops its residue at the limit. A one-shot `LimitTimer` at 1 GHz on
  `machine.ClockSource` therefore fires exactly `n` ns after it is armed,
  and it is a clock limit that `NearestLimitIn` (the lookahead promise)
  sees.

## Decision

### D1. Scope

1. **`BbStm32F0Adc`, an STM32F0x1/x2/x8 ADC (RM0091 chapter 13)**, written
   in C# in the repo (`engines/cpu-renode/platforms/bb_stm32f0_adc.cs`),
   with the register subset and behaviour of D2, the timing of D4 and the
   inputs of D5. It implements `IADC`, so External Control `ADC
   GetCount/GetValue/SetValue` keeps working (D5.4).
2. **`BbStm32Timer`, `STM32_Timer` plus TRGO**
   (`engines/cpu-renode/platforms/bb_stm32_timer.cs`): the minimal
   timer-side addition. It keeps `CR2.MMS[2:0]` (read back, RM0091 18.4.2)
   and drives a `TRGO` GPIO:
   - `MMS = 010` (update): one pulse (`Blink`, set then unset at the same
     instant) per update event, i.e. per `LimitReached` unless `UDIS`, and
     on an `EGR.UG` write unless `UDIS` (RM0091: UG generates an update);
   - `MMS = 000` (reset): one pulse per `EGR.UG` write;
   - `MMS = 001, 011, 1xx`: not modelled; a warning once, no TRGO.

   Its counting is the stock model's, including the `ARR`-tick period
   (Amendment 5 fact 5) and the fixed 10 MHz clock: this ADR changes no
   timer behaviour except the new output.
3. **Trigger inputs.** F0 `EXTSEL` (RM0091 Table 45): TRG0 `TIM1_TRGO`,
   TRG1 `TIM1_CC4`, TRG2 `TIM2_TRGO`, TRG3 `TIM3_TRGO`, TRG4 `TIM15_TRGO`,
   TRG5–7 reserved. The ADC is an `IGPIOReceiver` whose GPIO input `n` is
   TRGn. The overlay wires TRG0, 2, 3, 4 from `BbStm32Timer`s replacing
   `timer1`, `timer2`, `timer3`, `timer15`. TRG1 (`TIM1_CC4`, a compare
   *event*) is not wired: the compare channels are private to
   `STM32_Timer`; selecting it logs a warning and never triggers.
4. **DMA requests: yes** (cheap): at each EOC with `DMAEN` and no pending
   OVR, `dma.RequestTransfer(1)` as the stock repl wires it (STM32G0DMA
   copies one unit per request in the peripheral-to-memory direction,
   Amendment 8). `DMACFG = 1` (circular) is exact. `DMACFG = 0`
   (one-shot) needs the DMA's end-of-transfer, which `IDMA` does not
   report: the model then behaves as circular and warns once (deviation,
   recorded).
5. **STM32F1 (RM0008): not in the first steps, but the design shares a
   core.** The F1 ADC has another register map (`SR`, `CR1`, `CR2`,
   `SMPR1/2`, `SQR1–3`, `JSQR`, `HTR/LTR`), a regular sequencer,
   `EXTTRIG` + `EXTSEL[2:0]` in `CR2` bits 20 and 19:17 (ADC1/2:
   `100` = TIM3_TRGO, `111` = SWSTART; RM0008 Rev 6 10.12.3), injected
   channels and `ADCCLK = PCLK2 / {2, 4, 6, 8}`. `Tconv = sampling time +
   12.5 cycles` is the same (RM0008 10.6). The conversion scheduler, the
   timed inputs (D5), the host port, the code mapping and `BbStm32Timer`
   are family-independent; only a register front-end differs. An F1
   front-end is plan step 7 (optional), mapped at `0x40012400` with IRQ 18
   on `stm32f103-offline.repl`, which today has no ADC.
6. **nRF52840 SAADC (the next reference board): the design generalises.**
   SAADC also converts only on a task (`START`/`SAMPLE`, typically through
   PPI from a TIMER or RTC event), has a fixed acquisition time `TACQ`
   (3–40 µs) plus conversion time, writes results by EasyDMA, and checks
   its `LIMITH/LIMITL` events on each sample, like the AWD. So its inputs
   are sampled inputs in the sense of D3, its conversion instants are
   clock limits, and the core (timed inputs, scheduler, host port) carries
   over. The register front-end, PPI wiring and EasyDMA are new work, and
   the lookahead plugin's self-test must pass on that platform. Not in
   this ADR's steps.

### D2. Register and behaviour subset (RM0091 13.12)

Offsets and reset values as RM0091; reserved bits read 0 and ignore writes.

| Register | Offset | Modelled | Notes |
| --- | --- | --- | --- |
| `ADC_ISR` | 0x00 | `ADRDY` 0, `EOSMP` 1, `EOC` 2, `EOSEQ` 3, `OVR` 4, `AWD` 7 | rc_w1. Reading `ADC_DR` clears `EOC` (13.5.3) |
| `ADC_IER` | 0x04 | `ADRDYIE`, `EOSMPIE`, `EOCIE`, `EOSEQIE`, `OVRIE`, `AWDIE` | IRQ = OR of flag & enable, level, `nvic@12` |
| `ADC_CR` | 0x08 | `ADEN` 0, `ADDIS` 1, `ADSTART` 2, `ADSTP` 4, `ADCAL` 31 | see below |
| `ADC_CFGR1` | 0x0C | `DMAEN` 0, `DMACFG` 1, `SCANDIR` 2, `RES[1:0]` 4:3, `ALIGN` 5, `EXTSEL[2:0]` 8:6, `EXTEN[1:0]` 11:10, `OVRMOD` 12, `CONT` 13, `WAIT` 14, `AUTOFF` 15, `DISCEN` 16, `AWDSGL` 22, `AWDEN` 23, `AWDCH[4:0]` 30:26 | all stored and read back |
| `ADC_CFGR2` | 0x10 | `CKMODE[1:0]` 31:30 | selects the ADC clock (D4) |
| `ADC_SMPR` | 0x14 | `SMP[2:0]` | 1.5, 7.5, 13.5, 28.5, 41.5, 55.5, 71.5, 239.5 cycles |
| `ADC_TR` | 0x20 | `LT[11:0]`, `HT[11:0]` | reset 0x0FFF0000 |
| `ADC_CHSELR` | 0x28 | `CHSEL[18:0]` | |
| `ADC_DR` | 0x40 | `DATA[15:0]` | Figure 37 alignment |
| `ADC_CCR` | 0x308 | `VREFEN` 22, `TSEN` 23, `VBATEN` 24 | gate channels 17, 16, 18 |

Behaviour:

- **On/off.** `ADEN = 1` sets `ADRDY` after `t_STAB`, modelled as 0
  (datasheet Table 58: min 0, typ 0, max 1 µs), i.e. in the write, as the
  stock model does. `ADDIS` clears `ADEN` and `ADDIS` at once (allowed
  only with `ADSTART = 0`; otherwise a warning and no effect). With
  `AUTOFF = 1` `ADRDY` is not set (13.4.2 note) and `t_STAB` is 0.
- **Calibration.** `ADCAL = 1` (only with `ADEN = 0`) reads 1 for
  `t_CAL = 83` ADC clock cycles (datasheet: 83 1/f_ADC, 5.9 µs at
  14 MHz), then clears; `DR[6:0]` then holds the calibration factor, a
  constructor value (default 0). Firmware that polls `ADCAL` now spends
  that virtual time.
- **Start.** `ADSTART = 1` (only with `ADEN = 1`, `ADDIS = 0`): with
  `EXTEN = 00` the first conversion starts now; otherwise the ADC arms and
  waits for an active edge (13.4.9). A trigger edge is taken only while
  armed and idle; a trigger during a conversion, or during a `WAIT` hold,
  is ignored (13.5, 13.7.1 note). `EXTEN = 01/10/11`: rising, falling or
  both edges of the selected TRGn GPIO input.
- **Sequence.** The channels set in `CHSELR`, ascending (`SCANDIR = 0`) or
  descending. `DISCEN = 0`: one trigger converts the whole sequence;
  `DISCEN = 1`: one channel per trigger, `EOSEQ` after the last (13.5.1).
  `CONT = 1`: the sequence restarts immediately (13.4.8). `CONT = 1` with
  `DISCEN = 1` is forbidden: warning, `DISCEN` wins.
- **`ADSTART` clearing** exactly as RM0091 13.4.9 / 13.12.3: at `EOSEQ`
  only in single mode with a software trigger; at `EOC` in discontinuous
  mode with a software trigger; with a hardware trigger or `CONT = 1` only
  by `ADSTP`.
- **`ADSTP`.** Aborts the conversion in progress (its result is
  discarded, `DR` unchanged), resets the sequence, disarms the trigger,
  cancels the model's pending clock events and clears `ADSTP` and
  `ADSTART` at once (13.4.11).
- **End of conversion.** At the conversion's EOC instant (D4): if `EOC`
  is still set, `OVR` is set and, with `OVRMOD = 0`, `DR` keeps the old
  data (13.6.2); otherwise `DR` takes the new data; `EOC` is set; the AWD
  check runs; at the last channel `EOSEQ` is set; a DMA request is made if
  `DMAEN` and not `OVR` (13.6.5: requests are blocked until `OVR` is
  cleared).
- **`WAIT = 1`.** The next conversion (the next channel, the `CONT`
  restart, or an accepted trigger) is held until `DR` is read or `EOC` is
  cleared (13.7.1); it then starts at that instant.
- **Data.** `code12` from D5.2; with `RES` the data is `code12 >> (12 − n)`
  for n = 10, 8, 6 bits; `ALIGN = 1` places it per Figure 37 (12/10/8
  bits at bit 15 down, 6 bits at bits 7:2).
- **Analog watchdog** (13.8, Tables 47–48). On every conversion of a
  guarded channel (`AWDEN`; all channels with `AWDSGL = 0`, channel
  `AWDCH` with `AWDSGL = 1`): `AWD` is set when the raw 12-bit
  left-aligned value (`code12` with the low `12 − n` bits cleared, before
  alignment) is `< LT` or `> HT`. It is set at the conversion's EOC
  instant, never at any other time, and stays set until written 1.
- **Channels 16–18** (temperature sensor, VREFINT, VBAT/2) take their
  values from the timed inputs like any channel, with constructor
  defaults (`tempSensorMicrovolts`, `vrefintMicrovolts` 1 230 000,
  `vbatMicrovolts`); with `TSEN`, `VREFEN`, `VBATEN` clear they convert 0
  (warning once).
- **No `ExecuteInNearestSyncedState`, no managed threads, no host time.**
  Every deferred action is a one-shot `LimitTimer` at 1 GHz on
  `machine.ClockSource` (D4), so it is a clock limit the lookahead promise
  sees, and no synced action is ever queued by this model.
- **Not modelled** (warnings): `AUTOFF` power-down timing (treated as
  always on), the `TSEN`/`VREFEN` stabilisation times, `CKMODE = 11`
  (reserved; treated as `00`), the analog behaviour of the sample-and-hold
  (source impedance, `RAIN`), offset and gain errors, the
  `TIM1_CC4` trigger (D1.3), DMA one-shot end (D1.4). Writes that RM0091
  forbids while `ADSTART = 1` (13.4.4) are applied and logged once, as the
  hardware has "no hardware protection".

### D3. Sampled inputs: the protocol change

**Definition (new, `spec/time-grant.md`).** An input pin of an engine is
**sampled** when a change of its value can affect the engine's outputs and
internal state only at *sample instants* the engine itself schedules, and
the engine's `quiet_until` promise never extends past its next sample
instant. Then, for the purpose of `quiet_until`, the input need not stay
constant: the promise holds for *any* values on sampled inputs, as long
as each change is delivered before the engine advances past it.

- **Trait.** `Engine::input_is_sampled(&self, pin: &str) -> bool`, default
  `false` (`bbcore/src/engine.rs`). Additive: every existing engine keeps
  `false`.
- **Kernel.** In Query, when a quiet grant is built (`query_quiet`), a
  producer's output pin enters `watch` only if at least one engine of `Q`
  reads its net through a **non**-sampled input. Nothing else changes: the
  structural checks (acyclic remainder, exactly one producer feeding `Q`,
  `can_stop_early`, first in the Kahn order), the pre-advance delivery of
  the producer's whole trace to `Q`, the early-stop check (with an empty
  watch it expects `r = G`) and the audit (invariant 8) stay as they are.
- **Renode.** `RenodeEngine::input_is_sampled(pin)` is `true` for an
  `adc_in` pin bound to the `BbStm32F0Adc` model, `false` for everything
  else (digital inputs: EXTI can wake the CPU at once; the stock ADC model
  keeps today's watch).

**Why the promise stays honest with the model.**

1. The model reads a channel's value only at a *sample instant* `s` (the
   end of the sampling phase, D4), and only through D5's lookup. A push
   only appends to the channel's timed input list; it sets no flag, raises
   no interrupt, arms no timer and queues no synced action. So between
   sample instants the value of a sampled input has no effect on anything.
2. Every sample instant is the deadline of a one-shot `LimitTimer`
   (EOSMP). While a conversion chain runs (a sequence, `CONT`), the model
   always has a limit armed at or before its next sample instant: it arms
   `EOSMP_{i+1}` no later than when `EOSMP_i` fires. A chain cannot start
   without either a CPU write (`ADSTART`, or an `EOC` clear or `DR` read
   that ends a `WAIT` hold: the CPU is awake, so there is no promise) or a
   trigger edge from a `BbStm32Timer`, whose update is itself a clock
   limit (`STM32_Timer` is a `LimitTimer`) before the sample it causes.
   Therefore `NearestLimitIn` at Query is at or before the next sample
   instant: `q = clock + NearestLimitIn <= s` for every future `s`.
3. The quiet grant ends at `r <= floor_L(q) <= q <= s`. The sample at `s`
   uses the pushes with `t < s` (D5.2); every push in `(now, r]` reached
   Renode before it advanced (the producer runs first), and any push in
   `(r, s)` reaches it in a later grant before Renode advances past it
   (D5.3). So the run reads the same values at every `s` as the one-cell
   run.
4. The AWD flag and the AWD interrupt appear only at a conversion's EOC
   instant, which is a clock limit `>= s >= q`: a wake by the AWD is a
   wake *at* or after the promise's limit, which the audit allows (a wake
   at `q` is allowed, ADR-003 Amendment 8). **An AWD crossing caused by a
   `SetValue` or a push is therefore reported at the next EOC of a
   conversion of that channel** (RM0091 13.8: the AWD compares converted
   data), never at the push.
5. **The audit stays the safety net.** If a future model or a bug made a
   push wake the CPU, the WFI exit before `q` raises
   `PromiseBroken` (ADR-003 D5.4), and an output change before `q` fails the
   kernel's invariant-8 check: the run fails loudly, it does not diverge
   silently. A kernel mock gate (plan step 3) pins this with an engine
   that declares a sampled input and wakes on it.

**What changes on `examples/adc`.** ngspice is still the one producer
feeding U1 and still runs first in a quiet grant, but with an empty watch
it runs to `G` (U1's promise: the next TIM3 update) instead of stopping at
each push: the 5 140 `stop:ngspice` grants disappear, and the pushes of
`(now, G]` reach U1 in one pre-advance delivery.

**Rejected: keeping the watch and making pushes cheap only.** It removes
cost 2 but keeps 5 140 extra grants (about half of today's simulation
time). **Rejected: a sampled-input flag on the pin model** (`adc_in`)
instead of the engine: whether a pin is sampled depends on the peripheral
model behind it (the stock model and a comparator-style ADC are not), so
the engine that owns the model must say it.

### D4. Timing model (deterministic, virtual time only)

- **ADC clock** `f_ADC` from `CKMODE` (13.4.3, 13.12.5): `00` the
  asynchronous clock (`asyncClockFrequency`, default 14 MHz, the HSI14),
  `01` `pclkFrequency / 2`, `10` `pclkFrequency / 4`. `pclkFrequency` is a
  constructor value (default 8 MHz, HSI after reset), because Renode's
  `STM32F0_RCC` models no clock tree (like the timers' fixed 10 MHz,
  Amendment 5 fact 5).
- **Trigger instant `T`.** Hardware trigger: the clock-source time at
  which the TRGO edge arrives, which is the timer's update limit exactly
  (the prototype's k · 1 999 000 ns). Software trigger: the clock-source
  time of the `ADSTART` write after `cpu.SyncTime()` (instruction
  accurate, as `STM32_Timer` does before reading `CNT`).
- **Latency** `t_LAT` (13.4.3 Table 43, datasheet `tlatr`): `CKMODE = 01`:
  2.75 ADC cycles (= 5.5 PCLK cycles); `10`: 2.625 ADC cycles (= 10.5 PCLK
  cycles); `00`: the datasheet gives 0.188–0.259 µs at 14 MHz, i.e.
  2.625–3.625 cycles with one cycle of jitter; the model uses the lower
  bound, **2.625 cycles, without jitter** (deviation from silicon, which
  is not deterministic in this mode; recorded). The same latency applies
  to a software start (RM0091 Figure 31, note 1: "EXTEN = 00 or EXTEN ≠
  00").
- **Sampling** `t_SMPL` = `SMP` cycles (13.12.6); **SAR** `t_SAR` = 12.5,
  11.5, 9.5, 7.5 cycles for 12, 10, 8, 6 bits (Table 46).
- **DR write latency** `t_W` (datasheet Table 58 `WLATENCY`): `00`:
  1.5 ADC cycles + 2 PCLK cycles (the minimum); `01`: 4.5 PCLK cycles;
  `10`: 8.5 PCLK cycles.
- **Instants of conversion `i` of a chain started at `T`** (exact rational
  arithmetic in 128-bit integers, cycles in eighths, rounded up to whole
  ns only for the scheduled event, never accumulated):
  - start of sampling `a_0 = T + t_LAT`; `a_{i+1} = a_i + t_SMPL + t_SAR`
    for the next channel of a sequence and for the `CONT` restart
    (back to back, 13.4.8); with `WAIT`, `a_{i+1}` is the instant the hold
    ends;
  - **sample instant** `s_i = ceil_ns(a_i + t_SMPL)`: `EOSMP` is set and
    the input value is read (D5.2);
  - **EOC instant** `c_i = ceil_ns(a_i + t_SMPL + t_SAR + t_W)`: `DR`,
    `EOC`, `OVR`, `AWD`, `EOSEQ`, DMA request, IRQ update.
- **Example (the board firmware, D7):** `CKMODE = 01`, PCLK 8 MHz, so
  `f_ADC` = 4 MHz; `SMP = 000`; 12 bits. `s = T + ceil(4.25 cycles) =
  T + 1 063 ns` (1 062.5 ns), `c = T + ceil(16.75 cycles + 4.5 PCLK) =
  T + 4 750 ns` (4 187.5 + 562.5 ns).
- **Mechanics.** One one-shot `LimitTimer` at 1 GHz per pending event
  kind (EOSMP, EOC, `ADCAL`), armed with the ns difference to its
  instant. At 1 GHz the `ClockEntry` ratio is 1, so the event fires
  exactly at that ns, independent of where `RUN_FOR`s end: the model's
  behaviour is a function of the virtual time and the inputs only. The
  added limits are extra sync points of Renode's CPU loop, the same in
  every partition (they do not depend on grants).

### D5. Inputs: timed values, host port, `SetValue` compatibility

1. **Timed input lists.** Each channel has a list of `(t_ns, uV)` entries,
   `t_ns` increasing. The engine writes pushes in batches through a
   **host port**, a second small peripheral `BbAdcHostPort` (constructor
   `adc: bbAdc`) mapped at an unused address (default `0x5FFF0100`, next to
   the lookahead block at `0x5FFF0000`), with one External Control
   `SYSTEM_BUS` write of `2 + 2n` dwords per batch: a header (count,
   flags), then per push `(t_ns − base` in ns, `channel << 24 | ...)` and
   the microvolts; the exact layout is fixed in step 2. Firmware accesses
   to the port are ignored and logged (the `bb_lookahead` test:
   `TryGetTransactionInitiator`, `TryGetCurrentCPU`). The port also reads
   back, per channel, the value in effect after the current time (for
   `Verify`), an error word (D5.3) and a ring of the last conversions
   `(s_i, c_i, channel, code12)` for tests. A batch is applied while
   Renode is paused; it changes no emulation state other than the lists.
2. **Lookup (sample and hold at the lattice).** At a sample instant `s` the
   value of channel `ch` is the `uV` of its **latest entry with
   `t_ns < s`** (strictly earlier). Entries older than the latest such
   entry are dropped. Pushes are at lattice points (`adc_in`), so the ADC
   sees the value of the latest lattice point strictly before `s`, at most
   `L` old, the same rule as today's `SetValue` while paused (ADR-003 D6),
   and it is **never early**: a push at `t` cannot influence a sample at
   `s <= t`. A sample exactly on a lattice point sees the previous cell's
   value, the ADC counterpart of the +1 fs convention (approver
   decision 6). The conversion is
   `code12 = min(4095, floor((2 · uV · 4095 + V) / (2 · V)))` with
   `V = round(vref · 1e6)` µV: round half up of `uV · 4095 / V` in integers
   (RM0091 13.9: `V_CHANNEL = V_DDA · DATA / FULL_SCALE`, `FULL_SCALE =
   4095`), saturating at 4095 as silicon does. `vref` is a constructor
   value (3.3 V; the board's `adc_in vref` must equal it, `bb` checks).
3. **Causality check.** A batch entry with `t_ns < s_last`, where `s_last`
   is the latest sample instant already taken on that channel, would have
   changed a sample that is already done: the port sets its error word and
   ignores the entry, and the engine turns the error into an
   `EngineError::Protocol` after the advance. By the kernel's order this
   never happens (D3 item 3); the check makes "never stale" testable.
4. **`SetValue` compatibility.** External Control `ADC SetValue` keeps
   working on `bbAdc` (`IADC`, default children). A channel that has never
   received a timed entry converts its `IADC` child's `Sample` at `s`
   (any `IRESDSampleSource`, so RESD files work too), in µV with the D5.2
   mapping, with no whole-millivolt truncation. `bb` uses timed inputs
   only; mixing both on one channel is undefined and logged.
5. **Engine side.** For an `adc_in` pin on `BbStm32F0Adc`, the Renode
   engine's `apply_input_trace` appends every `Analog` entry to a pending
   batch instead of queuing a `RUN_FOR` split; before each `RUN_FOR` it
   flushes the batch (one `SYSTEM_BUS` write). Digital inputs keep the D2
   split. `quiet_until` no longer caps a promise at a queued ADC push.
   The `Verify` cross-check reads the port's per-channel value instead of
   `ADC GetValue`. Startup checks the channel against the model (19).

**`adc_in` push key (spec change).** Today the key is whole millivolts,
because the stock model truncates to mV (Amendment 5 fact 1). The model
converts microvolts, so the key becomes **`code12(uV)`** of D5.2, the
finest code the model produces at any resolution or alignment (lower
resolutions and the AWD use `code12` with low bits cleared), and the
delivered value becomes the **raw clamped `uV`** of the lattice point. A
push at every key change then gives the converter, at every lattice
point, the same `code12` as the every-lattice-point reference. `bits` must
be 12 on this model (`bb` checks).

### D6. Loading: compiled once, cached, swapped in by an overlay

- **One assembly** for all of Breadboard's Renode plugins
  (`bb_lookahead.cs`, `bb_stm32_timer.cs`, `bb_stm32f0_adc.cs`), compiled
  once with Renode's own `AdHocCompiler` and cached in
  `~/.cache/bb/renode-plugins` under an FNV-1a 64 key of all sources and
  the canonical Renode launcher path, by the generated loader of ADR-003
  Amendment 8 (`ScanFile` on a hit; compile, load and atomic rename on a
  miss; any failure writes an error file and exits Renode with code 3).
  One `ScanFile` per start, as today; the lookahead block is still mapped
  only on cyclic boards.
- **Order.** The loader runs **before** `LoadPlatformDescription` (the
  overlay names the new types; the prototype did exactly this), then the
  platform, then `bbLookahead`'s `LoadPlatformDescriptionFromString` as
  today.
- **Overlay** `engines/cpu-renode/platforms/stm32f072-bb.repl`:
  `using "platforms/cpus/stm32f072.repl"`; `adc`, `timer1`, `timer2`,
  `timer3`, `timer15` get `@none` and `-> none` (also their named DMA and
  interrupt lines); `bbAdc: Analog.BbStm32F0Adc @ sysbus 0x40012400`
  (`-> nvic@12`, `dmaPeripheral: dma`, `dmaChannel: 1`, clocks and
  `vref`), `bbTimerN: Timers.BbStm32Timer` at the stock addresses with the
  stock frequency, limits, interrupt and DMA lines, and `TRGO -> bbAdc@n`;
  the timers' alternate-function wiring (`bbTimer3: 0 -> gpioPortA#06@1 |
  …`, and `gpioPortA: 6 -> bbTimer3@00 | timer16@00`, …) is restated for
  the new names; `bbAdcPort: Miscellaneous.BbAdcHostPort @ sysbus
  0x5FFF0100`. The unregistered stock objects still exist but are
  unreachable from the bus; their interrupt lines are cancelled.
- **`bb`.** `platform=stm32f072` loads the overlay and names the ADC
  `bbAdc`. A test knob `--adc-model stock` keeps the ADR-003 path
  (stock platform, `SetValue` splits, the code-change watch) for the
  Amendment 5 test and comparisons (approver question 3).
- **Startup gate.** The ADC board's engine startup with a warm cache
  within 0.3 s of a start with only the lookahead plugin (the ADR-003
  step-5 protocol), and a cold start (compile) within 3 s of a warm one.
  This keeps the parallel goal "cut `bb run` startup time" unaffected
  (one assembly, one `ScanFile`).

### D7. `examples/adc`: firmware, board and goldens

- **Firmware** (`adc_board.elf`, `-DADC_BOARD`) becomes the hardware way:
  `CKMODE = 01` (deterministic latency), `SMP = 000`, 12 bits,
  `EXTSEL = TRG3` (TIM3_TRGO), `EXTEN = rising`, `CONT = 0`, `ADSTART`
  once; TIM3 `MMS = update`, **no TIM3 interrupt** (`UIE = 0`); the ADC ISR
  (`EOCIE | AWDIE`) reads `DR`, copies bits 4 and 8 to PA6/PA7, toggles
  PA4 every 50 conversions, drives ALARM on `AWD` and releases it on the
  next in-window code. The CPU sleeps in `wfi` from one EOC to the next.
  `adc.elf` and `adc_hwtrig.elf` stay byte-identical (the stock-model
  test pins them).
- **Board files** (`board.cir`, `board-open.cir`): circuit unchanged; the
  comments and the open board's PULSE timing follow the new PA4 instants
  (PA4 now toggles at the EOC of every 50th conversion).
- **Expected conversion instants:** TIM3 updates at `T_k` (k · 999 µs plus
  the start-up phase, Renode's `ARR` period); sample `s_k = T_k + 1 063 ns`,
  EOC `c_k = T_k + 4 750 ns`; every PA4/PA5/PA6/PA7 edge is stamped `c_k`
  exactly (a limit-driven sync point, D2 rule 2 of ADR-003), or
  `c_k + 1 fs` when `c_k` is a lattice point.
- **`scripts/check_adc.py`** is rewritten for the model: from `nets.csv`
  (ADC_IN on every lattice point) it predicts each conversion's code from
  the value at the latest lattice point strictly before `s_k`, with the
  D5.2 mapping, and every edge with its stamp, and the run must match it
  exactly.
- **Goldens change, by design.** The model's semantics differ (conversion
  instants, µV instead of mV, saturation, AWD at EOC). The **new one-cell
  run** (`--lookahead off --adc-push every`, the model) becomes the
  reference: `make golden-update` rewrites
  `tests/golden/adc/outputs.sha256` and `tests/golden/adc-open/outputs.sha256`
  (reason: ADR-005), after the checks pass. No other golden file changes
  (spike, feedback, feedback-open boards do not load the overlay).

## Expected speed-up on `examples/adc` (300 ms)

- **Grants.** With the watch empty, a flat stretch already costs 4 grants
  per conversion today (measured above), and the moving stretches will
  cost the same: per TIM3 period one quiet grant to `floor_L(T_k)`, then
  the one-cell grant holding `T_k` and `c_k` (sometimes two, when `c_k`
  crosses a lattice point), then two one-cell grants whose Queries the
  back-off skips. **About 1 200–1 500 grants** against 6 344 (4.2–5.3×
  fewer), with 0 `stop:ngspice`.
- **Splits.** 0 `RUN_FOR` splits for pushes (5 353 today). About 150 quiet
  grants carry pushes, one `SYSTEM_BUS` write each (0.1–0.5 ms).
- **Simulation time**, from the per-call costs above and the feedback
  board's ratio (117 grants, 1.19 s per 500 ms): about 900–1 200 one-cell
  grants × 0.45–0.55 ms + 300 ms of quiet grants at about 2 ms per
  simulated ms + about 600 Queries × 0.1–0.14 ms + about 150 batch writes
  ≈ **1.0–1.8 s**, against 7.64 s (README, B median) and 16.13 s (one
  cell): **about 4–7× faster than today's default**, 9–15× faster than
  one cell. The open board drops from 3.89 s (this ADR's single run) to
  about its push-free cost, ≈ 0.6–0.9 s.
- **What remains.** Four grants per conversion: the promise ends at every
  TIM3 update even though no interrupt follows it (the plugin cannot see
  that the limit is silent), and the back-off's skipped Queries after the
  wake cost two more cells. Both are kernel or plugin changes outside
  this ADR (follow-up candidates).
- **Proposed gates** (step 6, `make cost-table` protocol, N = 5,
  interleaved, medians, idle host): `examples/adc` with lookahead on
  takes at most 2 000 grants per 300 ms and at least 3× less simulation
  time than the ADR-003 default (`--adc-model stock`, measured in the
  same protocol run); equivalence as in the test plan.

## Test plan

1. **Timer TRGO** (Renode-only, External Control, a test firmware or host
   register pokes): TRGO pulses at every update instant with `UIE = 0`
   and `UIE = 1`; none with `UDIS`; one per `EGR.UG` with `MMS = 000` and
   `010`; `MMS` reads back; other `MMS` values give no pulse and one
   warning; all other `STM32_Timer` behaviour unchanged (the
   `engines/cpu-renode/tests/lookahead.rs` gate firmware give their pinned
   edges on F103, which does not load the overlay, and a TIM3 PWM on PA6
   through the restated AF wiring on F072).
2. **Register-level unit tests of the ADC** (Renode-only, a `wfi` loop
   firmware with interrupts masked; the host drives the registers through
   `SYSTEM_BUS` and reads the port's conversion ring):
   reset values (`ADC_TR` 0x0FFF0000); `ADRDY`; `ADCAL` duration and
   `DR[6:0]`; the exact `s_i` and `c_i` for every `SMP` × `RES` ×
   `CKMODE` combination against the D4 formulas (computed independently
   in the test); sample-and-hold at the boundary (a push at lattice `t`:
   a sample at `s = t` reads the previous value, at `t + 1 ns` the new
   one); `code12` mapping incl. saturation, `RES` and Figure 37
   alignment; AWD per Tables 47–48 (single channel, all channels, reduced
   resolution, left alignment); `EOC` cleared by a `DR` read; `OVR` with
   `OVRMOD` 0 and 1; `WAIT`; `CONT` back-to-back instants; `DISCEN`;
   `SCANDIR` order and instants; `ADSTP` abort; `EXTEN` rising, falling,
   both; a trigger during a conversion ignored; `ADSTART` kept in
   hardware-trigger single mode; DMA request count; host port: a
   firmware access ignored, a late push sets the error word; two runs
   byte-identical.
3. **Firmware tests** (Renode-only): a TIM3-TRGO-triggered build converts
   at exactly `s_k = T_k + 1 063 ns` and `c_k = T_k + 4 750 ns` for 30
   conversions, with `T_k` the TRGO instants; the AWD interrupt drives
   PA5 at `c_k` of exactly the out-of-window conversions; pushes between
   conversions never wake the CPU (the lookahead block's WFI-exit counter
   does not move across a push batch); the timed-input path and the
   `SetValue` path (paused at the same lattice points, D2 splits) give
   byte-identical transcripts on a profile with no sample instant on a
   lattice point.
4. **Lookahead gate firmware** (`engines/cpu-renode/tests/lookahead.rs`,
   new `la_adc` on the F072 overlay): pushes inside promised windows,
   every promise honoured, outputs equal to the one-cell reference.
5. **Kernel mock gates** (`bbcore/tests/`): a sampled input is not
   watched, and the quiet grant runs to `G` with outputs equal to the
   one-cell run; a non-sampled input on the same board still stops the
   producer (control); an engine that declares a sampled input but wakes
   on a push fails with `PromiseBroken`, from the audit (WFI exit) and
   from the kernel's invariant-8 check (output before `q`).
6. **Board gates** (`make adc`, `make adc-equiv`): the new one-cell
   reference passes `check_adc.py` and matches its committed hashes; Part
   A (one cell, `--adc-push change`), Part B (lookahead at 1 ms and
   10 ms) and the open board at 1, 10 and 50 ms match their references in
   `nets.csv`, `nets.vcd` and every non-grant record; no promise broken;
   10-run determinism; `--engine-order analog-first` identical.
7. **Speed** (`make cost-table`, step 6): the D-gates above; startup gate
   of D6.

## Steps (each gated, each committed after its gates)

1. **Plugin bundling and `BbStm32Timer`.** One assembly, loader before
   the platform, the F072 overlay with the timers (no ADC change yet).
   Gates: test plan 1; `make test`, `make lookahead-inert`,
   `make lookahead-startup` unchanged.
2. **`BbStm32F0Adc` and `BbAdcHostPort`.** Gates: test plan 2 and 3
   (Renode-only harness, like `examples/adc/renode_adc_test.py`).
3. **Kernel: sampled inputs.** `Engine::input_is_sampled`, the watch
   filter, `spec/time-grant.md`. Gates: test plan 5; every existing
   golden and partition hash unchanged (no engine says `true` yet).
4. **Renode engine integration.** Timed batches, `input_is_sampled`,
   `Verify` through the port, `bb` platform switch and
   `--adc-model stock`, `spec/pin-model.md` (`adc_in` key and delivery,
   `bbcore::pin::AdcIn` mapping). Gates: test plan 4; `make adc` with
   `--adc-model stock` reproduces today's committed ADC hashes.
5. **`examples/adc` on the model.** Firmware, board comments,
   `check_adc.py`, `make golden-update` of the two ADC hash files.
   Gates: test plan 6; README (ADC example, `make test` list) and
   AGENTS.md (repo map, the ADC push rule) updated.
6. **Cost table and speed gates.** `make cost-table` row for the ADC
   board on the model; the proposed gates. An amendment here records the
   measured numbers.
7. **Optional: STM32F1 front-end** (RM0008) on `stm32f103-offline.repl`,
   with an F103 ADC test firmware; only if approved.

## Determinism argument

The model's state changes only in register accesses (CPU instructions, in
Renode's serial execution), in its own one-shot clock events and in the
timer's `LimitReached` (both clock-source updates at exact ns), and in
host-port batches applied while Renode is paused. None reads host time or
thread timing. Event instants are pure functions of the trigger instant
and the configuration (D4), and input values are pure functions of the
pushes' lattice times (D5.2), which ngspice produces partition-
independently (ADR-003 D2). So the model's behaviour does not depend on
where `RUN_FOR`s end, on the grant partition or on lookahead, and the
existing equivalence gates apply unchanged.

## Open questions for the approver

1. **Transfer function.** `round(uV · 4095 / V)` (half up, saturating),
   following RM0091 13.9's `FULL_SCALE = 4095`, or the textbook
   `floor(uV · 4096 / V)`? Both are deterministic; the choice changes
   codes by at most one.
2. **`CKMODE = 00` latency.** A fixed 2.625 cycles (the datasheet's lower
   bound) without jitter, as proposed, or the upper bound, or a seeded
   jitter? A seeded jitter would be deterministic but makes conversion
   instants depend on the seed.
3. **Keep the stock path?** `--adc-model stock` keeps the ADR-003
   behaviour reachable (and `make adc-renode`, the Amendment 5 test); it
   costs a second code path in the engine. Alternatively retire it once
   step 5 lands.
4. **Timer period.** Renode's `STM32_Timer` updates every `ARR` ticks,
   not `ARR + 1` (Amendment 5 fact 5), and runs at a fixed 10 MHz. This
   ADR leaves both alone (the TRGO subclass inherits them). Fixing the
   period in `BbStm32Timer` would make TIM3 realistic but would depend on
   private state of `STM32_Timer` (`autoReloadValue`, the preload); a
   separate decision.
5. **Sampled inputs as a protocol term.** The trait method is generic (an
   nRF SAADC or another sampled peripheral can use it). Is the definition
   in D3 acceptable for `spec/time-grant.md`, or should it be limited to
   `adc_in` pins?
6. **F1 front-end (step 7)** now, or when an F103 board needs an ADC?

## Amendments

### Amendment 1: steps 1-3 as implemented (2026-11-09)

**Step 1 (plugin bundling, `BbStm32Timer`, the overlay; commit `ba883f0`).**

- One assembly, `bb_plugins-<key>.dll` in `~/.cache/bb/renode-plugins`,
  keyed by FNV-1a 64 over every source's file name and text
  (`PLUGIN_SOURCES`: `bb_lookahead.cs`, `bb_stm32_timer.cs` and, from step
  2, `bb_stm32f0_adc.cs`) and the canonical Renode launcher. The loader
  (`bb_plugins_load.py`) compiles all sources in one `AdHocCompiler` call
  on a miss. It is now the first line of `session.resc`, before `mach
  create` and `LoadPlatformDescription`, and runs whenever the lookahead
  plugin is configured **or** the platform names a plugin type
  (`Platform::needs_plugins`, true for `Platform::Stm32f072Bb`); the
  lookahead block is still mapped and armed only with
  `LookaheadCfg::plugin`. The pool's pre-warm loads the same assembly and,
  when it holds it, also pre-warms the overlay (third platform after the
  F103 and the stock F072).
- `BbStm32Timer` as D1.2. Implementation notes: the base constructor calls
  `Reset()` before the subclass constructor creates `TRGO`, so `Reset`
  tolerates a null `TRGO`; `CR2` writes reach the base model with the MMS
  bits masked out, so the stock tag warning no longer appears; the
  update-disable state before an `EGR` write decides whether UG pulses
  TRGO with `MMS = 010`; `TrgoPulses` counts pulses (a monitor property, for
  debugging).
- The overlay restates the stock interrupt, DMA and alternate-function
  wiring for `bbTimer1/2/3/15`, including the `gpioPortX: n -> timerN@m`
  inputs. The unregistered stock timers keep their old GPIO connections,
  which never fire.
- Gates: `make check` green with every committed hash unchanged (the ADC
  board ran on the overlay's timers with the stock ADC and gave its
  ADR-003 hashes); `make lookahead-inert` green; `make lookahead-startup`
  +0.069 s with the bigger assembly (N = 7, load 5.2; gate 0.3 s); TIM3 CH1
  PWM on PA6 (`tim3_pwm.elf`) through the restated AF wiring gives the
  stock platform's 19 PA6 edges in 1 ms, byte for byte.
- **Deviation (test order):** the TRGO tests of test plan 1 need an
  observer of the TRGO line. They use the ADC model's trigger-edge ring
  (port offset `0xA00`), so they landed with step 2 rather than step 1.
- Firmware: the former `adc_board.elf` is now built as
  `adc_board_isr.elf` (byte-identical, sha256 `b262c0eb…`), the ADR-003
  board on the stock model; `adc_board.elf` is the D7 build
  (`-DADC_BOARD -DADC_BB_MODEL`); `adc.elf` and `adc_hwtrig.elf` are
  byte-identical to before (the Amendment 5 test pins them). Test firmware
  `bbtest.c`: `adc_idle.elf` (wfi forever) and `tim3_pwm.elf`.

**Step 2 (`BbStm32F0Adc`, `BbAdcHostPort`; commit `6b55b19`).**

- As D2, D4 and D5, with these implementation choices:
  - One one-shot 1 GHz `LimitTimer` (`bbAdcEvents`) for all pending events,
    armed for the earliest of a time-ordered list (sample, end of
    conversion, end of calibration). With slow DR write latencies the end
    of conversion `i` can come after the sample of conversion `i + 1`
    (`CKMODE = 10`: `t_W` = 2.125 ADC cycles > `t_SMPL` = 1.5), which one
    timer per event kind could not express.
  - The next conversion of a chain is scheduled when the previous one's
    sample instant fires, so a limit is always armed at or before the next
    sample instant (D3 item 2).
  - `cpu.SyncTime()` (the instruction-accurate time of an `ADSTART` write
    or of the `DR` read / `EOC` clear that ends a `WAIT` hold) is called only
    for a register access by the CPU itself, never from inside the model's
    clock event or a trigger edge (both are clock handlers, already at their
    exact time; a nested `ReportProgress` would re-enter the clock source).
  - `WAIT` with a hardware trigger: a trigger that arrives while `EOC` is
    still set is accepted and held; the chain starts (with its trigger
    latency) when the data is read.
  - Channels 16-18 with no timed input and a zero channel source read the
    constructor defaults (`tempSensorMicrovolts` 1 430 000, `vrefintMicrovolts`
    1 230 000, `vbatMicrovolts` 1 650 000, the datasheet typicals); with
    `TSEN`/`VREFEN`/`VBATEN` clear they convert 0 (warning once).
  - Host port layout (D5.1 left it to step 2): reads `0x000` ID
    `0x50414242`, `0x004` sticky error word (bit 0 late, 1 out of order,
    2 framing, 3 firmware access, 4 bad channel; a host write clears it),
    `0x008/0x00C/0x010` the first error's time and channel, `0x014` inputs
    accepted, `0x018` conversions, `0x01C` samples, `0x020/0x024` triggers
    accepted/ignored, `0x028` DMA requests, `0x02C` trigger-input edges,
    `0x030 + 4n` rising edges on TRGn, `0x058/0x05C` the ADC's clock time,
    `0x100 + 4ch` the channel's latest timed input, `0x200 + 32j` a ring of
    the last 64 conversions (`T`, `s`, `c`, channel and flags, `code12 |
    DR << 16`), `0xA00 + 8j` a ring of the last 64 trigger-input edges.
    Writes into `0x1000-0x1FFF` are one stream, whatever the offset: header
    `0xBA7C0000 | n`, then `n` entries `(t_ns low, t_ns high (16 bits) |
    channel << 16, uV)`. The engine writes at most 1 020 words per
    `SYSTEM_BUS` request.
- `bb run` kept the stock platform during this step (no output changed).
- Gates (`make adc-model`, `examples/adc/bb_adc_test.py`, in `make test`;
  `make check` green with every committed hash unchanged):
  - **trgo** (test plan 1): TIM1/2/3/15 with `MMS = 010` pulse TRGO at
    exactly `t0 + k · 99 µs` (host-programmed `PSC = 9`, `ARR = 99`: ARR
    ticks at 1 MHz) with `UIE` 0 and 1; none with `UDIS`; one pulse per
    `EGR.UG` with `MMS = 000` and `010`; `MMS` reads back; `MMS = 100` gives
    none and one warning.
  - **registers** (test plan 2): reset values (`ADC_TR` 0x0FFF0000);
    `ADRDY`; `ADCAL` clears exactly 5 929 ns after the write (83 cycles of
    14 MHz) with `DR` = the calibration factor; **96/96** `SMP × RES ×
    CKMODE` sample and end-of-conversion instants equal the D4 formulas
    (computed independently with Python fractions); sample-and-hold: a
    push at lattice `T` is not seen by a sample at `T`, is seen at `T + 1
    ns`; `code12` rounding half up and saturation (0, 1, 402, 403, 329 999,
    330 000, …, 4 000 000 µV); `RES × ALIGN` per Figure 37; AWD single
    channel, all channels, `AWDEN = 0`, the 8-bit raw comparison (a code
    inside the 12-bit window, outside after clearing the low bits) and left
    alignment; `EOC` cleared by a `DR` read; `OVR` with `OVRMOD` 0 and 1;
    `CONT` back to back at the D4 instants; `ADSTP`; `WAIT`; `DISCEN` with
    `SCANDIR`; `EXTEN` rising, falling and both from TIM3 TRGO (with both,
    the falling half of each blink arrives during the conversion of its
    rising half and is ignored: 10 accepted, 10 ignored); `ADSTART` kept in
    hardware-trigger single mode; DMA (DMA1 channel 1 circular: 2 requests
    for a 2-channel sequence, RAM = `DR`, no `OVR`); a late push sets the
    port's error word; two runs byte-identical.
  - **firmware** (test plan 3, `adc_board.elf`, 31 ms): 31 conversions at
    `s_k = T_k + 1 063 ns` and `c_k = T_k + 4 750 ns`, `T_k` the TRGO
    instants, 999 µs apart (`T_1` = 1 019 750 ns: TIM3 starts at the end of
    the calibration, 20 750 ns); ALARM at `c_k` of exactly the
    out-of-window conversions, or at the next lattice point when the ISR's
    write crosses one (16 010 000 ns for `c_k` = 16 009 500 ns: Renode
    stamps a write with the sync point opening its slice, D2 of ADR-003);
    31 WFI exits for 31 conversions (no push batch woke the CPU); the
    timed-input path and the `SetValue` path give byte-identical
    transcripts; after a `SetValue` crossing (0.6 V at 3.5 ms) the AWD
    interrupt drives ALARM at 4 021 500 ns, the end of the first conversion
    sampled after it.
- **Correction to D7:** "every PA4/PA5/PA6/PA7 edge is stamped `c_k`
  exactly" holds only for writes before the next lattice point; a write
  after it is stamped with that lattice point (+1 fs). `check_adc.py`
  accepts exactly these two stamps.

**Step 3 (the kernel; this commit).**

- `Engine::input_is_sampled(&self, pin) -> bool`, default `false`
  (`bbcore/src/engine.rs`), and the watch filter in `query_quiet`: a
  producer's output pin enters `watch` only if an engine of `Q` reads its
  net through an input it does not declare sampled. Nothing else in the
  scheduler changed. `spec/time-grant.md`: the sampled-input definition,
  `quiet_until`, the early stop, Quiet grants step 3, Advance step 1 and
  "What engines MUST NOT do", as D3 specified.
- Gates: `bbcore/tests/sampled_input.rs` (test plan 5): with the input
  declared sampled, no grant stops at a push and every record but the
  grants equals the one-cell run (198 grants for 50 ms against 5 000; two
  pushes exactly on on-lattice sample instants are not seen there); the
  control (same engine, nothing declared) stops 100+ times (487 grants)
  with the same records; an engine that declares the input sampled but
  changes its output at a push fails with `PromiseBroken` from the kernel
  audit (invariant 8), and one that reports its own wake fails with
  `PromiseBroken` from its advance. `make check`, `make partition`, `make
  lookahead-equiv` and `make adc-equiv` green with every committed hash
  unchanged (no engine declares a sampled input yet).
