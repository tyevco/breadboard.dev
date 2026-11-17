#!/usr/bin/env bash
# Phase 2 E1 (ADR-011 G1-G5, G9, G10; plan section 4 track E): the registry
# end to end against local fixtures only, a file:// registry and a local
# static HTTPS server on 127.0.0.1. Nothing here reaches the network: the
# signing keys are throwaway ones made in the work directory, the HTTPS
# certificate is self-signed for 127.0.0.1 and trusted through
# CURL_CA_BUNDLE, and the `bb run`s run with a `curl` on PATH that records
# any call, the file:// registry moved away and (where unprivileged user
# namespaces work) in a network namespace with loopback only.
#
# usage: registry_test.sh <bb> [test|gate]
#   test (make registry-test, in make test): every check below.
#   gate (make registry-gate): the same, then G7 (scripts/registry_g7.sh).
set -euo pipefail

BB=$(readlink -f "$1")
MODE=${2:-test}
REPO=$(cd "$(dirname "$0")/.." && pwd)
W=${BB_SCRATCH:-$HOME/.cache}/bb-registry-test-$MODE
rm -rf "$W"
mkdir -p "$W"
PYTHON=${PYTHON:-python3}
FAILS=0
N=0
T0=$(date +%s.%N)

ok() {
    N=$((N + 1))
    echo "  ok   $*"
}
bad() {
    N=$((N + 1))
    FAILS=$((FAILS + 1))
    echo "  FAIL $*"
}
check() { # check <name> <command...>
    local name=$1
    shift
    if "$@" >"$W/last.out" 2>&1; then ok "$name"; else
        bad "$name"
        sed 's/^/       | /' "$W/last.out" | tail -20
    fi
}
refused() { # refused <name> <needle> <command...>: must fail, saying <needle>
    local name=$1 needle=$2
    shift 2
    if "$@" >"$W/last.out" 2>&1; then
        bad "$name (succeeded)"
        sed 's/^/       | /' "$W/last.out" | tail -10
    elif grep -qF -- "$needle" "$W/last.out"; then
        ok "$name: $(grep -F -- "$needle" "$W/last.out" | head -1 | sed 's/^ *//' | cut -c1-160)"
    else
        bad "$name (failed without \"$needle\")"
        sed 's/^/       | /' "$W/last.out" | tail -10
    fi
}
says() { # says <name> <needle> <command...>: must succeed, saying <needle>
    local name=$1 needle=$2
    shift 2
    if ! "$@" >"$W/last.out" 2>&1; then
        bad "$name (failed)"
        sed 's/^/       | /' "$W/last.out" | tail -10
    elif grep -qF -- "$needle" "$W/last.out"; then
        ok "$name: $(grep -F -- "$needle" "$W/last.out" | head -1 | sed 's/^ *//' | cut -c1-160)"
    else
        bad "$name (no \"$needle\")"
        sed 's/^/       | /' "$W/last.out" | tail -10
    fi
}

for t in minisign openssl curl tar "$PYTHON"; do
    command -v "$t" >/dev/null || {
        echo "registry-test: $t is missing (make setup installs it)"
        exit 1
    }
done

# --- keys, certificate, server, fixtures -------------------------------------------
minisign -G -W -p "$W/key.pub" -s "$W/key.sec" >/dev/null 2>&1
KEY=$(tail -1 "$W/key.pub")
minisign -G -W -p "$W/acme.pub" -s "$W/acme.sec" >/dev/null 2>&1
ACME_KEY=$(tail -1 "$W/acme.pub")
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 2 \
    -subj /CN=127.0.0.1 -addext subjectAltName=IP:127.0.0.1 \
    -keyout "$W/tls.key" -out "$W/tls.pem" >/dev/null 2>&1
export CURL_CA_BUNDLE="$W/tls.pem"
WWW="$W/www"
mkdir -p "$WWW/vendor"
"$PYTHON" "$REPO/scripts/registry_https.py" "$WWW" "$W/tls.pem" "$W/tls.key" "$W/port" \
    2>"$W/https.log" &
HTTPS_PID=$!
trap 'rc=$?; kill $HTTPS_PID 2>/dev/null || true; exit $rc' EXIT
for _ in $(seq 100); do [ -s "$W/port" ] && break; sleep 0.05; done
PORT=$(cat "$W/port")
HTTPS="https://127.0.0.1:$PORT"
EMPTY="$W/no-first-party"
mkdir -p "$EMPTY"
PUBTIME=2026-11-15
CACHE_N=0
fresh_cache() {
    CACHE_N=$((CACHE_N + 1))
    export BB_REGISTRY_CACHE="$W/cache-$CACHE_N"
}
fresh_cache

sign() { # sign <registry dir> [secret key]: bump the serial, sign the snapshot
    "$BB" registry snapshot "$1" >/dev/null
    minisign -S -s "${2:-$W/key.sec}" -m "$1/snapshot.json" -t "bb-registry test" >/dev/null
}

# A CPU package like st/stm32f103c8 under another id or version (a board
# file's parts need a behavioural layer; the platform is the spike's).
fixture() { # fixture <dir> <id> <version> [extra [behavioral] line] [part major]
    local d=$1 id=$2 v=$3 extra=${4:-} major=${5:-1}
    mkdir -p "$d"
    sed -e "s#^schema = \"bb-pins/1\"#schema = \"bb-pins/$major\"#" \
        "$REPO/packages/st/stm32f103c8/pins.toml" >"$d/pins.toml"
    sed -e "s#^id = \"st/stm32f103c8\"#id = \"$id\"#" -e "s#^version = \"0.1.0\"#version = \"$v\"#" \
        -e "s#^schema = \"bb-part/1\"#schema = \"bb-part/$major\"#" \
        "$REPO/packages/st/stm32f103c8/part.toml" >"$d/part.toml"
    if [ -n "$extra" ]; then
        sed -i "s#^platform = \"stm32f103-offline@1\"#&\n$extra#" "$d/part.toml"
    fi
}

