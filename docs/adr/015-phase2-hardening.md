# ADR-015: Phase 2 hardening (ngspice robustness, External Control exposure, lock scope, the Renode pin)

- **Status:** Accepted (2026-11-15; see "Approver decisions
  (2026-11-15)"). Proposed 2026-11-14, revised after design review round 1
  (see "Review notes").
- **Part of:** the Phase 2 plan, milestones H1-H4 and H6. H2 is the
  SAADC-stall spike that D1 makes bounded.
- **Normative changes when implemented:**
  - `spec/time-grant.md`: the engine progress rule, and the debug-run
    sentence;
  - `spec/package.md`: §9 platform identities, §11 lint, §12 board-file
    search path;
  - `spec/pin-model.md`: `adc_in` without the stock model;
  - `spec/project.md`: `run.json`.
- **Evidence:** the Phase 1 retrospective (`~/bbscratch-p2-retro`):
  - the SAADC stall bracketed and ablated;
  - E3's RSS growth, and the `.save` experiment;
  - the External Control exposure and its GDB precedent;
  - lock churn;
  - the package-path trap;
  - CI and local runtimes.

  The `pi` research pass adds the libngspice segfault and the
  operating-point failure.

## Summary

Seven independent decisions, each with its own gate:

1. A deterministic **progress watchdog** turns an ngspice stall into an
   error.
2. The engine **saves only the vectors it reads**.
3. **Lint parses B expressions** against ngspice's function table.
4. **Operating points fail fast.**
5. **Only bb can connect** to Renode's External Control: Renode connects
   out to `bb` (a reverse connection), because 1.17.0's server has no
   accept hook.
6. **A platform's hash covers only what it loads**, with the residual
   stated.
7. **A board file can name its package path**, and a path mismatch warns.

Plus four policies:

- the Renode pin;
- the removal of `--adc-model stock`;
- the debug-run sentence;
- two gate tiers, PR and nightly, with budgets relative to a measured
  baseline (D11; AGENTS.md, no `spec/` change).

## Context

**The SAADC stall** (ADR-008 Amendment 11 Finding 2; retrospective §2.1):

- E3's board with the loading `saadc_se` view stalls 39.65 ms into a
  quiet stretch, at 77.43 ms, on the lattice point.
- ngspice prints `Reference value : 7.74300e-02` forever, with no error.
- `bb` stays at 99 % CPU, and its RSS grows without bound (2.3 GB after
  10 minutes).
- The stall time moves with the lattice: 52.315 ms at 5 µs, 156.94 ms at
  20 µs.
- It needs the pre-charge B source's reference to `V(vdd)` on the P3V3
  Norton rail, and trapezoidal integration. Gear passes 200 ms. A constant
  3.3 V in place of `V(vdd)` passes.
- The suspected cause recorded in Phase 1 (the 2.5 pF node and the 20 ns
  delay) is contradicted by the ablation.

**Memory:**

- E3 `typ`: `bb`'s RSS grows about 0.55 MB per simulated ms, to 1.5 GB.
  The deck has no `.save`, so ngspice keeps every vector at every solver
  point.
- `.save` of the 10 vectors read cut the RSS at 500 ms from 285 MB to
  75 MB, with byte-identical `nets.csv`, `nets.vcd` and non-grant
  records.

**libngspice 42:**

- a three-argument `min()` in a B source segfaults the process (exit 139,
  no message);
- a flat B-source region made the operating point fail after 31 s.

**External Control** (ADR-004 approver decision 5; Amendment 2, open
question 3):

- Renode 1.17.0 binds `IPAddress.Any`, with no address setting.
- The windows are milliseconds: from server creation to bb's connect, and
  from disconnect to kill or clear.
- A peer in the window gets run, time, GPIO, ADC and system-bus read and
  write.
- The GDB stub had the same class of bug, and it was real. A local client
  won the slot 5 times out of 5 and got `monitor python`. Amendment 8 of
  ADR-008 fixed it with a `SocketServerProvider` accept hook and a
  loopback and endpoint check, through public API.

**Lock churn:**

- The platform content hash includes every plugin source
  (`cli/src/platforms.rs:66`).
- The nRF PWM change (`ee40df0`) rewrote all 11 locks and every
  `board.bbm`, the STM32 examples included.
- 11 of 83 commits touch `bb.lock`.

**The package-path trap:** a board file resolves packages only from the
first-party directory compiled into `bb` (`bbpkg/src/lib.rs:131`), or from
`$BB_FIRST_PARTY_PACKAGES`. A modified package beside a board copy is
silently ignored, and the lock's `path` is informative only.

**External Control's socket** (verified for the review at the `v1.17.0`
tag): a raw `Socket` bound to `IPAddress.Any`, with `Accept()` called
directly, not through `SocketServerProvider`. The GDB fix's hook point
does not exist for it (D5).

