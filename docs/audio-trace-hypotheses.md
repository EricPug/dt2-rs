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
| TRIG/PLAY input is dead before the audio handoff | Ruled out for trigger delivery; audio handoff unresolved | The matched 1,000,000-instruction window immediately after TRIG had 20,315 CPU writes, 2 UART RX DMA writes, and two DSPI2 blocks; idle had 110 CPU writes and no DSPI2 blocks. In that narrow window, the two DSPI2 payloads differed from the prior idle block at counter halfword indices 270 and 462. This does not characterize the longer trigger state: the 999-block comparison later found note-dependent fields at TX data offsets `0x02`/`0x06` (see below), superseding any broader “only counters change” conclusion. Neither immediate payload contained the searched sample address/offset values, and `DSP_LINK` still supplies fixed `0xffff` replies. This confirms firmware activity in the diagnostic path, not physical DSP behavior or audible playback. Logs: `out/trigger-window-experiment/idle.log` and `out/trigger-window-experiment/trig-play.log`. |
| A separate bulk eDMA transfer carries sample PCM during sample load or trigger/play | No bulk transfer observed in these runs | `EDMA_TRACE` saw channels 14, 15, 28, 29, 35, and 59 during sample load; only 28, 29, and synthetic panel RX channel 35 during trigger/play. Channel 59 performed two 32 KiB eMMC reads into `0x4060d770` and `0x40605770`. Channel 50 SSI TX is scheduled by the emulator's timer diagnostic rather than an observed firmware SERQ request. This only rules out other eDMA request starts in these exact diagnostic runs. |
| The post-load DSPI1 eDMA 14/15 transfer is sample payload | Ruled out as bulk PCM; role unresolved | It runs once after the eMMC reads. Channel 15 sends 8 framed bytes from `0x4fe7a340` to DSPI1 PUSHR (`8001 0001 0801 012c`); channel 14 is configured for two 16-bit reads. Disassembly traces the request through `0x4003314e` to helper `0x40012562`, which constructs the 4-byte request `0001 012c`: word `0001` is set by the helper, and `012c` is copied from a caller argument. Related OS 1.16 research labels this helper `small` and describes the packet as two big-endian `u16` fields (`kind`, `slot`), suggesting kind 1 / slot 300; the same source warns that the waveform cache had not yet been shown calling this transport. The entry trace shows caller return `0x4019fc7e`, argument `0x012c`, and pointer `0x4525722c`; that caller is in a routine iterating over `0x64400` bytes in `0x191`-byte strides, but its context is not yet identified. The helper passes request length 4 and response length `0x300` to `0x400124a0`, using response buffer `0x4fe79340`; the observed channel-14 transfer reads only two halfwords. Digiemu's older-version notes likewise classify DSPI1 channels 14/15 as short opcode request/reply transactions, probably to a codec. These are useful cross-checks, not proof this exchange is an audio handoff. The watched `0x43176a34` and `0x43176a38` writes are the saved A6 from `LINK` and return address from `JSR`, not additional request fields. |
| CPU or modeled eDMA reads/writes the loaded sample during trigger/play | No accesses observed in that trigger/play run; sample-load CPU-read question remains inconclusive | Replaying trigger/play from the loaded snapshot with an exact watch on `0x40605770..0x40615770` produced zero CPU-bus or modeled-eDMA accesses. That watch did not cover sample load, so it says nothing about CPU reads during loading. The separate staging-buffer watch below is not a direct watch of the `0x406...` source span. |
| OS 1.17 GPIO serial traffic carries sample bytes | Sample page stream confirmed; downstream DSP receipt/consumption inconclusive | Calls 2–10 contain exact 4 KiB 16-bit-byte-swapped sample pages from `0x4060d770..+0x8000` and `0x40605770`; call 11 carries a matching `0xc00`-byte tail plus `0x400` bytes of staging/padding. Pages 0–9 reconstruct the converted `0x9c00`-byte sample exactly. Calls 4, 5, and 11 also match staging-buffer pages. Calls 1 and 12 are sentinel header/control packets. This confirms serialized bytes at the RGPIO data port, not physical DSP receipt. |
| Second RGPIO upload selects an independent sample slot and enables two-track playback | Inconclusive; no two-track playback demonstrated | The UI action was changed to `LOAD TO TRK 02` for `303 clap real`. Its upload contained indexed pages 0–3 and a final header with slot `300`, length `0x3022`; the first sample also used slot `300` with length `0x9c00`. The header's slot field therefore did not distinguish these samples, and the emulator's modeled DSP still has one sample buffer. A separate modeled run played the clap upload as a single sample (6,158 PCM frames), but no evidence yet associates two distinct uploads with two DSPI2 tracks. Logs: `out/modeled-dsp/clap-track2-loaded.log`, `out/modeled-dsp/clap-track1-retry.log`. |
| Note 60 and note 45 select different playback rates on one sample-machine track | Inconclusive; pitch change not observed on the sample-machine track | The Track 02 capture had note `0x2d00` at data offset `0x06` with machine type 2 at offset `0x98`. A keyboard/TRIG 16 attempt changed offset `0x02` from `0x3c00` to `0x4800`, while offset `0x06` remained `0x2d00` and machine type at `0x94` remained zero. This changed another track, not the sample-machine track. The proposed equal-tempered rule `rate(n) = rate(60) * 2^((n - 60) / 12)` remains an assumption; no DSP rate response was measured. Logs: `out/modeled-dsp/clap-track2-note60-attempt.log`, `out/modeled-dsp/clap-track2-note60-select.log`. |
| The DSPI1 response buffer at `0x4fe79340..0x4fe79640` is populated or consumed during sample load | inconclusive | A repeat sample-load run starts with all `0x300` bytes zero and records zero overlapping CPU-bus reads and writes. The channel-14 TCD is configured for two 16-bit reads from DSPI1 POPR into the buffer, but the emulator's synthetic DSPI diagnostic does not execute eDMA channels 14/15. Therefore the empty buffer is evidence about this emulator run, not evidence that real hardware returns no response. |
| Physical DSP receives and consumes the confirmed RGPIO pages | Unresolved | Firmware serializes the sample pages to RGPIO, but the emulator has no physical DSP peer. The per-block DSPI2 stream has no sample address, and the OS 1.17 SHARC loader contains no literal `0x40605770`/adjacent address or recognizable voice/sample descriptor table. Computed or relocated addresses and DSP-side consumption remain possible; the modeled WAV is not physical-device evidence. |
| `SYNTH_VOICE` output demonstrates firmware playback | Rejected as progress evidence | It injects sample bytes directly into the SSI TX source and creates nonzero output, but does not exercise the firmware's handoff. Keep it diagnostic-only; the trigger/play SSI capture remains zero. |

