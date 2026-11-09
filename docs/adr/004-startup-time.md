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

Measured, A1 + A2 save only 0.2–0.4 s, too close to G1's thresholds on
this host. Amendment 6 adds A3, a cached .NET multi-core JIT startup
profile, which is an engine-internal addition that still needs the
approver's review. With A3, G1 passes by a wide margin.

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

### Amendment 2: step 2 (the engine host and the console) (2026-11-08)

**Status:** Accepted with step 2. Engine-internal; it answers open
questions 3 and 4 (approver decisions 5 and 6).

- **Host.** `RenodeEngine` holds a `Host`: a spawned `Child` (killed on
  drop, as before) or a `pool::Lease`. With `RenodeConfig::pool` (default
  `$BB_RENODE_POOL`; nothing sets it in this step) the engine asks the
  pool before spawning. `BUSY`, `NOMATCH`, no pool behind the socket, or
  an error all mean a fresh start. A leased run writes the same
  `session.resc` without the plugin loader `include`, has the manager run
  it (`RUN <workdir> <script> <port>` → `READY` or `ERR`), connects to its
  External Control server and continues as a fresh run would. On drop it
  closes External Control first, then sends `END ok` or `END failed` (a
  failed engine or a panic) and waits for `DONE`. `renode_pid()` and
  `leased()` report where the Renode came from.
- **Console facts** (Renode 1.17.0 over pipes, `src/console.rs`, tested
  in `tests/console.rs`): each line is echoed, then its output, then the
  prompt `(monitor) ` or `(<machine>) ` with no newline. A failed command
  prints `There was an error executing command '<cmd>'`, and inside an
  `include` the rest of that script is skipped. An unknown word prints
  `No such command or device: <word>`. `Console::run` therefore ends each
  batch with `bbsync_<n>` and waits for that line. The console runs lines
  in order, so the line cannot appear before the batch is done; a
  prompt-based wait would race with log lines from other threads.
  Renode's default log level is INFO (1) on the console and memory
  backends; a lease restores it before its script, which sets 2 as a
  fresh run's does.
- **Open question 4, the lingering listener: root cause.** In
  `ExternalControlSocket.RxThreadBody` (1.17.0), when the client
  disconnects, the receive thread calls `Disconnect(State.Unconnected)`.
  That runs `RestartConnection()`, which binds a new listening socket on
  the same port with a new receive thread. A `Clear` disposes the
  emulation's externals on the monitor thread
  (`Dispose → Disconnect(Disposed)`). If it runs while the receive thread
  is still in that restart, one of two things happens. It may close the
  half-built socket, and the receive thread then dies in
  `Socket.Bind` with an unhandled `ObjectDisposedException`, which kills
  Renode (stack trace captured). Or it may run before the new socket
  exists and null the fields afterwards, so a listener remains that no
  one closes. The prototype's first lease was the slow, not yet compiled
  path of that race.
  Reproduced (`leak_probe.py`, 20 lease cycles per session, `Clear` sent
  straight after the disconnect): 5 sessions gave 1 leaked listener, 0, and 3 Renode
  crashes. `tests/console.rs::external_control_relisten_and_clear`
  checks the state the fix relies on: after the disconnect Renode listens
  again on the port, and a `Clear` with the server back in `Accept`
  releases it.
- **The fix (step 3):** at the end of a lease the manager waits (up to
  2 s, polling `/proc/<pid>/fd` against `/proc/net/tcp`) until the
  process listens on the lease's port again. Only then does it send
  `Clear`, and it requires afterwards that the process listens on no port.
  A process still listening, or one whose console does not answer, is
  discarded. With the wait, 4 sessions of 20 cycles gave 0 leaks and
  0 crashes. The engine closes External Control before `END` for the
  same reason.
