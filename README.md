# Breadboard

Breadboard is an open hardware simulator, aiming to be "QEMU for whole circuit boards". You give
it firmware and a board, and it simulates both together. CPU engines run the
firmware, analog engines solve the circuit, and a kernel that owns
simulated time couples them at the pin boundary.

**Phase 1** (`docs/phase1-plan.md`) runs a real board: it imports a KiCad
project, resolves its parts to component packages (`packages/`,
`spec/package.md`), and runs Zephyr firmware on an nRF52840 against the
analog part of the board. Its reference demo is below. Phase 0, the spike
the rest of this README describes, came first.

## Quickstart: the reference demo

The demo is the LED rail power sequencing (E1) of a real board,
dial-me-spinner rev A, imported from its KiCad netlist (201 parts, 142
nets; vendored in `examples/dial-me-spinner/kicad/`). The firmware is
prebuilt Zephyr v4.5.0 running on the nRF52840 inside the Raytac module U10,
in Renode. It switches the TPS22917 load switch U2, which powers the
48-LED WS2812C ring (VLED), and it drives the ring's data line. ngspice
solves the part of the board the bench puts in scope (132 instances). The
check script then tests three rules from the board's own SPICE suite, and
the firmware rule's own timing:

- **LED-DATA-V4:** VLED is at least 2.67 V whenever data starts.
- **LSW-3:** DIN never goes more than 0.7 V above VLED.
- **LSW-2:** VLED falls below 10 % within 30 ms of switch-off.
- **FW-6.4:** data starts at least 10 ms after LED_PWR_EN rises, and
  LED_PWR_EN stays low at least 30 ms before it rises again.

You need a clean Ubuntu 24.04 (a container, a WSL distro or a machine) and
network access for the toolchain downloads (about 800 MB: Rust, Renode).
You do **not** need KiCad, the Zephyr SDK, an ARM toolchain or the
dial-me-spinner repository. As root (for example in a container), run:

<!-- stranger-test:begin -->
```sh
apt-get update && apt-get install -y git make ca-certificates   # as a user: sudo apt-get update && sudo apt-get install -y git make ca-certificates
git clone https://github.com/tyevco/breadboard.dev breadboard.dev && cd breadboard.dev
make setup SETUP=demo                                            # Rust 1.99.1, Renode 1.17.0, ngspice 42; builds bb into ~/.local/bin
export PATH="$HOME/.local/bin:$PATH"
cd examples/dial-me-spinner
bb open                                                          # imports the pinned netlist, resolves parts, writes board.bbm
bb flash --target U10 firmware/prebuilt/led_rail.elf
bb run --for 120ms --bench bench/cold.toml --probe net:VLED --probe net:LED_DIN --out out/rail
python3 ../../scripts/check_dms_rail.py out/rail                 # PASS: LED-DATA-V4, LSW-3, LSW-2, FW-6.4
bb flash --target U10 firmware/prebuilt/led_rail_wrong_order.elf
bb run --for 120ms --bench bench/cold.toml --probe net:VLED --probe net:LED_DIN --out out/rail-wrong
python3 ../../scripts/check_dms_rail.py out/rail-wrong           # expected to FAIL: LSW-3
```
<!-- stranger-test:end -->

> To try a local copy instead, clone from its path or `file://` URL, for
> example `git clone /path/to/breadboard.dev breadboard.dev`.

`make setup SETUP=demo` takes most of the time. It installs the apt
packages, Rust 1.99.1, Renode 1.17.0 and ngspice 42, then builds `bb`.
Each run then takes a few seconds. Timed from a local clone, the whole
block took 1 min 54 s in a clean `ubuntu:24.04` container and 1 min 47 s
in a fresh WSL distro. `make setup` took about 50 s of that, the local
clone 25–45 s, and each run under 4 s. That was on a 24-CPU host with a working Ubuntu mirror
(Phase 1 plan, "Review notes: M7"). The 15-minute budget leaves room for a
slower network, but it assumes a reachable Ubuntu mirror: almost all of the
time is apt and the downloads, and Breadboard's own steps take seconds.

**If `apt-get` is very slow or times out** (the review of M7 measured
20 min for this block with `ubuntu:24.04`'s default sources while
`archive.ubuntu.com` ran at 63 kB/s and `security.ubuntu.com` timed out,
and 71 s with another mirror), point apt at another Ubuntu mirror and run
the line again, for example as root:

```sh
sed -i 's#http://\(archive\|security\).ubuntu.com/ubuntu#http://mirrors.edge.kernel.org/ubuntu#' /etc/apt/sources.list.d/ubuntu.sources
```

If `apt-get` fails with "Mirror sync in progress?", run the line again;
`make setup` retries on its own.

