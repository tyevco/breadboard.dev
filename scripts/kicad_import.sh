#!/usr/bin/env bash
# Phase 1 plan M4, gates M4b-M4f (ADR-007 "Gates (M4)", restated for the
# reference revision 6854d25 by approver decision C):
#
#  M4b  the dial-me-spinner main board at 6854d25, from the vendored
#       netlist: 201 parts, 142 nets, 657 pin nodes. (The synthetic
#       two-board fixture is cli/tests/kicad_import.rs, in make test.)
#  M4f  the resolution counts: 118 generic passives (89 C, 28 R, 1 L), 12
#       that need no model, J11 by its override; since plan M5 51 parts by
#       MPN (ti/tps22917 x2: U2 and U21; worldsemi/ws2812c-2020 x48; the
#       raytac/mdbt50q-u1mv2 module U10) and 19 MPN parts unresolved (70 at
#       M4, before the E1 packages); without the override J11 is the only
#       unresolved part with no MPN and no LCSC (the one "prompt").
#  M4c  one board.bbm from: the committed KiCad 10.0.8 netlist; a fresh
#       Linux kicad-cli export (as a .net, and through `kicad =`); a
#       Windows kicad-cli export (as a .net, and through `kicad =`); the
#       schematics after a partial KiCad 10 re-save (some sheets 20260306,
#       the rest as committed). Secondary (not a gate for E1, decision C):
#       dc96515's KiCad 9.0.9 netlist, compared and reported.
#  M4d  two `bb open` runs byte-identical (board.bbm, bb.lock,
#       .bb/resolution.json, .bb/unresolved.toml); the KiCad sources
#       unchanged by `bb open` (no .kicad_prl). (`bb run` is read-only:
#       scripts/adc_kicad.sh.)
#  M4e  refused: a schematic stamped 20261106, a .net from Eeschema 11, a
#       netlist with a stacked pin [1-4].
#
# Everything runs on copies in $KICAD_IMPORT_WORK (off /mnt/c); the
# repository is never written. Needs kicad-cli 10 in WSL; the Windows build
# (BB_KICAD_CLI_WIN, default C:\Program Files\KiCad\10.0\bin\kicad-cli.exe)
# unless KICAD_WINDOWS=0, which is then reported as skipped.
#
# usage: scripts/kicad_import.sh <bb> <out>
set -euo pipefail
BB=$(realpath "$1")
OUT=$(mkdir -p "$2" && realpath "$2")
ROOT=$(cd "$(dirname "$0")/.." && pwd)
DMS=$ROOT/examples/dial-me-spinner
W=${KICAD_IMPORT_WORK:-$HOME/.cache/bb-kicad-import}
WIN_CLI=${BB_KICAD_CLI_WIN:-/mnt/c/Program Files/KiCad/10.0/bin/kicad-cli.exe}
KICAD_WINDOWS=${KICAD_WINDOWS:-1}
SUMMARY=$OUT/summary.txt
rm -rf "$W"
mkdir -p "$W"
: > "$SUMMARY"
say() { echo "kicad-import: $*" | tee -a "$SUMMARY"; }
fail() { echo "kicad-import: FAIL: $*" | tee -a "$SUMMARY" >&2; exit 1; }

# A project copy whose main board comes from <source line>.
proj() { # <name> <toml line for the board source>
    local d=$W/$1
    mkdir -p "$d"
    cp -r "$DMS/kicad" "$d/kicad"
    sed "s|^netlist = \"kicad/main/dial-me-spinner-main.net\"$|$2|" "$DMS/bb.toml" > "$d/bb.toml"
    grep -qF "$2" "$d/bb.toml" || fail "$1: bb.toml edit"
    echo "$d"
}
open_q() { "$BB" open "$1" > "$1/open.log" 2>&1 || { cat "$1/open.log"; return 1; }; }

