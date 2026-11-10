# Fidelity and honesty

A simulation is only as good as its models, and a reader of a result needs to
know which models produced it. Breadboard records that in the package and in
every run instead of leaving it implicit.

## Provenance and confidence

Every analog view in a [component package](../reference/packages) carries a
provenance block (`spec/package.md` section 2):

- **`source`**: how the view was made: `datasheet`, `vendor_model`,
  `vendor_derived`, `measured`, `estimated` or `agent_generated`.
- **`confidence`**, an enum rather than a number:

  | Value | Meaning |
  | --- | --- |
  | `unchecked` | Not checked. |
  | `self_checked` | Passes its own test vectors. |
  | `datasheet_checked` | The vectors are datasheet table values. |
  | `measured` | The vectors are bench measurements. |

- Optional fields name the basis and the gaps: `base`, `refs`,
  `deviations`, `not_modelled`, `origin` (the source of a migrated file),
  `generated_by` and `reviewed_by`.

`source` says how the view was made, not where its numbers come from. A view
an agent wrote is `agent_generated` even when every number in it is a
datasheet value, and it stays flagged until a person reviews it
(`reviewed_by`). A figure taken from a secondary source names that source in
`refs`; `datasheet_checked` needs the vectors checked against the datasheet
itself (ADR-006 Amendment 5).

## What a run reports

`bb run` on a project writes `run.json`, which lists every analog view in the
run with its package, version, view, the requested fidelity and the rung it
was chosen at, its provenance `source` and `confidence`, its qualifications
and whether one covers the run. It also lists the instances that were out of
the bench's analog scope or not simulated, with the reason.

**`bb run` flags every view whose provenance is `estimated` or
`agent_generated` in its summary.** In the E1 demo that is the ideal R and C,
and the TPS22917's datasheet-fitted view.

## No silent defaults

- **The fidelity ladder** is `fast < pi < si < em`. Phase 1 runs `fast` only.
  An instance uses the view its project override names, else the view that is
  the default for the requested rung, else the default of the first lower
  rung. Otherwise it has **no analog model**: it is listed as unmodelled,
  never given a silent default. A view never moves up the ladder.
- **Lattice qualification.** A view is qualified for a (lattice, option set)
  pair by running its test vectors through the real ngspice engine
  (`bb pkg test --record`). `bb run` refuses a bench whose option set some
  in-scope view is not qualified for, unless `--allow-unqualified` names that
  view; the exception is recorded in `run.json`.
- **Unresolved parts are visible.** `bb open` prints the resolution counts
  and the unresolved instances. A part with no package needs an explicit
  decision in `bb.toml` (for example `package = "none"` with a reason).
- **Values are never guessed.** A KiCad value that does not parse is an
  error, and a bare `M` (mega or milli?) is an error that suggests `Meg` or
  `m`.

## Local-only models stay local

A view derived from a vendor model whose redistribution terms were not checked
lives only in a package with `license = "LicenseRef-local-only"`, outside the
committed tree (Phase 1 approver decision A3). `make packages` fails if one is
tracked. For the TPS22917 the committed default is therefore a
datasheet-fitted view, gated locally against the vendor-derived one on the E1
checks.

## What the bench does not model

Benches state their own limits. The E1 cold bench, for example, says in its
comments that it does not reproduce the BQ25180 power-path dynamics, the
base-board VSYS capacitance, tolerances, DC bias or Monte Carlo corners.
Higher fidelity rungs (`pi`, `si`, `em`) and rail-tracking pin outputs are
[Phase 2 roadmap items](../guide/roadmap), not features.
