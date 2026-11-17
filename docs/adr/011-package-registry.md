# ADR-011: Package registry, signing and `bb-lock/1`

- **Status:** Accepted (2026-11-15; see "Approver decisions
  (2026-11-15)"). Proposed 2026-11-14, revised after design review round 1
  (see "Review notes").
- **Amendments:** 1 (2026-11-15), milestone S as built: the schema epoch
  (every Phase 2 schema major of ADR-009 to ADR-013), `bb-lock/1`, the
  D11 renames and the one relock. 2 (2026-11-15), milestone E1 as built:
  the client, the archives, the registry's CI tools and repository, the
  local fixtures and gates.
- **Part of:** the Phase 2 plan, milestones E0-E1, then E3 and E5.
  `bb-lock/1` lands in milestone S, the schema epoch, before anything is
  published.
- **Normative changes when implemented:**
  - `spec/package.md` (sections 1, 9, 10 and 11; a new section on
    registries);
  - a new `spec/registry.md` (index, archives, snapshots, keys);
  - `spec/schema/lock.schema.json` (`bb-lock/1`);
  - `spec/project.md` (`[registries]` in `bb.toml`).
- **Evidence:**
  - the ecosystem research pass (`~/bbscratch-p2-eco`): crates.io API
    dependency counts from `cargo generate-lockfile`, the Cargo sparse
    index and Go module references, GitHub Pages, Releases and R2 limits,
    KiCad's library licence, the SVD and IBIS licence terms, and package
    sizes;
  - ADR-006 D8 ("Signing is deferred"), D3 (view-name prefixing deferred
    to the registry ADR) and D11.

## Summary

- The public registry is a **git repository served as static files**:
  - an index of one JSON-lines file per package;
  - deterministic archives addressed by the existing tree hash;
  - a snapshot signed with minisign by CI;
  - GitHub build-provenance attestations.
- Versions are immutable, except for yanking and legal removal.
- `bb-lock/1` records each package's source and the vendor files fetched
  at use.
- Only `bb open` fetches. `bb run` stays offline and read-only.
- Local sources (project paths, then first-party) resolve before any
  registry, so development and `make test` never need the network.
- Private registries route by namespace.
- Registry content is readable at every major it was published at.
- Registry packages are **data only** in Phase 2.

## Context

- Packages today are local directories plus `bb.lock` (`bb-lock/0`:
  `id`, `version`, an informative `path`, and `tree`). The tree hash
  (`spec/package.md` §9) is a dirhash over sorted `path\0sha256` lines and
  does not depend on any archive format.
- **Size:** the 11 first-party packages total 290 KB, mean about 26 KB;
  `ti/tps22917` is the largest at 104 KB. 500 packages would be about
  13 MB.
- **Prior art (verified):**
  - Cargo's sparse index (`config.json`; one file per crate, one JSON line
    per version; lines "should not be modified after being added, except
    for yanked"; static HTTP with ETag);
  - Go's GOPROXY protocol and `retract`.
- **Signing crates** (crates.io API, lockfile resolution counts, all
  platforms):
  - `minisign-verify` 0.3.0, MIT: **0 dependencies**;
  - `sigstore` 0.15.0, Apache-2.0: 414 packages (390 reduced). Its README
    says it is "not stable until 1.0" and that attestation verification
    is not implemented;
  - `ed25519-dalek` 2: 37;
  - `ureq` 3: 43.
- The workspace uses 16 crates.io crates. ADR-006 said "No new crates".
- **Licences:**
  - the vision's "OSI licence required for public publish" collides with
    KiCad's libraries (CC-BY-SA-4.0 with a design exception; no CC
    licence is OSI-approved);
  - ST's SVDs are under an ST EULA (cmsis-svd-data `STMicro/License.html`);
  - Nordic's MDK SVD is BSD-3-Clause;
  - ADI IBIS files restrict copies to use within the company.
  - Renode already fetches SVDs at use (`ApplySVD @https://dl.antmicro.com/…`).
- **Hosting:**
  - GitHub Pages: 1 GB site, a soft 100 GB/month, not for commercial
    hosting, $0;
  - Releases: assets under 2 GiB, "no limit" on bandwidth;
  - Cloudflare R2: 10 GB-month, 1M Class A and 10M Class B operations free,
    $0 egress;
  - the `breadboard.dev` domain resolves, but its ownership was not
    checked.
- bb has no HTTP client.

## Decision

### D1. Registry layout (static, git-hosted)

```
config.json                 { "format": 1, "dl": "https://<host>/dl/{id}/{vers}.tar", "keys": ["RW…"] }
index/<ns>/<name>.jsonl     one line per version, append-only
dl/<ns>/<name>/<vers>.tar   deterministic archive
snapshot.json               { "serial": N, "files": { "index/st/stm32f103c8.jsonl": "sha256:…", … } }
snapshot.json.minisig       minisign signature by the registry key
owners.toml                 namespace → GitHub users/teams allowed to merge
```

An index line, in canonical JSON:

```json
{"id":"ti/tps22917","vers":"0.2.0","tree":"sha256:…","size":104231,"license":"Apache-2.0","schema":"bb-part/1","deps":[{"id":"…","req":"^0.1"}],"provenance":{"source":"datasheet","confidence":"datasheet_checked","reviewed_by":[{"login":"…","review":123}],"generated_by":null,"unverified_claims":0},"platforms":[],"yanked":false,"pubtime":"2027-01-10"}
```