- **Open question 3, loopback-only binding: not supported by 1.17.0.**
  `RestartConnection` binds `new IPEndPoint(IPAddress.Any, port)`, and
  neither `CreateExternalControlServer(name, port)` nor any setting
  selects an address. `CreateExternalControlClient` makes Renode connect
  out to `IPAddress.Loopback`, which would avoid the listener. But it
  reverses the roles: Renode sends `CHECK_VERSION`, and the handlers
  choose message ids differently (`GetHandlerForCurrentThread`). That
  means a new client implementation, so it is not adopted here. **The
  exposure:** the port listens on all interfaces (1) from
  `CreateExternalControlServer` until `bb` connects (milliseconds;
  `Accept` closes the listener, and the backlog is 1), and (2) from the
  run's disconnect until the process is killed (fresh) or cleared (pool:
  the manager clears right after the re-listen, so also milliseconds). In
  those windows a host that can reach the port could take the connection.
  It would get the External Control commands (run, time, GPIO, ADC,
  system-bus reads and writes of the emulated machine), which include no
  monitor or Python access. A connection held in window 1 makes the `bb`
  run fail (its connect is refused until the startup timeout). One that
  connects, acts and disconnects before `bb` connects could change the
  run: the `GET_TIME == 0` check catches a `RUN_FOR`, but not a GPIO,
  ADC or bus write. This exposure predates ADR-004 and is unchanged by
  it. In WSL2 the VM's NAT and the host firewall keep the ports off the
  network. A pool process listens on no TCP port
  between leases: it is driven by its console pipe and has no monitor
  port (`--port -1`).

### Amendment 3: step 3 (`bb pool`) as implemented (2026-11-08)

**Status:** Accepted with step 3. No change to `spec/`, the protocol or
any output.

- **Manager** (`engines/cpu-renode/src/manager.rs`, so that engine tests
  can run it in-process). `PoolConfig` has slots (default 1), `max_runs`
  (100), `rss_limit` (2 GiB), the private directory, the plugin and a
  console timeout (120 s; at most 20 s for an end-of-lease `Clear`). The
  directory is created 0700 and must be owned by the user, and the
  socket is `<dir>/pool-<pid>-<n>.sock`. The default directory is
  `$XDG_RUNTIME_DIR/bb`, else `$BB_CACHE_DIR/pool`, else
  `~/.cache/bb/pool`. Each slot has a long-lived thread that spawns,
  pre-warms and replaces its process. That makes it the parent thread of
  `PR_SET_PDEATHSIG`: the processes die with the manager, however it
  ends. Each lease runs on its own connection thread, takes an idle
  process (`BUSY` if none; a lease never waits) and gives it back idle,
  or retired (the slot thread kills it and spawns a pre-warmed
  replacement).
- **Pre-warm, as built.** The plugin loader runs once (it compiles and
  caches the assembly if it is not cached yet). If it fails, the pool
  carries on without the plugin, and leases that need it get `NOMATCH`.
  Then a throwaway machine of the offline STM32F103 and of the stock
  STM32F072 is built. No ELF is loaded: three `sysbus WriteDoubleWord`
  put a vector table and `wfi; b .-2` in flash, and
  `cpu VectorTableOffset 0x08000000` covers the F072, whose flash is not
  aliased at 0. The machine gets the determinism settings, the
  lookahead block and `Arm`, and an External Control server. Over that
  server the manager makes the calls an engine makes (handshake,
  machine, GPIO, ADC on the F072, the lookahead read, 5 one-cell
  `RUN_FOR`s, `GET_TIME`), then ends the session as a lease ends it
  (Amendment 2). Pre-warm of one slot took 2.6 s (`pool-perf`); 5 slots
  in parallel took 6.6 s.
- **End of lease**, as Amendment 2 decided: wait for the re-listen,
  `Clear`, then require that no port is listening (otherwise
  `leaked_listeners` is counted and the process is discarded).
  `logLevel 1` and the run's `include` start each lease. The
  console's output goes to `<workdir>/renode.log` for the length of the
  lease.
- **CLI** (`cli/src/pool.rs`). `bb pool run [--slots N] [--max-runs K]
  [--rss-limit-mb M] [--renode P] [--dir D] [--no-wait] -- <cmd>`
  waits for the pre-warm by default, runs the command with
  `BB_RENODE_POOL`, prints the lease counters, stops the pool and exits
  with the command's status (128 + signal for a signalled command).
  `bb pool start` runs `bb pool serve` in its own session (`setsid`),
  logging to `<dir>/serve-<pid>.log`. It waits for the pre-warm, then
  prints `export BB_RENODE_POOL=...`. It exits after `--idle-exit`
  without a lease (default 1800 s; `never` disables it), or on
  `bb pool stop`. `bb pool status` prints each slot (state, pid, runs) and
  the counters. `bb run --renode-pool off|env` (default `env`). `bb run`
  prints `bb run: renode: leased from the pool (pid N)` or `started fresh
  (pid N)`, on stdout only. `make pool-equiv` and `make pool-perf`
  (`scripts/pool_equiv.sh`, `scripts/pool_perf.sh`) are added in this
  step because they gate it.