# A board file whose U1 is package <spec> (the spike's circuit).
board() { # board <dir> <spec>
    rm -rf "$1"
    mkdir -p "$1"
    sed "s#^\*bb part U1 package=st/stm32f103c8#*bb part U1 package=$2#" \
        "$REPO/examples/spike/board.cir" >"$1/board.cir"
}

# A one-MCU KiCad netlist project whose U1 names <spec> in its bb:part field.
project() { # project <dir> <spec> [bb.toml tail]
    rm -rf "$1"
    mkdir -p "$1"
    cat >"$1/board.net" <<EOF
(export (version "E")
  (design (source "board.kicad_sch") (date "2026-11-15T00:00:00") (tool "Eeschema 10.0.8"))
  (components
    (comp (ref "U1") (value "MCU") (footprint "Package_QFP:LQFP-48_7x7mm_P0.5mm")
      (fields (field (name "bb:part") "$2"))
      (libsource (lib "MCU_ST_STM32F1") (part "STM32F103C8Tx"))
      (sheetpath (names "/") (tstamps "/"))))
  (libparts
    (libpart (lib "MCU_ST_STM32F1") (part "STM32F103C8Tx")
      (pins (pin (num "15") (name "PA5") (type "passive")) (pin (num "47") (name "VSS") (type "passive")))))
  (nets
    (net (code "1") (name "GND") (node (ref "U1") (pin "47")))
    (net (code "2") (name "PA5") (node (ref "U1") (pin "15")))))
EOF
    printf 'schema = "bb-project/1"\n\n[[board]]\nid = "main"\nnetlist = "board.net"\nground = "GND"\n%s' \
        "${3:-}" >"$1/bb.toml"
}

ELF="$REPO/examples/spike/firmware/build/blinky.elf"
[ -f "$ELF" ] || make -C "$REPO/examples/spike/firmware" >/dev/null
RUN_ARGS=(--firmware "$ELF" --target U1 --for 500ms --probe net:LED_A --probe net:PA5)
mkdir -p "$W/shim"
cat >"$W/shim/curl" <<EOF
#!/bin/sh
echo "curl \$*" >>"$W/curl-called"
exit 7
EOF
chmod +x "$W/shim/curl"
NETNS=none
if bash "$REPO/scripts/offline.sh" true >/dev/null 2>&1; then NETNS=unshare; fi
# bb run with no registry reachable: run_offline <out> [VAR=value...] -- <bb run args...>
run_offline() {
    local out=$1 envs=()
    shift
    while [ "$1" != "--" ]; do
        envs+=("$1")
        shift
    done
    shift
    if [ "$NETNS" = unshare ]; then
        # A fresh Renode inside the namespace (a pool's Renode is outside it).
        env -u BB_RENODE_POOL "${envs[@]}" PATH="$W/shim:$PATH" \
            bash "$REPO/scripts/offline.sh" "$BB" run "$@" --out "$out"
    else
        env "${envs[@]}" PATH="$W/shim:$PATH" "$BB" run "$@" --out "$out"
    fi
}
# The locked packages (without their source) and platforms of a lock.
lock_ids() {
    "$PYTHON" - "$1" <<'EOF'
import re, sys
t = open(sys.argv[1]).read()
for b in re.split(r"\n(?=\[\[)", t):
    if b.startswith("[[package]]") or b.startswith("[[platform]]"):
        kv = dict(re.findall(r'^(\w+) = "([^"]*)"$', b, re.M))
        kv.pop("source", None)
        print(" ".join(f"{k}={kv[k]}" for k in sorted(kv)))
EOF
}
lock_field() { # lock_field <lock> <id> <key>
    "$PYTHON" - "$@" <<'EOF'
import re, sys
t = open(sys.argv[1]).read()
for b in re.split(r"\n(?=\[\[)", t):
    kv = dict(re.findall(r'^(\w+) = "([^"]*)"$', b, re.M))
    if b.startswith("[[package]]") and kv.get("id") == sys.argv[2]:
        print(kv[sys.argv[3]])
EOF
}

echo "registry-test ($MODE): $W; HTTPS on $HTTPS; bb run isolation: curl shim + $NETNS"

# --- G5: deterministic archives -------------------------------------------------------
echo "G5: deterministic archives"
for p in st/stm32f103c8 ti/tps22917 nordic/nrf52840-qiaa; do
    "$BB" pkg pack "$REPO/packages/$p" -o "$W/a1.tar" >/dev/null
    "$BB" pkg pack "$REPO/packages/$p" -o "$W/a2.tar" >/dev/null
    check "$p: two packs byte-identical" cmp "$W/a1.tar" "$W/a2.tar"
    rm -rf "$W/x"
    check "$p: the extracted tree hash is the source's" \
        test "$("$BB" pkg unpack "$W/a1.tar" "$W/x")" = "$("$BB" pkg tree "$REPO/packages/$p")"
    check "$p: ustar sorted, 0644/0755, uid/gid 0, no names, mtime 0; GNU tar reads it" \
        "$PYTHON" - "$W/a1.tar" <<'EOF'