- `reviewed_by` and `unverified_claims` are written **by CI** from the PR
  approval and the claim checker (ADR-012). A package file cannot set them.
- Deps already exist: a module depends on its chip.

### D2. Immutability, yanking and removal

- **An index line never changes, except `yanked`.** CI enforces this with
  a diff check on every registry PR.
- **Archives are content-addressed.** The client recomputes the tree hash
  after extraction and refuses a mismatch.
- **A yanked version:**
  - is skipped by a new resolution;
  - an existing lock keeps working, with a warning from the cached index.
- **Legal removal** (licence violation, takedown) is the one exception:
  - the archive is deleted;
  - the line gains `removed: "<reason>"`;
  - a cached copy still verifies against the lock.
- **Rollback protection.** The client stores the highest snapshot
  `serial` it has seen and refuses a lower one: a light form of TUF.

### D3. Signing and trust

1. **The snapshot is signed by CI** with one Ed25519 minisign key. The key
   is an Actions environment secret behind required reviewers.
   - `bb` verifies it offline with `minisign-verify`, against a public key
     compiled into `bb` for the default registry, and against keys in
     `bb.toml` for private registries.
   - What a signature says: "this registry's CI checked this content".
2. **Each archive gets `actions/attest-build-provenance`.** These are
   keyless Sigstore attestations on the public-good Rekor. They can be
   verified offline with `gh attestation verify --bundle …
   --custom-trusted-root …`, and `bb` needs no dependency for that.
   - GitHub's limits: trust roots rotate "a few times a year", and an
     offline check cannot detect a repository rename.
3. **No per-author signatures in Phase 2.** The author's identity is the
   PR identity. Native Sigstore verification in `bb` is revisited after
   sigstore-rs 1.0.
4. **The vision's "refuse unsigned unless `--allow-unsigned`"** becomes:
   - a registry package must verify;
   - a local path package is unsigned by definition and is accepted as
     today;
   - no flag is needed.

### D4. Client: resolution, fetch and cache

- **The search path becomes:** `[packages] paths` (and a board file's
  `*bb packages path=`, ADR-015 D7), then **first-party** (the checkout's
  `packages/`, or `$BB_FIRST_PARTY_PACKAGES`), then the named registries,
  then the default public registry.
  - Local sources come first, so a developer's edit to a first-party
    package is used, as today (`spec/package.md` §1: a private package
    shadows a first-party one). A registry is consulted only for an id no
    local source has.
  - First-party packages are published to the public registry too, for
    users without a checkout. A package is the same package from either
    source when its tree hash is the same.
  - Every committed example's lock keeps `path+` sources, so `make test`
    and `make packages` never need the network (gate G8).
- **Routing by namespace**, among registries only. `[registries.acme]
  url = "…", key = "RW…", namespaces = ["acme"]`.
  - Among registries, a routed namespace resolves only on its own
    registry and never falls through to another one or to the public
    registry, which blocks dependency confusion.
  - Among registries, an unrouted namespace resolves only on the public
    registry.
  - Local sources are not registries: a path package with a routed
    namespace is used if a `[packages] path` provides it.
- **Version requirements** use Cargo's caret semantics on `MAJOR.MINOR.PATCH`.
- **Only `bb open` fetches.** `bb open --offline` uses the cache only.
  `bb run` never touches the network (ADR-007: only `bb open` writes).
- **The cache** is `~/.cache/bb/registry/<host>/` (index files, snapshot,
  archives and extracted trees, keyed by tree hash). A warm cache resolves
  offline.
- **Transport: shell out to `curl`** (`--proto =https --fail
  --location`), the way bb already runs `kicad-cli`. This adds no crate
  and gives HTTPS through the system's CA store. A missing `curl` is a
  clear error.
- **Local registries.** A registry whose `url` is `file://…` is read
  directly from the file system, without `curl`; the signature and
  serial checks are the same. That is how a private registry on a shared
  drive works, and how gate G1's fixture works. `curl` is never asked for
  a non-HTTPS URL.

### D5. `bb-lock/1`

```toml
schema = "bb-lock/1"

[[package]]
id = "ti/tps22917"
version = "0.2.0"
source = "registry+https://<host>/"      # or "path+../../packages/ti/tps22917"
tree = "sha256:…"                        # unchanged: the integrity root

[[vendor_file]]                          # fetch-at-use, never redistributed
url = "https://…/STM32F103.svd"
sha256 = "…"
license = "LicenseRef-ST-EULA"
used_by = "st/stm32f103c8"

[[platform]]
id = "renode:stm32f103-offline@1"
content = "sha256:…"
engine = "renode"                        # engine-neutral keys: a QEMU backend (Phase 3) is
engine_version = "1.17.0"                # another value, not another lock major
```

- `source` replaces the informative `path`.
- **Engine-neutral names.** `bb-lock/0`'s `renode = "1.17.0"` becomes
  `engine` plus `engine_version` in `/1`, so a Phase 3 QEMU or Verilator
  platform needs no new lock major (spec §10 allows additions only
  through reserved keys). The schema epoch reviews every new public
  format for the same issue (ADR-013 adds `--lattice`).
- `bb open` refuses a change of source for a locked id unless
  `--update-lock`.
- Signatures are not in the lock. They authenticate the first fetch, and
  the tree hash covers every fetch after it.
