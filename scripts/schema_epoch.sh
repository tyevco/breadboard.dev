#!/usr/bin/env bash
# Milestone S (the Phase 2 schema epoch) deck gate: every committed board file
# and every project with each of its benches elaborates to the same deck before
# and after the epoch's migration, so every output hash holds.
#
#   scripts/schema_epoch.sh <bb> <base commit> [<out>]
#
# The base tree (`git archive <base>`: bb-part/0, bb-pins/0 and bb-lock/0 files
# with the legacy platform hashes) is elaborated three ways:
#   A  the base commit's own bb on the base tree (projects; that bb has no
#      --elaborate-only for a board file);
#   B  this bb on the base tree (its first-party packages through
#      BB_FIRST_PARTY_PACKAGES): it reads the previous majors;
#   C  this bb on this tree (the migrated /1 files, the renamed views, the
#      relocked bb-lock/1 files).
# Gates: A = B byte for byte (the code change alone moves no deck), and B = C
# byte for byte up to the first-party view renames (scripts/schema_epoch_decks.py:
# only whole-word renames of the ADR-011 D11 list, and the definition blocks the
# rename reorders in the deck's sorted (kind, name) order). A rename that a deck
# shows is counted, so the gate is not vacuous: at least one deck must carry
# each renamed view. Every `bb run` is --elaborate-only (no engine starts).
set -euo pipefail

BB=$(readlink -f "$1")
BASE=$2
OUT=$(mkdir -p "${3:-out/schema-epoch}" && cd "${3:-out/schema-epoch}" && pwd)
ROOT=$(cd "$(dirname "$0")/.." && pwd)
WORK=${BB_SCRATCH:-$HOME/.cache/bb}/bb-schema-epoch
PYTHON=${PYTHON:-python3}

rm -rf "$WORK/old" "$OUT"/{a,b,c} && mkdir -p "$WORK/old" "$OUT"
git -C "$ROOT" archive "$BASE" | tar -x -C "$WORK/old"
# The firmware a project's targets name (images are hashed, not read, by an
# elaboration) and the gitignored build outputs are not needed.
echo "schema-epoch: building the base commit's bb ($BASE) in $WORK/target"
( cd "$WORK/old" && CARGO_TARGET_DIR="$WORK/target" cargo build --release -q -p bb-cli )
OLD_BB=$WORK/target/release/bb

# <base path>[=<path in this tree>]. Phase 2 H5 (ADR-014 D4, a recorded model change) made the homing
# boards' five driver nets digital-only; their pre-H5 deck is e3/board-glued.cir's (M1's parameters,
# which tell board.cir, board-ts75.cir and board-park.cir apart, are not in the deck).
E3H=examples/dial-me-spinner/e3
BOARDS=(examples/spike/board.cir examples/feedback/board.cir examples/feedback-open/board.cir
        examples/adc/board.cir examples/adc/board-open.cir examples/nrf-saadc/board.cir
        examples/nrf-pwm/board.cir tests/device-smoke/board.cir "$E3H/board.cir=$E3H/board-glued.cir"
        "$E3H/board-ts75.cir=$E3H/board-glued.cir" examples/dial-me-spinner/e3/board-step.cir
        "$E3H/board-park.cir=$E3H/board-glued.cir")
PROJECTS=(examples/adc/kicad examples/bench examples/dial-me-spinner examples/dial-me-spinner/system
          examples/dial-me-spinner/e3/home examples/dial-me-spinner/e3/step)

# elaborate <bb> <tree> <first-party packages> <label> <case dir> <bb run args...>
elaborate() {
  local bb=$1 tree=$2 fp=$3 out=$4; shift 4
  mkdir -p "$out"
  ( cd "$tree" && BB_FIRST_PARTY_PACKAGES=$fp "$bb" run "$@" --for 10us --elaborate-only --out "$out" \
      > "$out/log" 2>&1 ) || { echo "schema-epoch: FAIL: elaboration failed: $out ($*)"; cat "$out/log"; exit 1; }
}