**The Renode pin:**

- 1.17.0 (2026-09-07) is still the latest release.
- 4 342 lines of plugin C# are compiled against it.
- The plugins reflect on `NVIC.pendingIRQs` and
  `TimeSourceBase.delayedActions`.
- The engine works around the External Control stack leak and the `Clear`
  race.

## Decision

### D1. A deterministic progress watchdog in `NgspiceEngine`

- **The engine counts accepted solver points per lattice cell.**
- **The run fails with `EngineError::NoProgress`** in either case:
  - one cell takes more than `max_points_per_cell` accepted points
    (default 200 000; E3's PWM case measured about 60 per edge and at most
    a few thousand per cell);
  - `K` consecutive accepted points (default 10 000) do not advance the
    rounded fs time.
- **The message names** the stuck time, the lattice point and the
  `Reference value` line.
- **Counts, not wall time.** So the error itself is deterministic.
- `bb pkg test` and `bb run` report the error, and the outputs written so
  far stay valid up to the last completed grant.
- **H2 (the stall itself)** is a timeboxed spike that this watchdog makes
  bounded. Its candidates, in order:
  1. VPRE delivered per acquisition by the engine, from the VDD `adc_in`
     value it already pushes, instead of a live `V(vdd)`;
  2. the view qualified under a Gear option set;
  3. a view-local restatement of `Brst`.

### D2. Engine-owned `.save`

- The engine writes `.save` for exactly the vectors it reads:
  - every glue node;
  - every `digital_in` and `adc_in` net;
  - every probed analog net;
  - every `observe` node;
  - every assertion monitor's nets (ADR-010).
- The elaborated deck shows the line. A deck that already has `.save` is
  refused, because the engine owns the deck.
- A test checks that every vector the engine reads is in the list.

### D3. Lint of B expressions

- `bb pkg lint` and `bb run`'s deck check tokenise every B, E and G
  expression and `.func`, and check each function name and its arity
  against the ngspice 42 function table.
- The table is taken from the installed version's sources at
  implementation time; it is not guessed. Breadboard's own canonical arity
  is `min`/`max` = 2.
- Unknown functions and wrong arities are errors.
- The same tokeniser serves ADR-011 D11's name collisions.

### D4. Operating-point fail-fast

- `bb pkg test` solves the operating point of every vector at every
  qualification corner first, with the option set's iteration limits.
- It reports a failure with ngspice's named nodes in seconds, not after
  31 s.
- `bb run` reports an operating-point failure as an error that names the
  views on the nodes ngspice lists.

### D5. Only bb connects to External Control: a reverse connection

- **Verified for the review:** Renode 1.17.0's External Control server is
  a raw `System.Net.Sockets.Socket`. `RestartConnection` calls
  `Bind(new IPEndPoint(IPAddress.Any, port))` and `Listen(backlog: 1)`,
  and the handler thread calls `communicationSocket.Accept()`
  (`src/Renode/Network/ExternalControl/ExternalControlSocket.cs` at the
  `v1.17.0` tag, lines 166-180 and 252). It does not go through
  `SocketServerProvider`, which is what ADR-008 Amendment 8 hooked for the
  GDB stub, and it exposes no accept event. **There is no accept hook to
  install.** The first draft's first choice is withdrawn.
- **The design: the roles reverse.** `bb` listens on `127.0.0.1` on an
  ephemeral port, and Renode connects out to it
  (`CreateExternalControlClient`; in client mode `RestartConnection`
  connects to `IPAddress.Loopback`). Nothing listens on Renode's side, so
  no foreign client can reach Renode.
