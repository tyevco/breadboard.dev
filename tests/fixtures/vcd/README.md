# tests/fixtures/vcd: the viewer check's committed input (ADR-016 D1, G1)

`mixed.vcd` and `mixed.csv` are the `nets.vcd` and `nets.csv` of a real `bb
run`: the dial-me-spinner E1 project with `led_rail.elf` at the cold bench,
32 ms at a 5 kHz sample rate. They hold five `real` nets, three 1-bit wires
(`U10.P1.04` rises at 130 µs, `U10.P0.06` at 30.19 ms), the 1 fs timescale
and the `$comment` name map (`LED_ring_LED_DIN = /LED ring/LED_DIN`).
`make vcd-check` loads them with `tools/vcd-check`, so the check has an input
when the demo changes. Its negative controls edit copies of them.

They were made at Phase 2 E4 with (the project copy of `dms_rail.sh prepare`):

```sh
bb run --project <proj-ok> --bench <proj-ok>/bench/cold.toml --for 32ms \
    --sample-rate 5kHz --out <out>
cp <out>/nets.vcd tests/fixtures/vcd/mixed.vcd
cp <out>/nets.csv tests/fixtures/vcd/mixed.csv
```

Two runs gave identical bytes. A later change to bb's outputs does not need
them regenerated: they are a fixed input to the viewers, not a golden.