- **G2** (`make pool-perf`, 1 slot, N = 7, load 3.4): first lease 0.31 s
  (≤ 0.5). Median engine startup: spike 0.14 s, feedback 0.15 s, adc
  0.16 s (≤ 0.30). Spike median total wall 0.85 s (≤ 1.2). Every run was
  leased. **Pass.**
- **G4** (`make pool-equiv`, 1 slot, 2 rounds, forward then reversed).
  The configurations: spike default, 1 ms and 500 ms; feedback on,
  `--lookahead off`, `--lookahead-plugin never` and `feedback_poll.elf`;
  adc on and off; adc-open. All 20 pooled runs were served by one Renode
  process. Each was byte-identical to its fresh run (`nets.csv`,
  `nets.vcd`, `events.jsonl`, non-grant records) and to the committed
  hashes. Counters: 20 kept, 0 discarded, 0 leaked listeners.
  `bb pool run --slots 5 -- make determinism feedback-determinism`
  (10 runs each, 5 parallel) passed both with 20 leases, 0 `BUSY` and
  0 leaked listeners, in 18 s for the two targets. **Pass.**
- **G5** (`engines/cpu-renode/tests/pool.rs`; in-process pools; the
  spike blinky to 250 ms and the lookahead firmware `la_silent`):
  - leases equal a fresh run, the second lease reuses the process, and
    `max_runs 2` puts the third on a new process (3 leases, 2 kept,
    1 recycled, no listener left);
  - a Renode SIGKILLed at 100 ms fails the run, is discarded, and the
    next lease (new pid) equals the fresh run;
  - an injected protocol error (`RunForExtraNs` under `--verify every`)
    and a broken promise (`StretchPromise`) are discarded the same way;
    after the broken promise, the lease on the replacement equals a
    fresh lookahead run;
  - a foreign Renode path and a plugin from another cache get `NOMATCH`
    (fresh), a busy pool gets `BUSY` (fresh), and a socket with no pool
    behind it means fresh.
  All 5 pass. `cli/tests/pool.rs` checks three things. `bb pool run`
  passes the socket and the exit status and removes the socket. A
  SIGKILLed `bb pool run` leaves neither of its 2 Renode processes
  alive. `bb pool start`, `status` and `stop` work, a pooled `bb run` of
  the spike is byte-identical to a fresh one, `--renode-pool off`
  starts fresh, and after `stop` the socket and the process are gone.
- **G3 (fresh)**: `make check` passes (188 s, load 10); `tests/golden`
  unchanged.

### Amendment 4: step 4 (make and CI on the pool) (2026-11-08)

**Status:** Accepted with step 4.

- **Make.** `test`, `partition`, `lattice-check`, `feedback-open`,
  `lookahead-inert`, `lookahead-equiv`, `adc-equiv`, `spike-verify-every`,
  `determinism` and `feedback-determinism` build the firmware and `bb`,
  then run their recipe (now `pooled-<target>`) through
  `scripts/with_pool.sh`. That script calls `bb pool run --slots
  $(POOL_SLOTS)` (default 5), or joins the pool already in
  `BB_RENODE_POOL`, so a nested make (`make test` → `make determinism`)
  uses one pool. `POOL=0` replaces the wrapper with
  `env -u BB_RENODE_POOL`, so everything runs fresh. The performance
  targets (`perf`, `lookahead-perf`, `startup-perf`, `lookahead-startup`,
  `cost-table`) are not wrapped. `scripts/determinism.sh` also reports how
  its runs got their Renode.
