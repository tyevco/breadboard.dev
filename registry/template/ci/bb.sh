#!/usr/bin/env bash
# Build the bb that the registry's CI runs: breadboard.dev at the commit in BB_REV, with the
# toolchain its `make setup SETUP=demo` installs (Rust, ngspice for `bb pkg test`). Prints the
# path of the binary.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
rev=$(tr -d ' \n' <"$here/BB_REV")
src=${BB_SRC:-$RUNNER_TEMP/breadboard.dev}
if [ ! -d "$src/.git" ]; then
    git clone --quiet https://github.com/tyevco/breadboard.dev "$src"
fi
git -C "$src" fetch --quiet origin "$rev"
git -C "$src" checkout --quiet "$rev"
sudo apt-get install -y -qq libngspice0-dev ngspice minisign >/dev/null
make -C "$src" setup SETUP=demo >&2
echo "$HOME/.local/bin/bb"
