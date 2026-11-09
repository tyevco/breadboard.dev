"""pytest plugin for scripts/dms_suite.sh: records every ngspice run of the dial-me-spinner suite.

Loaded with `-p dms_suite_plugin` from a directory that holds only this file. It does not change what
the suite computes: it wraps `spicelib.Deck._run_once` (the one place every deck reaches ngspice, also
through `Deck.run(fallback=True)` and `test_load_switches.run()`'s ladder) and records, per call, the
test, the deck title, a hash of the deck without its option lines (so the retries of one deck share
it), the option lines used and the result, plus a per-process id of the Deck object (`obj`, used
only to group the attempts of one deck; never written to results.json). It also records each
test's outcome.

Each pytest process writes `$BB_DMS_LOG/<worker>.json` at session end; scripts/dms_suite_collect.py
merges them. Nothing here depends on wall time, process ids or paths.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import subprocess
import sys
import threading

import pytest

_LOCK = threading.Lock()
_CALLS: list = []
_OUTCOMES: dict = {}
_CURRENT = {"test": None}
_SEQ = [0]
_PATCHED = [False]


def _worker(config) -> str:
    wi = getattr(config, "workerinput", None)
    return wi["workerid"] if wi is not None else "main"


def _patch():
    if _PATCHED[0]:
        return
    spicelib = sys.modules.get("spicelib")
    if spicelib is None:
        raise RuntimeError("dms_suite_plugin: spicelib was not imported by the suite's conftest")
    orig = spicelib.Deck._run_once

    def _run_once(self, timeout, keep):
        text = self.text()
        body = "\n".join(ln for ln in text.splitlines() if not ln.lstrip().lower().startswith(".option"))
        with _LOCK:
            obj = getattr(self, "_dms_obj", None)
            if obj is None:
                _SEQ[0] += 1
                obj = self._dms_obj = _SEQ[0]
        rec = dict(test=_CURRENT["test"], title=self.title, obj=obj,
                   deck=hashlib.sha256(body.encode("utf-8")).hexdigest()[:16],
                   options=list(self.options), timeout=timeout)
        try:
            res = orig(self, timeout, keep)
        except subprocess.TimeoutExpired:
            rec["result"] = "timeout"
            _record(rec)
            raise
        errs = res.get("_errors", [])
        if any(re.search(r"(?i)timestep too small", e) for e in errs):
            rec["result"] = "timestep_too_small"
        elif errs:
            rec["result"] = "errors_reported"
        else:
            rec["result"] = "ok"
        _record(rec)
        return res

    spicelib.Deck._run_once = _run_once
    _PATCHED[0] = True


def _record(rec):
    with _LOCK:
        _SEQ[0] += 1
        rec["seq"] = _SEQ[0]
        _CALLS.append(rec)


def pytest_collection_finish(session):
    _patch()
    session.config._dms_items = [it.nodeid for it in session.items]


@pytest.hookimpl(hookwrapper=True)
def pytest_runtest_protocol(item, nextitem):
    _CURRENT["test"] = item.nodeid
    yield
    _CURRENT["test"] = None


def pytest_runtest_logreport(report):
    # In an xdist run the controller sees every worker's reports; in a serial run the one process does.
    nid = report.nodeid
    wasx = hasattr(report, "wasxfail")
    if report.when == "call":
        if report.passed:
            _OUTCOMES[nid] = "xpassed" if wasx else "passed"
        elif report.skipped:
            _OUTCOMES[nid] = "xfailed" if wasx else "skipped"
        else:
            _OUTCOMES[nid] = "failed"
    elif report.failed:
        _OUTCOMES[nid] = "error"
    elif report.skipped and report.when == "setup":
        _OUTCOMES[nid] = "xfailed" if wasx else "skipped"


def pytest_sessionfinish(session, exitstatus):
    out = os.environ.get("BB_DMS_LOG")
    if not out:
        return
    os.makedirs(out, exist_ok=True)
    config = session.config
    part = dict(worker=_worker(config), items=getattr(config, "_dms_items", []), calls=_CALLS)
    if getattr(config, "workerinput", None) is None:
        part["outcomes"] = _OUTCOMES
        part["exitstatus"] = int(exitstatus)
    with open(os.path.join(out, f"{part['worker']}.json"), "w", encoding="utf-8") as f:
        json.dump(part, f, indent=1)
