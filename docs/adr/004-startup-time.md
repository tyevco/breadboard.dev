# ADR-004: Engine startup time (Renode runtime settings and a warm Renode pool)

- **Status:** Accepted (2026-11-08), with the approver decisions below.
  Part A and Part B are approved; ReadyToRun is not.
- **Scope:** engine-internal (`engines/cpu-renode`), the `bb` CLI (a new
  `bb pool` subcommand, a `--renode-pool` option) and the Makefile. It
  changes no file in `spec/`, nothing in the kernel time-grant protocol,
  no output format, and no golden or partition hash.
- **Evidence:** profiling and prototypes of 2026-11-08 in a scratch clone
  (`~/bbscratch-startup/repo` at `dcc9d66`, instrumented with
  `~/bbscratch-startup/prototype.diff`; tools and raw audit output in
  `~/bbscratch-startup/tools`). Renode facts were checked against the
  installed Renode 1.17.0 (`~/.local/opt/renode-1.17.0`) and the upstream
  sources it was built from (`renode` tag `v1.17.0`, `renode-infrastructure`
  at `066a7f13`).

## Approver decisions (2026-11-08)

Recorded after the human approver reviewed this ADR and ADR-005.

1. **Part A is approved:** `DOTNET_TieredPGO=0` on the Renode child and
   the 5 ms External Control connect poll.
2. **Part B is approved:** the opt-in warm Renode pool (`bb pool run`).
3. **ReadyToRun (open question 1) is not approved.** Nothing downloads or
   runs crossgen2. It stays deferred, unmeasured.
4. **Pool discovery (open question 2):** only through `BB_RENODE_POOL`,
   as proposed. No well-known socket.
5. **External Control binding (open question 3):** find out whether
   Renode 1.17.0 can bind the server to loopback only and use that if it
   can; if not, document the exposure.
6. **The lingering listener (open question 4):** find and fix the cause
   in step 3.
7. **Orphans:** the orphaned Renode processes have been killed. The pool
   and the engines must never leave orphans (kill on parent death,
   process-group cleanup), with a test.

## Context

After ADR-003, simulation is no longer the main cost of a typical `bb run`.
At 500 ms simulated, the spike spends about 1 s simulating and about 3 s
starting its engines. The feedback board is the same. Every `make test`
target, every determinism run and every gate script pays that startup
again, once per `bb run` and once per Renode test in `cargo test`.

## Measurement method

- **Host.** WSL2 Ubuntu 24.04, 24 logical CPUs (i7-14650HX). The host was
  never idle. The load average was 2.4 to 7. Another agent was running tests
  at the same time, and three orphaned Renode processes from earlier sessions
  were alive (one of them using a full core). They were left alone because
  they do not belong to this task. All absolute numbers carry that noise, so
  every comparison below is interleaved A/B with medians, and the decisions
  rest on differences measured in the same session.
- **`bb` stages.** The scratch build adds `BB_PROFILE=1` timestamps inside
  `RenodeEngine::start` (spawned, External Control connected, setup done)
  and `bb run` (Renode start, `NgspiceEngine::new`, teardown). Script:
  `tools/bbprof.py`.
- **Renode stages.** `tools/prof_markers.py` runs Renode with the same
  generated script plus `log "BBMARK <stage>" 2` lines between the commands.
  `log` costs about 5 ms. Stages are read from Renode's own timestamped log
  lines ("Loaded monitor commands", "Including script(s)", the markers),
  and the server's `listen` is observed by polling `connect` every 2 ms. An
  earlier attempt used `python "..."` markers. It added about 0.6 s, because
  the first `python` command compiles in IronPython, so those numbers were
  thrown away.
- **Other tools.** `strace -f --seccomp-bpf` (file, socket and exec
  timeline), `perf record` (CPU by DSO and thread), and `tools/r2r_check.py`
  (which DLLs are ReadyToRun).
