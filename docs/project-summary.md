# Project overview

## Project

`dt2-rs` is an independent Rust project for firmware-format analysis and
partial emulation of selected Digitakt II firmware execution paths. It
combines a partial ColdFire V4e interpreter, modeled peripherals, diagnostic
execution, firmware-container utilities, and trace-analysis tools. It is
research software, not a complete or hardware-accurate emulator.

## Current technical state

The workspace can inspect and decompress firmware containers, execute selected
diagnostic paths with modeled hardware, decode display traffic, and reproduce
some sample-loading behavior using synthetic card inputs. The documented OS
1.17 run reaches a sample page under an explicitly prepared diagnostic setup.
Peripheral and processor coverage is incomplete. Hardware behavior has not
been established merely by matching emulator traces. Firmware-driven audio
playback is not implemented; modeled-DSP WAV output is a separate experimental
path and is not device or firmware audio.

## Planned work

I intend to continue the project by improving ColdFire and peripheral
coverage, building stronger regression and trace-reproduction tests,
documenting further firmware paths, and seeking independently verifiable
hardware observations. The [roadmap](roadmap.md) lists concrete tasks and
estimated effort; those estimates are planning estimates, not guarantees.
This work is substantial because it requires continued CPU and peripheral
implementation, experimental tracing, test-fixture development, and careful
separation of observed behavior from assumptions.

## Technology and licensing

The project uses Rust (minimum 1.80 for default features; 1.88 for the optional
window feature) in a Cargo workspace, with shell and Python utilities for
diagnostics and analysis. The original project code and
documentation are MIT-licensed. Retained third-party material
and notices are listed in [THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md);
the MIT license is not asserted to cover every repository item.

The repository does not distribute Digitakt II firmware, extracted firmware
sections, sample WAVs, card images, or other third-party device assets. Local
test assets must be supplied separately.
