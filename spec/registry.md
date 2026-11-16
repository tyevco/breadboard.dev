# Package registries

Status: normative. Decided by ADR-011 (accepted 2026-11-15, approver decisions Q3 and Q9;
Amendment 2, milestone E1 as built). Implementation: `bbpkg::registry`, `bbpkg::archive` and
`bbpkg::semver` (the client, the archive format, versions), `cli/src/registry.rs` (`bb
registry`, the public registry's identity, vendor files) and `bb open` / `bb run`. JSON Schemas:
`spec/schema/index-line.schema.json`, `snapshot.schema.json` and `registry-config.schema.json`.
The registry repository's own layout and CI are in breadboard.dev's `registry/template/`.
**Changing this file requires an ADR.**

A registry is a set of static files: a git repository served by any static web host (GitHub
Pages for the public one), or a directory read through `file://`. `bb open` is the only command
that reads one; `bb run` never does (section 7).

## 1. Layout

```
config.json                 {"dl":"dl/{id}/{vers}.tar","format":1,"keys":["RW..."]}
index/<ns>/<name>.jsonl     one line per published version, append-only (section 2)
dl/<ns>/<name>/<vers>.tar   the deterministic archive of each version (section 4)
snapshot.json               {"files":{"config.json":"sha256:...","index/...":"sha256:..."},"serial":N}
snapshot.json.minisig       the snapshot's minisign signature by the registry key (section 3)
owners.toml                 who may change each namespace (the registry's CI, section 9)
```

- **`config.json`** (canonical JSON): `format` is 1; a client refuses another format with
  "update bb". `dl` is the archive URL template; `{id}` is `<ns>/<name>` and `{vers}` the
  version; a template without `://` is relative to the registry's URL (so a mirror works
  unchanged). `keys` lists the registry's public keys, informatively: the key a client trusts
  is the one compiled into `bb` (the public registry) or written in `bb.toml` (section 6).
- Every JSON file is canonical (`spec/canonical-json.md`), and a client refuses an index line or
  a snapshot that is not.

## 2. The index

One file per package, one canonical JSON line per version, in publication order:

```json
{"deps":[],"id":"st/stm32f103c8","license":"Apache-2.0","platforms":["renode:stm32f103-offline@1"],"provenance":{"confidence":"unchecked","generated_by":null,"reviewed_by":[],"source":"estimated","unverified_claims":null},"pubtime":"2026-11-15","schema":"bb-part/1","size":10752,"tree":"sha256:b4b8...","vers":"0.1.0","yanked":false}
```

| Key | Meaning |
| --- | --- |
| `id`, `vers` | the package and its version (`MAJOR.MINOR.PATCH`); a version appears once |
| `tree` | the package's tree hash (`spec/package.md` section 9): the identity, and what a lock pins |
| `size` | the archive's size in bytes, at most 2 000 000 |
| `license` | `part.toml`'s `[part] license` |
| `schema` | `part.toml`'s schema id (`bb-part/<major>`, section 5) |
| `deps` | `{id, req}`: a module's chip (`[module] chip`), `req` a caret requirement on the chip version published when the module was |
| `provenance` | the least confident view's `source`, `confidence` and `generated_by` (the package-level block for a package without views; `null` for neither); `reviewed_by` (`{login, review}`) and `unverified_claims` are written by the registry's CI from pull-request approvals and the claim checker (ADR-012, milestone E2; until then `[]` and `null`), never from a package file |
| `platforms` | the lock identities of the engine platforms the package names |
| `yanked` | section 5 |
| `pubtime` | the publication date, `YYYY-MM-DD` |
| `removed` | only after a legal removal: the reason (section 5) |

**Immutability.** A line never changes, except `yanked`, and `removed` (with `yanked`) on a legal
removal. Lines are only appended. A published archive never changes. The registry's CI enforces
this on every pull request (`bb registry check --base`, section 9). Publishing a version that is
already published with another tree is refused: bump the version.

## 3. The snapshot, the signature and the serial

- `snapshot.json` maps `config.json` and every index file to its sha256, and carries a `serial`
  that every new snapshot increases by one.
- `snapshot.json.minisig` is a minisign signature of `snapshot.json`'s bytes (prehashed, the
  minisign default; a legacy signature is refused). The registry's CI makes it with the registry
  key, an Ed25519 minisign key held as a CI environment secret behind required reviewers.
- A client verifies the signature with its trusted key, then every file it reads against the
  snapshot's sha256 ("a modified file is refused"), and every archive by its tree hash after
  extraction (section 4).
- **Rollback.** The client keeps, per registry, the highest serial it has accepted, and refuses
  a lower one ("a rolled-back registry is refused"), and a different snapshot with the same
  serial.
