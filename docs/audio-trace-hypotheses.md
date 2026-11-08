# Audio Handoff Hypothesis Log

This is the evidence ledger for the firmware-driven sample-to-audio path. Project
status and milestone order remain in [section3-handoff.md](./section3-handoff.md).
The reproducible capture is `tools/audio-edma-trace.sh`; raw firmware-derived
logs, RAM images, and snapshots stay under ignored `out/audio-edma-trace/`.
`DSP_LINK` opens the firmware's DSPI2 gate and supplies `0xffff` reply words; it is
a diagnostic stand-in, not a DSP model or evidence of audible playback.

| Candidate | Status | Evidence and limits |
|---|---|---|
| Missing SSI worker/service call | Ruled out for the recorded trigger/play run | The active SSI worker is called each block and clears its 2 KiB TX source. With the mode-2 path, its USB-list pointer is null and it skips list processing. This does not explain or rule out sample-voice activation. |
| Per-block DSPI2 eDMA 28/29 stream carries PCM or the sample address | Ruled out for the observed stream | `DSP_LINK` captured a 5,496-byte source block of 1,374 32-bit PUSHR entries; each entry carries a tag/control halfword and one transmitted data halfword. The receive TCD reads 1,374 16-bit words (2,748 bytes). Values change at key events, but no sample address or sample-data n-gram appears in trigger/play or sample-load captures. A version-labeled Digiemu analysis independently identifies the same 1,374-frame / 2,748-byte full-duplex DSPI2 exchange. This is a parameter/state path; direct shared-memory reads are not excluded. |
| Fixed DSPI2 RX reply gates sample playback | No requested behavior change observed | Channel 28 writes 1,374 reply halfwords to `0x80001000`; caller `0x400cd448` copies `0xabc` bytes to `0x8000488c` through helper `0x40136388`. The helper reads/writes 16-byte chunks at `0x4013639e`/`0x401363a4`, with a length-driven loop at `0x401363ac`. The copied values are subsequently read at `0x4002f0e6` (eight halfwords at `0x80005328..0x80005336`) and `0x4002f10e` (`0x800048a2`). The scan loop at `0x4002f0f0` is index-driven. The `TST.L D0` at `0x4002f126` saw the same D0 sequence (0, 1 alternating) for both replies; the following `BGE` at `0x4002f128` therefore takes the same path. Matched 25M-instruction runs with `0xffff` and `0x0000` had 226,398 CPU writes each and 249 DSPI2 blocks each (10 channel-28 reply blocks in offsets 6M..7M); the sample-span watch recorded zero accesses in both. SSI captures were both 509,952 zero bytes with identical SHA-256 `53f1414830fecff40fcea5e308a74d60e2c6b655c469d16992fa5d0ae4797fd6`. The word values do change in the copied SRAM, but CPU write volume, DSPI2 block count, sample access, and SSI output do not. This only tests the emulator's synthetic reply path, not physical DSP behavior. Logs: `out/dspi2-reply-experiment/ffff.log`, `out/dspi2-reply-experiment/0000.log`, and the matched consumer traces. |
| TRIG/PLAY input is dead before the audio handoff | Ruled out for trigger delivery; audio handoff unresolved | Matched idle and TRIG/PLAY runs from `main-after-load.snap` traced the same 1,000,000-instruction window after the TRIG packet. The input arrived by UART8 DMA; the triggered window had 20,315 CPU writes, 2 UART RX DMA writes, and two DSPI2 blocks, while idle had 110 CPU writes and no DSPI2 blocks. After subtracting the identical 21,120 synthetic SSI channel-50 writes, trigger-added writes were the sequencer/UI activity plus two channel-28 receive blocks (2,748 words). The two DSPI2 data payloads changed from the prior idle block only at counter halfword indices 270 and 462. Neither payload contains `0x4060`, `0x5770`, `0x8000`, `0xd770`, or `0xd776`; `0x012c`, `0x0006`, and six `0001 0000` pairs occur unchanged in the prior idle payload. These are not evidence of a sample address, sample offset, or trigger-dependent length. `DSP_LINK` supplies fixed `0xffff` replies, so this confirms firmware activity in the diagnostic path, not physical DSP behavior or audible playback. Logs: `out/trigger-window-experiment/idle.log` and `out/trigger-window-experiment/trig-play.log`. |
| A separate bulk eDMA transfer carries sample PCM during sample load or trigger/play | No bulk transfer observed in these runs | `EDMA_TRACE` saw channels 14, 15, 28, 29, 35, and 59 during sample load; only 28, 29, and synthetic panel RX channel 35 during trigger/play. Channel 59 performed two 32 KiB eMMC reads into `0x4060d770` and `0x40605770`. Channel 50 SSI TX is scheduled by the emulator's timer diagnostic rather than an observed firmware SERQ request. This only rules out other eDMA request starts in these exact diagnostic runs. |
| The post-load DSPI1 eDMA 14/15 transfer is sample payload | Ruled out as bulk PCM; role unresolved | It runs once after the eMMC reads. Channel 15 sends 8 framed bytes from `0x4fe7a340` to DSPI1 PUSHR (`8001 0001 0801 012c`); channel 14 is configured for two 16-bit reads. Disassembly traces the request through `0x4003314e` to helper `0x40012562`, which constructs the 4-byte request `0001 012c`: word `0001` is set by the helper, and `012c` is copied from a caller argument. Related OS 1.16 research labels this helper `small` and describes the packet as two big-endian `u16` fields (`kind`, `slot`), suggesting kind 1 / slot 300; the same source warns that the waveform cache had not yet been shown calling this transport. The entry trace shows caller return `0x4019fc7e`, argument `0x012c`, and pointer `0x4525722c`; that caller is in a routine iterating over `0x64400` bytes in `0x191`-byte strides, but its context is not yet identified. The helper passes request length 4 and response length `0x300` to `0x400124a0`, using response buffer `0x4fe79340`; the observed channel-14 transfer reads only two halfwords. Digiemu's older-version notes likewise classify DSPI1 channels 14/15 as short opcode request/reply transactions, probably to a codec. These are useful cross-checks, not proof this exchange is an audio handoff. The watched `0x43176a34` and `0x43176a38` writes are the saved A6 from `LINK` and return address from `JSR`, not additional request fields. |
| CPU or modeled eDMA reads/writes the loaded sample during trigger/play | No accesses observed in this run | The two channel-59 eMMC transfers wrote adjacent 32 KiB windows spanning `0x40605770..0x40615770`. Replaying trigger/play from the loaded snapshot with an exact span watch produced zero CPU-bus or modeled eDMA accesses. This excludes only the emulator-observable access paths, not an external DSP or other unmodeled hardware reader. |
| The OS 1.17 accesses at `0x8c000000..0x8c00000f` carry sample-page descriptors | Ruled out for the observed accesses | With `RGPIOBAR=0x8c000035`, this 16-byte window is decoded as RGPIO in the current OS 1.17 trace. The sample-load capture records 135,300 RGPIO events, including repeated reads at `+0x2` and writes at `+0x2`/`+0xa`, consistent with GPIO polling and strobes rather than descriptor memory. The runner enables `RGPIO_READY_MODEL`, so the modeled pin-0 ready response is synthetic; the access addresses and operations are firmware-observed. Digiemu's OS 1.15C/1.16 findings label `0x8c000000` as a sample-page/slot-header region; that older-version clue does not establish the OS 1.17 decode or handoff. This conclusion is limited to the RGPIO window, not the surrounding address space. |
| The DSPI1 response buffer at `0x4fe79340..0x4fe79640` is populated or consumed during sample load | inconclusive | A repeat sample-load run starts with all `0x300` bytes zero and records zero overlapping CPU-bus reads and writes. The channel-14 TCD is configured for two 16-bit reads from DSPI1 POPR into the buffer, but the emulator's synthetic DSPI diagnostic does not execute eDMA channels 14/15. Therefore the empty buffer is evidence about this emulator run, not evidence that real hardware returns no response. |
| DSP reads the sample directly from shared DDR/FlexBus, or receives a descriptor through another path | Unresolved | The sample bytes are present in ColdFire DDR, while the DSPI2 stream has no sample address. The OS 1.17 SHARC loader contains no literal `0x40605770`/adjacent sample address and no recognizable voice/sample descriptor table was identified by the raw scan. It remains possible that firmware computes or relocates the address, or uses another interface; do not infer a physical bus or peer from the emulated MMIO model. |
| `SYNTH_VOICE` output demonstrates firmware playback | Rejected as progress evidence | It injects sample bytes directly into the SSI TX source and creates nonzero output, but does not exercise the firmware's handoff. Keep it diagnostic-only; the trigger/play SSI capture remains zero. |

