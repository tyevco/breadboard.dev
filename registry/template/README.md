# bb-registry: the public Breadboard package registry

This repository is a package registry for [Breadboard](https://github.com/tyevco/breadboard.dev),
served as static files (ADR-011, `spec/registry.md` in breadboard.dev). `bb open` resolves a
package id that no local source provides from here, checks the signed snapshot, and keeps the
verified archives in `~/.cache/bb/registry`. `bb run` never fetches.

| Path | What it is | Written by |
| --- | --- | --- |
| `packages/<ns>/<name>/` | the package source, one version at a time | a pull request |
| `index/<ns>/<name>.jsonl` | one canonical JSON line per published version, append-only | CI on merge |
| `dl/<ns>/<name>/<vers>.tar` | the deterministic archive of each version | CI on merge |
| `config.json` | `format`, the archive URL template `dl`, the registry's public keys | once |
| `snapshot.json`, `snapshot.json.minisig` | the serial and the sha256 of every index file, signed | CI on merge |
| `owners.toml` | who may change each namespace | a pull request, reviewed |
| `BB_REV` | the breadboard.dev commit whose `bb` the CI runs | a pull request |

## Publishing a package

1. Open a pull request that adds or changes `packages/<ns>/<name>/` (with a new `version` in
   `part.toml` for a changed package: published versions are immutable).
2. The `pr` workflow runs the registry's checks (ADR-011 D8): schema validation, `bb pkg lint
   --publish` (the view-name rule and the licence gate), `bb pkg test`, the size cap, the index
   diff check and `owners.toml`.
3. On merge, the `publish` workflow builds the archives, appends the index lines, bumps the serial,
   signs the snapshot with the registry key (an environment secret behind required reviewers),
   attests the archives and deploys the site.

**Yanking** is a pull request that sets `"yanked":true` on the version's index line (the only
change an index line may ever get). **Legal removal** is done by a maintainer with `bb registry
remove`, which deletes the archive and records the reason.

## Licences

Every package needs an SPDX licence that is OSI-approved, or CC0-1.0, CC-BY-4.0 or CC-BY-SA-4.0
for data-only packages (no view code). `LicenseRef-*` is refused. A vendor file with restrictive
terms (an SVD, an IBIS file) is only ever referenced (`[behavioral] svd`), fetched by `bb open`
at use and never stored here.

## Checking a published registry

    bb registry verify https://tyevco.github.io/bb-registry/ --key RWR3ioZYEKjIEvJyXiAwNL6czjJWdX3+bxrhw4QWChjFKUCBHjYT6EGI
