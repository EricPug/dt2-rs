# Verified Section-3 Handoff

This is a diagnostic handoff checkpoint for the local Digitakt II OS 1.17 image. It uses modeled/synthetic peripherals and a source-only RAM shadow to preserve the packed section during its overlapping decompression. It is not a hardware-faithful boot. All firmware-derived binaries and snapshots remain under ignored `out/` and are not committed.

## Checkpoints

The pre-entry run stops before `0x400004e8`. Its clean decoded image is `out/section3-pre-entry-image.bin`; full DDR and SRAM snapshots are `out/section3-pre-entry-ddr.bin` (64 MiB) and `out/section3-pre-entry-sram.bin` (64 KiB). The callback descriptor at `0x4031153c` matches the host section: count `0xa1`, first function `0x40198e3c`, second function `0x401999f8` at `0x40311548`.

The periodic resume snapshots are `out/section3-dspi-ctar-idle-ddr.bin` and `out/section3-dspi-ctar-idle-sram.bin`; the complete 8-bit CTAR0 run log and ordered PUSHR/POPR list are `out/section3-dspi-ctar-full-log.log`. The callback descriptor remains unchanged in that idle snapshot.

The section image compares exactly against the host decoder: 3,275,616 bytes, zero differing bytes and ranges.

## Reproduction

Capture the pre-entry checkpoint with source-only shadowing and the CR27 preset:

```sh
cargo build --release -p dt2-cli
target/release/dt2-cli run-bin out/section_2.bin 0x40000000 0x08000000 0x800003fc 0x80000de8 380000000 \
  A7=0x80010000 SR=0x2700 MCF5441X_SBF_BOOT MCF5441X_SRAM_OVERLAY \
  SYNTHETIC_MCF_MMIO PIT_BOOTSTRAP INTC2_BOOTSTRAP PIT0_ONESHOT \
  PIT_TICKS_PER_INSTRUCTION=1024 UART8_TX DDR_BOOTSTRAP \
  DSPI0_FLASH=1:out/dt2-os-1.17-container-nor.bin \
  DSPI0_CTAR0=0x3e000020 \
  MMIO16@0xEC09000E=0x187D MMIO16@0xEC09001A=0 \
  MMIO32@0xFC0C0004=0 MMIO32@0xFC0C0008=0x10 MMIO32@0xFC0B806C=0x8 \
  RAM_SNAPSHOT=0x403182bf:0x40200000:0x44000000:0x1182c0:0x80004980:0x80004984:0x80005740:0x80005870 \
  STOP_PC=0x400004e8 DUMP_RAM=0x4031153c:32 \
  DUMP_RAM_FILE=0x40000000:0x4000000:out/section3-pre-entry-ddr.bin \
  DUMP_RAM_FILE=0x80000000:0x10000:out/section3-pre-entry-sram.bin \
  DUMP_RAM_FILE=0x40000400:0x31fb60:out/section3-pre-entry-image.bin \
  > out/section3-pre-entry-snapshot.log
```

Resume from that clean snapshot with periodic PIT0, normal INTC2 delivery, and the DSPI1 register/FIFO model:

```sh
target/release/dt2-cli run-bin out/section3-pre-entry-image.bin 0x40000000 0x08000000 0x40000400 0x400004e8 380000000 \
  MCF5441X_SBF_BOOT MCF5441X_SRAM_OVERLAY SYNTHETIC_MCF_MMIO \
  PIT_BOOTSTRAP INTC2_BOOTSTRAP DSPI1_BOOTSTRAP \
  PIT_TICKS_PER_INSTRUCTION=1024 UART8_TX DDR_BOOTSTRAP \
  DSPI0_FLASH=1:out/dt2-os-1.17-container-nor.bin \
  DSPI0_CTAR0=0x3e000020 \
  MMIO16@0xEC09000E=0x187D MMIO16@0xEC09001A=0 \
  MMIO32@0xFC0C0004=0 MMIO32@0xFC0C0008=0x10 MMIO32@0xFC0B806C=0x8 \
  RAM_LOAD_FILE=0x40000000:out/section3-pre-entry-ddr.bin \
  RAM_LOAD_FILE=0x80000000:out/section3-pre-entry-sram.bin \
  PIT0_PERIODIC \
  D0=0x40 D1=0x980 D2=0xf50d2004 D3=0x7f6b778b \
  A0=0x400004e8 A1=0x4031ff60 A2=0x80008404 A3=0 \
  A6=0x80009ab0 A7=0x80009a70 SR=0x2700 \
  RAMBAR=0x80000235 ACR0=0 VBR=0x80000000 CACR=0xa50ce100 \
  TRACE_REGS=0x400cc9c8 DUMP_RAM=0x4031153c:32 \
  DUMP_RAM_FILE=0x40000000:0x4000000:out/section3-dspi-ctar-idle-ddr.bin \
  DUMP_RAM_FILE=0x80000000:0x10000:out/section3-dspi-ctar-idle-sram.bin \
  > out/section3-dspi-ctar-full-log.log
```

