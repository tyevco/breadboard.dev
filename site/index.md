---
layout: home

hero:
  name: Breadboard
  text: QEMU for whole circuit boards
  tagline: Give it firmware and a board, and it simulates both together. CPU engines run the firmware, analog engines solve the circuit, and a kernel that owns simulated time couples them at the pin boundary.
  actions:
    - theme: brand
      text: Quickstart
      link: /guide/quickstart
    - theme: alt
      text: What is Breadboard?
      link: /guide/introduction
    - theme: alt
      text: GitHub
      link: https://github.com/tyevco/breadboard.dev

features:
  - title: Real firmware on a real KiCad board
    details: Phase 1 imports dial-me-spinner rev A from its KiCad netlist (201 parts, 142 nets), resolves its parts to component packages, and runs prebuilt Zephyr v4.5.0 firmware on the nRF52840 inside the board's Raytac module.
    link: /guide/demo
    linkText: The E1 demo
  - title: Renode and ngspice, co-simulated
    details: Renode runs the MCU, ngspice solves the analog part of the board, and both advance only on time grants from a Rust kernel. Values cross between them only through pins bound to nets.
    link: /concepts/cosim
    linkText: The time-grant protocol
  - title: Deterministic, byte for byte
    details: Integer femtosecond time, no wall-clock reads and fixed record order. The same inputs and seed give byte-identical nets.csv, nets.vcd and events.jsonl, and the test gates check it against committed hashes.
    link: /concepts/determinism
    linkText: Determinism
  - title: Lookahead speed-ups that change no result
    details: Lattice-aligned long grants, edge traces and CPU lookahead cut the number of grants (for example 117 instead of 50 000 on the feedback board), and every speed-up is gated against its one-cell reference.
    link: /concepts/performance
    linkText: Measured cost
  - title: Component packages
    details: Each part is a local package (part.toml, pins.toml, SPICE views with provenance and confidence, test vectors), pinned by tree hash in bb.lock together with the engine platform's content hash.
    link: /reference/packages
    linkText: Package format
  - title: Honest about fidelity
    details: run.json lists every analog view with its provenance, confidence and qualification, and bb run flags views that are estimated or agent-generated.
    link: /concepts/fidelity
    linkText: Fidelity and honesty
---
