#!/usr/bin/env bash
# Phase 1 plan M3 gate (R7): time `make setup SETUP=demo` in a clean
# ubuntu:24.04 container, as root, with nothing installed beforehand.
#
#   bash scripts/setup_container_test.sh [--rev <commit>|--worktree] [--out <dir>] [--keep]
#
# The source is `git archive <commit>` (default HEAD) of this repository, or
# with --worktree every tracked and untracked-but-not-ignored file of the
# working tree (for a trial before committing). It is copied into the
# container (not bind-mounted), so the container's build sees a plain
# directory, as a fresh clone would give.
#
# Inside the container, timed separately:
#   1. prerequisites: apt-get update && apt-get install -y make (a clean
#      Ubuntu has no make; git comes with setup's own apt step);
#   2. make setup SETUP=demo (apt packages, rustup + Rust 1.99.1, Renode
#      1.17.0 portable, the release build of bb, bb into ~/.local/bin);
#   3. make doctor SETUP=demo, and $BB_CHECK (default `bb pkg lint
#      packages`, the installed bb on the first-party packages).
# The log and a timing summary are written to <out> (default
# out/setup-container). Uses podman (or docker if podman is absent), with
# sudo when not root. The container is removed unless --keep.
set -euo pipefail

REV=HEAD
WORKTREE=0
OUT=out/setup-container
KEEP=0
IMAGE=docker.io/library/ubuntu:24.04
while (($#)); do
  case "$1" in
    --rev) REV="$2"; shift 2 ;;
    --worktree) WORKTREE=1; shift ;;
    --out) OUT="$2"; shift 2 ;;
    --keep) KEEP=1; shift ;;
    --image) IMAGE="$2"; shift 2 ;;
    *) echo "unknown argument $1" >&2; exit 2 ;;
  esac
done

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if command -v podman >/dev/null 2>&1; then ENGINE=podman
elif command -v docker >/dev/null 2>&1; then ENGINE=docker
else echo "setup_container_test: neither podman nor docker is installed" >&2; exit 1; fi
if [ "$(id -u)" -eq 0 ]; then C=("${ENGINE}"); else C=(sudo "${ENGINE}"); fi

mkdir -p "${OUT}"
OUT="$(cd "${OUT}" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "${work}"' EXIT
if [ "${WORKTREE}" = 1 ]; then
  (cd "${REPO}" && git ls-files -co --exclude-standard -z | tar --null -T - -cf "${work}/src.tar")
  src_desc="worktree of $(git -C "${REPO}" rev-parse --short HEAD) (+ uncommitted changes)"
else
  git -C "${REPO}" archive --format=tar -o "${work}/src.tar" "${REV}"
  src_desc="git archive $(git -C "${REPO}" rev-parse --short "${REV}")"
fi

cat > "${work}/run.sh" <<'EOF'
#!/bin/bash
set -euo pipefail
export HOME=/root
now() { date +%s.%N; }
mkdir -p /src && tar -xf /work/src.tar -C /src
t0=$(now)
apt-get update -qq
DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends make >/dev/null
t1=$(now)
cd /src
make setup SETUP=demo
t2=$(now)
export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"
make doctor SETUP=demo
command -v bb
bash -c "${BB_CHECK}"
t3=$(now)
awk -v a="$t0" -v b="$t1" -v c="$t2" -v d="$t3" 'BEGIN {
  printf "prereq_s %.1f\nsetup_s %.1f\ncheck_s %.1f\ntotal_s %.1f\n", b-a, c-b, d-c, d-a }' > /work/timing.txt
du -sh /root/.rustup /root/.cargo /root/.local/opt /root/.cache/bb-target 2>/dev/null >> /work/timing.txt || true
EOF
chmod +x "${work}/run.sh"

name="bb-setup-test-$$"
log="${OUT}/setup.log"
echo "setup_container_test: ${ENGINE} ${IMAGE}, source ${src_desc}, log ${log}"
start=$(date +%s.%N)
set +e
"${C[@]}" run --name "${name}" -e BB_CHECK="${BB_CHECK:-bb pkg lint packages}" -v "${work}:/work" "${IMAGE}" /work/run.sh > "${log}" 2>&1
rc=$?
set -e
end=$(date +%s.%N)
if [ "${KEEP}" = 0 ]; then "${C[@]}" rm -f "${name}" >/dev/null 2>&1 || true; fi
{
  echo "engine ${ENGINE} $("${C[@]}" --version 2>/dev/null | head -1)"
  echo "image ${IMAGE} $("${C[@]}" image inspect --format '{{.Id}}' "${IMAGE}" 2>/dev/null | cut -c1-19)"
  echo "source ${src_desc}"
  echo "host $(nproc) cpus, load $(cut -d' ' -f1-3 /proc/loadavg)"
  echo "exit ${rc}"
  [ -f "${work}/timing.txt" ] && cat "${work}/timing.txt"
  awk "BEGIN{printf \"container_wall_s %.1f\n\", ${end}-${start}}"
} > "${OUT}/timing.txt"
cat "${OUT}/timing.txt"
if [ "${rc}" != 0 ]; then tail -30 "${log}"; echo "setup_container_test: FAIL (exit ${rc})"; exit 1; fi
echo "setup_container_test: PASS"