# --- M4b, M4f, M4d: the vendored netlist.
REF=$(proj ref 'netlist = "kicad/main/dial-me-spinner-main.net"')
open_q "$REF"
"$BB" open "$DMS" --check > /dev/null || fail "examples/dial-me-spinner: board.bbm or bb.lock not current"
cmp "$REF/board.bbm" "$DMS/board.bbm" || fail "M4b: the committed board.bbm differs from a fresh import"
python3 - "$REF" <<'EOF' | tee -a "$SUMMARY"
import collections, json, sys
d = sys.argv[1]
b = json.load(open(f"{d}/board.bbm"))
r = json.load(open(f"{d}/.bb/resolution.json"))
inst, nets = b["instances"], b["nets"]
nodes = sum(len(n["nodes"]) for n in nets)
print(f"kicad-import: M4b: main@6854d25: {len(inst)} parts, {len(nets)} nets, {nodes} pin nodes")
assert (len(inst), len(nets), nodes) == (201, 142, 657), "M4b counts"
st = collections.Counter(i["resolution"]["stage"] for i in inst)
gen = collections.Counter(i["package"]["id"] for i in inst if i["resolution"]["stage"] == "generic")
print(f"kicad-import: M4f: stages {dict(sorted(st.items()))}; generic {dict(sorted(gen.items()))}")
assert st == {"generic": 118, "no_model": 12, "mpn": 51, "unresolved": 19, "override": 1}, "M4f stages"
mpn = collections.Counter(i["package"]["id"] for i in inst if i["resolution"]["stage"] == "mpn")
print(f"kicad-import: M4f: by MPN {dict(sorted(mpn.items()))}")
assert mpn == {"ti/tps22917": 2, "worldsemi/ws2812c-2020": 48, "raytac/mdbt50q-u1mv2": 1}, "M4f mpn (plan M5)"
assert sorted(b["platforms"], key=lambda x: x["id"])[0]["id"] == "renode:nrf52840-bb@2", "the module's chip platform"
assert gen == {"generic/capacitor": 89, "generic/resistor": 28, "generic/inductor": 1}, "M4f generic"
nm = sorted(i["footprint"].split(":")[0] for i in inst if i["resolution"]["stage"] == "no_model")
assert collections.Counter(nm) == {"MountingHole": 3, "TestPoint": 9}, nm
assert [i["ref"] for i in inst if i["resolution"]["stage"] == "override"] == ["J11"]
unres = [i for i in inst if i["resolution"]["stage"] == "unresolved"]
assert all("MPN" in i.get("fields", {}) for i in unres), "every unresolved part has an MPN"
assert r["stages"] == dict(st)
print("kicad-import: M4f: J11 resolved by its override; the 19 unresolved all carry an MPN")
EOF
# M4f without the override: J11 is the only part with no key.
NOOV=$W/no-override
mkdir -p "$NOOV" && cp -r "$DMS/kicad" "$NOOV/" && sed '/^\[instances."main:J11"\]/,$d' "$DMS/bb.toml" > "$NOOV/bb.toml"
open_q "$NOOV"
python3 - "$NOOV" <<'EOF' | tee -a "$SUMMARY"
import json, sys
b = json.load(open(f"{sys.argv[1]}/board.bbm"))
un = [i for i in b["instances"] if i["status"] == "unmodelled"]
nokey = [i["ref"] for i in b["instances"] if i["resolution"]["stage"] == "unresolved"
         and not ({"MPN", "LCSC"} & set(i.get("fields", {})))]
print(f"kicad-import: M4f: without the override, {len(un)} unmodelled; with no MPN or LCSC: {nokey}")
assert nokey == ["J11"]
EOF
grep -q '\[instances."main:J11"\]' "$NOOV/.bb/unresolved.toml" || fail "M4f: no stub for J11"
# M4d: a second import, byte for byte.
cp -r "$REF" "$W/ref-first"
open_q "$REF"
for f in board.bbm bb.lock .bb/resolution.json .bb/unresolved.toml; do
    cmp "$REF/$f" "$W/ref-first/$f" || fail "M4d: $f differs between two bb open runs"
done
say "M4d: two bb open runs give byte-identical board.bbm, bb.lock, .bb/resolution.json and .bb/unresolved.toml"
REFH=$(sha256sum < "$REF/board.bbm" | cut -d' ' -f1)
say "M4c: reference board.bbm (vendored 6854d25 netlist): sha256 $REFH"

