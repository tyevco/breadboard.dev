#!/usr/bin/env bash
# Phase 1 plan M11 (ADR-007 D5, D7, Amendment 5): the dial-me-spinner system,
# examples/dial-me-spinner/system (main + base, mated by the link harness),
# at 6854d25.
#
#  1. Import: a copy of the project gives the committed board.bbm byte for
#     byte, twice (board.bbm, bb.lock, .bb/resolution.json and
#     .bb/unresolved.toml identical); `bb open --check` on the committed
#     project. The base board is 21 parts, 17 nets, 81 pin nodes; the main
#     board 201, 142, 657 (as M4b); the 15 shared references; each board its
#     own ground; [[connection]] main:J1 - base:J3 through
#     dial-me-spinner/link-harness@0.1.0:wires with pins 1-12 mated, every
#     pair on nets of the same leaf name; the harness locked.
#  2. The kicad = route for both boards (kicad-cli 10 in WSL; KICAD=0 skips
#     it, reported) gives the same board.bbm.
#  3. Secondary (as M4c's): dc96515's KiCad 9.0.9 base netlist differs
#     exactly where ADR-007 Context 12 says (J1's shield pin S1 is SH at
#     6854d25): reported.
#  4. Only the simulated board is elaborated (D5): with E1's bindings,
#     firmware and benches, `bb run --elaborate-only` on the system writes
#     the deck of the single-board E1 project (examples/dial-me-spinner)
#     with every name qualified (mapped through the two board.bbm files),
#     and warns that the base side of J1 is left open.
# The name-resolution errors (a bare U1, U2, J1 or TP1 lists both
# candidates) are cli/tests/system.rs (in make cargo-test).
#
# Everything runs on copies in $DMS_SYSTEM_WORK (off /mnt/c); the repository
# is never written.
#
# usage: scripts/dms_system.sh <bb> <out>
set -euo pipefail
BB=$(realpath "$1")
OUT=$(mkdir -p "$2" && realpath "$2")
ROOT=$(cd "$(dirname "$0")/.." && pwd)
DMS=$ROOT/examples/dial-me-spinner
W=${DMS_SYSTEM_WORK:-$HOME/.cache/bb-dms-system}
KICAD=${KICAD:-1}
SUMMARY=$OUT/summary.txt
rm -rf "$W"
mkdir -p "$W"
: > "$SUMMARY"
say() { echo "dms-system: $*" | tee -a "$SUMMARY"; }
fail() { echo "dms-system: FAIL: $*" | tee -a "$SUMMARY" >&2; exit 1; }
export BB_FIRST_PARTY_PACKAGES=$ROOT/packages

# A copy of the system project at <name>/system, with the vendored KiCad
# inputs at <name>/kicad (bb.toml names ../kicad/...).
proj() { # <name>
    local d=$W/$1
    mkdir -p "$d"
    cp -r "$DMS/kicad" "$d/kicad"
    rsync -a --exclude .bb "$DMS/system/" "$d/system/"
    echo "$d/system"
}
open_q() { "$BB" open "$1" > "$1/open.log" 2>&1 || { cat "$1/open.log"; return 1; }; }

# --- 1. Import.
"$BB" open "$DMS/system" --check > /dev/null \
    || fail "examples/dial-me-spinner/system: board.bbm or bb.lock not current (run bb open)"
REF=$(proj ref)
open_q "$REF"
cmp "$REF/board.bbm" "$DMS/system/board.bbm" || fail "the committed board.bbm differs from a fresh import"
for f in board.bbm bb.lock .bb/resolution.json .bb/unresolved.toml; do cp "$REF/$f" "$W/first.$(basename "$f")"; done
open_q "$REF"
for f in board.bbm bb.lock .bb/resolution.json .bb/unresolved.toml; do
    cmp "$REF/$f" "$W/first.$(basename "$f")" || fail "a second bb open changed $f"
done
say "import: the committed board.bbm ($(sha256sum "$REF/board.bbm" | cut -c1-16)...), twice byte-identical (board.bbm, bb.lock, .bb/resolution.json, .bb/unresolved.toml)"
python3 - "$REF" <<'EOF' | tee -a "$SUMMARY"
import json, sys
d = sys.argv[1]
b = json.load(open(f"{d}/board.bbm"))
def counts(board):
    ins = [i for i in b["instances"] if i["board"] == board]
    nets = [n for n in b["nets"] if n["board"] == board]
    return len(ins), len(nets), sum(len(n["nodes"]) for n in nets)
for board, want in (("main", (201, 142, 657)), ("base", (21, 17, 81))):
    got = counts(board)
    print(f"dms-system: {board}@6854d25: {got[0]} parts, {got[1]} nets, {got[2]} pin nodes")
    assert got == want, f"{board}: {got} != {want}"
