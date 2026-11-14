# Debugging firmware with GDB

`bb run --gdb <port>` lets you debug the simulated MCU's firmware with GDB
while the circuit runs around it (Phase 1 M9, ADR-008 D10 and Amendment 7).
It works on every board with one MCU: the STM32 examples and the nRF52840
of dial-me-spinner.

## Start a debug run

Run `bb run` with `--gdb` and a port, then attach GDB to that port with the
same ELF:

```sh
cd examples/dial-me-spinner
bb run --gdb 3333 --for 120ms --bench bench/cold.toml --out out/debug
```

```sh
gdb-multiarch firmware/prebuilt/led_rail.elf -ex 'target remote :3333'
```

`bb` listens on `127.0.0.1` only. `--gdb 0` picks a free port and prints
it. By default the CPU **waits at reset** until a debugger attaches and
continues, like QEMU's `-S`, so you can set breakpoints before the first
instruction. With `--gdb-wait off` the run starts at once, and you can
attach whenever you like: the CPU halts at its next slice boundary.

`make setup` installs `gdb-multiarch`. Any ARM-capable GDB works, for
example the Zephyr SDK's `arm-zephyr-eabi-gdb`.

## What a halt means for the circuit

Virtual time stops while the CPU is halted. The MCU is in the middle of a
time grant: `bb` has asked Renode to run to some time `g`, and Renode has
stopped part-way. ngspice waits at or before `g`. No grant runs past it,
however long you look. When you continue, the run picks up exactly where
it stopped.

## What changes the results, and what does not

These **change nothing**. The run is byte-identical to the run without
GDB, and the gate checks this on the spike and on E1:

- attaching, at reset or in the middle of a run;
- halting with Ctrl-C;
- reading registers and RAM;
- detaching.

These **can change the results**:

- a breakpoint or watchpoint stop, and inserting one;
- a single-step;
- a register or memory write;
- a read of a peripheral or system register, since some registers clear
  bits when read.

A breakpoint ends Renode's time slice at the instruction where it hit.
Renode stamps every pin change in a slice with the slice's start, so the
pin changes in that slice now carry the halt instant instead. On the spike,
two edges move by 40 and 50 ns. On E1, one rise moves by 7.6 µs and VLED
changes by up to 12 mV. A breakpoint early in `main` shifted every later
edge of the spike by up to 150 ns. How long you stay halted does not
matter: the change comes from where the CPU stopped, not when.

So a debug run is a **debug run**. The first operation that can change the
results marks it:

- `events.jsonl` gets one `warn` record from the MCU's engine: `debug run:
  GDB: <what> in (p ns, g ns]: ...`.
- A project's `run.json` has `"debug": true` for every `--gdb` run.
- The summary line says what the debugger did.

Never compare a marked run against a reference. A session that only
attached, read and detached carries no mark, because its outputs are those
of a run without GDB.

A scripted session (GDB in batch mode, the same commands) does give the
same outputs every time: the gate repeats one 21 times on the spike and 5
times on E1.

## What `--gdb` changes about the run

- **Lookahead is off.** A board with a feedback loop runs in one-cell
  grants (`--gdb --lookahead on` is refused). The lookahead promises
  assume the CPU sleeps only in WFI, and a debugger-halted CPU breaks that.
  Lookahead never changes results anyway, only how fast the run goes.
- **Renode starts fresh.** A Renode pool is never used.
- **No timeouts.** A halted CPU can hold a grant open for as long as you
  like.

## What GDB cannot do here

- **`monitor` is refused.** GDB's `monitor` reaches Renode's own monitor,
  including `start` and `pause`, which would take time control away from
  `bb`.
- **No reverse execution and no `run`.** Continue and step (also as
  `vCont`) are all GDB needs.
- **One debugger at a time.** A second connection is refused. After you
  detach, another debugger may attach.
- **`kill` does not stop the run.** It detaches, like closing GDB does: the
  breakpoints are removed and the CPU continues. To stop the run, stop
  `bb`. Its Renode stops with it.

## Notes

- **Stepping at a WFI** advances virtual time while the CPU sleeps,
  without moving the program counter, until an interrupt wakes it. A step
  can carry the CPU to the end of a grant; the next step then runs in the
  next grant.
- **After Ctrl-C, the CPU halts at its next slice boundary at which it is
  awake.** Ctrl-C during a long sleep halts at the wake.
- **Packet trace.** `BB_GDB_TRACE=<file>` writes every packet between GDB,
  the proxy and Renode to a file. It is a diagnostic, never an output.

## How it works

Renode has a GDB stub of its own, but `bb` never hands it to you. Used
directly, it breaks `bb`'s time control:

- a step between two grants starts Renode's free-running time loop;
- Ctrl-C pauses the CPU in the middle of a slice and can hang a grant;
- its stop replies race GDB's continue packets.

So `bb` starts the stub on an internal port, takes its only connection
itself, and runs a small proxy in front of it
(`engines/cpu-renode/src/gdb.rs`):

- A continue or a step reaches Renode only when the CPU is parked inside a
  running grant, before the grant's end. A state read confirms this.
- Ctrl-C becomes a switch to single-step mode between two grants, so the
  CPU halts at a slice boundary.
- Detaching removes your breakpoints and lets the CPU run.

The engine, the kernel and the grant protocol are unchanged.

```sh
make gdb-test   # in make test, about 45 s
make gdb        # the full M9 gate
```