- **Outputs.** Every timed run hashes `nets.csv`, `nets.vcd`, `events.jsonl`
  and the non-grant records. The pool runs compare them with a fresh run of
  the same configuration and with the committed hashes.

## Profile

### Stages of one `bb run` (fresh process, page cache warm)

Medians over N runs. For a Renode stage the time is measured from the
previous mark.

| Stage | spike | feedback | How measured |
| --- | --- | --- | --- |
| `bb` before engines (args, board file, output dir) | < 5 ms | < 5 ms | process wall minus `bb`'s own total |
| Spawn of the Renode launcher | 1 ms | 1 ms | `BB_PROFILE` |
| .NET host + CoreCLR init up to `Renode.dll` load | ~0.09 s | ~0.09 s | strace, 1 run (inflated by tracing) |
| Renode init: `TypeManager` Cecil scan of the install dir (~140 non-System assemblies, 273 DLL opens), plugin manager, IronPython engine, `monitor.py`. Includes the row above | 1.39–1.95 s | 1.39–1.95 s | log markers, N = 7–9, three sessions |
| Monitor ready ("Including script(s)") | 0.15–0.18 s | 0.15–0.18 s | log markers |
| `mach create` | 0.02 s | 0.02 s | log markers |
| `machine LoadPlatformDescription` (repl parse, peripheral construction, tlib copy + `dlopen` + init) | 0.82–1.10 s | 0.82–1.10 s | log markers |
| Lookahead plugin: cached `ScanFile` + map | — | 0.035–0.045 s | log markers |
| `sysbus LoadELF` | 0.06 s | 0.06 s | log markers |
| `SetSeed`, `SetGlobalQuantum`, `SetGlobalSerialExecution`, `SetGlobalAdvanceImmediately` | 0.11–0.16 s | 0.11–0.16 s | log markers |
| `sysbus.bbLookahead Arm` | — | 0.02–0.04 s | log markers |
| `CreateExternalControlServer` until `listen` | 0.01 s | 0.01–0.03 s | log marker → polled connect |
| `bb`'s connect poll (100 ms sleep between tries) | 0–100 ms, ~50 ms average | same | `BB_POLL_MS=5` A/B below |
| External Control handshake, pins, `GET_TIME`, lookahead startup read | 0.016 s | 0.022 s | `BB_PROFILE` |
| `NgspiceEngine::new` (dlopen, `ngSpice_Init`, netlist) | 0.003 s (0.021 s on a cold first `dlopen`) | 0.003 s | `BB_PROFILE` |
| **Engine startup as `bb` reports it** | **2.93–3.33 s** | **2.93–3.44 s** | N = 5–7 per session |
| First grant (ngspice operating point + Renode's first `RUN_FOR`). Counted in simulation | 0.05–0.10 s | 0.05–0.10 s | `--for 10us`, N = 3 |
| Teardown (kill Renode, release ngspice) | 0.03 s | 0.03 s | `BB_PROFILE` |
| Cold page cache (`drop_caches` before the run) | +0.4 to +0.65 s | | 2 runs |

The ADC board uses the stock `stm32f072.repl`. Its fresh engine startup is
2.83–3.04 s (N = 3), the same profile.

### Where the time goes

- **Almost all of it is the Renode process.** 98 % of engine startup is
  the time from spawning Renode to its External Control server listening.
  ngspice (3 ms), the External Control setup (16–22 ms) and `bb`'s own work
  (< 5 ms) do not matter. The monitor script runs inside Renode, so it has
  no "monitor round trips" to save. Every command except the platform load
  costs ≤ 0.16 s.
- **CPU (`perf`, one run, whole process).** libcoreclr 44 % (type loading,
  metadata, stubs), libclrjit 26 %, JIT-compiled code 18 %, CoreLib 4 %,
  libc 2.5 %, the tlib translation library 0.7 %. Two threads carry 97 %,
  one after the other: `renode` (init, 51 %) and `Shell thread` (the
  script, 46 %). Startup is serial, single-core JIT and type-loading work.
- **ReadyToRun.** 87 DLLs in the install (51 MB, the .NET framework) are
  ReadyToRun. 145 DLLs (44 MB) are IL only, Renode's own among them:
  `Infrastructure.dll` 9.5 MB, `Renode.dll`, IronPython, Mono.Cecil,
  Migrant. Turning ReadyToRun off for the framework
  (`DOTNET_ReadyToRun=0`) costs +0.74 s (2.86 → 3.60 s, N = 5). So JIT is
  the lever, and everything Renode-specific is JIT-compiled on every start.
- **`TypeManager`** (upstream `src/Emulator/Main/Utilities/TypeManager.cs`).
  Its static constructor always scans the executable's directory
  (`*.dll`, `*.exe`, with a blacklist of `System.`/`Microsoft.` names) with
  Mono.Cecil. It keeps no persistent cache and reads no setting or
  environment variable that could skip the scan.

## Options evaluated

All the "Δ" values are same-session interleaved medians. "Hashes" means
that every run of the option gave the committed hashes.

| Option | Measurement | Δ engine startup | Verdict |
| --- | --- | --- | --- |
| **A1. `DOTNET_TieredPGO=0` in Renode's environment** | Renode only, feedback, N = 7: 2.56 → 2.14 s. `bb`, N = 7: spike 3.33 → 2.87 s, feedback 3.44 → 2.90 s (with A2). Simulation not slower: spike 0.92 → 0.80 s, one-cell spike (50 000 grants) 26.55 → 25.19 s (N = 3). Hashes identical (1 distinct set per board over all configs) | **−0.42 to −0.54 s (−14 to −16 %)** | **Accept** |
| **A2. Connect poll 100 ms → 5 ms** | `bb`, N = 5–7: −0.08 to −0.14 s in the medians (noisy). In the pool it is what stands between 0.21 s and 0.13 s | −0.05 to −0.1 s | **Accept** |
| `DOTNET_TieredCompilation=0` | 2.86 → 3.36 s | +0.50 s | Reject |
| `DOTNET_TC_QuickJitForLoops=0` | 2.86 → 2.91 s | +0.05 s | Reject |
| `DOTNET_gcConcurrent=0`, `DOTNET_GCgen0size=64 MB`, `DOTNET_TC_CallCountThreshold=100` (each on top of A1) | 2.14 → 2.34 / 2.30 / 2.22 s | none | Reject |
| Trim the generated `.resc` | All commands except the platform total 0.25 s, and each one is needed for determinism or the pins. There are no round trips (the script runs inside Renode) | ≤ 0.05 s possible | Reject |
| Start ngspice concurrently with Renode | `NgspiceEngine::new` takes 3 ms. Its first operating point needs the inputs at t = 0, so it cannot run ahead | ≤ 0.02 s | Reject |
| More Renode flags | `bb` already runs `--disable-xwt --plain --port -1` with no Robot and no extra loggers. 1.17.0 has no flag that skips the `TypeManager` scan (upstream source) | 0 | Nothing to do |
| Machine snapshot (`Save`/`Load`) in a fresh process | `Load` of the saved spike machine (1.07 MB) takes 1.11–1.20 s against 0.8–1.3 s to build it, and the process start stays (process wall 2.78–2.95 s, N = 3) | 0 | Reject (it also serializes tlib state, a new determinism surface for no gain) |
| ReadyToRun images of Renode's IL-only assemblies (crossgen2 8.0.12) | **Not measured.** Running the crossgen2 NuGet package (downloaded from nuget.org) was refused by this session's permission policy. Ceiling estimate: libclrjit's 26 % of CPU plus part of libcoreclr's 44 % | unknown, plausibly −0.5 to −1 s | Deferred (open question 1) |
| Trimmed install view (a symlink farm without GUI/unused assemblies, to shrink the `TypeManager` scan) | Not measured. Risks missing dependencies | unknown | Deferred |
| **B. Warm Renode pool** (one process serves many runs; `Clear` + the same script per run) | Prototype (below): engine startup **0.13 s** (spike and feedback, N = 7, with A1 + A2), 0.21 s with the 100 ms poll. Total wall spike 3.66 → 0.66 s, feedback 3.89 → 0.83 s. Simulation is faster too: spike 0.76 → 0.52 s, feedback 0.91 → 0.69 s, because the C# grant path is already tier-1. 50 pool runs over 10 configurations byte-identical to fresh runs and to the committed hashes | **−2.7 to −2.8 s (−95 %)** | **Accept** (opt-in, then the default for the test suite) |
| Pre-spawned single-use processes (spawn ahead, use once) | The process still JIT-compiles the platform load on its run: the first run in a new process takes 1.16–1.43 s against 0.09–0.21 s later | −1.5 s at best | Reject in favour of B (same isolation argument, a fifth of the gain) |

## Decision

### Part A: every fresh `bb run` (no daemon, always on)

1. `RenodeEngine::start` sets `DOTNET_TieredPGO=0` on the Renode child's
   environment (`Command::env`, so it overrides the user's environment for
   that child only; nothing else reads it). This removes the tier-0
   instrumentation .NET 8 adds to every method that tiers up. That
   instrumentation is pure startup overhead for Renode, whose hot loop is
   native tlib code.
2. The External Control connect poll sleeps 5 ms instead of 100 ms between
   tries. The 500 ms `connect_timeout`, the self-connect guard and the
   startup deadline are unchanged.

Expected: fresh engine startup −0.45 to −0.55 s (−14 to −16 %), with no
other change in behaviour.

### Part B: a warm Renode pool (opt-in per process tree)

- **`bb pool run [--slots N] [--max-runs K] -- <command>`** starts a pool
  manager (a `bb` process), runs `<command>` with
  `BB_RENODE_POOL=<socket>` in its environment, and stops the pool when
  the command exits. It returns the command's exit status. This is how
  `make test` and the gate scripts use the pool: no daemon outlives the
  command. `bb pool start` / `bb pool stop` / `bb pool status` give the
  same thing for interactive use. `start` prints the `export` line, and
  nothing finds the pool unless `BB_RENODE_POOL` is set.
- **The manager** owns N Renode processes, started as
  `renode --disable-xwt --plain --console --port -1` with A1 applied. Each
  is driven through its **console pipe** (stdin/stdout), not a TCP monitor
  port: the prototype shows that the console accepts `Clear` and `include`
  the same way, and no new network-reachable monitor (which would allow
  arbitrary code through `python`) is opened. `bb run` talks to the
  manager over a Unix socket in a 0700 directory
  (`$XDG_RUNTIME_DIR/bb/` or `$BB_CACHE_DIR`).
- **Pre-warm.** At spawn, each process scans the cached lookahead assembly
  once, then builds and runs a throwaway machine of each platform `bb`
  knows (the offline STM32F103 and the stock STM32F072) for a few lattice
  cells, then sends `Clear`. Every lease therefore sees the same type
  registry, and the first lease is warm. In the prototype a second machine
  in the same process loaded in 0.20–0.22 s against 1.31 s for the first
  (`tools/warm_reload.sh`).
- **Lease** (one run per lease, one lease per process at a time):
  1. `bb run` asks the manager for a process that matches the canonical
     Renode path and the plugin cache key. If none is idle, or there is no
     pool, `bb run` spawns a fresh Renode exactly as today. It never waits
     on a busy pool, so parallel `make` jobs keep working.
  2. The manager writes `Clear`, `logLevel <Renode's default>` and
     `include @<workdir>/session.resc` to the console. The script is the
     same one a fresh run uses. In pool mode the plugin `include` line is
     left out, because the assembly was pre-scanned; `machine
     LoadPlatformDescriptionFromString` and `Arm` stay. Readiness is the
     console prompt after the `include`, followed by the External Control
     connect (no fixed sleep).
  3. From there the engine is unchanged: the same handshake, the
     `GET_TIME == 0` check, pin registration, lookahead startup read,
     `RUN_FOR` on the lattice and verification. The manager copies the
     process's console output into the lease's `renode.log` in the run's
     workdir, so failure diagnostics work as before.
  4. At the end of the lease (the `bb run` socket closes) the manager sends
     `Clear` at once. **No machine exists in a pool process between
     leases.** The pool is not an engine. It holds no simulated time, so
     invariant 3 ("no engine free-runs") is untouched.
  5. **Discard rules.** The manager kills a process and spawns a fresh,
     pre-warmed one in its place when: the lease ended with an error (any
     `EngineError`, a broken promise, a protocol error, or `bb` dying); the
     process exits or its console stops answering; the process has served
     `--max-runs` leases (default 100); or its RSS exceeds a limit (default
     2 GiB). The prototype process sat at 398 MB after 10 runs. A process
     is never reused after a failure.

## Determinism argument

The claim: a run served by a pool process gives byte-identical outputs to
a run in a fresh process. This section shows why, and then lists the
checks.

1. **Machine-scoped state is new on every lease.** `Clear` is
   `EmulationManager.Clear()` → `CurrentEmulation = GetNewEmulation()`.
   The setter disposes the old `Emulation` (`PauseAll`, externals
   including the External Control server, backends, every machine, the
   `MasterTimeSource`, the per-emulation logger and the PRNG) and installs
   a new `Emulation`. `Emulation` has no static fields (upstream
   `Emulation.cs`). The machine, its peripherals, the time sources and the
   External Control server are built from scratch by the same script.
2. **Native CPU state is new.** Each `TranslationCPU` loads its own
   temporary copy of `translate-arm-m-le.so`. After 10 machines in one
   process, exactly one copy was mapped (`/proc/<pid>/maps`): old copies
   are unloaded with their machine. The tlib globals therefore start fresh
   on every machine.
3. **Process-wide state is inventoried.** A reflection audit inside Renode
   (`tools/audit_statics.py`, output `tools/statics.tsv`) lists every
   mutable static field of `Infrastructure` and `Renode`: 99 fields. Most
   belong to GUI, video, network or SiLabs code that these boards never
   load. The ones in the code these boards run, with every IL reader
   (Mono.Cecil scan, `tools/audit_refs.py`, output `tools/refs.tsv`):
   - `PseudorandomNumberGenerator.baseSeed`. In a fresh process it is
     random (`new Random().Next()`) until `emulation SetSeed`, which
     resets it and clears the per-thread generators. A draw before
     `SetSeed` would already make fresh runs nondeterministic, and they
     are not. After `SetSeed 0` the state is the same in both cases
     (measured: `baseSeed` is 0 after each `SetSeed`).
   - `IdentifiableObject.IdCounter` keeps counting (0 → 2 → 4 across two
     machines). Its only reader is `PeripheralAccessProfiler.LogStart`
     (profiling, off). Object IDs reach no result.
   - `EmulationManager.EmulationEpoch` is read only by snapshot and
     preservable bookkeeping (`Load`, `PreservableManager`,
     `AutoSnapshotCreator`).
   - `Logger.minLevel` and `nextEntryId` affect log output only. The lease
     resets the level to the fresh-process default anyway.
   - The `TypeManager` registry only grows. Pre-warm makes it the same for
     every lease, the lookahead type included.
   - IronPython globals from the plugin loader (`BB_*` names) are not read
     in pool mode.
4. **Codegen state is not a new variable.** Which methods run as tier-0,
   tier-1, OSR or (with A1) un-instrumented code already depends on host
   timing in every fresh run: the tier-up thread works by call counts and
   a timer. The determinism gates (`make determinism` at 5 parallel jobs)
   pass under that variation, so results do not depend on the codegen
   tier. A pool process is the same IL at a later tier, and A1 is one more
   such tier choice.
5. **Hash codes.** String hashing is already randomized per process in
   .NET. Object hash codes continue across leases. Neither reaches a
   result: .NET `Dictionary`/`HashSet` enumerate in entry order, which
   depends on the insert and remove sequence, not on hash values. A result
   that depended on hash values would already break fresh-run determinism
   for strings.
6. **What stays the same in both modes:** the script, the ELF, the
   platform, the seed, the quantum, serial execution, advance-immediately,
   and lattice-aligned `RUN_FOR`. ADR-003's determinism conditions do not
   depend on process history.
7. **Residual risk.** This argument covers the code these boards run.
   A future peripheral with process-wide static state could break it.
   Mitigations: the gate below runs every board in fresh and pool mode in
   mixed orders, the CI fresh-only job keeps the fresh path covered, and a
   new board or platform must pass `make pool-equiv` before it uses the
   pool (the same rule as AGENTS.md's equivalence gate for speed-ups).

**Prototype evidence.** `tools/pool_equiv.py` started one warm Renode and
alternated warm and fresh `bb run`s, reversing the order every round, so
boards and platforms (STM32F103 and STM32F072) followed each other in one
process. All 50 warm runs were byte-identical in `nets.csv`, `nets.vcd`,
`events.jsonl` and non-grant records to the fresh run of the same
configuration and to the committed hashes:

| Configuration | Warm runs | Committed reference matched |
| --- | --- | --- |
| spike (default) | 1 + 3 + 1 + 5 + 7 | `tests/golden/spike/outputs.sha256` (grant records included) |
| spike `--max-quantum 1ms`, `500ms` | 3, 3 | `tests/golden/spike/partition.sha256` |
| feedback (lookahead on) | 1 + 3 + 5 + 7 | `tests/golden/feedback/outputs.sha256` (non-grant) |
| feedback `--lookahead off` (one-cell, 50 000 grants) | 1 | same, all four hashes |
| feedback `--lookahead-plugin never` (in a process that had loaded the plugin) | 1 | same, all four hashes |
| `feedback_poll.elf` (one-cell) | 1 | same (non-grant) |
| adc (lookahead on), adc `--lookahead off` | 3 + 1, 1 | `tests/golden/adc/outputs.sha256` |
| adc-open | 3 | `tests/golden/adc-open/outputs.sha256` |

The prototype drove the warm process through a TCP monitor port
(`BB_RENODE_WARM`, `tools/patch_warm.py`). The console-pipe transport of
the design was tested separately for spike and feedback leases
(`tools/console_mon.py`: 0.14–0.24 s per warm lease).

## Gates

Every gate is run in the implementation step that introduces it. No gate
is weakened. A gate that turns out infeasible gets an amendment here.

- **G1, Part A startup.** `make startup-perf` (new,
  `scripts/startup_perf.sh`, ADR-003 measurement protocol: interleaved,
  N = 7, medians, load average recorded): `bb run` engine startup for the
  spike and the feedback board with Part A at least 12 % and 0.3 s below
  the same-session baseline. The baseline comes from a hidden switch,
  `BB_RENODE_STOCK_RUNTIME=1`, that drops A1 and A2. Prototype: −14 % /
  −0.46 s (spike), −16 % / −0.54 s (feedback).
- **G2, pool startup.** `make pool-perf` (new): with a pool of 1 slot and
  after its pre-warm, the median engine startup is ≤ 0.30 s for spike,
  feedback and adc (N = 7). The first lease after `bb pool run` starts is
  ≤ 0.5 s. The spike's total wall is ≤ 1.2 s. Prototype: 0.13 s, the first
  lease not yet measured with pre-warm (the second machine in a process
  loaded in 0.20–0.22 s), spike wall 0.66 s.
- **G3, results unchanged.** No change under `tests/golden/`. `make check`
  passes fresh (CI's fresh-only job). `make test`, `make partition`,
  `make lattice-check`, `make feedback-open`, `make lookahead-inert`,
  `make lookahead-equiv`, `make adc-equiv` and `make spike-verify-every`
  pass both fresh and under `bb pool run --slots 5`.
- **G4, pool equivalence.** `make pool-equiv` (new,
  `scripts/pool_equiv.sh`): on one pool process, two rounds in alternating
  order of spike (default, 1 ms, 500 ms), feedback (on, `--lookahead off`,
  `--lookahead-plugin never`, `feedback_poll.elf`), adc (on, off) and
  adc-open. Each run must give byte-identical `nets.csv`, `nets.vcd`,
  `events.jsonl` and non-grant records to a fresh run of the same
  configuration and to the committed hashes. `make determinism` and
  `make feedback-determinism` under `bb pool run --slots 5` (10 runs,
  5 parallel leases) must be byte-identical.
- **G5, isolation.** Engine tests: a lease whose Renode is killed mid-run,
  or that ends with an injected protocol error or `PromiseBroken`, makes
  the manager discard the process. The next lease gets a fresh pre-warmed
  process and gives the committed hashes. `--max-runs 2` recycles after
  two leases with identical outputs. A pool that does not match the Renode
  path or plugin key is not used.
- **G6, existing startup gate.** `make lookahead-startup` still passes
  (fresh mode).

## Implementation plan (each step gated, one commit per step)

1. **Part A** in `engines/cpu-renode`: `DOTNET_TieredPGO=0` on the child,
   the 5 ms poll, the `BB_RENODE_STOCK_RUNTIME` switch for the A/B, and
   `scripts/startup_perf.sh` with `make startup-perf`. Gates G1, G3
   (fresh), G6. Amend this ADR with the measured numbers.
2. **Engine lease abstraction.** `RenodeEngine` gets a host that is either
   a spawned child or a pool lease (script delivery, readiness, log
   routing, end-of-lease report). Nothing uses leases yet. Gate: G3
   (fresh), unit tests for the console protocol against a real Renode
   (`BB_REQUIRE_RENODE=1`).
3. **`bb pool`** (manager, Unix socket protocol, pre-warm, discard and
   recycle rules, `run`/`start`/`stop`/`status`, `--renode-pool off|env`
   on `bb run`). Gates: G2, G4, G5.
4. **Make and CI.** `make test` and the gate scripts run under
   `bb pool run` (`POOL=0` turns it off), a fresh-only CI job, and
   `make pool-perf` / `make pool-equiv`. The `cargo test` Renode tests use
   the pool when `BB_RENODE_POOL` is set. Gates: G3 in both modes;
   `make check` wall time before and after, recorded with its load average
   (reported, not gated).
5. **Docs.** README (a startup column in the cost table measured with
   `make cost-table`, fresh and pool), AGENTS.md (commands, the
   `make pool-equiv` rule for new boards and platforms, the "pool is not
   an engine" note), and an amendment here with the final numbers.

## Open questions

1. **ReadyToRun.** May the implementation download
   `microsoft.netcore.app.crossgen2.linux-x64` 8.0.12 from nuget.org
   (SHA-256 `57a87e36…f748` of the package fetched here) and run it to
   build ReadyToRun images of Renode's IL-only assemblies? They would be
   cached like the plugin and keyed by the Renode install, and they would
   cut fresh starts further. Running it was refused in this session, so
   the gain is unmeasured.
2. **Pool discovery.** Only `BB_RENODE_POOL` (as proposed), or also a
   well-known per-user socket that a plain `bb run` finds on its own?
3. **External Control binds 0.0.0.0** (seen with `ss -ltnp` and strace).
   The exposure exists today. A pool makes Renode processes live longer,
   though the server itself is still created per lease. Whether 1.17.0
   can bind loopback only has not been checked.
4. **A lingering listener.** In the console-pipe test, the first lease's
   External Control port stayed in `LISTEN` after `Clear`; later leases'
   ports were released. Outputs are not affected (each lease uses a new
   port), and `--max-runs` bounds the leak. Step 3 should find the cause.

## Amendments

### Amendment 1: step 1 (Part A) as implemented (2026-11-08)

**Status:** Accepted with step 1.

- **Code.** `bb_cpu_renode::renode_command` builds every Renode `Command`:
  `DOTNET_TieredPGO=0` (A1) on the child only, and kill on parent death
  (`PR_SET_PDEATHSIG` = `SIGKILL` from a `pre_exec` hook, with a `getppid`
  check for a parent that died before the call; approver decision 7). The
  launcher is a native apphost (`file`: ELF executable), so Renode is that
  one process and the signal reaches it. The signal fires when the
  *thread* that spawned the child exits, so a Renode must be spawned by a
  thread that lives as long as it is used (`bb`'s main thread, a test's
  thread; no test moves an engine to another thread). The connect poll
  (A2) and the self-connect retry sleep 5 ms.
  `BB_RENODE_STOCK_RUNTIME=1` restores the stock runtime and the 100 ms
  poll for the G1 baseline; it keeps the parent-death signal.
- **Orphan test.** `cli/tests/orphans.rs` (`killed_bb_leaves_no_renode`,
  in `make cargo-test`) SIGKILLs a `bb run` of the spike 1.5 s after its
  Renode child appears and requires that child to be gone within 10 s.
  The same procedure with the binary of `dcc9d66` leaves the Renode alive
  (`ORPHAN alive: ... (renode) S 1650299`, then killed by hand), so the
  test fails without the change.
- **G1** (`make startup-perf`, `scripts/startup_perf.sh`, N = 7,
  interleaved, medians, against `BB_RENODE_STOCK_RUNTIME=1`; outputs
  identical in every run). Every attempt is listed:

  | Attempt | Board | Stock | Part A | Cut | Load | Gate |
  | --- | --- | --- | --- | --- | --- | --- |
  | 1 | spike | 2.54 s | 2.19 s | −0.350 s (13.8 %) | 2.7 | pass |
  | 1 | feedback | 2.55 s | 2.41 s | −0.140 s (5.5 %) | 2.4–3.9 | **fail** |
  | 2 | feedback | 2.64 s | 2.24 s | −0.400 s (15.2 %) | 1.9–2.1 | pass |
  | 3 | spike | 2.54 s | 2.17 s | −0.370 s (14.6 %) | 3.6–9.1 | pass |
  | 3 | feedback | 2.56 s | 2.28 s | −0.280 s (10.9 %) | 3.6–4.4 | **fail** |

  Against `bb` built from `dcc9d66` (the commit before ADR-004, N = 5,
  spike only, because make stops at the first failing board): 2.44 →
  2.32 s, −0.120 s (4.9 %), **fail**.

  The failures come from host noise, not from Part A. A run sometimes
  takes about 1.5 s longer (3.6–4.4 s) with no memory or I/O pressure in
  WSL (`/proc/pressure/*` at 0, 10 GB free); another agent's CPU-bound
  job was running throughout. With N = 7 two such runs on one side
  move the median. In the failing attempts these outliers landed on the
  Part A side (feedback attempt 1: 4.04 s; the `dcc9d66` comparison: 4.06
  and 3.65 s). A separate interleaved check (N = 8, spike: stock, stock
  with an inherited `DOTNET_TieredPGO=0`, Part A) had outliers on the
  stock side (4.36 s) and the Part A side (3.75 s) alike. Its medians
  were 2.585 / 2.24 / 2.20 s: A1 alone gives −0.35 s, and Part A gives
  −0.385 s (14.9 %). G1 is re-measured at step 5 (Amendment 5). It is
  not marked as met until a full `make startup-perf` passes for both
  boards.
- **G3 (fresh)** passed: `make check` (150 s, load 12 during its own
  parallel tests), `make partition`, `lattice-check`, `feedback-open`,
  `lookahead-inert`, `lookahead-equiv`, `adc-equiv` and
  `spike-verify-every`. Nothing under `tests/golden/` changed.
- **G6** passed: `make lookahead-startup`, without the plugin 2.204 s,
  with it 2.194 s (difference −0.010 s ≤ 0.3 s, N = 7, load 4.2).
