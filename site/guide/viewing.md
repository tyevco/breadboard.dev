# Viewing waveforms and debugging in VS Code

Every `bb run` writes its waveforms to `nets.vcd` in the output directory.
Breadboard has no viewer of its own. Existing viewers open the file, and
this page says which ones and what to expect from them (Phase 2 E4,
ADR-016). The second half of the page shows how to debug a run's firmware
from VS Code.

## Which file to open

| File | What it holds | Open it in |
| --- | --- | --- |
| `nets.vcd` | Every probed net. An analog net is a `real` variable in volts. An MCU pin net is a 1-bit `wire` with every edge at its exact time. | Surfer, Vaporview or GTKWave |
| `nets.csv` | The analog nets, sampled at `--sample-rate` (default 100 kHz) | a spreadsheet, Python, any plotting tool |
| `events.jsonl` | Breadboard's records: `pin_edge`, `log` and `warn`, grants | a text editor or `jq`; no waveform viewer shows them |

Some details of `nets.vcd` (see [Outputs](../reference/outputs#nets-vcd)):

- **The timescale is 1 fs.** Times are integer femtoseconds, the
  simulation's own clock. Viewers show them in whatever unit you zoom to.
- **All variables are in one scope, `board`.** A pin net keeps its dotted
  name as one variable, for example `U10.P1.04`.
- **Analog values change only at sample instants.** They are written at
  the fixed sample times, and only when the value rounded to 1 µV changes.
  Between two changes the value holds, so draw a real variable as a step
  if your viewer offers the choice.
- **The `$comment` name map.** A VCD name cannot contain a space or a
  slash, so in a project the analog nets carry their SPICE names. When a
  name differs from the net's name in the schematic, the header maps them:

  ```text
  $comment
    LED_ring_LED_DIN = /LED ring/LED_DIN
    LED_ring_LED_DIN_R = /LED ring/LED_DIN_R
  $end
  ```

  The left side is the variable you see in the viewer. The right side is
  the net name, which is also the column name in `nets.csv`.

## The viewers

| Viewer | Licence | Version checked | How to get it |
| --- | --- | --- | --- |
| [Surfer](https://surfer-project.org/) | EUPL-1.2 | 0.7.0 | the web app at [app.surfer-project.org](https://app.surfer-project.org/), a desktop build, or its VS Code extension |
| [Vaporview](https://marketplace.visualstudio.com/items?itemName=lramseyer.vaporview) | GPL-3.0 | VS Code extension 1.6.1 | the VS Code extension |
| [GTKWave](https://gtkwave.github.io/gtkwave/) | GPL-2.0 | 3.3.116 (Ubuntu 24.04) | `sudo apt install gtkwave` |

- **Surfer.** Surfer has drawn real variables as analog waveforms since
  0.5.0. The web app runs in your browser and opens a file you choose
  locally; its own page notes that some features of the desktop build are
  missing. Breadboard links to it and does not host a copy.
- **Vaporview** opens `nets.vcd` inside VS Code, next to your firmware.
- **GTKWave** is the classic choice: `gtkwave out/spike/nets.vcd`. A real
  variable can be drawn as an analog trace (Data Format, Analog).

**PulseView (sigrok) cannot read Breadboard's VCD.** libsigrok's VCD input
turns the timescale into a sample rate: 1 fs becomes 10^15 samples per
second. E3's 2.975 s run would then be about 3 × 10^15 samples. Use one of
the viewers above. Breadboard does not write a coarser VCD for PulseView,
because that would be lossy and a second format to keep honest.

### How this page is checked

`make vcd-check` loads the VCDs that `make demo` writes, and a small
committed fixture (`tests/fixtures/vcd/mixed.vcd`), with
[wellen](https://github.com/ekiwi/wellen) 0.25.7. wellen is Surfer's own
waveform library. The check requires that:

- the timescale reads as 1 fs;
- every declared variable is present, `real` variables as Real and wires
  as 1-bit wires;
- every value change is there;
- every analog value equals the run's `nets.csv` at every sample.

Its negative controls show that a wrong timescale, a changed value or a
dropped change each fail. The loader is a tiny crate, `tools/vcd-check`,
kept outside the Cargo workspace, so `bb` never depends on wellen. CI runs
it in the `dms-rail` job.

## Debugging from VS Code

`bb run --gdb` puts a GDB server in front of the simulated MCU (see
[Debugging with GDB](./debugging)). Two templates connect VS Code to it
with the [cortex-debug](https://marketplace.visualstudio.com/items?itemName=marus25.cortex-debug)
extension:

- `docs/templates/vscode/tasks.json`: a task that starts the run;
- `docs/templates/vscode/launch.json`: a debug configuration that attaches.

Copy them into your project by hand. `bb open` does not write them.

```sh
cd examples/dial-me-spinner      # or your project
mkdir -p .vscode
cp ../../docs/templates/vscode/tasks.json ../../docs/templates/vscode/launch.json .vscode/
```

Then edit the two lines that are specific to the dial-me-spinner E1 run
the templates show:

- the task's `args`: the bench, `--for` and `--out` of your run;
- the configuration's `executable`: the ELF your `bb.toml` target names.

Install cortex-debug and an ARM-capable GDB (`make setup` installs
`gdb-multiarch`). Then start **bb: attach (cortex-debug)** from the Run and
Debug view. VS Code first runs the task `bb: run --gdb`, waits until bb
prints `GDB: listening on`, and then attaches. The CPU waits at reset, so
you can set breakpoints before you continue.

### Why the templates look the way they do

- **`request: attach`, `servertype: external`.** bb is the GDB server.
  cortex-debug starts no server of its own and loads nothing: the firmware
  is already in the simulated MCU.
- **`overrideAttachCommands: []`.** cortex-debug's default attach for an
  external server sends `monitor halt`. bb refuses every `monitor`
  command, because `monitor` reaches Renode's own time control. Nothing
  is needed instead: the CPU is already halted when GDB attaches.
- **No `svdFile`.** cortex-debug's peripheral view reads peripheral
  registers (`0x40000000`-`0x5FFFFFFF`). Some registers clear bits when
  they are read, so such a read marks the run as a debug run.
- **A background task.** The run keeps going after the debugger attaches.
  The problem matcher's `endsPattern` (`GDB: listening on`) tells VS Code
  when the debugger may attach. Its `beginsPattern` matches nothing on
  purpose. The task is active from its start (`activeOnStart`), and VS
  Code does not test a line that matched `beginsPattern` against
  `endsPattern`.

### What changes the results

The same rules hold as for plain GDB. A session that only attaches,
halts, reads registers, variables and RAM, and then stops (cortex-debug
detaches) leaves `nets.csv`, `nets.vcd` and `events.jsonl` byte-identical
to a run without GDB. A breakpoint stop, a step or a write marks the run
as a debug run, with a `warn` record and `"debug": true` in `run.json`.
Never compare a marked run against a reference.

### How the templates are checked

`make vscode-test` runs the templates, copied byte for byte into a copy of
the dial-me-spinner project, inside a real VS Code on Linux. The setup:

- pinned versions: VS Code 1.145.3 and cortex-debug 1.12.1 with its four
  `mcu-debug` dependencies;
- VS Code under Xvfb, in a network namespace with only loopback.

The test starts the launch configuration the way the Run and Debug view
does and checks two cases:

- **Attach, read, stop.** It reads the stack, the core registers and the
  word at `$sp`, then stops the session. All four output hashes must
  equal those of the run without GDB.
- **The negative control.** A breakpoint at `gpio_nrfx_port_set_bits_raw`
  is set and hit through cortex-debug. It must mark the run and change its
  outputs.

`make vscode-templates`, in `make test`, checks the templates' settings
statically, including that the `endsPattern` still matches the line in
`bb`'s source.

On Windows, the templates were checked only through the Linux VS Code
above. With the repository in WSL, open the project through VS Code's
WSL remote, so that the task and GDB run in Linux next to `bb`.
