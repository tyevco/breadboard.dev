# Canonical JSON

Status: normative. Decided by ADR-007 D6 (accepted 2026-11-09, approver
decision A2; implemented in Phase 1 M4, ADR-007 Amendment 1).
Implementation: `bbcore::cjson`. Used by `board.bbm` (`spec/board-bbm.md`),
`run.json` and `.bb/resolution.json`. **Changing this file requires an
ADR.**

Breadboard's own canonical form. It is not RFC 8785 (JCS): JCS requires
numbers to be IEEE 754 doubles, and Breadboard's integer sub-units exceed
2^53 (2^53 fs is about 9.007 s, so a 10 s run would already be out of
range). Everything else follows JCS.

## 1. Form

- UTF-8, no byte-order mark.
- No insignificant whitespace. A document is one line followed by exactly
  one `\n`.
- Object keys are sorted by their UTF-16 code units (JCS §3.2.3). A key
  appears at most once.
- Strings are escaped as JCS does: `"` and `\` with a backslash; the
  control characters U+0000 to U+001F as `\b`, `\t`, `\n`, `\f`, `\r` or
  `\u00xx` (lower-case hex); every other character, `/` and non-ASCII
  included, as itself.
- `true`, `false` and `null` as themselves.
- **Numbers are integers only**, written as arbitrary-precision decimal
  literals: an optional `-`, no leading zeros, no fraction, no exponent,
  never `-0`.

## 2. Reading

- A reader parses numbers as `i128` (or big integers). A value outside
  `i128` is an error.
- A fraction or an exponent is an error: there are **no floats** in
  `board.bbm`, `run.json` or `.bb/resolution.json`. A quantity is an
  integer in a fixed sub-unit (`spec/board-bbm.md` section 3); an
  enumerated judgement (a view's `confidence`) is a string.
- A duplicate key is an error.
- A reader may accept whitespace between tokens and keys in any order;
  writing the value back gives the canonical text.

## 3. Arrays

Array order is part of the value. Each format that uses canonical JSON
says how its arrays are sorted (`board.bbm`: bytewise UTF-8 order of a
stated key, `spec/board-bbm.md` section 2). Comparisons are bytewise on
UTF-8, which is deterministic but not "natural" (`R10` sorts before `R2`).

## 4. Gate

ADR-007 M4g (`bbcore::cjson` unit tests): a time of 2^63 fs and a negative
value round-trip exactly; a reader refuses `1.5`, `1e3`, `01`, `-0`, a
duplicate key and a number outside `i128`; keys sort by UTF-16 code units
(U+FB01 after U+1F600) and strings escape as JCS does.
