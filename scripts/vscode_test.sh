#!/usr/bin/env bash
# Phase 2 E4 (ADR-016 D2, G2; Amendment 1): the committed VS Code templates
# (docs/templates/vscode/{tasks,launch}.json) attach cortex-debug to
# `bb run --gdb` inside a real, headless VS Code, and a session that only
# attaches, reads and detaches leaves the outputs byte-identical.
#
#   scripts/vscode_test.sh <bb> <out> <work>          (make vscode-test)
#   scripts/vscode_test.sh --static                   (make vscode-templates, in make test)
#
# --static checks the templates alone (scripts/vscode_templates.py): the
# settings ADR-016 D2 names, the task's port equal to gdbTarget, the
# preLaunchTask naming the task, and the endsPattern matching the line bb
# prints when its proxy listens (`cli/src/main.rs`).
#
# The full test:
#   1. fetches, once, a pinned VS Code (VSCODE_VERSION, linux-x64) and the
#      pinned cortex-debug 1.12.1 with its four mcu-debug dependencies from
#      Open VSX into $BB_CACHE_DIR/vscode-test (default ~/.cache/bb), each
#      checked against its sha256 below, and unpacks the extensions into a
#      private extensions directory (no marketplace call, no `code
#      --install-extension`);
#   2. copies the dial-me-spinner project with led_rail.elf recorded
#      (`dms_rail.sh prepare`, as `make gdb-test` does) and the templates,
#      byte for byte, into its .vscode/;
#   3. runs VS Code under Xvfb in a network namespace of its own (only
#      loopback: nothing can reach the network, and port 3333 is private),
#      with telemetry and updates off, workspace trust disabled, and
#      scripts/vscode_test/runner.js as the extension test. The runner
#      starts the launch configuration by name, as the Run and Debug view
#      does: VS Code runs the preLaunchTask (bb run --gdb 3333 ...), waits
#      for its problem matcher's endsPattern, then cortex-debug starts
#      gdb-multiarch and attaches;
#   4. `attach`: the stop after the attach, the stack, every scope's
#      variables (the core registers) and the word at $sp read, the session
#      stopped (cortex-debug deletes its breakpoints and detaches), the run
#      finished: exit code 0, bb's summary "debugger sessions 1, no stop
#      inside a slice and no write", no debug-run warn record, and all four
#      hashes equal to tests/golden/dms-rail/ok-cold/outputs.sha256 (the
#      run without GDB);
#   5. `break`, the negative control (the comparison and the GDB path
#      acted): a function breakpoint at gpio_nrfx_port_set_bits_raw hit
#      through cortex-debug marks the run (exactly one "debug run: GDB"
#      warn record, run.json "debug": true) and its outputs differ from
#      the golden.
# A bb shim on PATH only tees bb's stderr into <out>/<mode>/bb.log and
# records its argv: the templates call `bb` as written.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
PY=${PYTHON:-python3}

if [[ ${1:-} == --static ]]; then
  exec "$PY" "$ROOT/scripts/vscode_templates.py" --root "$ROOT"
fi