## 32-byte sample signature scan

The signature `010e0110010a010300fb00fb00fe00ff00f900f200ec00ec00db00ce00cb00c7`
was taken from `0x40605770`, the loaded sample's first 32 bytes. All sparse
snapshot pages for emulated RAM (`0x40000000..0x4fffffff` and
`0x80000000..0x8000ffff`) were searched; omitted pages are zero and cannot
contain this nonzero signature. Checkpoints:

| Checkpoint | Instruction count | Exact matches |
|---|---:|---|
| Right after load | 9,400,000,000 | `0x40605770` only |
| 200k instructions after TRIG | 9,406,200,000 | `0x40605770` only |
| 200k instructions after PLAY | 9,418,200,000 | `0x40605770` only |

No second copy appeared at these checkpoints. A `SAMPLE_ACCESS_TRACE` on the
source window `0x40605770..0x40605790` across the full 20M-instruction
TRIG/PLAY replay recorded zero CPU or modeled-DMA accesses. This exact-signature
test does not detect transformed copies or an external DSP's memory reads.

The saved TCD map at the TRIG checkpoint and the `EDMA_TRACE` SERQ records show
the following relevant configurations and requests. Length is `NBYTES ×
CITER`; “requests” counts SERQs in the indicated 1M-instruction window.
Channel 28/29 handling is the synthetic `DSP_LINK` diagnostic, not physical
eDMA execution.

