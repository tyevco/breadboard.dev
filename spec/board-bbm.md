# The elaborated board (`board.bbm`, `bb-board/0`)

Status: normative. Decided by ADR-007 D3, D4 and D6 (accepted 2026-11-09;
implemented in Phase 1 M4, ADR-007 Amendment 1). Implementation:
`bbcore::board` (types, reading and writing), `importers/kicad` (the
netlist reader and the version gates) and `cli/src/import.rs` (resolution).
JSON Schema: `spec/schema/board-bbm.schema.json`. **Changing this file
requires an ADR.**

`board.bbm` is what `bb open` writes next to `bb.toml`, and what CI diffs
when a schematic changes. It is [canonical JSON](canonical-json.md).

## 1. What it is a function of

`board.bbm` is a pure function of:

- the **content** of each board's KiCad netlist that section 5 lists
  (never where it came from: a committed `.net` and `kicad-cli` on the
  schematics, KiCad 9 and 10, Linux and Windows `kicad-cli` give the same
  bytes);
- `bb.toml`'s `[[board]]` ids and grounds, `[instances]` overrides,
  `[packages]` search path, `[import] fields` and `[[connection]]` (not
  whether a board is `netlist =` or `kicad =`);
- the packages found on the search path, with their versions and tree
  hashes, and the content hashes of their platforms.

Source paths, their sha256, schematic versions, the `kicad-cli` and
netlist tool strings and the host are in `.bb/import.json`, which is not
canonical and not compared. Firmware, the bench and run options are in
`run.json` (written by `bb run`).

## 2. Top level

| Key | Type | Meaning |
| --- | --- | --- |
| `schema` | string | `bb-board/0` |
| `boards` | array | `{id, ground}` per `[[board]]`, sorted by `id`. `ground` is the ground net's display name |
| `nets` | array | every net of every board, sorted by `name` (section 4) |
| `instances` | array | every component, sorted by `board` then `ref` (section 3) |
| `platforms` | array | `{id, content}` of every engine platform a resolved package names (`renode:stm32f072-bb@1`), sorted by `id` |
| `probes` | array | `{alias, net}`: test points as probe aliases, sorted by `alias` |
| `connections` | array, optional | `{a, b, model?, pins?}` per `[[connection]]` (systems; resolved at M11) |
| `spice_body` | string, optional | a hand-written board's verbatim SPICE (ADR-007 D9); an imported board has none |

All sorting is bytewise on UTF-8.

## 3. Instances

| Key | Type | Meaning |
| --- | --- | --- |
| `board`, `ref` | string | the board id and the reference designator |
| `value_text` | string | the KiCad value field as written |
| `value` | object, optional | a generic passive's parsed value: exactly one of `ohm_u` (µΩ), `farad_a` (aF), `henry_f` (fH), plus optional `tol_ppm`, `rating_uv`, `power_uw`, `dielectric`, `technology` (ADR-006 D5 and Amendment 1 item 1) |
| `footprint` | string | the footprint id (`Resistor_SMD:R_0402_1005Metric`) |
| `lib`, `part` | string | the symbol's `libsource` |
| `sheet` | string | the sheet path names (`/MCU, flash & debug/`) |
| `fields` | object, optional | the fields read (`MPN`, `Manufacturer`, `LCSC`, `bb:part`, and `[import] fields`) that are present and not empty or `~` |
| `flags` | array, optional | the set flags among `dnp`, `exclude_from_bom`, `exclude_from_sim`, sorted |
| `package` | object, optional | `{id, version, tree}` of the resolved package |
| `resolution` | object | `{stage, key?}`: the stage that resolved it (section 6) and what matched |
| `status` | string | `ok` (has a package), `no_model` (needs none), `excluded` (an override `package = "none"`, `dnp` or `exclude_from_sim`), `unmodelled` (unresolved) |
| `reason` | string, optional | why it is excluded |
| `pins` | array | `{number, name, net?}`, sorted by `number`: every pin of the symbol's `libpart` (`~` read as an empty name) and the net it is on |

Quantities are integers in the units of ADR-007 D6: voltage µV, time fs,
resistance µΩ, capacitance aF, inductance fH, current pA, temperature
m°C, power µW, ratios ppm.

## 4. Nets and their names

| Key | Type | Meaning |
| --- | --- | --- |
| `name` | string | the **display name**: the KiCad net name; in a project with more than one `[[board]]`, qualified `<board>:<name>` |
| `board` | string | the board id |
| `spice` | string | the SPICE node name (below) |
| `nodes` | array | `{ref, pin}` (the pin **number**), sorted by `ref` then `pin` |
| `unconnected` | `true`, optional | a KiCad `unconnected-(…)` net: probe-only, never bound |