import sys, tarfile, subprocess
t = tarfile.open(sys.argv[1], format=tarfile.USTAR_FORMAT)
ms = t.getmembers()
names = [m.name + ("/" if m.isdir() else "") for m in ms]
assert [n.encode() for n in names] == sorted(n.encode() for n in names), names
for m in ms:
    assert m.uid == 0 and m.gid == 0 and m.uname == "" and m.gname == "" and m.mtime == 0, m
    assert m.mode == (0o755 if m.isdir() else 0o644), (m.name, oct(m.mode))
    assert m.isdir() or m.isfile(), m.name
subprocess.run(["tar", "-tf", sys.argv[1]], check=True, capture_output=True)
EOF
done

# --- the public registry stand-in: every first-party package ----------------------------
echo "registry: every first-party package published (D7 licence gate, D11 names)"
R="$WWW/public"
"$BB" registry init "$R" --key "$W/key.pub" >/dev/null
printf 'default = ["tyevco"]\n\n[namespaces]\nacme = ["acme-bot"]\n' >"$R/owners.toml"
check "bb registry publish every first-party package" \
    "$BB" registry publish "$R" --pubtime $PUBTIME "$REPO/packages"
NPKG=$(find "$REPO/packages" -name part.toml | wc -l)
check "$NPKG index files" test "$(find "$R/index" -name '*.jsonl' | wc -l)" = "$NPKG"
sign "$R"
check "bb registry check (the PR checks) passes" "$BB" registry check "$R"
# ADR-011 Amendment 3 (the technical exit): versions are immutable (D2), so a version published
# while bb provided its platform stays valid in the index after a later bb retires that platform
# (as nrf52840-bb@3 and dms/mechanism@1 were); `bb registry check` does not re-lint the published
# versions' [behavioral] ids, while a NEW version naming a platform this bb lacks is still refused.
RT="$W/retired"
rm -rf "$RT" "$W/pkg-retired" "$W/pkg-retired-new"
cp -a "$R" "$RT"
fixture "$W/pkg-retired" fixture/retired 0.1.0
"$BB" registry publish "$RT" --pubtime $PUBTIME "$W/pkg-retired" >/dev/null
sed -i 's#^platform = "stm32f103-offline@1"#platform = "retired-bb@1"#' "$W/pkg-retired/part.toml"
"$BB" pkg pack "$W/pkg-retired" -o "$RT/dl/fixture/retired/0.1.0.tar" >/dev/null
"$PYTHON" - "$RT/index/fixture/retired.jsonl" "$RT/dl/fixture/retired/0.1.0.tar" \
    "$("$BB" pkg tree "$W/pkg-retired")" <<'EOF'
import json, os, sys
idx, tar, tree = sys.argv[1:]
l = json.loads(open(idx).read())
l.update(tree=tree, size=os.path.getsize(tar), platforms=["renode:retired-bb@1"])
open(idx, "w").write(json.dumps(l, sort_keys=True, separators=(",", ":")) + "\n")
EOF
"$BB" registry snapshot "$RT" >/dev/null
check "bb registry check: a published version whose platform this bb has retired stays valid (D2)" \
    "$BB" registry check "$RT"
fixture "$W/pkg-retired-new" fixture/retired-new 0.1.0
sed -i 's#^platform = "stm32f103-offline@1"#platform = "retired-bb@1"#' "$W/pkg-retired-new/part.toml"
refused "  a new version naming that platform is refused by publish" "this bb provides" \
    "$BB" registry publish "$RT" --pubtime $PUBTIME "$W/pkg-retired-new"
check "index lines, snapshot and config valid against spec/schema" "$PYTHON" - "$R" "$REPO" <<'EOF'
import json, pathlib, sys, jsonschema
r, repo = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
s = lambda n: json.load(open(repo / "spec/schema" / n))
n = 0
for f in sorted((r / "index").rglob("*.jsonl")):
    for l in f.read_text().splitlines():
        jsonschema.validate(json.loads(l), s("index-line.schema.json"))
        n += 1
jsonschema.validate(json.load(open(r / "snapshot.json")), s("snapshot.schema.json"))
jsonschema.validate(json.load(open(r / "config.json")), s("registry-config.schema.json"))
assert n >= 11, n
EOF
idx_before=$(cat "$R"/index/*/*.jsonl | sha256sum)
check "a second publish of the same trees (another pubtime) is a no-op" \
    "$BB" registry publish "$R" --pubtime 2099-01-01 "$REPO/packages"
check "  (the index is byte-identical)" test "$(cat "$R"/index/*/*.jsonl | sha256sum)" = "$idx_before"
FILE_ENV=(BB_PUBLIC_REGISTRY="file://$R" BB_PUBLIC_REGISTRY_KEY="$KEY")
HTTPS_ENV=(BB_PUBLIC_REGISTRY="$HTTPS/public/" BB_PUBLIC_REGISTRY_KEY="$KEY")

# --- G1: end to end ----------------------------------------------------------------------
echo "G1: publish, resolve, offline run (file://)"
S="$W/spike-file"
board "$S" st/stm32f103c8
check "bb open resolves st/stm32f103c8 on the registry (no first-party packages)" \
    env "${FILE_ENV[@]}" BB_FIRST_PARTY_PACKAGES="$EMPTY" "$BB" open "$S/board.cir"
check "  the lock's source is registry+file://" \
    grep -qF "source = \"registry+file://$R/\"" "$S/bb.lock"
check "  its packages and platforms equal the committed spike lock's (same trees)" \
    diff <(lock_ids "$S/bb.lock") <(lock_ids "$REPO/examples/spike/bb.lock")
