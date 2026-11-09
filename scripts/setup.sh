#!/usr/bin/env bash
# Breadboard toolchain bootstrap. Idempotent: safe to re-run.
# Target host: Ubuntu 24.04 (WSL, native or a container), x86_64.
#
#   bash scripts/setup.sh            # full (developer): everything `make test` needs
#   bash scripts/setup.sh demo       # demo (Phase 1 plan M3, R7): no ARM toolchain, no uv
#   SETUP=demo bash scripts/setup.sh # the same; `make setup SETUP=demo` passes it
#
# Both modes install `bb` into ~/.local/bin (a release build of this
# checkout). Run as root (a container), it uses apt-get directly; otherwise
# it uses sudo for apt only.
set -euo pipefail

MODE="${1:-${SETUP:-full}}"
case "${MODE}" in
  full|demo) ;;
  *) echo "setup.sh: unknown mode ${MODE:?} (expected full or demo)" >&2; exit 2 ;;
esac

# ---- Pinned versions -------------------------------------------------------
RUST_VERSION="1.99.1"
UV_VERSION="0.13.7"
RENODE_VERSION="1.17.0"
RENODE_TARBALL="renode-${RENODE_VERSION}.linux-portable.tar.gz"
RENODE_URL="https://github.com/renode/renode/releases/download/v${RENODE_VERSION}/${RENODE_TARBALL}"
RENODE_SHA256="4ba7c68b59e2447f188ef4b4b112fcccd2802582c460beedceb63b84e5605a5f"

# What `bb run` and the demo need: a C toolchain for the Rust build (cc, the
# linker), Python for the check scripts, ngspice 42 and its shared library,
# and the .NET runtime dependency of Renode portable (run headless, so no GTK).
APT_PACKAGES=(
  build-essential make pkg-config curl ca-certificates git python3
  ngspice libngspice0 libngspice0-dev
  libicu74
)
# Developer additions: the ARM toolchain for the firmware and the gate
# firmware, cmake, and python3-jsonschema for the package schema validation
# (`make packages`, ADR-006 G1).
if [ "${MODE}" = full ]; then
  APT_PACKAGES+=(cmake gcc-arm-none-eabi libnewlib-arm-none-eabi binutils-arm-none-eabi python3-jsonschema)
fi

PREFIX_OPT="${HOME}/.local/opt"
PREFIX_BIN="${HOME}/.local/bin"
mkdir -p "${PREFIX_OPT}" "${PREFIX_BIN}"

log() { printf '\n==> %s\n' "$*"; }
log "setup mode: ${MODE}"

# Root (e.g. a clean ubuntu:24.04 container, which has no sudo) runs apt
# directly.
if [ "$(id -u)" -eq 0 ]; then
  SUDO=()
else
  SUDO=(sudo)
fi

# ---- apt -------------------------------------------------------------------
missing=()
for p in "${APT_PACKAGES[@]}"; do
  dpkg-query -W -f='${Status}' "$p" 2>/dev/null | grep -q "install ok installed" || missing+=("$p")
done
if ((${#missing[@]})); then
  log "apt: installing ${missing[*]}"
  "${SUDO[@]}" apt-get update
  "${SUDO[@]}" env DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends "${missing[@]}"
else
  log "apt: all packages present"
fi

# ---- rustup ----------------------------------------------------------------
if [ -f "${HOME}/.cargo/env" ]; then . "${HOME}/.cargo/env"; fi
if ! command -v rustup >/dev/null 2>&1; then
  log "rustup: installing (toolchain ${RUST_VERSION}, minimal profile)"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --profile minimal --default-toolchain "${RUST_VERSION}" --component rustfmt,clippy
  . "${HOME}/.cargo/env"
else
  log "rustup: present; ensuring toolchain ${RUST_VERSION}"
fi
rustup toolchain install "${RUST_VERSION}" --profile minimal --component rustfmt --component clippy
# The repo pins its toolchain through rust-toolchain.toml; do not touch the
# user's global default unless rustup has none at all.
if ! rustup default >/dev/null 2>&1; then
  rustup default "${RUST_VERSION}"
fi

# ---- uv (developer only: the dial-me-spinner suite tooling) ----------------
if [ "${MODE}" = full ]; then
  if ! command -v uv >/dev/null 2>&1 || [ "$(uv --version | awk '{print $2}')" != "${UV_VERSION}" ]; then
    log "uv: installing ${UV_VERSION}"
    curl -LsSf "https://astral.sh/uv/${UV_VERSION}/install.sh" | env UV_NO_MODIFY_PATH=1 UV_INSTALL_DIR="${PREFIX_BIN}" sh
  else
    log "uv: ${UV_VERSION} present"
  fi
fi

# ---- Renode (portable) -----------------------------------------------------
RENODE_DIR="${PREFIX_OPT}/renode-${RENODE_VERSION}"
if [ ! -x "${RENODE_DIR}/renode" ]; then
  log "renode: installing ${RENODE_VERSION} from ${RENODE_URL}"
  tmp="$(mktemp -d)"
  trap 'rm -rf "${tmp}"' EXIT
  curl -fL --retry 3 -o "${tmp}/${RENODE_TARBALL}" "${RENODE_URL}"
  echo "${RENODE_SHA256}  ${tmp}/${RENODE_TARBALL}" | sha256sum -c -
  mkdir -p "${tmp}/x"
  tar -xzf "${tmp}/${RENODE_TARBALL}" -C "${tmp}/x"
  src="$(find "${tmp}/x" -mindepth 1 -maxdepth 1 -type d | head -n1)"
  rm -rf "${RENODE_DIR}"
  mv "${src}" "${RENODE_DIR}"
else
  log "renode: ${RENODE_VERSION} present at ${RENODE_DIR}"
fi
ln -sfn "${RENODE_DIR}/renode" "${PREFIX_BIN}/renode"
if [ -x "${RENODE_DIR}/renode-test" ]; then ln -sfn "${RENODE_DIR}/renode-test" "${PREFIX_BIN}/renode-test"; fi

# ---- bb ----------------------------------------------------------------------
# A release build of this checkout, copied (not linked) into ~/.local/bin so
# that a later `cargo build` of another checkout cannot change it. The
# first-party package directory is found through the path compiled into it
# (this checkout's packages/), so keep the checkout where it is.
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${HOME}/.cache/bb-target}"
log "bb: cargo build --release -p bb-cli (CARGO_TARGET_DIR=${CARGO_TARGET_DIR})"
(cd "${REPO}" && cargo build --release --locked -p bb-cli)
install -m 0755 "${CARGO_TARGET_DIR}/release/bb" "${PREFIX_BIN}/bb"
log "bb: installed ${PREFIX_BIN}/bb"

case ":${PATH}:" in
  *":${PREFIX_BIN}:"*) ;;
  *) log "NOTE: add ${PREFIX_BIN} to PATH (Ubuntu's ~/.profile does this for login shells once the dir exists): export PATH=\"${PREFIX_BIN}:\$PATH\"";;
esac

log "setup (${MODE}) complete; run 'make doctor$([ "${MODE}" = demo ] && echo ' SETUP=demo')'"