- What a signature says: this registry's CI checked this content. There are no per-author
  signatures; the author is the pull request's.

## 4. Archives

A POSIX ustar of the files the tree hash covers (dotfiles and `tests/out/` excluded), with their
parent directories as entries:

- entries sorted by path, bytewise (a directory as `<path>/`);
- mode 0644, 0755 for a directory; uid and gid 0; empty user and group names; mtime 0; type `0`
  or `5` only; no extended (pax or GNU) headers; a path longer than 100 bytes split into ustar's
  prefix and name;
- then two zero blocks, and nothing after them.

`bb pkg pack <dir>` writes it; two packs of one tree are byte-identical. The reader (`bb pkg
unpack`, and every fetch) accepts only that form: regular files and directories, relative paths
without `.`, `..`, empty or dot components, valid header checksums, no entry twice, no data after
the end. Extraction writes fresh files, so only paths and contents reach the tree hash, which is
recomputed and must equal the index line's ("a tampered archive is refused").

## 5. Versions, yanking, removal and schema majors

- **Requirements** use Cargo's caret semantics on `MAJOR.MINOR.PATCH` (`^1.2.3` is `>=1.2.3,
  <2.0.0`; `^0.2.3` is `>=0.2.3, <0.3.0`; `^0.0.3` is `=0.0.3`). `id@1.2.3` in a board file or
  `bb.toml` asks for exactly that version.
- **A new resolution** takes the highest version that is neither yanked nor removed (and matches a
  dependency's requirement, if one applies). An exact request for a yanked or removed version is
  refused.
- **A locked version** is kept: yanked, with a warning (`bb open` from the fresh index, `bb run`
  from the cached one); removed, only from a verified copy in the cache, with a warning, and
  refused without one.
- **Legal removal** (a licence violation, a takedown) deletes the archive and records the reason
  in the line (`bb registry remove`).
- **Schema majors** (ADR-011 D12). `bb` reads every major ever published to a registry it
  supports, by migrating in memory when it reads the extracted package; the archive's bytes and
  tree hash never change, so a lock still matches. This build reads `bb-part/0` and `bb-part/1`.
  An index line or a package of a major `bb` does not know is refused with "update bb". A change
  of meaning that an in-memory migration cannot express is not allowed for a published schema; it
  needs a new package version.

## 6. Which registries a package comes from

The search path (`spec/package.md` section 1) is: the project's `[packages] paths` (a board
file's `*bb packages path=` lines), the first-party packages, then the registries. **A registry
is asked only for an id no local source provides.** Local sources are not registries: a local
package of a routed namespace is used.

- **The public registry** is compiled into `bb`: `https://tyevco.github.io/bb-registry/` with
  the key `RWR3ioZYEKjIEvJyXiAwNL6czjJWdX3+bxrhw4QWChjFKUCBHjYT6EGI` (key id
  `12C8A81058868A77`). `BB_PUBLIC_REGISTRY=<url>` uses a mirror, verified with the compiled key
  (or with `BB_PUBLIC_REGISTRY_KEY`, for a test fixture); `BB_PUBLIC_REGISTRY=off` disables it.
- **Private registries** are `bb.toml`'s `[registries.<name>]` (`url`, `key`, `namespaces`;
  `spec/project.md` section 1). **Routing:** an id whose namespace is routed resolves on that
  registry only and never falls through to another registry or to the public one (a missing id
  is an error naming the route), which blocks dependency confusion; every other namespace
  resolves on the public registry. A board file has no `bb.toml`, so only the public registry.
- **Which ids.** A board file: its `package=` parts. A project: the ids it names (an
  `[instances]` override, a `bb:part` field, a `[[device]]` package, a `[[connection]]`
  harness), the ids its lock pins with a `registry+` source, and the chips of the modules
  fetched; MPN and LCSC matching then run over the local and fetched packages together. A
  registry is not searched by MPN.
- **Sources in the lock** (`spec/package.md` section 9): `registry+<url>` with the registry's
  URL (one trailing `/`). `bb open` refuses a change of source for a locked id (`path+` to
  `registry+`, or another registry) unless `--update-lock`; `bb run` refuses it too. A `path+`
  source with another path is only a warning (ADR-015 D7).

## 7. Fetching and the cache

- **Only `bb open` fetches**; `bb open --offline` uses the cache only; `bb run` (and `bb flash`)
  take a locked registry package from the cache by its tree hash, recomputing the hash, and never
  read a snapshot or the network. A tree missing from the cache is an error that says to run `bb
  open`.
