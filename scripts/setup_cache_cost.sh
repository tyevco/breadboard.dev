#!/usr/bin/env bash
# Phase 2 H6 (ADR-015 D11, Amendment 5): the local equivalent of what caching `make setup` saves a
# CI job. CI cannot run from here (no push), so each part of setup.sh that the bb-setup action now
# restores from its cache is timed both ways on this host, N times interleaved, medians:
#
#   rust    H0: `rustup toolchain install <RUST_VERSION> --profile minimal --component rustfmt
#               --component clippy` into an empty RUSTUP_HOME (the download and unpack every CI
#               job did); H6: restoring that toolchain directory and its update hash from a
#               zstd tar (what actions/cache does on a hit: `tar -I 'zstd -d -T0'`), after which
#               the same rustup command is a no-op (timed with it);
#   renode  H0: unpacking the cached Renode tarball (BB_DOWNLOAD_CACHE, H0's cache); H6:
#               restoring the unpacked tree from a zstd tar;
#   uv      H0: uv's installer for UV_VERSION; H6: restoring uv and uvx from a zstd tar.
#
# apt is not part of it: its packages still install each run (from H0's cached .debs). The
# network legs (rustup's and uv's downloads) are this host's; CI's differ, so the first push must
# confirm the setup step's time (AGENTS.md "Gate budgets"). Writes <out>/results.tsv and prints the
# medians. Under the measurement lock (make setup-cache-cost).
#
#   scripts/setup_cache_cost.sh <out-dir> <n>
set -euo pipefail
out=$1 n=${2:-3}
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
pin() { sed -n "s/^$1=\"\(.*\)\"$/\1/p" "$root/scripts/setup.sh"; }
RUST=$(pin RUST_VERSION) UV=$(pin UV_VERSION) RENODE=$(pin RENODE_VERSION) RSHA=$(pin RENODE_SHA256)
tarball=${BB_DOWNLOAD_CACHE:-$HOME/.cache/bb-ci-downloads}/renode-${RENODE}.linux-portable.tar.gz
command -v zstd >/dev/null || { echo "setup-cache-cost: zstd is not installed" >&2; exit 1; }
command -v rustup >/dev/null || { echo "setup-cache-cost: rustup is not installed" >&2; exit 1; }
mkdir -p "$out" "$(dirname "$tarball")"
out=$(cd "$out" && pwd)
if ! echo "$RSHA  $tarball" | sha256sum -c --status - 2>/dev/null; then
  curl -fL --retry 3 -o "$tarball.part" \
    "https://github.com/renode/renode/releases/download/v${RENODE}/renode-${RENODE}.linux-portable.tar.gz"
  mv "$tarball.part" "$tarball"
  echo "$RSHA  $tarball" | sha256sum -c -
fi
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
tc="${RUST}-x86_64-unknown-linux-gnu"
now() { date +%s.%N; }
since() { awk -v a="$1" -v b="$(now)" 'BEGIN { printf "%.2f", b - a }'; }
rustup_install() {
  RUSTUP_HOME="$1" rustup toolchain install "$RUST" --profile minimal \
    --component rustfmt --component clippy >/dev/null 2>&1
}

# The cache archives, made once (a CI cache miss pays this once per key, in its post step).
rustup_install "$work/seed-rustup"
tar -C "$work/seed-rustup" -I 'zstd -T0' -cf "$work/rust.tar.zst" "toolchains/$tc" "update-hashes/$tc"
mkdir -p "$work/seed-renode"
tar -xzf "$tarball" -C "$work/seed-renode"
tar -C "$work/seed-renode" -I 'zstd -T0' -cf "$work/renode.tar.zst" .
mkdir -p "$work/seed-uv"
curl -LsSf "https://astral.sh/uv/${UV}/install.sh" | env UV_NO_MODIFY_PATH=1 UV_INSTALL_DIR="$work/seed-uv" sh >/dev/null 2>&1
tar -C "$work/seed-uv" -I 'zstd -T0' -cf "$work/uv.tar.zst" uv uvx

printf 'round\tpart\tcase\tseconds\n' > "$out/results.tsv"
for i in $(seq 1 "$n"); do
  # rust
  d="$work/r$i-h0"; mkdir -p "$d"; t=$(now); rustup_install "$d"; printf '%s\trust\th0\t%s\n' "$i" "$(since "$t")" >> "$out/results.tsv"
  d="$work/r$i-h6"; mkdir -p "$d"; t=$(now)
  tar -C "$d" -I 'zstd -d -T0' -xf "$work/rust.tar.zst" && rustup_install "$d"
  printf '%s\trust\th6\t%s\n' "$i" "$(since "$t")" >> "$out/results.tsv"
  rm -rf "$work/r$i-h0" "$work/r$i-h6"
  # renode
  d="$work/n$i-h0"; mkdir -p "$d"; t=$(now); tar -xzf "$tarball" -C "$d"; printf '%s\trenode\th0\t%s\n' "$i" "$(since "$t")" >> "$out/results.tsv"
  d="$work/n$i-h6"; mkdir -p "$d"; t=$(now); tar -C "$d" -I 'zstd -d -T0' -xf "$work/renode.tar.zst"; printf '%s\trenode\th6\t%s\n' "$i" "$(since "$t")" >> "$out/results.tsv"
  rm -rf "$work/n$i-h0" "$work/n$i-h6"
  # uv
  d="$work/u$i-h0"; mkdir -p "$d"; t=$(now)
  curl -LsSf "https://astral.sh/uv/${UV}/install.sh" | env UV_NO_MODIFY_PATH=1 UV_INSTALL_DIR="$d" sh >/dev/null 2>&1
  printf '%s\tuv\th0\t%s\n' "$i" "$(since "$t")" >> "$out/results.tsv"
  d="$work/u$i-h6"; mkdir -p "$d"; t=$(now); tar -C "$d" -I 'zstd -d -T0' -xf "$work/uv.tar.zst"; printf '%s\tuv\th6\t%s\n' "$i" "$(since "$t")" >> "$out/results.tsv"
  rm -rf "$work/u$i-h0" "$work/u$i-h6"
done
ls -l "$work"/*.tar.zst | awk '{ printf "archive %s %.1f MB\n", $NF, $5 / 1e6 }' | sed "s|$work/||" > "$out/archives.txt"
python3 - "$out/results.tsv" <<'EOF' | tee "$out/summary.txt"
import csv, statistics, sys
rows = list(csv.DictReader(open(sys.argv[1]), delimiter="\t"))
tot = {"h0": 0.0, "h6": 0.0}
for part in ("rust", "renode", "uv"):
    m = {c: statistics.median(float(r["seconds"]) for r in rows if r["part"] == part and r["case"] == c) for c in ("h0", "h6")}
    tot["h0"] += m["h0"]; tot["h6"] += m["h6"]
    print(f"{part}: H0 {m['h0']:.2f} s, H6 {m['h6']:.2f} s, saving {m['h0'] - m['h6']:.2f} s")
print(f"total: H0 {tot['h0']:.2f} s, H6 {tot['h6']:.2f} s, saving {tot['h0'] - tot['h6']:.2f} s (medians, N = {len(rows) // 6})")
EOF
cat "$out/archives.txt"
