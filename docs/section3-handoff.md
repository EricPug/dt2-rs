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
- INTC2 unmasked sources 13, 15, 29, and 30. Source 13 is PIT0 and is delivered by the model. `PIT2_PERIODIC` provides an opt-in approximate source-15 producer for diagnostics. Sources 29/30 are configured at levels 6/5 but still have no ADC producer model, so they are not delivered.
- In the earlier baseline section-3 capture, no UART8/9 accesses were observed. The `0xec094xxx` accesses are GPIO registers, not UART9. The first section-3 MMIO trace entries include synthetic MCF register writes; they are not evidence of display output.
- The count-`0xa1` table at `0x4031153c` is a one-shot initializer list, not the timer-wheel list: all 161 targets execute in order, and 134 share a short 64-bit counter-increment prologue. The active PIT2 timer-wheel list is separate: head `0x40964db8` points to one node at `0x47db3f98`, callback `0x400cc8b2`, mask `0x80`, next `0`.
- In the live OS1.17 vector table, vector 207 points to PIT2 ISR `0x40002a18`, which acknowledges PIT2 and posts semaphore `0x47db3f90`; the timer-wheel task is `0x40002a46`. An injected vector 207 woke that task. The opt-in `PIT2_PERIODIC` probe at approximate 60 Hz invoked the registered callback 14 times by 35M instructions; that callback only accumulates the DMA timer counter, and UART8 TX remained at zero. This confirms PIT2 delivery but does not produce the screen.
- The separate natural-PIT UART capture has 1,321 UART8 status reads and 1,321 transmitted bytes, with no reads from the shared URB/RX register at `0xEC07000C`. Startup writes UMR1=`0x13` (RXRDY mode) and UIMR=`0x02` (RXRDY/FIFO-full request mask), but the idle snapshot's shifted RX ring base, consume index, and callback globals at `0x40964d84`, `0x40964da4`, and `0x40964db4` are all zero. Vector 154 points to the generic dispatcher at `0x40001252`; the natural capture contains no TCD34 programming or `0xFC045450` access. UART8 RX is therefore enabled at the device but not shown wired to a live DMA ring/callback in these runs. The capture validates TX framing only; it does not prove that RX is needed for display startup.
- RX setup is gated in the caller at `0x400cca40`: if `0x4029eef0 & 0x40` is nonzero, it skips the initializer. With that bit clear, the UART8 initializer at `0x4000243e` takes baud divisor `0x2625a` and callback `0x4011e648`, sends query bytes `60 02 70 00`, then waits for five RX bytes. The diagnostic reply `70 0b 00 00 00` passes the observed checks. The baud calculation at `0x400024b0` required the observed `DIVS.L D0,D2` form; the regression checks `0x07de2900 / 0x0002625a = 0x34c`.
- With the gate open, `USR=0x0d` seeded for TX/RX-ready, and `UART8_RX=700b000000ffffffffffffffffffff`, the initializer consumed the five-byte reply, configured TCD34 with SADDR `0xEC07000C`, NBYTES `1`, DADDR/ring base `0x4FE3A000`, and patched vector slot `0x40000268` to ISR `0x40001F1A`; callback pointer is `0x4011E648` at `0x40964DB4`. This validates the setup branch only: `UART8_RX=hex` preloads polled bytes, serial timing/eDMA are not modeled, and no physical RX event, real device reply, or display output is established.
- Section 2 initializes the launch flags by ORing `0x40000` into global `0x80008404` at `0x80000b1a`. Its discovery helper at `0x80001276` clears a 16-byte scratch area, sends UART8 query `60 01`, and waits for button-state headers on channels 0 through 6 (`0x20` through `0x26`) plus each payload byte. The polled ring is at `0x80008424`, with available-byte count at `0x80008410`; a timeout or missing channel makes the caller at `0x80000b9a` OR `0x60` into the launch flags. `PANEL_STUB` now recognizes `60 01` and supplies the seven channel/payload pairs through that ring. The natural run reaches section-3 entry `0x400004e8` with argument `0x00040000` at `A7+4`; section 3 copies this to `0x4029eef0`. No flag or ring override is used.
- Resuming from that natural handoff, section 3 copies `0x00040000` to `0x4029eef0`, sends UART8 TX query `60 02 70 00`, receives the synthetic `70 0b 00 00 00` handshake, and configures TCD34. The stub DMA-delivers `20 01` (group-0 button press), `30 01` (encoder-0 +1), and `20 00` (button release) to `0x4FE3A000`; vector 154 invokes the byte callback six times. No additional UART8 TX records or screen-specific MMIO were observed. After the 380M-instruction diagnostic run, execution is in the idle loop at `0x400cc912` (instruction-limit stop). These panel records and eDMA timing are synthetic diagnostics, not hardware behavior.
- The UART callback at `0x4011e648` dispatches type 2 to the button-state handler and type 3 to the encoder handler. For type 7 it increments a byte index and stores each incoming byte at `0x44dc7150 + index`; on each callback it copies the eight-byte staging buffer unchanged to `0x44dc75a8` with `0x40136388`, then publishes that block's pointer in `0x44dc75a4`. The callback does not interpret the fields; downstream code retrieves the pointer through `0x4011e310` and copies/uses the block.
- A separate `UART8_RX=700b000000200130012000` run with `UART8_TX_READY` asserted reaches `0x4011e648` for all six bytes: `20 01`, `30 01`, and `20 00`. It emits only TX `60 02 70 00`; the event-state setter's counters at `0x44e4adb8` are unchanged. Type-2 handling at `0x4011e318` updates the button-state table at `0x44dc8634`, builds normalized 16-byte records in the `0x44dc7e14` ring (write offset at `0x44dc7e10`), and submits them to queue object `0x402b45a0` through `0x40001896`; it does not call `0x40133e58` in this probe. `UART8_TX_READY` only seeds TXRDY/TXEMP; it supplies no panel response or input records. This does not establish a hardware panel's behavior.
- In the raw-input full run, queue handle `0x402b45a0` points to kernel queue `0x47db3f70`, whose count reaches 2; its storage pointer at `0x40964920` points to `0x44dc7e24`. Receiver task `0x400337ba` would be created at `0x4003324e`, from gated app path `0x400cce4a`, but its TCB/queue (`0x40966ee8`/`0x40966f3c`) remain zero at the captured stop. Treat this as not-created, not blocked.
- The application worker's saved wait frame points to `0x401365d6`, which waits on object `0x44e6662c`. Setup at `0x40136544` installs vector 97 at `0x40000184`, sets INTC0 source 33 to priority 2/unmasked, writes reference `0x000f4240` to DMA Timer 1 `DTRR1` (`0xFC074004`), then writes `DTMR1=0x841b` (`0xFC074000`). The MCF54418RM identifies `0xFC074000` as DMA Timer 1; source 33 is vector 97. With opt-in `DTIM1_BOOTSTRAP DTIM1_TICKS_PER_INSTRUCTION=1`, the modeled reference compare reaches the guest ISR and releases the worker; at 380M it proceeds beyond the earlier `0x400cd3b2` DSPI2 RXCTR poll.
- Firmware-owned PIT0 ordering was checked without `PIT0_PERIODIC`. The boot task context builder at `0x40001148` configures TCB `0x4096369c` for entry `0x40000ea0`, priority 9, stack `0x409636f0`, and returns `D0=1`; its ready-list insertion at `0x40001176` occurs at instruction 32,360,048. PCSR0 is then written `0x053f` at `0x400012c0`, instruction 32,360,079 (PIE enabled). The UART/event worker context builder at `0x400cc8fc` configures TCB `0x42944aac` for entry `0x400cc9c8`, priority 1, and its ready-list insertion occurs at instruction 32,360,137. The first natural PIT0 source 13/vector `0xcd` arrives at instruction 34,457,230, after both contexts are linked. Thus the default one-tick-per-instruction model does not deliver PIT0 before these initial task setups, though PIE is enabled before the second task is linked.
- `WATCH_WRITE=0x40964914:0x40964918` shows the ready-list head array initialized/updated at PC `0x40001176`; scheduler removal/reordering writes also occur at `0x400013ec`, `0x4000145e`, and `0x400014ba`. `WATCH_WRITE=0x47db3f40:0x47db3f44` shows node `0x47db3f40` zeroed at instructions 32,240,821 and 32,359,972, then first populated at instruction 52,094,733 by PC `0x40001180` with TCB `0x4314d558`. That node is priority 7; its saved entry is `0x400d1a12`. It is linked after the first PIT0 interrupt, not one of the initial boot/worker contexts.
- A controlled `PIT_FIRST_TICK_DELAY=50000000` run delayed the first source-13 interrupt to instruction 84,457,230, but still stopped at the null-context `RTE` at `0x40000458` after 103,756,650 instructions. `STOP_PC=0x400cce50` confirmed the receiver-task call at `0x400cce4a` was never reached; no error return from that API was observed. The initial context-builder calls returned `D0=1`; the first later priority-7 insertion is a list-link operation, not a task-creation error result.
- Opt-in `DSPI2_BOOTSTRAP` at `0xEC038000` uses the existing DSPI register/FIFO model with no attached slave, returning `0xffff`. With that diagnostic fill, startup passes the RXCTR poll and reaches software floating-point conversion. The CPU now models the observed `MULS.W`, `BCHG.B`, and `NEGX.L` forms on this path. The 380M run stops at `RTE` (`0x4e73`) at `0x40000458`; the failure is a null scheduler context, not a malformed exception frame from an entering exception. The preceding timer interrupt is vector `0xcd`, with format/vector word `0x40cd`, saved SR `0x2004`, saved PC `0x4018125a`, and frame at `0x4314d264`. The scheduler reads a zero ready-list head at `0x47db3f40`, restores an all-zero context (A7 becomes 0), then `RTE` reads format/vector `0x0000` from the SBF low-address SRAM alias and rejects it. A confirmation run with `STOP_PC=0x400cce4a` still stops at this RTE, so the gated receiver-task creation path is not reached; its TCB remains zero and two normalized records remain queued. Full PC/SR/D0-D7/A0-A7 snapshots are in `out/invalid-rte-trace-confirm.log`. This is not a successful boot and does not establish the real DSPI2 reply or a display path.
- The new opt-in `TRACE_TAIL=n` records the last 1-1024 pre-instruction snapshots, including PC, SR, D0-D7, and A0-A7. For the instruction-flag additions, `NEGX.L` updates X/N/Z/V/C with cumulative Z, `MULS.W` updates N/Z, clears V/C, and preserves X, and `BCHG.B` changes only Z based on the old bit. The local MCF54418RM defines CCR bit meanings but does not give per-instruction CCR masks; its MULS/MULU N/Z note describes MACSR, not the CPU CCR.
- The input-event state setter at `0x40133e58` signals semaphore `0x44e4ae88` via `0x400016d2` after accepting a nonnegative slot from the map at `0x4020adb8`, updating the slot value/bitmap, and running the pending-event helper. It first acquires the same semaphore through `0x400015a0`. The 50-slot task scan at `0x40133dd6` waits on that semaphore, checks/decrements the per-slot counts in `0x44e4adb8`, clears expired bits, dispatches changed inputs, then waits again. The semaphore activity is therefore triggered by valid event-state updates, not by an unconditional UART receive; direct callers include event/control paths at `0x4005c0fc`, `0x4005c14e`, `0x400aacea`, and `0x400b9a1c`.
- Idle-snapshot injections reached their expected handlers: source 15/vector `0xCF` acknowledged PIT2 at `0xFC088000` and returned to `0x400CC912`; sources 29/30 vectors `0xDD`/`0xDE` acknowledged their INTC2 forced bits and returned to idle with null ADC hooks; vector `0x9A` reached the generic dispatcher and stalled polling UART TX-ready, not the RX ISR. None produced UART output.

## Scheduler and Timer 1 Follow-up

> Superseded in part: the statements below that `0x400cce4a` is never reached describe runs made before the decoder and UART8 DMA fixes listed in "Status After the Emulator Fixes" at the end of this file.

