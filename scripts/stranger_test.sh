#!/usr/bin/env bash
# Phase 1 plan M7 (section 5, the exit): the stranger test, timed, from a LOCAL clone (approver
# decision B: nothing is published; the README's clone URL is a placeholder).
#
#   bash scripts/stranger_test.sh container [--url <url>] [--out <dir>] [--image <ref>] [--keep]
#   bash scripts/stranger_test.sh wsl       [--url <url>] [--out <dir>] [--image <ref>] [--keep]
#
# The steps are the README's own: the fenced block between `<!-- stranger-test:begin -->` and
# `<!-- stranger-test:end -->` in README.md of the commit being tested, with `<CLONE-URL>` replaced
# by --url, nothing else. Each line runs in one bash shell (so `cd` and `export` carry on), as
# root, from $HOME, with stdin closed. A line must exit 0, except a line marked `expected to FAIL`,
# which must exit non-zero and print `FAIL: LSW-3` with LED-DATA-V4 and LSW-2 passing (the
# wrong-order firmware). The README time is the wall time from the first line to the last.
#
# Afterwards, outside the timed steps: the runs' nets.csv, nets.vcd and non-grant records against
# the committed hashes (tests/golden/dms-rail), `make demo` (timed on its own), and the
# environment's facts (no KiCad, no Zephyr SDK, no ARM toolchain, no dial-me-spinner).
#
# Environments (a clean Ubuntu 24.04 each time, nothing preinstalled, as root):
#   container  rootless podman (or docker), image --image (default docker.io/library/ubuntu:24.04).
#              The repository is bind-mounted read-only at /repo; default --url file:///repo.
#              Rootless, so the container's root owns the mount, as a user owns their own clone
#              source (a rootful container's root does not, and git refuses "dubious ownership").
#   wsl        a fresh WSL 2 distro: the same image's root filesystem (podman export), imported
#              with `wsl.exe --import` into $STRANGER_WSL_DIR (default %TEMP%\bb-stranger-wsl-<pid>),
#              run, then unregistered and its directory deleted (--keep keeps both). Run it from WSL
#              with Windows interop. Default --url: this repository's path (/mnt/c/...), which the
#              new distro sees at the same path.
# Results: <out>/timing.txt (the summary), steps.tsv, step logs; default out/stranger/<mode>.
set -euo pipefail

MODE=${1:?usage: stranger_test.sh container|wsl [--url <url>] [--out <dir>] [--image <ref>] [--keep]}
shift
REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
URL=
OUT=out/stranger/$MODE
IMAGE=docker.io/library/ubuntu:24.04
KEEP=0
while (($#)); do
  case "$1" in
    --url) URL=$2; shift 2 ;;
    --out) OUT=$2; shift 2 ;;
    --image) IMAGE=$2; shift 2 ;;
    --keep) KEEP=1; shift ;;
    *) echo "stranger_test: unknown argument $1" >&2; exit 2 ;;
  esac
done
case "$MODE" in
  container) URL=${URL:-file:///repo} ;;
  wsl) URL=${URL:-$REPO} ;;
  *) echo "stranger_test: unknown mode $MODE (container or wsl)" >&2; exit 2 ;;
