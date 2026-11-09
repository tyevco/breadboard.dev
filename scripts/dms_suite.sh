#!/usr/bin/env bash
# Re-run the dial-me-spinner SPICE suite (hardware/sim) at the reference
# commit and write examples/dial-me-spinner/suite/results.json's format
# (Phase 1 plan M0; ADR-006 D9 rule 5).
#
#   scripts/dms_suite.sh --out <results.json> [--repo <dial-me-spinner>] [--commit <sha>]
#                        [--select '<pytest -m expr>'] [--jobs N]
#
# The suite runs from `git archive <commit>` in a scratch directory under
# $DMS_SUITE_CACHE (default ~/.cache/bb-dms-suite, off /mnt/c): the
# dial-me-spinner working tree is never read or written. It reads the
# committed .net and .kicad_pcb files there (and hardware/design, docs/);
# it needs no KiCad. ngspice is
# the system ngspice and must be 42 (as `make setup` installs). Python
# packages come from suite/requirements.txt in a uv venv (offline from uv's
# cache when possible).
#
# A pytest plugin (scripts/dms_suite_plugin.py) records every ngspice run, so
# results.json gives the numeric fallback level each deck used. Tests are
# distributed per file (--dist loadfile): the suite caches results in
# module globals, so per-file order keeps which test ran which deck fixed.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
REPO=${DMS_REPO:-$ROOT/../dial-me-spinner}
COMMIT=6854d25e0fde9b5d60da07b2c2b6183fd36dfc5a
SELECT="not slow"
JOBS=${DMS_SUITE_JOBS:-12}
OUT=
while [[ $# -gt 0 ]]; do
  case $1 in
    --out) OUT=$2; shift 2 ;;
    --repo) REPO=$2; shift 2 ;;
    --commit) COMMIT=$2; shift 2 ;;
    --select) SELECT=$2; shift 2 ;;
    --jobs) JOBS=$2; shift 2 ;;
    *) echo "dms-suite: unknown argument $1" >&2; exit 2 ;;
  esac
done
[[ -n $OUT ]] || { echo "usage: dms_suite.sh --out <results.json> [--repo R] [--commit C] [--select EXPR] [--jobs N]" >&2; exit 2; }
OUT=$(realpath -m "$OUT")

NGSPICE=$(command -v ngspice) || { echo "dms-suite: ngspice not found (make setup)" >&2; exit 1; }
NGV=$("$NGSPICE" -v 2>&1 | grep -o 'ngspice-[0-9][0-9.]*' | head -1)
[[ $NGV == ngspice-42 ]] || { echo "dms-suite: need ngspice 42, found '$NGV'" >&2; exit 1; }
NGPKG=$(dpkg-query -W -f '${Version}' ngspice 2>/dev/null || echo unknown)
NGKLU=$("$NGSPICE" -v 2>&1 | grep -q 'KLU' && echo true || echo false)

COMMIT=$(git -C "$REPO" rev-parse --verify "$COMMIT^{commit}")
TREE=$(git -C "$REPO" rev-parse "$COMMIT^{tree}")
SIMTREE=$(git -C "$REPO" rev-parse "$COMMIT:hardware/sim")

CACHE=${DMS_SUITE_CACHE:-$HOME/.cache/bb-dms-suite}
REQ=$ROOT/examples/dial-me-spinner/suite/requirements.txt
VENV=$CACHE/venv-$(sha256sum "$REQ" | cut -c1-12)
if [[ ! -x $VENV/bin/python ]]; then
  mkdir -p "$CACHE"
  uv venv -q --python python3.12 "$VENV.tmp"
  uv pip install -q --offline --python "$VENV.tmp/bin/python" -r "$REQ" 2>/dev/null \
    || uv pip install -q --python "$VENV.tmp/bin/python" -r "$REQ"
  mv "$VENV.tmp" "$VENV"
fi

RUN=$(mktemp -d "$CACHE/run.XXXXXX")
[[ ${KEEP:-0} == 1 ]] || trap 'rm -rf "$RUN"' EXIT
mkdir -p "$RUN/src" "$RUN/plugin" "$RUN/log"
# The whole tree: the suite also reads hardware/design, hardware/tools and docs/.
git -C "$REPO" archive "$COMMIT" | tar -x -C "$RUN/src"
cp "$ROOT/scripts/dms_suite_plugin.py" "$RUN/plugin/"

PY=$VENV/bin/python
"$PY" - "$RUN/meta.json" "$COMMIT" "$TREE" "$SIMTREE" "$NGV" "$NGPKG" "$NGKLU" "$SELECT" <<'EOF'
import json, sys, platform, importlib.metadata as md
out, commit, tree, simtree, ngv, ngpkg, klu, select = sys.argv[1:]
pk = ["pytest", "pytest-xdist", "numpy", "scipy", "execnet", "pluggy"]
meta = {
    "reference": {"repository": "dial-me-spinner", "commit": commit, "branch": "kicad10-port",
                  "tree": tree, "tree_hardware_sim": simtree,
                  "export": "git archive <commit> | tar -x (the whole tree)"},
    "tools": {"ngspice": ngv, "ngspice_package": ngpkg, "ngspice_klu": klu == "true",
              "python": platform.python_version(),
              "packages": {p: md.version(p) for p in pk}},
    "run": {"command": f"python -m pytest hardware/sim -q -ra -m '{select}' -n <jobs> --dist loadfile",
            "selection": select},
}
json.dump(meta, open(out, "w"), indent=1)
EOF

echo "dms-suite: $COMMIT ($NGV, $NGPKG), selection '$SELECT', $JOBS jobs, in $RUN"
start=$(date +%s)
set +e
(cd "$RUN/src" && BB_DMS_LOG=$RUN/log PYTHONPATH=$RUN/plugin PYTHONDONTWRITEBYTECODE=1 \
  "$PY" -m pytest hardware/sim -q -ra -m "$SELECT" -n "$JOBS" --dist loadfile \
  -p dms_suite_plugin -p no:cacheprovider) > "$RUN/pytest.log" 2>&1
rc=$?
set -e
tail -n 1 "$RUN/pytest.log"
echo "dms-suite: pytest exit $rc after $(( $(date +%s) - start )) s (load: $(cut -d' ' -f1-3 /proc/loadavg))"
cp "$RUN/pytest.log" "${OUT%.json}.pytest.log" 2>/dev/null || true

"$PY" -I "$ROOT/scripts/dms_suite_collect.py" --suite-results "$RUN/src/hardware/sim/results/results.json" \
  --log "$RUN/log" --meta "$RUN/meta.json" --out "$OUT"
exit $rc
