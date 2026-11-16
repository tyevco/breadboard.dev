# Publishing the public package registry (the human's step)

Phase 2 milestone E1 built the registry and tested it against local fixtures only (approver
decision, 2026-11-15: no agent creates the repository, its Pages site, a release or the signing
secret, and nothing is pushed). This page is the exact procedure for putting it on GitHub Pages
under the hosting default of ADR-011 (`https://tyevco.github.io/bb-registry/`). The format and the
trust model are in `spec/registry.md`; the decisions in ADR-011 and its Amendment 2.

## What exists locally

| Thing | Where | Notes |
| --- | --- | --- |
| The registry key | `~/.config/bb-registry/bb-registry.key` (secret), `bb-registry.pub` | made with `minisign -G -W` (no password), key id `12C8A81058868A77`. **Never commit it, never paste it anywhere but the GitHub secret below.** Back the secret key up offline |
| The public key in `bb` | `cli/src/registry.rs`, `PUBLIC_KEY` | `RWR3ioZYEKjIEvJyXiAwNL6czjJWdX3+bxrhw4QWChjFKUCBHjYT6EGI`; `bb` verifies the public registry with it |
| The public URL in `bb` | `cli/src/registry.rs`, `PUBLIC_URL` | `https://tyevco.github.io/bb-registry/` |
| The registry repository | `~/src/bb-registry` (WSL), branch `main`, no remote | made by `make registry-repo`: every first-party package published at serial 1, signed; the CI workflows of `registry/template/` |

`make registry-gate` checked that repository end to end: served by a local HTTPS server and
verified with the compiled key, dial-me-spinner's E1 project resolves every package from it with
a cold cache and offline with a warm one, giving its committed hashes, and the registry's own PR
check passes for an owner and refuses a non-owner.

## Before you start

1. Merge E1 to breadboard.dev's `main` and **push breadboard.dev** first. The registry's CI
   builds `bb` from `https://github.com/tyevco/breadboard.dev` at the commit in the registry's
   `BB_REV` file, so that commit must be public.
2. Rebuild the local registry from that commit, so `BB_REV` names it:

   ```sh
   cd ~/src/<your breadboard.dev checkout>   # at the pushed main
   make registry-repo                        # updates ~/src/bb-registry and commits locally
   make registry-gate                        # optional: the full local check again
   git -C ~/src/bb-registry log --oneline -3
   ```

   If a first-party package changed since the last publication without a version bump,
   `make registry-repo` refuses ("versions are immutable"): bump the package's version first.

## On GitHub

3. **Create the repository** `tyevco/bb-registry`: public, empty (no README, licence or
   `.gitignore`; the local repository has its own).
4. **Pages:** Settings → Pages → Build and deployment → Source: **GitHub Actions**.
5. **The signing environment:** Settings → Environments → New environment
   `registry-signing`:
   - Deployment protection rules: **Required reviewers**: `tyevco` (ADR-011 D3: the key is
     behind required reviewers). Optionally restrict deployment branches to `main`.
   - Environment secret **`MINISIGN_SECRET_KEY`**: the whole content of
     `~/.config/bb-registry/bb-registry.key` (both lines), e.g. `cat
     ~/.config/bb-registry/bb-registry.key | clip.exe` from WSL and paste.
6. **Protect `main`** (Settings → Rules → Rulesets → New branch ruleset, target `main`):
   require a pull request with 1 approval, require the status check `check` (the `pr`
   workflow), block force pushes and deletions. Add **GitHub Actions** to the bypass list (the
   `publish` workflow commits the index lines and the signed snapshot back to `main`).
7. **Actions permissions:** Settings → Actions → General → Workflow permissions: "Read and write
   permissions" (the publish job pushes its commit; it also declares `pages`, `id-token` and
   `attestations` write).

## Push

8. From WSL:

   ```sh
   cd ~/src/bb-registry
   git remote add origin git@github.com:tyevco/bb-registry.git
   git push -u origin main
   ```

   The push runs the `publish` workflow. Approve its `registry-signing` deployment when GitHub
   asks. With nothing new to publish (the local repository is already signed), `ci/publish.sh`
   prints `publish: nothing new`; the job then attests every archive
   (`actions/attest-build-provenance`) and deploys the site.

## Check it

9. From any machine with `bb` (built from the pushed breadboard.dev):

   ```sh
   bb registry verify https://tyevco.github.io/bb-registry/ \
       --key RWR3ioZYEKjIEvJyXiAwNL6czjJWdX3+bxrhw4QWChjFKUCBHjYT6EGI
   ```

   It must print `serial 1, 11 packages, 11 archives verified`. Then a resolve without a
   checkout's packages:

   ```sh
   mkdir -p /tmp/regcheck && cp examples/spike/board.cir /tmp/regcheck/
   BB_FIRST_PARTY_PACKAGES=/nonexistent bb open /tmp/regcheck/board.cir
   grep source /tmp/regcheck/bb.lock     # registry+https://tyevco.github.io/bb-registry/
   ```

   And the build provenance of an archive:

   ```sh
   curl -sSfLO https://tyevco.github.io/bb-registry/dl/st/stm32f103c8/0.1.0.tar
   gh attestation verify 0.1.0.tar --repo tyevco/bb-registry
   ```

## Afterwards

- **New packages and versions** arrive as pull requests that add `packages/<ns>/<name>/`
  (README of the registry repository). The `pr` workflow runs `ci/pr-check.sh`; on merge,
  `publish` signs a new snapshot (approve the deployment each time).
- **First-party releases:** after a first-party package changes in breadboard.dev (with its
  version bumped), copy it into the registry repository in a pull request, or rerun `make
  registry-repo` locally and open a pull request from its commit.
- **Yanking:** a pull request that sets `"yanked":true` on the version's line.
- **Key rotation and compromise:** `spec/registry.md` section 10.
- **Another URL** (another account, or the `breadboard.dev` domain once its ownership is
  confirmed): change `PUBLIC_URL` in `cli/src/registry.rs` with an ADR-011 amendment, and keep
  the old URL working (a redirect, or a mirror) for released `bb` versions. A mirror needs no
  change: `BB_PUBLIC_REGISTRY=<url>` is verified with the compiled key.