- **`bb`'s listener admits one peer:** the connection whose socket is
  owned by `bb`'s own Renode child, checked on Linux through
  `/proc/net/tcp` (the peer's inode) and `/proc/<pid>/fd`. Any other
  connection is closed before a byte is read. This protects `bb` from a
  process posing as Renode; it is not needed to protect Renode.
- **What it costs (ADR-004 Amendment 2):** in client mode Renode sends
  `CHECK_VERSION` first and its handlers choose message ids differently
  (`GetHandlerForCurrentThread`). So this is a **new client
  implementation** in `bb_cpu_renode`: the handshake in the other
  direction, the message-id handling, and the reconnect every 100 000
  messages of ADR-004 Amendment 8 (in client mode Renode reconnects to
  `bb`'s listener). `make ec-reconnect`, `make ec-long` and
  `make pool-equiv` re-prove it. The plan costs H3 at 1.5-2 agent-weeks.
- **Pool processes** keep `--port -1` between leases, and a lease creates
  the client connection the same way a fresh run does.
- **If H3 slips:** the exposure stays as ADR-004 documents it (milliseconds,
  loopback-reachable on a shared host), and the plan's Q7 records it.

### D6. A platform's hash covers what it loads

- **The hash** is the `.repl` plus the plugin sources the platform
  declares in `cli/src/platforms.rs` (its own models and the shared ones:
  `bb_lookahead.cs`, the host-port base).
- **Hidden dependencies are caught by compiling.** Each plugin source
  states its dependencies in a header line, `// bb-plugin-deps: …`. A
  test compiles each platform's declared subset as a separate assembly and
  fails if it does not build.
- **At run time** the cache still compiles one assembly with every source
  (startup is unchanged, ADR-005 D6). The subset compile is a test only.
- **The residual, stated:** a run on platform A loads code from sources
  that A's lock does not cover. Two effects of a change to such a source
  are therefore not caught by A's lock: a compile failure of the whole
  assembly (visible at once: every run fails, so it cannot pass silently),
  and a static side effect at assembly load. .NET runs a type's static
  initialiser only when the type is first used, so a model that platform
  A never instantiates cannot act on A's run, unless the assembly has a
  module initialiser. A test asserts that the compiled assembly has none.
  Process-wide static state is already covered by the pool rule (a
  peripheral with process-wide state must pass `make pool-equiv`). The
  compile failure is loud by nature. What remains uncovered is stated in
  `spec/package.md` §9: A's lock does not pin sources A does not
  instantiate.
- **The result:** an nRF model change no longer re-locks the STM32
  examples. The relock under the new rule lands in the plan's schema-epoch
  commit (milestone S).

### D7. The package path of a board file

- A board file may name its search path: `*bb packages path=<dir>`
  (relative, before first-party), as `bb.toml`'s `[packages] paths` does.
- `bb open` and `bb run` **warn** when a locked package's recorded `path`
  differs from where it resolved now. The tree hash still decides
  acceptance.

### D8. The Renode pin

- **Stay on 1.17.0 for Phase 2.**
- When upstream releases a version, a spike evaluates it on a branch. It
  runs:
  - the AGENTS.md upgrade gates (`lookahead-inert`, `lookahead-equiv`,
    `adc-equiv`, the `la_*` gates, `nrf-gates`, `pool-equiv`);
  - `ec-long` without the reconnect workaround, to learn whether the leak
    is fixed;
  - the D5 race test.
- An upgrade then needs its own ADR, as today.

### D9. Remove `--adc-model stock`

- ADR-005 approver decision 4 kept it "for now". It costs a reference
  set (`tests/golden/adc-stock`, `adc-open-stock`), `make adc-stock` and
  CI time.
- **Removing it** deletes:
  - the stock path in `RenodeEngine::apply_input_trace`;
  - `adc_board_isr.elf`'s gate;
  - its pool-equiv entries.
- The ADR-005 speed gate's comparison (`make adc-speed`) loses its
  baseline. Its last result is recorded in the README, and the gate is
  retired.

### D10. The debug-run sentence

ADR-008 Amendment 7's open question is answered by adding, to
`spec/time-grant.md`'s wall-clock rule:

> "A debug run (`bb run --gdb`) whose outputs carry the debug mark
> (ADR-008 D10, Amendment 7) is exempt: it may depend on when a debugger
> acts, and it is never compared against a reference."

### D11. Two gate tiers

- **PR tier:** `make check` and the CI jobs on a pull request. Its budget
  is relative to the plan's H0 baseline (measured on an idle ext4
  worktree, N = 3): `make check` ≤ baseline + 4 minutes, the slowest CI PR
  job ≤ 15 minutes.
- **Nightly tier:** a scheduled CI workflow and `make nightly`. It takes,
  by name: the `fresh` job (`make test POOL=0`, 13:58 in Phase 1's last
  CI run, the slowest job), `make ec-long`, `make dms-home`, the full
  `make dms-vsense`, and any later gate over its budget.
- **A move is measured.** H6 records each move's saving against the
  baseline. If the named moves fall short of the target, more gates move
  by name; the target does not move.
- A nightly failure is a release blocker for the next milestone merge,
  not a silent badge.

## Exact `spec/` changes (described, not applied)

- **`spec/time-grant.md`:**
  - "What engines MUST NOT do" gains "accept unbounded solver work in one
    cell: an engine reports `NoProgress`" (D1);
  - the wall-clock rule gains D10's sentence.
- **`spec/package.md`:**
  - §9: platform identity = the `.repl` plus the declared plugin sources
    (D6);
  - §11: B-expression lint (D3), the operating point at every corner
    (D4);
  - §12: `*bb packages path=` (D7).
- **`spec/pin-model.md`:** `adc_in` drops the stock-model paragraph (D9).
- **`spec/project.md` §6:** `run.json` drops `adc_model` (D9), and records
  the watchdog's limits among the options that can change results.

## Determinism impact

- **The watchdog** counts points, so a run either completes identically
  or fails identically.
- **`.save`** changes no output (measured, byte-identical on E3's first
  500 ms).
- **D5** changes no protocol message.
- **D6** changes lock hashes once, not results.

## Gates

| # | Gate |
| --- | --- |
| G1 | The SAADC stall repro (E3 board with `saadc_se`, 100 ms) fails with `NoProgress` at 77.43 ms, in bounded memory (< 500 MB) and time (< 2 min); every committed hash unchanged |
| G2 | `.save`: every committed hash unchanged; E3 `typ` peak RSS with `.save` first **measured over the full 2.7 s** (only 500 ms was measured: 75 MB), then gated at that figure + 20 %; a test that removes one vector from the list fails |
| G3 | Lint: a three-argument `min`, an unknown function and a wrong arity are each refused; every first-party view passes |
| G4 | External Control: nothing listens on Renode's side (a port scan of the Renode process finds no listening socket); a foreign client connecting to `bb`'s listener first is refused 20/20 and the run still starts with its own Renode; `make ec-reconnect`, `make ec-long` and `make pool-equiv` unchanged |
| G5 | Platform scope: editing an nRF-only plugin source changes no STM32 lock; each platform's subset assembly compiles alone; the compiled assembly has no module initialiser; the relock in the schema-epoch commit |
| G8 | Tiers: the nightly workflow green twice; each moved gate's saving recorded against the H0 baseline; the PR-tier targets of D11 met |
| G6 | Package path: a modified package beside a board copy, named by `*bb packages`, is used; without the directive, the mismatch warning is printed |
| G7 | `--adc-model stock` removed: `make test` and `make pool-equiv` green without its targets |

## Rejected alternatives

- **A wall-clock timeout for ngspice.** Not deterministic, and it hides
  the cause.
- **Running ngspice out of process** to survive segfaults. It adds a
  round-trip per grant to every run. Lint covers the known crash, and an
  out-of-process mode for untrusted registry views stays an open question
  (Q2).
- **Splitting the plugin into one assembly per platform at run time.** It
  costs a compile per platform (about 2 s each, cold) for a hashing
  benefit that the subset test gives for free.
- **A firewall rule or a network namespace** for External Control. That
  depends on the host and needs privileges.

## Questions for the approver

Consolidated in the plan's section 9: **Q7** (build the reverse External
Control client in Phase 2) and **Q8** (baseline-relative targets and the
nightly tier).