| Channel | SADDR → DADDR | Length | TRIG / PLAY observation |
|---:|---|---:|---|
| 14 | `0xfc03c03a` → `0x4fe79340` | 4 B | Retained DSPI1 TCD; no request |
| 15 | `0x4fe7a340` → `0xfc03c034` | 8 B | Retained DSPI1 TCD; no request |
| 28 | `0xec03803a` → `0x80001000` | 2,748 B (`2 × 0x55e`) | 10 requests in each TRIG and PLAY window; synthetic reply fill |
| 29 | `0x80001bc0` → `0xec038034` | 5,496 B (`4 × 0x55e`) | 10 requests in each window; outgoing block capture |
| 34 | `0xec07000c` → `0x00000000` in saved TCD map | 1 B | UART8 input is specially injected by the emulator: the TRIG packet wrote two bytes to `0x4fe3a042..0x4fe3a043`; no SERQ/TCD transfer is modeled |
| 35 | `0x4fe3b21f..0x4fe3b243` (TRIG), `0x4fe3b49a..0x4fe3b4e1` (PLAY) → `0xec07000c` | `NBYTES=1`, variable `CITER` | 13 requests / 38 B in the TRIG window; 23 requests / 72 B in the PLAY window |
| 36 | `0xec07400c` → `0x4fe30000` | 1 B | Retained TCD; no request |
| 37 | `0x00000000` → `0xec07400c` | 1 B | Retained TCD; no request |
| 48 | `0xfc0bc008` → `0x4fe57100` | 2,048 B (`0x20 × 0x40`) | Retained TCD; no request |
| 50 | `0x4fe58100` → `0xfc0bc000` | 2,048 B (`0x20 × 0x40`) | SSI timer diagnostic only; not a firmware SERQ |
| 59 | `0xfc0cc020` → `0x40605770` | 32,768 B (`0x10 × 0x800`) | The prior eMMC sample-load TCD remains configured, but no request occurred near TRIG or PLAY |

Channel 59 is the important qualification: a TCD still points *to* the original
sample because the eMMC loader's completed descriptor remains in the register
map. It is not a second copy or a trigger-time transfer. No observed TRIG/PLAY
SERQ points at the sample, and the source-window access watch is empty.

## OS 1.17 SHARC loader scan