- **Transport.** A `file:///<absolute path>` registry is read from the file system directly. An
  `https://` one is fetched with `curl --proto =https --proto-redir =https --location`; a
  missing `curl` is a clear error. `bb` never fetches any other scheme. `curl` honours the
  system CA store (and `CURL_CA_BUNDLE`).
- **The cache:** `$BB_REGISTRY_CACHE`, else `$BB_CACHE_DIR/registry`, else
  `$XDG_CACHE_HOME/bb/registry`, else `~/.cache/bb/registry`. One directory per registry, named
  by its URL without the scheme, every byte outside `[A-Za-z0-9.-]` as `_` (`file` plus the path
  for a `file://` registry): `snapshot.json`, `snapshot.json.minisig`, `serial` (the highest
  accepted), `files/` (`config.json` and the index files as verified), `index/` (the index files
  for `bb run`'s yank warning) and `trees/<hex>` (the extracted trees, keyed by tree hash).
  Vendor files are in `vendor/<sha256>`. Deleting the cache only costs a fetch.

## 8. Vendor files

A vendor file whose terms forbid redistribution (an SVD, an IBIS file) is only ever referenced.
A package names it (`[behavioral] svd = { url, sha256, license }`, `spec/package.md` section 2;
`https://` only, any licence id including `LicenseRef-`), and it is never in the package. `bb
open` fetches it at use into the cache, checks its sha256 ("refused" on a mismatch) and records
it in the lock (`[[vendor_file]]`: `url`, `sha256`, `license`, `used_by`); a changed declaration
needs `--update-lock`. `bb run` checks the lock records what the packages declare. Nothing in this
build reads a vendor file during a run.

## 9. Publishing (the registry's CI)

`bb registry` holds the registry side; none of it signs (only `minisign`, in the registry's CI,
holds the key).

| Command | What it does |
| --- | --- |
| `bb registry init <dir> --key <pub>` | `config.json` and `index/` |
| `bb registry publish <dir> --pubtime <date> <pkg dir>...` | for each new version: `bb pkg lint --publish` clean (the D11 view names, the D7 licence gate), the 2 MB cap, `dl/` and the index line; the same tree again is a no-op, another tree for a published version is refused; chips before the modules that depend on them |
| `bb registry yank <dir> <id>@<v> [--undo]` | set or clear `yanked` |
| `bb registry remove <dir> <id>@<v> --reason <text>` | a legal removal |
| `bb registry snapshot <dir>` | bump the serial, write `snapshot.json`, delete the stale signature |
| `bb registry check <dir> [--base <dir>] [--author <login>] [--approver <login>]...` | every line's archive (size, strict form, tree hash, re-pack byte-identical), each extracted package's publication lint, no top-level view name defined differently by two packages, no stray archive, the snapshot covering exactly the files; with `--base` (a pull request), the immutability rules of section 2, a higher serial, and `owners.toml` (of the base) for every touched namespace, and for any change of `owners.toml` or `config.json` the default owners |
| `bb registry verify <url> --key <pub>` | a client's full check of a published registry: signature, every index file, every archive |

**The licence gate** (ADR-011 D7, `bb pkg lint --publish`): every licence id of `part.license`
OSI-approved (the list in `bbpkg::lint::OSI_LICENSES`), or CC0-1.0, CC-BY-4.0 or CC-BY-SA-4.0
for a package without view code; no `LicenseRef-*`; every view with a provenance `source`; a
`vendor_derived` view with a redistributable `upstream_license`.

**Owners** (`owners.toml`): `default = [<login>...]` for every namespace not listed under
`[namespaces]` (`<ns> = [<login>...]`). At first the project owns every namespace (ADR-011 D10).

The registry repository's workflows (`registry/template/.github/workflows`: `pr` runs
`ci/pr-check.sh`, `publish` runs `ci/publish.sh`, attests every archive with
`actions/attest-build-provenance` and deploys Pages) are the D8 pipeline. Milestone E1 built and
ran them locally only; `docs/registry-publish.md` is the human's step to put them on GitHub.

## 10. Keys and rotation

The registry key is one Ed25519 minisign key. To rotate it:

1. make the new key (`minisign -G`) and add its public key to `config.json` `keys` (a reviewed
   pull request, the default owners);
2. release a `bb` whose compiled `PUBLIC_KEY` list holds both keys (a client accepts a signature
   by any trusted key);
3. switch the CI secret to the new key; the next snapshot is signed with it;
4. after the old `bb` releases are out of support, drop the old key from `config.json` and from
   `bb`.

A private registry rotates the same way, through its `bb.toml` `key`. A compromised key: replace
the secret, publish a snapshot with a higher serial signed by the new key, release a `bb` that
trusts only the new key, and tell users of private registries to update `bb.toml`.