**Defaults (apply unless the approver objects):**

1. **The watchdog's defaults** (200 000 points per cell, 10 000 points
   without progress), recorded in `run.json`, so a board that needs more
   says so.
2. **No out-of-process ngspice** in Phase 2; revisit if a registry view
   crashes `bb` despite lint.
3. **Remove `--adc-model stock`** (D9), its references and its gate.
4. **The debug-run sentence** (D10), with this text.

## Approver decisions (2026-11-15)

All twelve questions of the plan's section 9 were accepted with their
recommended answers (plan section 10). For this ADR:

- **Q7, accepted.** The reverse-connection External Control client (D5) is
  built in Phase 2 (H3). If H3 slips, the exposure stays documented as in
  ADR-004.
- **Q8, accepted.** The PR tier's runtime targets are relative to the
  plan's H0 baseline (measured on an idle ext4 worktree, N = 3), and the
  nightly tier of D11 takes, by name, the `fresh` job, `ec-long`,
  `dms-home` and `dms-vsense`.
- **Build scope.** H1-H4 and H6 are in this build. H0 (the process
  baseline, no ADR) records the baseline in AGENTS.md's gate-budget table.

**Defaults.** No objection was made to the defaults listed under
"Questions for the approver", so they apply as written.

## Review notes (design review round 1, 2026-11-14)

