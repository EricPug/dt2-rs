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
| Per-block DSPI2 eDMA 28/29 stream carries PCM or the sample address | Ruled out for the observed stream | `DSP_LINK` captured 5,496 bytes per block (1,374 data halfwords). Values change at key events, but no sample address or sample-data n-gram appears in trigger/play or sample-load captures. The stream is a parameter/state exchange; direct DSP reads from shared DDR are not excluded. |
| A separate bulk eDMA transfer carries sample PCM during sample load or trigger/play | No bulk transfer observed in these runs | `EDMA_TRACE` saw channels 14, 15, 28, 29, 35, and 59 during sample load; only 28, 29, and synthetic panel RX channel 35 during trigger/play. Channel 59 performed two 32 KiB eMMC reads into `0x4060d770` and `0x40605770`. Channel 50 SSI TX is scheduled by the emulator's timer diagnostic rather than an observed firmware SERQ request. This only rules out other eDMA request starts in these exact diagnostic runs. |
| The post-load DSPI1 eDMA 14/15 transfer is sample payload | Ruled out as bulk PCM; role unresolved | It runs once after the eMMC reads. Channel 15 sends 8 framed bytes from `0x4fe7a340` to DSPI1 PUSHR (`8001 0001 0801 012c`); channel 14 is configured for two 16-bit reads. Disassembly traces the request through `0x4003314e` to helper `0x40012562`, which constructs the 4-byte request `0001 012c`: word `0001` is set by the helper, and `012c` is copied from a caller argument. The entry trace shows caller return `0x4019fc7e`, argument `0x012c`, and pointer `0x4525722c`; that caller is in a routine iterating over `0x64400` bytes in `0x191`-byte strides, but its context is not yet identified. The helper passes request length 4 and response length `0x300` to `0x400124a0`, but the observed channel-14 transfer reads only two halfwords. The watched `0x43176a34` and `0x43176a38` writes are the saved A6 from `LINK` and return address from `JSR`, not additional request fields. Command semantics, response contents, peer, and any descriptor/control relationship to sample handoff remain unestablished. |
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

In the 100M-instruction trigger/play phase, the firmware makes 999 requests on
each DSPI2 channel (28 and 29); no DSPI1 or other SERQ channel is observed.
SSI TX captures 999 blocks / 2,045,952 bytes, all zero. The emulator's synthetic
SSI channel-50 servicing is outside the firmware SERQ trace.

## Next trace

Identify what request word `0x012c` denotes in the caller and whether the
nominal `0x300`-byte response is backed by a separate buffer or another
transfer path. Then trace whether sample-load creates a separate descriptor
for the DSP or a shared-DDR reader. Keep both paths open until that evidence is
captured. Do not add a DSP model or count synthetic audio as playback progress.
