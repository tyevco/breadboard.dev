#!/usr/bin/env bash
# Phase 2 E1, ADR-011 G7 on the LOCAL public registry (approver decision: nothing is published):
# the registry repository with every first-party package (scripts/registry_repo.sh), signed with
# the registry key, served by a local static HTTPS server; bb verifies it with the key compiled
# into bb. Then dial-me-spinner's E1 project (examples/dial-me-spinner) as a user without a
# breadboard.dev checkout gets it: no first-party packages, its bb.lock's sources the registry's:
#   - with a cold cache, bb open fetches every package and finds board.bbm and bb.lock current;
#     the ok-cold E1 run passes check_dms_rail.py and gives the committed hashes
#     (tests/golden/dms-rail/ok-cold, non-grant records, nets.csv, nets.vcd);
#   - with the server stopped (a warm cache), bb open --offline is current and the same run, in a
#     network namespace where possible, gives the same bytes.
# Also the registry's CI scripts on a local pull request (ci/pr-check.sh), passing for an owner
# and refused for a non-owner.
#
#   scripts/registry_g7.sh <bb>
set -euo pipefail
BB=$(readlink -f "$1")
REPO=$(cd "$(dirname "$0")/.." && pwd)
REG=${BB_REGISTRY_REPO:-$HOME/src/bb-registry}
W=${BB_SCRATCH:-$HOME/.cache}/bb-registry-g7
rm -rf "$W"
mkdir -p "$W"
export BB_REGISTRY_CACHE="$W/cache"
PY=${PYTHON:-python3}
FAILS=0
ok() { echo "  ok   $*"; }
bad() {
    echo "  FAIL $*"
    FAILS=$((FAILS + 1))
}
run() { # run <name> <command...>
    local n=$1
    shift
    if "$@" >"$W/last.out" 2>&1; then ok "$n"; else
        bad "$n"
        tail -15 "$W/last.out" | sed 's/^/       | /'
    fi
}

echo "G7: the local public registry"
run "scripts/registry_repo.sh: $REG" bash "$REPO/scripts/registry_repo.sh" "$BB" "$REG"
cat "$W/last.out" | tail -1
NPKG=$(find "$REPO/packages" -name part.toml | wc -l)
run "every first-party package is in its index ($NPKG)" \
    test "$(find "$REG/index" -name '*.jsonl' | wc -l)" = "$NPKG"
run "bb registry check (the merge state)" "$BB" registry check "$REG"
run "the registry repository is committed (a clean tree)" test -z "$(git -C "$REG" status --porcelain)"
run "it has no remote" test -z "$(git -C "$REG" remote)"

openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 2 \
    -subj /CN=127.0.0.1 -addext subjectAltName=IP:127.0.0.1 \
    -keyout "$W/tls.key" -out "$W/tls.pem" >/dev/null 2>&1
export CURL_CA_BUNDLE="$W/tls.pem"
"$PY" "$REPO/scripts/registry_https.py" "$REG" "$W/tls.pem" "$W/tls.key" "$W/port" 2>"$W/https.log" &
PID=$!
trap 'rc=$?; kill $PID 2>/dev/null || true; exit $rc' EXIT
for _ in $(seq 100); do [ -s "$W/port" ] && break; sleep 0.05; done
URL="https://127.0.0.1:$(cat "$W/port")/"
# The public registry at a mirror URL, verified with the key compiled into bb (no key override).
export BB_PUBLIC_REGISTRY="$URL"
unset BB_PUBLIC_REGISTRY_KEY
KEY=$(sed -n 's/^pub const PUBLIC_KEY: &str = "\(.*\)";/\1/p' "$REPO/cli/src/registry.rs")
run "bb registry verify over HTTPS with the compiled key" "$BB" registry verify "$URL" --key "$KEY"
tail -1 "$W/last.out"

# dial-me-spinner's E1 project, as a user without a checkout gets it.
P=$W/dms
mkdir -p "$P/kicad/main" "$P/firmware/prebuilt"
SRC=$REPO/examples/dial-me-spinner
cp "$SRC/bb.toml" "$SRC/board.bbm" "$P/"
cp "$SRC/kicad/main/dial-me-spinner-main.net" "$P/kicad/main/"
cp -r "$SRC/bench" "$P/bench"
cp "$SRC/firmware/prebuilt/led_rail.elf" "$P/firmware/prebuilt/"
sed -E "s#^source = \"path\+[^\"]*\"#source = \"registry+$URL\"#" "$SRC/bb.lock" >"$P/bb.lock"
cp "$P/bb.lock" "$W/bb.lock.published"
EMPTY=$W/no-first-party
mkdir -p "$EMPTY"
export BB_FIRST_PARTY_PACKAGES="$EMPTY"
run "the published lock names only registry sources" \
    bash -c "! grep -q 'path+' '$P/bb.lock' && grep -c 'registry+$URL' '$P/bb.lock' | grep -qv '^0$'"