**SPICE names**, a pure function of the net names:

1. every character other than `[A-Za-z0-9_]` becomes `_`; runs of `_`
   collapse to one; a leading `_` is dropped (`/LED ring/LED_DIN_R` →
   `LED_ring_LED_DIN_R`);
2. in a project with more than one board, the result is prefixed with
   `<board>_`;
3. an empty result is `n`; a leading digit gets an `n` prefix (`+3V3` →
   `n3V3`);
4. a name equal to `gnd` in any case gets `_n` appended (ngspice treats
   `gnd` as ground);
5. the simulated board's ground net is `0`;
6. names that collide case-insensitively (ngspice compares node names
   without case) get `_2`, `_3`, … in display-name order; the first keeps
   its name.

Phase 0 names are unchanged by these rules (`LED_A`, `PA5`, `ADC_IN`).
Glue element names keep `spec/pin-model.md` glue rule 3 over pin names
(`U10.P0.06` → `V_bb_U10_P0_06`).

## 5. What the importer reads (ADR-007 D3)

From a KiCad netlist (`(export (version "E") …)`):

- per component: `ref`, `value`, `footprint`, `libsource` (`lib`, `part`),
  `sheetpath` `names`, the fields by name from `(fields (field (name N)
  V))`, and the flags from `(property (name "dnp"))`,
  `exclude_from_bom`, `exclude_from_sim`;
- per net: `name` and its `(node (ref R) (pin N))` entries;
- per `libpart`: its pins' `num`, `name` and `type` (the type for lint
  only).

Nothing else is read: the `date`, `units` blocks, the datasheet, net
classes and `pinfunction` (KiCad 10 writes `name_number`) cannot reach
`board.bbm`. Line endings and token order do not matter.

**Version gates** (refused with an error that names the file):

- the netlist format must be `"E"`; the `(design (tool "Eeschema
  <major>.…"))` major must be 9 or 10; 6 to 8 are accepted with an
  "untested" warning; anything newer or older is refused;
- each schematic of the `kicad =` route (the root and every sheet it
  reaches) must have `(version V)` with 20250114 ≤ V ≤ 20260306, in any
  mix; KiCad 6-8 versions warn; newer versions are refused;
- refused in Phase 1: a pin number containing `[` (stacked pins), a root
  schematic that defines variants, a symbol with jumper pin groups.

**Rules on the content:**

- **Pin names:** for an instance resolved to a package whose kind is not
  `passive`, every pin's name from the netlist must equal the package's
  `pins.toml` name for that number, and every pin number must be in
  `pins.toml`.
- **Ground:** the net named by `[[board]] ground` (default `GND`) must
  exist.
- **References** are unique within a board.

## 6. Resolution stages (ADR-007 D4)

Per instance, the first stage that yields a package wins:

| `stage` | Rule |
| --- | --- |
| `override` | `[instances."<ref>"] package = "<id>[@<version>]"` in `bb.toml`, or `package = "none"` with a `reason` (status `excluded`) |
| `bb_part` | the symbol's `bb:part` field |
| `mpn` | the normalised `MPN` field (trimmed, one trailing parenthesised comment dropped) equals an entry of a package's `mpn`, or fully matches one of its `mpn_patterns`; two packages matching is an error |
| `lcsc` | the `LCSC` field equals an entry of a package's `lcsc`; two is an error |
| `generic` | `generic/resistor`, `generic/capacitor` or `generic/inductor` when the footprint matches one of the package's `[footprints] kicad` globs, or (a footprint no generic package names) the reference is `R`, `C` or `L` followed by digits; the value must parse (ADR-006 D5) |
| `no_model` | footprint library `MountingHole`, `Fiducial` or `TestPoint` (status `no_model`; a test point becomes a probe alias of the net on its pin 1) |
| `unresolved` | none of the above: status `unmodelled`, its nets still exist |

`dnp` and `exclude_from_sim` keep the stage's package and set status
`excluded` with a reason.

`.bb/resolution.json` (canonical JSON, `bb-resolution/0`) lists per
instance the stage reached, every stage tried with its key and result, the
normalised MPN and, for an agent to look at, packages whose MPNs share a
prefix with it (`candidates`; never chosen). `.bb/unresolved.toml` holds a
commented `[instances."<board>:<ref>"]` stub per unmodelled instance. Both
are rewritten whole by every `bb open` and are byte-identical across runs.