mv "$R" "$R.away"
rm -f "$W/curl-called"
check "bb run with the registry gone, no curl ($NETNS): from the cache" \
    run_offline "$W/out-file" "${FILE_ENV[@]}" BB_FIRST_PARTY_PACKAGES="$EMPTY" -- \
    --netlist "$S/board.cir" "${RUN_ARGS[@]}"
mv "$R.away" "$R"
check "  nets.csv, events.jsonl, nets.vcd = tests/golden/spike/outputs.sha256 (the committed run)" \
    bash -c "cd '$W/out-file' && sha256sum -c --quiet '$REPO/tests/golden/spike/outputs.sha256'"
check "  curl was never called" test ! -e "$W/curl-called"

echo "G1: the same over HTTPS (curl, cold cache)"
fresh_cache
S2="$W/spike-https"
board "$S2" st/stm32f103c8
check "bb open over https://127.0.0.1 with a cold cache" \
    env "${HTTPS_ENV[@]}" BB_FIRST_PARTY_PACKAGES="$EMPTY" "$BB" open "$S2/board.cir"
check "  the lock's source is registry+https://" \
    grep -qF "source = \"registry+$HTTPS/public/\"" "$S2/bb.lock"
check "  the same trees as the committed lock" \
    diff <(lock_ids "$S2/bb.lock") <(lock_ids "$REPO/examples/spike/bb.lock")
check "  the server served the snapshot, its signature, config, index and archive" \
    bash -c "grep -c 'GET /public/' '$W/https.log' | grep -qv '^0$' && grep -q 'dl/st/stm32f103c8/0.1.0.tar' '$W/https.log'"
kill $HTTPS_PID
wait $HTTPS_PID 2>/dev/null || true
says "bb open --offline with the server down: the warm cache" "is current" \
    env "${HTTPS_ENV[@]}" BB_FIRST_PARTY_PACKAGES="$EMPTY" "$BB" open --offline "$S2/board.cir"
refused "bb open (online) with the server down" "fetch $HTTPS/public/snapshot.json" \
    env "${HTTPS_ENV[@]}" BB_FIRST_PARTY_PACKAGES="$EMPTY" "$BB" open "$S2/board.cir"
check "bb run --elaborate-only from the warm cache with the server down" \
    env "${HTTPS_ENV[@]}" BB_FIRST_PARTY_PACKAGES="$EMPTY" PATH="$W/shim:$PATH" \
    "$BB" run --netlist "$S2/board.cir" "${RUN_ARGS[@]}" --elaborate-only --out "$W/out-https"
"$PYTHON" "$REPO/scripts/registry_https.py" "$WWW" "$W/tls.pem" "$W/tls.key" "$W/port2" \
    2>>"$W/https.log" &
HTTPS_PID=$!
for _ in $(seq 100); do [ -s "$W/port2" ] && break; sleep 0.05; done
PORT=$(cat "$W/port2")
HTTPS="https://127.0.0.1:$PORT"
HTTPS_ENV=(BB_PUBLIC_REGISTRY="$HTTPS/public/" BB_PUBLIC_REGISTRY_KEY="$KEY")
check "bb registry verify over HTTPS (signature, every index file, every archive)" \
    "$BB" registry verify "$HTTPS/public/" --key "$KEY"
refused "an http:// registry is refused (curl is only asked for https://)" "https:// or file://" \
    env BB_PUBLIC_REGISTRY="http://127.0.0.1:$PORT/public/" BB_PUBLIC_REGISTRY_KEY="$KEY" \
    BB_FIRST_PARTY_PACKAGES="$EMPTY" "$BB" open "$S2/board.cir"

# --- G2: tampering ---------------------------------------------------------------------------
echo "G2: tampering"
tamper() { # tamper <name>: a copy of the public registry at $WWW/<name>
    rm -rf "$WWW/$1"
    cp -a "$R" "$WWW/$1"
    echo "$WWW/$1"
}
open_from() { # open_from <registry url> <board dir>: cold cache, no first-party
    fresh_cache
    env BB_PUBLIC_REGISTRY="$1" BB_PUBLIC_REGISTRY_KEY="${KEY_OVERRIDE:-$KEY}" \
        BB_FIRST_PARTY_PACKAGES="$EMPTY" "$BB" open "$2/board.cir"
}
T=$(tamper t-archive)
"$PYTHON" - "$T/dl/st/stm32f103c8/0.1.0.tar" <<'EOF'
import sys
p = sys.argv[1]
b = bytearray(open(p, "rb").read())
i = b.index(b'version = "0.1.0"')  # inside part.toml's content, not a header
b[i + 11] = ord("9")
open(p, "wb").write(bytes(b))
EOF
board "$W/b-tamper" st/stm32f103c8
refused "a modified archive byte (same size, valid headers): the tree hash" \
    "a tampered archive is refused" open_from "file://$T" "$W/b-tamper"
T=$(tamper t-header)
"$PYTHON" - "$T/dl/st/stm32f103c8/0.1.0.tar" <<'EOF'
import sys
p = sys.argv[1]
b = bytearray(open(p, "rb").read())
b[0] ^= 0x01  # the first header's name
open(p, "wb").write(bytes(b))
EOF
refused "a modified archive header byte" "header checksum mismatch" open_from "file://$T" "$W/b-tamper"
T=$(tamper t-index)
sed -i 's/"license":"Apache-2.0"/"license":"MIT"/' "$T/index/st/stm32f103c8.jsonl"
refused "a modified index line without a new signature" "a modified file is refused" \
    open_from "file://$T" "$W/b-tamper"