run "bb open, cold cache: every package from the registry, current" "$BB" open --check "$P"
run "  bb open writes nothing new (board.bbm and bb.lock unchanged)" bash -c \
    "'$BB' open '$P' >/dev/null && cmp '$P/board.bbm' '$SRC/board.bbm' && cmp '$P/bb.lock' '$W/bb.lock.published'"
run "  the trees equal the committed lock's" diff \
    <(grep '^tree' "$P/bb.lock") <(grep '^tree' "$SRC/bb.lock")
run "  the HTTPS server served every archive" bash -c \
    "test \$(grep -c 'GET /dl/' '$W/https.log') -ge \$(grep -c '^\[\[package\]\]' '$SRC/bb.lock')"
run "bb flash (the package from the cache)" "$BB" flash --project "$P" --target U10 "$P/firmware/prebuilt/led_rail.elf"
e1_run() { # e1_run <out> <prefix command...>: the ok-cold case at bb's defaults
    local out=$1
    shift
    "$@" "$BB" run --project "$P" --bench "$P/bench/cold.toml" --for 120ms --out "$out"
}
gold() { # gold <out>: check_dms_rail.py and the committed one-cell hashes except events.jsonl
    grep -v '"type":"grant"' "$1/events.jsonl" >"$1/non-grant-records"
    "$PY" "$REPO/scripts/check_dms_rail.py" "$1" --project "$SRC" >"$1/checks.txt" &&
        (cd "$1" && grep -v ' events.jsonl$' "$REPO/tests/golden/dms-rail/ok-cold/outputs.sha256" |
            sha256sum --quiet -c)
}
OUT=$W/out-online
run "E1 ok-cold from the registry packages" e1_run "$OUT" env
run "  check_dms_rail.py passes; non-grant records, nets.csv, nets.vcd = tests/golden/dms-rail/ok-cold" gold "$OUT"
run "  run.json records the registry sources" grep -qF "\"source\":\"registry+$URL\"" "$OUT/run.json"

kill $PID
wait $PID 2>/dev/null || true
run "warm cache, server stopped: bb open --offline is current" "$BB" open --offline --check "$P"
mkdir -p "$W/shim"
printf '#!/bin/sh\necho "curl $*" >>%s/curl-called\nexit 7\n' "$W" >"$W/shim/curl"
chmod +x "$W/shim/curl"
NS=(env PATH="$W/shim:$PATH")
if bash "$REPO/scripts/offline.sh" true >/dev/null 2>&1; then
    NS=(env -u BB_RENODE_POOL PATH="$W/shim:$PATH" bash "$REPO/scripts/offline.sh")
    echo "  (the offline run in a network namespace with loopback only)"
fi
OUT=$W/out-offline
run "E1 ok-cold offline from the warm cache" e1_run "$OUT" "${NS[@]}"
run "  the same checks and hashes" gold "$OUT"
run "  byte-identical to the online run" bash -c \
    "cmp '$W/out-online/nets.csv' '$OUT/nets.csv' && cmp '$W/out-online/nets.vcd' '$OUT/nets.vcd' && cmp '$W/out-online/non-grant-records' '$OUT/non-grant-records'"
run "  curl was never called" test ! -e "$W/curl-called"

echo "the registry's CI scripts on a local pull request"
C=$W/registry-clone
git clone -q "$REG" "$C"
git -C "$C" worktree add -q "$W/registry-base" HEAD
git -C "$C" checkout -q -b pr
mkdir -p "$C/packages/generic"
cp -a "$REPO/packages/st/stm32f103c8" "$C/packages/generic/pr-mcu"
sed -i -e 's#^id = "st/stm32f103c8"#id = "generic/pr-mcu"#' "$C/packages/generic/pr-mcu/part.toml"
git -C "$C" add -A
git -C "$C" -c user.name=pr -c user.email=pr@localhost commit -q -m "add generic/pr-mcu"
run "ci/pr-check.sh: a new package by an owner passes" \
    bash "$C/ci/pr-check.sh" "$BB" "$C" "$W/registry-base" tyevco
if bash "$C/ci/pr-check.sh" "$BB" "$C" "$W/registry-base" mallory >"$W/last.out" 2>&1; then
    bad "ci/pr-check.sh: the same package by a non-owner (passed)"
elif grep -q "neither the author nor an approver" "$W/last.out"; then
    ok "ci/pr-check.sh: the same package by a non-owner is refused (owners.toml)"
else
    bad "ci/pr-check.sh: refused for another reason"
    tail -5 "$W/last.out"
fi
run "ci/pr-check.sh: by a non-owner, approved by an owner" \
    bash "$C/ci/pr-check.sh" "$BB" "$C" "$W/registry-base" mallory tyevco

if [ "$FAILS" -ne 0 ]; then
    echo "registry-g7: FAIL ($FAILS)"
    exit 1
fi
echo "registry-g7: PASS"