- `SCHED_TRACE` records task-context creation and every transition at `0x400013ec`, `0x4000145e`, and `0x400014ba`, including the current TCB, mapped entry, registers, and raw TCB/stacks. The normal and immediate-release logs are `out/scheduler-transitions-normal-timer1.log` and `out/scheduler-transitions-immediate-timer1.log`.
- Five task contexts are observed: boot task `0x4096369c` -> `0x40000ea0`; launcher `0x42940a58` -> `0x400cc8e4`; worker `0x42944aac` -> `0x400cc9c8`; timer-wheel task `0x40964dbc` -> `0x40002a46`; and priority-7 task `0x4314d558` -> `0x400d1a12`.
- The boot task calls queue receive through `0x40001928` and blocks in the scheduler wait path with queue condition `0x409648f8` (queue object `0x409648f0`). It does not reach `0x400cce4a`; that app path is not a continuation of entry `0x40000ea0` in these traces.
- The worker first waits on semaphore `0x44e6662c`. Timer 1 source 33/vector 97 does release into ISR `0x40136514`: without `PIT0_PERIODIC`, the normal-scale frame at `0x42944a98` is format/vector `0x4061`, saved SR `0x2004`, saved PC `0x400cc912`; at `DTIM1_TICKS_PER_INSTRUCTION=1000000`, the immediate frame is also vector `0x61`, but saves SR `0x2008` and PC `0x401365d0`. The release order therefore changes with the synthetic clock. Both runs still stop at the null-context RTE.
- After Timer 1 activity, the worker can wait on `0x44e4ae98`; the timer-wheel task pends on semaphore `0x44e4ae80` and repeatedly uses timed event wait `0x40001608` with object `0x47db4058`. It re-pends on `0x44e4ae80` at instruction 53,756,600; its final ready-list removal scan is at `0x400013ec`, instructions 53,756,619, 53,756,625, and 53,756,631. The priority-7 task's `0x40001608` call uses object `0x44e08654` with timeout `0x400`.
- The ready list does not cleanly empty after all tasks block. The priority-7 TCB's first word is overwritten at `0x40181264` during the numeric conversion loop, along with its list link. The scheduler later reinserts that stale TCB (`0x4314d558`) at instruction 53,756,065 with saved SP `0`; the timer-wheel task then blocks, and the next context restore reaches `RTE` with a null frame. The task-return trampoline at `0x40001344` is present in the stale task's old stack but was not observed executing. This is context corruption/reuse, not evidence that the final task exited normally.
- The first corrupting store in the original run was instruction 52,961,867 at `0x40181264`: `MOVE.L D3,(A1)+` wrote zero to `0x4314d558`, replacing its self pointer; the next stores at `+4` and `+8` overwrote the link and ready-node pointer. Registers before the first store were SR=`0x2000`, D0-D7=`0,0,2,0,2,1,0x4314d4dc,0`, A0-A7=`0x4030c958,0x4314d558,0x40183058,0x40182d76,0x4314d4c0,8,0x4314d4b8,0x4314d26c`. The pointer slot was initially set at `0x40180f94` to a stack-local temporary, then reset at `0x40181218`: `A3 = A7 + 0x154 + 4*D1`. With D1=`0x64` (100), A3 became `0x4314d550`; subsequent postincrement stores reached `0x4314d558`. The caller supplied 0x78 (120) source limbs and no output-capacity argument. The frame is 0x1a4 bytes; scratch starts at `A7+0x154`, leaving 0x50 bytes (20 words) before the frame boundary. The task stack is `[0x43149558,0x4314d558)`, so the first TCB hit is exactly one-past-stack. This is the observed downstream corruption, not the upstream cause.
- The upstream cause found in the emulator is missing `ADDX.L`/`SUBX.L` register-register decoding. In the original trace, firmware executes `SUBX.L D2,D0` at `0x40182f08` while subtracting software-float mantissas. It fell through the generic SUB-to-EA path, which computed the reverse operation into D2 and left D0 unchanged. For the captured operands D0=`0x01921fb6`, D2=`0x006487ed`, X=0, the required D0 result is `0x012d97c9`; the old path instead changed D2 to `0xfed26837`. The untouched D0 then propagated into the NaN-like helper result `0xff5bc094` and the bad conversion path. The source bytes at `0x40226df2` match all 480 host bytes and had zero runtime writes. The outer `CMP.L $44(A7),D1`/BGT at `0x401811d2` showed no branch mismatch; its D1=100 comparison against loop index 109 had CCR=`0x09` and correctly fell through.
- `ADDX.L` and `SUBX.L` register forms are decoded explicitly, including X input, carry/borrow, overflow, and cumulative Z behavior. The decoder accepts only the documented long register form; byte/word and memory-predecrement encodings remain illegal. `NEGX.L Dn` is tested with X set and clear. ROXL/ROXR and rotate look-alikes remain illegal because the MCF54418 instruction table lists arithmetic/logical shifts, not rotate-through-extend instructions. Focused core regressions cover X-set/X-clear results and flags, overflow, cumulative Z, and rejected forms.
- In a post-fix replay using the current `ESDHC_EMMC` and `GPIO_LOOPBACK` probes, the runner retired 380M instructions and stopped at `0x401363a4`; neither the converter (`CONVERT_TRACE` recorded zero events) nor `0x400cce4a` was reached. The final dump of `0x4314d558` remained `43 14 d5 58 43 14 d5 58 47 db 3f 40 47 db 3f 28`, so the TCB self-pointer and ready-list links were intact at that checkpoint. Because the converter did not run, this replay cannot confirm its post-fix D1 output index or prove the old store unreachable. The source consists of 120 2/pi table limbs, so a decimal digit count of those bytes is not a valid expected converter iteration count. The eSDHC/eMMC model executed 14 commands, but its card contents and responses are synthetic, not hardware evidence. `WATCH_WRITE` evidence for the original corruption remains in `out/priority7-first-corruption-with-registers.log`; the local MCF54418RM documents the ADDX/SUBX forms and CCR/X meanings.
- In a post-fix replay using the current `ESDHC_EMMC` and `GPIO_LOOPBACK` probes, the runner retired 380M instructions and stopped at `0x401363a4`; neither the converter (`CONVERT_TRACE` recorded zero events) nor `0x400cce4a` was reached. A second 150M-instruction replay with `DSPI2_BOOTSTRAP` and `CONVERT_TRACE` also recorded zero converter events and ended at `0x401830ea`; it did not confirm the post-fix D1 output index or prove the old store unreachable. The older 52M D1 trace is pre-fix evidence only. The source consists of 120 2/pi table limbs, so a decimal digit count of those bytes is not a valid expected converter iteration count. The eSDHC/eMMC model executed 14 commands, but its card contents and responses are synthetic, not hardware evidence. `WATCH_WRITE` evidence for the original corruption remains in `out/priority7-first-corruption-with-registers.log`; the local MCF54418RM documents the ADDX/SUBX forms and CCR/X meanings.
- Register-rich `WATCH_WRITE=0x4314d558:0x4314d564` evidence is in `out/priority7-first-corruption-with-registers.log`. The exact first write was width 32, PC `0x40181264`, old `0x4314d558`, new `0x00000000`; D3=`0`, A1=`0x4314d558`, A7=`0x4314d26c`, SR=`0x2000`. A7+`0x38` is `0x4314d2a4`; the loop loads its destination from that pointer slot, increments the slot by four, then stores through the retained old A1. Stack bounds are `[0x43149558,0x4314d558)`, confirming the destination is exactly the exclusive upper boundary, not an address inside the running task's stack.
- Making Timer 1 effectively immediate changes the worker's first resumed frame and later wait path, but does not keep the ready list valid or reach `0x400cce4a`. Both timer scales stop at `0x40000458` after 53,756,650 instructions. Timer 1 timing and DSPI2's no-slave `0xffff` response remain synthetic diagnostics, not hardware timing or device evidence.

This completion mode intentionally executes queued PUSHRs immediately even when MCR.HALT/MDIS is set; it is a diagnostic instant-clock override, not hardware-faithful MCR gating. The PUSHR/POPR log is the primary evidence for the transfer sequence. These responses and bus events identify the polled device path but do not yet establish display payload. Synthetic MMIO responses are diagnostic approximations.

- `REMS.L`/`REMU.L` (`DIV.L` extension word with Dr != Dq) write only the remainder register and leave the dividend unchanged. This is inferred, not confirmed against the MCF54418 manual; the Dr == Dq form (quotient only) is the one the firmware's compiler pattern supports.

## Status After the Emulator Fixes