- `bb pkg migrate --lock` rewrites `/0` to `/1`: `path` becomes
  `path+<path>`.

### D6. Deterministic archives

An archive is a POSIX ustar with:

- entries sorted by path;
- mode 0644 (0755 for directories);
- uid and gid 0, empty user and group names;
- mtime 0;
- no extended headers;
- dotfiles and `tests/out/` excluded (the tree hash's rule).

`bb pkg pack` builds it. Two builds give byte-identical archives, and the
tree hash of the extracted tree equals the source's.

### D7. The licence gate

- Every package has an SPDX expression. Publishing requires each licence
  to be either OSI-approved, or one of **CC0-1.0, CC-BY-4.0 or
  CC-BY-SA-4.0**, and the latter only for data-only files (`pins.toml`,
  tables and vectors; never view code).
- Every view needs a `source`.
- A `vendor_derived` view needs `upstream_license` with a known
  redistributable SPDX id, otherwise publishing is refused. This is the
  TPS22917 rule.
- `LicenseRef-*` is refused, so `check_local_only.sh` is enforced by the
  registry CI too.
- A vendor file with restrictive terms is only ever a `vendor_file`
  reference. `bb pkg init --from-svd/--from-ibis` reads it through that
  reference and emits only derived facts (pin lists, buffer tables), with
  the upstream licence recorded.

### D8. CI on the registry repository

**On a PR:**

1. schema validation;
2. `bb pkg lint` and `bb pkg test` (ngspice only, since Renode platforms
   are compiled into `bb`);
3. the licence gate;
4. the provenance gate (ADR-012);
5. pins against the KiCad symbol where one is named (0 mismatches);
6. a size cap of 2 MB per package;
7. the index diff check;
8. `owners.toml` for the namespace.

**On merge:**

1. build the archives;
2. append the index lines;
3. bump the serial;
4. sign the snapshot;
5. attest;
6. deploy Pages.

A staging channel (`index-staging/`) holds agent-generated packages until
review (ADR-012).

### D9. Data only in Phase 2

Registry packages may not contain device code, `.repl` files or C#.
`[behavioral]` may name only platforms and devices compiled into `bb`.
Third-party behaviour waits for the WASM or out-of-process device
decision (ADR-006 open question 5), which is a Phase 3 candidate.

### D10. Namespaces and owners

- A namespace names the part's manufacturer (ADR-006 D1), not the author.
- At first the project owns every namespace.
- A vendor claims its namespace later: through a GitHub organisation with
  a verified domain, a PR to `owners.toml`, and a "vendor-verified" badge
  in the index. Past versions stay immutable.
- Ids are lower-case ASCII (`[a-z0-9._-]`), so lookalike characters cannot
  occur.

### D11. View-name collisions

- ADR-006 D3 deferred prefix-renaming of view top-level names, which needs
  a SPICE tokenizer, to this ADR.
- **Decision:** no automatic renaming in Phase 2. Instead the names are
  made unique by rule before anything is published:
  - **publishing requires** every top-level name a view defines (a
    `.subckt`, `.model`, `.func`) to start with `<NS>_<NAME>_` (the
    package id, upper-cased, `/` and `-` as `_`), and **no top-level
    `.param`** (parameters go inside the subcircuit or on its line);
  - `bb pkg lint` reports both as errors for a package marked for
    publication, and as warnings otherwise;
  - the first-party views are renamed in the schema-epoch commit (for
    example `packages/ti/drv8837`'s top-level `.param gleg`, `rdst` and
    `vth_k` move into `DRV8837_BRIDGE`, which becomes
    `TI_DRV8837_BRIDGE`), so nothing generic is published;
  - the registry CI still refuses a package whose view defines a
    top-level name that another published package defines differently.

### D12. Registry content across schema majors

- `spec/package.md` §10 reads the current and the previous major of an
  **authored** file. Registry archives are immutable and may be older.
- **Rule:** `bb` reads **every major ever published** to a registry it
  supports, by migrating in memory at read time. The archive's bytes, and
  so its tree hash, never change; the lock still matches.
- A migration that cannot be done in memory (a meaning that changed, not
  a key that moved) is not allowed for a schema whose files are
  published. Such a change needs a new package version, republished by
  its owner.
- The registry CI records each index line's `schema`, and `bb` refuses a
  major it does not know with "update bb", never with a migration hint.

## Exact `spec/` changes (described, not applied)

- **`spec/package.md`:**
  - §1: the search path with registries;
  - §9: `bb-lock/1`, `source`, `vendor_file`;
  - §10: the lock migration;
  - §11: lint rules for the licence gate and name prefixes;
  - a new §14 "Registries" pointing to `spec/registry.md`.
- **New `spec/registry.md`:**
  - layout;
  - the index line schema;
  - immutability;
  - snapshot and serial;
  - minisign verification;
  - the archive format;
  - the yank and removal semantics;
  - routing;
  - reading every published major (D12);
  - the view-name rule (D11).
- **`spec/package.md` §10:** authored files read two majors; registry
  content reads every published major (D12).
- **New `spec/schema/index-line.schema.json` and `snapshot.schema.json`**;
  `lock.schema.json` gets `/1`.
- **`spec/project.md` §1:** `[registries.<name>]`.

## Determinism impact

None on runs. A package resolved from the registry has the same tree hash
as the same package from a path. Decks, outputs and `run.json` are a
function of tree hashes, not of sources. `run.json` gains each package's
`source`, which is an input recorded, not a result.

## Gates

| # | Gate |
| --- | --- |
| G1 | End to end against a local registry fixture (a `file://` index and archives): `bb pkg pack` → index → `bb open` resolves and fetches → `bb run` offline gives the committed hashes of the example |
| G2 | Tampering: a modified archive byte is refused (tree hash); a modified index line without a signature is refused; a snapshot with a lower serial is refused |
| G3 | Yanking: a new resolve skips the version; an existing lock runs with a warning |
| G4 | Routing: a routed namespace with a same-named public package never resolves publicly |
| G5 | Determinism of archives: two `bb pkg pack` runs are byte-identical; extracted tree hash = source tree hash |
| G6 | Migration: every committed `bb.lock` migrated to `/1` gives the same tree hashes; `make packages` passes |
| G7 | The public registry: every first-party package published; dial-me-spinner resolves from it with a cold cache, and offline with a warm cache |
| G8 | No network: `make test` and `make packages` pass in a network namespace with no route (`unshare -n`, or a CI job without network); a first-party package edited in the checkout is used over its published copy |
| G9 | Majors: a fixture registry holding a `bb-part/0` and a `bb-part/1` archive of the same package resolves both, with the archive tree hashes unchanged; an unknown major is refused with "update bb" |
| G10 | Names: a view with an unprefixed top-level name or a top-level `.param` cannot be published; every first-party view passes after the epoch's renames |

## Rejected alternatives

- **The `sigstore` crate in `bb`.** It brings 390-414 packages, is pre-1.0,
  and has no attestation verification. GitHub's offline verification
  covers attestations without it.
- **OCI artifacts (ORAS, GHCR).** A client protocol and auth for about
  26 KB per package.
- **A dynamic registry server.** Static files are enough, and a mirror is
  a copy.
- **Per-author keys.** They add key management while the PR identity is
  the author.
- **`ureq` as the HTTP client.** It brings 43 packages. `curl` is
  ubiquitous on the supported hosts. Revisit if Windows-native `bb`
  becomes a goal.
- **Redistributing vendor SVDs and IBIS files.** Their licences forbid it.

## Questions for the approver

Consolidated in the plan's section 9 as **Q9** (licence policy,
`minisign-verify`, `curl`, hosting, search order), asked in round 2,
before E1. **Q3** (the schema epoch) covers D12's timing.

