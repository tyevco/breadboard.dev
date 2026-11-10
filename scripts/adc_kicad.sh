#!/usr/bin/env bash
# Phase 1 plan M4, gate M4a (ADR-007 D9 and Amendment 1): examples/adc
# drawn in KiCad (examples/adc/kicad), imported and run as a project,
# against the hand-written examples/adc/board.cir.
#
#  1. The committed board.bbm and bb.lock are current (`bb open --check`),
#     and a fresh import gives the same bytes (part of gate M4d), from the
#     committed .net and, when kicad-cli is installed, from the schematics
#     (`kicad =`; required with KICAD_REQUIRED=1).
#  2. The project run: check_adc.py; equal, except grant records, to its
#     own one-cell reference (`--lookahead off --adc-push every`, AGENTS.md:
#     every new run mode has an equivalence gate), and both equal to the
#     committed hashes tests/golden/adc-kicad/outputs.sha256. The `kicad =`
#     route's run is byte-identical to the .net route's.
#  3. Against board.cir's committed one-cell reference
#     (tests/golden/adc/outputs.sha256): the non-grant records (every
#     pin_edge and log line) byte for byte; nets.csv and nets.vcd byte for
#     byte, or else (D9's stated fallback) within the golden tolerance
#     (1 mV) with every digital value and timing identical.
#  4. The cause of an analog difference is shown, not assumed: board.cir
#     with C1 written as the decimal the importer emits (4.7e-7, not 470n,
#     which ngspice parses to a double one ulp away) must give the KiCad
#     run's nets.csv, nets.vcd and non-grant records byte for byte.
#  5. bb run changes no file of the project (part of gate M4d).
#
# The project is copied off /mnt/c (AGENTS.md: per-run file I/O off the
# Windows drive) into $ADC_KICAD_WORK and the firmware recorded there with
# `bb flash`; `--prepare` does only that (for pool-equiv and cost-table,
# which run $ADC_KICAD_WORK/net).
#
# usage: scripts/adc_kicad.sh <bb> <out> [extra bb run args...]
#        scripts/adc_kicad.sh --prepare <bb>
set -euo pipefail
PREPARE=0
if [[ $1 == --prepare ]]; then PREPARE=1; shift; fi
BB=$(realpath "$1")
OUT=${2:-}
shift $(( PREPARE ? 1 : 2 ))
ROOT=$(cd "$(dirname "$0")/.." && pwd)
WORK=${ADC_KICAD_WORK:-$HOME/.cache/bb-adc-kicad}
ELF=$ROOT/examples/adc/firmware/build/adc_board.elf
HASHES=$ROOT/tests/golden/adc/outputs.sha256
OWN=$ROOT/tests/golden/adc-kicad/outputs.sha256
FOR=${ADC_FOR:-300ms}
PROBES=(--probe net:ADC_IN --probe net:U1.PA4 --probe net:U1.PA5 --probe net:U1.PA6 --probe net:U1.PA7 --probe net:LED)
ROUTES=(net)
if command -v "${BB_KICAD_CLI:-kicad-cli}" >/dev/null 2>&1 && [[ $PREPARE == 0 ]]; then
    ROUTES+=(kicad)
elif [[ ${KICAD_REQUIRED:-0} == 1 ]]; then
    echo "adc-kicad: FAIL: kicad-cli not found (KICAD_REQUIRED=1)" >&2
    exit 1
fi

rm -rf "$WORK"
mkdir -p "$WORK"
nongrant() { grep -v '"type":"grant"' "$1/events.jsonl" > "$1/non-grant-records"; }

# 1. Import.
"$BB" open "$ROOT/examples/adc/kicad" --check >/dev/null
for route in "${ROUTES[@]}"; do
    cp -r "$ROOT/examples/adc/kicad" "$WORK/$route"
    rm -rf "$WORK/$route/.bb"
    cp "$ELF" "$WORK/$route/adc_board.elf"
    if [[ $route == kicad ]]; then
        sed -i 's|^netlist = "adc.net".*$|kicad = "adc.kicad_pro"|' "$WORK/kicad/bb.toml"
        grep -q '^kicad = "adc.kicad_pro"' "$WORK/kicad/bb.toml"
    fi
    "$BB" open "$WORK/$route" >/dev/null
    cmp "$WORK/$route/board.bbm" "$ROOT/examples/adc/kicad/board.bbm"
    "$BB" flash --project "$WORK/$route" --target U1 "$WORK/$route/adc_board.elf" >/dev/null
done
if [[ $PREPARE == 1 ]]; then
    echo "adc-kicad: prepared $WORK/net"
    exit 0