- **Cargo tests.** `RenodeConfig::stm32f103` takes `BB_RENODE_POOL`, so
  the Renode engine tests lease under `make test`. Tests that need a
  fresh start set `pool = None` or remove the variable:
  `plugin_startup_overhead` (G6), the orphan test and the CLI pool tests.
  `plugin_is_compiled_once_and_cached` uses its own cache directory, so it
  never matches a pool.
- **CI.** A new job, `fresh`, runs `make test POOL=0`. The other jobs use
  the pool by default.
- **G3 in both modes: pass.** Every target below passed, and
  `tests/golden/` is unchanged in both modes:

  | Target | Pool (default) | Fresh (`POOL=0`) |
  | --- | --- | --- |
  | `make check` | 160 s, load 7.6–9.6 | 383 s, load 8–10 |
  | `partition` | 32 s | 42 s |
  | `lattice-check` | 14 s | 22 s |
  | `feedback-open` | 33 s | 82 s |
  | `lookahead-inert` | 50 s | 177 s |
  | `lookahead-equiv` | 92 s | 226 s |
  | `adc-equiv` | 86 s | 221 s |
  | `spike-verify-every` | 37 s | 47 s |

  Pool counters over these runs: 0 discarded, 0 recycled, 0 leaked
  listeners. The `make test` pool served 41 leases, and 30 runs found
  every slot busy and started fresh: the cargo tests run more Renode
  engines in parallel than 5 slots. Two `NOMATCH`es came from the
  test that uses its own plugin cache.
- **`make check` wall time.** Before this step (fresh, the same tree at
  `590ff34`): 190 s, load 5.9–11.3. After it, on the pool: 160 s, load
  7.6–9.6. The fresh run of the same tree took 383 s, but at load 8–10
  with another agent's job on the host, so the two fresh figures do not
  compare. The cut is smaller than the per-run startup gain because the
  cargo tests' own work and their parallel fresh starts dominate.
  Reported, not gated.

### Amendment 5: step 5, final numbers and the open gate (2026-11-08)

**Status:** Accepted with step 5. README (cost table with fresh and pool
startup columns, the startup paragraph) and AGENTS.md (the new commands,
the "pool is not an engine" and `make pool-equiv` rule, the no-orphans
rule, the `Clear` race as a known bad idea) are updated.

- **Startup against the commit before ADR-004** (`dcc9d66`, built in a
  separate tree with its own target directory). Interleaved, N = 5,
  medians, `scripts/startup_perf.sh` with `STARTUP_BASE`; outputs
  identical in every run:

  | Session | Board | `dcc9d66` | This change | Cut |
  | --- | --- | --- | --- | --- |
  | step 1, load 3 | spike | 2.44 s | 2.32 s fresh | −0.12 s (4.9 %), fail |
  | final, slow host | spike | 3.62 s | 3.19 s fresh | −0.43 s (11.9 %), fail on 12 % |
  | final | spike | 2.63 s | 0.22 s pool | −2.41 s (91.6 %), pass |
  | final | feedback | 3.45 s | 0.17 s pool | −3.28 s (95.1 %), pass | During the final session the
  host ran 1.5–3× slower than earlier (spike simulation 3 s instead of 1
  s; Windows reported 34 % CPU load outside WSL). The fresh comparison
  stopped after the spike in both sessions, because make stops at the
  first failing line. The pool comparison passes G1's thresholds for both
  boards by a wide margin.
- **G1 is not stably met at N = 7 on this host. This is a recorded
  deviation, not a weakened gate.** Every `make startup-perf` attempt
  (stock baseline, N = 7):

  | Attempt | spike cut | feedback cut |
  | --- | --- | --- |
  | 1 (step 1) | −0.350 s, 13.8 % pass | −0.140 s, 5.5 % fail |
  | 2 (step 1) | — | −0.400 s, 15.2 % pass |
  | 3 (step 1) | −0.370 s, 14.6 % pass | −0.280 s, 10.9 % fail |
  | 4 (final, slow host) | −0.170 s, 4.7 % fail | not run (make stopped) |
  | 5 (final) | −0.470 s, 11.9 % fail (≥ 0.3 s met, 12 % missed) | −0.430 s, 15.6 % pass |

  Five of eight board attempts cleared 0.3 s, and four of eight cleared
  both thresholds. The ADR's prototype estimate (−0.46 to −0.54 s) is
  within the range of the passing attempts. Failures follow 1.5–5 s
  outlier runs (one B run took 8.77 s) and phases where the whole host
  slowed down. Without those, Part A measures −0.35 to −0.47 s. The
  stock-vs-A1-only-vs-Part-A check of Amendment 1 had outliers on both
  sides, so Part A does not cause them. The gate stays as written
  (N = 7, ≥ 12 % and ≥ 0.3 s). It should be re-run on an idle host
  before Part A is called met.