assert [(x["id"], x["ground"]) for x in b["boards"]] == [("base", "base:GND"), ("main", "main:GND")]
gnd = {n["name"]: n["spice"] for n in b["nets"] if n["name"] in ("main:GND", "base:GND")}
assert gnd == {"main:GND": "0", "base:GND": "base_GND"}, gnd
refs = lambda board: {i["ref"] for i in b["instances"] if i["board"] == board}
shared = sorted(refs("main") & refs("base"))
print(f"dms-system: shared references ({len(shared)}): {' '.join(shared)}")
assert shared == sorted(["C1", "C2", "C3", "C4", "J1", "R1", "R2", "R3", "R4",
                         "TP1", "TP2", "TP3", "TP4", "U1", "U2"]), shared
ins = {(i["board"], i["ref"]): i for i in b["instances"]}
for q in ("U1", "U2", "J1", "TP1"):
    print(f"dms-system:   {q}: main:{q} {ins[('main', q)]['value_text']}, base:{q} {ins[('base', q)]['value_text']}")
(c,) = b["connections"]
assert (c["a"], c["b"], c["model"]) == ("main:J1", "base:J3", "dial-me-spinner/link-harness@0.1.0:wires"), c
assert c["pins"] == sorted([[str(n), str(n)] for n in range(1, 13)]), c["pins"]
leaf = lambda n: n.split(":", 1)[1].rsplit("/", 1)[-1]
net = lambda board, ref, pin: next(p.get("net") for p in ins[(board, ref)]["pins"] if p["number"] == pin)
for x, y in c["pins"]:
    na, nb = net("main", "J1", x), net("base", "J3", y)
    assert leaf(na) == leaf(nb), (x, na, nb)
print(f"dms-system: connection {c['a']} - {c['b']} through {c['model']}: {len(c['pins'])} pins, every pair on nets of one leaf name")
lock = open(f"{d}/bb.lock").read()
assert 'id = "dial-me-spinner/link-harness"' in lock
EOF

# --- 2. The kicad = route for both boards.
if [ "$KICAD" = 1 ] && command -v kicad-cli > /dev/null; then
    K=$(proj kicad)
    sed -i -e 's|^netlist = "../kicad/main/dial-me-spinner-main.net"$|kicad = "../kicad/main/dial-me-spinner-main.kicad_pro"|' \
           -e 's|^netlist = "../kicad/base/dial-me-spinner-base.net"$|kicad = "../kicad/base/dial-me-spinner-base.kicad_pro"|' "$K/bb.toml"
    [ "$(grep -c '^kicad = ' "$K/bb.toml")" = 2 ] || fail "kicad route: bb.toml edit"
    ( cd "$W/kicad/kicad" && find . -type f -exec sha256sum {} + | sort ) > "$W/src.before"
    open_q "$K"
    ( cd "$W/kicad/kicad" && find . -type f -exec sha256sum {} + | sort ) > "$W/src.after"
    cmp "$W/src.before" "$W/src.after" || fail "kicad route: bb open changed the KiCad sources"
    cmp "$K/board.bbm" "$DMS/system/board.bbm" || fail "kicad route: board.bbm differs from the netlist route"
    say "kicad route: kicad-cli $(kicad-cli --version) on both boards gives the same board.bbm; sources unchanged"
else
    say "kicad route: SKIPPED (KICAD=$KICAD, kicad-cli $(command -v kicad-cli || echo missing))"
fi

# --- 3. Secondary: dc96515's KiCad 9.0.9 base netlist.
S=$(proj dc96515)
cp "$DMS/kicad/kicad9-dc96515/base/dial-me-spinner-base.net" "$W/dc96515/kicad/base/dial-me-spinner-base.net"
open_q "$S"
python3 - "$S/board.bbm" "$DMS/system/board.bbm" <<'EOF' | tee -a "$SUMMARY"
import json, sys
k9, k10 = (json.load(open(p)) for p in sys.argv[1:3])
def flat(b):
    out = {}
    for i in b["instances"]:
        out[("instance", i["board"], i["ref"])] = json.dumps(i, sort_keys=True)
    for n in b["nets"]:
        out[("net", n["name"])] = json.dumps(n, sort_keys=True)
    for k in ("boards", "platforms", "probes", "connections"):
        out[(k,)] = json.dumps(b.get(k), sort_keys=True)
    return out