## OS 1.17 FlexBus/RGPIO and helper trace

The Digiemu OS 1.15C/1.16 report is a useful lead, but its `0x8c000000`
sample-page/slot-header interpretation is not directly transferable to OS 1.17.
In this OS 1.17 run `RGPIOBAR=0x8c000035`; the first 16 bytes
(`0x8c000000..0x8c00000f`) decode as RGPIO. The rest of the `0x8c000000..0x8cffffff`
range is not dropped or a bus-error window in this emulator: with the MCF SRAM
overlay active, it aliases the 64 KiB SRAM through the `0x80000000..0x8fffffff`
window. No access at offset `>=0x10` occurred in either phase, so the trace
does not test writes to that alias.

An OS 1.17 scan for `0x8c0000xx` references, excluding the three boot/control
functions, leaves the GPIO bit-bang routines around `0x400ccf04` and
`0x400ccf90` as the plausible transfer path; the other relevant references
around `0x400cd298..0x400cd2ac` are initialization. The transfer dispatcher at
`0x400cd79c` selects/calls those routines. It is the best functional candidate
for the older `FUN_400cfd40`, but the address/name correspondence is not proven.
The `TRACE_REGS=0x400cd79c` hook with `WATCH_WRITE=0x43176a30:0x43176a40`
captured 12 calls with all D0–D7/A0–A7 registers and 12 stack words visible.
Ten calls use source pointers in `0x450f1c40..0x450f8c40` (4 KiB-spaced pages)
and `0x450f1c40`/`0x450f2c40`. D7 is `0x8000` in ten captures, with the same
value in a captured stack word; its precise length/argument role is not
established. Another frame has source pointer `0x450ecc40` and `0x9c00` on the
stack, matching the loaded sample byte count. The loaded sample itself is at
`0x40605770`. Neither `0x40605770` nor `0x10000` appears in any captured data
or address register or stack word. The `0x9c00` size match is suggestive, not
proof that this path carries those sample bytes. Full hook output is in
`out/audio-edma-trace/sample-load-helper-400cd79c.log`.

