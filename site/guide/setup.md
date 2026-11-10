# Installing and setup modes

Run every command inside **WSL (Ubuntu 24.04) or Linux**, from the repository
root. `make setup` runs `scripts/setup.sh`, which installs or verifies a
pinned toolchain (it is idempotent), builds `bb` and installs it into
`~/.local/bin`.

## Setup modes

`make setup` takes a mode in `SETUP` (`scripts/setup.sh` accepts `full`,
`demo` and `zephyr`):

| Command | Installs |
| --- | --- |
| `make setup` (`SETUP=full`, the default) | The developer toolchain: Rust 1.99.1, uv, Renode 1.17.0 portable, arm-none-eabi-gcc, ngspice 42 and libngspice0-dev, python3-jsonschema; then builds `bb`. |
| `make setup SETUP=demo` | Only what running boards needs: Rust, Renode, ngspice and `bb`. No ARM toolchain, no uv, no jsonschema. |
| `make setup SETUP=zephyr` | Optional. Adds the Zephyr host tools and the pinned Zephyr SDK 1.0.1, for `make fw-rebuild`. |

As root (for example in a container) setup runs apt without sudo. A clean
Ubuntu needs `apt-get install make` first.

`make doctor` prints every tool's version and exits non-zero if anything is
missing.

## From Windows

Use WSL. From PowerShell:

```powershell
wsl -d Ubuntu-24.04 -- bash -lc 'cd /mnt/c/source/tyevco/breadboard.dev && make setup && make test'
```

Use single quotes so that PowerShell does not expand `$`. `AGENTS.md` has more
WSL notes, for example:

- Building on `/mnt/c` is slow, so the Makefile defaults
  `CARGO_TARGET_DIR=$HOME/.cache/bb-target`.
- A second checkout or `git worktree` must use its own `CARGO_TARGET_DIR`:
  two checkouts sharing one target directory overwrite each other's crates.
- Renode is installed at `~/.local/opt/renode-1.17.0` (symlink
  `~/.local/bin/renode`).

## Caches

- Breadboard's Renode plugins are compiled once into one assembly cached in
  `~/.cache/bb/renode-plugins`. Deleting it costs one compile (about 2 s).
- A fresh Renode plays back a .NET JIT startup profile cached in
  `~/.cache/bb/renode-jit`. The first run for a given Renode install, platform
  and plugin setting gathers it. `BB_RENODE_JIT_PROFILE=off` disables it.
- `$BB_CACHE_DIR` or `$XDG_CACHE_HOME/bb` override the cache location.

## Running a board by hand

The Phase 0 boards are plain board files (a SPICE body plus `*bb`
directives). For example, the spike:

```sh
make firmware bb
~/.cache/bb-target/release/bb run --netlist examples/spike/board.cir \
    --firmware examples/spike/firmware/build/blinky.elf --target U1 --for 500ms \
    --probe net:LED_A --probe net:PA5 --out out/spike
```

Optional flags include `--max-quantum 10ms`, `--renode-quantum 10us`,
`--sample-rate 100kHz` and `--verify edges` (these values are the defaults).
See the [CLI reference](../reference/cli).

`bb` prints the wall-clock time to the terminal, never into the outputs.
`out/spike/nets.vcd` opens in GTKWave.
