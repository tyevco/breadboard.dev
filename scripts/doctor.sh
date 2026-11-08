#!/usr/bin/env bash
# Print toolchain versions; exit non-zero if anything required is missing.
set -uo pipefail
if [ -f "${HOME}/.cargo/env" ]; then . "${HOME}/.cargo/env"; fi
export PATH="${HOME}/.local/bin:${PATH}"

fail=0
check() {
  local name="$1"; shift
  local out
  if out="$("$@" 2>&1)"; then
    printf '%-18s %s\n' "${name}:" "$(printf '%s' "${out}" | head -n1)"
  else
    printf '%-18s MISSING (%s)\n' "${name}:" "$*"
    fail=1
  fi
}

check cargo            cargo --version
check rustc            rustc --version
check rustfmt          rustfmt --version
check clippy           cargo clippy --version
check arm-none-eabi-gcc arm-none-eabi-gcc --version
check renode           renode --version
check ngspice          sh -c 'ngspice --version | grep -m1 -i "ngspice-"'
check uv               uv --version
check pkg-config       pkg-config --version

lib="$(ldconfig -p | awk '/libngspice\.so\.0 /{print $NF; exit}')"
if [ -n "${lib}" ] && [ -e "${lib}" ]; then
  printf '%-18s %s -> %s\n' "libngspice.so.0:" "${lib}" "$(readlink -f "${lib}")"
else
  printf '%-18s MISSING\n' "libngspice.so.0:"
  fail=1
fi
hdr=/usr/include/ngspice/sharedspice.h
if [ -f "${hdr}" ]; then printf '%-18s %s\n' "sharedspice.h:" "${hdr}"; else printf '%-18s missing (libngspice0-dev)\n' "sharedspice.h:"; fi

ecc="$(dirname "$(readlink -f "$(command -v renode 2>/dev/null || echo /nonexistent)")")/tools/external_control_client"
if [ -f "${ecc}/include/renode_api.h" ]; then
  printf '%-18s %s\n' "renode ext-ctrl:" "${ecc}"
else
  printf '%-18s not found\n' "renode ext-ctrl:"
fi

printf '%-18s %s\n' "CARGO_TARGET_DIR:" "${CARGO_TARGET_DIR:-<unset>}"
exit "${fail}"
