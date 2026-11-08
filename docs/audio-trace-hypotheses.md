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
| A separate bulk eDMA transfer carries sample PCM during sample load or trigger/play | No bulk transfer observed in these runs | `EDMA_TRACE` saw channels 14, 15, 28, 29, 35, and 59 during sample load; only 28, 29, and synthetic panel RX channel 35 during trigger/play. Channel 59 performed two 32 KiB eMMC reads into `0x4060d770` and `0x40605770`. Channel 50 SSI TX is scheduled by the emulator's timer diagnostic rather than an observed firmware SERQ request. This only rules out other eDMA request starts in these exact diagnostic runs. |
| The post-load DSPI1 eDMA 14/15 transfer is sample payload | Ruled out as bulk PCM; role unresolved | It runs once after the eMMC reads. Channel 15 sends 8 framed bytes from `0x4fe7a340` to DSPI1 PUSHR (`8001 0001 0801 012c`); channel 14 is configured for two 16-bit reads. Disassembly traces the request through `0x4003314e` to helper `0x40012562`, which constructs the 4-byte request `0001 012c`: word `0001` is set by the helper, and `012c` is copied from a caller argument. Related OS 1.16 research labels this helper `small` and describes the packet as two big-endian `u16` fields (`kind`, `slot`), suggesting kind 1 / slot 300; the same source warns that the waveform cache had not yet been shown calling this transport. The entry trace shows caller return `0x4019fc7e`, argument `0x012c`, and pointer `0x4525722c`; that caller is in a routine iterating over `0x64400` bytes in `0x191`-byte strides, but its context is not yet identified. The helper passes request length 4 and response length `0x300` to `0x400124a0`, using response buffer `0x4fe79340`; the observed channel-14 transfer reads only two halfwords. Digiemu's older-version notes likewise classify DSPI1 channels 14/15 as short opcode request/reply transactions, probably to a codec. These are useful cross-checks, not proof this exchange is an audio handoff. The watched `0x43176a34` and `0x43176a38` writes are the saved A6 from `LINK` and return address from `JSR`, not additional request fields. |
| The OS 1.17 accesses at `0x8c000000..0x8c00000f` carry sample-page descriptors | Ruled out for the observed accesses | With `RGPIOBAR=0x8c000035`, this 16-byte window is decoded as RGPIO in the current OS 1.17 trace. The sample-load capture records 135,300 RGPIO events, including repeated reads at `+0x2` and writes at `+0x2`/`+0xa`, consistent with GPIO polling and strobes rather than descriptor memory. The runner enables `RGPIO_READY_MODEL`, so the modeled pin-0 ready response is synthetic; the access addresses and operations are firmware-observed. Digiemu's OS 1.15C/1.16 findings label `0x8c000000` as a sample-page/slot-header region; that older-version clue does not establish the OS 1.17 decode or handoff. This conclusion is limited to the RGPIO window, not the surrounding address space. |
| The DSPI1 response buffer at `0x4fe79340..0x4fe79640` is populated or consumed during sample load | No CPU-bus access observed; hardware response unresolved | A repeat sample-load run starts with all `0x300` bytes zero and records zero overlapping CPU-bus reads and writes. The channel-14 TCD is configured for two 16-bit reads from DSPI1 POPR into the buffer, but the emulator's synthetic DSPI diagnostic does not execute eDMA channels 14/15. Therefore the empty buffer is evidence about this emulator run, not evidence that real hardware returns no response. |
| DSP reads the sample directly from shared DDR/FlexBus, or receives a descriptor through another path | Unresolved | The sample bytes are present in DDR, while the DSPI2 stream has no sample address. No trace yet identifies a firmware descriptor or a hardware-side shared-memory read. Do not infer the physical bus or peer from the emulated MMIO model. |
| `SYNTH_VOICE` output demonstrates firmware playback | Rejected as progress evidence | It injects sample bytes directly into the SSI TX source and creates nonzero output, but does not exercise the firmware's handoff. Keep it diagnostic-only; the trigger/play SSI capture remains zero. |

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
each DSPI2 channel (28 and 29); no DSPI1 or other SERQ channel is observed.
SSI TX captures 999 blocks / 2,045,952 bytes, all zero. The emulator's synthetic
SSI channel-50 servicing is outside the firmware SERQ trace.

## Next trace

Trace the sample-page descriptor/shared-memory path beyond the RGPIO register
window, then determine how a real DSPI1 channel-14 response would populate or
be consumed from `0x4fe79340`. Keep both direct shared-memory and descriptor
paths open; do not model an unverified peripheral or count synthetic audio as
playback progress.
