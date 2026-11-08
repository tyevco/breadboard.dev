# ADR-001: Kernel language is Rust, exposed through a C ABI

- **Status:** Accepted. Approved via the Phase 0 plan on 2026-11-07.
- **Deciders:** project lead (plan approval); recorded by the foundations agent.
- **Scope:** `bbcore/` and every engine adapter crate under `engines/`.

## Context

The kernel owns the master clock, the time-grant scheduler, the elaborated board
model and the event stream (design doc, "Kernel"). It has to:

1. Be deterministic by construction: same inputs and seed give byte-identical outputs.
2. Host engines that are foreign code: ngspice is a C shared library, Renode is a
   .NET process reached over a socket, and Verilator and QEMU come later.
3. Expose a stable C ABI, with Python bindings on top of it, for tools and agents.
4. Be safe for coding agents to change. Agents write most of the code, so the
   compiler should catch as many mistakes as possible.

We considered C++, Rust, Go and Python.

| Option | Determinism / safety | FFI to C libs | C ABI out | Agent-friendliness |
| --- | --- | --- | --- | --- |
| C++ | Manual; UB is easy to hit | Native | Native | Weak: errors show up at run time |
| **Rust** | Ownership and `Send`/`Sync` checked at compile time; no UB in safe code | `libloading` / `extern "C"` | `#[no_mangle] extern "C"`, `cbindgen` | Strong: the compiler and clippy act as reviewers |
| Go | GC pauses and goroutine scheduling make ordering hard to pin down | cgo overhead and awkward callbacks | `c-shared` is awkward | Good |
| Python | Too slow for the scheduler's inner loop | ctypes | No | Good, but it belongs in the binding layer |

## Decision

- The kernel (`bbcore`) and the engine adapters are written in **Rust**. The
  toolchain is pinned in `rust-toolchain.toml`; it is 1.99.1 at the time of writing.
- The public surface is a **C ABI** generated from `bbcore`. It is added when the
  first non-Rust consumer arrives; Phase 0 only has the `bb` CLI. Python bindings
  wrap the C ABI, either through PyO3 or ctypes, to be decided in a later ADR.
- Rules for the kernel, which `make clippy` and review enforce:
  - Scheduling decisions use only integer `bbcore::Time` (femtoseconds, `u64`),
    never floats.
  - Every collection whose iteration order reaches an output is ordered
    (`BTreeMap`, `Vec`). `HashMap` iteration must not reach any output.
  - The kernel has no wall-clock reads and no unseeded randomness.
  - `unsafe` is allowed only in FFI adapter modules, and each use needs a
    `// SAFETY:` comment.
- Errors use `thiserror` and serialization uses `serde`/`serde_json`.

## Consequences

- Good: whole classes of nondeterminism, such as data races and unordered
  iteration, are compile errors or lint failures rather than flaky CI.
- Good: binding to `libngspice.so.0` (via `dlopen`) and to the Renode External
  Control API client is ordinary Rust FFI.
- Cost: contributors need Rust, and `make setup` installs it. Build times are
  longer than for C, so the target dir is moved off `/mnt/c` (`CARGO_TARGET_DIR`).
- Cost: the C ABI has to be designed explicitly. Rust types do not cross it
  directly.