During sample load, the complete FlexBus trace has 135,300 accesses:

| PC sites | Count | Access |
|---|---:|---|
| `0x400ccf40`, `0x400ccfcc` | 2,060; 10,240 | 16-bit writes of `0x0080` to `0x8c00000a` (strobe) |
| `0x400ccf48`, `0x400ccfd4` | 4,120; 20,480 | 16-bit reads from `0x8c000002` (status/pin read) |
| `0x400ccf56`, `0x400ccf5c`, `0x400ccf62`, `0x400ccf68`, `0x400ccf6e`, `0x400ccf74`, `0x400ccf7a`, `0x400ccf84` | 2,060 each | 16-bit writes to `0x8c000002` (serialized values) |
| `0x400ccfe2`, `0x400ccfe8`, `0x400ccfee`, `0x400ccff4`, `0x400ccffa`, `0x400cd000`, `0x400cd006`, `0x400cd010` | 10,240 each | 16-bit writes to `0x8c000002` (serialized values) |

All 135,300 accesses are at offsets `<0x10`; there are zero at offsets
`>=0x10`. Trigger/play has zero accesses in the whole `0x8c000000..0x8cffffff`
range. `EDMA_TRACE` shows no source/destination in this range; all recorded
accesses are CPU bus cycles. The full ordered trace, including every PC, width,
address and value, is in `out/audio-edma-trace/sample-load.log` (consecutive
identical events are folded with an `xN` repeat count); the trigger/play
zero-count is in `out/audio-edma-trace/trig-play.log`. `RGPIO_READY_MODEL`
synthesizes the pin-ready response, so read values are emulator-model
responses; the firmware accesses and write values are traced.

### 1. Reassembled GPIO payloads

The `0x8c000002` write stream contains 12 groups of 8,200 16-bit writes. Each
byte is strobed twice with the same high byte; taking one high byte from each
pair reconstructs 4,100 bytes per call: a 4-byte prefix followed by 4,096
payload bytes. The sample comparison combines `0x8000` bytes from `0x4060d770` and
`0x1c00` bytes from `0x40605770`; the helper-buffer comparison covers
`0x450ecc40`. The four-byte prefix is little-endian: calls 2–11 decode to page indices
0–9; calls 1 and 12 use the `0xffffffff` sentinel. Exact and partial matches,
after stripping the prefix, are:

| Call | Prefix bytes → value | Body match |
|---:|---|---|
| 1 | `ff ff ff ff` → sentinel | Pre-card header/control packet; body begins `0x12c, 0xfffffc00, 0xfffffc00`; not a match for the later sample data. Remaining semantics are inconclusive. |
| 2 | `00 00 00 00` → 0 | Exact 16-bit-byte-swapped page 0 of first channel-59 source at `0x4060d770`. |
| 3 | `01 00 00 00` → 1 | Exact 16-bit-byte-swapped page 1 at `0x4060d770 + 0x1000`. |
| 4 | `02 00 00 00` → 2 | Exact sample page at `0x4060d770 + 0x2000`; also matches helper buffer `0x450ecc40 + 0x7000`. |
| 5 | `03 00 00 00` → 3 | Exact sample page at `0x4060d770 + 0x3000`; also matches helper buffer `0x450ecc40 + 0x8000`. |
| 6 | `04 00 00 00` → 4 | Exact 16-bit-byte-swapped page 4 at `0x4060d770 + 0x4000`. |
| 7 | `05 00 00 00` → 5 | Exact 16-bit-byte-swapped page 5 at `0x4060d770 + 0x5000`. |
| 8 | `06 00 00 00` → 6 | Exact 16-bit-byte-swapped page 6 at `0x4060d770 + 0x6000`. |
| 9 | `07 00 00 00` → 7 | Exact 16-bit-byte-swapped page 7 at `0x4060d770 + 0x7000`. |
| 10 | `08 00 00 00` → 8 | Exact 16-bit-byte-swapped page 8 from `0x40605770`. |
| 11 | `09 00 00 00` → 9 | First `0xc00` bytes match converted source `0x40605770 + 0x1000`; full body matches helper buffer `0x450ecc40 + 0x6000`. Its final `0x400` bytes are padding/other staging data, not confirmed sample audio. |
| 12 | `ff ff ff ff` → sentinel | Final header/control packet; sample length `0x9c00` is at body offset `0x10`. Other fields are not interpreted. |