`PIT0_PERIODIC` configures PCSR `0x053f`, PMR `0xa121`, INTC2 source 13 at level 1, and unmasks source 13. It does not suppress later PIT0 requests. `RAM_LOAD_FILE` restores a mapped RAM/SRAM image before execution.

## Observations

- The run delivered periodic PIT0 source 13/vector `0xcd`; it recorded 269,688 RTEs by 380 million instructions.
- RTE restored PCs in the `0x400cc87c`, `0x400cc882`, `0x400cc886` loop, and later `0x4013705a..0x40137066`.
- The callback runner at `0x400cc9c8` was entered. The verified table count and pointer were `0xa1` and `0x401999f8`.
- The MCF54418 DSPI status layout is TXCTR `[15:12]`, TXNXTPTR `[11:8]`, RXCTR `[7:4]`, POPNXTPTR `[3:0]`. Each accepted PUSHR is logged first with instruction count, guest PC, raw word, PCS mask, and post-write DSR; each POPR read is logged with its reply and post-pop status. The complete run logged 185,033 PUSHR writes and 184,996 POPR reads without truncation.
- DSPI1's nine PUSHR writes comprise three groups, each bracketed by `0x20000000` and EOQ `0x28000000`; the data words with PCS mask `0x02` are `0x2002a268`, `0x2002a00f`, and `0x2002a113`. No DSPI1 slave is attached, so each reply is the diagnostic `0xffff` fill.
- DSPI0 issues seven command-`0x03` reads, all on PCS mask `0x02`, with a three-byte NOR address after the command. Ranges are `0x080000+0x24`, `0x080020+0x14`, `0x080030+0x14`, `0x080040+0x14`, `0x080050+0x14`, `0x080060+0x14`, and `0x1a4280+0x2d21c` (end-exclusive). The first six cover `0x80000..0x80074`, rereading 20 bytes in overlapping windows; the large read covers `0x1a4280..0x1d149c`. Total data clocks are 184,996, matching the POPR read count. The first request begins with PUSHR words `0x84020003`, `0x80020008`, `0x80020000`, `0x80020000`; with the restored 8-bit CTAR0, POPR returns the NOR signature bytes `45 4c 45 33` after the command/address phase. CTAR0 `0x3e000020` is essential; reset CTAR0 is 16-bit. The RXCTR poll at `0x4013705a` passes, and execution returns to the `0x400cc912` idle loop after 380 million instructions.
- First MMIO after section-3 entry is a synthetic write8 at PC `0x40000500` to `0xfc04002d` with value `0x11`; that is not display-path evidence. No UART8/9 payload traffic was observed. DSPI transfers are device-transaction evidence, not proof of display payload.
- INTC2 unmasked sources 13, 15, 29, and 30. Source 13 is PIT0 and is delivered by the model. Sources 29/30 are configured at levels 6/5 but have no ADC producer model, so they are not delivered. Source 15 is configured at level 3; no corresponding pending producer is modeled.
- No UART8/9 accesses were observed. The `0xec094xxx` accesses are GPIO registers, not UART9. The first section-3 MMIO trace entries include synthetic MCF register writes; they are not evidence of display output.

This completion mode intentionally executes queued PUSHRs immediately even when MCR.HALT/MDIS is set; it is a diagnostic instant-clock override, not hardware-faithful MCR gating. The PUSHR/POPR log is the primary evidence for the transfer sequence. These responses and bus events identify the polled device path but do not yet establish display payload. Synthetic MMIO responses are diagnostic approximations.
