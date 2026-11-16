# `bb test`: `bb-test/0`, `test.json` and JUnit

Normative for `bb test` (ADR-010 D8-D9, Amendment 5; Phase 2 milestone B3).
A test file is TOML (`bb-test/0`, `spec/schema/test.schema.json`); `bb test`
runs its cases, each an ordinary `bb run`, and writes a canonical
`test.json` (`spec/schema/test-result.schema.json`) and a `junit.xml` (the
subset of `spec/schema/junit-subset.xsd`).

## 1. The file (`bb-test/0`)

```toml
schema = "bb-test/0"
project = ".."                        # a bb.toml project, or board = "<board.cir>"; per case too

[defaults]                            # run options every case takes unless it sets its own
for = "120ms"
assert_files = ["../asserts/e1.toml"]

[[case]]
name = "ok-cold"                      # letters, digits, _ . - ; unique; not ending in -ref
bench = "../bench/cold.toml"
targets = { "main:U10" = "../firmware/prebuilt/led_rail.elf" }
golden = "../../../tests/golden/dms-rail/ok-cold/outputs.sha256"

[[case]]
name = "wrong-cold"
bench = "../bench/cold.toml"
targets = { "main:U10" = "../firmware/prebuilt/led_rail_wrong_order.elf" }
expect = { "LSW-3" = "fail" }         # a negative control; every other assertion expected pass

[[case]]
name = "ok-cold-order"
bench = "../bench/cold.toml"
targets = { "main:U10" = "../firmware/prebuilt/led_rail.elf" }
engine_order = "analog-first"
same_as = "ok-cold"                   # byte-identical to that case

[[case.assert]]                       # an inline assertion (spec/project.md section 3.4)
name = "DIN-driven"
kind = "count"
net = "U10.P0.06"
min = 1
```

Every path is relative to the test file. Unknown keys are errors.

- **What a case runs.** `project` (a `bb.toml` project: `bb run
  --project`) or `board` (a board file: `bb run --netlist`), the case's own
  or else the file's, never both.
- **Run options** (in a case, or in `[defaults]`; a case's value wins, and a
  list replaces): `for` (required), `fidelity` (`fast` or `pi`; a project
  only), `bench` (a project only), `targets` (`{ "<instance>" = "<elf>" }`:
  for a project each becomes `bb run --image <instance>=<elf>`, in place of
  `[targets]`; a board file needs exactly one, which becomes `--firmware`
  and `--target`), `probes` (each `--probe net:<name>`; a board file needs
  at least one), `sample_rate`, `assert_files` (each `--assert`),
  `max_quantum`, `renode_quantum`, `engine_order`, `lookahead`, `adc_push`,
  `seed` and `allow_unqualified` (a project only), each the `bb run` option
  of that name.
- **Assertions.** A case's assertions are its bench's `[[assert]]`, its
  `assert_files` and its inline `[[case.assert]]` tables, which `bb test`
  writes as an assertion file `<out>/.bb-test/<case>.asserts.toml` for the
  run. `asserts` (optional) lists the names the case reports, each of which
  must give a record; without it, every `assert` record of the run is
  reported, in the run's order.
- **`expect`**: `{ "<name>" = "pass" | "fail" | "not_evaluable" }` for
  reported assertions; every other one is expected to pass. **Gate G9:** a
  case with an expected `fail` (a negative control) must report a `count` or
  `event` assertion expected to pass, showing that the controlled event
  occurred; otherwise the file is refused before anything runs.
- **`golden`**: a `sha256sum` file (`<hex>  <name>` lines). `nets.csv` and
  `nets.vcd` are compared with the run's files, `non-grant-records` with the
  run's `events.jsonl` without its `grant` and `assert` records (a golden
  is a run's records without assertions; the assertions are passive). An
  `events.jsonl` line is not compared (its grant records depend on the
  partition) and is listed as `not_checked`; any other name is an error.
- **`reference = true`**: the case is also run as its one-cell reference,
  `--max-quantum <L> --lookahead off` (L its `renode_quantum`, default
  10 µs), into `<out>/<case>-ref/`; the case's `nets.csv`, `nets.vcd` and
  records without its grant records (the `assert` records included) must
  equal the reference's. When no grant of the case is longer than one
  lattice cell, the partitions coincide and `test.json` calls the
  comparison `"consistency"`, otherwise `"equivalence"`.