esac
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
rm -f "$OUT"/*

# The README's steps, as committed (HEAD: what a clone gets).
git -C "$REPO" show HEAD:README.md | awk '
  /<!-- stranger-test:end -->/ { inblk = 0 }
  inblk && /^```/ { next }
  inblk { print }
  /<!-- stranger-test:begin -->/ { inblk = 1 }' > "$OUT/readme-steps.sh"
[[ -s $OUT/readme-steps.sh ]] || { echo "stranger_test: README.md at HEAD has no stranger-test block" >&2; exit 1; }
grep -q 'git clone <CLONE-URL> ' "$OUT/readme-steps.sh" || { echo "stranger_test: the block has no 'git clone <CLONE-URL>'" >&2; exit 1; }
sed "s|git clone <CLONE-URL> |git clone $URL |" "$OUT/readme-steps.sh" > "$OUT/steps.sh"
HEAD_ID=$(git -C "$REPO" rev-parse HEAD)

# The runner, inside the clean environment: runner.sh <steps> <outdir>
cat > "$OUT/runner.sh" <<'EOF'
#!/bin/bash
set -uo pipefail
STEPS=$1 R=$2
export HOME=/root
cd "$HOME"
now() { date +%s.%N; }
{
  echo "os $(. /etc/os-release && echo "$PRETTY_NAME") kernel $(uname -r), $(nproc) cpus, $(awk '/MemTotal/ {printf "%.1f GB", $2/1048576}' /proc/meminfo)"
  echo "user $(id -un) home $HOME"
  for t in git make cargo rustc renode ngspice bb kicad-cli eeschema west arm-none-eabi-gcc python3; do
    echo "before: $t $(command -v $t > /dev/null && echo present || echo absent)"
  done
  echo "before: zephyr-sdk $(ls -d /opt/zephyr-sdk* $HOME/zephyr-sdk* $HOME/.local/opt/zephyr-sdk* 2>/dev/null | head -1 || true)"
  echo "before: dial-me-spinner $(find / -xdev -maxdepth 4 -name 'dial-me-spinner*' -not -path '/proc/*' 2>/dev/null | head -1)"
} > "$R/env.txt"
: > "$R/steps.tsv"
fail=0
n=0
T0=$(now)
while IFS= read -r line; do
  [[ -z ${line//[[:space:]]/} || $line =~ ^[[:space:]]*# ]] && continue
  n=$((n + 1))
  t=$(now)
  echo "stranger: [$n] \$ $line"
  eval "$line" > "$R/step$n.log" 2>&1 < /dev/null
  rc=$?
  dt=$(awk -v a="$t" -v b="$(now)" 'BEGIN { printf "%.1f", b - a }')
  tail -n 12 "$R/step$n.log" | sed 's/^/    /'
  if [[ $line == *"expected to FAIL"* ]]; then
    if [[ $rc != 0 ]] && grep -q 'FAIL: LSW-3' "$R/step$n.log" && grep -q 'PASS: LED-DATA-V4' "$R/step$n.log" \
        && grep -q 'PASS: LSW-2' "$R/step$n.log"; then verdict="ok (failed as expected: exactly LSW-3)"
    else verdict="WRONG (expected exactly LSW-3 to fail; exit $rc)"; fail=1; fi
  elif [[ $rc == 0 ]]; then verdict=ok
  else verdict="FAILED (exit $rc)"; fail=1; fi
  printf '%s\t%s\t%s\t%s\n' "$n" "$dt" "$verdict" "$line" >> "$R/steps.tsv"
  echo "stranger: [$n] ${dt} s: $verdict"
  [[ $fail == 0 ]] || break
done < "$STEPS"
T1=$(now)
awk -v a="$T0" -v b="$T1" 'BEGIN { printf "readme_wall_s %.1f\n", b - a }' > "$R/readme-time.txt"
[[ $fail == 0 ]] || { echo "stranger: README steps FAILED"; exit 1; }
PROJ=$PWD  # the README ends in examples/dial-me-spinner
REPO=$(git rev-parse --show-toplevel)
{
  echo "clone $(git -C "$REPO" rev-parse HEAD) $(git -C "$REPO" log -1 --format=%s)"
  echo "clone status after the README (bb flash edits bb.toml; out/ is ignored): $(git -C "$REPO" status --short | tr '\n' ' ')"
  echo "after: bb $(command -v bb) $(du -sh "$HOME/.rustup" "$HOME/.local/opt" 2>/dev/null | awk '{print $2"="$1}' | tr '\n' ' ')"
  for t in kicad-cli west arm-none-eabi-gcc; do echo "after: $t $(command -v $t > /dev/null && echo present || echo absent)"; done
} >> "$R/env.txt"
post=0
for pair in rail:ok-cold rail-wrong:wrong-cold; do
  d=$PROJ/out/${pair%%:*} c=${pair#*:}
  grep -v '"type":"grant"' "$d/events.jsonl" > "$d/non-grant-records"
  if (cd "$d" && grep -v ' events.jsonl$' "$REPO/tests/golden/dms-rail/$c/outputs.sha256" | sha256sum --quiet -c); then
    echo "post: PASS: out/${pair%%:*} equals tests/golden/dms-rail/$c (nets.csv, nets.vcd, non-grant records)"
  else echo "post: FAIL: out/${pair%%:*} differs from tests/golden/dms-rail/$c"; post=1; fi
done | tee "$R/post.txt"
grep -q 'post: FAIL' "$R/post.txt" && post=1
cp "$PROJ/out/rail/run.json" "$R/run.json" 2>/dev/null || true
t=$(now)
(cd "$REPO" && make demo) > "$R/make-demo.log" 2>&1 < /dev/null
rc=$?
awk -v a="$t" -v b="$(now)" -v rc="$rc" 'BEGIN { printf "make_demo_s %.1f exit %d\n", b - a, rc }' >> "$R/readme-time.txt"
tail -n 25 "$R/make-demo.log"
[[ $rc == 0 ]] || post=1
exit $post
EOF
chmod +x "$OUT/runner.sh"

if command -v podman > /dev/null 2>&1; then ENGINE=podman
elif command -v docker > /dev/null 2>&1; then ENGINE=docker
else echo "stranger_test: neither podman nor docker is installed" >&2; exit 1; fi
"$ENGINE" image exists "$IMAGE" 2> /dev/null || "$ENGINE" pull -q "$IMAGE" > /dev/null
IMAGE_ID=$("$ENGINE" image inspect --format '{{.Id}}' "$IMAGE" | cut -c1-12)

now() { date +%s.%N; }
echo "stranger_test: $MODE, image $IMAGE ($IMAGE_ID), clone $URL (HEAD $HEAD_ID), out $OUT"
load0=$(cut -d' ' -f1-3 /proc/loadavg)
start=$(now)
set +e
if [[ $MODE == container ]]; then
  name=bb-stranger-$$
  "$ENGINE" run --name "$name" -v "$REPO:/repo:ro" -v "$OUT:/work" "$IMAGE" \
    bash /work/runner.sh /work/steps.sh /work 2>&1 | tee "$OUT/run.log"
  rc=${PIPESTATUS[0]}
  [[ $KEEP == 1 ]] || "$ENGINE" rm -f "$name" > /dev/null 2>&1
  where="$ENGINE $("$ENGINE" --version | head -1), rootless=$([[ $(id -u) != 0 ]] && echo yes || echo no)"
else
  command -v wsl.exe > /dev/null || { echo "stranger_test: wsl mode needs WSL with Windows interop" >&2; exit 1; }
  export WSL_UTF8=1
  win_tmp=$(wslpath "$(cmd.exe /c 'echo %TEMP%' 2> /dev/null | tr -d '\r')")
  dir=${STRANGER_WSL_DIR:-$win_tmp/bb-stranger-wsl-$$}
  name=bb-stranger-$$
  mkdir -p "$dir/distro"
  cid=$("$ENGINE" create "$IMAGE" true)
  "$ENGINE" export -o "$dir/rootfs.tar" "$cid"
  "$ENGINE" rm -f "$cid" > /dev/null
  cp "$OUT/runner.sh" "$OUT/steps.sh" "$dir/"
  mkdir -p "$dir/out"
  case "$OUT" in /mnt/*) R=$OUT ;; *) R=$dir/out ;; esac
  wsl.exe --import "$name" "$(wslpath -w "$dir/distro")" "$(wslpath -w "$dir/rootfs.tar")" --version 2
  rm -f "$dir/rootfs.tar"
  start=$(now) # the import is not the stranger's time; the distro starts here
  wsl.exe -d "$name" --user root --exec bash "$dir/runner.sh" "$dir/steps.sh" "$R" 2>&1 | tee "$OUT/run.log"
  rc=${PIPESTATUS[0]}
  [[ $R == "$OUT" ]] || cp -r "$R"/. "$OUT"/
  where="fresh WSL 2 distro $name ($(wsl.exe --version 2> /dev/null | tr -d '\0\r' | head -1))"
  if [[ $KEEP == 1 ]]; then echo "stranger_test: kept distro $name in $dir"
  else wsl.exe --unregister "$name" > /dev/null; rm -rf "$dir"; fi
fi
set -e
end=$(now)
{
  echo "mode $MODE: $where"
  echo "image $IMAGE $IMAGE_ID"
  echo "clone url $URL"
  echo "repository HEAD $HEAD_ID"
  echo "host $(nproc) cpus, load before $load0, after $(cut -d' ' -f1-3 /proc/loadavg)"
  cat "$OUT/env.txt" 2> /dev/null
  echo "steps (seconds, verdict, README line):"
  sed 's/^/  /' "$OUT/steps.tsv" 2> /dev/null
  cat "$OUT/readme-time.txt" 2> /dev/null
  cat "$OUT/post.txt" 2> /dev/null
  awk -v a="$start" -v b="$end" 'BEGIN { printf "environment_wall_s %.1f\n", b - a }'
  echo "exit $rc"
} > "$OUT/timing.txt"
echo
cat "$OUT/timing.txt"
s=$(awk '/^readme_wall_s/ { print $2 }' "$OUT/timing.txt")
if [[ $rc == 0 && -n $s ]] && awk -v s="$s" 'BEGIN { exit !(s < 900) }'; then
  echo "stranger_test: PASS ($MODE: the README in $s s, limit 900 s)"
else
  echo "stranger_test: FAIL ($MODE: exit $rc, README ${s:-?} s, limit 900 s)"; exit 1
fi
