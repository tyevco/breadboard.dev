# ADR-015: Phase 2 hardening (ngspice robustness, External Control exposure, lock scope, the Renode pin)

- **Status:** Accepted (2026-11-15; see "Approver decisions
  (2026-11-15)"). Proposed 2026-11-14, revised after design review round 1
  (see "Review notes"). Amendment 1: H1 as built (D1-D4). Amendment 2: H3
  as built (D5; it corrects D5's claim that a Renode client reconnects by
  itself). Amendment 3: H4 as built (D6, D7; the declarations live in
  Rust, not in `// bb-plugin-deps:` header lines, and a matching legacy
  hash is kept until S's relock). Amendment 4: H2 as built, the lattice
  snap (approver decision D-SNAP, 2026-11-16; it resolves ADR-008
  Amendment 11 Finding 2).
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

## Amendment 2: H3 as built (D5, 2026-11-15)

Milestone H3 implements D5: Renode's External Control is a client of
`bb`'s listener, and that listener admits only `bb`'s own Renode. D5
changes no protocol message of the kernel and no file under `spec/`. What
was verified, what was built, where the build differs from D5's text, and
what was measured.

### Facts verified (Renode 1.17.0)

Against the sources at the `v1.17.0` tag
(`src/Renode/Network/ExternalControl/`) and a running Renode 1.17.0 (a
Python prototype first, then the engine):

- **Client mode connects to loopback only.** `RestartConnection` calls
  `Connect(new IPEndPoint(IPAddress.Loopback, port))` for a client
  (`ExternalControlSocket.cs` line 174) and never binds or listens.
- **The handshake runs the other way.** `ExternalControlClient`'s
  constructor sends `CHECK_VERSION` from the monitor thread and blocks the
  monitor until it is answered (`ExternalControlClient.cs` line 15). That
  thread is not the default handler thread, so the request comes from the
  handler with bit 15 set: its id is `0x8001`.
- **Message ids.** Renode routes an incoming message by id bit 15
  (`GetHandlerForMessage`, line 342). In client mode its default handler
  thread serves the handler of ids with bit 15 *clear*
  (`GetHandlerForCurrentThread`, lines 345-353). So `bb`'s requests carry
  ids with bit 15 clear (`Client::accept_reverse`, `Client::reverse`); a
  request with bit 15 set would be queued for a handler nobody serves.
  GPIO events from the emulation thread carry bit 15 (`0x8002`, ...).
  `bb` still classifies by message type (ADR-003 D1.3).
- **A client does not reconnect.** This corrects D5, which says that
  "in client mode Renode reconnects to `bb`'s listener". When the peer
  closes, `RxThreadBody` calls `Disconnect(isClient ? State.Disposed :
  State.Unconnected)` (line 295). A client is disposed: its handlers are
  set to `null` and its threads end, and nothing re-opens it.
- **A second client object is no way out.** A GPIO event registration adds
  a hook to the pin (`GPIOPort.cs` line 76, `AddStateChangedHook`) that is
  never removed and that sends through the object it was registered on.
  After `DisposeHandlers` that object's handlers are `null`, so the next
  edge would throw a `NullReferenceException` on the emulation thread.
- **The stack leak applies to the client too.** `RxThreadBody`'s
  `stackalloc` loop is shared, so the reconnect of ADR-004 Amendment 8 is
  still needed.

### What was built

- **`bb_cpu_renode::ec`** holds the listener, the owner check, the
  reconnect command and the thread checks.
- **`bb` listens.** The engine binds `127.0.0.1:0` before Renode starts.
  `session.resc` ends with `emulation CreateExternalControlClient "bb"
  <port>`, where the server line was before. The listener lives as long as
  the engine, because a reconnect goes to the same port. The engine no
  longer takes a port from `free_port` for External Control; the GDB stub
  still does.
- **The owner check.** For each accepted connection, the engine reads
  `/proc/net/tcp`. It takes the entry whose local address is the peer's
  and whose remote address is the listener's, and so finds the peer
  socket's inode. It admits the connection only if
  `socket:[<inode>]` is among `/proc/<pid>/fd` of its Renode: its child, or
  the leased process whose pid the pool's `OK` names. Any other
  connection is shut down and closed before a byte is read. The engine
  counts it (`RenodeEngine::ec_refused`), prints a line on stderr, and
  keeps waiting. Without `/proc` (not Linux) nothing is admitted, so the
  check fails closed.
- **The handshake.** `Client::accept_reverse` answers Renode's
  `CHECK_VERSION`: success for version 0, and an error otherwise.
- **The reconnect** (ADR-004 Amendment 8, unchanged in when it happens):
  1. The engine closes the connection.
  2. It waits until Renode's client threads are gone
     (`ExternalControlClient_RxHandler` and `…_DefaultHandlerThread`,
     `comm` `ExternalControl…` in `/proc/<pid>/task`). That takes about
     4 ms.
  3. It has Renode run `ec::RECONNECT_PY`, a monitor `python` command. The
     command calls the private `ExternalControlSocket.RestartConnection` by
     reflection on the one External Control object of the emulation and
     prints `bb-ec-reconnect 1`. That is a new socket, `Connect`, new
     handlers and a new receive thread, on the same object. So the hooks of
     the first registration send through the new connection, as they did
     through the server's restart before.
  4. It accepts the new connection, with the owner check.
  5. `Client::reverse` checks the version with a request of `bb`'s own,
     because a restart sends nothing first.

  Like the lookahead plugin, this reads a 1.17.0 internal by reflection.
  If the method is missing, the command fails with a Python error and
  prints no `bb-ec-reconnect 1`. Renode then never connects, the
  reconnect fails at its deadline (the I/O timeout, at most 30 s) with
  Renode's log tail, and the run fails. A pool reports `ERR` at once. No
  result changes silently. A Renode upgrade (D8) re-runs `make
  ec-exposure`, `make ec-reconnect` and `make ec-long`.
- **The monitor channel of a fresh Renode.** A reconnect needs a monitor
  command. So a fresh child now starts with `--console` and its stdin a
  pipe, instead of `/dev/null` and no console. It still has `--port -1`,
  so it has no TCP monitor, and the pipe is the only way in. Renode exits
  when the pipe closes. The script is still Renode's argument (the console
  runs it as `i @session.resc`), so startup is unchanged. Four interleaved
  pairs of fresh spike runs, before and after, gave 1.33-1.40 s and
  1.31-1.41 s of engine startup. That was a check, not a gated
  measurement.
- **Pool protocol 2** (`pool.rs`):
  - The `RUN` line's port is now the engine's listener. The script's last
    line blocks the console until the engine has answered
    `CHECK_VERSION`, so the engine sends `RUN`, accepts (watching the lease
    stream for an early `ERR`), answers, and only then reads `READY`.
  - `RECONNECT` (any number of times before `END`) has the manager run
    `RECONNECT_PY` on the console. It answers `RECONNECTING` when the
    output has `bb-ec-reconnect 1`, else `ERR`, and the process is then
    discarded.
  - Pre-warm runs each platform through a client connection to the pool's
    own listener, with the owner check against the console's pid. Once per
    process it also runs a reconnect, so a broken reflection fails at
    pre-warm, not in a run.
- **Teardown.** The end of a lease (the manager's `end_session`) and a
  gathering engine's `SIGTERM` (ADR-004 Amendments 6 and 9) now wait until
  the client's threads are gone, instead of waiting for the server to
  listen again. The re-listen race of ADR-004 Amendments 2 and 9 cannot
  happen any more, because nothing re-listens. Disposing a client that is
  still closing is the same class of race, hence the wait. `end_session`
  still requires no listener after `Clear`, and now also no External
  Control thread. `ec_server_accepting` and its test are removed.
- **The test hook** `RenodeConfig::ec_hook` (`#[doc(hidden)]`, never set
  by `bb`) is called with the listener's port before Renode is started or
  the lease runs the script, and before each reconnect. The gate uses it.

### G4, measured

`make ec-exposure` (`engines/cpu-renode/tests/ec_exposure.rs`, also in
`cargo-test`) ran 20 engine starts on the spike's blinky, 60 ms in 5 ms
grants, with a reconnect every 8 messages: 4 fresh starts in parallel and
16 leases from a 2-process pool. Before each start, and before each
reconnect, a separate `bash` process connected to `bb`'s listener with
`/dev/tcp` and waited for one byte. The hook returns only when that
connection is established, so it is queued before Renode's. Result:

- **20/20 starts**, each with 4 connections (the start and 3 reconnects):
  80 foreign clients, every one refused. Every foreign client read 0
  bytes, and every run admitted its own Renode.
- **Nothing listened on Renode's side.** The Renode process listened on no
  TCP port (`listening_ports`, its fds against `/proc/net/tcp{,6}`: the
  exact form of a port scan) after start, after each of the 12 grants and
  at the end of every run.
- **No result changed.** The PA5 edges of every run equal a reference run
  without foreign clients and without reconnects.
- **No process was discarded.** The pool discarded none and leaked no
  listener.

The test takes 8.2 s alone. Its negative controls:

- the unit test `ec::tests::listener_admits_only_the_owner` (a foreign
  `bash` queued first is refused and reads nothing; the owner is
  admitted; another pid never matches);
- `protocol::tests::reverse_connection_wire_format` (ids, the event, the
  reconnect's version check, a wrong version refused).

**The rest of the row.** The row's other gates are re-proven unchanged
because the reconnect is new code:

- `make ec-reconnect`, fresh and pooled, gave its committed hashes
  (86 reconnects on the ADC board and 25 on the feedback board, every 2 000 messages; fresh and leased alike);
- `make ec-long` passed in 130 s: the 1 500 ms every-lattice-point reference ran 150 000 grants across 9 reconnects (every 100 000 messages), passed `check_adc.py` and equals the lookahead run except grant records;
- `make pool-equiv` passed in 1 602 s (beside nothing else): 30 configurations, 2 rounds in alternating order, all 60 pooled runs leased from one Renode process, each byte-identical to its fresh run and to the committed hashes; 60 leases kept, none discarded, no listener leaked;
- `make jit-profile-stress` (the gathering teardown changed; under the
  measurement lock) passed: 32 of 32 gathers left one profile, 4 rounds of
  8 parallel runs beside 48 busy loops, at a 1-minute load of 66.

### Residuals, stated

- **The Renode-only model tests still open a server.**
  `examples/adc/bb_adc_test.py`, `examples/adc/renode_adc_test.py`,
  `examples/nrf-saadc/bb_saadc_test.py` and `tests/console.rs` drive Renode
  with their own clients through `CreateExternalControlServer`. They are
  test harnesses, not `bb` runs. For them the exposure is as ADR-004
  documents (milliseconds, loopback-reachable on a shared host).
- **The GDB stub** of `bb run --gdb` still listens inside Renode on an
  internal port, behind ADR-008 Amendment 8's accept check. That is
  unchanged.
- **The threat model is D5's.** The check is a local, unprivileged process
  posing as Renode or racing for the slot. A process of the same user that
  can `ptrace` Renode or read its memory is outside the model, as it was
  for the GDB fix.
- **A foreign client may sit in the backlog.** One that connects while a
  connection is in use stays in the listener's backlog until the next
  accept (a reconnect), which refuses it. It never gets a byte.

### H1 follow-up: ngspice failure text in any case

`NgspiceEngine`'s failure report (`failure_lines`, Amendment 1) matched
"timestep too small" and "singular matrix" in lower case only. ngspice's
`doAnalyses` prints "Timestep too small", so a collapsed step with no
error line was reported as "(ngspice printed no error or warning)". Every
match of that function now ignores ASCII case: the error and warning
prefixes and both texts. The regression test
`failure_text_matches_ignore_case` fails on the old matching (checked by
reverting it) and passes now. No committed hash depends on the text; it
only reaches the message of a failed run.

### Budget

`make test`'s addition is `ec_exposure` and unit tests well under a
second: within H3's +10 s.

- **`ec_exposure`** took 8.2 s alone, 8.93 s inside a pooled `make check`
  and 8.44 s inside `make check POOL=0`.
- **The two `make check` runs** were both green, one after the other in
  this worktree with nothing else running: 481 s pooled (the H0 baseline
  is 470 s; main has gained B1, R1, E4 and P0 since) and 581 s with
  `POOL=0`.
- **`ec-reconnect`** took 34.9 s pooled and 36.6 s with `POOL=0` in those
  runs, against 37.0 s in the H0 baseline.

The full gates are outside `make test`:

- `make ec-long`: 130 s;
- `make pool-equiv`: 1 602 s;
- `make jit-profile-stress`: 194 s.

## Amendment 3: H4 as built (D6, D7, 2026-11-15)

Milestone H4 implements D6 and D7 and applies their `spec/package.md`
changes (§1, §9, §12; described under "Exact `spec/` changes"). No output
hash, no `bb.lock` and no `board.bbm` changes: the plan puts the relock
into milestone S's commit, and this amendment records how H4 keeps it
there.

### D6: what a platform declares

- **The declaration** is one table, `bb_cpu_renode::PLATFORM_PLUGINS`
  (each `.repl`'s own models) plus `SHARED_PLUGINS`
  (`bb_lookahead.cs`), read by `PlatformDef::plugins` in
  `cli/src/platforms.rs`:
  - `stm32f103-offline@1`: `bb_lookahead.cs`;
  - `stm32f072-bb@1`: that, `bb_stm32_timer.cs`, `bb_stm32f0_adc.cs`;
  - `nrf52840-bb@3`: that, `bb_nrf52_saadc.cs`, `bb_nrf52_pwm.cs`.
- **There is no shared host-port base.** D6 names one, but each host port
  (`BbAdcHostPort`, `BbSaadcHostPort`, `BbNrf52HostPort`) is defined in its
  model's own source, and no source uses another's types (checked by the
  subset compile below). The shared set is the lookahead block alone.
- **The content hash** is the list hash of the `.repl` and the declared
  sources (`PlatformDef::content`). The plugin cache key and the run-time
  assembly are unchanged: one assembly of every source (ADR-005 D6).

### Deviation 1: no `// bb-plugin-deps:` header lines

D6 has each plugin source state its dependencies in a header line. H4
declares them in the Rust table instead, for two reasons:

- **A header line changes the source's bytes.** So it changes every
  platform's hash, legacy and scoped alike, and every committed `bb.lock`
  and `board.bbm` would need a relock in H4. The plan puts the relock in
  S's commit.
- **The table is what the hash follows anyway.** D6 already says the hash
  covers "the plugin sources the platform declares in
  `cli/src/platforms.rs`". A per-source header would be a second copy of
  that list to keep in step. The subset compile proves the table complete,
  which is the point of the headers: a hidden dependency fails it.

S may add header lines with its relock if review wants them in the
sources. The test would then check them against the table.

### Deviation 2: a matching legacy hash is kept until S

The committed locks and `board.bbm` files hold the pre-H4 hash, which
covers every plugin source (the "legacy" hash). A scoped hash would make
every one of them stale, so `make test` would fail without a relock. H4
therefore accepts the legacy form:

- **The rule** (`PlatformDef::locked_for`). A lock entry whose content is
  exactly the legacy hash of the current files is kept as it stands, also
  by `--update-lock`, which rewrites only entries that changed. The import
  carries it into `board.bbm` and `run.json`, so those stay byte for byte.
  Any other content gets the scoped entry, and `drift` reports it. A
  missing entry is added with the scoped hash, and so is every new lock.
- **It is not a weaker pin.** The legacy hash matches only if every plugin
  source and the `.repl` are unchanged, which implies the scoped hash
  matches. A legacy entry pins strictly more than a scoped one.
- **The cost until S.** An nRF model edit still breaks the STM32 locks
  that hold the legacy hash. `bb open --update-lock` then writes the
  scoped hash, and from then on that entry no longer moves with nRF edits.
  `make lock-scope` measures both sides of this.
- **S's relock.** The `bb-lock/1` migration (ADR-011 D5) writes every
  platform entry with `PlatformDef::locked()`, the scoped hash. With it
  `board.bbm` and `run.json` record the scoped hash too. After that the
  legacy acceptance is needed only to read a `bb-lock/0` file. S keeps it
  for that case, or removes it with `/0`. The plan's S row says so.

### D7: the package path

- **`*bb packages path=<dir>`** (`spec/package.md` §12). The line has no
  name and exactly the key `path`. The path is relative to the board file,
  and every line adds one directory, in order, before the first-party
  directory. An absolute path is an error.
- **A named directory that does not exist is also an error.** D7 does not
  say this. A skipped entry is how `bb.toml`'s `[packages] paths` behaves,
  but in a board file it would hide the package the line was written for:
  the same trap.
- **The warning** (`bbpkg::lock::path_warnings`). For every locked package
  whose recorded `path` differs from where it resolved now, `bb open` and
  `bb run` print a warning, for board files and projects alike. A path that
  names the same directory differently, such as through a symbolic link,
  does not count. The warning says what is at the recorded path:
  - nothing with that id;
  - an identical copy, not used;
  - a different package with that id, by tree hash, that "is NOT used".
    This is the trap itself, and the warning names the fix.

  When `bb run` refuses a lock, the warnings are part of the error.
- **Copies warn.** A project copied out of the repository, which many gate
  scripts make, now prints one warning per first-party package. That is
  D7 as written: such a copy resolves its packages away from their
  recorded paths. No script checked for silence on stderr.
  `cli/tests/system.rs` did, so its copy now sits beside a link to the
  first-party directory, and the paths agree.

### Gates (G5, G6), measured

- **G5, unit** (`platforms::tests`, in `cargo-test`):
  - an edit to each plugin source changes exactly the hashes of the
    platforms that declare it. An edit to `bb_nrf52_saadc.cs` changes no
    STM32 hash.
  - every source is declared by some platform, and every declared name
    exists.
  - the scoped hash differs from the legacy one.
  - a legacy entry is kept only on an exact match: no lock, another
    content, another Renode version or another platform's legacy hash all
    give the scoped entry, and `drift` reports the last three as
    mismatches.
- **G5, end to end** (`make lock-scope`, 47 s, 20 s of it the second
  build):
  - a `bb` is built from a copy of the tree with one comment line added to
    `bb_nrf52_saadc.cs`.
  - scratch copies of all 15 `PKG_BOARDS` entries are relocked under D6 by
    the stock `bb`.
  - under the edited `bb`, all 8 STM32 boards and projects stay current
    (`bb open --check`), and each of the 7 nRF52840 ones has exactly one
    drift, `renode:nrf52840-bb@3`.
  - the controls:
    - the stock `bb` finds every relocked copy current;
    - under the edited `bb`, all 15 copies of the committed (legacy) locks
      drift on their platform, STM32 ones included. So the edit took
      effect, and this is the churn S's relock removes.
- **G5, subset compile** (`make plugin-scope`, 4.2 s of Renode, about 6 s
  in all; CI job `adc`). With Renode 1.17.0's `AdHocCompiler`, the one the
  plugin cache uses:
  - each platform's declared subset compiles alone (4, 18 and 27 type
    definitions);
  - the whole assembly compiles (39), and none has a module initialiser.
    The check is a `.cctor` on `<Module>`, read through
    `System.Reflection.Metadata` without loading the assembly.
  - the negative controls: a source that uses `BbStm32F0Adc` fails alone
    with CS0246 and compiles beside `bb_stm32f0_adc.cs`; a source with
    `[ModuleInitializer]` is detected.
- **G5, "the relock in the schema-epoch commit":** no committed `bb.lock`
  or `board.bbm` changes in H4 (Deviation 2).
- **G6** (`cli/tests/lock.rs`, in `cargo-test`, under a second). A copy of
  `examples/spike` sits under `<tmp>/examples/spike`, so its lock's
  recorded path `../../packages/st/stm32f103c8` points at
  `<tmp>/packages`:
  - with nothing there, an identical copy there, then a modified copy
    there, `bb open --check`, `bb open` and `bb run` (up to its firmware
    check, after the lock check) accept the first-party tree. Each prints
    the matching warning. For the modified copy that is "a different
    st/stm32f103c8 (tree <its hash>) is at the recorded path and is NOT
    used", with the fix.
  - with `*bb packages path=../../packages`, the modified copy is used:
    `bb open` and `bb run` refuse its tree against the lock, and `bb open
    --update-lock` records exactly its tree hash. One line changes; the
    legacy platform entry is kept. After that no warning is printed.
  - a directive naming a missing directory is an error.
  - a project copy (`examples/bench` elsewhere) warns once per moved
    first-party package (3) and not for its own packages. The committed
    example boards, in place, print no warning.

### Budget

H4's `make test` budget is none (plan section 4, "—"). Its additions are
unit and CLI tests that take well under a second (`platforms::tests`,
`board::tests::packages_directive`, three new tests in
`cli/tests/lock.rs`). `plugin-scope` and `lock-scope` are outside
`make test`. Two `make check` runs were made in this worktree, one after
the other, at a 1-minute load of 1.7-2.5 before each. Both were green:

- pooled: 447 s (the H0 baseline is 470 s);
- `POOL=0`: 540 s.

`cargo-test` took 146.0 s in the pooled run.

## Amendment 4: H2 as built, the lattice snap (decision D-SNAP, 2026-11-16)

Milestone H2 was D1's timeboxed spike on the SAADC stall. Its report found
the cause in `NgspiceEngine`, not in the `saadc_se` view, and stopped at an
approval point with an engine fix that changes committed references. The
approver decided (**D-SNAP**, 2026-11-16):

- the **lattice snap** is approved, as H2, in place of D1's three view
  candidates;
- `bb run --ngspice-lattice-snap off` keeps the clamp before H2, so `make
  ngspice-stall` still reproduces the 77.43 ms stall for H1's gate, and
  `run.json` records the option only when it is off (the `spec/project.md`
  line is approved);
- `make ngspice-stall` adds H2's completion runs: 300 ms at L = 5, 10 and
  20 us, and a 1 s E3 run;
- the changed references are re-recorded with this reason and their
  measured per-sample differences, at most 1 uV per sample, with the
  0.285 ns `pin_edge` shift recorded.

### The cause

- **The drift.** In a quiet stretch ngspice steps exactly TMAX = L. The
  sum `time + TMAX` is rounded, and the lattice points `kL` (exact in fs,
  converted to f64) are not TMAX apart in f64. So the accepted points
  drift off the lattice by rounding, an ulp or so per step.
- **The micro-step.** `next_lattice_s` counts a point within the grant
  tolerance (`max(4 ulp, 0.5 fs)`) of `kL` as on it. Once the drift takes a
  landing just outside the tolerance, short of `kL`, the clamp limited the
  next step to `kL - time`: a sub-femtosecond step.
- **The limit cycle.** Such a step restarts the integration at about
  1e-16 s. On E3's board with the loading view, trapezoidal integration's
  Newton iteration then fails to settle at every step: the crawl of about
  0.3 fs per accepted point that H1's watchdog trips on (Amendment 1).
- **Not the view.** The loading view only makes the board stiff enough to
  fall into the cycle early. The committed E3 board (`load=none`) falls
  into it at 502.47 ms of a 1 s `homing_step.elf` run, which no gate ran
  that long (H2's open finding).
- **D1's candidates, as the spike measured them** (its logs, not re-run
  here): a fixed VPRE source in place of `V(vdd)` (candidate 1's stand-in)
  completed 300 ms at 10 and 20 us, but stalled at 253.21 ms at 5 us and
  at 524.96 ms in 1 s. `METHOD=GEAR` in the deck (candidate 2) completed
  the repro, but it is a deck-wide option that changes the integration of
  every element, not a view fix. Neither touches the micro-step.

### What was built

- **The snap** (`State::clamp`, `engines/analog-ngspice`). When no ramp or
  table corner comes first and a step would land short of the grant or
  the next lattice point by a gap `g` with `tolerance < g <= snap`, the
  step is stretched to land on the point. `snap = min(1 ps, 1 % of the
  step)`, so a stretch never changes a step by more than ngspice's own
  truncation-error control would. A landing within the tolerance is left
  as it is, so a run whose steps never drift that far keeps its solver
  points. Measured: the spike, E2's `s1`, `s50` and `c5` and their
  `load = "none"` twins, E3, and six of the seven bench cases are
  byte-identical.
- **The option.** `NgspiceConfig::lattice_snap` (default `true`) and
  `bb run --ngspice-lattice-snap on|off`. `off` is exactly the clamp before
  H2: with it, every committed reference on `main` before H2 regenerates
  byte for byte (all of `make golden-update` and `make bench-update`, E2's
  eight references included). `bb run` prints a warning when it is off,
  and a project run's `run.json` records `options.ngspice_lattice_snap:
  "off"`; the key is absent otherwise, so no other `run.json` changes
  (`spec/project.md` section 6).
- **`bb pkg test`** runs with the snap on, like `bb run`. No vector or
  qualification changed (`make packages`).
- **Determinism and the protocol.** The snap is a pure function of the
  solver's `time` and step and adds no state. Grants, the lattice,
  `ceil_L` and the boundaries are unchanged; a solver point still lands
  on every lattice point. Every changed run below has identical grant
  records.
- **Tests** (`cargo-test`):
  - `lattice.rs`, `quiet_tmax_steps_take_no_micro_step`: 400 ms of a
    settled RC in 10 ms grants takes no step under 1 ps with the snap,
    every point the clamp put on a lattice point is within 0.5 fs of it,
    and it takes fewer points; the negative control (`lattice_snap =
    false`) takes such micro-steps. Measured: with the snap no step under
    1 ps, the largest `|t - kL|` 5.0e-16 s, 40 062 points; without it 517
    micro-steps, points up to 5.2e-13 s off the lattice, 41 891 points.
  - `threshold.rs`, `lattice_point_crossings_after_2s_are_partition_independent`:
    its expected trace changes. The lattice points had drifted a few ulps
    short by 4.137 s, so the PULSE corner value came out a hair under
    `vih` and PB0 rose one point later. Now the rise is exactly at 4.137 s
    and the fall one point after 4.153 s. The partition independence it
    tests holds as before.

### The changed references

Each was regenerated twice from the same tree: with the snap off (it gave
the committed files byte for byte) and with it on. The differences, from
the two runs' `nets.csv` and non-grant records (`nets.vcd` follows
`nets.csv`). The outputs are written in whole microvolts, so "1 uV" is one
count of the last digit: the solver's values differ by less than 2 uV.

| Reference | Values that differ | Largest difference | Non-grant records |
| --- | --- | --- | --- |
| `feedback` (one-cell, and the default run's snapshots) | 28 of SENSE | 1 uV | U1.PB0's fall at 101.4525 ms: -285 275 fs (0.285 ns) |
| `feedback-open` (partition hashes) | 31 of SENSE | 1 uV | the same fall: -285 275 fs |
| `nrf-feedback` | 42 of SENSE | 1 uV | U1.P0.11's fall at 101.4282 ms: -283 679 fs (0.284 ns) |
| `adc` | 10 of ADC_IN | 1 uV | identical |
| `adc-open` | 11 of ADC_IN | 1 uV | identical |
| `adc-stock` | 19 of ADC_IN | 1 uV | identical |
| `adc-open-stock` | 11 of ADC_IN | 1 uV | identical |
| `adc-kicad` | 11 of ADC_IN | 1 uV | identical |
| E1 `dms-rail` ok-cold | 369 (VLED 5, LED_DIN 182, LED_DIN_R 182) | 1 uV | identical |
| E1 ok-nominal | 373 (VLED 4, LED_DIN 182, LED_DIN_R 182, VSYS 5) | 1 uV | identical |
| E1 wrong-cold | 372 (LED_DIN 185, LED_DIN_R 186, VSYS 1) | 1 uV | identical |
| E1 wrong-nominal | 373 (VLED 1, LED_DIN 186, LED_DIN_R 186) | 1 uV | identical |
| `bench` trigger | 1 of RC_N | 1 uV | identical |
| E2 `dms-vsense` sag | 4 of VLED | 1 uV | identical |
| E2 rec | 1 of VLED | 1 uV | identical |

- **The edges.** The two `pin_edge` shifts are `digital_in` threshold
  crossings, interpolated between solver points that moved. Every MCU
  edge that follows them is unchanged, because each is applied at
  `ceil_L` of the crossing, which did not move.
- **`make gdb-test`**'s E1 check compares against E1's ok-cold hashes, so
  it follows them.
- **E1's VLED snapshots** (`tests/golden/dms-rail/ok-*/vled.csv`) were
  already stale before H2. P1 restated the TPS22917 view and re-recorded
  E1's hashes but not its snapshots, which `make dms-rail-test` checks
  only within 1 mV. Against the snap-off regeneration, the committed
  ok-cold snapshot differed at 235 points, by up to 96 uV, and ok-nominal
  at 2 points by 1 uV. The re-recorded snapshots carry both changes; H2's
  share is 1 point of ok-cold, by 1 uV.

### Deviation: E2's sag and rec are not byte-identical

D-SNAP records "E2 and E3 byte-identical" from H2's spike, which ran on
`main` before P1. P1 then restated the TPS22917 view, which drives VLED,
and re-recorded E2's `sag` and `rec`. On P1's views the snap moves 4 of
`sag`'s VLED samples and 1 of `rec`'s by 1 uV; E2's other six cases
are byte-identical. These two references are re-recorded under the plan's
own H2 rule for E2 ("changed E2 hashes are re-recorded with the reason and
the per-sample difference, at most 1 uV"). E2's checks pass on them
(`make dms-vsense`, below). E3 is byte-identical.

### Gates, measured

On `a944cf2` (`main` with B2, E1 and P1) plus this change, on 2026-11-16.

- **`make ngspice-stall`: all checks passed** (1 min 46 s, idle host,
  1-minute load 0.3 at the start).
  - Part 1, the snap off: the repro ends in `NoProgress:ngspice at
    77430000059213fs` in the cell (77.43 ms, 77.44 ms], as in H1, in
    23.9 s with bb's peak RSS 38.6 MB; the outputs before the error equal a
    `--for 77430us` run; `--ngspice-watchdog 20000,10000` trips identically
    twice; the committed board runs 100 ms clean.
  - Part 2, the snap on: the repro completes 300 ms at
    `--renode-quantum` 5 us (34.2 s), 10 us (19.5 s) and 20 us (11.7 s),
    and 1 s at 10 us (52.7 s); the committed E3 board completes 1 s
    (50.8 s); with the snap off that run stalls at 502.47 ms, in the cell
    (502470us, 502480us].
- **`make dms-vsense` (E2): gate PASS** (20 min 45 s, beside `make check
  POOL=0`): every `check_dms_vsense.py` check, the refusal and the
  determinism runs; all eight references equal `tests/golden/dms-vsense` in every record
  (`sag` and `rec` as re-recorded here, the other six unchanged); the
  defaults equal them except grants.
- **`make dms-home` (E3): gate PASS** (37 min 19 s, beside `make check`):
  `step`, `typ`, `worst` and `park` equal their committed one-cell hashes
  in every record, unchanged; analog-first byte-identical; `--max-quantum
  1ms` equal except grants; 10 runs of `typ` byte-identical.
- **`make docs-plots-check`: PASS** (177 s). No figure changed: the E1
  and E2 differences are below the plots' resolution.
- **`make check`: green, pooled** (1 049 s, beside `make dms-home`, load
  up to 28) **and `POOL=0`** (1 313 s, beside `make dms-home` and `make
  dms-vsense`). Every step of `make test` that reads a changed reference
  passes on it: `feedback`, `feedback-order`, `nrf-feedback`, `adc`,
  `adc-order`, `adc-stock`, `adc-kicad`, `dms-rail-test`, `bench-test`,
  `ec-reconnect` and `gdb-test`; `dms-vsense-test` and `dms-home-test`
  unchanged; `packages` unchanged.
- **The CI gates on the changed references: PASS** (210 s in all):
  `partition` (the spike and `feedback-open`), `feedback-open`,
  `lookahead-inert` (117 grants with lookahead on, as before),
  `lookahead-equiv`, `adc-equiv`, `lattice-check` and `make dms-rail` (the
  M6 gate, with its 10-run determinism).
- **`make pool-equiv`: PASS** (22 min 52 s): 30 configurations, two
  rounds, every pooled run byte-identical to its fresh run and to the
  committed hashes, the re-recorded ones included.

### Budget

H2's `make test` budget is none ("—"). It adds one engine test,
`quiet_tmax_steps_take_no_micro_step` (the `lattice` test binary takes
0.57 s for its three tests), and changes no
gate's runtime measurably. `make ngspice-stall` is nightly. With H2's runs
it took 1 min 46 s on an idle host (load 0.3); its H1 repro took 23.9 s,
against 77.9-120.6 s under load in H1.