Concatenating pages 0–9, truncating to `0x9c00`, and reversing each byte pair
exactly matches the converted source assembled from `0x4060d770..+0x8000` and
`0x40605770..+0x1c00`; its SHA-256 is
`5f84301e6021dd5530e71614a3c4b04cf61d88ee4bcf4bfba0fb12865fc7d2a4`. This
confirms serialized sample bytes at the RGPIO data port and their page order,
not downstream DSP receipt or consumption.

### 2. Staging-buffer accesses

The sample-load watch covered the half-open range
`0x450ecc40..0x450f6840`, including the later `0x450f1c40` staging pages.
It retained all 15,683 matching CPU accesses (no truncation): 8,982 `read32`
and 6,701 `write32`; there were no 8- or 16-bit accesses and no eDMA endpoint
in the range. Let C1 be channel-59 completion to `0x4060d770` at instruction
8,489,477,206 and C2 completion to `0x40605770` at 8,497,412,556. Each table
cell is the first-to-last instruction delta from that completion.

| Access site | Address range | Count | Delta from C1 | Delta from C2 |
|---|---|---:|---:|---:|
| `read32` at `0x400cd7d0` | `0x450ecc40..0x450f683c` | 8,960 | -870,937..+10,436,471 | -8,806,287..+2,501,121 |
| `read32` at `0x4013639e` | `0x450f1c40..0x450f1c7c` | 16 | +513,908..+513,926 | -7,421,442..-7,421,424 |
| `read32` at `0x4011ea7e` | `0x450ecc48..0x450f1c48` | 4 | -871,087..+9,888,124 | -8,806,437..+1,952,774 |
| `read32` at `0x4011e844` | `0x450edc3c..0x450f1c3c` | 2 | +10,713,261..+11,236,099 | +2,777,911..+3,300,749 |
| `write32` at `0x401363a4` | `0x450f1c40..0x450f683c` | 6,676 | +236..+8,200,170 | -7,935,114..+264,820 |
| `write32` at `0x4011e916` | `0x450ecc48..0x450f1c48` | 4 | -10,785..+11,236,128 | -7,946,135..+3,300,778 |
| `write32` at `0x4011e91e` | `0x450ecc44..0x450f1c44` | 4 | -10,782..+11,236,131 | -7,946,132..+3,300,781 |
| `write32` at `0x4011e922` | `0x450ecc40..0x450f1c40` | 4 | -10,781..+11,236,132 | -7,946,131..+3,300,782 |
| `write32` at `0x4011e924` | `0x450edc3c..0x450f1c3c` | 3 | -10,780..+10,713,295 | -7,946,130..+2,777,945 |
| `write32` at `0x40153e8c` | `0x450ecc40` | 2 | -871,018..+9,888,193 | -8,806,368..+1,952,843 |
| `write32` at `0x40153e90` | `0x450ecc50` | 2 | -871,017..+9,888,194 | -8,806,367..+1,952,844 |
| `write32` at `0x40153e96` | `0x450ecc44` | 2 | -871,016..+9,888,195 | -8,806,366..+1,952,845 |
| `write32` at `0x40153e9a` | `0x450ecc4c` | 2 | -871,015..+9,888,196 | -8,806,365..+1,952,846 |
| `write32` at `0x40153ea0` | `0x450ecc48` | 2 | -871,014..+9,888,197 | -8,806,364..+1,952,847 |