[[ $# == 3 ]] || { sed -n '2,45p' "$0"; exit 2; }
BB=$(realpath "$1")
OUT=$(mkdir -p "$2" && realpath "$2")
WORK=$(mkdir -p "$3" && realpath "$3")

VSCODE_VERSION=1.145.3
VSCODE_SHA256=567e067c3ce043605af1b466e916173193100cf8c7da911b85e4c5c0869fe621
# <publisher>.<name> <version> <sha256> (Open VSX)
EXTENSIONS=(
  "marus25.cortex-debug 1.12.1 f315306b5d964c1e40d2802fdf8aa2ec86c62c0a73477d46964666a8381da71f"
  "mcu-debug.debug-tracker-vscode 0.0.15 0bf99ffb60dc82d8d2e5f27a64dde7b54fc292236902edaaefccdbb1675bfd3e"
  "mcu-debug.memory-view 0.0.29 3a874aa4f88481f56edc83fd8116525f8e589c00f0f57f6bb16a6304a11932bd"
  "mcu-debug.rtos-views 0.0.18 1f7ba752c4dad5b58898ae9bd077a9876d88a23384966553174755a2747151e7"
  "mcu-debug.peripheral-viewer 1.6.5 552b152466ea1156c7d18f316efa8ce311cd745ae5e43d7b21d033d616f186f6"
)
GDB=${GDB:-gdb-multiarch}
BREAK_AT=gpio_nrfx_port_set_bits_raw
GOLDEN=$ROOT/tests/golden/dms-rail/ok-cold/outputs.sha256
CACHE=${BB_CACHE_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/bb}/vscode-test

fail() { echo "vscode-test: FAIL: $*"; exit 1; }
pass() { echo "vscode-test: PASS: $*"; }

command -v "$GDB" > /dev/null || fail "no $GDB (make setup installs gdb-multiarch)"
command -v xvfb-run > /dev/null || fail "no xvfb-run (apt install xvfb)"
command -v unshare > /dev/null || fail "no unshare (util-linux)"

"$PY" "$ROOT/scripts/vscode_templates.py" --root "$ROOT" || fail "the templates' static check"

fetch() { # <url> <file> <sha256>
  local url=$1 f=$2 sum=$3
  if [[ ! -f $f ]] || ! echo "$sum  $f" | sha256sum -c --quiet - 2> /dev/null; then
    echo "vscode-test: fetching $url"
    curl -fsSL --retry 3 -o "$f.part" "$url"
    echo "$sum  $f.part" | sha256sum -c --quiet - || fail "sha256 of $url"
    mv "$f.part" "$f"
  fi
}

mkdir -p "$CACHE"
fetch "https://update.code.visualstudio.com/$VSCODE_VERSION/linux-x64/stable" \
  "$CACHE/vscode-$VSCODE_VERSION-linux-x64.tar.gz" "$VSCODE_SHA256"
VSC=$CACHE/vscode-$VSCODE_VERSION
if [[ ! -x $VSC/code ]]; then
  rm -rf "$VSC.tmp" && mkdir -p "$VSC.tmp"
  tar xzf "$CACHE/vscode-$VSCODE_VERSION-linux-x64.tar.gz" -C "$VSC.tmp" --strip-components=1
  rm -rf "$VSC" && mv "$VSC.tmp" "$VSC"
fi
EXT=$WORK/extensions
rm -rf "$EXT" && mkdir -p "$EXT"
for e in "${EXTENSIONS[@]}"; do
  read -r id ver sum <<< "$e"
  pub=${id%%.*} name=${id#*.}
  fetch "https://open-vsx.org/api/$pub/$name/$ver/file/$id-$ver.vsix" "$CACHE/$id-$ver.vsix" "$sum"
  "$PY" - "$CACHE/$id-$ver.vsix" "$EXT/$id-$ver" << 'EOF'
import sys, zipfile, shutil, pathlib
src, dst = sys.argv[1], pathlib.Path(sys.argv[2])
tmp = dst.with_name(dst.name + ".tmp")
with zipfile.ZipFile(src) as z:
    for m in z.namelist():
        if m.startswith("extension/") and not m.endswith("/"):
            p = tmp / m[len("extension/"):]
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_bytes(z.read(m))
shutil.move(str(tmp), str(dst))
EOF
done
echo "vscode-test: VS Code $(DONT_PROMPT_WSL_INSTALL=1 "$VSC/bin/code" --version < /dev/null | head -1), extensions: $(ls "$EXT" | tr '\n' ' ')"

DMS_RAIL_WORK=$WORK bash "$ROOT/scripts/dms_rail.sh" prepare "$BB"

mkdir -p "$WORK/bin"
cat > "$WORK/bin/bb" << EOF
#!/usr/bin/env bash
# Test shim (scripts/vscode_test.sh): records the argv, tees stderr, runs bb.
printf '%s\n' "\$@" > "\$BB_VSC_DIR/bb.argv"
# The shim stays the task's process until tee has written bb's last line
# (with exec and a process substitution, tee died with the terminal first).
"$BB" "\$@" 2>&1 | tee -a "\$BB_VSC_DIR/bb.log"
exit \${PIPESTATUS[0]}
EOF
chmod +x "$WORK/bin/bb"
CONFIG=$("$PY" "$ROOT/scripts/vscode_templates.py" --root "$ROOT" --print-config-name)

run_mode() { # <mode>
  local mode=$1 proj=$WORK/vsc-$1 dir=$OUT/$1
  rm -rf "$proj" "$dir" && mkdir -p "$dir"
  cp -r "$WORK/proj-ok" "$proj"
  mkdir -p "$proj/.vscode"
  cp "$ROOT/docs/templates/vscode/tasks.json" "$ROOT/docs/templates/vscode/launch.json" "$proj/.vscode/"
  local ud=$WORK/user-data-$mode
  rm -rf "$ud" && mkdir -p "$ud/User"
  cat > "$ud/User/settings.json" << 'EOF'
{
  "telemetry.telemetryLevel": "off",
  "update.mode": "none",
  "extensions.autoCheckUpdates": false,
  "extensions.autoUpdate": false,
  "workbench.enableExperiments": false,
  "workbench.startupEditor": "none",
  "security.workspace.trust.enabled": false,
  "terminal.integrated.inheritEnv": true
}
EOF
  local t0=$SECONDS rc=0
  # A network namespace of our own: loopback up, then back to our own uid.
  BB_VSC_DIR=$dir BB_VSC_RESULT=$dir/result.json BB_VSC_MODE=$mode BB_VSC_CONFIG=$CONFIG \
  BB_VSC_BREAK=$BREAK_AT PATH="$WORK/bin:$(dirname "$(command -v "$GDB")"):$PATH" \
  timeout 600 unshare -rn sh -c \
    'u=$0 g=$1; shift; ip link set lo up && exec unshare --user --map-user="$u" --map-group="$g" "$@"' \
    "$(id -u)" "$(id -g)" \
    xvfb-run -a -s "-screen 0 1280x1024x24 -nolisten tcp" \
    "$VSC/code" --no-sandbox --disable-gpu --disable-workspace-trust --skip-welcome \
      --skip-release-notes --disable-telemetry --user-data-dir "$ud" --extensions-dir "$EXT" \
      --extensionDevelopmentPath="$ROOT/scripts/vscode_test" \
      --extensionTestsPath="$ROOT/scripts/vscode_test/runner.js" "$proj" \
    > "$dir/vscode.log" 2>&1 || rc=$?
  echo "vscode-test: $mode: VS Code exited $rc after $((SECONDS - t0)) s (log $dir/vscode.log)"
  if [[ -d $proj/out/debug ]]; then cp -r "$proj/out/debug" "$dir/run"; fi
  "$PY" "$ROOT/scripts/vscode_templates.py" --root "$ROOT" --check-run "$mode" --dir "$dir" \
    --golden "$GOLDEN" --vscode-rc "$rc" --project "$proj"
}

status=0
for m in ${VSCODE_TEST_MODES:-attach break}; do run_mode "$m" || status=1; done
if [[ $status == 0 ]]; then echo "vscode-test: PASS"; else echo "vscode-test: FAIL"; fi
exit $status