**Defaults (apply unless the approver objects):**

1. **Hosting details:** a new public repository (name to decide, for
   example `tyevco/bb-registry`) on GitHub Pages, R2 as the fallback, and
   the registry URL under `<user>.github.io` until the `breadboard.dev`
   domain's ownership is confirmed.
2. **The registry key:** one key for the snapshot, held as an Actions
   environment secret with required reviewers, and a rotation procedure
   in `spec/registry.md`.
3. **First-party packages are published** and keep their development copy
   in `packages/`, which resolves first; the release process publishes
   them.

## Approver decisions (2026-11-15)

All twelve questions of the plan's section 9 were accepted with their
recommended answers (plan section 10); round 2 was answered with round 1,
so E0 is signed off. For this ADR:

- **Q9, accepted.** The licence policy (OSI, plus CC0-1.0, CC-BY-4.0 and
  CC-BY-SA-4.0 for data-only files; vendor files fetched at use), the one
  new crate `minisign-verify`, `curl` as the fetcher, GitHub Pages
  hosting, and the search order with first-party before the public
  registry.
- **Q3, accepted.** `bb-lock/1` lands in milestone S, before any `/1` file
  reaches `main` and before anything is published.
- **Build scope.** E1 is in this build, **built and tested against local
  fixtures only**: a local git repository or `file://` registry, and a
  local static HTTP server. No agent creates the registry repository, its
  Pages site, a release or the signing secret, and nothing is pushed.
  Publishing is the human's step afterwards; hosting default 1 names
  where it will go.

**Defaults.** No objection was made to the defaults listed under
"Questions for the approver", so they apply as written.

## Review notes (design review round 1, 2026-11-14)

**Adopted:**

- **The search order shadowed the development copy** (major): first-party
  now resolves before every registry, routing applies among registries
  only, and G8 runs `make test` without a network.
- **Immutable archives across schema majors** (major): D12 reads every
  published major by in-memory migration, so tree hashes and locks hold;
  the schema epoch (plan milestone S) lands `bb-lock/1` before anything
  is published.
- **`file://` against `--proto =https`** (minor): local registries are
  read without `curl`.
- **Global view names** (minor): the `<NS>_<NAME>_` prefix and no
  top-level `.param` are publish-time errors, and the first-party views
  are renamed in the epoch.
- **Renode-specific lock keys** (minor): `engine` and `engine_version`.

**Not adopted:** making registry use opt-in per project (one critique's
alternative). With local sources first, a registry is consulted only for
an id no local source provides, which already keeps development builds
and `make test` off the network; an opt-in flag would add a second way to
say the same thing.

## Amendment 1 (2026-11-15): milestone S, the schema epoch, as built