same() { # <label> <project dir>
    cmp -s "$2/board.bbm" "$REF/board.bbm" || { diff <(python3 -m json.tool "$REF/board.bbm") <(python3 -m json.tool "$2/board.bbm") | head -40; fail "M4c: $1 gives a different board.bbm"; }
    say "M4c: $1: identical board.bbm"
}
tree() { (cd "$1" && find . -type f -print0 | sort -z | xargs -0 sha256sum); }

# --- M4c: fresh Linux kicad-cli.
command -v kicad-cli >/dev/null || fail "kicad-cli not found (KiCad 10 in WSL)"
say "kicad-cli (Linux): $(kicad-cli version)"
EXP=$W/linux-export && mkdir -p "$EXP" && cp -r "$DMS/kicad/main/." "$EXP/"
(cd "$EXP" && kicad-cli sch export netlist --format kicadsexpr -o fresh.net dial-me-spinner-main.kicad_sch >/dev/null)
L=$(proj linux-net 'netlist = "fresh.net"')
cp "$EXP/fresh.net" "$L/fresh.net"
open_q "$L" && same "fresh Linux kicad-cli export (.net)" "$L"
K=$(proj linux-kicad 'kicad = "kicad/main/dial-me-spinner-main.kicad_pro"')
tree "$K/kicad" > "$W/src-before.txt"
open_q "$K" && same "kicad = route, Linux kicad-cli" "$K"
tree "$K/kicad" > "$W/src-after.txt"
cmp -s "$W/src-before.txt" "$W/src-after.txt" || fail "M4d: bb open changed the KiCad sources"
test -z "$(find "$K/kicad" -name '*.kicad_prl')" || fail "M4d: a .kicad_prl appeared"
say "M4d: the KiCad sources are unchanged after bb open (no .kicad_prl; $(wc -l < "$W/src-before.txt") files)"
python3 -c "import json,sys; j=json.load(open(sys.argv[1])); b=j['boards'][0]; print('kicad-import: import.json:', b['route'], b['tool'], len(b['sources']), 'sources,', sorted(set(s['version'] for s in b['schematics'])))" "$K/.bb/import.json" | tee -a "$SUMMARY"

# --- M4c: Windows kicad-cli.
if [ "$KICAD_WINDOWS" = 1 ]; then
    [ -f "$WIN_CLI" ] || fail "Windows kicad-cli not found: $WIN_CLI (KICAD_WINDOWS=0 to skip)"
    say "kicad-cli (Windows): $("$WIN_CLI" version | tr -d '\r')"
    WK=$(proj win-kicad 'kicad = "kicad/main/dial-me-spinner-main.kicad_pro"')
    BB_KICAD_CLI="$WIN_CLI" "$BB" open "$WK" > "$WK/open.log" 2>&1 || { cat "$WK/open.log"; fail "Windows kicad= route"; }
    same "kicad = route, Windows kicad-cli" "$WK"
    WT=$(cd /mnt/c && cmd.exe /C "echo %TEMP%" 2>/dev/null | tr -d '\r')
    WD=$(wslpath -u "$WT")/bb-kicad-import-$$
    rm -rf "$WD" && mkdir -p "$WD" && cp -r "$DMS/kicad/main/." "$WD/"
    "$WIN_CLI" sch export netlist --format kicadsexpr -o "$(wslpath -w "$WD")\\win.net" "$(wslpath -w "$WD")\\dial-me-spinner-main.kicad_sch" >/dev/null
    WN=$(proj win-net 'netlist = "win.net"')
    cp "$WD/win.net" "$WN/win.net"
    rm -rf "$WD"
    grep -q $'\r' "$WN/win.net" && say "Windows export is CRLF, as ADR-007 Context 2 says"
    open_q "$WN" && same "Windows kicad-cli export (.net, CRLF)" "$WN"
else
    say "M4c: Windows kicad-cli SKIPPED (KICAD_WINDOWS=0)"
fi

