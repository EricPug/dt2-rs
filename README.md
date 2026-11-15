# dt2-rs

`dt2-rs` is an independent Rust project for analyzing Digitakt II firmware
formats and partially emulating selected firmware execution paths. It is
research software, not a complete or hardware-accurate device emulator. It does
not flash or modify device firmware, and the repository does not distribute
Digitakt II firmware.

## Current state

**Implemented:** SysEx/container inspection and section decompression; a partial
ColdFire V4e interpreter; modeled peripherals; diagnostic execution with
explicit synthetic inputs; UART display decoding; and scripts for tracing and
reproducing selected emulator runs. A documented OS 1.17 diagnostic path
reaches the SAMPLE page and loads sample data from a synthetic eMMC image.

**Experimental:** The clickable live viewer uses panel coordinates based on
Elekfake's reference measurements; it is not a hardware-accurate panel model.
A modeled-DSP path can write
WAV files from captured sample pages and note fields. Its DSP, timing, pitch,
gain, and input interpretation are assumptions; it is not firmware playback,
SHARC emulation, or evidence of hardware audio.

**Not implemented:** Complete ColdFire/peripheral coverage, general cold boot,
verified hardware behavior, persistent parameter editing, and firmware-driven
audio output. The detailed findings and limitations are in
[docs/section3-handoff.md](docs/section3-handoff.md), the authoritative project
status and evidence document.

## Architecture

- `dt2-fw` parses the firmware container format and decompresses sections.
- `dt2-core` provides the partial ColdFire CPU and modeled peripherals.
- `dt2-cli` runs diagnostics, renders display data, and offers optional UI and
  trace tools.
- `docs/` and `research/` record emulator observations, assumptions, external
  research, and unresolved questions.

The workspace requires Rust 1.80 or later for its default features. The
optional `window` feature currently requires Rust 1.88 or later for its image
dependency.

## Build and test

A clean clone needs no firmware or sample assets to build and run the test
suite:

```sh
cargo build --workspace
cargo test --workspace
cargo run -p dt2-cli -- render-pattern diagnostic-pattern.ppm
```

The `render-pattern` command creates a deterministic diagnostic image, not a
device screenshot. Firmware-container inspection is optional and requires a
firmware file obtained independently:

```sh
mkdir -p firmware
# Place your locally supplied firmware file here.
cargo run -p dt2-cli -- firmware/Digitakt_II_OS1.17.syx
```

To reproduce the documented diagnostic, first build `dt2-cli`, then use the
commands and locally prepared inputs described in
[docs/section3-handoff.md](docs/section3-handoff.md) and
[docs/ui-recipes.md](docs/ui-recipes.md). These runs use modeled peripherals
and synthetic inputs; they are not hardware validation.

## UI smoke test and local assets

`tools/ui-smoke.sh` is an end-to-end diagnostic, not a clean-clone test. It
expects locally prepared firmware-derived diagnostics and checkpoints under
`out/`, including:

- `out/section3-pre-entry-image.bin`,
  `out/section3-panel-discovery-pre-entry-ddr.bin`,
  `out/section3-panel-discovery-pre-entry-sram.bin`, and
  `out/dt2-os-1.17-container-nor.bin`.
- `out/shots/main-after-load.snap` and `.png`, plus
  `out/shots/action-down.snap` and `.png`.
- A populated synthetic eMMC image at `out/run-card.img`.
- Source WAVs at `samples/008 kick verb.wav` and
  `samples/303 clap real.wav`.

These are locally prepared inputs, not repository assets. The documented
procedures for preparing them are in the handoff documentation. The sample
WAVs are local-only; their redistribution rights are not established.

The script checks the diagnostic boot display, sample loading and browsing,
page changes, a temporary Tune readout change, and WAV comparison for the
modeled-DSP path. That WAV is synthesized by the model; it does not demonstrate
real firmware audio playback or device output. The script writes diagnostic
outputs under `out/ui-smoke/`; review its output paths before running it.

Firmware, sample WAVs, card images, snapshots, and extracted firmware sections
are not required for a clean build or test run. Keep locally supplied assets
in the ignored `firmware/`, `samples/`, and `out/` paths.

## Licensing

The project's original code and documentation are MIT-licensed;
see [LICENSE](LICENSE). This does not imply that every repository item or
external asset is MIT-licensed. Retained third-party material and its notices
are listed in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md). In particular,
the root MIT license does not apply to firmware or other assets supplied by
users; no firmware or sample assets are distributed here. Earlier commits
include an optional Unicorn/QEMU patch identified as GPL-2.0, not MIT; it has
been removed from the current tree. See the third-party notices for details.

## Further reading

- [docs/section3-handoff.md](docs/section3-handoff.md) — detailed current
  status, evidence, limitations, and diagnostic procedures.
- [docs/roadmap.md](docs/roadmap.md) — proposed development tasks and estimates.
- [docs/project-summary.md](docs/project-summary.md) — concise project
  overview.
- [tools/README.md](tools/README.md) — diagnostic scripts and tools.