Milestone S lands every Phase 2 schema major in one commit (plan section
4, track S; approver decision Q3). The governing decisions are ADR-009 to
ADR-013; this ADR holds the record because `bb-lock/1`, the view names
and the relock are its own. `spec/package.md` (sections 2-4 and 9-11),
`spec/project.md` (sections 1 and 5), `spec/event-stream.md` (version 2),
`spec/pin-model.md` and `spec/schema/{part,pins,lock,project}.schema.json`
state the result.

**1. The majors.** `bb-part/1`, `bb-pins/1` and `bb-lock/1` are new;
`bb-project/1`, `bb-board/1` and `bb-bench/1`, which R1 and B1 carried as
drafts (ADR-013 Amendment 1, ADR-010 Amendment 1), are S's with the same
keys plus the ones below. `bb-pkgtest` stays at major 0 (no Phase 2 key).
Event stream version 2 fixes the `reset` record and `assert`'s version 2
keys (ADR-009 D7, ADR-010 D7) in `bbcore::event`; nothing emits them yet,
and no version 1 record changes. Every committed file is at its current
major: 19 `part.toml`, 19 `pins.toml` (`bb pkg migrate`, the schema line),
14 `bb.lock` and 6 `board.bbm` (`make locks`).

**2. Accepted and inert.** The new package keys parse, are checked by
lint and change no `fast` deck: `[power]` (`idd`, `vdd_min`, `io_rail`),
a view's `rails` and `[[analog.corners]]`, the `supply_load` view kind
(never valid at `fast`, never a rung's default), `[[document]]`, `[claims]
file`, a provenance's `upstream_license`, `pins.toml` `limits`, and the
`digital_out` parameters `rail` and `rhiz` (sets and bindings). Shapes the
ADRs leave open are decided in the spec: state ports `{ role = "state",
state }`, limit bounds `<value>` or `<pin> ± <value>`, `[claims] file` as
`[pins] file` is, `udegC` for µ°C. Still reserved: `layers`, `thermal`,
`rtl`, `[pins] ibis`, `[behavioral] cpu`/`svd`, and the pin model name
`power` (ADR-009 D3 names it but defines no parameters, so P1 does).

**Refused, naming the milestone, rather than ignored:** a project's
`[run]` (R3, not in this build), a `simulate` list of several boards (R2;
a list of one is that board), and B1's bench keys (`fidelity`, `corner`,
`[[battery]]`, `[[activity]]`, `[[env]]`, `[probes]`, `replace`), which
ADR-010 A1.7 prefers refused. A key that changes what a run does would
otherwise run as other than written. `[registries.<name>]` (D4) is read,
checked and inert.

**3. `bb-lock/1`** (D5). `source = "path+<path>"` replaces `path`,
`engine` and `engine_version` replace `renode`, and `[[vendor_file]]` and
`registry+` sources are read, checked and written back. `bb-lock/0` is
read and migrated in memory. `bb run` accepts its legacy all-sources
platform hash (ADR-015 Amendment 3) and prints the migration hint. `bb
open` and `bb pkg migrate --lock` write the scoped hash, without
`--update-lock`, because the legacy hash pins strictly more. `bb open
--check` fails on a `/0` file. In a `/1` file only the scoped hash
matches. `run.json` (`bb-run/0`) keeps its `renode` key.

- **Deviation, recorded.** The migration drops a `renode:` entry for a
  platform this `bb` does not provide. That is
  `examples/dial-me-spinner/bb.lock`'s stale `renode:nrf52840-bb@1`, left
  by `--update-lock`, which never removes an entry. It has no scoped
  hash. No package names it, so nothing checked it.
- **Not done: `engine:bench@1`.** The bench engine's lock entry (ADR-010
  D4; A1.7 deferred it to S) is not written. Its id fits `bb-lock/1`'s
  grammar. But every project lock would move with every `bbbench` edit, and
  B2 and B3 edit it throughout. That is the churn H4 removed for plugins.
  Which projects lock it is ADR-010's decision, so track B makes it.
  `run.json` still records the identity (`bench.engine`).

**4. View names** (D11). The first-party views now define only prefixed
names: `GENERIC_{RESISTOR,CAPACITOR,INDUCTOR}_IDEAL`,
`NORDIC_NRF52840_QIAA_SAADC_SE`, `TI_DRV8837_BRIDGE` and
`TI_DRV8837_BRIDGE_BODY`, `TI_TPS22917_LSW_FIT` and
`WORLDSEMI_WS2812C_2020_LSW`. The `part.toml` `subckt` keys and the
WS2812C vectors that instantiate the view follow, and so does
`scripts/dms_packages.py`, so `make dms-vectors-check` regenerates them
byte for byte, and `scripts/dms_home_boards.py`, which copies the DRV8837
view verbatim into E3's `board-step.cir` and instantiates it there (its
`--check` in `make dms-home-test` requires the regenerated file). D11's example is wrong in one point: DRV8837's `.param
rdst`, `gleg` and `vth_k` were already inside its subcircuit, and no
first-party view had a top-level `.param`. Lint reports either rule as a
warning, and as an error with `bb pkg lint --publish`. `make packages`
lints `packages/` with `--publish` (G10: 11 packages, 0 findings). The
project-local example packages keep their names (5 warnings; they are not
published).

- No package version changes. A subcircuit rename changes no result, no
  port and no parameter set (`spec/package.md` section 10), and nothing is
  published yet. The trees change, and the relock records it.
- The WS2812C view was migrated from the suite "unchanged". It now carries
  one recorded change: its `deviations` list it, and
  `examples/dial-me-spinner/SOURCE` gains a `rename=<old>:<new>` field,
  which `scripts/dms_source_check.sh` applies to the upstream bytes before
  it compares them. With `DMS_REPO` the check passes, still against
  `6854d25`.

**5. The gate, as measured.** The row asks for "decks byte-identical, so
every output hash holds". A renamed view changes the deck's text, so the
closest correct gate is `make schema-epoch` (new; about 4 min, most of it
building the base `bb`). Every committed board file and every project
with each bench is elaborated three ways (`bb run --elaborate-only`; for a
board file this now writes the analog engine's netlist):

- A, the base commit's `bb` (`61a3845`) on the base tree;
- B, this `bb` on the base tree;
- C, this `bb` on this tree.

Results:

- A = B byte for byte on all 17 project decks.
- B = C on all 29 decks, byte for byte up to the rename list. The check
  renames whole words only, and lets only the renamed definition blocks
  move within the sorted definitions. 19 decks carry renamed views, and
  every rename that is in a base deck was compared.
- Two negative controls (a digit changed inside a renamed definition, and
  one in an element line) are both told apart.
- Not elaborated there: the system project, whose elaboration needs E1's
  bindings; `make dms-system` (in `make test`) does that. `generic/inductor`
  is in no committed deck; its own vectors build its renamed view.

Every committed output hash holds: `make test` passes, pooled and with
`POOL=0`, and so do `make packages`, `make dms-vectors-check`, `make
pi-spike` (37/37, P0's numbers unchanged), `make dms-source` and `make
lock-scope`. `lock-scope` now checks the committed locks themselves. A
stock relock of each copy gives the committed `bb.lock` and `board.bbm`
byte for byte. Under the edit to `bb_nrf52_saadc.cs`, 8 STM32 boards stay
current, and 7 nRF52840 ones show exactly their platform's drift.

**6. Order of merges, recorded.** The plan puts S on `main` "before any
`/1` file" (approver decision Q3). B1 and R1 were merged first. They
carried the `bb-bench/1`, `bb-project/1`, `bb-board/1` and `switch`
pieces of the draft (16 benches, 7 projects, their `board.bbm` and two
example packages were already `/1` on `main`). Those keys are the epoch's
own, unchanged here, so no file needs a second migration. Nothing has
been published (E1 has not landed), so no registry content is affected.

## Amendment 2 (2026-11-15): milestone E1 as built

Milestone E1 (plan section 4, track E) built D1-D12 against local fixtures
only (approver decision: nothing is published; plan section 10). The
normative result is the new `spec/registry.md`, `spec/package.md` sections
1, 2, 9-11 and 14, `spec/project.md` sections 1 and 6, and the new
`spec/schema/index-line.schema.json`, `snapshot.schema.json` and
`registry-config.schema.json` (`part.schema.json` gains `[behavioral] svd`).

**1. What was built.**

- `bbpkg::archive` (D6: the deterministic ustar and a strict reader),
  `bbpkg::registry` (D1-D4: the formats, minisign verification with
  `minisign-verify` 0.3.0, the one crate Q9 approved, with 0 dependencies;
  the serial; `file://` and `curl` transport; the cache; the `Resolver`)
  and `bbpkg::semver` (D4: caret requirements).
- `bb open` resolves an id no local source provides on the registries,
  `--offline` from the cache only; `bb run` and `bb flash` take a locked
  registry package from the cache by its tree hash and never fetch.
- `bb pkg pack` / `unpack` (D6), and `bb registry init`, `publish`, `yank`,
  `remove`, `snapshot`, `check` and `verify`, the tools of the registry's
  CI (D8). None of them signs: the key is only ever used by `minisign`.
- The licence gate (D7) is part of `bb pkg lint --publish`, which `make
  packages` already runs on `packages/` (11 packages, 0 findings).
- The registry repository's own files (README, `owners.toml`, the `pr` and
  `publish` workflows and their scripts) are `registry/template/` in
  breadboard.dev. `make registry-repo` makes the local repository
  `~/src/bb-registry` from them: every first-party package published,
  signed with the registry key that was generated locally into
  `~/.config/bb-registry` (never committed, never uploaded), committed on
  its `main`, with no remote. `docs/registry-publish.md` is the human's
  procedure to put it on GitHub Pages.