T=$(tamper t-snapshot)
sed -i 's/"serial":1}/"serial":7}/' "$T/snapshot.json"
refused "a modified snapshot (its old signature)" "the signature does not verify" \
    open_from "file://$T" "$W/b-tamper"
T=$(tamper t-unsigned)
rm "$T/snapshot.json.minisig"
refused "an unsigned snapshot" "an unsigned registry is refused" open_from "file://$T" "$W/b-tamper"
T=$(tamper t-otherkey)
minisign -S -s "$W/acme.sec" -m "$T/snapshot.json" -t x >/dev/null
refused "a snapshot signed by another key" "the signature does not verify" \
    open_from "file://$T" "$W/b-tamper"
rm -rf "$WWW"/t-*

echo "G2: a rolled-back snapshot"
cp -a "$R" "$W/public-serial1"
FX="$W/fixtures"
fixture "$FX/mcu-0.1.0" fixture/mcu 0.1.0
fixture "$FX/mcu-0.1.1" fixture/mcu 0.1.1
check "publish fixture/mcu 0.1.0 and 0.1.1 (serial 2)" \
    "$BB" registry publish "$R" --pubtime $PUBTIME "$FX/mcu-0.1.0" "$FX/mcu-0.1.1"
sign "$R"
board "$W/b-roll" fixture/mcu
fresh_cache
ROLL_CACHE=$BB_REGISTRY_CACHE
check "bb open sees serial 2" env "${FILE_ENV[@]}" BB_FIRST_PARTY_PACKAGES="$EMPTY" "$BB" open "$W/b-roll/board.cir"
mv "$R" "$W/public-serial2"
cp -a "$W/public-serial1" "$R"
refused "the registry served at serial 1 again (validly signed)" "a rolled-back registry is refused" \
    env "${FILE_ENV[@]}" BB_FIRST_PARTY_PACKAGES="$EMPTY" "$BB" open "$W/b-roll/board.cir"
rm -rf "$R"
mv "$W/public-serial2" "$R"
check "  serial 2 again: accepted" env "${FILE_ENV[@]}" BB_FIRST_PARTY_PACKAGES="$EMPTY" \
    "$BB" open "$W/b-roll/board.cir"

# --- G3: yanking ------------------------------------------------------------------------------
echo "G3: yanking"
Y1="$W/b-yank-locked"
board "$Y1" fixture/mcu
check "a lock on the newest version, 0.1.1" env "${FILE_ENV[@]}" BB_FIRST_PARTY_PACKAGES="$EMPTY" \
    "$BB" open "$Y1/board.cir"
check "  (locked 0.1.1)" test "$(lock_field "$Y1/bb.lock" fixture/mcu version)" = 0.1.1
cp -a "$R" "$W/public-before-yank"
check "bb registry yank fixture/mcu@0.1.1" "$BB" registry yank "$R" fixture/mcu@0.1.1
sign "$R"
check "  the yank PR passes the diff check (an owner)" \
    "$BB" registry check "$R" --base "$W/public-before-yank" --author tyevco
Y2="$W/b-yank-new"
board "$Y2" fixture/mcu
check "a new resolve skips the yanked version" env "${FILE_ENV[@]}" BB_FIRST_PARTY_PACKAGES="$EMPTY" \
    "$BB" open "$Y2/board.cir"
check "  (locked 0.1.0)" test "$(lock_field "$Y2/bb.lock" fixture/mcu version)" = 0.1.0
sha_before=$(sha256sum <"$Y1/bb.lock")
says "the existing lock keeps 0.1.1, with a warning" "is yanked on registry public" \
    env "${FILE_ENV[@]}" BB_FIRST_PARTY_PACKAGES="$EMPTY" "$BB" open "$Y1/board.cir"
check "  (bb.lock unchanged)" test "$(sha256sum <"$Y1/bb.lock")" = "$sha_before"
says "bb run on the existing lock warns from the cached index" "is yanked on file://" \
    env "${FILE_ENV[@]}" BB_FIRST_PARTY_PACKAGES="$EMPTY" PATH="$W/shim:$PATH" \
    "$BB" run --netlist "$Y1/board.cir" "${RUN_ARGS[@]}" --elaborate-only --out "$W/out-yank"
board "$W/b-yank-explicit" fixture/mcu@0.1.1
refused "an explicit new request for the yanked version" "is yanked on registry public" \
    env "${FILE_ENV[@]}" BB_FIRST_PARTY_PACKAGES="$EMPTY" "$BB" open "$W/b-yank-explicit/board.cir"

echo "G3: legal removal (D2)"
cp -a "$R" "$W/public-before-remove"
check "bb registry remove fixture/mcu@0.1.0" \
    "$BB" registry remove "$R" fixture/mcu@0.1.0 --reason "test takedown"
sign "$R"
check "  its archive is deleted" test ! -e "$R/dl/fixture/mcu/0.1.0.tar"
check "  the removal passes the diff check" \
    "$BB" registry check "$R" --base "$W/public-before-remove" --author tyevco
says "a lock on the removed version with a verified cached copy: kept, warned" "was removed from registry public" \
    env "${FILE_ENV[@]}" BB_FIRST_PARTY_PACKAGES="$EMPTY" "$BB" open "$Y2/board.cir"
fresh_cache
refused "  the same lock with a cold cache" "no verified copy is in the cache" \
    env "${FILE_ENV[@]}" BB_FIRST_PARTY_PACKAGES="$EMPTY" "$BB" open "$Y2/board.cir"

