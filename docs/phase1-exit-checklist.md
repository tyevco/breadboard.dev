# Phase 1 exit checklist: the one human run

Phase 1's definition of done (`docs/phase1-plan.md` §5) says that a person
who has not worked on Breadboard can follow the README on a clean Ubuntu
24.04 and run the reference demo **in under 15 minutes of wall time,
including `make setup`**. M7 measured this with a timed script
(`scripts/stranger_test.sh`) in a clean container and in a fresh WSL distro;
see the plan's "Review notes: M7". This checklist covers what the script
cannot do: **one run by a person**. Approver decision B still holds:
nothing is published, the clone comes from a local path or `file://` URL,
and the human decides on publication afterwards (step 6).

## Roles

- **Runner:** someone who has not worked on Breadboard. They read only the
  README of the clone, from its top. They do not read this checklist, the
  plan or AGENTS.md, and they get no help beyond the source path in step 2.
- **Observer:** keeps the time and writes down where the runner hesitates,
  what they misread and what they ask. The observer does not help, except
  to unblock something outside the README's scope (for example, the host's
  network). Any such help is written down.

## 1. Prepare a clean Ubuntu 24.04 (observer, before the timer)

Choose one environment and record which. It must have no KiCad, no Zephyr
SDK, no ARM toolchain, no Rust, no Renode and no dial-me-spinner checkout.

**A. A container** (from WSL or a Linux host with podman or docker):

```sh
podman pull docker.io/library/ubuntu:24.04
podman run -it --name bb-human -v /mnt/c/source/tyevco/breadboard.dev:/repo:ro \
    docker.io/library/ubuntu:24.04 bash
```

Use rootless podman. A rootful container's root does not own `/repo`, so
`git clone` refuses it as "dubious ownership". In the runner's step 2,
the source is then `file:///repo`.

**B. A fresh WSL distro** (from Windows PowerShell; the name and folder are
examples):

```powershell
wsl --install Ubuntu-24.04 --name bb-human --location C:\wsl\bb-human
wsl -d bb-human
```

On a WSL that predates `--name`, import a root filesystem instead with
`wsl --import bb-human C:\wsl\bb-human <ubuntu-24.04 rootfs .tar or .wsl>`.
The M7 script exports `docker.io/library/ubuntu:24.04` with
`podman export` for this. A Store install asks for a user name and runs as
that user, so the README's first line needs `sudo` (the README says so). In
step 2, the source is the Windows path as WSL sees it:
`/mnt/c/source/tyevco/breadboard.dev`.

Check from the host that the Ubuntu archive is reachable first (a clean
image has no curl):
`curl -sI http://archive.ubuntu.com/ubuntu/dists/noble/Release` must
answer within a few seconds. During M7 Canonical's servers were unreachable
from the development host for over an hour, and the timed runs used
`mirrors.kernel.org` instead (plan, "Review notes: M7"). Record which
mirror the human run used. Use the image's default unless it is down.

Before the timer starts, check the environment in its shell. Every command
must print nothing:

```sh
command -v make cargo renode bb kicad-cli west arm-none-eabi-gcc
ls -d ~/zephyr-sdk* /opt/zephyr-sdk* 2>/dev/null
```

`git` may already be present in a Store distro, but not in the container.

## 2. What the runner is given

- the environment's shell, in the home directory;
- one sentence: "Follow the README of breadboard.dev, from the top. In
  place of `<CLONE-URL>`, use `<source>`." Here `<source>` is
  `file:///repo` (A) or `/mnt/c/source/tyevco/breadboard.dev` (B);
- for reading the README before cloning: the README at the commit under
  test, printed or shown from `git show <commit>:README.md`. The runner may
  read it at any time; reading counts toward the time.

Record the commit under test. It is the one the clone gets, which is the
source repository's `HEAD` on `main`:
`git -C /mnt/c/source/tyevco/breadboard.dev rev-parse HEAD`.

## 3. The timed run

Start the timer when the runner starts reading the README. Stop it when
the runner has seen the wrong-order check fail. The README's quickstart is:

1. install `git`, `make` and `ca-certificates` (`apt-get`, with `sudo` as a
   user);
2. `git clone <source> breadboard.dev && cd breadboard.dev`;
3. `make setup SETUP=demo`;
4. `export PATH="$HOME/.local/bin:$PATH"`;
5. `cd examples/dial-me-spinner`, then `bb open`;
6. `bb flash --target U10 firmware/prebuilt/led_rail.elf`;
7. `bb run --for 120ms --bench bench/cold.toml --probe net:VLED --probe net:LED_DIN --out out/rail`;
8. `python3 ../../scripts/check_dms_rail.py out/rail`;
9. the same with `firmware/prebuilt/led_rail_wrong_order.elf` and
   `--out out/rail-wrong`.

The observer writes down the clock time at the end of steps 2, 3, 8 and 9.

## 4. Pass criteria (observer ticks each)

- [ ] Total wall time, from first reading the README to the wrong-order
      result, is **under 15 minutes**: ____ min ____ s.
- [ ] `make setup SETUP=demo` ended with `setup (demo) complete`:
      ____ min ____ s.
- [ ] `bb open` printed `1 board, 201 instances, 142 nets` and
      `unchanged: ./board.bbm`.
- [ ] Step 8 printed `check_dms_rail: PASS: LED-DATA-V4, LSW-3, LSW-2` and
      exited 0 (`echo $?`).
- [ ] Step 9 printed `check_dms_rail: FAIL: LSW-3: max(V(LED_DIN) -
      V(VLED)) = 0.792868 V ...`, with a DIN clamp current of about
      4.08 mA. It ended with `FAIL: LED-DATA-V4, LSW-3, LSW-2 (failed:
      LSW-3)` and exited 1.
- [ ] `bb run` noted the `agent_generated` views (the ideal R and C), and
      `out/rail/run.json` lists the analog views with provenance,
      confidence and qualification (`grep -o '"provenance"'
      out/rail/run.json | wc -l` gives 132).
- [ ] The runner needed no help beyond step 2's sentence. Otherwise,
      record each point of help, and whether it shows a README defect.

Then, as a check after the timer (not timed), from the clone's root:

- [ ] `make demo` ends with `demo: PASS`. This also compares both runs
      with the committed hashes in `tests/golden/dms-rail/`.

## 5. Record the result

Add a line to the plan's "Review notes: M7" under "The human run". Give the
date, the environment (A or B, and the host), the commit, the total time
and step 3's time, each ticked or failed item, and each point where the
runner hesitated or needed help. Fix any README defect the run shows in a
commit of its own, then re-run `make stranger-test-container` and `make
stranger-test-wsl`. Run the human test again if the fix changed a step.

Clean up: `podman rm -f bb-human` (A), or `wsl --unregister bb-human` and
delete its folder (B).

## 6. After the exit: publication (the human's decision)

Phase 1 milestones never publish (approver decision B). If and when the
human decides to publish:

- create the repository and push `main`, by hand;
- replace `<CLONE-URL>` in `README.md` with the real URL, and the plan §5
  placeholder likewise. Keep the stranger-test markers, because
  `scripts/stranger_test.sh --url <real URL>` then tests the published
  clone;
- add the `LICENSE` file (Apache-2.0, approver decision A2) if it is not
  yet in the tree;
- check once more that nothing local-only is tracked
  (`bash scripts/check_local_only.sh`);
- run the stranger test against the published URL.

None of this is part of M7.