**2. Decisions taken inside the ADR, recorded.**

1. **`[behavioral] svd` declares a vendor file** (D5, D7). A `[[vendor_file]]`
   needs a declaration in the package, and no `bb-part/1` key declared one.
   The plan's rule for a key missing after S is to use a key S reserved
   (section 4, track S); `[behavioral] svd` was reserved for exactly this
   SVD. It is `{ url, sha256, license }`, `https://` only, any licence id
   (it is never redistributed). `bb open` fetches it at use into the cache,
   checks the sha256 and locks it; a changed declaration is a lock mismatch.
   Nothing in this build reads it during a run (`bb pkg init --from-svd` is
   not in E1's row). `[pins] ibis` stays reserved. No first-party package
   uses the key, so no tree hash changed.
2. **Which ids reach a registry.** A board file's `package=` parts; a
   project's named ids (an `[instances]` override, a `bb:part` field, a
   `[[device]]` package, a `[[connection]]` harness), its lock's
   `registry+` entries, and the chips of fetched modules (an index line's
   `deps`). A registry is not searched by MPN: index lines carry no MPNs,
   and D4 asks a registry only for an id. MPN and LCSC matching then run
   over the local and fetched packages together, which is how a project
   whose lock has registry sources resolves with no checkout (G7). A
   `path+` lock entry whose directory is gone is not looked up on a
   registry, since that would be a change of source.