a, b = flat(k9), flat(k10)
diff = sorted(k for k in set(a) | set(b) if a.get(k) != b.get(k))
print(f"dms-system: secondary: dc96515's KiCad 9.0.9 base netlist differs in {diff}")
assert diff == [("instance", "base", "J1"), ("net", "base:GND")], diff
j9 = {p["number"] for p in json.loads(a[("instance", "base", "J1")])["pins"]}
j10 = {p["number"] for p in json.loads(b[("instance", "base", "J1")])["pins"]}
print(f"dms-system: secondary: base:J1 pins only at dc96515 {sorted(j9 - j10)}, only at 6854d25 {sorted(j10 - j9)} (ADR-007 Context 12: KiCad 10.0.8's USB-C shield pin SH)")
assert (j9 - j10, j10 - j9) == ({"S1"}, {"SH"})
EOF

# --- 4. Only the simulated board is elaborated: E1 on the system.
E=$(proj elab)
cp -r "$DMS/bench" "$E/bench"
mkdir -p "$E/firmware/prebuilt"
cp "$DMS/firmware/prebuilt/led_rail.elf" "$E/firmware/prebuilt/"
python3 - "$E/bb.toml" "$DMS/bb.toml" <<'EOF'
import re, sys
sys_toml, e1 = sys.argv[1], open(sys.argv[2]).read()
# E1's [targets] and [[bind]]s (already qualified main:...); the probes of
# main only (the base board's nets are not in the deck).
blocks = re.findall(r"^(\[targets\.\"main:U10\"\]\n(?:[^\[\n].*\n)+|\[\[bind\]\]\n(?:[^\[\n#].*\n)+)", e1, re.M)
assert len(blocks) == 5, blocks
s = open(sys_toml).read()
s = s.replace('nets = ["main:TP1", "base:TP1", "base:TP2", "base:TP3"]',
              'nets = ["VLED", "LED_DIN", "LED_DIN_MCU", "LED_DIN_R", "VSYS"]')
assert '"VLED"' in s
open(sys_toml, "w").write(s + "\n" + "\n".join(blocks))
EOF
"$BB" open "$E" --check > /dev/null || fail "elab: the system's board.bbm changed with E1's bindings"
for bench in cold nominal; do
    "$BB" run --project "$E" --elaborate-only --for 120ms --bench "$E/bench/$bench.toml" \
        --out "$W/sys-$bench" > "$W/sys-$bench.log" 2>&1 || { cat "$W/sys-$bench.log"; fail "elab: system $bench"; }
    "$BB" run --project "$DMS" --elaborate-only --for 120ms --bench "$DMS/bench/$bench.toml" \
        --out "$W/e1-$bench" > "$W/e1-$bench.log" 2>&1 || { cat "$W/e1-$bench.log"; fail "elab: E1 $bench"; }
    grep -qF "[[connection]] main:J1 - base:J3: base:J3 is not simulated (Phase 1 simulates main only, ADR-007 D5), so these nets at main:J1 are left open: main:BTN_N, main:CHG_INT_N, main:I2C_SCL, main:I2C_SDA, main:USB_DM, main:USB_DP, main:VBUS" "$W/sys-$bench.log" \
        || { cat "$W/sys-$bench.log"; fail "elab: $bench: the bench-port warning"; }
    python3 - "$W/sys-$bench/deck.cir" "$W/e1-$bench/deck.cir" "$E/board.bbm" "$DMS/board.bbm" "$bench" <<'EOF' | tee -a "$SUMMARY"
import json, re, sys
sdeck, edeck, sbbm, ebbm, bench = sys.argv[1:6]
sys_nets = {n["name"]: n["spice"] for n in json.load(open(sbbm))["nets"] if n["board"] == "main"}
e1_nets = {n["name"]: n["spice"] for n in json.load(open(ebbm))["nets"]}
assert {k.split(":", 1)[1] for k in sys_nets} == set(e1_nets)
m = {sys_nets[k]: e1_nets[k.split(":", 1)[1]] for k in sys_nets}
pre = ("B_bench_", "V_bench_", "V_pwl_", "R_pwl_", "bb_pwl_")
def tok(t):
    if t in m:
        return m[t]
    if t.startswith("Xmain_"):
        return "X" + t[len("Xmain_"):]
    for p in pre:
        if t.startswith(p) and t[len(p):] in m:
            return p + m[t[len(p):]]
    g = re.fullmatch(r"(I_bench_load\d+_)(.*)", t)
    if g and g.group(2) in m:
        return g.group(1) + m[g.group(2)]
    return t
s = open(sdeck).read()
mapped = re.sub(r"[A-Za-z0-9_]+", lambda x: tok(x.group(0)), s)
e = open(edeck).read()
assert "Xbase_" not in s and "dms_link_harness" not in s
assert mapped == e, "the system's deck is not E1's with qualified names"
n_inst = sum(1 for l in s.splitlines() if l.startswith("Xmain_"))
print(f"dms-system: elab {bench}: the system's deck ({n_inst} instances, all main:) is E1's deck with qualified names; base side of J1 open (warned)")
EOF
done
say "PASS"
