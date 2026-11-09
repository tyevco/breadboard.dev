# dial-me-spinner SPICE suite, re-run at the reference commit

`results.json` is the dial-me-spinner SPICE suite (`hardware/sim`, its
everyday subset `-m "not slow"`) re-run at the reference commit `6854d25`
(Phase 1 plan M0, approver decision C). The commit itself carries no results
(`hardware/sim/results/` is git-ignored). These are the expected values the
E1 package vectors cite (ADR-006 D9 rule 5).

Produced and checked with:

```sh
make dms-suite-check DMS_REPO=<dial-me-spinner repo>    # a fresh run must give these bytes
make dms-suite-update DMS_REPO=<dial-me-spinner repo>   # regenerate (review the diff)
```

`scripts/dms_suite.sh` exports the commit with `git archive` into
`~/.cache/bb-dms-suite` (the dial-me-spinner working tree is never read or
written), runs pytest there with ngspice 42 and the packages pinned in
`requirements.txt` (`--dist loadfile`, because the suite caches results in
module globals), and records every ngspice run through a pytest plugin
(`scripts/dms_suite_plugin.py`). `scripts/dms_suite_collect.py` writes
`results.json`. The suite needs no KiCad: it reads the committed `.net` and
`.kicad_pcb` files of the export.

## What `results.json` holds

| Key | Content |
| --- | --- |
| `reference` | the commit, its tree and `hardware/sim` tree ids, how it was exported |
| `tools` | ngspice version (`ngspice-42`), the Debian package version, KLU, Python and package versions |
| `run` | the pytest command and test selection |
| `summary` | test outcome counts, check counts, deck counts by fallback level, decks that hit a timeout |
| `fallback_levels` | what each level means |
| `outcomes` | each collected test's outcome (`passed`, `xfailed`, ...), in collection order |
| `decks` | every ngspice deck a test ran: test, title, a hash of the deck without its option lines, `fallback_level`, the options it finished with (`options`) and started with (`own_options`), and each attempt's result |
| `checks` | the suite's own `results.json`, unchanged: every `check()` with value, limits, unit, source and pass/fail, in collection order |

**Fallback level.** The suite retries a deck with looser numerics only on
"Timestep too small" or a hang (`spicelib.NUMERIC_FALLBACKS`, and the same
ladder in `test_load_switches.run()`): level 1 adds `reltol=1e-3`, level 2
also Gear and `rshunt=1e12`. Level 0 means the deck ran on its own options.
A deck may also start on looser options by itself; `own_options` shows that.

**`ok: false` checks are expected.** 49 belong to the 44 `xfailed` tests
(documented design issues); 16 are fault-injection checks that must fail
(`FAULT ...` inside `pytest.raises`).

## This run (6854d25, ngspice 42+ds-3build1)

- 718 passed, 44 xfailed (the count the dial-me-spinner commit message
  gives); 4 465 checks.
- 2 318 decks, 2 322 ngspice runs. 2 314 decks at fallback level 0, **4 at
  level 1** (all in `test_load_switches.py`: `test_fault_ct_bridged_to_vin`,
  `test_u2_qod_discharge[usb5.0]`,
  `test_u21_turn_on_dead_battery_usb_100ma_with_fw_rule[ILIM80min-SYSREG4.4-2pct]`,
  `test_bat_pin_vs_buvlo_at_firmware_floor[U21+motor--10]`), none at level 2,
  none after a timeout.
- About 3 minutes on 12 jobs (24 cores).

**Limit.** A retry after a hang depends on wall time (90 s per attempt in
`test_load_switches.py`), so a heavily loaded host could move a deck to a
higher level and change these bytes. No deck timed out in the M0 runs; the
`decks_with_timeouts` count says when one does.