**Adopted:**

- **D5's accept hook does not exist for External Control** (major):
  verified in the 1.17.0 source; the reverse connection is now the design,
  with its real cost (a new client: handshake direction, message ids, the
  reconnect of ADR-004 Amendment 8), and G4 tests both sides.
- **D6's residual** is stated, and a module-initialiser check closes the
  only route by which an uninstantiated model could act.
- **G2's 300 MB** was extrapolated from 500 ms; the threshold now comes
  from a full-length measurement.
- **The runtime budget arithmetic** (both critiques): D11 adds the
  nightly tier and baseline-relative targets.

**Not adopted:** compiling one assembly per platform at run time (one
critique's alternative for D6). It costs a cold compile per platform,
about 2 s each, for a residual that is now stated and narrowed.

## Amendment 1: H1 as built (D1-D4, 2026-11-15)

Milestone H1 implements D1-D4 and applies their `spec/` changes
(`spec/time-grant.md` "What engines MUST NOT do", `spec/package.md`
sections 8 and 11, `spec/project.md` section 6). What was built, what was
measured, and where the build differs from the text above.

### D1: the watchdog

- **Where:** `NgspiceEngine` (`engines/analog-ngspice`), at every accepted
  point (`GetSyncData`, `loc == 0`). `NgspiceConfig::watchdog` holds the
  limits, with ADR's defaults (200 000 points per cell, 10 000 points
  without progress). A point's cell is `ceil(point / L)`, so a point on
  `kL` belongs to `((k-1)L, kL]`. Without a lattice only the
  no-progress rule applies. `bb run --ngspice-watchdog <per cell>,<without
  progress>` sets the limits, and `run.json` records them
  (`options.ngspice_watchdog`).
- **The error:** `EngineError::NoProgress { at, detail }`, which the
  scheduler reports as `SchedError::EngineNoProgress`
  (`NoProgress:<engine> at <time>: <detail>`). On a trip the solver thread
  stays parked inside the callback until the engine is dropped, so it takes
  no more points and no more memory. The outputs of every completed grant
  stay written.
- **The `Reference value` line is computed, not read** (a deviation from
  "the message names ... the `Reference value` line"). ngspice prints that
  line from `OUTpData` on a CPU-clock timer (every 0.25 s of `clock()`), so
  whether and with what value it appears in the log depends on wall time.
  The message gives the same text, `Reference value : 7.74300e-02`, from
  the solver time, in C's `%.5e` format.
