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
`DUMP_RAM_FILE=address:length:path` writes 1-64 MiB of mapped RAM/SRAM to a raw
binary file after execution; MMIO is not readable through this option.
`RAM_LOAD_FILE=address:path` restores a raw file into mapped RAM/SRAM before
execution, for resuming a saved diagnostic snapshot.
`STOP_PC=pc` stops before executing that instruction, allowing an exact handoff
snapshot with `DUMP_RAM_FILE`.
`PIT_BOOTSTRAP` advances modeled PIT counters by retired instruction, not bus
cycles. With `INTC2_BOOTSTRAP`, the observed PIT0 source can naturally reach the
partial INTC2 model; this remains an approximate timing model. `PIT0_ONESHOT` is
an intentionally behavior-changing probe that suppresses PIT0 requests after the
first one, useful for isolating post-interrupt code but not representative of
normal timer behavior. Runs print a bounded RTE history with restored PCs.
`PIT_FIRST_TICK_DELAY=n` adds `n` modeled ticks after PIT0 is first enabled and
before its initial counter interval; it is a timing probe, not a hardware clock
setting.
`PIT0_PERIODIC` seeds PIT0/INTC2 for periodic interrupt delivery when resuming a
saved section-3 handoff. `PIT2_PERIODIC` is an opt-in approximate 60 Hz PIT2
source-15 probe; it is not a measured OS1.17 timer rate. `WATCH_WRITE=address`
or `WATCH_WRITE=start:end` reports overlapping writes with old/new bytes, guest
PC, instruction count, and pre-instruction D/A/SR registers. `TRACE_REGS=pc`
reports the most recent 192 register
snapshots at one PC; `TRACE_TAIL=n` reports the last 1-1024 pre-instruction
snapshots with PC, SR, D0-D7, and A0-A7. `TRACE_A1=offset,...` captures A0/A1
pairs at output offsets. `SCHED_TRACE` logs scheduler list transitions with the
current TCB, mapped task entry, registers, and raw TCB/live/saved stack words.
`CONVERT_TRACE` records the conversion output index and the outer CMP/BGT flags
and branch result around `0x401811d2`.
Run logs list DSPI0/DSPI1 PUSHR words first, with instruction count, PC, PCS mask,
and post-write status. The DSPI model completes queued transfers instantly as a
diagnostic override, even when MCR.HALT/MDIS is set; it is not hardware timing.
`RAM_REMAP=from:to:length` aliases one bounded DDR window for a diagnostic run.
`RAM_SNAPSHOT=watch:src:dst:length:trigger_start:trigger_end:read_start:read_end`
copies the input window when the watched write occurs in the trigger PC range and
redirects reads from the source window only in the selected decoder PC range;
writes remain at their original addresses. This deliberately changes memory
aliasing to test source/output overlap and is not a hardware memory model.
`UART8_TX` captures transmitted UART8 bytes; `UART8_TX_READY` seeds TXRDY/TXEMP
at startup without injecting a panel response; `UART8_RX=hex` queues explicit
receive bytes for a diagnostic handshake. These options do not model serial
timing or eDMA.
`ESDHC_EMMC` attaches a synthetic blank eMMC. `ESDHC_FILE=path` instead uses a
sparse 4 GiB raw backing image that persists card blocks across separate runs;
`ESDHC_TRACE` logs command arguments, block ranges, completed writes, and eDMA
channel-59 activity. The host image and command responses are synthetic, not
hardware evidence. A restored snapshot must use the same `ESDHC_FILE` path.
`PANEL_STUB` answers section 2's `60 01` panel-discovery request through its
polled SRAM ring, then answers the section-3 UART8 handshake and DMA-delivers a
group-0 button press, encoder-0 delta, and button release after TCD34 is enabled.
Its panel records are synthetic diagnostics, not hardware behavior.
`DTIM1_BOOTSTRAP` models DMA Timer 1 reference compare and INTC0 source 33;
`DTIM1_TICKS_PER_INSTRUCTION=n` sets an approximate instruction-to-bus-tick
scale. `DSPI2_BOOTSTRAP` enables the DSPI2 register/FIFO model with no attached
slave, so reads return the diagnostic `0xffff` fill. These are opt-in probes,
not evidence of an attached timer clock or peripheral response.
These are opt-in approximations; section-2 direct entry is not an SBF boot.

`trace-depack <packed-section.bin> [step output-end [output-start]]` logs host
aPLib input cursors at selected output lengths. The verified section-3 handoff,
periodic resume command, snapshots, callback-table check, and current device
limitations are recorded in [docs/section3-handoff.md](docs/section3-handoff.md).
Snapshot binaries and logs are local ignored artifacts under `out/` and are not
part of the repository.
