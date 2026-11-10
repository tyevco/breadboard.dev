#!/usr/bin/env bash
# Breadboard toolchain bootstrap. Idempotent: safe to re-run.
# Target host: Ubuntu 24.04 (WSL, native or a container), x86_64.
#
#   bash scripts/setup.sh            # full (developer): everything `make test` needs
#   bash scripts/setup.sh demo       # demo (Phase 1 plan M3, R7): no ARM toolchain, no uv
#   SETUP=demo bash scripts/setup.sh # the same; `make setup SETUP=demo` passes it
#   bash scripts/setup.sh zephyr     # optional (Phase 1 M6, ADR-008 D5): full, plus the Zephyr
#                                    # host tools and the pinned Zephyr SDK, to rebuild the E1
#                                    # firmware (make fw-rebuild, make dms-firmware). Neither
#                                    # make test nor the demo needs it: the images are prebuilt.
#
# Both modes install `bb` into ~/.local/bin (a release build of this
# checkout). Run as root (a container), it uses apt-get directly; otherwise
# it uses sudo for apt only.
set -euo pipefail

MODE="${1:-${SETUP:-full}}"
case "${MODE}" in
  full|demo|zephyr) ;;
  *) echo "setup.sh: unknown mode ${MODE:?} (expected full, demo or zephyr)" >&2; exit 2 ;;
esac

# ---- Pinned versions -------------------------------------------------------
RUST_VERSION="1.99.1"
UV_VERSION="0.13.7"
RENODE_VERSION="1.17.0"
RENODE_TARBALL="renode-${RENODE_VERSION}.linux-portable.tar.gz"
RENODE_URL="https://github.com/renode/renode/releases/download/v${RENODE_VERSION}/${RENODE_TARBALL}"
RENODE_SHA256="4ba7c68b59e2447f188ef4b4b112fcccd2802582c460beedceb63b84e5605a5f"
# Optional (SETUP=zephyr; Phase 1 M6, ADR-008 D5): the E1 firmware's toolchain. The versions and
# the SDK tarballs' sha256s are pinned in scripts/dms_zephyr.sh (Zephyr v4.5.0 = 41d10b59, SDK
# 1.0.1 minimal + arm-zephyr-eabi, west 1.5.0, Python packages in
# examples/dial-me-spinner/firmware/requirements.lock); the workspace manifest is
# examples/dial-me-spinner/firmware/west.yml.
ZEPHYR_SDK_VERSION="1.0.1"

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
if [ "${MODE}" != demo ]; then
  APT_PACKAGES+=(cmake gcc-arm-none-eabi libnewlib-arm-none-eabi binutils-arm-none-eabi python3-jsonschema)
fi
# The Zephyr host tools a west build of the E1 firmware uses (ninja, gperf, a venv for west).
if [ "${MODE}" = zephyr ]; then
  APT_PACKAGES+=(ninja-build gperf python3-venv xz-utils file)
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
# An Ubuntu mirror in the middle of a sync serves index files that do not match
# their Release file ("File has unexpected size ... Mirror sync in progress?"),
# and apt-get update then exits 100. The Phase 1 M7 stranger test hit this, so
# apt retries each download and the whole command, three times each.
# Retries do not help when the archive is slow or down rather than syncing (the
# post-M7 review measured 63 kB/s and timeouts); then the hint names the way out.
apt_hint() {
  log "apt: if the Ubuntu archive is slow or unreachable, point apt at another Ubuntu mirror"
  log "apt: (README, \"If apt-get is very slow or times out\") and run make setup again"
}
apt_retry() {
  local i
  for i in 1 2 3; do
    "$@" && return 0
    [ "$i" = 3 ] || { log "apt: failed (attempt $i of 3; a mirror sync?), retrying in 30 s"; sleep 30; }
  done
  apt_hint
  return 1
}
if ((${#missing[@]})); then
  log "apt: installing ${missing[*]}"
  apt_t0=$(date +%s)
  apt_retry "${SUDO[@]}" apt-get -o Acquire::Retries=3 update
  apt_retry "${SUDO[@]}" env DEBIAN_FRONTEND=noninteractive apt-get -o Acquire::Retries=3 install -y \
    --no-install-recommends "${missing[@]}"
  apt_s=$(($(date +%s) - apt_t0))
  if [ "$apt_s" -gt 300 ]; then log "apt: took $apt_s s"; apt_hint; fi
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
if [ "${MODE}" != demo ]; then
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

# ---- Zephyr SDK (optional, SETUP=zephyr) -----------------------------------------
if [ "${MODE}" = zephyr ]; then
  log "zephyr: SDK ${ZEPHYR_SDK_VERSION} (scripts/dms_zephyr.sh sdk; the workspace is made by make fw-rebuild / dms-firmware)"
  bash "${REPO}/scripts/dms_zephyr.sh" sdk
fi

log "setup (${MODE}) complete; run 'make doctor$([ "${MODE}" = demo ] && echo ' SETUP=demo')'"