The correct-order firmware follows dial-me-spinner's `docs/firmware.md` 6.4.
It raises LED_PWR_EN before it sends data, and it takes the data line low
before it drops LED_PWR_EN. In this Zephyr image, data starts 30 ms after
LED_PWR_EN, not the 10 ms that 6.4 gives (ADR-008 Amendment 3); FW-6.4
only requires at least 10 ms. All four checks pass:

```text
check_dms_rail: PASS: LED-DATA-V4: VLED 2.971510 V at 30.190000 ms (P0.06 rose 30.060 ms after LED_PWR_EN), ...
check_dms_rail: PASS: LSW-3: max(V(LED_DIN) - V(VLED)) = 0.320308 V at 30.400 ms ...
check_dms_rail: PASS: LSW-2: LED_PWR_EN fell at 35.230000 ms from VLED 2.971510 V: below 10 % after 24.420 ms; ...
check_dms_rail: PASS: FW-6.4: P0.06 rose 30.060 ms after LED_PWR_EN; ...; LED_PWR_EN off for 35.026 ms before rising again ...
check_dms_rail: PASS: LED-DATA-V4, LSW-3, LSW-2, FW-6.4
```

The wrong-order firmware reverses the switch-off. It drops LED_PWR_EN
first and keeps the data line high while VLED discharges. Its waits are
unchanged, so FW-6.4 passes. LSW-3 fails, and the message gives the
excursion and the clamp current it drives into the first LED's DIN. The
script exits 1:

```text
check_dms_rail: FAIL: LSW-3: max(V(LED_DIN) - V(VLED)) = 0.792868 V at 45.260 ms (VLED 1.486643 V, LED_DIN 2.279511 V;
    DIN clamp current 4.083 mA from the R40 drop ...) (limit <= 0.7 V)
check_dms_rail: FAIL: LED-DATA-V4, LSW-3, LSW-2, FW-6.4 (failed: LSW-3)
```

What the steps do:

