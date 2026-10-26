# dt2-rs

An early-stage Digitakt II emulator and firmware analysis toolkit written in Rust.
Not affiliated with or endorsed by Elektron.

The CLI can inspect and unpack SysEx firmware containers, and run raw binary images
on a partial ColdFire V4e core with configurable RAM and MMIO seeds. Optional models
cover MCF5441x SRAM/SBF reset-vector mapping, DSPI0 with a W25Q128JV SPI NOR device,
the DDR controller registers observed during bootstrap, and an opt-in synthetic MMIO
diagnostic mode.

The emulator can execute diagnostic firmware traces under explicit or synthetic
startup assumptions. It does not yet boot the full operating system: DDR timing and
SDRAM behavior, later peripherals, the display, controls, and audio are incomplete.

```sh
cargo test
cargo run -p dt2-cli -- path/to/firmware.syx
cargo run -p dt2-cli -- path/to/firmware.syx out
cargo run -p dt2-cli -- render-pattern out/diagnostic-pattern.ppm
```

`render-pattern` writes a deterministic 320x240 PPM color-bar and grayscale image.
It is an emulator diagnostic artifact, not a screenshot of the Digitakt II OS.

`run-bin` accepts a raw binary image, RAM mapping, load address, entry point (or
`SBF_RESET_VECTOR`), instruction limit, and optional register/RAM/MMIO overrides.
Its usage text lists the available diagnostic options.

For the current section-3 direct-entry experiment, `MCF5441X_SRAM_OVERLAY` combines
the flat DDR image region with the documented 64 KiB mirrored SRAM window.
`SYNTHETIC_MCF_MMIO` is an explicit approximation that returns zero for unclaimed
peripheral reads and drops writes; it does not model hardware reset values or timing.
`INSPECT_TRAP0` prints the live TRAP #0 vector after execution stops.
`DUMP_RAM=address:length` prints 1-256 bytes from mapped RAM/SRAM after execution;
it does not read MMIO.
`PIT_BOOTSTRAP` advances modeled PIT counters by retired instruction, not bus
cycles. With `INTC2_BOOTSTRAP`, the observed PIT0 source can naturally reach the
partial INTC2 model; this remains an approximate timing model. `PIT0_ONESHOT` is
an intentionally behavior-changing probe that suppresses PIT0 requests after the
first one, useful for isolating post-interrupt code but not representative of
normal timer behavior. Runs print a bounded RTE history with restored PCs.
`WATCH_WRITE=address` reports the first 16 CPU writes overlapping one address,
and `TRACE_REGS=pc` reports the most recent 16 register snapshots at one PC.
`UART8_TX` captures transmitted UART8 bytes but does not model receive data or
serial timing. These are opt-in diagnostic approximations; section-2 direct
entry is not an SBF boot.