# --- M4c: a partial KiCad 10 re-save.
P=$(proj partial 'kicad = "kicad/main/dial-me-spinner-main.kicad_pro"')
UP=$W/upgraded && mkdir -p "$UP" && cp -r "$DMS/kicad/main/." "$UP/"
for s in dial-me-spinner-main.kicad_sch mcu.kicad_sch leds.kicad_sch; do
    kicad-cli sch upgrade --force "$UP/$s" >/dev/null 2>&1 || true
done
for s in dial-me-spinner-main.kicad_sch mcu.kicad_sch leds.kicad_sch; do
    cp "$UP/$s" "$P/kicad/main/$s"
done
rm -f "$P"/kicad/main/*.kicad_prl
python3 - "$P/kicad/main" <<'EOF' | tee -a "$SUMMARY"
import glob, os, re, sys
v = {}
for f in sorted(glob.glob(sys.argv[1] + "/*.kicad_sch")):
    head = open(f).read(400)
    v[os.path.basename(f)] = re.search(r"\(version (\d+)\)", head).group(1)
print("kicad-import: partial re-save versions:", v)
assert set(v.values()) == {"20250114", "20260306"}, "need a mix of KiCad 9-header and KiCad 10 sheets"
EOF
open_q "$P" && same "kicad = route after a partial KiCad 10 re-save (root, mcu, leds at 20260306)" "$P"

# --- M4c secondary: dc96515 (KiCad 9.0.9), not a gate for E1.
S=$(proj dc96515 'netlist = "kicad/kicad9-dc96515/main/dial-me-spinner-main.net"')
open_q "$S"
if cmp -s "$S/board.bbm" "$REF/board.bbm"; then
    say "M4c secondary: dc96515's KiCad 9.0.9 netlist gives the identical board.bbm (connectivity and D3 content identical)"
else
    diff <(python3 -m json.tool "$REF/board.bbm") <(python3 -m json.tool "$S/board.bbm") > "$OUT/dc96515.diff" || true
    say "M4c secondary: dc96515 DIFFERS (recorded in $OUT/dc96515.diff)"
    fail "M4c secondary: dc96515 differs; record the difference in ADR-007"
fi

# --- M4e: refusals.
refuse() { # <label> <project> <expected text>
    if "$BB" open "$2" > "$2/open.log" 2>&1; then fail "M4e: $1 was accepted"; fi
    grep -q "$3" "$2/open.log" || { cat "$2/open.log"; fail "M4e: $1: unexpected message"; }
    say "M4e: refused: $1: $(grep -m1 "$3" "$2/open.log" | cut -c1-160)"
}
E1=$(proj k11-sch 'kicad = "kicad/main/dial-me-spinner-main.kicad_pro"')
sed -i 's/(version 20250114)/(version 20261106)/' "$E1/kicad/main/mcu.kicad_sch"
grep -q '(version 20261106)' "$E1/kicad/main/mcu.kicad_sch"
refuse "a sheet stamped 20261106 (KiCad 11)" "$E1" "newer than this bb supports"
E2=$(proj k11-net 'netlist = "kicad/main/dial-me-spinner-main.net"')
sed -i 's/(tool "Eeschema 10.0.8-10.0.8~ubuntu24.04.1")/(tool "Eeschema 11.0.0")/' "$E2/kicad/main/dial-me-spinner-main.net"
grep -q 'Eeschema 11.0.0' "$E2/kicad/main/dial-me-spinner-main.net"
refuse "a .net from Eeschema 11.0.0" "$E2" "KiCad 11 is newer"
E3=$(proj stacked 'netlist = "kicad/main/dial-me-spinner-main.net"')
python3 - "$E3/kicad/main/dial-me-spinner-main.net" <<'EOF'
import sys
p = sys.argv[1]
s = open(p).read()
i = s.index("(nets")
j = s.index('(pin "1")', i)
open(p, "w").write(s[:j] + '(pin "[1-4]")' + s[j + len('(pin "1")'):])
EOF
refuse "a netlist with a stacked pin [1-4]" "$E3" "stacked pin"
E4=$(proj nonet 'netlist = "kicad/main/dial-me-spinner-main.net"')
sed -i 's/(version "E")/(version "D")/' "$E4/kicad/main/dial-me-spinner-main.net"
refuse "a netlist of format D" "$E4" "this bb reads format"

say "PASS"