- **`bb open`** imports the netlist and writes `board.bbm`. The netlist is
  dial-me-spinner `6854d25`, KiCad 10.0.8, with its sha256 in
  `examples/dial-me-spinner/SOURCE`. It resolves each part to a package by
  its MPN or as a generic R, C or L, and writes `board.bbm`. The committed
  `board.bbm` is identical, so it reports `unchanged`. With KiCad 10
  installed, the schematics give the same `board.bbm`, and dial-me-spinner's
  KiCad 9 netlist (`dc96515`) imports to it too (`make kicad-import`; no
  gate runs KiCad 9's own `kicad-cli`).
- **`bb flash`** records the image and its hash in `bb.toml`.
- **`bb run`** writes the following to `out/rail/`:
  - `nets.csv` and `nets.vcd`: VLED, LED_DIN and the nets the project
    probes;
  - `events.jsonl`: every pin edge;
  - `deck.cir`: the elaborated SPICE deck;
  - `run.json`: every analog view in the run, with its provenance,
    confidence and qualification, plus the solver option set.

  `bb run` also notes the views whose provenance is `agent_generated` (the
  ideal R and C, the TPS22917's datasheet-fitted view, and the SAADC input
  load on VSYS_SENSE that E2 binds; E1's firmware never starts the SAADC).
- The two runs' `nets.csv`, `nets.vcd` and non-grant records are
  byte-identical to the committed hashes in `tests/golden/dms-rail/`.

From the repository root, **`make demo`** does all of the above in one
command and also checks those hashes. It works on a copy of the project, so
the checkout is never written.

`make stranger-test-container` and `make stranger-test-wsl` run the block
above line by line and time it. The first uses a clean `ubuntu:24.04`
container; the second uses a freshly imported WSL distro, which it removes
afterwards. Both clone from this repository's local path.

## E2: battery reading and the low-battery LED shed

The second worked example (Phase 1 plan section 2 E2, M8) runs on the same
imported board. The firmware reads the battery the way dial-me-spinner's
`docs/firmware.md` 12.4 does it: VSYS_SENSE (VSYS through R1 2.2 MΩ over
R2 1 MΩ, with C5 100 nF) on AIN0 (P0.02), the SAADC at gain 1/6 against
the 0.6 V reference, 12 bits, 8× oversampling with BURST and TACQ 40 µs,
and VBAT_est = 0.979 × code × 2.8125 mV. When VBAT_est falls below 3.53 V,
the firmware sheds the LEDs with E1's off sequence. Each reading is a UART
line, and so a log record.

The SAADC is Breadboard's own model (ADR-008 D7), because Renode 1.17.0 has
none. The input **loads** VSYS_SENSE, as the silicon does. At each
acquisition, a 2.5 pF sample capacitor pre-charged to VDD shares its charge
with C5 and the 3 pF pad. The firmware reads the shared value. That charge
drawn at every acquisition makes the reading depend on how often the
firmware samples. dial-me-spinner's own SPICE suite measured that effect.
Every sample is also a log record of the voltage the model sampled.

```sh
make dms-vsense        # the M8 gate, about 14 min; make dms-vsense-test (in make test) is the short part
```

What the gate checks (`scripts/dms_vsense.sh`, `scripts/check_dms_vsense.py`):

- **The analog side.** Each loaded run is compared with the same run with
  `load = "none"` on the binding, which reads the unloaded pad. The bench is
  `bench/vsense.toml`, the suite's worst case: VSYS 3.0 V, with the sample
  capacitor pre-charged to VDD max, 3.366 V. The mean error of the 8 samples
  in a burst, in steady state, is:

  | Interval | Co-simulation | The suite's arithmetic, nominal parts | The suite at its corner (C5 −23.5 %, R +1 %) |
  | --- | --- | --- | --- |
  | 1 s (`vsense.elf`) | 272.5 µV | 272.6 µV | 356.2 µV |
  | 50 ms (`vsense_50ms.elf`) | 726.4 µV | 726.3 µV | 761.3 µV |
  | C5 fitted at 10 nF, one burst | 2 676.6 µV | 2 675.0 µV | 3 474.3 µV |

  The suite's figures need part tolerances that a Phase 1 bench cannot set.
  So the check ties the co-simulation to them through the suite's own
  charge-sharing arithmetic. At this bench's nominal parts, that arithmetic
  must match every sample (within 1 µV plus 0.25 %) and the burst mean
  (within 3 µV). At the suite's corner, it must reproduce the suite's
  figures within 2 µV.
- **Firmware-visible.** Every code is the one that arithmetic predicts. The
  codes follow code = V / 2.8125 mV to within the rounding and the sampling
  error: 1067 at 1 s, and 1068 from the sixth burst at 50 ms, where the
  0.83 LSB error crosses a rounding boundary. C5 ÷ 10 reads 1070, 3 LSB
  high.
- **The LED shed** (`bench/sag.toml`). The battery's open-circuit voltage
  ramps down by 10 mV/s behind 0.32 Ω. The LEDs come on at boot (code
  1296). The readings then go 1293, 1289 and 1286. At the reading of 1282,
  VBAT_est is 3.5299 V, and the firmware sheds the LEDs: LED_PWR_EN falls
  154 ms after the threshold crossing, within one reading period.
- **Equivalence and determinism.** Every case equals its every-lattice-point
  reference (`--lookahead off --adc-push every`) in `nets.csv`, `nets.vcd`
  and every non-grant record. The references match `tests/golden/dms-vsense/`.
  No grant stops early at a push (`stop:`). `--engine-order analog-first` is
  byte-identical. The sag run is identical over 10 runs.

The firmware is in `examples/dial-me-spinner/firmware/vsense/`. Both images
are prebuilt, and `make fw-rebuild` reproduces them from a fresh Zephyr
v4.5.0 workspace. Details are in ADR-008 Amendment 5.

## Phase 0

Phase 0 was a spike that tested the project's top risk: is
co-simulation sync too slow or unstable? Firmware runs on an STM32F103 in
[Renode](https://renode.io) and toggles PA5. The pin drives a 330 Ω resistor
and a red LED, and [ngspice](https://ngspice.sourceforge.io) solves that
circuit. Both engines advance only on time grants from the Rust kernel
(`bbcore`, `spec/time-grant.md`). The output must be byte-identical from run
to run.

```
U1 (STM32F103, Renode)        generated glue                    board.cir
  PA5 digital_out  ──edge──▶  V_bb_U1_PA5 (EXTERNAL) ─ 25 Ω ─▶ PA5 ─ R1 330 Ω ─ LED_A ─ D1 (red) ─ GND
  {3.3 V, 5 ns, 25 Ω}         (from the pin model)
```

## Developer setup

Run these inside WSL Ubuntu 24.04 or Linux, from the repo root:

```sh
make setup   # pinned toolchain: Rust, Renode 1.17.0 portable, arm-none-eabi-gcc, ngspice 42; installs bb into ~/.local/bin
make test    # cargo tests + 500 ms spike + 10-run determinism + golden diff + feedback and ADC boards + packages
```

`make setup SETUP=demo` installs only what running boards needs (Rust,
Renode, ngspice and `bb`, no ARM toolchain); it runs as root without sudo,
for example in a clean `ubuntu:24.04` container (`make setup-container`
times exactly that). A clean Ubuntu needs `apt-get install make` first.

From Windows: `wsl -d Ubuntu-24.04 -- bash -lc 'cd /mnt/c/source/tyevco/breadboard.dev && make setup && make test'`.

Run the simulation by hand:

```sh
make firmware bb
~/.cache/bb-target/release/bb run --netlist examples/spike/board.cir \
    --firmware examples/spike/firmware/build/blinky.elf --target U1 --for 500ms \
    --probe net:LED_A --probe net:PA5 --out out/spike   # [--max-quantum 10ms] [--renode-quantum 10us] [--sample-rate 100kHz] [--verify edges]
```

The run writes these files to `out/spike/`:

| File | Contents |
| --- | --- |
| `events.jsonl` | One `grant` record per grant, plus `pin_edge` records with Renode's exact edge times (`spec/event-stream.md`, schema version 1) |
| `nets.csv` | `t_fs,LED_A,PA5` sampled every 10 µs, in volts with 6 decimals. Samples never add grant boundaries: inside each grant, analog values are read from ngspice's solver points (one lands on every 10 µs lattice point) and digital values from the edge trace (ADR-002 Amendment 3, ADR-003 D3) |
| `nets.vcd` | The probed analog nets as `real`, sampled at `--sample-rate`, and the MCU pin net `U1.PA5` as a wire with every edge (timescale 1 fs) |

`bb` prints the wall-clock time to the terminal, never into the outputs.

## What `make test` checks

1. `cargo test --workspace`: scheduler mock tests, the ngspice RC test against
   the analytic answer, the Renode blinky edge-timing tests and the CLI's
   unit tests. `make test` sets `BB_REQUIRE_RENODE=1`, so the Renode tests
   fail rather than skip when Renode or the ARM toolchain is missing.
2. `make spike`: the 500 ms run, then `scripts/check_spike.py`. The LED_A high
   plateau must be in [1.7, 2.2] V and the low plateau below 0.1 V. There must
   be at least 8 PA5 edges and at least 8 LED_A transitions, each spaced
   50 ms ± 0.1 ms. LED_A must have switched by the first sample after each PA5
   edge.
3. `make determinism`: 10 runs. The sha256 of `nets.csv`, `events.jsonl` and
   `nets.vcd` must be identical across all of them.
4. `make golden`: LED_A from the spike run is diffed against
   `tests/golden/spike/led_a.csv` (a 10 kHz snapshot) within 1 mV / 1 sample,
   and the run's `nets.csv`, `events.jsonl` and `nets.vcd` must match
   `tests/golden/spike/outputs.sha256` byte for byte.
5. `make feedback`: 500 ms of `examples/feedback`, a board with a feedback
   loop (below), then `scripts/check_feedback.py`, the golden snapshot
   `tests/golden/feedback/` and the one-cell reference hashes (lookahead
   changes only `grant` records).
6. `make feedback-order`: the same run with the engines registered in the
   other order must be byte-identical.
7. `make adc`: 300 ms of `examples/adc`, an STM32F072 that reads back
   through its ADC the divider it drives (below), then `scripts/check_adc.py`
   and the hashes of the one-cell reference that pushes the ADC value at
   every lattice point (lookahead changes only `grant` records);
   `make adc-order` (the engines registered in the other order),
   `make adc-stock` (the ADR-003 board on Renode's stock ADC, `--adc-model
   stock`, against its own references) and `make adc-model` (the Renode-only
   tests of Breadboard's ADC model and timer TRGO, ADR-005).
   `make nrf-saadc-model` (Phase 1 M8, ADR-008 D7/D8): the Renode-only tests
   of Breadboard's nRF52840 SAADC, then `examples/nrf-saadc`, whose AIN0
   input loads its pad (one charge transfer per acquisition, counted
   against the analytic charge sharing) and whose firmware's UART lines
   become log records, against its one-cell every-lattice-point reference.
   `make dms-vsense-test` (Phase 1 M8, E2): the battery reading on
   dial-me-spinner at 50 ms and with C5 fitted at 10 nF, each against its
   `load = "none"` run, with `scripts/check_dms_vsense.py` and the committed
   hashes (below, "E2").
8. `make packages` (ADR-006): the package files and lockfiles against
   `spec/schema`, `bb pkg lint`, `bb pkg test` (each view's test vectors
   through the ngspice engine at two grant partitions) and the examples'
   `bb.lock` files, and that nothing local-only is committed (the
   vendor-derived TPS22917 view lives only in the git-ignored
   `local-packages/`; the committed `ti/tps22917` is datasheet-fitted,
   Phase 1 M5b).

The example boards take their MCU and its pin models from component
packages (`packages/`, `spec/package.md`): `*bb part U1
package=st/stm32f103c8` instead of writing every pin parameter out. Each
board's `bb.lock` pins the package's tree hash and the Renode platform's
content hash; `bb open <board.cir>` writes it, `bb run` checks it. The
outputs are byte-identical to the Phase 0 boards' committed hashes.

`make feedback-determinism` (10 runs), `make feedback-open` (the partition
gate of the open-loop variant), `make lookahead-inert` and
`make lookahead-equiv` (the lookahead equivalence gate) run in their own CI
job, and `make adc-equiv` (the ADC gates), `make adc-determinism` (10 runs)
and `make adc-renode` (the Renode-only test of the stock ADC model) in
another.

## Feedback example

`examples/feedback` is a board on which the MCU reads back a node it drives
(ADR-003 "Feedback-path correctness"):

```
U1.PA5 ─ 25 Ω ─ R2 1 kΩ ─ SENSE ─ C1 1 µF ─ GND
                          SENSE ──▶ U1.PB0 digital_in {vih 2.0 V, vil 0.8 V, cin 5 pF}
U1.PA6 ─ 25 Ω ─ R3 330 Ω ─ LED_B ─ D2 (red) ─ GND
```

The firmware toggles PA5 every 50 ms (SysTick). An EXTI0 interrupt on both
edges of PB0 copies PB0 to PA6. ngspice thresholds SENSE with the
`digital_in` hysteresis at every solver point and reports each crossing,
interpolated between solver points, as a PB0 edge. PB0 rises 0.954859 ms
after each PA5 rise, in the 10 µs cell 50.95–50.96 ms, and falls 1.452500 ms
after each fall, in 101.45–101.46 ms. Renode sees the change at the end of
that cell, `ceil_L(t_x)`, and PA6 follows 1 fs later. The loop
U1 → ngspice → U1 caps a grant at one Renode quantum (10 µs), except while
the MCU sleeps: with CPU lookahead (ADR-003 Part B, on by default since plan
step 8) the sleeping MCU promises to stay quiet until its next timer event,
ngspice runs first and stops early at the lattice point after a PB0
crossing, and the run takes 117 grants instead of 50 000 with byte-identical
`nets.csv`, `nets.vcd` and non-grant records (`make lookahead-equiv`;
`--lookahead off` gives the one-cell reference). Firmware that never sleeps
(the polling variant `feedback_poll.elf`) gets no promise and stays in
one-cell grants. Opening the loop (`examples/feedback-open`, SENSE driven by
a circuit source) gives long grants with byte-identical outputs too.

## ADC example

`examples/adc` (ADR-003 Part C, ADR-005) runs on Breadboard's STM32F072:
Renode's stock platform with Breadboard's own ADC model (`BbStm32F0Adc`,
RM0091: hardware triggers, conversion timing, the analog watchdog) and
timers with a trigger output (`BbStm32Timer`, TRGO), loaded from one cached
plugin assembly (`engines/cpu-renode/platforms/stm32f072-bb.repl`):

```
U1.PA4 ─ 25 Ω ─ R1 10 kΩ ─ ADC_IN ─ R2 33 kΩ ∥ C1 470 nF ─ GND
                           ADC_IN ──▶ U1.PA1 adc_in {vref 3.3 V, 12 bits}
U1.PA5 (ALARM) ─ 25 Ω ─ R3 330 Ω ─ LED ─ D1 (red) ─ GND
```

TIM3's update event (every 999 µs; Renode counts ARR ticks) triggers each
conversion through TRGO, with no TIM3 interrupt. The ADC samples ADC_IN
1 063 ns after the trigger and ends the conversion 4 750 ns after it
(PCLK/2, 1.5 cycles of sampling, 12 bits, RM0091 and the datasheet's
latencies); its interrupt then drives ALARM from the analog watchdog
([1.0 V, 2.0 V]), copies code bits 4 and 8 to PA6/PA7 and toggles PA4 every
50th conversion. ngspice pushes ADC_IN at the 10 µs lattice points where its
12-bit code changes; the Renode engine writes the pushes of a grant to the
ADC model as timed inputs, and a conversion reads the latest push strictly
before its sample instant. Because the model reads its input only at sample
instants that the sleeping MCU's lookahead promise never passes, the ADC pin
is a *sampled input* (ADR-005 D3): quiet grants run to the next TIM3 update
whatever ADC_IN does, and an out-of-window value is reported at the next end
of conversion, as on silicon. The run takes about 1 200 grants for 300 ms
instead of 30 000, with byte-identical outputs (`make adc-equiv`).
`board-open.cir` drives the divider from a circuit source instead (no loop):
30 grants of 10 ms. `bb run --adc-model stock` (with
`firmware/build/adc_board_isr.elf`) runs the ADR-003 board on Renode's stock
`STM32F0_ADC` (`make adc-stock`).

## Result

LED_A in `nets.csv` is a clean 10 Hz square wave:

- **Before the first edge:** 0 V until the firmware's first SysTick toggle at
  50 ms.
- **High plateau:** a flat **1.866945 V**, the red LED's forward voltage at
  (3.199 − 1.867) V / 330 Ω ≈ **4.0 mA**.
- **PA5 when high:** sits at 3.199 V, because 25 Ω × 4 mA is dropped in the
  pin's output impedance.
- **Low plateau:** exactly 0 V.
- **Edges:** PA5 changes level at 50, 100, …, 450 ms and 499.99987 ms. The
  later edges drift by about 20 ns per period because of the timer model
  rounding in Renode. That drift is deterministic and is well inside the
  ±0.1 ms tolerance.
- **LED_A transitions:** each one has completed by the next 10 µs sample. The
  5 ns pin ramp is applied at Renode's exact edge timestamp: the kernel
  advances the CPU engine first in every grant because it drives ngspice's
  input (the plan order, ADR-003 D3), and every edge of the grant reaches
  ngspice before ngspice advances through it (ADR-002 Amendment 2).

`out/spike/nets.vcd` opens in GTKWave. Plot `LED_A` and `PA5` (real) together
with `U1.PA5` (wire) to see this.

## Measured cost (the "co-sim is slow" risk)

These figures were measured in WSL2 Ubuntu 24.04 on the development machine
(24 logical CPUs) with `make cost-table`, which follows ADR-003's
measurement protocol: for each board, the one-cell run (A) and `bb`'s
defaults (B) interleaved, N = 5 of each, medians, engine startup reported
separately from simulation, user+sys CPU and the load average recorded.
`make cost-table` measures every row twice (ADR-004): every Renode started
fresh, and on a one-slot warm Renode pool (`bb pool run`; all 40 pool runs
were leased). Every B run gave the same `nets.csv`, `nets.vcd` and
non-grant `events.jsonl` records as A. The simulation column is from the
fresh runs.

| Board, run | Simulated | Grants | Simulation (median) | Engine startup, fresh | Engine startup, pool | Total wall, fresh | Total wall, pool |
| --- | --- | --- | --- | --- | --- | --- | --- |
| spike, `--max-quantum 10us` (the Phase 0 setting: one-cell lockstep) | 500 ms | 50 000 | 23.52 s | 2.02 s | 0.15 s | 25.66 s | 24.89 s |
| spike, defaults (`make spike`: 10 ms grants) | 500 ms | 50 | **0.81 s** (29×) | 2.04 s | **0.18 s** | 3.02 s | 1.16 s |
| `examples/feedback --lookahead off` (a feedback loop: one-cell grants whatever `--max-quantum` is) | 500 ms | 50 000 | 24.74 s | 2.13 s | 0.30 s | 27.04 s | 24.66 s |
| `examples/feedback`, defaults (`make feedback`: CPU lookahead on) | 500 ms | 117 | **1.19 s** (21×) | 2.22 s | **0.39 s** | 3.49 s | 1.78 s |
| `examples/feedback-open` (the loop opened), `--max-quantum 10us` | 500 ms | 50 000 | 24.36 s | 2.25 s | 0.08 s | 26.86 s | 22.63 s |
| `examples/feedback-open`, defaults (10 ms grants) | 500 ms | 50 | **0.56 s** (44×) | 2.05 s | **0.10 s** | 2.66 s | 0.50 s |
| `examples/adc --lookahead off` (one-cell grants; Breadboard's ADC model) | 300 ms | 30 000 | 11.50 s | 1.46 s | 0.09 s | 13.00 s | 11.26 s |
| `examples/adc`, defaults (`make adc`: lookahead on, Breadboard's ADC model) | 300 ms | 1 209 | **1.11 s** (10.4×) | 1.43 s | **0.09 s** | 2.61 s | 0.97 s |
| `examples/adc --adc-model stock` (the ADR-003 default: Renode's ADC, lookahead on) | 300 ms | 6 344 | 5.08 s | 1.38 s | | 6.53 s | |
| `examples/nrf-feedback --lookahead off` (Phase 1 M2: the feedback circuit on Breadboard's nRF52840, one-cell grants) | 500 ms | 50 000 | 13.93 s | 1.41 s | 0.05 s | 15.36 s | 14.23 s |
| `examples/nrf-feedback`, defaults (`make nrf-feedback`: lookahead on) | 500 ms | 120 | **0.52 s** (27×) | 1.39 s | **0.05 s** | 1.95 s | 0.42 s |
| `examples/adc/kicad --lookahead off` (Phase 1 M4: the ADC board drawn in KiCad, run as a `bb.toml` project; one-cell grants) | 300 ms | 30 000 | 12.66 s | 1.49 s | | 14.17 s | |
| `examples/adc/kicad`, defaults (`make adc-kicad`: lookahead on) | 300 ms | 1 209 | **1.13 s** (11.2×) | 1.58 s | | 2.64 s | |
| `examples/dial-me-spinner` E1, `--max-quantum 10us` (Phase 1 M6: the imported main board, Zephyr on the nRF52840, cold bench, 132 instances in the deck; one-cell grants) | 120 ms | 12 000 | 6.83 s | 1.64 s | 0.06 s | 8.53 s | 6.64 s |
| `examples/dial-me-spinner` E1, defaults (`make dms-rail`, `make demo`: lookahead on, since M8's SAADC bindings make the board cyclic) | 120 ms | 71 | **1.87 s** (3.7×) | 1.62 s | **0.08 s** | 3.55 s | 1.71 s |
| `examples/nrf-saadc --lookahead off` (Phase 1 M8: the loading `adc_in`, an 8× SAADC burst every 5 ms; one-cell grants) | 30 ms | 3 000 | 1.55 s | 1.68 s | 0.07 s | 3.31 s | 1.27 s |
| `examples/nrf-saadc`, defaults (`make nrf-saadc-model`: lookahead on) | 30 ms | 209 | **0.30 s** (5.2×) | 1.59 s | **0.11 s** | 1.95 s | 0.33 s |
| `examples/dial-me-spinner` E2 s50, `--lookahead off --adc-push every` (Phase 1 M8: `vsense_50ms.elf`, an 8× burst on the loaded VSYS_SENSE every 50 ms, `bench/vsense.toml`; the every-lattice-point reference) | 650 ms | 65 000 | 39.51 s | 1.63 s | 0.07 s | 41.18 s | 39.88 s |
| `examples/dial-me-spinner` E2 s50, defaults (`make dms-vsense`: lookahead on) | 650 ms | 732 | **12.85 s** (3.1×) | 1.58 s | **0.07 s** | 14.48 s | 12.84 s |

The two `examples/nrf-feedback` rows (ADR-008 Amendment 2) were measured
later, with the same protocol (`make cost-table`'s nRF row, N = 5,
interleaved, medians): fresh at load 2.4–4.0 and on a one-slot pool at
load 1.6–2.0, every pooled run leased, every B run identical to A except
grant records. They are not comparable with the rows above, which ran at a
higher load.

The two `examples/dial-me-spinner` E1 rows (Phase 1 M6, ADR-007 Amendment 3)
were measured again at M8 (ADR-008 Amendment 5) with `make
cost-table-dms-rail` (N = 5, interleaved, medians). The project now binds
the SAADC on VSYS_SENSE for E2. That makes the board cyclic, so bb's
defaults run lookahead (71 grants). The runs used the correct-order
firmware at the cold bench, which is what `make demo` and the README
quickstart run. Fresh runs were at load 1.2–1.5, and runs on a one-slot
pool at load 0.9–1.3, with all 10 pooled runs leased. Every B run was
identical to A except for grant records. M7 measured these rows on an idle
host, before the bindings, and got 6.34 s (one cell) and 1.83 s (12
acyclic grants) of simulation; M6 got 9.21 s and 1.87 s at load 3.9–11.6.
The analog deck dominates on this board:
the same deck through `NgspiceEngine` alone (`deck_run`) takes about 1.9 s
for 120 ms. Long grants therefore gain less here than on the Phase 0
boards.

The two `examples/nrf-saadc` rows (Phase 1 M8, ADR-008 Amendment 4) were
measured with `make cost-table-nrf-saadc` (N = 5, interleaved, medians):
fresh at load 0.7–0.9, on a one-slot pool at load 0.9–1.1, all 10 pooled
runs leased, every B run identical to A except grant records. Each 8×
burst holds the grants around its eight acquisitions to about one cell
each (209 grants for 5 bursts in 30 ms, against 3 000 one-cell grants).

The two E2 rows (Phase 1 M8, ADR-008 Amendment 5) were measured with
`make cost-table-dms-vsense` (N = 5, interleaved, medians). Fresh runs were
at load 1.0–1.2, and runs on a one-slot pool at load 1.0–1.4, with all 10
pooled runs leased. Every B run was identical to A except for grant
records. The 13 bursts take 732 grants, about 56 each. The analog deck
dominates again: ngspice steps the whole E1 scope (132 instances) at
TMAX = 10 µs through 650 ms whatever the grants are, so lookahead gains
3.1× here, not the 5× of `examples/nrf-saadc`. The pool saves only the
1.6 s of startup.

The two `examples/adc/kicad` rows (ADR-007 Amendment 1) were measured
fresh only, with `make cost-table-adc-kicad` (N = 5, interleaved, medians,
load 1.6–2.2), in the same session as the hand-written board's row for
comparison: `examples/adc` took 11.79 s (one cell) and 1.09 s (defaults)
of simulation, 2.62 s in total, so the project route costs no measurable
simulation time; its outputs differ from the board file's in two 1 µV
samples (ADR-007 Amendment 1). Before the engines start, `bb run` on a
project re-imports the netlist and checks `board.bbm` and `bb.lock`: under
20 ms with the packages off `/mnt/c`, 0.2–1.2 s when the first-party
packages are read from the checkout on `/mnt/c` (WSL file I/O; not in the
table's columns).

Measured on 2026-11-09 (ADR-004 Amendment 7), load average 2.3–6.6
while another agent's job ran on the host, so the figures are noisy. The
three `examples/adc` rows were measured again later that day with the
same protocol (`scripts/perf_ab.sh`, N = 5, interleaved, medians;
ADR-005 Amendment 4): the first two as `make cost-table`'s ADC row, fresh
(load 3.1–7.5) and on a one-slot pool (load 1.8–2.7), and the stock row as
`make adc-speed`'s A case on an idle host (load 1.2), where the defaults
took 1.07 s (4.75× less than the stock model). They
are not comparable with the figures of earlier sessions (ADR-003 step 10,
ADR-004 Amendments 5 and 6), which ran with 8 KiB output buffers: with
`out/` on WSL's `/mnt/c`, writing `nets.csv` then cost up to about 2 s of
"simulation" on a loaded host. The pool session's simulation figures
differ from the fresh ones (for example feedback-open defaults 0.36 s,
adc defaults 6.60 s) because the host's speed drifted, so compare within
a row and between the two startup columns.

The fresh-startup column follows the host's load. On an idle host (load
1.1–1.2, ADR-004 Amendment 8; `make startup-perf`, `make adc-startup`,
N = 7 and 5, medians) fresh startup was 1.32 s for the spike, 1.39 s for
the feedback board and 1.46 s for the ADC board, about a third below the
spike and feedback figures in the table. The ADC board starts about
0.1–0.3 s slower than the spike (it loads Breadboard's plugin assembly and
the STM32F072 overlay); its lower figure in the table is an artifact of
the busier host the spike and feedback rows were measured on.

**Engine startup** is the time `bb run` waits before the first grant.
Almost all of it is Renode's own start: .NET, Renode init and the platform
load (ADR-004). A fresh run sets `DOTNET_TieredPGO=0` on Renode, plays
back a cached .NET multi-core JIT startup profile (`~/.cache/bb/renode-jit`;
the first run for a Renode install, platform and plugin setting gathers
it), polls its External Control server every 5 ms, and hands Renode a copy
of the firmware in its temp workdir, read once (Renode's `LoadELF` opens
the file about a dozen times, each costly on `/mnt/c`). Together these cut
fresh startup by 0.58–1.17 s against the stock runtime (`make
startup-perf`: 0.69–0.74 s on an idle host, 0.58–0.66 s at load 1.2–1.4,
0.77–1.17 s at load 2.9–3.6, where `/mnt/c` is slower and the firmware
copy saves more), and by 0.95–1.09 s against the commit before ADR-004
(ADR-004 Part A, Amendments 6, 7 and 8). On
a warm pool (`bb pool run -- <command>`), each run leases a pre-warmed
Renode process instead of starting one, and startup falls to 0.08–0.39 s.
`make test` and the gate targets run on a pool by default (`POOL=0` runs
them fresh).

On Breadboard's ADC model (ADR-005) the ADC board gains 10× over its
one-cell run: the model reads ADC_IN only at its sample instants, so the
ADC pin is a sampled input and a quiet grant runs from one TIM3 trigger to
the next whatever the RC node does (four grants per conversion: the quiet
grant, the cell holding the trigger and the end of conversion, and two
cells whose Queries the back-off skips). Renode's stock model kept ADR-003's
code-change watch, which stopped a quiet grant at every millivolt change
(6 344 grants). Per-run numbers are in `out/cost-table/{fresh,pool}/*/results.tsv`
after a run (`out/cost-adc` and `out/adc-speed` for the ADR-005
measurements).
`make perf` and `make lookahead-perf` are the speed gates of plan steps 3
and 8; `make cost-table` only reports.

The cost of a co-simulation is almost all fixed cost per grant (Renode's
pause/resume per `RUN_FOR`, 0.4–0.5 ms per grant in the one-cell rows), so the lever
is fewer grants. Since ADR-003 the
grant length is a pure cap and changes no output: Renode reports every edge
of a grant in a trace, its quantum (the lattice) stays 10 µs, and ngspice
lands on every lattice point. The spike gives byte-identical `nets.csv`,
`nets.vcd` and `pin_edge` records at 10 µs, 100 µs, 10 ms, 50 ms and 500 ms
grants (`make partition`). A board with a feedback loop (an analog net read
back by the MCU, `examples/feedback`) runs in one-cell grants unless the MCU
sleeps: CPU lookahead (ADR-003 Part B, on by default) lifts the cap while it
does, with byte-identical outputs.

**Edge times depend on the Renode quantum, not on the grant partition**
(ADR-003 D2). Renode's quantum (`--renode-quantum`, default 10 µs) is a grid
that every grant boundary must sit on; it is run configuration, like the seed.
Renode stamps a pin change with the sync point that opens the slice it ran
in; a stamp on the grid is reported 1 fs later, a timer-driven stamp exactly.
With every `RUN_FOR` ending on the grid, 10 µs, 70 µs and 100 µs, and 1, 10,
50 and 500 ms grants give the identical edge list
(`engines/cpu-renode/tests/blinky.rs`), and the spike's outputs do not depend
on `--max-quantum` (a multiple of the quantum; `make lattice-check`,
`make partition`). A grant may hold any number of edges per pin. MCU inputs
take effect at the next grid point, never earlier.

## Layout

See `AGENTS.md` for the repo map, the invariants (determinism, the
pin-boundary contract, no engine free-runs) and the rules for changing
`spec/`. The decisions are recorded in `docs/adr/`.
