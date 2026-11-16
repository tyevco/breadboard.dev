# ADR-012: The package-generation pipeline, and the provenance and review policy

- **Status:** Accepted (2026-11-15; see "Approver decisions
  (2026-11-15)"). Proposed 2026-11-14, revised after design review round 1
  (see "Review notes"). Amendment 1: the `part.toml` keys landed in
  milestone S.
- **Part of:** the Phase 2 plan, milestones E2 and E3. Its `part.toml`
  keys (`[[document]]`, `claims`, `upstream_license`) land in milestone S,
  the schema epoch.
- **Normative changes when implemented:**
  - `spec/package.md` (provenance keys, `[[document]]`, `claims.toml`,
    confidence rules, lint);
  - a new `spec/claims.md`;
  - `spec/schema/claims.schema.json`;
  - `spec/registry.md` (ADR-011: the provenance gate and the index's
    `reviewed_by`).
- **Evidence:**
  - the ecosystem research pass (`~/bbscratch-p2-eco`):
    - KiCad 10.0.8's library counted by script;
    - GitHub code search for `lib_id` frequencies;
    - the JLCPCB parts dataset;
    - the MPN fields of dial-me-spinner and caliper;
    - a seeded fetch of 60 KiCad datasheet URLs;
  - ADR-006 Amendment 5: the TPS22917 view cited SLVSDW8B, which was
    never opened; its figures came from the suite's headers.

## Summary

- An agent may write a package, but it may not decide how far the package
  is trusted.
- Every number in a view or a vector is a **claim** that cites a hashed
  document. A printed value cites a page and a verbatim quote; a value
  read from a figure cites the figure and its digitised points; a fitted
  value cites its inputs and its fit. A **mechanical checker**, not an
  agent, verifies what can be verified mechanically.
- `confidence = datasheet_checked` requires every claim to pass, and
  figure and fit claims also need a reviewer's sign-off.
- Agent PRs are authored by a **pipeline bot identity**; `reviewed_by`
  comes only from a person's approval of such a PR, recorded by the
  registry's CI.
- Claim checks that need a datasheet run on the maintainer's machine and
  are committed as a **signed result** that CI verifies. No self-hosted
  runner ever runs registry PRs.
- Parts are chosen by **MPN usage on real boards**, not by KiCad's library,
  which carries almost no MPNs.

## Context

**KiCad's library cannot rank parts.** KiCad 10.0.8 has 22 911 symbols,
22 808 of them not power symbols (counted from the installed libraries;
Windows and WSL agree). Of those 22 808:

- **7 (0.03 %)** have an MPN or Manufacturer field;
- 397 (1.7 %) have any `Sim.*` field, and only 29 have `Sim.Params`;
- 20 066 (88 %) have a datasheet URL;
- 18 206 (80 %) have a footprint.

**The user's boards keep their ICs in custom symbols.**

| Board | Instances | Unique MPNs | lib_ids | Custom lib_ids |
| --- | --- | --- | --- | --- |
| dial-me-spinner | 289 | 67 | 41 | 10, exactly the ICs |
| caliper | 141 | 46 | — | 7 IC symbols |

Both boards carry `MPN`, `Manufacturer` and `LCSC` fields. Together they
have 93 unique MPNs, about 60 of them plain R/C.

**Usage proxies:**

- GitHub code search of `.kicad_sch` files. It counts files and is
  approximate:

  | lib_id | Files |
  | --- | --- |
  | `Device:R` | 67 584 |
  | `Device:LED` | 30 208 |
  | `Regulator_Linear:AMS1117-3.3` | 2 748 |
  | `RF_Module:ESP32-S3-WROOM-1` | 1 070 |
  | `MCU_RaspberryPi:RP2040` | 968 |
  | `MCU_ST_STM32F1:STM32F103C8Tx` | 611 |

  Code search allows 10 requests a minute and 1 000 results per query.
- The JLCPCB basic and preferred list (CDFER/jlcpcb-parts-database,
  snapshot 2026-10-28): 1 577 rows, 87 % passives, diodes or protection;
  only 42 basic ICs or transistors.

**Datasheets often cannot be fetched automatically.** A seeded sample of
60 of KiCad's 8 188 distinct datasheet URLs:

- 25 fetched;
- 20 timed out (st.com 15/15, analog.com 4/4);
- 7 returned 404 (4 of them TI);
- 4 returned 403.

**Phase 1's failure, to design against.** The TPS22917 view said `source
= "datasheet"`, and its `refs` named SLVSDW8B. The figures came from the
suite's headers, and nobody opened the datasheet. ADR-006 Amendment 5 then
made `source` mean "how it was made", and DRV8837 followed the corrected
pattern. Its `refs` still name a secondary source. No first-party view has
reached `datasheet_checked` through a check against the datasheet itself.

## Decision

### D1. Tiers

| Tier | Content | Who may publish |
| --- | --- | --- |
| T0 | pins only (`pins.toml`, footprints, MPNs); generic analog through generic packages | pipeline + review |
| T1 | simple analog views: LDO, load switch, op-amp macro, passive with ESR, diode | pipeline + review, with vectors from typical-value tables |
| T2 | behavioural devices (code) | blocked: registry packages are data-only (ADR-011 D9) |
| T3 | Renode platforms | first-party only |

### D2. Choosing the parts

The rank is at the **MPN level**, collapsed to package granularity: one
generic R/C/L/LED package covers every value. It merges three lists:

- (a) the MPNs on dial-me-spinner, caliper and every external board in the
  Phase 2 list;
- (b) the JLCPCB basic and preferred ICs and discretes;
- (c) the top `lib_id`s by GitHub code search, mapped to MPNs where the
  symbol names one.

Parts that `bb` cannot use at `fast` stay T0. A pins-only package still
resolves an instance and lets the importer bind it.

**The target is coverage, not raw count.** At least 90 % of the distinct
MPNs on the reference and external boards must resolve.

### D3. Documents

New `[[document]]` entries in `part.toml`:

```toml
[[document]]
id = "slvsdw8b"
title = "TPS22917 datasheet"
revision = "B"
url = "https://www.ti.com/lit/ds/symlink/tps22917.pdf"   # where it was fetched; informative
sha256 = "…"                       # of the PDF the claims were checked against: the identity
fetched_by = "agent:…" | "user:…"  # a person may supply the PDF (st.com, analog.com time out)
```

- The PDF is **never** committed or published.
- It lives in a private document cache, `~/.cache/bb/docs/<sha256>.pdf`.
- **The sha256 is the document's identity, not the URL.** Some vendor URLs
  float to the latest revision (TI's `/lit/ds/symlink/` path is named for
  it), so a later fetch may give another file. Lint warns on a URL known
  to float and asks for the `revision`; a reviewer who fetches a different
  sha256 has a different document, and the claims are not checked
  against it.
- **Who runs the check** (D10): the maintainer, locally, with the PDF in
  the cache. The registry CI never fetches datasheets and never runs on a
  self-hosted runner.
- An unchecked document keeps the package at `self_checked`.

### D4. Claims

`claims.toml` lists every number that a view parameter, a pin parameter
or a test vector uses:

```toml
schema = "bb-claims/0"
[[claim]]
key = "analog.lsw.params.ron"          # what it feeds
value = "22mOhm"
doc = "slvsdw8b"
page = 6
locator = "Table 6.5, RON, VIN = 3.6 V, TA = 25 C, TYP"
quote = "VIN = 3.6 V 22 30 mΩ"         # verbatim, as extracted
kind = "printed"                       # the default

[[claim]]
key = "analog.buck.params.eta_table"   # an η(Vin, Iout) table (ADR-009 D2 rule 3)
kind = "figure"
doc = "slvsfa2"                        # illustrative: the ids, page and figure here are examples
page = 12
figure = "Figure 7-3, Efficiency vs Output Current, VOUT = 3.3 V"
axes = { x = "Iout", x_unit = "A", x_scale = "log", y = "eta", y_unit = "%" }
calibration = [["0.001", "50"], ["1", "100"]]   # two axis ticks per axis, as printed (quoted from the page text)
points = "claims/eta_vin3v6.csv"       # the digitised points, canonical CSV
points_sha256 = "…"
digitiser = "manual"                   # or a tool name and version

[[claim]]
key = "analog.lsw.params.ron_tc"
kind = "fit"
inputs = ["analog.lsw.params.ron", "fig_ron_vs_temp"]   # other claims
script = "claims/fit_ron_tc.py"        # deterministic; its sha256 recorded
script_sha256 = "…"
value = "3.1m/K"
residual = { metric = "max_abs", value = "0.4mOhm", bound = "1mOhm" }
```

- **`printed`**: a number printed in the document's text (D5 checks it
  exactly).
- **`figure`**: values read off a plot. The checker cannot read a plot; it
  checks what is checkable: the cited page's text contains the figure's
  caption and the quoted axis ticks, the points file has the recorded
  sha256, and the view's table equals the points file. The review sheet
  renders the digitised points over the cropped page image, and the
  reviewer signs off on the digitisation.
- **`fit`**: a parameter fitted to other claims. The checker re-runs the
  recorded script on the recorded inputs and requires the same value and
  residual to the base unit, and the residual within its bound. Whether
  the fitted form is right is the reviewer's call.

### D5. The mechanical claim check

`bb pkg claims check` is deterministic code, not an agent:

1. Take the document with that sha256, and extract its text with
   `pdftotext -layout` from a pinned poppler (run as a tool; its GPL does
   not reach `bb`). The extracted text's sha256 is recorded, so a poppler
   change shows up.
2. Normalise whitespace and Unicode. The quote must occur **on the cited
   page**.
3. The claimed value, in the value grammar, must equal a number in the
   quote after unit conversion. "Equal" means the same integer in the
   grammar's base unit: no tolerance.
4. For `figure` and `fit` claims, the checks of D4 apply instead of steps
   2-3.
5. Any failure marks the claim `unverified`, with the reason.
6. **The result** is a canonical JSON file, `claims.result.json`: the
   package tree hash (without the result file), each claim's outcome and
   reason, the document and extracted-text sha256s, and the poppler
   version. The maintainer signs it with their minisign key (D10).

**Its limit, stated plainly:** the check proves the number is printed on
that page next to that text, that a figure's points are the ones
reviewed, and that a fit reproduces. It does not prove the number was the
right row, condition or corner, or that a digitisation is faithful. That
is the reviewer's job (D7).

### D6. Pipeline steps

1. **Select:** the next MPN from the rank (D2).
2. **Acquire:** fetch or accept a supplied PDF; record the `[[document]]`.
3. **Extract:** produce the page-numbered text (D5 step 1).
4. **Draft (agent):**
   - `part.toml`, `pins.toml` and `claims.toml`;
   - vectors from the typical-value tables;
   - `source = "agent_generated"` and `generated_by` set **by the
     pipeline**.
5. **Check (mechanical, before the PR):**
   - the claim check, as a dry run on the pipeline host's copy of the
     document (the maintainer's signed run in step 7 is the one that
     counts);
   - pins against the KiCad symbol where one exists (0 mismatches) and
     against the datasheet's pin table;
   - `bb pkg lint`;
   - `bb pkg test` with up to N = 3 repair rounds by the agent.
6. **Stage:** a PR to the registry's staging channel, **authored by the
   pipeline bot identity** (D10), with a generated review sheet listing:
   - every claim, its page and its quote; figure claims with their points
     drawn over the page image;
   - the vectors;
   - the lint and test results;
   - the KiCad diff.
7. **Check (maintainer):** the maintainer runs `bb pkg claims check` with
   the documents in their cache and pushes the signed
   `claims.result.json` to the PR.
8. **Review:** a person checks the rows, conditions and digitisations D5
   cannot check. Their approval of the bot's PR moves the package to the
   public index.

### D7. Confidence and review rules

These are enforced by lint and by the registry CI, never trusted from the
package file:

- **`source` is set by the pipeline.** A package with `generated_by`, or a
  PR authored by the pipeline bot identity, is `agent_generated`. Agents
  in this project commit as the user (with a co-author trailer), so the
  author of a commit is not evidence; the bot identity is.
- **`datasheet_checked` requires** every `refs` entry to resolve to a
  `[[document]]` with a sha256, every claim to pass D5 in a signed result
  whose tree hash matches the package, and every `figure` or `fit` claim
  to have the approving reviewer's sign-off. Without that, a view built
  from figures or fits (every averaged `pi` converter, ADR-009 D2) tops
  out at `self_checked` plus `reviewed_by`, and the plan says so.
- **A ref to a secondary source caps confidence at `self_checked`.** A
  secondary source is a header, a suite file or an application note that
  quotes the datasheet.
- **`measured`** requires a measurement record: an instrument, a date and
  a raw-data sha256.
- **`reviewed_by`** is written into the **index** by CI from the PR's
  approval (login and review id), only when the approver is a person
  listed in `owners.toml` and is not the PR's author. An approval by the
  bot identity is refused. A `reviewed_by` in `part.toml` is ignored and
  gives a lint warning.
- **What `bb run` flags in its summary and in JUnit properties:**
  - `agent_generated`;
  - **unreviewed** (no index `reviewed_by`, or a path package);
  - **N unverified claims**.

### D8. Applying the rules to the first-party packages

E2 re-runs the claim check on every first-party view. Where a view's
`refs` point at a secondary source, its confidence is lowered to
`self_checked` in the same commit. That is a provenance-only patch bump
(`spec/package.md` §10). This also corrects ADR-006 D3's statement about
the TPS22917 `pi` view (ADR-009).

### D9. Review budget

- **Estimate:** 15-30 minutes per T0 package and 1-3 hours per T1 view,
  so 50-150 hours for the exit's 50 reviews. This is an estimate, not a
  measurement.
- **The queue is ordered by coverage gain:** the MPN that unblocks the
  most boards first.
- **Pilot measurement:** E2's 10 pilot packages record actual review
  minutes. The exit target is re-planned from them.

### D10. Identities and CI security

- **The pipeline bot identity.** A dedicated GitHub account (or a GitHub
  App) authors every agent PR to the registry. It is distinct from every
  reviewer, and it has no approval rights in `owners.toml`. GitHub does
  not let a PR's author approve it, so this is also what makes the user's
  approval possible: the user reviews the bot's PRs.
- **The claim result is signed by the maintainer.** Each maintainer has
  a minisign key listed in `owners.toml`. The registry CI verifies the
  signature of `claims.result.json` and that its tree hash matches the
  package; it never needs the PDF. A changed package invalidates the
  result.
- **No self-hosted runner on the registry.** A public repository's PRs
  are untrusted code; a self-hosted runner would execute them on a
  maintainer's machine. Registry CI runs only on GitHub-hosted runners,
  and the document-dependent step is the maintainer's local command
  above, which runs on files the maintainer chose to fetch.
- One maintainer today: the user. Adding a reviewer is a PR to
  `owners.toml`, approved by the user.

## Exact `spec/` changes (described, not applied)

- **`spec/package.md`:**
  - §2 adds `[[document]]`, the `claims` file key and `upstream_license`;
  - the provenance paragraph gains D7's rules;
  - §11 lint gains the claim and confidence rules.
- **New `spec/claims.md`:** the `bb-claims/0` schema with the `printed`,
  `figure` and `fit` kinds, the normalisation and equality rules, the
  extractor pin, `bb pkg claims check`, and `claims.result.json` with its
  signature.
- **`spec/registry.md` (ADR-011):** the provenance gate; how the index
  records `reviewed_by` and `unverified_claims`; the bot identity and the
  maintainers' keys in `owners.toml`; hosted runners only.

## Determinism impact

- None on runs.
- The claim check is deterministic given the document's sha256 and the
  pinned extractor.
- `run.json` gains each view's review status and unverified-claim count
  as recorded inputs.

## Gates

| # | Gate |
| --- | --- |
| G1 | Negative controls: a changed number, a wrong page, an absent quote, a wrong document hash and a unit error each give `unverified` with the right reason |
| G2 | A package with one `unverified` claim cannot be published as `datasheet_checked`; one with a secondary-source ref is capped at `self_checked` |
| G3 | `reviewed_by` in a package file is ignored; the index's comes only from an approval (a CI test with a fake approval event) |
| G4 | 10 pilot packages from dial-me-spinner and caliper MPNs pass through the pipeline; review minutes recorded |
| G5 | The claim check on the same PDF gives byte-identical results twice and on a second host |
| G6 | Figure and fit claims: a changed digitised point, a changed points sha256, a fit script that gives a different value and a residual over its bound each give `unverified`; a figure claim without a reviewer sign-off caps the view at `self_checked` |
| G7 | Identities: an approval by the bot identity, or by the PR's author, writes no `reviewed_by`; a `claims.result.json` with a bad signature or a stale tree hash is refused by CI; the registry workflow file names only GitHub-hosted runners (a CI lint) |

## Rejected alternatives

- **Ranking from KiCad's library.** 7 MPNs in 22 808 symbols.
- **An agent verifying another agent's claims.** That repeats Phase 1's
  failure mode. The check must be mechanical.
- **Redistributing datasheets.** Vendor terms forbid it.
- **Numeric confidence scores** (the vision's `confidence = 0.7`). Phase 1
  chose an enum (approver decision A7), and a score cannot be checked.
- **Auto-publishing agent packages with a warning.** Unreviewed volume is
  exactly the risk the vision names ("agent-generated packages are subtly
  wrong and users trust them").

## Questions for the approver

Consolidated in the plan's section 9, round 2: **Q10** (the bot identity,
locally run and signed claim checks, no self-hosted runner) and **Q11**
(figure and fit claims, and the confidence ceiling without them).

**Defaults (apply unless the approver objects):**

1. **The confidence rules of D7**, including the cap for secondary
   sources.
2. **A person supplies PDFs** that cannot be fetched (st.com, analog.com),
   with the sha256 recorded and the PDF never committed.
3. **Who reviews:** the approver, plus any maintainer later listed in
   `owners.toml`; an agent or the bot never. The exit stays at 50
   reviewed packages and is re-planned after the pilot.
4. **Lower the first-party confidences** where refs are secondary (D8).

## Approver decisions (2026-11-15)

All twelve questions of the plan's section 9 were accepted with their
recommended answers (plan section 10). For this ADR:

- **Q10, accepted.** Agent PRs are authored by a dedicated pipeline bot
  account and approved by the user; claim checks run on the maintainer's
  machine and are committed as a signed result that CI verifies; no
  self-hosted runner ever runs registry PRs.
- **Q11, accepted.** Values read from datasheet figures or fitted to them
  are figure and fit claims, with digitised points, a fit record and a
  reviewer sign-off. Without them a `pi` view tops out at `self_checked`
  plus `reviewed_by`.
- **Q3, accepted.** The `part.toml` keys (`[[document]]`, `claims`,
  `upstream_license`) land in milestone S, accepted but inert.
- **Build scope.** E2 and E3 are **not** in this build: they need the bot
  account and human review time. Their decisions stand for when they
  start.

**Defaults.** No objection was made to the defaults listed under
"Questions for the approver", so they apply as written.

## Review notes (design review round 1, 2026-11-14)

**Adopted:**

- **The checker could not verify figure-derived or fitted values**
  (major): `figure` and `fit` claim kinds, what the checker verifies for
  each, and the confidence ceiling without them (D4, D5, D7).
- **Review mechanics for one person plus agents** (major): GitHub does
  not let an author approve their own PR, and agents commit as the user.
  A pipeline bot identity now authors agent PRs (D10).
- **The self-hosted runner fallback** (major, security): removed. Claim
  checks run on the maintainer's machine and are committed as a signed
  result that hosted CI verifies.
- **Floating datasheet URLs**: the sha256 is the identity, and lint warns
  on floating URLs (D3).

**Not adopted:** none.

## Amendment 1 (2026-11-15): the keys landed (milestone S)

Milestone S landed this ADR's `part.toml` keys, accepted and inert. ADR-011
Amendment 1 is the record, and `spec/package.md` section 2 states them.

- **`[[document]]`** (D3) takes `id` (lower case `[a-z0-9._-]`, unique),
  `sha256` (64 lower-case hex digits; required: the document's identity),
  and optional `title`, `revision`, `url` and `fetched_by`
  (`agent:<who>` or `user:<who>`).
- **`[claims] file`** (D4) names the package's `claims.toml`. It is a table
  key, as `[pins] file` and `[tests] dir` are. This build checks only that
  the file exists. `bb-claims/0` and its checker are E2's.
- **`upstream_license`** (ADR-011 D7) goes in a provenance block (a view's
  or the package's) and must be an SPDX expression.

None of them changes a deck or an output. D7's rules on `reviewed_by` and
on `refs` resolving to documents are E2's and are not applied here.