Extracted ELE3 section 7 from `firmware/Digitakt_II_OS1.17.syx` with the
repository's `dt2-cli`; the 321,016-byte result is
`out/sharc-os117/section_7.bin` (SHA-256
`0f514a12a2255f5c081e292c47f1f29462003177658da4bbae0a22fd737fffa2`). It
parses as 104 16-byte little-endian loader records: each header XOR is zero,
the fill records carry no payload, and the final record ends exactly at EOF.
The header layout and fill handling match the public
[ADI `adsp-ldr` SC5xx loader parser](https://github.com/analogdevicesinc/adsp-ldr/blob/main/src/ldr/lfd_sc589.c).

The encoded target ranges below are envelopes; the 0x2824/0x2838/0x8000
envelopes contain sparse data and fill blocks, not one continuous initialized
region. The `0x00120230` and `0x001c1338` values occur in header-only loader
records, not as payload ranges.

| Loader target range | Record shape |
|---|---|
| `0x20000000..0x2001ab7c` | One data block |
| `0x28240000..0x282d7564` | Sparse data and fill blocks |
| `0x28380000..0x2839c000` | Sparse data and fill blocks |
| `0x80000000..0x8055e890` | Sparse data and fill blocks |
| `0x80a00000..0x82a001c4` | One fill-only block |

A two-endian scan of all block payloads found no literal `0x40605770`,
`0x4060d770`, or `0x40615770`. The few aligned `0x4060xxxx` values do not
resemble that pointer: for example, `0x40602f3d` and `0x406066c0` sit in
smooth numeric tables and decode as floating-point values near 3.5. Raw
`0x40000000`/`0x80000000` bit patterns elsewhere are not by themselves evidence
of ColdFire DDR/SRAM references; the loader's `0x80000000` targets belong to
the DSP image's encoded target-address space.

No sample/voice descriptor list was recognizable from the literal-address
scan. This is a static negative result, not proof the SHARC never reads the
sample: an address can be computed, relocated, or supplied through an indirect
descriptor or interface. The memory map and exact consumer path remain
unresolved; this result does not justify implementing a SHARC core or eDMA.

## Latest reproducible trace

Run:

```sh
tools/audio-edma-trace.sh
```

In the 200M-instruction sample-load phase, the trace records two channel-59
eMMC transfers and one channel-15/14 DSPI1 exchange after the load. The
channel-14/15 requests use TCDs with a two-iteration count; the watched source
buffer changes from zero to `80 01 00 01 80 01 01 2c`, then firmware sets the
last tag's end bit, leaving `80 01 00 01 08 01 01 2c` for transfer. The channel
15 TCD sends those 8 bytes to `0xfc03c034`; channel 14 reads two halfwords from
`0xfc03c03a` into `0x4fe79340`. The synthetic DSPI1 model does not execute
eDMA channels 14/15, so its reply buffer is not hardware evidence.

The runner also watches the producer's stack around `0x4003314e`. The nested
helper `0x40012562` builds a 4-byte request `0001 012c`: it sets word `0001`
and copies the caller's `0x012c` argument. `0x40033160` pushes that argument;
`0x40012568` copies it into the request and `0x4001257a` writes the first word.
The nearby writes of `0x40001608` and `0x4003316a` are respectively the saved
A6 from `LINK` and the return address pushed by `JSR`, not request fields.
Although the helper passes a 4-byte request and `0x300` response length to
`0x400124a0`, the observed channel-14 TCD reads only two 16-bit values. The
entry trace identifies the caller as `0x4019fc78` (returning to `0x4019fc7e`)
with arguments `0x012c` and `0x4525722c`. That call follows an iteration over
`0x64400` bytes in `0x191`-byte strides; the routine's purpose and the reply's
source, contents, and relationship to sample handoff are unresolved.

The runner repeats sample load with CPU read/write watches on the `0x300`-byte
response buffer. It starts as all zeroes, with no overlapping CPU reads or
writes. Since the emulator does not execute the DSPI1 channel-14/15 DMA
transaction, this does not establish what a hardware response contains. The
same capture's RGPIO trace shows the 16-byte `0x8c000000` window being used for
GPIO polling/strobes, not sample-page data.

## Cross-project checks

[Digiemu's Digitakt II findings](https://github.com/irpina/digiemu/blob/6f2d619fd1bc9579a62868688fc6bdd1dcf844db/docs/FINDINGS.md#L399)
target OS 1.15C and include checks against 1.16. They describe a fixed
1,374-frame / 2,748-byte full-duplex DSPI2 exchange on eDMA 28/29, matching the
frame count in this OS 1.17 trace. The same notes identify FlexBus
`0x8c000000` as carrying sample pages and slot headers, and classify DSPI1
channels 14/15 as short opcode requests/replies, probably to a codec. This is
version-specific evidence, not a peripheral model or an assumption that the
older routing is unchanged. In the active OS 1.17 configuration,
`0x8c000000..0x8c00000f` is the RGPIO register window; the image's writes to
`0x8c000000..0x8c000006` at `0x400cd298..0x400cd2ac` are therefore not evidence
of sample-page descriptors.

[Related OS 1.16 research](https://github.com/armio/dn2-dt2-firmware-research/blob/0d1244dff0d85eb829dc4681611b3f3887d69702/port/main/tree/scripts/build_dt2_waveform_rpc.py)
labels `0x40012562` as the small request helper, describes the 4-byte request
as big-endian `kind,slot` words, and identifies response/DMA storage beginning
at `0x4fe79340`. It explicitly leaves the waveform cache's use of that
transport unproven, so the labels narrow what to trace but do not establish an
audio handoff.

In the 100M-instruction trigger/play phase, the firmware makes 999 requests on
each DSPI2 channel (28 and 29), plus 1,775 synthetic panel-input channel-35
requests; no DSPI1 SERQ is observed. SSI TX captures 999 blocks / 2,045,952
bytes, all zero. The emulator's synthetic SSI channel-50 servicing is outside
the firmware SERQ trace.

## Non-RAM bulk transfer experiment

`NONRAM_BULK_TRACE` records every CPU write run or completed eDMA transfer of at
least 1 KiB to a non-RAM destination, including PC, destination, size, and the
first 16 bytes. The classification uses the configured RAM window even when a
watch disables the RAM fast path. The exact per-transfer records are in
`out/audio-edma-trace/sample-load.log` and `out/audio-edma-trace/trig-play.log`;
the runner now checks the counts and absence of qualifying CPU writes.

| Phase | Actor | Count | PC | Destination / size | Source | First 16 bytes |
|---|---|---:|---|---|---|---|
| Sample load | DMA29, synthetic `DSP_LINK` | 1,999 | `0x400cd5d2` | `0xec038034` / 5,496 B | `0x80001bc0` | `80010003800100008001000080010000` |
| Sample load | DMA50, modeled SSI servicing | 1,000 | Per-event PC in log (289 distinct PCs) | `0xfc0bc000` / 2,048 B | `0x4fe58900` | all zero |
| Sample load | DMA50, modeled SSI servicing | 999 | Per-event PC in log (294 distinct PCs) | `0xfc0bc000` / 2,048 B | `0x4fe58100` | all zero |
| TRIG/PLAY | DMA29, synthetic `DSP_LINK` | 64 | Per-event PC in log | `0xec038034` / 5,496 B | `0x80001bc0` | `80010003800100008001000080010000` |
| TRIG/PLAY | DMA29, synthetic `DSP_LINK` | 570 | Per-event PC in log | `0xec038034` / 5,496 B | `0x80001bc0` | `8001000380013c008001000080010000` |
| TRIG/PLAY | DMA29, synthetic `DSP_LINK` | 365 | Per-event PC in log | `0xec038034` / 5,496 B | `0x80001bc0` | `8001000380013c008001000080012d00` |
| TRIG/PLAY | DMA50, modeled SSI servicing | 500 | Per-event PC in log (6 distinct PCs) | `0xfc0bc000` / 2,048 B | `0x4fe58900` | all zero |
| TRIG/PLAY | DMA50, modeled SSI servicing | 499 | Per-event PC in log (7 distinct PCs) | `0xfc0bc000` / 2,048 B | `0x4fe58100` | all zero |

No qualifying CPU bulk-write run or other non-RAM DMA destination was observed.
DMA29 is the diagnostic DSPI2/PUSHR path, not physical DSP evidence. DMA50 is
the emulator's timer-scheduled SSI service, not a firmware-triggered hardware
transfer. The sample-load channel-59 writes go to DDR and are correctly excluded
as RAM destinations. The transfer records are evidence about this emulator run
only; they do not establish a physical DSP sample-delivery mechanism.

## Digiemu mk1 cross-check

Digiemu's [Digitakt mk1 audio notes](https://github.com/irpina/digiemu/blob/master/docs/mk1/11-audio.md)
place the sample engine on the ColdFire and describe SSI1 as its audio output;
they do not show a separate DSP sample-upload path. Its
[`emu/samples.py`](https://github.com/irpina/digiemu/blob/master/emu/samples.py)
handles sample files on the +Drive, not delivery to a DSP. The
[`emu/dsplink.py`](https://github.com/irpina/digiemu/blob/master/emu/dsplink.py)
code is for Digitone mk1's second ColdFire / DSPI2 link and is not evidence for
Digitakt mk1 or Digitakt II sample delivery. This comparison provides no
transfer mechanism to reuse for the current Digitakt II handoff.

## Next step

No second exact sample signature or active trigger-time eDMA transfer to the
sample was observed. The only sample-address TCD is channel 59's completed
eMMC-load descriptor, retained in the snapshot. Further firmware-side tracing
has reached the CPU-visible limit: decide whether to build a minimal DSP
stand-in around the unresolved DSP-visible/shared-memory path. Do not model an
unverified peripheral or count synthetic audio as playback progress.