echo "G2/D8: the registry's diff checks"
D="$W/public-diff"
rm -rf "$D"
cp -a "$R" "$D"
sed -i 's/"pubtime":"2026-11-15"/"pubtime":"2026-11-16"/' "$D/index/st/stm32f103c8.jsonl"
refused "a published line changed (not yanked)" "changed beyond yanked/removed" \
    "$BB" registry check "$D" --base "$R" --author tyevco
rm -rf "$D"
cp -a "$R" "$D"
head -1 "$D/index/fixture/mcu.jsonl" >"$D/x" && mv "$D/x" "$D/index/fixture/mcu.jsonl"
refused "a published line deleted" "lines were deleted" "$BB" registry check "$D" --base "$R" --author tyevco
rm -rf "$D"
cp -a "$R" "$D"
"$BB" registry yank "$D" st/stm32f103c8@0.1.0 >/dev/null
refused "a yank by someone who owns no namespace" "neither the author nor an approver" \
    "$BB" registry check "$D" --base "$R" --author mallory
check "  the same yank approved by an owner" \
    "$BB" registry check "$D" --base "$R" --author mallory --approver tyevco
printf 'default = ["tyevco", "mallory"]\n' >"$D/owners.toml"
refused "a PR that makes its author an owner (the base's owners.toml decides)" \
    "only the default owners (tyevco) may approve that" \
    "$BB" registry check "$D" --base "$R" --author mallory
rm -rf "$D"
cp -a "$R" "$D"
"$PYTHON" - "$D/dl/st/stm32f103c8/0.1.0.tar" <<'EOF'
import sys
p = sys.argv[1]
b = bytearray(open(p, "rb").read())
i = b.index(b'version = "0.1.0"')
b[i + 11] = ord("9")
open(p, "wb").write(bytes(b))
EOF
refused "a replaced archive" "the archive's tree is" "$BB" registry check "$D" --base "$R" --author tyevco
rm -rf "$D"
fixture "$FX/mcu-0.1.1-changed" fixture/mcu 0.1.1
echo "# changed" >>"$FX/mcu-0.1.1-changed/part.toml"
refused "republishing a version with another tree" "versions are immutable" \
    "$BB" registry publish "$R" --pubtime $PUBTIME "$FX/mcu-0.1.1-changed"
fixture "$FX/lic" fixture/lic 0.1.0
sed -i 's/^license = "Apache-2.0"/license = "LicenseRef-local-only"/' "$FX/lic/part.toml"
refused "the licence gate: LicenseRef- (D7)" "cannot be published" \
    "$BB" registry publish "$R" --pubtime $PUBTIME "$FX/lic"
sed -i 's/^license = "LicenseRef-local-only"/license = "CC-BY-NC-4.0"/' "$FX/lic/part.toml"
refused "the licence gate: not OSI-approved (D7)" "not an OSI-approved licence" \
    "$BB" registry publish "$R" --pubtime $PUBTIME "$FX/lic"
mkdir -p "$FX/vd"
cp -a "$REPO/packages/generic/resistor/." "$FX/vd/"
sed -i -e 's#^id = "generic/resistor"#id = "fixture/vd"#' -e 's/^source = "[a-z_]*"/source = "vendor_derived"/' \
    "$FX/vd/part.toml"