The bulk copy site `0x401363a4` first writes the watched staging range 236
instructions after C1 and continues through C2+264,820. In the corresponding
100M-instruction TRIG/PLAY run, the same staging-range watch recorded zero
reads and zero writes. That run does not establish whether the CPU reads the
`0x406...` sample span during loading; it was not watched there. Logs:
`out/audio-edma-trace/sample-load-watch-ecc.log` and
`out/audio-edma-trace/trig-play-watch-ecc.log`.

### 3. Dispatcher and high-window behavior

Disassembly of `0x400cd79c` shows a function-pointer dispatcher: it calls
`0x400ccf04`, tests D2, and branches at `0x400cd7ce`. Nonzero D2 selects the
supplied function pointer in A2; zero selects `0x400ccf90`. The selected
routine processes the `0x1000`-byte buffer from A3 in 4-byte steps. This
condition selects a routine, not a sample-page address range. The dispatcher
itself does not access `0x8c000010+`; the RGPIO trace contains only offsets
`0x2` and `0xa`. Whether an unexecuted helper branch can access the higher
window remains inconclusive.

The emulator routes `0x8c000000..0x8c00000f` to RGPIO. Addresses from
`0x8c000010` onward are not dropped as unmapped writes: with the MCF SRAM
overlay active they route through the 64 KiB SRAM alias in
`0x80000000..0x8fffffff`. No trace touched that alias at `0x8c`, so the
Digiemu sample-page path at higher offsets remains **untested**, not ruled out.

### 4. Trigger-dependent DSPI2 fields and helper-call timing

The longer 999-block comparison uses data-only TX offsets `0x02` and `0x06`
(raw PUSHR halfword positions 3 and 7). For trigger `2301`, the pairs are
`0000/0000` for blocks 0–63, `3c00/0000` for 64–633, and `3c00/2d00` for
634–998. For trigger `2302`, they are `0000/0000` for 0–645 and
`0000/2d00` for 646–998. These are note/track state fields, not the
free-running counters at data halfword indices 270 and 462. The earlier
“only two counters change” result applies only to its short immediate window
and is superseded as a broader description. The comparison is checked by
`tools/audio-edma-trace.sh`; logs are `out/audio-edma-trace/trig-play.log` and
`out/audio-edma-trace/trig-play-note2.log`.

The `action-down.snap` snapshot restores the cumulative instruction counter
at 8,400,000,000, so these call counts are cumulative since boot. Deltas are
relative to C1 and C2 above.

| Call | Instruction count | Delta from C1 | Delta from C2 |
|---:|---:|---:|---:|
| 1 | 8,488,606,197 | -871,009 | -8,806,359 |
| 2 | 8,489,991,179 | +513,973 | -7,421,377 |
| 3 | 8,490,804,426 | +1,327,220 | -6,608,130 |
| 4 | 8,491,664,866 | +2,187,660 | -5,747,690 |
| 5 | 8,492,489,291 | +3,012,085 | -4,923,265 |
| 6 | 8,493,312,634 | +3,835,428 | -4,099,922 |
| 7 | 8,494,397,994 | +4,920,788 | -3,014,562 |
| 8 | 8,495,497,246 | +6,020,040 | -1,915,310 |
| 9 | 8,496,584,405 | +7,107,199 | -828,151 |
| 10 | 8,497,677,495 | +8,200,289 | +264,939 |
| 11 | 8,498,496,112 | +9,018,906 | +1,083,556 |
| 12 | 8,499,365,408 | +9,888,202 | +1,952,852 |

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
RGPIO window is also used to serialize sample bytes: reassembly of the 12
sample-load calls found five exact 4 KiB matches to sample/helper-buffer data.
This confirms bytes sent through the GPIO data register, not the older
high-offset sample-page map or downstream DSP consumption.

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
has reached the CPU-visible limit. The modeled-DSP stand-in below uses only the
confirmed RGPIO page stream and firmware DSPI2 note fields; it is not evidence
of physical DSP behavior.

### Confirmed page reconstruction

The 12 dispatcher calls each produce a four-byte little-endian prefix followed
by 4,096 body bytes. Calls 1 and 12 use `0xffffffff`; calls 2–11 use prefixes
`0..9`, which appear to be page indices. The sentinel packet is treated as
metadata/control, not a numbered sample page.

