# Determinism

The same inputs and seed must produce **byte-identical** outputs:
`events.jsonl`, `nets.csv` and `nets.vcd`. This is the first of the three
invariants in `AGENTS.md`, and the test gates enforce it against committed
hashes rather than by inspection.

## How it is kept

- **Integer time.** Every scheduling decision uses integer femtosecond `Time`.
  Floats appear only at an engine's FFI boundary.
- **Integer values in the outputs.** `events.jsonl` carries times in
  femtoseconds and voltages in integer microvolts, so its text never depends
  on float formatting. `nets.csv` prints volts with exactly 6 decimals through
  integer µV.
- **Ordered collections.** No `HashMap` iteration into any output.
- **No wall-clock reads, no unseeded randomness**, and no results that depend
  on thread timing. `bb` prints the wall-clock time to the terminal, never
  into the outputs. No output file holds a date, a timestamp, a PID or an
  absolute path.
- **Fixed record order.** The records of one exchange are sorted by time,
  then by the producing engine's registration index, then by pin, never by the
  order in which engines advanced.
- **Samples do not move boundaries.** Samples are taken at fixed instants
  and never add grant boundaries (that would change Renode's results).
- **Canonical files.** `board.bbm` and `run.json` are canonical JSON
  (integer-only, `spec/canonical-json.md`) with no paths, hashes of sources,
  tool strings or dates.

## The partition is not part of the result

How time is cut into grants changes only the `grant` records.
`--max-quantum`, `--lookahead`, the engine registration order
(`--engine-order`), the Renode pool and `--verify` must all leave `nets.csv`,
`nets.vcd` and the non-grant records byte-identical. To compare two runs,
strip the grant records first:

```sh
grep -v '"type":"grant"' out/a/events.jsonl > a.records
grep -v '"type":"grant"' out/b/events.jsonl > b.records
cmp a.records b.records && cmp out/a/nets.csv out/b/nets.csv && cmp out/a/nets.vcd out/b/nets.vcd
```

Calling two runs equivalent from `events.jsonl` alone, or from `nets.csv`
alone, is one of the "known bad ideas" in `AGENTS.md`.

What **does** change results, and so is run configuration: the Renode
quantum (`--renode-quantum`), the seed (`--seed`), the sample rate, the
ADC model (`--adc-model`) and, for a project, the bench and its option set.

## The gates

| Gate | What it pins |
| --- | --- |
| `make determinism`, `feedback-determinism`, `adc-determinism` | 10 runs, identical sha256 of every output. |
| `make golden` | The spike's outputs against `tests/golden/spike/outputs.sha256`. |
| `make partition`, `feedback-open` | Every `--max-quantum` gives the committed one-cell reference hashes. |
| `make feedback-order`, `adc-order` | The engines registered in the other order, byte-identical. |
| `make pool-equiv` | Leased and fresh Renode give the same bytes. |
| `make dms-rail` | E1: one-cell references, both engine orders and 10-run determinism. |

A new speed-up or a new board gets an equivalence gate against its one-cell
reference before it is on by default.