Decoder fixes (each with unit tests): `ADDX.L`/`SUBX.L` register forms, `ASL.L`, ADD/SUB/AND/OR opmode-6 forms that are illegal on ColdFire, byte `-(A7)`/`(A7)+` stepping by 1 (the firmware's `clr.b -(a7); subq.l #3,a7` only keeps A7 aligned that way), general `DIVS/DIVU.L` and `REMS/REMU.L`, and `CPUSHL` as a privileged no-op. A Capstone differential over 3,005 distinct executed opcode words found no length mismatches and checked every word against the correctly based static image. The 142 Capstone-invalid words are ColdFire-only `FF1` and `MVS`/`MVZ` forms; the only executed word rejected by the emulator was `RTE` at `0x40000458`, the known null-context stop. This checks acceptance and length only, not flags.

What changed so that the receiver-task creation call site is reached (instruction 577,643,284 in `out/run-idle3b.log`; the matching pre-instruction snapshot is `out/idle3b.snap`):

- Before, the priority-7 task spun in the UART8 TX writer at `0x4000220c`, waiting for room in the 4 KiB TX ring computed from eDMA TCD35 CITER/BITER (`0xfc045474`/`0xfc04547c`). Writes to the TCD window were discarded, so the ring filled to `0xffe` bytes and never drained, starving the lower-priority boot task.
- The diagnostic eDMA model now keeps a TCD register file and completes a memory-to-peripheral channel when SERQ (`0xFC044018`) starts it: it moves the bytes, reloads CITER from BITER, sets DONE, and for channel 35 raises INTC1 source 27 / vector 155 (ISR `0x40001e7c`). This produced the first UART8 tile records of the main OS (`b7 ff b8`, 128 ten-byte records, `b8` fill).
- That run next stopped at unsupported `CPUSHL` (`0xF468`) at `0x4000678c`, 23K instructions before `0x400cce4a`; implementing it as a no-op let startup reach `0x400cce4a` at instruction 577,643,284. `0x400cce4a` is `JSR $4003324e`; that reset run stopped before executing it. A follow-up from `out/idle3b.snap` executed the call and returned to `0x400cce50` after 70 instructions; `SCHED_TRACE` recorded a new context, TCB `0x40966ee8`, entry `0x400337ba`, priority 6. This confirms the context was created, but not that it was scheduled or that a project loaded. Both earlier changes were needed to reach the site; the DIV/REM and A7 fixes were needed to get that far.

Machine snapshots (`SNAPSHOT_SAVE=`/`SNAPSHOT_LOAD=`) hold the CPU, RAM, PIT0-3, INTC0/2, DTIM1, DSPI0-2 including flash state, RGPIO, UART8, DDR controller, synthetic MMIO, eDMA TCDs and panel-stub flags. Restoring needs the same device flags. The RGPIO ready-model poll counter is not saved. A 40M + restore + 10M run matches a direct 50M run byte for byte.

RGPIO bit-banged bus (`0x400ccf04`, byte-order variant `0x400ccf90`):

- Callers are `0x400cd79c(header, buffer, mode)`, which takes semaphore `0x42948b14`, sends the header, then 1024 longwords from a 4096-byte buffer, delays 100 ticks and releases the semaphore.
- Per word: set pin 7, poll pin 0 until high, then write three bytes on GPIO[15:8] and one on the low lane, each with a pin-7 strobe pulse. The pin-0 poll has no timeout, retry or error return, so with nothing driving pin 0 the priority-3 task spins and starves every lower-priority task.
- With the synthetic ready model (`RGPIO_READY_MODEL`: pin 0 low for one poll after pin 7 rises, then high) 1024 uploads occur between instructions 826,059,048 and 891,399,789, about 63,838 instructions apart, then none up to 2.3B. Header is always `0xffffffff`, mode `1`, buffer `0x450cfc40`. Consecutive buffers differ by exactly one byte (a 16-bit counter in bytes 2-3 plus two constant `ff ff fc 00` words at bytes 4-11, the rest zero). That does not look like 4bpp or 8bpp pixel data; renders under `out/upload-*.png` are blank. The peer is unidentified. A display is unconfirmed.
- Forcing pin 0 high (`RGPIO_INPUT_HIGH`) and running 1.5B instructions does not wake the boot task: `0x40000eba` (message dispatch) is never reached. The RGPIO handshake is not the gate to the boot queue or to `0x400cce4a`.

Boot work queue `0x409648f0`: a 128-slot deferred-call queue drained by the boot task (`0x40000ea0`), which calls the posted function pointer with 0-5 arguments. Static post-wrapper callers are `0x400d67c4`, `0x400d86aa`, `0x40139f96` and `0x400042ac`; tracing what triggers them is still open. At the idle snapshot the queue was empty with the boot task as its only waiter.

Open diagnostics, all synthetic or unconfirmed: the RGPIO ready line, `REM.L` writing only the remainder register (noted above), and the INTC2 source 29/30 ISRs, which hang because their ADC peripheral is not modeled.

## UART8 TX Stream After the eDMA TX Fix

`UART8_TX_FILE=path` writes every TX byte to `path` and the instruction count of each byte to `path.times` (the log's hex line is capped at 4,096 bytes). A run from reset to 900M instructions with `RGPIO_READY_MODEL` captured 46,451 bytes; all of them are sent by instruction 826,047,202, and nothing more follows through 2.3B.

- Command set seen: `60 02`, `70 00`, `73 00`, `60 00` (2-byte commands), `b7 ff` (twice), `b5 idx r g b` palette entries (136, indices 0-135, components up to `0x3f`), `b8` (update terminator or keep-alive), and 10-byte tile records (tag `0x10`-`0x17`, y, 8 payload bytes). Section 2 used `ba 00`, `70 00`, `60 01`, `60 02` and a 4-entry palette; `ba 00` does not appear in section 3, and `b7`, `73 00`, `60 00` and the 136-entry palette are new. Record counts by tag `0x10`..`0x17`: 432/396/699/670/748/566/559/487 (4,557 in total).
- Replaying the records into a 64x128 framebuffer gives 115 record-bearing updates (plus 72 bare `b8`). The first is a full 128-record frame at instruction 202.5M, followed by delta updates every about 4.4M instructions (the PIT3 period) that show the OS startup logo animation, then three full 128-record redraws at 818-823M, the palette load at 823.17M, and final updates to 826.0M. 113 of 115 update record-sets are distinct; three consecutive framebuffer states are pixel-identical. Frame 0 matches the existing `render-uart-records` renderer pixel for pixel (checked with a synthetic 1321-byte stream).
- The earlier 900M capture (`out/uart-tx-final-upright-x4.png`) showed a popup reading `MMC NOT IN SLC MODE`; that run did not attach the later synthetic eSDHC/eMMC model. With `ESDHC_EMMC`, the deterministic 1B baseline frame (`out/repeat-a-1b-final-frame.png`) instead reads `FACTORY PROJECT + DRIVE...`, with the warning absent. After a further 200M-instruction snapshot continuation, `out/continuation-1p2b-frame.png` shows the A01 sample interface (`SMP 1`, `TUNE`, `PLAY`, `LEV`/`STRT`/`LEN`/`LOOP`/`LEV`). This is synthetic-card behavior, not evidence from hardware.
- Panel input after the receiver task exists: `UART8_RX_PERIODIC=period:hex[,hex...]` cycles messages into the RX ring. From `out/tx900m.snap`, with no traffic no TX is produced in 300M instructions; with `2001,2000,3001,30ff` every 2M instructions the firmware sends only `bc 01` (130 times in 300M). `3001`/`30ff` (encoder) and `2001`/`2101`/`2201`/`2601` (button press) each produce `bc 01`; `2000` (release) produces none. No tile records follow, so the screen did not change. `bc 01` is a new command; its meaning is unknown.

## Repeatable 1B eSDHC Capture and Continuation

- The `0x400cce4a` hit at instruction 577,643,284 is a pre-instruction breakpoint at `JSR $4003324e`: it proves the receiver-task creation call site was reached, not that the call executed. The identical 1B runs below end later at `0x400e5d0c`; the 1.2B continuation ends at `0x400cce3c`.
- The clean persistent-card run uses the sparse 4 GiB raw image `out/dt2-emmc-corrected-20261101.img` through `ESDHC_FILE=path`. First and second reset boots both retire 1B instructions and stop at `0x400e5cfe`. Their 44,329 UART bytes are identical (SHA-256 `c035461de5181d9c778e02dd2797d8468ef09abe4c0c6cfe202a3fe3f2801a06`); the card image occupies 8 KiB on disk.
- `ESDHC_TRACE` command counts differ by two between first and second boot. Opcode, counts, and instruction ranges:

| Opcode | First boot | Second boot | Arguments / address range |
| --- | --- | --- | --- |
| `0x00`, `0x01`, `0x02`, `0x03` | 1 each, 52,092,903-52,226,270 | 1 each, same range | initialization |
| `0x06` | 3, 52,226,615-52,227,161 | 3, same range | EXT_CSD switches |
| `0x07`, `0x08`, `0x09`, `0x0a`, `0x0e`, `0x10`, `0x13` | 1 each, 52,226,515-52,227,280 | 1 each, same range | status, CID/CSD, EXT_CSD, bus test |
| `0x12` (CMD18) | 2, 587,611,825-587,613,130 | 2, 587,611,825-587,612,484 | reads blocks `0x0` and `0x800` |
| `0x17` (CMD23) | 3, 587,612,479-823,497,457 | 2, 823,496,626-823,497,457 | block-count argument `0x80000001` |
| `0x19` (CMD25) | 3, 587,612,612-823,497,757 | 2, 823,496,759-823,497,757 | writes blocks `0x0`, `0x0`, `0x800`; each count is 1 x 512 bytes |
| `0x23` (CMD35) | 2,120, 822,316,892-823,496,001 | 2,120, same range | argument min/max `0x12000`-`0x457800` |
| `0x24` (CMD36) | 2,120, 822,316,976-823,496,085 | 2,120, same range | argument min/max `0x127ff`-`0x457fff` |
| `0x26` (CMD38) | 2,120, 822,317,055-823,496,164 | 2,120, same range | erase argument `1` |

- The first boot writes block `0x0` at instruction 587,612,612 after CMD23 count 1. On second boot the firmware reads block `0x0`, then reads `0x800` without that CMD23/CMD25 block-0 write; this is the one-time factory bootstrap write that is skipped. The later block-0 and block-`0x800` writes still occur, and all 2,120 CMD35/36/38 erase triplets still run, so the entire storage/erase initialization is not skipped. The screen/UART stream at 1B is unchanged between boots.
- CMD25 now consumes only the number of blocks requested by CMD23 and clears write-active on completion. The captured eDMA preloads the 512-byte block before CMD25; the model stages that FIFO payload until the command arrives. The first boot completes three 512-byte writes, and the second completes two; there is no four-block overrun. These are synthetic writes to the persistent host image, not physical-card evidence.
- Continuing the second-boot snapshot for 200M instructions writes block `0x800`, reaches the A01 sample interface, and saves the full device state as `out/dt2-emmc-a01-interface.snap`. The sequential tile replay reports 10 continuation updates containing 290 tile records and 2,331 changed pixels from the 1B baseline; renders are `out/dt2-emmc-a01-replay-input-000-final.png` and `out/dt2-emmc-a01-replay-input-001-final.png`.
- A further 100M-instruction live window from the A01 snapshot emits zero UART bytes and no eSDHC commands. The final CPU PC is `0x400cce3c`; the live snapshot is `out/dt2-emmc-live-100m.snap`, and its replayed final frame is pixel-identical to the A01 checkpoint frame (`out/dt2-emmc-live-replay-final.png`).

## Live Panel Display

`LIVE_WINDOW[=scale]` (build with `--features window`, uses `minifb`) runs the CPU in 400,000-instruction slices and shows the UART8 tile stream decoded by `crates/dt2-cli/src/display.rs`, rotated 90 degrees counter-clockwise (upright) and scaled (default 4); closing the window stops the run. `LIVE_PNG=path` writes the final upright frame at 4x as a grayscale PNG with no windowing dependency. Palette (`b5`) and command bytes are consumed by length but not rendered, so the display is 1-bit. The stream grammar is inferred from captures. The device state behind it (RGPIO ready model, GPIO loopback, eMMC content) is synthetic. Keep rendered images and firmware dumps out of Git (`/out/` is ignored).

## Performance Baseline (1B Boot)

`tools/check-1b-baseline.sh <binary> <label>` runs the 1B-instruction section-3 boot with `RGPIO_READY_MODEL GPIO_LOOPBACK ESDHC_EMMC` and prints the saved snapshot's SHA-256. The direct-RAM fast path (`6e2281d`) and the pre-fast-path build (`e9fe6db`) both end at PC `0x400e5cfe` with snapshot SHA-256 `a81cd2785e62c6bbb622cea0a7406aba3787d47a7be938dcc8dec53a76c017d0`, at 8.4M and 3.0M instructions/s respectively. Performance changes must reproduce this hash; an intentional timing change needs its own labeled baseline. The run prints `fast path: on` or `fast path: off (reason)`.

### Lazy timers (scheduler step 1)

PIT and DMA timer 1 now batch retired instructions and apply their ticks only when a PIF/reference event is due or a register is accessed; the snapshot/print paths call `sync()` first. `PIT_EAGER` restores the per-instruction path for comparison, and `IRQ_LOG=path` logs every interrupt entry (instruction count, level, vector, PIT PCSR/counter) and every RTE (instruction count, PC, restored PC/SR). Random-traffic unit tests keep both paths identical, and the 1B baseline reproduces the same snapshot hash `a81cd278…17d0` and the same interrupt/RTE log (46,376 lines, sha256 `6dbffe8b…1c`) in both modes. Measured on the 1B benchmark: pre-fast-path 3.0M instructions/s (332 s), direct-RAM fast path 8.4M (119 s), this step 14.5M (69 s); eager timers on the same build run 13.1M (76 s), so the lazy timers alone give about 10% and the rest comes from the cheaper interrupt poll (a PIT source mask, and INTC2 returns early when nothing is pending).

### Opt-in instruction hooks and cached PIT pending mask

`report_instruction_state` now only copies registers and runs its trace checks when a diagnostic needs them (`DUMP_CALL`, `TRACE_TAIL`, `SCHED_TRACE`, `CONVERT_TRACE`, `TRACE_A0_RANGE`, `TRACE_A1`, `TRACE_REGS`; register copies also for `WATCH_WRITE`, `RGPIO_TRACE`, `CONVERT_TRACE`). The run prints `instruction hooks: minimal` or `full`. The PIT keeps its pending-source mask cached instead of recomputing it every instruction. The 1B baseline again reproduces snapshot `a81cd278…17d0` and the same interrupt/RTE log; throughput is 18.1M instructions/s (55 s).

### Panel display in snapshots

The UART8 tile decoder now lives in `dt2-core` (`display.rs`) and its framebuffer, including a half-received record, is saved in snapshots written with a display attached (`LIVE_WINDOW` or `LIVE_PNG`); snapshots without one are byte-identical to before, so the 1B baseline hash `a81cd278…17d0` is unchanged. To skip the boot in the viewer: `tools/run-section3.sh target/release/dt2-cli 1200000000 RGPIO_READY_MODEL GPIO_LOOPBACK ESDHC_EMMC LIVE_PNG=out/viewer.png SNAPSHOT_SAVE=out/viewer-1p2b.snap` once (about 70 s), then `... ESDHC_EMMC LIVE_WINDOW SNAPSHOT_LOAD=out/viewer-1p2b.snap` shows the saved screen immediately. Restoring that snapshot reproduces the saved frame pixel for pixel. Snapshots are git-ignored (`/out/`).

### Panel input

`LIVE_WINDOW` forwards the keyboard to the firmware as UART8 RX packets between slices (about 25 ms of emulation). Packet format and control names are **unverified leads** taken as facts only (no code) from public notes on the Digitakt II: ElekLabs/digikit `docs/findings/58-panel-input-path.md` and `devices/digitakt-ii.toml` / `digitakt-ii-leds.toml` (AGPL-3.0, OS 1.15C-1.17 emulator work). Our own scans from the A01 snapshot (`tools/scan-panel-input.sh`) are consistent with them, which is the only confirmation on OS 1.17 so far.

- Two-byte packets: `0x20 | channel, mask` is a button channel's whole 8-bit mask (not press/release; the firmware compares it with the channel's previous mask and queues one event per changed bit); `0x30 | (encoder - 1), delta` is a signed 8-bit encoder delta (encoders 1-8 are A-H, 9 is LEVEL). `0x70` carries console/discovery. Seven button channels (0-6).
- Key code = channel * 8 + bit + 1 for codes 1-48; channel 6 bit 1 is code 49 and bit 0 is code 50. Names: 1-6 TRIG SRC FLTR AMP FX MOD; 7 PRESET, 8 SETTINGS, 9 TEMPO; 10 YES, 11 UP, 12 NO, 13 LEFT, 14 DOWN, 15 RIGHT; 16 TRK, 17 FUNC, 18 KEYBOARD; 19 RECORD, 20 PLAY, 21 STOP; 22 PAGE, 23 PTN, 24 SONG; 25-40 TRIG 1-16; 41-48 ENCODER A-H push; 49 LEVEL push; 50 SAMPLING.
- Encoder deltas are accumulated and flushed on a periodic tick (clamped to +-30 about every 4 ticks), a turn shows on screen some emulated time later, and one is dropped for 40 ticks after that encoder's push is released. The first turn may only show the value label.
- Front-panel LEDs are firmware TX records `b9 idx value` (written twice); the display decoder now consumes them (3 bytes) instead of misreading them as tiles. They have not appeared in our captures yet.
- Our scan explained with this format: `21 FF` is channel 1 with all eight bits down (codes 9-16, TEMPO .. TRK), `22 FF` codes 17-24, `25 xx` encoder pushes A-H (the parameter value labels such as `-3.00` appear), `3x vv` encoder `x+1` turning by `vv`; `20 00`-style releases do nothing. Single-key taps from the snapshot (`23 01`/`23 00` TRIG 1, `22 01` FUNC) only produced the `bc 01` reply, whose meaning is still unknown.
- Keyboard layout (my own, not the hardware's): F1-F6 pages, F7-F9 PRESET/SETTINGS/TEMPO, Enter YES, Backspace NO, arrows, Tab TRK, Ctrl FUNC, F10 KEYBOARD, F11 RECORD, Space PLAY, F12 STOP, PageUp PAGE, Home PTN, End SONG, 1-8 and Q-I TRIG 1-16, Z X C V B N M Comma encoder pushes A-H, / LEVEL push, Insert SAMPLING; A S D F G H J K turn encoders A-H and L turns LEVEL, Shift reverses (clockwise = positive delta is a guess).

### Panel input on OS 1.17: what we confirmed

- The published decoder addresses match our image: UART8 byte callback `0x4011e648`, button decoder `0x4011e318`, encoder decoder `0x4011e598`, UI queue send `0x40001896`. A `2x` packet reaches the byte callback, the button decoder and the queue within about 170 instructions of delivery; a `3x` packet reaches the encoder decoder.
- `bc 01` is sent once per received packet (19 for 20 packets in 60M instructions), so it is an acknowledgement, not LED or UI data.
- A missing `MULU.W <ea>,Dn` (opcode `0xC0C0` group) stopped the core at `0x40151b58` on the first TRIG page key; it is now implemented with a test. Single-key press-and-release scans from the A01 snapshot (`RELEASE=1 tools/scan-panel-input.sh`) now redraw the screen for page keys TRIG, FLTR, AMP, FX and MOD, TRK, SONG and ENCODER D push. SRC leaves the screen unchanged (it is already the SRC page). PRESET, SETTINGS, TEMPO, YES, NO, the arrows, FUNC, PAGE, PTN and SAMPLING only produce `bc 01` and some `b8`; menus may need a different press pattern or state.
- All 16 TRIG keys (channels 3 and 4) stop the run with an unmapped read at `0x0001db79` (PC `0x400d8e4c`): the routine at `0x400d8e00` reads a per-track pointer from the table at `0x44f37bd0` (16 longs, initialised from `0x44f37c54` by the code near `0x400d84dc`), and all entries are zero in our state. The per-track sound or sample objects are never created, so the synthetic card state or the project load is incomplete. Earlier scans that looked quiet for these keys were crashes; `tools/scan-panel-input.sh` now prints the stop reason.
- LED records `b9 idx value` are built only at `0x40133d00` (group flush, called from `0x40133da4`) and `0x401340b4` (raw set), fed by the setters `0x40133e58` and `0x40134040`, through the UART8 TX enqueue at `0x4000220c`. Over a full boot to 1.3B instructions and through page, TRK, SONG and encoder-push taps, none of the setters ever runs (breakpoints at their entries never hit), so no `b9` is sent. The flush runs once around instruction 1.068B with nothing dirty. Why the UI never requests LED updates in our state is still open; a stricter panel handshake or the incomplete project/sound state are the leads.


### Why the TRIG keys crash: no project is ever loaded

- The per-track setup code (loops at `0x400d8148`..`0x400d84e2`, entered at `0x400d8362`, `0x400d5cd8`, `0x400d6426` and `0x400d84dc`) never executes: not during the boot to 1.3B instructions, and not in 3B further instructions from the A01 snapshot. The pointer table at `0x44f37bd0` therefore stays zero while its source array at `0x44f37c54` holds the default `0x41776cb8` in every slot, and the first TRIG key dereferences a null entry.
- In that time the firmware idles at the A01 screen (about 30M instructions/s, no further eSDHC commands: 6,384 in total). Over the whole boot the card is only read at blocks `0x0` and `0x800`; no project, kit or sound region is read. This fits a blank synthetic card that the firmware formats and then leaves empty.
- Leads (unverified, from public notes on the Digitakt II card layout): header at sector 0 (magic `0xbeefbace`, also a constant in our image), protect bitmaps at `0x800`, sounds from `0x12000`, kits from `0x16000`, two boot-project copies at `0x40000` and `0x48000`, the working project at `0x50000` (rewritten from a boot copy at start), project pool from `0x58000`. The earlier inference that eight `COKi` matches in `section_3.bin` were embedded default containers was incorrect: those matches are code immediates comparing against the magic, not container payloads. Next step: find what makes the firmware read a project at start (the reads after the header check) and what a valid card image must contain.

### Root cause chain: the audio-block interrupt never happens

The per-track setup that the TRIG keys need is the body of the interrupt handler at vector 121 (INTC0 source 57, `0x400d7b92`, which clears its own INTFRCH bit 25 on entry), not a function called from the UI. Vectors installed by the firmware (read from the vector table in a snapshot): 108 -> `0x400d5bb4` (INTC0 source 44), 121 -> `0x400d7b92`, 170 -> `0x400d30fc`, 191 -> `0x4002dd0c`. The chain, from the code: the audio DMA block interrupt (vector 170, eDMA channel 50 completion: it clears its request with CINT=`0x32`, compares TCD50 SADDR `0xFC045640` with the double-buffer base `0x4fe58900`) -> vector 191 (INTC1 source 63) -> code at `0x4002f2d8` / `0x4011ef92` forces source 44 (`or.l #0x1000` into INTFRCH `0xFC048010`) -> vector 108 forces source 57 (`or.l #0x2000000` at `0x400d5f46`) -> vector 121, the per-track engine setup (16 tracks, stride `0xdbba0`) that fills the pointer table at `0x44f37bd0`. None of the force sites run during our boot because nothing raises the vector-170 interrupt: the audio clock (SSI0 requests driving eDMA channel 50) is not modelled.

Address mapping matters when comparing these instructions: `section3-pre-entry-image.bin` is loaded at `0x40000400`, while the DDR dump is overlaid at `0x40000000`. With those bases, the bytes at `0x400d30fc` match and decode as `LEA -$14(A7),A7` (four bytes); the earlier six-byte `MOVE` reading came from treating a PC as a file offset. Likewise, the ICR44 write is at `0x400d78de`, not `0x400d74de`. Its containing initialization routine at `0x400d77ce` is called from `0x400ccd82`; it installs vectors 108 and 121 and sets their priorities (ICR44=5, source 57=2). A reset run reached its entry at instruction 577,605,799, with return address `0x400ccd88`; the initialized snapshot at `0x400cce4a` is 37,485 instructions later. This setup is therefore part of startup before the receiver-task call, not a call made by the vector-170 handler. It does not show that a project has loaded; the separate missing-project/factory-copy investigation remains open.

Scheduler findings from `out/idle3b.snap` (note `SNAPSHOT_LOAD=` is a CLI argument, not an environment variable; as an env var it silently runs a fresh boot). The startup task (TCB `0x42944aac`, entry `0x400cc9c8`, priority 1) ends in `bra.b .` at `0x400cce3c`, so it is the background spin; the 900M-instruction reset run also never enters the converter (`STOP_PC=0x40181060` never hit; idle at `0x400ccf48`), so the converter item is closed as not reachable post-fix. The receiver task (`0x400337ba`, priority 6) does run: about 13.3M instructions after the snapshot it creates TCB `0x43172de4`, entry `0x400f0df2`, priority 2, and that task is dispatched (`STOP_PC=0x400f0df2` stops there at instruction 13,287,878 after the snapshot; its saved context shows a deep stack). The task is a message worker: it pends on the semaphore at object+0x14 (object `0x450575c0`, pointer at `0x44f37090`), gets two posts, and returns to waiting. The scheduler therefore does switch to newly ready tasks; an earlier draft of this note claimed otherwise because `SNAPSHOT_LOAD` had been passed as an environment variable (it is a CLI argument), so those runs were fresh boots. The per-track setup (`0x400d8362`, `0x400d5cd8`, `0x400d6426`, vector 121 at `0x400d7b92`) is still not reached in 400M instructions from the snapshot. The UI/engine task (TCB `0x4314d558`, entry `0x400d1a12`, priority 7) pends on the semaphore at `0x43149548` (call site return `0x400d1a94`). A write watch shows that semaphore is posted from the ISR post routine `0x4000148c`/`0x4000149e` at a steady period of about 4.4M instructions (e.g. instructions 580,110,286, 584,510,414, 588,910,542, 593,310,670), so this is a periodic timer-driven task that wakes, runs and re-pends; it is not stalled on a missing one-off event. The stall is therefore not in this task, which leaves the vector-170/191/108/121 audio chain as the only known path to the per-track setup. The "bit `0x40` of `0x4029eef0`" test at `0x400ccd6e`/`0x400cce34` selects alternate startup paths (it was clear in these runs).

Update (injection from `out/idle3b.snap`, all diagnostic): the EMAC opcode gap noted below is already closed (`8eeea51`); injecting vector 191 now runs the handler through the force site `or.l d0,$fc048010` at `0x4002f2d8` (D0=`0x1000`, source 44). Nothing follows because INTC0 has sources 44 and 57 masked in the snapshot (`IMRH=0xfffffffd`; only source 33 is unmasked) and the firmware's immediate-operand CIMR writes (`0x40001af2`, `0x40001cc6`, `0x400c0b8c`, `0x400cd176`, `0x400cd1ee`, `0x400cd770`, `0x4012d892`, `0x40136596`) do not unmask them. With the new diagnostic flag `INTC0_UNMASK=44,57` (applied after `SNAPSHOT_LOAD`; the run also prints an `INTC0 state:` line) the chain runs: 191 -> 108 (`0x400d5bb4`) -> 121 (`0x400d7b92`) -> the per-track loop at `0x400d8362` executes 16 times, and the pointer table at `0x44f37bd0` holds the non-null default `0x41776cb8` in all 16 slots, so the null dereference the TRIG keys hit is gone in this injected run. Open: where the real firmware unmasks 44/57 (or whether INTFRC bypasses IMR on hardware; unverified), and the natural vector-170 trigger. `STOP_PC` is unreliable for PCs in code reached via the fast path in some runs; `TRACE_REGS` showed hits where `STOP_PC` did not.

Update (TRIG check and INTFRC mask): from `out/dt2-emmc-a01-interface.snap` the baseline still stops at the unmapped read `0x0001db79` (PC `0x400d8e4c`) after 3.0M instructions. With `INTC1_BOOTSTRAP INTC0_UNMASK=44,57 INJECT_IRQ=1000000:6:191` all 16 TRIG keys run 40M instructions with no crash (TX stays 52 bytes: no LED/UI response yet), so the missing 191 -> 108 -> 121 chain is the crash cause. The unmask question is settled by the MCF54418RM (17.2.3): "The assertion of an interrupt request via the interrupt force register is not affected by the interrupt mask register." The firmware never needs to unmask sources 44/57, because it forces them. `Intc0::pending_interrupt` now lets forced sources bypass IMR (unit test updated); the 1B baseline hash `a81cd278…17d0` and the irq/rte log are unchanged. `INTC0_UNMASK` is therefore no longer needed. The remaining gap is a natural trigger for vector 170 (SSI0/eDMA channel 50).

Update (natural vector 170, diagnostic `SSI_AUDIO_IRQ=<period>`): the firmware starts eDMA channels 48 and 50 with SERQ at `0x400d3466`/`0x400d346a` (instruction ~577,755,052 in a reset run; the audio init is the routine at `0x400d3226`, which memsets `0x4fe58100`..+0x1000, programs SSI0 at `0xFC0BC000` and builds a scatter/gather TCD chain in RAM at `0x4fe57080`..`0x4fe570ff`). TCD50 is SADDR `0x4fe58100`, 32-bit, SOFF 4, NBYTES 0x20, DADDR `0xFC0BC000`, BITER 0x40, CSR `0x12` (INTMAJOR + ESG, next TCD from DLAST_SGA `0x4fe570a0`). The new flag completes channel 50 like a real major loop (SADDR/DADDR advance, ESG loads the next TCD from memory), raises INTC1 source 42 (vector 170) subject to IMR/ICR, and re-arms `<period>` instructions after the handler's CINT. Default off; the 1B baseline hash is unchanged and it is not saved in snapshots. With `SSI_AUDIO_IRQ=100000 INTC1_BOOTSTRAP` vector 170 now fires naturally (the first one at instruction 577,755,064, 2,223 times in 800M), so the audio-clock interrupt itself is no longer missing. The chain still stops there. The handler at `0x400d30fc` counts (`0x43153a20`) blocks in which the idle half-buffer (`0x4fe57100` or `0x4fe57900`, chosen by whether SADDR < `0x4fe58900`) has a 32-bit sample equal to `0x007fffff` at 64-byte spacing; after 63 it masks INTC1 sources 40/42 and swaps the vector-170 slot (`0x400002a8`) to the handler queued at `0x43153a28` (`0x4002d322`), which forces INTC1 source 63 (vector 191). Nothing ever writes the buffers after the memset (WATCH_WRITE on `0x4fe57100`: zero writes in 650M), so the marker never appears and the counter stays 0. The marker is presumably produced by the audio engine/mixer, so this may be the circular dependency noted in the review: the engine needs the 191 chain and the chain needs engine output. Open: what writes `0x007fffff` into those buffers (the mixer routines near `0x4017fa70`/`0x40180b74` saturate to that value), or whether the real counter test uses different memory.

Update (natural chain, no injection): the vector-170 handler's block test is a frame-sync check on the SSI RX stream, not on the TX buffer. Channel 48 (TCD at `0x4fe570c0`, SADDR `0xFC0BC008` = SSI0 RX) fills the double buffer `0x4fe57100`/`0x4fe57900`; the handler scans 32 words at 64-byte spacing for `0x007FFFFF` (a codec/DSP frame marker), nudges channel 48's CITER/BITER by 0x3e/0x40 to align when the marker is not at word 0, and after 63 consecutive aligned blocks switches to the handler at `0x4002d322` that forces vector 191. `SSI_AUDIO_IRQ=<period>` now also writes that marker (word 0 of each 64-byte slot of both RX halves) on every channel-50 block, and arms itself on the first poll after a snapshot restore if TCD50 is active. With `RGPIO_READY_MODEL GPIO_LOOPBACK ESDHC_EMMC INTC1_BOOTSTRAP SSI_AUDIO_IRQ=100000` from reset, 170 -> 191 -> 108 -> 121 now fire without `INJECT_IRQ` or `INTC0_UNMASK` (900M instructions: 3,222 x 170, 3,158 x 191, 345 x 108, 345 x 121) and the pointer table at `0x44f37bd0` is populated. A snapshot taken at 700M with those flags (`out/ssi-natural.snap`), then 60M instructions with a periodic TRIG key (`UART8_RX_PERIODIC=3000000:2301,2300`), runs with no crash (66 x 108/121 in that window). The old `out/dt2-emmc-a01-interface.snap` still crashes with the flag because it was saved without INTC1, so the firmware's INTC1 ICRs are lost on load. The SSI marker is a diagnostic stand-in for the real codec/DSP, not hardware evidence; the 1B baseline hash is unchanged (flag off by default). Open: LED/screen response to TRIG presses (TX bytes), and a real source for the marker.

Update (LED/UI traffic with the natural chain; all from `out/ssi-natural.snap`, diagnostic `SSI_AUDIO_IRQ`): 80M-instruction runs with no input, a TRIG 1 tap (`2301` then `2300`, 13M instructions apart so the key is held across about three PIT3 ticks), a PLAY tap (`2208`/`2200`, code 20 = channel 2 bit 3) and both produce byte-identical UART8 TX streams (10,079 bytes each): tile records only, no `b9` LED records, no `bc`, no new tile content. The input does arrive: the RX ISR (`0x40001f1a`), the byte callback (`0x4011e648`), the button decoder (`0x4011e318`) and the queue send (`0x40001896`) all run, and the kernel queue at `0x47db3f70` fills (its count at +4 goes 0x0a with no input, 0x10 after 20M and 0x1a after 50M instructions of taps) because nothing consumes it. The queue handle at `0x402b45a0` is the boot-time queue; the receiver task's own queue (`0x40966f3c`) is registered by `0x40033996` (call of `0x4011e108`), which comes right after the LED setup calls `0x401340fe` (raw LED set) and `0x40133da4` (group flush). The receiver task entry `0x400337ba` runs exactly once (reset run to 800M) but `0x40033996` and `0x401340fe` are never reached, so the task is stuck or blocked somewhere between its entry and that point; that is why no LED record is ever sent and key events are never consumed. Panel TX senders found statically: `0x4011e1b4` sends `b7 x`, `0x4011e1e4` sends `bb x y z`, `0x4011e220` sends `bc x` (called at `0x40032aaa` with 0; the `bc 01` seen earlier is a command, not necessarily an acknowledgement), and LED records `b9` come from `0x40133b56`/`0x40133d76`/`0x40133f7a`/`0x401340ca`. `out/rt-entry.snap` (saved by `STOP_PC=0x400337ba SNAPSHOT_SAVE=...` from reset, with the SSI flags) restarts the receiver task at its entry. Also: loading a snapshot saved without INTC1 while `INTC1_BOOTSTRAP` is on now fails with a mismatch error instead of silently losing the firmware's INTC1 levels. Next: find where the receiver task blocks between `0x400337ba` and `0x40033996`.

Update (receiver task wait chain): the prior note that the receiver was merely "stuck somewhere between its entry and LED setup" is now narrowed to two waits. At `0x400337dc` the task calls `0x400d1502`, which waits via kernel routine `0x4000141a` on semaphore `0x43149550` (count zero). The engine task eventually signals this semaphore at `0x400d1a9a`; from `out/rt-entry.snap`, the receiver resumes at `0x400337e2` after 243,954,884 instructions. It then creates a helper task (entry `0x40133922`, priority 6), registers callbacks, and waits in `0x401338f2` on a second semaphore, `0x44e460e0`; its waiter pointer is `0x40966ee8`, the receiver TCB. The helper task is woken repeatedly through semaphore `0x44e460d8` (22 increments in a 100M-instruction watch), but `0x44e460e0` receives no writes in that run. TRIG press/release packets delivered while the receiver waits still reach the UART callback and increase the boot queue count, but do not write `0x44e460e0`; the registered signal callbacks (`0x400334bc` and related paths calling `0x40133902`) are not reached. So the missing step is now the event/resource callback that signals `0x44e460e0`, not the original engine semaphore. LED setup at `0x401340fe` and queue registration at `0x40033996` remain downstream of this wait. Next: identify what should invoke those callbacks and why it does not happen in the emulator.

Update (identity of the second wait): disassembly of the receiver shows that immediately before blocking it registers a named `"Factory reset"` observer: the event key is the string at `0x4023afff`, and the registered function pointers are `0x40032c58` and `0x400334bc`; the latter calls `0x40133902` to signal `0x44e460e0`. The `rt-await-event.snap` snapshot confirms the registration is present in RAM, so this is not a failed subscription. A generic dispatcher candidate is `0x400f0abc`, called by worker entry `0x400f0df2`, but its connection to the `"Factory reset"` publisher is not yet demonstrated. A static scan finds no other image reference to the event-name pointer, so the publisher may use a different key or an indirect/dynamic path. The wait is therefore for the factory-reset notification/completion path, not for TRIG input; its producer and any dependency on storage/project initialization remain open.

What exists now: INTC0 delivers forced (INTFRC) sources, and an optional INTC1 (`INTC1_BOOTSTRAP`, base `0xFC04C000`, vectors from 128, snapshot-saved only when present) uses the same model; the 1B baseline hash is unchanged because both stay off or idle by default. Injecting vector 170 from reset (`INJECT_IRQ=...:6:170`, with `INTC1_BOOTSTRAP`) runs its handler but does not reach vector 191; injecting 191 stops at an unsupported opcode `0xA988` at `0x4002dd36`, a ColdFire EMAC instruction (`MOVE.L MACSR,A0` by the public encodings; the core has no EMAC yet). Next steps: implement the EMAC register file and moves (then MAC/MSAC as the firmware needs them), then model the SSI0/eDMA-50 audio block request so vector 170 fires periodically. Any such audio clock is a synthetic timing model and needs its own labelled baseline hash.

Update (eMMC capacity and factory-project path): the synthetic eMMC encoded EXT_CSD `SEC_COUNT` little-endian, but OS 1.17 reads the DMA buffer as a big-endian long at `0x4012dcaa`. The model therefore made its advertised 4-GiB card appear to contain only `0x8000` sectors; reads of the boot-project headers at `0x40000` and `0x48000` failed the firmware's range check. The model now encodes that field in the order the firmware consumes, with a regression assertion in `cmd8_returns_ext_csd_and_raises_completion`. A fresh run reaches both header reads as CMD18 at `0x4012e32e`.

The headers are still empty, so the project is not copied. The fallback at `0x400f0646` scans DSPI0 NOR offsets `0x400000`, `0x800000`, and `0xc00000` for the `COKi` magic before writing a project to eMMC; the configured `out/dt2-os-1.17-container-nor.bin` has erased (`0xff`) bytes at those offsets. In the 1B-instruction diagnostic run, the fallback sets bit `0x4` in `0x4029eef0`, and only the three format writes (blocks `0`, `0`, and `0x800`) occur. All 16 entries in `0x44f37bd0` contain the same default pointer `0x41776cb8`; the receiver remains blocked on `0x44e460e0`, and the LED-setup breakpoint at `0x401340fe` is not reached. This leaves missing factory/project content as a plausible blocker for the `"Factory reset"` event producer, but that relationship is not yet demonstrated. Next: locate a valid factory `COKi` payload and establish its correct NOR/card mapping; do not synthesize project contents from the marker alone.

Update (FUNC held during section-2 startup): with the diagnostic panel-discovery reply reporting channel 2 / mask `0x01` (FUNC), section 2 takes an alternate path and remains in its input loop at `0x80000f78` after 380M instructions. It does not hand off to section 3 at `0x400004e8`; the eSDHC trace has zero commands, so this run does not reach any factory-reset source read. The startup-menu selection was confirmed as a direct TRIG 3 press. I tested channel 3 / mask `0x04` on the section-2 polled ring, alternating press/release every 40M instructions; after 1.2B instructions the firmware remained in section 2 at `0x8000067e`, with no eSDHC commands. A second run periodically supplied a full seven-channel state report with TRIG 3 down and likewise stayed in section 2, ending at `0x80000690` after 600M instructions with zero eSDHC commands. These synthetic input diagnostics do not confirm the hardware's post-discovery receive timing or complete the Factory Reset action.

The scanner evidence is separate: its three probes are DSPI0 NOR offsets `0x400000`, `0x800000`, and `0xc00000`, each erased in the configured NOR image. The eight `COKi` occurrences in the section-3 image are immediate constants embedded in code, not eight container payloads; the NOR image itself contains no `COKi` occurrences. No labeled placement test was made because the firmware image contains no verified container bytes to place.

Update (corrected section-2 input ring): late synthetic input was previously appended at `ring + available_count`, which overwrote discovery bytes already consumed by section 2. The poller advances its cursor at `0x8000840c` and tracks remaining bytes at `0x80008410`; input now appends at `ring + cursor + available_count`. With FUNC held in the discovery report and TRIG 3 pressed/released (`23 04`, `23 00`), the corrected run reaches `HALT` at `0x80000d7a` after 33,183,944 instructions. At that point `0x800083fc` is `1`; the section-2 eSDHC trace still has zero commands and zero storage writes. This confirms a changed section-2 path, not a completed Factory Reset: the emulator does not model the resulting hardware reset/reboot, and this run does not identify any factory source read. The NOR scanner evidence above remains a separate path and no payload placement is justified.

Update (factory install completes on its own; resolves the "missing factory content" lead): the earlier "only three format writes" came from stopping at 1B instructions. With `RGPIO_READY_MODEL GPIO_LOOPBACK ESDHC_EMMC ESDHC_TRACE INTC1_BOOTSTRAP SSI_AUDIO_IRQ=100000` on the blank synthetic card, a 3B run (10,032 eSDHC commands, 28,368 completed block writes) shows: header reads at `0x40000`/`0x48000` (instruction 52.2M), writes of blocks `0` and `0x800` (588.6M and 823.5M), erases of `0x12000..0x36000` and `0x58000..0x458000` (about 822M-823M), and reads of 3-block slots stepping 8 blocks from `0x12000`. A second write of `0x800` at 1,087,685,392 is followed by 28,364 contiguous blocks at `0x58000..0x5eecc` (1,087,704,091 to 1,091,559,773). No eSDHC command occurs after instruction 1,091,560,629. The receiver's semaphore `0x44e460e0` is signaled at instructions 1,091,563,089/093 (PCs `0x400011f6`/`0x40001200`, waiter `0x40966ee8`, the receiver TCB), and the LED-setup address `0x401340fe` is then reached once (1.5B run). So the receiver's wait ends when the firmware's own blank-card factory install finishes; no `COKi` payload or NOR content is involved, and the "Factory reset" notification is published at the end of that install. The bytes written at `0x58000` come from firmware logic whose source data has not been traced here; their content is synthetic-card state, not hardware evidence. The block map matches the earlier unverified card-layout leads (sounds region from `0x12000`, project pool from `0x58000`), not the Digitakt 1 layout from the sibling `digiemu` notes (project 0 at `0x80000`), so the two firmwares differ there.

Input after the install (snapshot `out/blank-factory-1p5b.snap` saved at 1.5B with the flags above; 150M-instruction resumes, diagnostic `SSI_AUDIO_IRQ`): with no input the UART8 TX stream is empty; a periodic TRIG 1 tap (`UART8_RX_PERIODIC=13000000:2301,2300`) emits 22 x `bc 01` and a PLAY tap (`2208,2200`) emits 11 x `bc 01`. These are the first input-dependent TX records; the earlier runs with the receiver blocked produced none. No `b9`, `b7`, or `bb` records appear, so the meaning of `bc 01` and the actual LED encoding in 1.17 remain open. Next: read what the code at the `bc` sender (`0x4011e220`) is reporting, and find the LED writer reached from `0x401340fe`.


### Update: `bc` sender trace

- `0x4011e220` sends `bc x`; `0x4011e1b4` sends `b7 x`. `0x40030b50` clears `+0xe4` of its object argument and tail-jumps to the `bc 01` send. In the first-TRIG stop its return address was `0x40030be6`, inside `0x40030bd0`, which `0x40033d94` calls from the receiver event loop (TCB `0x40966ee8`).
- After the post-install snapshot with TRIG taps, `0x4011e220` is hit, but none of the LED/raw senders (`0x40133b56`, `0x40133d76`, `0x40133f7a`, `0x401340ca`) or `0x40133da4`/`0x401340fe` are reached in 60M instructions. So the input is consumed by the receiver loop, and the LED writer is not triggered by it.
- The sibling Digitakt 1 project's notes list `0xB9 idx val` (3 bytes) as observed, but our DT2 captures have never shown `b9`, so the `b9` decoder assumption is unverified for 1.17. `b5 id r g b` palette records are sent after install (136 of them).
- `bc 01` meaning, the LED encoding, and the 22 vs 11 per-tap counts remain open.

### Update: `0x40030bd0` dispatcher

- `0x40030bd0(obj, ev)` (called from the receiver loop at `0x40033d94`, right after `0x40114904(ev, +4, +8, +c, 0x7f)`) does three things: (1) calls `0x40030b50(obj)`, which sends `bc 01` and clears `obj+0xe4`; (2) walks 16-byte entries in `[obj+0x94, obj+0x98)` and calls `entry+0xc(entry, ev)` for each entry whose `+8` is non-zero; (3) tail-calls `0x4011c79c(obj+0x40, ev)`, a tree lookup keyed on the event.
- So `bc 01` is sent per dispatched input event, before the per-listener callbacks. It looks like a panel activity/wake notification, not an LED update. LED output must come from a listener callback or from a separate refresh path that these taps do not trigger. The 22 (TRIG) vs 11 (PLAY) difference is probably the number of events each tap packet produces after filtering in `0x40114904`; this is unverified.
- Next: dump the listener list at `obj+0x94` after install (`obj = [receiver frame -0x80(a6)]`) and trace the callbacks.

### Update: dispatcher listeners

- The dispatcher object is `0x44f5fe40` (receiver stack slot `-0x80(a6)`); the event is a stack struct at `0x4098f728` (word `+0x14` = `0x19` for a TRIG tap, `+0x18` = 1, `+0x1c` = `0x7f`).
- The listener walk at `0x40030bf0..0x40030c0a` sees a one-entry list (`0x450cb250..0x450cb260`) whose callback is `0x40061940`, called once per dispatched event. It branches on the event type from `0x40114984(ev)` (`0xa`, `0xc`, ...) and, for a matching type and a true `0x401149f8(ev)`, tail-calls UI handlers (`0x4005af44`) with `obj+0x9c`. That is a widget-level UI event handler, not an LED writer.
- A dump of `obj+0x94..0x98` at the first dispatch does not look like a flat 16-byte vector (it reads as linked-list nodes), so the list is rebuilt between events. Do not trust a static dump; use `TRACE_REGS=0x40030c02` (callback call site) instead.
- Next: find which event types `0x40061940` accepts for TRIG/PLAY, and follow `0x4005af44` to see whether it reaches the LED refresh (`0x401340fe` / `0x40133xxx`).

### Update: LED setter exists but is never reached

- Dispatcher listener `0x40061940` accepts only event types `0xa`, `0xc`, `0x12` (type is `[ev+0xc]`; `0x401149f8` tests `[ev+0x10]` bit 0 is clear). The TRIG tap event type is `0x19`, so this listener ignores it. Only one listener per dispatch has `+8` non-zero (4 calls seen in 60M instructions, all `0x40061940`).
- `0x400308e0(obj, idx, mode, val)` looks like the LED setter: it validates idx via `0x40133ef2`, stores `obj+0x28+4*idx` and `obj+0xf0`, then tail-jumps to `0x40134040`. It has 11 direct callers and about 100 more pointer references across the UI code (`0x40016c72`, `0x4005c2e0`, `0x4005d892`, ...).
- In a full boot to 1.5B instructions (blank card, diagnostic `SSI_AUDIO_IRQ`), neither `0x400308e0` nor `0x40134040` is ever executed. So LED output depends on UI code that has not started yet; TRIG/PLAY input is not the gate.
- Next: find which task or wait stops the main UI/app task from reaching its first screen (run `SCHED_TRACE`, list blocked TCBs and their wait objects after 1.5B), rather than looking at the receiver.

### Update: all tasks idle at 1.5B (blank card, diagnostic `SSI_AUDIO_IRQ`)

Saved-SP is at TCB+0x48; the return address 3 words into each saved stack identifies the wait call:

| entry | prio | waits at (return) | wait object |
|---|---|---|---|
| `0x400d1a12` engine | 7 | `0x400d1ac4` | `0x43149548`-ish engine sem |
| `0x4012c590` | 8 | `0x4012c4ce`..`0x4012c5ac` | not identified |
| `0x4011f08e` | 7 | `0x4011f108` | `0x44e15bcc` |
| `0x40153f34` | 5 | `0x40153fba` | `0x405a0f44` |
| `0x400337ba` receiver | 6 | `0x40033d0c` | queue `0x40966f3c` (event loop, works) |
| `0x400f0df2` / `0x400f0f0a` | 2/3 | `0x400f0aec` | `0x450575d4` / `0x450cdfd4` |
| `0x40133922` | 6 | `0x401339f0` | `0x44e460d8` |
| `0x401353dc`/`0x40135530`/`0x40135656` | 5/4/4 | `0x4013539a`/`0x401354a6`/`0x401355fa` | not identified |

- Every task is blocked; nothing is spinning. The first UI-level candidates are `0x4011f08e` (`0x44e15bcc`) and `0x40153f34` (`0x405a0f44`). `WATCH_WRITE` on the first byte of each shows only the boot-time initialisation write (instruction 764,085 for `0x405a0f44`), so nothing posts to either afterwards (first-byte watch only; the count field may be elsewhere).
- `SCHED_TRACE` keeps only the last 1,024 events, so it cannot show earlier waits on a long run. Use TCB dumps instead.
- Sibling DT1 notes say its UI task never started until the ColdFire-side DSP-handshake sync primitives were satisfied (no SHARC modelling). DT2 likely needs the same: find the producer that should post to `0x44e15bcc` / `0x405a0f44`.

### Update: sibling DSP-handshake hint, checked

- The "status word 0 -> 2, edge-port signalling, boot task semaphore" handshake in the Digitone sibling's notes (`docs/FINDINGS.md`, "Digitone (mk1) 1.43: the second CPU") is specific to Digitone mk1, whose DSP is a second ColdFire. It does not apply to Digitakt II, whose DSP is a SHARC on a DSPI2 frame link.
- The Digitakt II analogue in the sibling's 1.15C/1.16 notes is the frame-link gate: `0x409664f4` gate, `0x409664f8` mode, `0x409664f0` countdown, `0x409664ec` stop flag. 1.17 uses the same addresses as 1.16. The helper `0x4002dc62` sets gate=mode=1 and installs the vector-191 handler; `0x4002dcb2(0)` (called by the receiver task at `0x4003395c`) opens the gate (clears gate and countdown).
- In `out/blank-factory-1p5b.snap` all four are 0, so the firmware itself has already opened the gate and the vector-0xbf frame handler is running. The gate is not what holds the UI back. No DSP-alive fake is needed for this experiment.
- UI queue `0x40966f3c` (count at `+4`): in the 1.5B boot it gets puts at about 1.113B, 1.196B, 1.201B, 1.289B, 1.377B and 1.466B instructions (about 88M apart after 1.2B), each consumed by the receiver. Job queue `0x405a0f44` (task `0x40153f34`) gets no puts at all, and the dispatcher semaphore `0x44e15bcc` / its queues are never written after init.
- Next: identify the producer of the periodic UI-queue puts (breakpoint at `0x40001896` / `0x400018e4` with the queue argument, read the return address) and see what message it carries; then find what should send the first "activate main view" message.

## Receiver-queue poster trace (`TRACE_REGS=0x40001896`, stack(A7) = `[ret, queue, msg, ...]`)

Queue-put `0x40001896(queue, msg)` over a full 1.5B boot (131 snapshots):

| Caller (return PC) | Queue | Msg | Count / when |
|---|---|---|---|
| `0x4002e750` | `0x44e25220` (unrelated, 14-byte entries `0x40966508..0x40966774`) | pool nodes | 111, ~1.094B |
| `0x400d8cfa` | `0x40966f3c` (receiver) | `0x4020e2ab` | ~every 88.2M from 1.113B |
| `0x400d8cfa`/`0x400d76e4`/`0x400d779a`/`0x400d8fd8`/`0x4011e4a4` | `0x47db3f70` (other queue) | `0x44dc7e14..`, `0x4020e2a9/ab` | early boot + periodic |
| `0x40153928` | `0x40966f3c` (receiver) | `0x405a0f9c` | once, ~1.196B |

- `0x400d8cfa` is inside the vector-0x79 (SSI) handler (`pea $4020e2ab; move.l $402a1c0c,-(a7); jsr $40001896`, `rte` at `0x400d8dda`). The periodic put is driven by the `SSI_AUDIO_IRQ` diagnostic, not by a real UI source. Payload at `0x4020e2ab` is constant data (`0a 07 00 00 00 05 26 5c ...`), delivered as event type `0x19`; the listener `0x40061940` ignores it.
- `0x40153928` is the tail of a post helper at `0x401538fe`: it writes an 8-byte record (type byte at `+0`, value at `+4`) into a ring of 0x800 entries at `0x405a0f9c` (index `0x405a4f9c`), then puts the record address into the queue held in `0x402b6b68` (= receiver queue). So the job-queue task `0x40153f34` does run and posts one record to the receiver; nothing after it.
- Posters to job queue `0x405a0f44` exist statically: `0x401540ea` (msg = a2) and `0x40154324` (msg `0x405a0f74`). Neither executes in the 1.5B boot. The queue is created at `0x40153c32..0x40153c38` (`0x40001834`), the task `0x40153f34` at `0x40153c4c`. Callers of those posters are the producers still missing (likely a timer/timeout path: `0x40154316` tests `d5 > 0` before posting, else `0x401588c0`).
- Dispatcher objects: `0x44e15bcc` (sem), `0x44e15bd4`, `0x44e15bf4`, `0x44e15c14` are referenced only from `0x4011f0xx-0x4011f2xx` (the task `0x4011f08e` itself) and the init at `0x40120a08-0x40120a44`. There is no external poster anywhere in the image via direct references, so posts must go through a pointer (object address passed at runtime or stored in a table). No put/post with those objects as arguments occurs in 1.5B.
- Conclusion: no ISR or task posts a UI/panel event to the receiver; the only real post is the one-off from the job-queue task. The dispatcher semaphore is not posted because its producer (reached via pointer) is never invoked; next step is to trace callers of `0x40154324`/`0x401540ea` statically and find who calls those routines.

## Static xref trace of the job-queue posters (`0x401540ea`, `0x40154324`)

Method: byte-level xref scan of `out/section3-pre-entry-image.bin` (abs/pc-relative jsr/jmp/bsr/bcc, `pea`/`lea`, data words), plus 1.5B-boot `TRACE_REGS` runs at every PC listed as "never runs".

**`0x401540ea` is not a separate function.** It is inside the job task `0x40153f34` itself (loop `0x40153fb2` .. `0x4015410e`, which blocks on `0x405a0f44` through `0x40001928`). After a chunk is processed it does `remaining = [a2+0x10] - chunk`; if `> 0` it re-queues the same job record (`a2`) onto `0x405a0f44`, otherwise it frees it via `0x401588c0`. The chunk sizes (`0x8000`, `0x4000`, copy through `0x40136388`) show it is a chunked file/flash copy job engine. It has no callers; it can only fire after the task has received a job whose `[a2+0x10] > 0`. Task `0x40153f34` is created at `0x40153c4c` by init `0x40153c26`, which is called once from `0x400cce44` (this does run). The task is blocked on `0x405a0f44`, as in the wait map.

**`0x40154324` is inside function `0x40154112`** (`link a6,#-0x80`; arg `8(a6)` = pointer to a file/job descriptor; fails with -1 if the pointer is NULL or `[arg]` is -1). It fills the static job record `0x405a0f74` (fields `0x405a0f84..0x405a0f98`) and, if the length is positive, puts it onto `0x405a0f44`; otherwise it calls `0x401588c0`. Callers of `0x40154112` (all of them, jsr/jmp/pc-rel/data):

| Caller | Enclosing routine | Belongs to | Reached how |
|---|---|---|---|
| `0x40030b14` | wrapper `0x40030b10(path)` (calls `0x40154112`, on success `0x40153dfa` then `0x40139d8c`) | called from `0x40026a4a` and `0x400345d6` | see below |
| `0x400f5aa8` | virtual method `0x400f5a50(this, arg)` (submits `this->obj + 0x440`) | C++ object method, vtable slot in 3 vtables (`0x401f2464`, `0x40216d90`, `0x4021ff60`) | only via an object of those classes, none instantiated/called in the boot |

Callers of the wrapper `0x40030b10`:
- `0x400345d6` is the **receiver task's switch case for event type `0x21`**. The jump table at `0x40033d22` (42 entries, index = `[msg]`) maps type 33 -> `0x400345ce` (-> `0x40030b10`) and type 34 -> `0x400345e0` (-> sibling `0x40030b3a`; the same wrapper is also reached from `0x40026a76`). The receiver task is alive and blocked on `0x40966f3c`.
- `0x40026a4a` is in virtual method `0x400269c6` (a UI object, `this+0x1fc/0x204/0x20c` fields), which sits in vtables at `0x401ee680` and `0x401f0650`.

**Who produces receiver types `0x21`/`0x22`:** the block at `0x400d4930-0x400d4988` builds a 10-byte message at `0x43158f68`. Its type byte is `0x22 + (flag ? 0xff : 0)`, i.e. `0x21` when the record flag byte `[a2]` is non-zero and `0x22` otherwise, with the two longs from `[a2+2]` and `[a2+6]`. It then puts it to `0x40966f3c` through `0x40001896` (return `0x400d4988`). It is entered at `0x400d4946`, only from the command switch at `0x400d4684` (command code `0x34`), inside the command decoder `0x400d45c4` (`link a6,#-0x4344`; compares codes `0x10 0x20 0x31..0x34 0x42 0x43 0x45 0x50 ...`). `0x400d45c4` is reached only by `jmp` from `0x4003372e` and `0x4003376e` (receiver-side callback region `0x400336f4`-`0x40033734`).

**Other producers of receiver events (static refs to `0x40966f3c`):** `0x400c0b44` (an `rte` handler that posts the constant `0x40241370`, the string "Fancy Error View": a fault/error-screen handler), `0x400d4982` (above), `0x400f0aa6` (generic allocator-based post from a pool at `0x431711dc`, type byte `6`; function entered at `0x400f09d6`, pc-relative ref from `0x400f0b8c`), `0x40153922` (job-task helper `0x401538fe`, via `0x402b6b68`, whose default queue is `0x47db3f70` when unset), `0x400d8cf4` (SSI diagnostic).

**1.5B boot (6+4 `TRACE_REGS` runs): hit counts.** `0x40033d00` (receiver loop head) 7; `0x40154112`, `0x40030b10`, `0x400345ce`, `0x400f5a50`, `0x400269c6`, `0x400d45c4`, `0x400d4946`, `0x400d3660`, `0x400f09d6` all **0**.

**Conclusion.** Neither poster has an independent caller. `0x401540ea` is the job task's own re-queue, so it needs an initial job. `0x40154324` is only reachable through `0x40154112`, whose only callers are (a) the receiver's type-`0x21` case, which needs the command decoder `0x400d45c4` to see command `0x34` with a non-zero flag, and (b) two object methods (`0x400f5a50`, `0x400269c6`) of UI/file objects that are never invoked. The command decoder itself never runs in the boot, so the missing first domino is whatever calls `0x400d45c4` (via `0x4003372e`/`0x4003376e`). Next: find which callback registers/enters that path (`0x400336f4`, `0x40033734`, `0x40032f4e`, `0x40033330` handler pointers stored at `-0x20/-0x24(a6)` in the receiver setup) and why it is not invoked.

## Receiver callback region: who feeds command 0x34 (static + 1.5B `TRACE_REGS` hit counts)

**The four "handler pointers" are not four handlers of one kind.** The receiver setup (`0x400337ba`, around `0x40033a40`-`0x40033b8c`) builds `std::function`-style objects on the stack (`-0x2c` data, `-0x24` manager, `-0x20` invoker) and assigns each to a *channel* of a global bus with `0x401347ba` (adds `0x2c`, tail-jumps to the function-object assign `0x401d96ba`; `0x401d9688` copies `+8` manager / `+0xc` invoker and calls the manager with op 2 = clone). The bus (`0x4019f91a`, singleton `0x44f371d8`, `0x82424` bytes) holds 8 channels of `0x10484` bytes each (`0x40134cc8(bus, n)` = `bus+4+n*0x10484`). The callback sits at channel `+0x2c`; the parser calls its invoker at `[ch+0x38]` when `[ch+0x34] != 0`.

| Pointer | Role | Channel(s) | Does |
|---|---|---|---|
| `0x40032efc` / `0x40032f4e` / `0x40032fa0` | **manager** routines (op 0 type, 1 clone, 2 create, 3 destroy) | n/a | not handlers; never the entry |
| `0x400336f4` (invoker, manager `0x40032efc`) | invoker | 0 | `0x4019e44e()` ctx, then `jmp 0x400d45c4` (command decoder) |
| `0x40033734` (invoker, manager `0x40032f4e`) | invoker | 1 | same thunk, then `jmp 0x400d45c4` |
| `0x40033330` (invoker, manager `0x40032fa0`) | invoker | 2-7 (loop at `0x40033aca`) | lazy-creates a `0x284` object `0x44f371d0`, forwards to `0x40135012` |
| `0x40032c36` / `0x40032c42` | invokers on named signals `0x44f2bc58` / `0x44f2bc48` | n/a | frame gate: `0x4002dcb2(1)` / `0x400d8f5e(1)` then `0x4002dcb2(0)`; unrelated to commands |

So the real entry to `0x400d45c4` / command `0x34` is the **channel-0 or channel-1 invoker** (`0x400336f4` / `0x40033734`).

**How they are called.** Only by the byte-stream parser `0x401349f0(channel, buf, len)` at `0x40134ab2` (`jsr [ch+0x38]`, args fo-ptr, channel id, header ptr, payload ptr, payload len). It requires: first byte `0xE0+n` equals the header's channel; a `0x1c`-byte header with valid magic/size words and a payload length that matches `[hdr+0x16] & 0x7fffffff`; and a non-null callback manager `[ch+0x34]`. The decoder reads the command byte at `hdr+0xe`; command `0x34` with payload flag byte `[a2]` builds the 10-byte type `0x21`/`0x22` receiver message.

**What feeds the parser.** Three pump tasks, each pending on a queue for a message whose first byte is `0x25`:

| Task | Queue | Parser call |
|---|---|---|
| `0x401353dc` (prio 5) | `0x44e4b028` (`[0x4029ef28]`) | channel 0, `0x40135360` |
| `0x40135530` (prio 4) | `0x44e4afb4` (`[0x4029ef2c]`) | channel from `byte-0xe0`; `0xFF` bytes are echoed (inverted) to `0x44e4b048`, `0x40135472` |
| `0x40135656` (prio 4) | `0x44e4af40` (`[0x4029ef24]`) | `0x40135302` |

The only producer is `0x40004c4a` (receive-completion callback in a transfer descriptor at `+0x24`, magic `0xdead0001`, buffer pool `0x4fe4a000`, `0x400` bytes, DMA/USB slot 6). It copies the buffer, appends a type-`0x25` record to the ring `0x40312004`, and routes by the first byte: `0xE0` to `[0x4029ef28]`, `0xE1` or `0xFF` to `[0x4029ef2c]`, `0xE2`-`0xEF` to `[0x4029ef24]`, then `jsr 0x40001896` (return `0x40004d16`). The poll tick `0x40004b74` reads `0xFC0B014C`; the driver touches `0xFC0B01B4`/`0x1C8`/`0x1CC`. `0xFC0B0000` is the **MCF5441x USB OTG (ChipIdea EHCI) controller** (sibling notes `docs/mk1/03-peripherals.md`: not modelled; vector 209 handler reads `0xFC0B014C/1A4`). So this is a **USB device-side data path**: a PC host sends `0xE0`-framed packets, not the front panel.

**1.5B boot hit counts (TRACE_REGS):** registration `0x401d96ba` 16, receiver setup `0x40033a64` 1, ctx getter `0x4019e44e` 9. **0 hits:** `0x400336f4`, `0x40033734`, `0x40033330`, `0x40032c36`, `0x40032c42`, `0x401349f0` (parser), `0x401354b2`/`0x401353a6` (pump receive), `0x40004c4a` (rx completion), `0x40004b74` (tick), `0x40004b20` (descriptor start), `0x40004dac` (USB driver init), `0x40004a4e`.

**Why it never fires / what is upstream.** The USB driver is never started: `0x40004dac` is reached only by `bra` from the init variants `0x40004e92` (jumped from `0x40003690`, inside start function `0x40003608`, which calls `0x400035ee` init) and `0x40004e9c`. `0x40003608`/`0x400035ee` appear only as data words in a driver record at `0x4029f06c`-`0x4029f074` (`0x40003608` start, `0x400035ee` init); no direct caller exists. Nothing walks that record in the 1.5B boot (I did not find the walker statically). Even if started, without a USB host nothing arrives, so no `0xE0` frame, no command `0x34`, no type `0x21` and no job on `0x405a0f44`.

**No other path:** the only statically-referenced producer of command-`0x34` records is that parser; the panel UART/SPI paths post receiver events of other types (not `0x21`/`0x22`).

**Conclusion.** The command-0x34 -> type-0x21 -> `0x40154112` -> job-queue chain is a USB host command path, not the front-panel path. It is expected to be idle on a headless boot and should not be used as the reason the UI is stuck. The dispatcher semaphore `0x44e15bcc` / job queue `0x405a0f44` producers should be looked for elsewhere (the task `0x4011f08e` side), not here.

## Dispatcher task `0x4011f08e` / semaphore `0x44e15bcc`: it is the MIDI-input dispatcher

**What the task is.** `0x4011f08e` links three queues to one select-semaphore through `0x4011d9fc(port, queue, sem)`, which stores `queue` at `0x44d4e8d8 + port*0x34 + 0x24` and `sem` at `+0x28`:

| port | state block | queue | fed by |
|---|---|---|---|
| 0 | `0x44d4e8d8` | `0x44e15c14` | `0x4011d4d6` byte feeder, via wrapper `0x4011da38` (UART RX callback) |
| 1 | `0x44d4e90c` | `0x44e15bf4` | `0x4011da4c` buffer parser (USB-MIDI packets) |
| 2 | `0x44d4e940` | `0x44e15bd4` | `0x4011dc80` -> `0x4011d4d6` with port-2 state |

The init at `0x40120a06-0x40120a48` creates the three queues (`0x40001834`) and the semaphore (`0x40001398`). The task body (`0x4011f146-0x4011f232`) takes the first byte of each message, tests `0xF0` or a channel-nibble match against `0x42936872`, and calls `[0x4021e67c + (status>>4)*4]`: a MIDI status-nibble dispatch (note on/off, CC, program change, sysex...). `0xF0`/`0xF7`/`0xF6`/`0xF8` real-time handling is in the byte feeder.

**Posters (complete list).** Post primitives: `0x4000155c` = sem post wrapper (disables IRQs, calls `0x400011ee`); `0x40001896` = queue put (posts the queue's own embedded sem at `+8`). A scan of every `jsr/jmp/bsr` to `0x400011ee` finds only `0x40001568` (inside the wrapper) and `0x400018d6` (queue put). Posters of `0x4000155c` that belong to the MIDI module are the `queue put -> sem post` pairs at `0x4011d5e8/0x4011d5f2`, `0x4011d632/0x4011d642`, `0x4011d6d4/0x4011d6e4` (inside `0x4011d4d6`, completed sysex/short messages) and `0x4011dc64/0x4011dc6c` (inside `0x4011da4c`). All of them take `queue`/`sem` from `[state+0x24]`/`[state+0x28]`, which is why the literal xref scan sees no external poster.

**Who calls the feeders.** `0x4011da38` is registered as the RX-byte callback of the MIDI UART (`0x40002844(0x7a12 = 31250 baud, cb)`, UART at `0xEC074000`, called from `0x400ccad0`). `0x4011da4c` / `0x4011dc80` are the two USB-MIDI callbacks stored at `0x409659d0` / `0x409659d4` by `0x4000434a` (called at `0x400ccae2`). There is no other caller.

**1.5B boot results** (`TRACE_REGS` hit counts and `WATCH_WRITE`):

| PC | hits |
|---|---|
| `0x40002844` (MIDI UART init) | 1 |
| `0x4000434a` (USB-MIDI callback registration) | 1 |
| `0x4011d9fc` (task links ports to the sem) | 3 (at instruction ~578.3M) |
| `0x4011d4d6` (byte feeder) | 0 |
| `0x4011da4c` (USB packet parser) | 0 |
| `0x4011dc80` | 0 |

- `WATCH_WRITE=0x44e15bcc:0x44e15c40` shows only the boot zero-fill, the three queue inits, the sem init, and one write at 578.30M (`0x40001432`, the task pending and registering itself as waiter at `+4`). No put or post ever touches the objects.
- `WATCH_WRITE=0x44d4e8d8:0x44d4e974` shows only the per-port init (~35.06M) and the six queue/sem link stores at 578.30M (`0x4011da1c/0x4011da32`). The port state is never written afterwards, so no byte was ever fed.

**Conclusion.** `0x4011f08e` waits for MIDI IN (UART 31250 baud) or USB-MIDI input. It is idle because nothing arrives; it is not part of the front-panel UI path, and the emulator need not "post" to it. (The "first frame ready / panel scan complete" hypothesis does not apply to this task.) Together with the previous section, both of the originally suspicious tasks (`0x4011f08e` and `0x40153f34`) are host-input paths (MIDI and USB).

**Remaining unidentified waiters (candidate UI path).** `0x4012c590` (prio 8; many sem posts nearby at `0x4012d5a8..0x4012d6b6`, `0x4012b76a`), `0x400f0df2` / `0x400f0f0a` (`0x450575d4` / `0x450cdfd4`), the engine task `0x400d1a12`, and `0x40133922` (`0x44e460d8`, the panel sender side). Panel (UART8) input becoming receiver events is still not located.

## Remaining waiters: `0x4012c590`, `0x40133922`, and the UART8 RX path

Trace only; static disassembly plus 1.5B-boot `TRACE_REGS` hit counts (`TRACE_REGS` keeps the newest 192 snapshots) and one `STOP_PC` run.

### `0x4012c590` (prio 8): audio-engine control task, alive, not on the panel path

- Created at `0x4012d4a6` (`0x400012c8`) with queue `0x44e25220` (`0x40001834` at `0x4012d492`). It pends on it at `0x4012c5a6` and switches on the message type word at `+0`. Records are 16 bytes in per-type rings of 64 (`0x44e25240`, `0x44e2614c`, `0x44e25c48`, `0x44e25744`, `0x44e3...`).
- Posters (all go through the put `0x40001896` with `pea 0x44e25220`):

| type | poster | owner | 1.5B hits |
|---|---|---|---|
| 1 | `0x4012d4ce` (builds a note/voice descriptor; put at `0x4012d578`) | note-on routine, callers `0x40120382` (in `0x401200f2`) and `0x401205d6` (in `0x401204ec`); those two functions have ~7 and ~8 callers across the sequencer/UI/command decoder (`0x4001a890`, `0x4005c660`, `0x400d470a`, ...) | 0 / 0 / 0 |
| 2 | `0x4002e74a` | inside the SSI audio-DMA ISR `0x4002dd0c` (reads TCD `0xFC045640`, compares with `0x4fe58900`) | **111** |
| 3 | `0x4002df6e` | same ISR, other branch | 0 |
| 4 | `0x4012c01e` (put at `0x4012c054`) and `0x4012c05e` | `0x4012c01e` called only from `0x400d678a`, `0x400d74ae`, `0x400d7a1e`: the USB/host command decoder region (`0x400d45c4` family); `0x4012c05e` has no caller | 0 |
| 5 | `0x4012c096` (put at `0x4012c0c4`) | called only from `0x400d7b38` (same command-decoder region) | 0 |

- The task is **not stuck**. It was released 112 times: once at about 578.3M (initial pend) and then 111 times from 1.0944B to the end, one every ~3.7M instructions, each time by the type-2 post from the SSI ISR (`D1` counts up in steps of `0x10`). So its only live input is the audio block tick that the diagnostic `SSI_AUDIO_IRQ` produces. The end-of-boot idle wait at `0x4012c5a6` is the normal state between ticks.
- What it emits: a `0x1d` record put (`0x4012d110`) on the queue returned by `0x4012bda0` (`[0x402b4f60]`, default `0x47db3f70`); `0x4012d10c` has 0 hits, so no such record is produced yet. Types 1/4/5 (note, host-command) never arrive: they are triggered by sequencer/pad trigger code or by the USB command decoder, neither of which runs.
- Verdict: audio/voice engine control (note triggers, host commands, audio ticks). Not the panel/UI gate. Its non-running posters are the same unexercised UI-trigger and USB-command callers found earlier.

### `0x40133922` (prio 6): factory-install progress bar, finished by design

- Waits on sem `0x44e460d8` (task create `0x4013388c`, queue/sem init `0x40133862`). That sem is posted only by the **PIT3 ISR** `0x401337f4` (vector 208; registered at `0x401338b0` and enabled by `0x401338c6-0x401338ee`), which sets bit 2 of `0xFC08C000` and posts via `0x4000148c`.
- Gate: it loops on `[0x44e4655c]` (total) until `0x40133822` stores total/current (`0x44e4655c`/`0x44e46560`, callers are the `jmp`s at `0x40032b7a`/`0x40032b8e`), then draws a bar scaled `current*0x27/total` after each tick (`0x80`/`0x40` geometry).
- Runtime: the PIT3 ISR hit 30 times at ~8.8M-instruction spacing, 831M to 1.086B (the whole factory-install progress window). `0x40133902` (stop PIT3: writes `0` to `0xFC08C000`, masks the interrupt, then posts the receiver sem `0x44e460e0` via `0x4000141a`) is reached at instruction **1,091,563,074** (`STOP_PC` run), 15 instructions before the receiver wake already recorded at 1,091,563,089. Its callers are `0x400334a0`, `0x400335b6`, `0x40033618`, all in the receiver's install-complete handler.
- Verdict: its idle wait at `0x401339f0` is expected after the install. Not a UI blocker. `WATCH_WRITE` cannot capture MMIO (`cannot capture WATCH_WRITE initial bytes at 0xfc08c000`), hence the `STOP_PC` approach.

### UART8 RX path (panel input to receiver events): present and working

Chain, with addresses (earlier hits in this file confirm each link):
1. Init `0x4000243e` (called at `0x400cca82`, gated by `0x4029eef0 & 0x40`): UART8 at `0xEC070000`, RX DMA TCD34 (`SADDR 0xEC07000C`, `DADDR` ring `0x4fe3a000`, 1 KiB ring, state `0x40964d74..0x40964db4`), callback pointer stored at `0x40964db4` = `0x4011e648`.
2. RX interrupt/DMA ISR `0x40001f1a` (installed at `0x40002576`): drains the ring (read index `0x40964da4`, mask `0x3ff`, compared with `[0xFC045450]`) and calls `jsr (a0)` with `a0 = [0x40964db4]` once per byte (`0x40001f6a`).
3. `0x4011e648` byte callback, then button decoder `0x4011e318`, encoder decoder `0x4011e598`, then the queue put `0x40001896` into the receiver queue `0x40966f3c`. With the `bc 01` ack at `0x4011e220`.
4. Receiver event loop `0x400337ba` -> `0x40030bd0` -> listener `0x40061940`.

This is exactly the path described in "Panel input on OS 1.17: what we confirmed" (a `2x` packet reaches the queue in ~170 instructions; the handshake bytes `70 0b 00 00 00 20 01 30 01 20 00` reach `0x4011e648`). So no link is missing: with no panel traffic the receiver gets only the SSI diagnostic and one self-post, which is the observed idle. The remaining open items are not input plumbing but UI-start: the 11 callers of the LED setter `0x400308e0` (`0x40016c72`, `0x4005c2e0`, `0x4005d892`, `0x4005d8c0`, `0x40066272`, `0x4007cf76`, `0x4007ef7e`, `0x40081376`, `0x40094ed4`, `0x400a6342`, `0x400fb370`) and the 4 callers of `0x40133e58` (`0x4005c0fc`, `0x4005c14e`, `0x400aacea`, `0x400b9a1c`) all sit in page/mode code that is never entered, plus the missing per-track objects (pointer table `0x44f37bd0`).

### Waiter map after this round

| task | wait | status |
|---|---|---|
| `0x4011f08e` | MIDI IN (`0x44e15bcc`) | host input, idle by design |
| `0x40153f34` | job queue `0x405a0f44` | USB host command path, idle by design |
| `0x401353dc`/`0x40135530`/`0x40135656` | USB packet queues | USB, idle by design |
| `0x4012c590` | `0x44e25220` | alive, audio ticks only; note/host types never posted |
| `0x40133922` | `0x44e460d8` (PIT3) | install progress bar, finished at 1.0916B |
| `0x400337ba` receiver | `0x40966f3c` | alive, no panel traffic |
| `0x400f0df2` / `0x400f0f0a` (`0x450575d4` / `0x450cdfd4`), engine `0x400d1a12` (`0x43149548`) | not traced in this round | still open |

Next candidates: (1) who enters the UI page/mode code that calls the LED setters (it must come from the receiver's post-install state machine or a project-load step, since neither the host paths nor the panel path starts it); (2) the three still-untraced waiters above; (3) feed real panel events (`UART8_RX_PERIODIC`) from the post-install snapshot while tracing the first page-code entry (e.g. the `0x40016c72` caller) to see which input first reaches UI code.

## Remaining waiters: `0x400f0df2`, `0x400f0f0a`, `0x400d1a12`

None of the three is a UI / "first view" task.

- `0x400f0df2` / `0x400f0f0a` are the worker tasks of two background job executors
  (singletons `0x44f37090`, prio 2, getter `0x40198cf2`; `0x44f37158`, prio 3, getter `0x4019e65c`).
  Each runs the worker loop `0x400f0abc` and pends on the executor's `+0x14` semaphore.
  Submit is `0x400f0c1c(executor, "name", fn...)`, with 82 call sites.
  After a job runs, `0x400f09d6` posts a type-6 record onto the receiver queue `0x40966f3c`.
- `0x400d1a12` (prio 7) is a boot-time engine helper. It checks CPU speed, stops PIT3,
  posts `0x43149550`, then pends forever on `0x43149548`. It is not on the UI path.

TRACE_REGS over 1.5B (hit counts):

| PC | hits | notes |
|----|------|-------|
| `0x400f0abc` worker loop | 2 | one per executor, at 592.0M and 1095.97M |
| `0x400f0c1c` submit | 4 | exec 1 at 592.0M (`0x4023b82e`), 592.0M (`0x4023bfcf`), 822.25M (`0x4023afff` "Factory reset"); exec 2 at 1095.66M (`0x4023aaa6` "Load all samples") |
| `0x400f0b3c` run job | 4 | one per submit |
| `0x400f09d6` completion post | 0 | no job completion is ever delivered to the receiver |

Findings:
- The receiver's startup submits "Factory reset" and "Load all samples". "Migrate presets" and
  "Update MMC Caches" were not submitted (conditional).
- "Project Init/Load" (`0x400a4af4`, a vtable method with no direct callers) is never submitted.
  `0x44f37bd0` stays empty.
- Type-6 continuations never reach the receiver, so the receiver cannot advance to the next
  startup step. The next question is why `0x400f09d6` never fires. Either the jobs block
  (for example on the card) or the post-run path is skipped.
- Option 2 (resume with `UART8_RX_PERIODIC` and trace the page code) is not yet done.

## Why job completions never reach the receiver (`0x400f09d6` = 0 hits)

Short answer: it is by design for these jobs, and the jobs are not blocked.

- Worker loop `0x400f0abc`: pend on `+0x14` sem, `jsr (a0)` at `0x400f0b3c` runs the job fn, then `0x400f0b4e` runs the job's `+8` cleanup (called with 3), then
  `0x400f0b6c: tst.l $18(a3)`. The completion post (`0x400f0b8c -> 0x400f09d6`) is only reached when the job record has a continuation at `+0x18`.
  There is no semaphore or return-value gate. The job's return value only goes into `d4`, which is passed to the post.
- Hit counts (`TRACE_REGS`, 1.5B) for `0x400f0b3e` (job fn returned), `0x400f0b4e`, `0x400f0b6c` and `0x400f0b9c` are all 4.
  `0x400f0b9c` is hit 2 instructions after `0x400f0b6c`, so `+0x18` was 0 (the `beq` was taken) every time.

| Job | Executor | Start (instr) | Returned (instr) |
|---|---|---|---|
| `KitActive::updateSingleMirror` (`0x4023b82e`) | 1 | 592.04M | 592.16M |
| `saveProjectToMmc(tempProject)` (`0x4023bfcf`) | 1 | 592.16M | 625.6M |
| "Factory reset" (`0x4023afff`, fn `0x400334bc`) | 1 | 822.3M | 1096.02M (the install itself; ends right after the 1.0916B PIT3 stop) |
| "Load all samples" (`0x4023aaa6`, fn `0x40030ce8`) | 2 | 1095.97M | 1198.8M |

All four finished, so nothing is stuck in the card layer. After their last job, both workers are back on `0x400f0aea`. None of the submitters passed a continuation, so the type-6 post is unused for this boot.

Skipped submissions:
- Receiver startup `0x400337ba` tests `0x4012ede4(..) == 1`. This boot takes the factory-reset branch (`d4 = d5 = 1`), submits only "Factory reset" and then jumps to `0x40033942`.
  "Migrate presets" (`0x400338e2`) and "Update MMC Caches" (`0x40033934`) sit on the other, normal-boot branch (`0x4003388c`), so they are skipped by design. It is the normal-boot branch that is not taken.
- "Project Init/Load" is not part of the receiver startup. Its job fn is `0x400a4af4`, built by the submitter `0x400a3c16`.
  Its only caller is the wrapper `0x401bb78a` (`jsr` at `0x401bb804`). That wrapper has no direct callers; its only reference is a data word at `0x401bbd58`.
  So it is a dispatch-table / command-object entry, invoked on a user or command action (load project). It is not auto-run at boot.

Receiver main loop (after `0x40033cf2`): get from `0x40966f3c` (`0x40001928`), then jump-table dispatch on msg type (0..0x29) at `0x40033d22`.
Handlers: 0 `0x40033d76`, 1 `0x40033da6`, 5 `0x40033df4`, 6 `0x40033e0c` (the job-completion type), 7 `0x40033e7a`, 8 `0x40033e82`, 9 `0x40033eee`, 10 `0x40033f58`, 12 `0x40033fda`, 13 `0x4003419c`, 14 `0x40034206`, 20-23 `0x40034216`, 24 `0x400342e6`, 25 `0x40034338`, 27 `0x40034354`, 29 `0x40034376`, 30 `0x400343ae`, 31 `0x400344a8`, 32 `0x4003450a`, 33 `0x400345ce`, 34 `0x400345e0`, 38 `0x400345ee`, 40 `0x400346bc`, 41 `0x40034662`. All other types loop back.

Conclusion: the receiver is a pure message loop that now idles. Its only inputs come from the type-0 / type-1 and other posters (`0x401538fe`, panel/UART8 decoders, the SSI diagnostic). The next step is to find which message types those handlers expect for "start the UI" and have the UART8 / panel path post them. The candidates to look at first are the type 0 and 1 handlers `0x40033d76` and `0x40033da6`, then the type 13 and 14 handlers.

## Receiver message types: who posts them, and what the type-0 (input) path does

The receiver loop `0x40033d00` reads `[0x40966f3c]` and dispatches on `mvz.b (msg),d0` (the type byte), with `cmp #0x29` and the jump table at `0x40033d22`.

**Types seen in the 1.5B boot** (`TRACE_REGS=0x40033d14`, D0 = type, A2 = msg): type `0x0a` x5 (msg `0x4020e2ab`, the SSI diagnostic tick; handler `0x40033f58`) and type `0x20` x1 (msg `0x405a0f9c`, from the USB job-queue task at ~1.196B). Nothing else is posted.

**Subsystem queue globals.** The receiver startup `0x40033992..0x400339f4` hands the receiver queue to eight subsystems, each of which stores it in a global and posts typed records to it:

| Global | Registrar | Posters (type) | Owner | Runs in boot? |
|---|---|---|---|---|
| `0x402b45a0` | `0x4011e108` | `0x4011e4a4` (type 0, 16-byte ring `0x44dc7e14`) | UART8 byte decoder, front-panel input | yes, but only when UART8 RX bytes arrive |
| `0x402a1c0c` | `0x400d605a` | `0x400d8cfa` (type `0x0a`) | SSI ISR (vector 0x79) | yes (diagnostic) |
| `0x402b6b68` | `0x40153a76` | `0x401538fe` (types `0x1f`/`0x20`, callers `0x401544e8`, `0x40154674`, `0x40154a74`, `0x40154a9e`, `0x40154fb2`) | USB job-queue task | the `0x20` once |
| `0x402b6804` | `0x40139084` | `0x4002e83e` (type `0x1d`, inside the audio-DMA ISR `0x4002dd0c`, ring `0x40966844`); `0x40139e7e` and others call the queue getter `0x40139092` | audio engine | 0 hits |
| `0x402b4f60` | `0x4012bd92` | the getter `0x4012bda0` has one caller, `0x4012d108` (audio-engine task) | audio engine | 0 hits |
| `0x402b4f3c` | `0x40120b46` | the 16 reads are in `0x4011ed12..0x40120994`, the MIDI dispatcher area | MIDI | 0 hits |
| `0x402b6b7c` | `0x4015a6f2` | `0x4015a6aa` (type `0x1e`; callers `0x4015652a`, `0x40156aea`, `0x40156dea`, `0x401587e0`, `0x4015b2ca`) | job/storage area | 0 hits |
| `0x4029ef78` | `0x400058f0` | `0x400055ce` (type `0x26`, callers `0x40005dfa`, `0x40006244`, `0x4000628e`, `0x40006efa`, `0x40006f26`), `0x40005fd0` (`0x26`), `0x40003af4` | USB driver (`0xFC0B0000` OTG) | 0 hits |

So the only type that carries front-panel input is **type 0**. Every other poster is USB, MIDI, audio-engine or the SSI diagnostic, and the earlier `0x40001896` trace showed none of them running.

**Type-0 path with real input.** Resuming `out/blank-factory-1p5b.snap` with `UART8_RX_PERIODIC=3000000:2301,2300` for 300M instructions (flags as in `run-section3.sh`): handler `0x40033d76` is hit 99 times. The type-0 records in the ring at `0x44dc7e14` are `{type 0, +4 = key id (0x01 or 0x19), +8 = state bits (0x01, 0x10, 0x05), +c = 0}`. The handler:
1. builds an event object with `0x40114904(obj, +4, +8, +c, 0x7f)`: `+0xc` is the key id and `+0x10` the state bits;
2. calls `0x40030bd0(ctx = 0x44f5fe40, obj)`, which walks a listener list (`ctx+0x94..+0x98`, 0x10-byte entries; the call is skipped when `+8` is 0) and calls each listener's `+0xc`;
3. calls `0x4019f58a(obj)`.

In the snapshot the list holds one listener at `0x450cb250`, whose callback is `0x40061940` (`+8 = 0x40061e2e`). The callback dispatches only on key ids `0x0a`, `0x0c` and `0x12` (and later ones I have not read), with a pressed-state test `(state & 1) ^ 1`, then jumps to `0x4005af44` or `0x4005af84`. Our synthetic TRIG key ids (`0x01`, `0x19`) fall through. The LED setter `0x400308e0` still has 0 hits in this 300M window.

**Conclusion.**
- The receiver, the UART8 decoder and the listener chain are all alive and receive input events. Nothing is missing upstream: the UI is not stuck on a missing message.
- The remaining gate is the key code. The listeners react to specific key ids, so the next experiment is to send `UART8_RX_PERIODIC` bytes that decode to key id `0x0a`, `0x0c` or `0x12` (map byte to key id in the decoder `0x4011e318`) and trace `0x4005af44` / `0x4005af84` and the 11 callers of `0x400308e0`.
- Note: the older "natural chain" update (see "SSI marker" above) reported `0x44f37bd0` populated in a from-reset run. The factory-install run keeps it zero. That difference (factory-install vs. normal-boot branch of `0x400337ba`) is not explained yet.

## UART8 key frames, key ids, and the UI view objects (trace only)

**UART8 front-panel frame format** (callback `0x4011e648`, decoders `0x4011e318` / `0x4011e598`):
- The first byte is a header. Its high nibble is the type and its low nibble is the row index (stored in `0x402b4588`).
- A type-`0x7` header starts an 8-byte block. Type `0x3` goes to `0x4011e598`. Type `0x2` goes to the button-matrix decoder `0x4011e318`.
- For type `0x2` the next byte is the 8-bit column mask of that row.
- Key id = `row*8 + col + 1`, from the table `0x4020b150` (6 rows x 8 = ids `0x01..0x30`). So `2301` = id `0x19` (TRIG-area key). `2102` = `0x0a`, `2108` = `0x0c`, `2202` = `0x12`, `2201` = `0x11`.
- The decoder debounces, then posts a type-0 record `{+4 = key id, +8 = state, +c = timestamp}` onto the receiver queue (ring `0x44dc7e14`).
- `0x4011e150(id)` is the key-state query: it reads the held-key bitmap at `0x44dc8644`.

**Runs** (resume `out/blank-factory-1p5b.snap`, 60-80M instructions, `UART8_RX_PERIODIC=3000000:<frames>`):

| Frames | Key | Result |
|---|---|---|
| `2102,2100` | `0x0a` | `0x4005af44` hit 26 times. It stores a held flag at `obj+0x2bd`. |
| `2108,2100` | `0x0c` | `0x4005af84` hit 8 times. It stores a held flag at `obj+0x2be`. |
| `2202,2200`, `2201,2200` | `0x12`, `0x11` | no hit on `0x4005af44`/`0x4005af84`, none on `0x40065f08`/`0x40066058`/`0x400308e0` |

- The listener callback `0x40061940` only tracks the modifier keys `0x0a`, `0x0c` and `0x12` (it sets held flags). It does not start any view or page.
- `0x400308e0` (LED setter), `0x40133e58` and the page-code caller `0x40016c72` all have 0 hits with any of these keys.

**View objects.**
- `0x40016c2a` (holds the call at `0x40016c72`) is a vtable method of `SamplerLedView` (vtable `0x401ed94c`..`0x401ed99c`, slot at `0x401ed990`). The class is constructed at `0x400169e0` (called from `0x401afd28`, in a shared-ptr style factory `0x401afcee` that is called from `0x4006690c`).
- `0x4006690c` is inside the `MainScreenView` virtual method `0x40065f08` (vtable `0x401f7ab0`, slot at `0x401f7b04`, the next slot `0x40066058`). The class is constructed at `0x40066584` (name string `0x4023d238`), from the lazy singleton getter `0x4019ae1c` (global `0x44f370f0`).
- That singleton exists in the snapshot: `[0x44f370f0] = 0x45057c40`. So `MainScreenView` is built, and the receiver loop calls the getter (`0x40033cf2`).
- `0x40065f08` tests key `0x11` (`pea 0x11; jsr 0x4011e150`) and creates the sub-views (`SamplerLedView` and others, stored at `+0xe4`, `+0xec`, and so on). Neither `0x40065f08` nor `0x40066058` is hit in any run, so nothing calls these virtual methods yet.
- Correction: `0x44f37bd0` is not zero. It holds sixteen copies of one default pointer `0x41776cb8`, both in the 1.5B snapshot and in a from-reset 900M run. It is an "all slots unpopulated" table, not an absent one.

**Open.** What dispatches `MainScreenView` (the vtable `0x401f7ab0` slots `0x54` / `0x58`)? A static scan for virtual calls at `+0x54` / `+0x58` finds about 180 each, so it is too noisy to read directly. The next step is `TRACE_REGS` at the receiver handlers that reach the main view (type-`0x0a` handler `0x40033f58` calls `0x4004af1a` / `0x40043480`; types `0x0d` / `0x0e` call `0x40031ec2`), and a watch on `[0x45057c40+0x68]`, to find the caller that passes this object.