sed -i 's/GENERIC_RESISTOR_IDEAL/FIXTURE_VD_IDEAL/g' "$FX/vd/part.toml" "$FX/vd/analog/resistor.lib" \
    "$FX/vd"/tests/*.cir
refused "a vendor_derived view without upstream_license (D7, the TPS22917 rule)" "vendor_derived" \
    "$BB" registry publish "$R" --pubtime $PUBTIME "$FX/vd"
mkdir -p "$FX/names"
cp -a "$REPO/packages/generic/resistor/." "$FX/names/"
sed -i 's#^id = "generic/resistor"#id = "fixture/names"#' "$FX/names/part.toml"
refused "an unprefixed top-level view name (G10, D11)" "does not start with FIXTURE_NAMES_" \
    "$BB" registry publish "$R" --pubtime $PUBTIME "$FX/names"

# --- G4: routing (dependency confusion) ------------------------------------------------------
echo "G4: routing by namespace"
A="$WWW/acme"
"$BB" registry init "$A" --key "$W/acme.pub" >/dev/null
fixture "$FX/acme-mcu-0.1.0" acme/mcu 0.1.0
check "the private registry acme publishes acme/mcu 0.1.0" \
    "$BB" registry publish "$A" --pubtime $PUBTIME "$FX/acme-mcu-0.1.0"
sign "$A" "$W/acme.sec"
fixture "$FX/evil-mcu-9.0.0" acme/mcu 9.0.0
fixture "$FX/evil-only" acme/only 0.1.0
check "the public registry publishes a higher acme/mcu 9.0.0 and acme/only" \
    "$BB" registry publish "$R" --pubtime $PUBTIME "$FX/evil-mcu-9.0.0" "$FX/evil-only"
sign "$R"
ROUTE=$(printf '\n[registries.acme]\nurl = "file://%s"\nkey = "%s"\nnamespaces = ["acme"]\n' "$A" "$ACME_KEY")
P1="$W/p-route"
project "$P1" acme/mcu "$(printf '%s\n[[bind]]\npin = "U1.PA5"\nmodel = "digital_out"\nnet = "PA5"\n' "$ROUTE")"
fresh_cache
P1_CACHE=$BB_REGISTRY_CACHE
check "a project routing acme to its registry resolves acme/mcu" \
    env "${FILE_ENV[@]}" BB_FIRST_PARTY_PACKAGES="$EMPTY" "$BB" open "$P1"
check "  from the private registry, at 0.1.0, not the public 9.0.0" \
    test "$(lock_field "$P1/bb.lock" acme/mcu source) $(lock_field "$P1/bb.lock" acme/mcu version)" = \
    "registry+file://$A/ 0.1.0"
P2="$W/p-route-missing"
project "$P2" acme/only "$ROUTE"
refused "an id of the routed namespace only the public registry has: never asked there" \
    "namespace acme is routed to it, so no other registry is asked" \
    env "${FILE_ENV[@]}" BB_FIRST_PARTY_PACKAGES="$EMPTY" "$BB" open "$P2"
check "  the public registry was not read for it" \
    test ! -e "$BB_REGISTRY_CACHE/$(echo "file$R" | tr -c 'A-Za-z0-9.\n-' _)/files/index/acme/only.jsonl"
P3="$W/p-noroute"
project "$P3" acme/only
fresh_cache
check "without the route, the same id resolves on the public registry" \
    env "${FILE_ENV[@]}" BB_FIRST_PARTY_PACKAGES="$EMPTY" "$BB" open "$P3"
P4="$W/p-route-local"
project "$P4" acme/only "$(printf '\n[packages]\npaths = ["pk"]\n%s' "$ROUTE")"
mkdir -p "$P4/pk/acme"
cp -a "$FX/evil-only" "$P4/pk/acme/only"
check "a local package of a routed namespace is used (local sources are not registries)" \
    env "${FILE_ENV[@]}" BB_FIRST_PARTY_PACKAGES="$EMPTY" "$BB" open "$P4"
check "  (path+ source)" test "$(lock_field "$P4/bb.lock" acme/only source)" = "path+pk/acme/only"
check "bb flash on the routed project finds the registry package (from the cache)" \
    env "${FILE_ENV[@]}" BB_FIRST_PARTY_PACKAGES="$EMPTY" PATH="$W/shim:$PATH" BB_REGISTRY_CACHE="$P1_CACHE" \
    "$BB" flash --target U1 "$ELF" --project "$P1"
check "bb run on the routed project reads the private tree from the cache" \
    env "${FILE_ENV[@]}" BB_FIRST_PARTY_PACKAGES="$EMPTY" PATH="$W/shim:$PATH" BB_REGISTRY_CACHE="$P1_CACHE" \
    "$BB" run --project "$P1" --for 1ms --probe net:PA5 --elaborate-only --out "$W/out-route"
check "  run.json records the package's registry source" \
    grep -qF "\"source\":\"registry+file://$A/\"" "$W/out-route/run.json"

# --- G9: every published major ------------------------------------------------------------------
echo "G9: reading every published major"
fixture "$FX/old-0.1.0" fixture/old 0.1.0 "" 0
fixture "$FX/old-0.2.0" fixture/old 0.2.0
check "publish fixture/old 0.1.0 (bb-part/0) and 0.2.0 (bb-part/1)" \
    "$BB" registry publish "$R" --pubtime $PUBTIME "$FX/old-0.1.0" "$FX/old-0.2.0"
sign "$R"
check "  the index records each line's schema" \
    bash -c "grep -q '\"schema\":\"bb-part/0\",.*\"vers\":\"0.1.0\"' '$R/index/fixture/old.jsonl' && grep -q '\"schema\":\"bb-part/1\",.*\"vers\":\"0.2.0\"' '$R/index/fixture/old.jsonl'"
for v in 0.1.0 0.2.0; do
    board "$W/b-old-$v" "fixture/old@$v"
    check "fixture/old@$v resolves" env "${FILE_ENV[@]}" BB_FIRST_PARTY_PACKAGES="$EMPTY" \
        "$BB" open "$W/b-old-$v/board.cir"
    check "  its locked tree is the source tree (the archive unchanged)" \
        test "$(lock_field "$W/b-old-$v/bb.lock" fixture/old tree)" = "$("$BB" pkg tree "$FX/old-$v")"
    check "  and it elaborates" env "${FILE_ENV[@]}" BB_FIRST_PARTY_PACKAGES="$EMPTY" PATH="$W/shim:$PATH" \
        "$BB" run --netlist "$W/b-old-$v/board.cir" "${RUN_ARGS[@]}" --elaborate-only --out "$W/out-old-$v"
done
fixture "$FX/new-0.1.0" fixture/new 0.1.0
"$BB" registry publish "$R" --pubtime $PUBTIME "$FX/new-0.1.0" >/dev/null
sed -i 's/"schema":"bb-part\/1"/"schema":"bb-part\/9"/' "$R/index/fixture/new.jsonl"
sign "$R"
board "$W/b-new" fixture/new
refused "an unknown major is refused with \"update bb\"" "update bb (ADR-011 D12)" \
    env "${FILE_ENV[@]}" BB_FIRST_PARTY_PACKAGES="$EMPTY" "$BB" open "$W/b-new/board.cir"

# --- vendor files (D5) ---------------------------------------------------------------------------
echo "vendor files: fetched at use, never redistributed"
printf '<?xml version="1.0"?>\n<device><name>STM32F103xx</name></device>\n' >"$WWW/vendor/STM32F103.svd"
SVD_SHA=$(sha256sum "$WWW/vendor/STM32F103.svd" | cut -d" " -f1)
fixture "$FX/svd" fixture/svd 0.1.0 \
    "svd = { url = \"$HTTPS/vendor/STM32F103.svd\", sha256 = \"$SVD_SHA\", license = \"LicenseRef-ST-EULA\" }"
check "a package naming a LicenseRef vendor file is publishable (the file is not in it)" \
    "$BB" registry publish "$R" --pubtime $PUBTIME "$FX/svd"
check "  its archive holds no .svd" bash -c "! tar -tf '$R/dl/fixture/svd/0.1.0.tar' | grep -q svd"
check "  its part.toml is valid against spec/schema/part.schema.json" "$PYTHON" - "$FX/svd/part.toml" "$REPO" <<'EOF'
import json, sys, tomllib, jsonschema
part = tomllib.load(open(sys.argv[1], "rb"))
assert "svd" in part["behavioral"], part["behavioral"]
jsonschema.validate(part, json.load(open(sys.argv[2] + "/spec/schema/part.schema.json")))
bad = dict(part, behavioral=dict(part["behavioral"], svd=dict(part["behavioral"]["svd"], url="http://x")))
try:
    jsonschema.validate(bad, json.load(open(sys.argv[2] + "/spec/schema/part.schema.json")))
    raise SystemExit("an http:// svd validated")
except jsonschema.ValidationError:
    pass
EOF
sign "$R"
V="$W/b-svd"
board "$V" fixture/svd
fresh_cache
check "bb open fetches the vendor file over HTTPS and records it" \
    env "${HTTPS_ENV[@]}" BB_FIRST_PARTY_PACKAGES="$EMPTY" "$BB" open "$V/board.cir"
check "  [[vendor_file]] url, sha256, license, used_by" "$PYTHON" - "$V/bb.lock" "$HTTPS" "$SVD_SHA" <<'EOF'
import sys
t = open(sys.argv[1]).read()
want = (f'[[vendor_file]]\nurl = "{sys.argv[2]}/vendor/STM32F103.svd"\nsha256 = "{sys.argv[3]}"\n'
        'license = "LicenseRef-ST-EULA"\nused_by = "fixture/svd"\n')
assert want in t, t
EOF
check "  the cache holds it by sha256" test -s "$BB_REGISTRY_CACHE/vendor/$SVD_SHA"
says "  bb open --offline: current from the cache" "is current" \
    env "${HTTPS_ENV[@]}" BB_FIRST_PARTY_PACKAGES="$EMPTY" "$BB" open --offline "$V/board.cir"
echo "tampered" >>"$WWW/vendor/STM32F103.svd"
fresh_cache
refused "a vendor file whose sha256 differs is refused" "the package declares" \
    env "${HTTPS_ENV[@]}" BB_FIRST_PARTY_PACKAGES="$EMPTY" "$BB" open "$V/board.cir"
fixture "$FX/svd-http" fixture/svdhttp 0.1.0 \
    "svd = { url = \"http://127.0.0.1/x.svd\", sha256 = \"$SVD_SHA\", license = \"LicenseRef-ST-EULA\" }"
refused "an http:// vendor file is refused at load" "https:// only" "$BB" pkg lint "$FX/svd-http"

# --- the local-first rule and the source-change rule (D4, D5, G8's second half) -----------------------
echo "D4: local sources first; D5: a change of source needs --update-lock"
FP="$W/first-party"
cp -a "$REPO/packages" "$FP"
echo "# edited in the checkout" >>"$FP/st/stm32f103c8/part.toml"
L="$W/b-local"
board "$L" st/stm32f103c8
fresh_cache
check "an edited first-party package resolves locally although the registry has it" \
    env "${FILE_ENV[@]}" BB_FIRST_PARTY_PACKAGES="$FP" "$BB" open "$L/board.cir"
check "  path+ source, the edited tree (not the published one)" \
    test "$(lock_field "$L/bb.lock" st/stm32f103c8 source) $(lock_field "$L/bb.lock" st/stm32f103c8 tree)" = \
    "path+$(realpath --relative-to="$L" "$FP/st/stm32f103c8") $("$BB" pkg tree "$FP/st/stm32f103c8")"
check "  and the registry was not asked" test ! -d "$BB_REGISTRY_CACHE"
refused "the registry-locked spike with the first-party package present: bb open" \
    "a change of source needs --update-lock" \
    env "${FILE_ENV[@]}" "$BB" open "$S/board.cir"
refused "  bb run refuses the same" "change of source" \
    env "${FILE_ENV[@]}" PATH="$W/shim:$PATH" \
    "$BB" run --netlist "$S/board.cir" "${RUN_ARGS[@]}" --elaborate-only --out "$W/out-src"
check "  bb open --update-lock records the path source" \
    env "${FILE_ENV[@]}" "$BB" open --update-lock "$S/board.cir"
check "  (path+, the same tree)" \
    diff <(lock_ids "$S/bb.lock") <(lock_ids "$REPO/examples/spike/bb.lock")
check "curl was never called by a bb run" test ! -e "$W/curl-called"

T1=$(date +%s.%N)
echo "registry-test ($MODE): $((N - FAILS))/$N checks passed in $("$PYTHON" -c "print(round($T1 - $T0, 1))") s"
if [ "$MODE" = gate ]; then
    kill $HTTPS_PID 2>/dev/null || true
    bash "$REPO/scripts/registry_g7.sh" "$BB" || FAILS=$((FAILS + 1))
fi
if [ "$FAILS" -ne 0 ]; then
    echo "registry-test: FAIL ($FAILS)"
    exit 1
fi
echo "registry-test: PASS"