3. **The source-change rule** (D5) compares the kind of source: `path`,
   or the registry's URL with one trailing `/`. A `path+` source with
   another path stays a warning (ADR-015 D7). `bb run` refuses a change of
   source too, like any other lock mismatch.
4. **The public registry** is compiled into `bb` (hosting default 1):
   `https://tyevco.github.io/bb-registry/` and the locally generated key
   `RWR3ioZYEKjIEvJyXiAwNL6czjJWdX3+bxrhw4QWChjFKUCBHjYT6EGI`.
   `BB_PUBLIC_REGISTRY=<url>` uses a mirror, verified with the compiled key
   (G7 runs that way), or a test fixture with `BB_PUBLIC_REGISTRY_KEY`;
   `BB_PUBLIC_REGISTRY=off` disables it.
5. **The snapshot also covers `config.json`**, and `dl` may be relative to
   the registry's URL, so a mirror and the `file://` fixture need no edit.
   A second, different snapshot with an already seen serial is refused too.
6. **The index line's provenance** is the least confident view's
   `source`, `confidence` and `generated_by` (the package-level block for
   a package without views). `reviewed_by` is `[]` and `unverified_claims`
   `null` until E2 writes them from approvals and the claim checker.
7. **The size cap** is 2 000 000 bytes per archive. A module's `deps` entry
   is `^<the chip's newest published version>`, and the chip must be
   published first (`bb registry publish` orders chips before modules).
8. **`run.json`** records each package's `source` as `path` for a local
   package, not its path: the path differs between copies of a project
   (every gate runs on copies), and `run.json` holds no paths. A registry
   package records its `registry+` source.
9. **The cache** is `$BB_REGISTRY_CACHE`, else `$BB_CACHE_DIR/registry`,
   else `$XDG_CACHE_HOME/bb/registry`, else `~/.cache/bb/registry`; the
   separate variable lets a test isolate the registry cache without
   moving the Renode plugin and JIT caches.
10. **Owners.** `owners.toml` is `default = [...]` plus `[namespaces]`. A
    PR is judged by the base's `owners.toml`, and a change to `owners.toml`
    or `config.json` needs a default owner, so a PR cannot make its author
    an owner.
11. **The plan's scope table** names `bb pkg publish` and `yank`. Here
    publishing is a pull request to the registry repository (D8), and its
    CI's tools are `bb registry publish` and `yank`.

**3. The gates, as measured** (2026-11-15, this worktree, host shared with
other tracks):

| Gate | Result |
| --- | --- |
| G1 end to end | `make registry-test`: the spike from the `file://` registry with no first-party packages, its lock's trees and platform equal to the committed lock's; `bb run` with the registry moved away, a recording `curl` and in a network namespace gives `tests/golden/spike/outputs.sha256` (nets.csv, events.jsonl, nets.vcd); the same resolve over the local HTTPS server with a cold cache; `--offline` with the server down |
| G2 tampering | refused: an archive content byte (the tree hash), a header byte (the checksum), an index line (its sha256 against the signed snapshot), a modified snapshot, an unsigned one, one signed by another key, and the registry served again at a lower serial |
| G3 yanking | a new resolve takes 0.1.0 over the yanked 0.1.1; the existing lock keeps 0.1.1 with a warning from `bb open` (fresh index) and `bb run` (cached index); an explicit request for the yanked version is refused; legal removal: kept from a verified cached copy, refused with a cold cache |
| G4 routing | a project routing `acme` resolves `acme/mcu` 0.1.0 from its registry although the public one has 9.0.0; an `acme` id only the public registry has is refused, naming the route; without the route it resolves publicly; a local `acme` package is used |
| G5 archives | three first-party packages: two packs byte-identical, the extracted tree hash = the source's, the ustar form checked with Python's `tarfile` and read by GNU tar; `bb registry check` re-packs every archive |
| G6 migration | milestone S (Amendment 1) |
| G7 the public registry | locally: `make registry-gate` (`scripts/registry_g7.sh`) on `~/src/bb-registry`, served over local HTTPS and verified with the compiled key (11 packages, 11 archives); dial-me-spinner's E1 project with no first-party packages and a `registry+` lock: `bb open` with a cold cache fetches every package and finds `board.bbm` and `bb.lock` current; ok-cold passes `check_dms_rail.py` and gives `tests/golden/dms-rail/ok-cold`; with the server stopped, `bb open --offline` is current and the same run in a network namespace gives the same bytes. The live part is the human's (`docs/registry-publish.md`) |
| G8 no network | `make test-offline`: `make test` (pooled) and `make packages` pass in a network namespace that has only `lo` (0 routes; the target refuses to start when `https://index.crates.io` answers), as the calling user (`scripts/offline.sh`). `registry-test` ran there too, its own `bb run` in a nested namespace. `make test` reads no network at all: a `curl` on PATH that records calls saw none in `cargo test -p bb-cli`, and every committed lock keeps `path+` sources |
| G9 majors | `fixture/old` 0.1.0 (`bb-part/0`, `bb-pins/0`) and 0.2.0 (`bb-part/1`) both resolve and elaborate, each locked tree = its source tree; an index line at `bb-part/9` is refused with "update bb" |
| G10 names | `make packages` (`bb pkg lint --publish packages`: 11 packages, 0 findings, now with the licence gate); a package with an unprefixed subcircuit is refused by `bb registry publish` |