- **`same_as = "<case>"`**: `nets.csv`, `nets.vcd` and `events.jsonl`
  byte-identical to that case's (an engine order, a pool). The named case
  may not have a `same_as` itself.

## 2. Running

- Every run is `bb run` of the same `bb`, in a child process, with its
  output in `<out>/<case>/` (`<out>/<case>-ref/` for a reference) and its
  stdout and stderr in `<out>/<case>.log`. A case directory is replaced.
  The children die with `bb test`.
- `--jobs N` runs at a time (default: the number of runs, at most 4).
- `--pool auto` (default): the warm Renode pool in `$BB_RENODE_POOL`, else
  `bb test` runs itself under `bb pool run --slots <jobs>`, which stops
  the pool afterwards; `env`: `$BB_RENODE_POOL` if set, else every Renode
  fresh; `off`: every run with `--renode-pool off`. Results never depend
  on leasing (ADR-004 Part B), so `test.json` does not either.
- **Exit status** 0 if and only if every case is ok: its run completed, every
  reported assertion's outcome equals its expectation (an unexpected pass
  of a negative control is a failure, and so is an assertion without a
  record), and each of its `golden`, `reference` and `same_as`
  comparisons holds. A file that does not parse, or breaks a rule of
  section 1, is refused before any run.

## 3. `test.json` (`bb-test-result/0`)

Canonical JSON (`spec/canonical-json.md`), deterministic: no wall time, no
path, no host. A pure function of the test file and the runs' outputs, so it
is byte-identical across runs (ADR-010 G4).

- `schema`, `file` (the test file's name), `sha256` (of its text), `ok`,
  `failed` (the names of the cases that are not ok, in the file's order),
  `cases`.
- Per case, in the file's order: `name`, `ok`, `run` (`"ok"` or
  `"failed"`); `asserts`, each reported assertion with `name`, `kind`,
  `expected`, `ok`, `outcome` (the record's, or `"missing"`) and, from the
  record, `t_fs`, `value`, `limit`, `unit`, `rung` and `msg`; `hashes`
  (`sha256:` of `events.jsonl`, `nets.csv`, `nets.vcd`, `run.json` where
  the run writes one, and `non-grant-records` as for a golden); `firmware`
  (the image hash, ADR-008 D5); and `golden` (`file`, `ok`, `checked`,
  `differs`, `not_checked`), `reference` (`kind`, `ok`, `differs`, the
  reference's `hashes`) and `same_as` (`case`, `ok`, `files`) for the
  comparisons the case asks for.

## 4. `junit.xml`

The Jenkins/Ant shape GitHub and GitLab reporters read, restricted to
`spec/schema/junit-subset.xsd`. It carries wall times (the `time`
attributes) and is not a deterministic output; `test.json` is.

- One `testsuites` (the test file's name, the totals), one `testsuite`
  per case (its run's wall time), in the file's order.
- `properties` of a case: `rung` (its records'), and for a project run from
  its `run.json`: `fidelity`, `deck` (the deck's sha256), `board_bbm`,
  `firmware` (the image hashes), `run_json` (its sha256), `agent_generated`
  (the views whose provenance source is `agent_generated`) and
  `unreviewed_views` (those `bb run` flags until a person reviews them:
  `agent_generated` or `estimated`, `spec/package.md` section 2), each a
  space-separated list of `<package>:<view>`; for a board file `rung` and
  `firmware`.
- One `testcase` (`classname` the case, `time` 0 except the run's) per
  case for the run itself (`run`: an `error` when it failed, so a reporter
  never drops a case), one per reported assertion and one per comparison
  (`golden`, `reference`, `same_as`).
- An assertion whose outcome differs from its expectation is a `failure`
  (`type` the outcome, `message` the outcome, the expectation and `value=…
  limit=… t=… fs`, text the record's `msg`); one without a record is a
  `failure` of type `missing`; an expected `not_evaluable` is `skipped`;
  otherwise the testcase has a `system-out` with the outcome and values. A
  comparison that does not hold is a `failure` of its own type.