fi
rm -rf "$OUT"
mkdir -p "$OUT"
echo "adc-kicad: board.bbm current, and identical from: ${ROUTES[*]}"
[[ ${#ROUTES[@]} == 1 ]] && echo "adc-kicad: kicad-cli not found: the kicad = route was not run"

# 2. The project run, its one-cell reference, the other route (5 on the first).
tree_list() { (cd "$1" && find . -type f -print0 | sort -z | xargs -0 sha256sum); }
tree_list "$WORK/net" > "$WORK/before.txt"
"$BB" run --project "$WORK/net" --for "$FOR" --out "$OUT/net" "$@"
tree_list "$WORK/net" > "$WORK/after.txt"
if ! cmp -s "$WORK/before.txt" "$WORK/after.txt"; then
    diff "$WORK/before.txt" "$WORK/after.txt" || true
    echo "adc-kicad: FAIL: bb run changed files of the project (ADR-007 D7)" >&2
    exit 1
fi
echo "adc-kicad: bb run changed no file of the project ($(wc -l < "$WORK/before.txt") files)"
python3 "$ROOT/scripts/check_adc.py" "$OUT/net" --require-no-stops
"$BB" run --project "$WORK/net" --for "$FOR" --out "$OUT/ref" --lookahead off --adc-push every "$@" >/dev/null
nongrant "$OUT/net"
nongrant "$OUT/ref"
for f in nets.csv nets.vcd non-grant-records; do
    cmp "$OUT/net/$f" "$OUT/ref/$f"
done
(cd "$OUT/ref" && sha256sum -c --quiet "$OWN")
(cd "$OUT/net" && grep -v ' events.jsonl$' "$OWN" | sha256sum -c --quiet)
echo "adc-kicad: the project run equals its one-cell reference except grant records, and both the committed $(basename "$(dirname "$OWN")") hashes"
if [[ " ${ROUTES[*]} " == *" kicad "* ]]; then
    "$BB" run --project "$WORK/kicad" --for "$FOR" --out "$OUT/kicad" "$@" >/dev/null
    for f in nets.csv nets.vcd events.jsonl; do
        cmp "$OUT/net/$f" "$OUT/kicad/$f"
    done
    echo "adc-kicad: the .net and kicad-cli routes give byte-identical nets.csv, nets.vcd and events.jsonl"
fi

# 3. Against board.cir's committed reference.
(cd "$OUT/net" && grep ' non-grant-records$' "$HASHES" | sha256sum -c --quiet)
echo "adc-kicad: non-grant records (every pin_edge and log) identical to board.cir's committed reference"
"$BB" run --netlist "$ROOT/examples/adc/board.cir" --firmware "$ELF" --target U1 --for "$FOR" \
    "${PROBES[@]}" --out "$OUT/board-cir" "$@" >/dev/null
nongrant "$OUT/board-cir"
(cd "$OUT/board-cir" && grep -v ' events.jsonl$' "$HASHES" | sha256sum -c --quiet)
if (cd "$OUT/net" && grep -v ' events.jsonl$' "$HASHES" | sha256sum -c --quiet >/dev/null 2>&1); then
    echo "adc-kicad: nets.csv and nets.vcd byte-identical to board.cir's committed reference"
else
    echo "adc-kicad: nets.csv/nets.vcd differ from board.cir's reference; D9 fallback: 1 mV, digital exact"
    python3 "$ROOT/scripts/compare_outputs.py" "$OUT/board-cir" "$OUT/net" --tol-uv 1000
fi

# 4. The cause: board.cir with C1's value as the importer writes it.
mkdir -p "$WORK/cir"
cp "$ROOT/examples/adc/bb.lock" "$WORK/cir/"
sed 's/^C1 ADC_IN 0 470n$/C1 ADC_IN 0 4.7e-7/' "$ROOT/examples/adc/board.cir" > "$WORK/cir/board.cir"
grep -q '^C1 ADC_IN 0 4.7e-7$' "$WORK/cir/board.cir"
"$BB" run --netlist "$WORK/cir/board.cir" --firmware "$ELF" --target U1 --for "$FOR" \
    "${PROBES[@]}" --out "$OUT/board-cir-4.7e-7" "$@" >/dev/null
nongrant "$OUT/board-cir-4.7e-7"
for f in nets.csv nets.vcd non-grant-records; do
    cmp "$OUT/net/$f" "$OUT/board-cir-4.7e-7/$f"
done
echo "adc-kicad: board.cir with C1 = 4.7e-7 gives the KiCad run's nets.csv, nets.vcd and non-grant records byte for byte"
test -s "$OUT/net/run.json"
echo "adc-kicad: PASS"