Also checked: the licence gate (a `LicenseRef-` and a non-OSI licence
refused, a `vendor_derived` view without `upstream_license` refused, a
`LicenseRef-` vendor file accepted and absent from the archive), the
registry PR diff checks (a changed line, a deleted line, a replaced archive,
a republished version with another tree, a yank by a non-owner and a PR
that makes its author an owner: each refused), and the registry's own
`ci/pr-check.sh` on a local pull request (an owner passes, a non-owner is
refused, a non-owner approved by an owner passes). `make registry-test` has
104 checks.

**Times.** In `make test`, `make registry-test` takes 7.1 s pooled and
9.6 s with `POOL=0` in the final `make check` runs (8.9 s in an earlier
pooled run, 8.1 s inside `make test-offline`); the budget is +20 s. `make
registry-gate` takes 34 s (`registry-test` 11.1 s, then G7, with the local
registry repository already made), and `make test-offline` 978 s. `make
check` is green pooled (1096 s) and with `POOL=0` (1161 s). Both ran beside
other tracks' gates (1-minute load 4-36), against H0's idle baseline of
470 s, so H6's idle measurement decides the budget.

**4. Deviations, recorded.**

- **D8 items 4 and 5 are not run by the registry's CI yet.** The provenance
  gate needs ADR-012's claim checker (milestone E2), and the pins-against-
  KiCad-symbol check needs the KiCad symbol libraries in that CI
  (`scripts/check_pins_kicad.py` exists; milestone E3, which publishes
  packages, adds it). `ci/pr-check.sh` names both.
- **The registry's workflows have never run on GitHub** (approver decision:
  nothing is published). Their steps run locally as scripts (`ci/publish.sh`
  signs the local registry, `ci/pr-check.sh` checks a local pull request);
  the attestation and Pages steps run only on GitHub.
- **G8 runs locally, not as a CI job.** `make test-offline` uses an
  unprivileged network namespace (`scripts/offline.sh`). GitHub's
  ubuntu-24.04 runners restrict unprivileged user namespaces, and a second
  full `make test` per PR would not fit H0's 15-minute CI job budget, so no
  CI job was added. `registry-test` itself, in `make test`, runs its `bb
  run` in a network namespace where one is available and otherwise with a
  recording `curl` only.

## Amendment 3 (2026-11-17): the technical exit's registry findings (immutable versions and retired platforms)

Found by `make registry-gate` (G7) at the Phase 2 technical exit
(plan section 5 item 6), on `main` at f91d025. `make registry-test` (in
`make test`) passed; G7, which re-publishes every first-party package into
the local public registry repository (`~/src/bb-registry`, serial 1 from
E1), failed 17 checks for two reasons:

1. **A first-party package changed without a version bump.** P4 changed a
   comment in `raytac/mdbt50q-u1mv2/part.toml` (the platform it names,
   `nrf52840-bb@3` to `@4`), so its tree hash changed while it stayed
   0.1.0. `bb registry publish` refused it, as D2 requires ("versions are
   immutable, bump the version"). Fixed: `raytac/mdbt50q-u1mv2` 0.1.1;
   `make locks` changed only that entry in the ten dial-me-spinner
   projects' `bb.lock` and `board.bbm` files (version and tree). No deck, run or output
   hash changes (a module's part.toml has no view).
2. **`bb registry check` re-linted every published version against the
   running bb.** It ran `bb pkg lint --publish`'s rules, including D3's
   "the platform (device) is one this bb provides", on every version in
   the index. Since H5 (`dms/mechanism@1` became `@2`) and P4 (`nrf52840-bb@3`
   became `@4`), bb no longer provides what `dms/detent-mechanism` 0.1.0
   and `nordic/nrf52840-qiaa` 0.1.0 name, so the registry's own merge check
   (`ci/publish.sh`) and every PR check (`ci/pr-check.sh`) failed on
   versions that D2 forbids changing. D8 asks for `bb pkg lint` on a PR's
   packages, which `ci/pr-check.sh` runs on the changed ones, and `bb
   registry publish` lints every new version. Fixed: `bb registry check`
   reads published versions with that one rule off (`lint::Context`'s new
   `check_behavioral`, false only there); every other check of a published
   version (the tree, the archive form, the id, version, schema and
   licence against its line, the licence gate, the D11 names) is
   unchanged. A lock on such an old version still fails at `bb open` and
   `bb run`, which refuse a platform this bb does not provide (D3).
   `make registry-test` gains two checks (106): a registry holding a
   version whose platform this bb lacks (`retired-bb@1`, written as a
   published line with its archive) passes `bb registry check`, and a new
   version naming it is refused by `bb registry publish`. The control: the
   bb of f91d025 fails exactly the first new check.

`make registry-gate` then passed (106/106 and G7): the local registry
repository at serial 2 with every first-party package (15), E1 ok-cold
from the registry with a cold cache and offline with a warm one, equal to
`tests/golden/dms-rail/ok-cold`, and `ci/pr-check.sh` passing for an owner
and refused for a non-owner. Nothing was pushed or published.