- **G2, G4, G5** passed in step 3, and **G3** in both modes in step 4.
  **G6** passed three times: −0.010 s (step 1), and 0.139 s in the
  final session (≤ 0.3 s), measured while the host was slow.
- **The cost table** (README), `make cost-table`, both passes on the
  slow host. Engine startup was 2.53–3.38 s fresh and 0.17–0.46 s on the
  pool. All 40 pool runs were leased; counters 40 kept, 0 discarded,
  0 leaked listeners. All 8 A/B comparisons gave identical outputs.
- **Deferred, unchanged:** ReadyToRun via crossgen2 (approver decision 3:
  not approved, not run); the trimmed install view; a client-mode
  External Control connection that would avoid the listener
  (Amendment 2).

### Amendment 6: A3, the startup JIT profile, and G1 re-measured (2026-11-08)

**Status:** Implemented. A3 is an engine-internal addition to Part A. It
is not in approver decision 1's list, so it is recorded here for the
approver's review. It uses only the installed .NET runtime: nothing is
downloaded or run besides Renode (compare decision 3, crossgen2).
`BB_RENODE_JIT_PROFILE=off` turns it off.

- **Why.** Amendment 5 left G1 open. Re-run first on a quieter host (load
  2.2–3.8; Windows CPU 3 %), A1 + A2 alone still sat on the threshold:

  | Comparison (A1 + A2 only) | spike | feedback |
  | --- | --- | --- |
  | G1, stock baseline, N = 7 | −0.200 s (7.0 %), fail | −0.380 s (12.4 %), pass |
  | `dcc9d66`, N = 5 | −0.210 s (7.6 %), fail | −0.290 s (9.5 %), fail |

  So the true A1 + A2 effect on this host is about 0.2–0.4 s (7–13 %),
  below the prototype's −0.46 to −0.54 s. G1 needs a larger real cut, not
  more attempts. `DOTNET_TC_CallCountingDelayMs=0` was tried as well and
  gave nothing outside the noise (2.65 → 2.49 s, N = 7, not adopted).
