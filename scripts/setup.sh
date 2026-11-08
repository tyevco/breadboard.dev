#!/usr/bin/env bash
# Breadboard toolchain bootstrap (Phase 0). Idempotent: safe to re-run.
# Target host: Ubuntu 24.04 (WSL or native), x86_64.
set -euo pipefail

# ---- Pinned versions -------------------------------------------------------
RUST_VERSION="1.99.1"
UV_VERSION="0.13.7"
RENODE_VERSION="1.17.0"
RENODE_TARBALL="renode-${RENODE_VERSION}.linux-portable.tar.gz"
RENODE_URL="https://github.com/renode/renode/releases/download/v${RENODE_VERSION}/${RENODE_TARBALL}"
RENODE_SHA256="4ba7c68b59e2447f188ef4b4b112fcccd2802582c460beedceb63b84e5605a5f"

APT_PACKAGES=(
  build-essential make cmake pkg-config curl ca-certificates git python3
  gcc-arm-none-eabi libnewlib-arm-none-eabi binutils-arm-none-eabi
  ngspice libngspice0 libngspice0-dev
  # Renode portable (self-contained .NET) runtime dep; we run it headless, so no GTK.
  libicu74
)

PREFIX_OPT="${HOME}/.local/opt"
PREFIX_BIN="${HOME}/.local/bin"
mkdir -p "${PREFIX_OPT}" "${PREFIX_BIN}"

log() { printf '\n==> %s\n' "$*"; }

# ---- apt -------------------------------------------------------------------
missing=()
for p in "${APT_PACKAGES[@]}"; do
  dpkg-query -W -f='${Status}' "$p" 2>/dev/null | grep -q "install ok installed" || missing+=("$p")
done
if ((${#missing[@]})); then
  log "apt: installing ${missing[*]}"
  sudo apt-get update
  sudo DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends "${missing[@]}"
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
rustup default "${RUST_VERSION}"

# ---- uv --------------------------------------------------------------------
if ! command -v uv >/dev/null 2>&1 || [ "$(uv --version | awk '{print $2}')" != "${UV_VERSION}" ]; then
  log "uv: installing ${UV_VERSION}"
  curl -LsSf "https://astral.sh/uv/${UV_VERSION}/install.sh" | env UV_NO_MODIFY_PATH=1 UV_INSTALL_DIR="${PREFIX_BIN}" sh
else
  log "uv: ${UV_VERSION} present"
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

case ":${PATH}:" in
  *":${PREFIX_BIN}:"*) ;;
  *) log "NOTE: add ${PREFIX_BIN} to PATH (Ubuntu's ~/.profile does this for login shells once the dir exists)";;
esac

log "setup complete; run 'make doctor'"
