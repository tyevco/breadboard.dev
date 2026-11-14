# Project layout

The repository is a Rust workspace plus examples, packages, scripts and the
normative specs. This map follows `AGENTS.md`, which is the authority.

| Path | What it holds |
| --- | --- |
| `bbcore/` | The kernel: `Time`, `PinSignal`, the `Engine` trait, the `Scheduler`, `EventSink`, `CsvWriter`, `VcdWriter`; `board.rs` (the `board.bbm` types) and `cjson.rs` (integer-only canonical JSON). |
| `engines/cpu-renode/` | The Renode engine over the External Control API. `platforms/` holds the plugin assembly's sources (the lookahead block, the STM32 timer with TRGO, Breadboard's STM32F0 ADC model), the STM32F072 overlay and Breadboard's nRF52840 platform (`nrf52840-bb.repl`). Also the warm Renode pool and the gate firmware. |
| `engines/analog-ngspice/` | The ngspice shared-library engine (`libngspice.so.0`). |
| `importers/kicad/` | `bb-kicad`: an S-expression reader, the netlist model and its version gates, and the `kicad-cli` runner (always on a temporary copy, never in the project). |
| `bbpkg/` | Component packages: the value grammar, `part.toml`/`pins.toml`, lint, view files and deck assembly, test vectors through `NgspiceEngine`, tree hashes and `bb.lock`. |
| `cli/` | The `bb` binary: `bb run`, `bb pool`, `bb open`, `bb flash`, `bb probe`, `bb pkg`; the board-file parser, projects, name resolution, import and elaboration. |
| `packages/` | First-party packages: `generic/{resistor,capacitor,inductor}`, `st/stm32f103c8`, `st/stm32f072rb`, `ti/tps22917` (datasheet-fitted view), `worldsemi/ws2812c-2020`, `nordic/nrf52840-qiaa`, `raytac/mdbt50q-u1mv2` (a module). |
| `local-packages/` | Git-ignored, local-only packages (the vendor-derived TPS22917). Never committed. |
| `examples/spike/` | Phase 0: STM32F103 blinky and `board.cir` (PA5 → 330 Ω → red LED). |
| `examples/feedback/`, `examples/feedback-open/` | A board on which the MCU reads back a node it drives, and the same with the loop opened. |
| `examples/adc/` | The STM32F072 ADC board, its firmware and the ADC model tests; `examples/adc/kicad/` is the same board drawn in KiCad, as a `bb.toml` project. |
| `examples/nrf-feedback/` | The feedback circuit on the nRF52840. |
| `examples/nrf-saadc/` | Breadboard's nRF52840 SAADC on a loading `adc_in` board, and the SAADC model's Renode-only tests. |
| `examples/dial-me-spinner/` | The Phase 1 reference board: vendored KiCad inputs, `SOURCE` (commits and sha256s), the suite re-run, `bb.toml`, `board.bbm`, `bb.lock`, the benches and the E1 and E2 firmware (sources and prebuilt ELFs). |
| `tests/golden/` | Waveform snapshots and output hashes for every gated board. |
| `spec/` | Normative protocol and schema documents, and `spec/schema/*.schema.json`. Changing them needs an ADR. |
| `docs/adr/` | Architecture decision records. |
| `docs/phase1-plan.md` | The Phase 1 plan, with its approver decisions and review notes. |
| `scripts/` | Setup, the check scripts (`check_spike.py`, `check_feedback.py`, `check_adc.py`, `check_dms_rail.py`, ...) and the gate drivers. |
| `site/` | This documentation site (VitePress), published to GitHub Pages by `.github/workflows/docs.yml`. `site/public/plots/` holds the waveform figures that `make docs-plots` draws from real runs. |
| `.github/workflows/` | `ci.yml` (CI, on every push and pull request) and `docs.yml` (the site, on pushes to `main`). |

## The rules that shape the code

`AGENTS.md` states three invariants. In short:

1. **Determinism.** The same inputs and seed produce byte-identical outputs.
   See [Determinism](../concepts/determinism).
2. **Pin-boundary contract.** Engines interact only through pins bound to
   nets. Each pin has exactly one pin model and each net exactly one driver.
   See [Pin models](../reference/pin-models).
3. **No engine free-runs.** Engines advance only on kernel grants and never
   pass the latest grant. See
   [Co-simulation and time grants](../concepts/cosim).

And a process rule: **no change to `spec/` without an ADR** in `docs/adr/` in
the same change. The [design records](../records/) section renders those
documents.