- **G1, measured** (`make ngspice-stall`, `scripts/ngspice_stall.sh`): the
  repro is `e3/board.cir` with `U10.P0.02` on the loading `saadc_se` view
  (its `load=none` removed) and `homing_step.elf`, 100 ms. (ADR-008
  Amendment 11 Finding 2 is a run that reads once and then stays quiet;
  `homing.elf` keeps the PWM running through 100 ms and does not stall
  there.) It trips the **cell rule**: 200 001 accepted points in
  (77.43 ms, 77.44 ms], with the solver at 77.4300000592 ms. So the stall
  is not a fixed point but a crawl of about 0.3 fs per accepted point. The
  time rounded to fs keeps advancing, and the no-progress rule never
  trips. `at` is therefore `77430000059213fs`, 59 ns past the lattice
  point, whose reference value is 7.74300e-02 to ngspice's five digits.
  The plan's "at 77.43 ms" holds to that precision.
- **G1's bounds, measured** on a shared host (other tracks' gates running):
  - bb's peak RSS was 35.8 MB in every run, against the 500 MB bound and
    2.3 GB after 10 minutes before H1;
  - the wall time was 77.9 s, 120.6 s and 85.7 s in three gate runs, at
    1-minute loads of about 12, about 19 (24 over 5 minutes) and 4 to 13.
    The 120.6 s run missed the 2-minute bound;
  - the time is about 8 s to reach 77.43 ms, then the 200 000 points, at
    2 000-3 000 points per second.

  So G1's wall-time bound holds on an otherwise idle host and not under
  heavy load. `make ngspice-stall` takes the measurement lock, which keeps
  other measurements off the host but not other gates.

  The rest of the gate passed in every run:
  - the outputs written before the error equal a `--for 77430us` run
    (`nets.csv`, `nets.vcd` and the non-grant records byte for byte; the
    stalled run adds the failed grant's own record);
  - at `--ngspice-watchdog 20000,10000` two runs fail with the same message
    in the same cell;
  - the negative control passes: the committed board (`load=none`) runs the
    same 100 ms clean.
- **Not caught:** a loop of rejected steps that never accepts a point.
  ngspice itself ends that with "timestep too small" once the step falls
  below `delmin`.

### D2: the engine's `.save`

- **The list:** every watched node (the analog outputs and probes, every
  thresholded `digital_in` node and every `adc_in` node, which for a
  loading `adc_in` is the view's hold node: exactly what `read_vectors`
  and the early-stop watch copy), plus every input glue node, as D2 lists
  them. `time` is saved by ngspice as the scale. The list goes on `.save`
  cards of eight vectors each, before `.tran`. A netlist with its own
  `.save` is refused; no committed deck had one. ADR-010's assertion
  monitors are not built yet; they will reach the list as watched nodes.
- **"The elaborated deck shows the line" (a deviation).** The `.save`
  cards are engine-owned lines, like the glue and `.tran`. A project's
  `deck.cir` stays the board body that `run.json`'s `deck_sha256` hashes,
  and it shows none of them. `NgspiceEngine::deck()` returns the full deck
  that ngspice gets, and a test reads the `.save` card from it.
- **The tests:** the `.save` card names exactly the read vectors. The
  negative control: without the watched node in the list, the run fails
  and reads nothing else. A deck with `.save` is refused.
- **G2, measured.** E3 `typ` over the full 2.7 s at bb's defaults had a
  peak RSS of 336 356 kB. That is bb's own `VmHWM`, which `bb run` now
  prints as `bb run: peak RSS`. Without `.save` the peak was 1.5 GB. The
  outputs are identical from run to run, but the RSS is not: the
  `make dms-home` gate's own `typ` run peaked at 316 608 kB, 6 % lower. The
  outputs equal `tests/golden/dms-home/typ` byte for byte. The gate is
  therefore **403 627 kB** (+20 %), checked by `make dms-home` on its `typ`
  run and by `make dms-home-rss` alone. `bb pkg test --report` on every
  first-party and example package is byte-identical before and after H1.
  So is every committed hash: `make test`, `make dms-home` and
  `make dms-vsense`.

### D3: lint

- **The tables** are taken from the installed ngspice 42 sources
  (`~/src/ngspice-42`). The B-expression parser's table is
  `src/spicelib/parser/inpptree.c`, `funcs[]` and `PT_mkfnode`. It also
  has `ternary_fcn`, with three arguments, and the `pwl` family, with an
  input and `x, y` pairs (`prepare_PTF_PWL`). Every other function takes
  one argument, except `pow`, `pwr`, `min` and `max`, which take two.
- **numparam is checked too.** Inside `{...}` numparam evaluates
  (`src/frontend/numparam/xpressn.c`, `fmathS`). It never crashes, but it
  is silently wrong: `min(a, b, c)` gives `min(a, c)`. So brace
  expressions are checked against numparam's table.
- **Where it runs:** `bbpkg::bexpr`, in `bb pkg lint` (each view file),
  `bb pkg test` (each vector's deck) and `bb run` (the whole deck, before
  ngspice reads it). It checks B `V=`/`I=`, E/G `value=`/`vol=`/`cur=` and
  `.func` bodies, and a call of a `.func` must match its parameter count.
  The three-argument `min()` was confirmed to segfault the ngspice 42 CLI
  (exit 139).
- **G3:** a three-argument `min`, an unknown function (`atan2`) and a
  wrong arity are each refused (`bbpkg/tests/lint.rs`). Every committed
  `.lib` and `.cir` under `packages/` and `examples/` passes (a test).
- **Residual:** `.param` values and brace values of other elements are not
  checked. They are numparam's, which never crashes.

### D4: the operating point

- **`bb_analog_ngspice::operating_point(cfg)`** builds the engine's deck
  with `.op` and runs it synchronously, then reports
  `EngineError::OperatingPoint { nodes, detail }`. The nodes are the ones
  ngspice names: `trouble with node`, `check node(s)`, and the `*` rows of
  "Last Node Voltages".
- **Found while building: ngspice 42's transient-op fallback.** ngspice 42
  makes `optran` its standard (`src/frontend/init.c`: `1 1 1 100n 10u 0`).
  After gmin and source stepping fail, it runs a 10 µs transient of its
  own. That transient is where a large deck spends its time before giving
  up.
  - In an `.op` analysis of the shared library, `optran` fails on a missing
    `op1.time` vector, and the synchronous command never returns (a hung
    test).
  - So the check deselects `optran` for its own circuit
    (`optran 1 1 1 0 10u 0`: a step size of 0) and restores the built-in
    default before removing the circuit. A test shows that the transient's
    own failure text is unchanged after a check.
  - This is "the option set's iteration limits": Newton (`ITL1`), gmin
    stepping and source stepping under the option set's card, and nothing
    after them.
- **Found too: `optran` calls `GetSyncData`.** `optran`'s transient calls
  it with its own time while the transient's vectors are still empty.
  - Before H1 the engine parked at that callback and failed with
    `vector tran1.time is not real`. A deck whose operating point converges
    only through `optran` could not start in bb, and a failing one was
    misreported.
  - The engine now lets those callbacks pass: only before the first park,
    and only while the time vector is empty. A converged operating point
    never gets there, so no committed result changes.
  - A failure at t = 0 is now `EngineError::OperatingPoint`.
- **`bb run`** names the views on those nodes. A subcircuit-internal node
  names its instance's `.subckt`, and a top-level node every instance on
  it, each with its package and view.
- **`bb pkg test`** runs the check before every vector's transient, under
  every pair the vector runs (its own and every recorded qualification).
  ADR-009's named corners do not exist before milestone S. The check adds
  no measurable time. `bb pkg test` on the three package roots took a
  median 31.4 s with H1 against 34.0 s before it (N = 5, interleaved,
  under the measurement lock, at load 23-60).
- **The 31 s give-up** of the `pi` research pass was not retained as a
  deck. The check's flat-region test deck (a B regulator with no
  conductance above regulation) fails in milliseconds, naming `out`.

### Budget

`make test`'s addition is the new unit and lint tests, well under 1 s, and
the operating-point checks inside `packages`, which are not measurable.
That is within H1's +30 s. The new gates `make ngspice-stall` (about
3.5 minutes) and `make dms-home-rss` (about 8.5 minutes beside other
gates) are outside `make test`. Their tier is nightly (D11; H6 builds it).
`make dms-home` (62 minutes beside other gates) and the full
`make dms-vsense` pass with every committed hash unchanged.