- **A3. The .NET multi-core JIT startup profile.** Checked against the
  .NET 8.0.12 sources (`src/coreclr/inc/clrconfigvalues.h`,
  `src/coreclr/vm/multicorejit.cpp`):
  - `DOTNET_MultiCoreJitProfile=<name>` (retail config) makes
    `MulticoreJitManager::AutoStartProfile` start profiling without any app
    code.
  - With a file `<name>_<domain>_<domain>_<n>.prof` present
    (`_clrhost_clrhost_1.prof` for Renode's apphost), a background thread
    JIT-compiles the listed methods while the main thread runs.
    `DOTNET_MultiCoreJitNoProfileGather=1` keeps the playback read-only.
  - On Unix the write-delay timer (`MultiCoreJitProfileWriteDelay`) is
    compiled out, so the profile is written only by `StopProfileAll` at a
    graceful shutdown. `MultiCoreJitMinNumCpus` defaults to 2.
  - Observed with Renode 1.17.0: `SIGTERM` exits Renode gracefully in
    about 80 ms and writes a 180–195 KB profile. `SIGKILL` writes nothing.
  - Renode only (spawn to External Control listening, feedback, N = 9
    interleaved, A1 on both sides): 2.775 s → 2.245 s median with the
    profile played back (−0.53 s).
- **Implementation** (`engines/cpu-renode/src/lib.rs`, `JitProfile`):
  - Only a fresh child uses the profile. A pool lease starts no process,
    and pool processes start without A3.
  - The key hashes a version tag, the canonical Renode launcher, the
    platform description (its content, or its install-relative name such as
    `platforms/cpus/stm32f072.repl`) and whether the lookahead plugin is
    loaded. Profiles live in `~/.cache/bb/renode-jit` (`BB_CACHE_DIR` and
    `XDG_CACHE_HOME` apply as for the plugin cache).
  - **Playback:** when a profile for the key exists, the child gets
    `DOTNET_MultiCoreJitProfile=<dir>/<key>` and
    `DOTNET_MultiCoreJitNoProfileGather=1`.
  - **Gather:** when none exists, the child gets a name unique to the
    engine (`<key>.tmp-<pid>-<instance>`). At a clean teardown, and only
    then (no engine failure, no panic), the engine closes External Control
    and sends `SIGTERM`. It waits up to 5 s, then sends `SIGKILL`, and
    renames the profile into place. This happens once per key and costs
    about 0.1 s of teardown. Parallel first runs each write their own file,
    and the last rename wins. A failed or killed run writes nothing and
    the next run gathers again. The parent-death `SIGKILL` (decision 7) is
    unchanged, so a killed `bb` still leaves no Renode.
  - `BB_RENODE_STOCK_RUNTIME=1` (G1's baseline) also turns A3 off.
- **Determinism.** "Determinism argument" 4 applies unchanged. A3 only
  changes which methods are compiled earlier, and on which thread. JIT
  compilation runs no static constructor and no managed code of the
  application. Renode's results depend on virtual time, never on how fast
  a method was compiled. An unreadable or stale profile only costs JIT
  work: the runtime skips what it cannot match. Evidence, all with
  profiles active:
  - `cli/tests/jit_profile.rs` (new): starting from an empty cache, the
    first run leaves exactly one `.prof` file and no temporary files. A
    playback run leaves it untouched, and so does a run with
    `BB_RENODE_JIT_PROFILE=off`. All three give byte-identical
    `nets.csv`, `nets.vcd` and `events.jsonl`.
  - Every `startup_perf.sh` B run matched its A run (stock, or `dcc9d66`,
    neither with a profile).
  - G3 below.
- **G1 with A3** (`scripts/startup_perf.sh`, stock baseline, N = 7,
  interleaved, medians, outputs identical; profiles already cached; load
  2.2–5.3). Both boards pass:
  - spike: 3.05 s → 2.12 s, −0.930 s (30.5 %), pass.
  - feedback: 2.94 s → 2.18 s, −0.760 s (25.9 %), pass.
- **Against the commit before ADR-004** (`dcc9d66`, N = 5, interleaved,
  same session, load 2.8–5.7, outputs identical). Both boards pass G1's
  thresholds:
  - spike: 3.96 s → 2.66 s, −1.30 s (32.8 %).
  - feedback: 4.01 s → 2.74 s, −1.27 s (31.7 %).
- **G3**, both modes, on the A3 tree:
  - All targets pass in both modes: `make check`, `partition`,
    `lattice-check`, `feedback-open`, `lookahead-inert`, `lookahead-equiv`,
    `adc-equiv`, `spike-verify-every`, `determinism` and
    `feedback-determinism`.
  - Timings, fresh (`POOL=0`): `make check` 326 s at load 7.6. Pool:
    `make check` 220 s at load 13.
  - Nothing under `tests/golden` changed.
- **G4.** `make pool-equiv` passes: 20 leases, 20 kept, 0 discarded,
  0 leaked listeners.
- **G6.** `make lookahead-startup` passes, with a difference of 0.161 s.
- **G2** (`make pool-perf`) did not pass on this host in this session. A3
  does not touch the pool's code path. Numbers:

  | Run | First lease | Median startup (spike / feedback / adc) | Spike wall |
  | --- | --- | --- | --- |
  | In the gate suite | 0.88 s, fail | 0.27 / 0.28 / 0.22 s, pass | 1.28 s, fail |
  | Standalone, right after | 0.35 s, pass | 0.24 / 0.28 / 0.28 s, pass | 1.29 s, fail |

  The spike's wall fails because of its simulation, 0.92 s here against
  0.52 s when G2 passed in step 3 (spike wall 0.85 s then). The host was
  slower: load 5–7, and another agent's layout job was using half a core.
  Pool startup itself stays within its 0.30 s gate. G2 stays as written,
  and the result is recorded, not waived.