fail=0 n_ab=0 n_bc=0 renamed=0
check_bc() {
  local name=$1 r
  r=$("$PYTHON" "$ROOT/scripts/schema_epoch_decks.py" "$OUT/b/$name/deck.cir" "$OUT/c/$name/deck.cir") || {
    echo "schema-epoch: FAIL: $name: $r"; fail=1; return; }
  n_bc=$((n_bc + 1))
  echo "  $name: B = C, $r"
  case $r in *renamed*) renamed=$((renamed + 1)); echo "$r" >> "$OUT/renames.txt" ;; esac
}
: > "$OUT/renames.txt"

for bb_ in "${BOARDS[@]}"; do
  b=${bb_%%=*} bn=${bb_#*=}
  name=$(echo "$b" | tr '/' '_')
  # A board file's run needs a firmware file, a target and a probe; an
  # elaboration reads none of them (it starts no engine).
  cpu=$(grep -m1 -E '^\*bb part [^ ]+ (package=(st|nordic)/|engine=renode)' "$ROOT/$bn" | awk '{print $3}')
  fw=(--firmware "$bn" --target "$cpu" --probe net:ELAB)
  elaborate "$BB" "$WORK/old" "$WORK/old/packages" "$OUT/b/$name" --netlist "$b" "${fw[@]}"
  elaborate "$BB" "$ROOT" "$ROOT/packages" "$OUT/c/$name" --netlist "$bn" "${fw[@]}"
  check_bc "$name"
done
for p in "${PROJECTS[@]}"; do
  # A committed project without [targets] (examples/adc/kicad, examples/bench:
  # their gates record the firmware in a copy with bb flash) is elaborated from
  # self-contained copies of it, with the gate's own target and image recorded
  # the same way in both trees (the image's hash is run.json's, not the deck's).
  ptree_old=$WORK/old ptree_new=$ROOT pp=$p
  if ! grep -q '^\[targets' "$ROOT/$p/bb.toml"; then
    case $p in
      examples/adc/kicad) tgt=U1 elf=$ROOT/examples/adc/firmware/build/adc_board.elf ;;
      examples/bench) tgt=U1 elf=$ROOT/examples/feedback/firmware/build/feedback.elf ;;
      # The system's elaboration needs E1's bindings added (make dms-system
      # does that and compares the system's deck with E1's): not repeated.
      examples/dial-me-spinner/system)
        echo "  $p: not elaborated here (no [targets]; make dms-system elaborates it with E1's bindings)"
        continue ;;
      *) echo "schema-epoch: FAIL: $p has no [targets] and no recipe here"; exit 1 ;;
    esac
    [[ -f $elf ]] || { echo "schema-epoch: FAIL: $elf missing (make firmware)"; exit 1; }
    for side in old new; do
      src=$WORK/old/$p; [[ $side == new ]] && src=$ROOT/$p
      dst=$WORK/copy-$side/$p
      rm -rf "$dst" && mkdir -p "$(dirname "$dst")" && cp -r "$src" "$dst"
      cp "$elf" "$dst/fw.elf"
      # examples/bench's playback-long reads a generated (uncommitted) CSV.
      if [[ $p == examples/bench ]]; then
        "$PYTHON" "$ROOT/examples/bench/make_data.py" --long "$dst/bench" > /dev/null
      fi
      BB_FIRST_PARTY_PACKAGES=$([[ $side == old ]] && echo "$WORK/old/packages" || echo "$ROOT/packages") \
        "$BB" flash --project "$dst" --target "$tgt" "$dst/fw.elf" > /dev/null
    done
    ptree_old=$WORK/copy-old ptree_new=$WORK/copy-new
  fi
  benches=()
  for f in "$ROOT/$p"/bench/*.toml "$ROOT/$p"/bench.toml; do
    [[ -f $f ]] && benches+=("${f#"$ROOT/$p/"}")
  done
  # A project without bench files is elaborated without one.
  [[ ${#benches[@]} -gt 0 ]] || benches=("")
  for bn in "${benches[@]}"; do
    name=$(echo "$p${bn:+_$bn}" | tr '/' '_')
    args=(--project "$pp")
    [[ -n $bn ]] && args+=(--bench "$pp/$bn")
    elaborate "$OLD_BB" "$ptree_old" "$WORK/old/packages" "$OUT/a/$name" "${args[@]}"
    elaborate "$BB" "$ptree_old" "$WORK/old/packages" "$OUT/b/$name" "${args[@]}"
    elaborate "$BB" "$ptree_new" "$ROOT/packages" "$OUT/c/$name" "${args[@]}"
    if cmp -s "$OUT/a/$name/deck.cir" "$OUT/b/$name/deck.cir"; then
      n_ab=$((n_ab + 1)); echo "  $name: A = B byte for byte"
    else
      echo "schema-epoch: FAIL: $name: the base bb and this bb build different decks from the base tree"; fail=1
    fi
    check_bc "$name"
  done
done

# Not vacuous: the renamed views are in the decks compared. Every renamed
# view some committed deck uses must have been compared renamed; a view no
# committed deck uses (generic/inductor) is qualified by its own vectors
# (make packages: bb pkg test builds its deck from the renamed view).
for old in bb_generic_resistor bb_generic_capacitor bb_generic_inductor NRF_SAADC_SE DRV8837_BRIDGE \
           drv8837_bridge_body TPS22917_LSW_FIT LSW_WS2812C; do
  n=$(grep -cw "$old" "$OUT/renames.txt" || true)
  in_base=$( { grep -rliw --include=deck.cir "$old" "$OUT/b" || true; } | wc -l)
  echo "  rename $old: compared in $n decks; in $in_base base decks"
  if [[ $in_base -gt 0 && $n -eq 0 ]]; then
    echo "schema-epoch: FAIL: $old is in a base deck but no comparison renamed it"; fail=1
  fi
done
if [[ $renamed -eq 0 ]]; then
  echo "schema-epoch: FAIL: no deck carried a renamed view (the gate would be vacuous)"; fail=1
fi
# Negative controls of the comparison: one changed value inside a renamed
# view's definition, and one changed element line outside the definitions,
# each must be told apart.
ctl=examples_dial-me-spinner_bench_cold.toml
mkdir -p "$OUT/ctl"
# def: a 9 before the first digit of the line after `.subckt TI_TPS22917_LSW_FIT`;
# body: the same on the first element line outside every .subckt.
awk 'f == 1 && /[0-9]/ { sub(/[0-9]/, "9&"); f = 2 } /^\.subckt TI_TPS22917_LSW_FIT / { f = 1 } { print }' \
  "$OUT/c/$ctl/deck.cir" > "$OUT/ctl/def.cir"
awk 'tolower($1) == ".subckt" { d++ } d == 0 && !done && /^[RCXBVrcxbv]/ && /[0-9]/ { sub(/[0-9]/, "9&"); done = 1 }
     tolower($1) == ".ends" { d-- } { print }' "$OUT/c/$ctl/deck.cir" > "$OUT/ctl/body.cir"
for c in def body; do
  if cmp -s "$OUT/ctl/$c.cir" "$OUT/c/$ctl/deck.cir"; then
    echo "schema-epoch: FAIL: negative control $c changed nothing"; fail=1
  elif "$PYTHON" "$ROOT/scripts/schema_epoch_decks.py" "$OUT/b/$ctl/deck.cir" "$OUT/ctl/$c.cir" > "$OUT/ctl/$c.txt"; then
    echo "schema-epoch: FAIL: negative control $c passed: $(cat "$OUT/ctl/$c.txt")"; fail=1
  else
    echo "  negative control $c: told apart ($(cat "$OUT/ctl/$c.txt"))"
  fi
done
echo "schema-epoch: $n_ab project decks A = B, $n_bc decks B = C ($renamed of them with renamed views)"
[[ $fail -eq 0 ]] || { echo "schema-epoch: FAIL"; exit 1; }
echo "schema-epoch: PASS"