| Call | Prefix | Interpretation and evidence |
|---:|---:|---|
| 1 | `ffffffff` | Pre-card header/control packet. Its body begins `0x12c`, `0xfffffc00`, `0xfffffc00`; it occurs before the first channel-59 read completes and does not match that later sample data. Its remaining semantics are inconclusive. |
| 2–9 | `00000000`–`00000007` | Pages 0–7; each body is the 16-bit-byte-swapped page at offsets `0x0000`–`0x7000` of the first channel-59 transfer (`0x4060d770..+0x8000`). |
| 10 | `00000008` | Page 8; exact 16-bit-byte-swapped match for the first 4 KiB of the second channel-59 transfer at `0x40605770`. |
| 11 | `00000009` | Page 9; the first `0xc00` bytes match the converted second-transfer bytes at offset `0x1000`. The complete 4 KiB body matches `0x450ecc40 + 0x6000`; its final `0x400` bytes are padding or other staging data, not confirmed sample audio. |
| 12 | `ffffffff` | Final header/control packet. Its body begins `0x12c, 0x40, 0x40, 0xb71b`; a 32-bit sample length `0x9c00` occurs at body offset `0x10`. Other fields are not interpreted by the model. |

Calls 4, 5, and 11 also match byte-swapped staging-buffer pages at offsets
`0x7000`, `0x8000`, and `0x6000`. Concatenating pages 0–9, truncating to
`0x9c00`, and reversing each byte pair exactly reconstructs the converted
sample: first `0x8000` bytes from `0x4060d770`, then `0x1c00` bytes from
`0x40605770`. The reconstructed sequence's SHA-256 is
`5f84301e6021dd5530e71614a3c4b04cf61d88ee4bcf4bfba0fb12865fc7d2a4`.
This confirms the upload bytes and page ordering, not physical DSP receipt.

### Modeled DSP stand-in and assumptions

`MODELED_DSP_WAV=<path>` is separate from `SYNTH_VOICE` and requires
`DSP_LINK`. At each `0x400cd79c` entry, it captures paired high-byte RGPIO
data writes at `0x8c000002`, validates the repeated-write byte strobe and 4 KiB
body size, stores numbered pages, and reads the final header's sample length.
It consumes data halfwords from DSPI2 channel-29 frames: per-track note fields
at data offsets `0x02 + 2*i` and machine fields at `0x94 + 2*i`. A
zero-to-nonzero note edge on observed machine type 2 starts the sample.

The following are explicit model assumptions, not hardware findings:

- The first six reconstructed bytes are a non-audio prefix; PCM begins at
  offset 6, consistent with the previously identified sample start at
  `0x4060d776`.
- The sample is signed 16-bit big-endian mono PCM. One source sample is emitted
  per 48 kHz output frame; note-to-pitch mapping, gain, envelopes, polyphony,
  and DSP mixing are not modeled.
- Each DSPI2 block advances 32 audio frames. The note edge starts playback at
  that block's first frame. The observed machine-type value `2` and note-edge
  interpretation are based on the saved OS 1.17 trace, not a decoded SHARC
  ABI.
- Sentinel-header fields other than sample length are ignored. The emulator's
  `DSP_LINK` replies remain synthetic `0xffff`; no SHARC or eDMA is modeled.

`tools/modeled-dsp-sample.sh` replays the saved load and TRIG sequence, writes a
48 kHz WAV, and compares the sample segment byte-for-byte (as decoded PCM)
against the concatenated channel-59 source ranges. A passing run demonstrates
the modeled upload/trigger path only; it does not demonstrate firmware-driven
or hardware audio.

The bounded run completed 2,999 DSPI2 blocks and captured all 12 RGPIO calls.
The first sample-machine edge was track 2, note `0x2d00`, at cumulative
instruction 8,582,436,295 / block 1,823. The WAV contains 95,968 mono frames;
the 19,965-frame played segment begins at frame 58,336 and exactly matches the
source ranges after the six-byte prefix is excluded. Its source-waveform
SHA-256 is `d9781f776139a7d97d42d89616f5b976803696fa2f03ede416688103166a99fb`.
The WAV and source dumps are reproducible under `out/modeled-dsp/`; these are
generated artifacts, not firmware or hardware output.
