# Quickstart

This runs the reference demo: the LED rail power sequencing (E1) of
dial-me-spinner rev A, with the correct firmware and then with firmware that
switches off in the wrong order.

## What you need

- A clean **Ubuntu 24.04**: a container, a WSL distro or a machine.
- Network access for the toolchain downloads (about 800 MB: Rust, Renode).

You do **not** need KiCad, the Zephyr SDK, an ARM toolchain or the
dial-me-spinner repository: the netlist and the prebuilt firmware are in the
repository.

## Run it

As root (for example in a container), run:

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

This is the README's quickstart block, copied verbatim. The README's copy is
the one the timed stranger test runs (`make stranger-test-container`,
`make stranger-test-wsl`).

::: tip Running from a local copy
To try a local copy instead, clone from its path or `file://` URL, for
example `git clone /path/to/breadboard.dev breadboard.dev`.
:::

## How long it takes

`make setup SETUP=demo` takes most of the time. It installs the apt packages,
Rust 1.99.1, Renode 1.17.0 and ngspice 42, then builds `bb`. Each run then
takes a few seconds.

The README records these timings, measured from a local clone on a 24-CPU
host with a working Ubuntu mirror (Phase 1 plan, "Review notes: M7"):

| Environment | Whole block |
| --- | --- |
| clean `ubuntu:24.04` container | 1 min 54 s |
| fresh WSL distro | 1 min 47 s |

Of that, `make setup` took about 50 s, the local clone 25–45 s, and each run
under 4 s. Almost all of the time is apt and the downloads; Breadboard's own
steps take seconds.

## If apt is slow

The review of M7 measured 20 min for this block with `ubuntu:24.04`'s default
sources while `archive.ubuntu.com` ran at 63 kB/s and `security.ubuntu.com`
timed out, and 71 s with another mirror. If `apt-get` is very slow or times
out, point apt at another Ubuntu mirror and run the line again, for example
as root:

```sh
sed -i 's#http://\(archive\|security\).ubuntu.com/ubuntu#http://mirrors.edge.kernel.org/ubuntu#' /etc/apt/sources.list.d/ubuntu.sources
```

If `apt-get` fails with "Mirror sync in progress?", run the line again;
`make setup` retries on its own.

## What the steps do

- **`bb open`** imports the netlist and writes `board.bbm`. The netlist is
  dial-me-spinner `6854d25`, KiCad 10.0.8, with its sha256 in
  `examples/dial-me-spinner/SOURCE`. It resolves each part to a package by its
  MPN or as a generic R, C or L. The committed `board.bbm` is identical, so it
  reports `unchanged`.
- **`bb flash`** records the image and its hash in `bb.toml`.
- **`bb run`** writes to `out/rail/`:
  - `nets.csv` and `nets.vcd`: VLED, LED_DIN and the nets the project probes;
  - `events.jsonl`: every pin edge;
  - `deck.cir`: the elaborated SPICE deck;
  - `run.json`: every analog view in the run, with its provenance, confidence
    and qualification, plus the solver option set.

  `bb run` also notes the views whose provenance is `agent_generated` (the
  ideal R and C, and the TPS22917's datasheet-fitted view).
- The two runs' `nets.csv`, `nets.vcd` and non-grant records are
  byte-identical to the committed hashes in `tests/golden/dms-rail/`.

## One command: `make demo`

From the repository root, `make demo` does all of the above in one command
and also checks those hashes. It works on a copy of the project (in
`~/.cache/bb-demo`), so the checkout is never written. It needs only
`make setup SETUP=demo`.

Next: [what the demo checks](./demo).
