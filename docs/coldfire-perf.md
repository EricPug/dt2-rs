# ColdFire core performance: design note

Status: **proposal, no implementation yet.** Written for review before any code.

Goal: take the interpreter from ~12 M instr/s to roughly 100 M instr/s (about the speed of the real
133 MHz part), without changing emulated behavior. A UI frame is ~3 M instructions, so 12 M/s is ~4 fps and
roughly 54 ms of key-to-screen latency per encoder message. Audio work will need real-time speed.

## 1. Where the time goes

Workload: panel snapshot (`out/panel-1p5b.snap`), `DTIM3_TICKS_PER_INSTRUCTION=16`, `SSI_AUDIO_IRQ=100000`,
encoder message every 8 M instructions, 40 M instructions. Baseline 12.2 M instr/s, i.e. **~82 ns (~250 host cycles)
per guest instruction**. A decent cached-decode interpreter spends 15-30 cycles, so there is a lot of headroom.

### Guest side (instrumented scratch build, 40 M instructions)

- 12,565 distinct PCs executed. The top 100 PCs cover 66% of executions, the top 1,000 cover 91%, the top 5,000 cover 99%.
- Executed code footprint is 536 pages of 256 B (134 KiB). A decode cache is tiny.
- 2,099 distinct 16-bit opcodes. Families: `2xxx` MOVE.L 24%, `6xxx` Bcc/BRA 24%, `4xxx` misc (LEA/JSR/RTS/TST/PEA/...) 18%,
  `5xxx` ADDQ/SUBQ/Scc 7%, `7xxx` MOVEQ 7%, everything else under 5% each.
- `60fe` (`bra.b self`, the idle loop) is **13.2%** of all instructions. `4e75` RTS 3.9%, `588f` ADDQ #4,A7 2.7%, `2f0a` MOVE.L A2,-(A7) 2.6%.
- **Writes into already-executed code pages: 0** in steady state. (Boot-time decompression and eMMC DMA do write code, so invalidation is still required.)
- Only ~2.7% of instructions take the non-fast-RAM bus path (SRAM overlay at `0x8000xxxx`).

### Host side (gdb stack sampling, 268 usable samples, so about ±3 points)

`perf` is not permitted here (`perf_event_paranoid=4`), so this is poor-man's sampling attributed to the owning function.

| Bucket | Share |
|---|---|
| Decode + execute (`step_with_loop_budget` 33%, `ea` 6%, `read`/`cond`/flags/EMAC ~4%) | ~43% |
| Instruction fetch + RAM access through `Bus` (`read16`, `fetch16`, `in_fast_ram`, `write32`, `bus.rs` helpers) | ~15% |
| Interrupt poll (`pending_interrupt` 6%) + timer `advance_instruction` (3%) + INTC (1%) | ~10% |
| Slow-path bus (`read8_inner`, `write8_inner`, `source_for_byte`, DDR/eMMC models) | ~10% |
| Run loop (`run_with_interrupt` 5%) + per-instruction hooks (`begin_instruction` 2%, `report_branch` 2%) | ~9% |
| Other | ~13% |

### Findings that are confirmed, corrected or ruled out

- Earlier claim "skipping interrupt polling tops out at ~13.5 M": **confirmed for the old code** (poll every 16th instruction: 10.3 → 13.5 M).
  After the early-out (b86c3d7) polling plus timers is ~10% of the profile, consistent with a ~10% ceiling from that line of attack.
- A "disable polling entirely" ablation is **invalid**: with no ticks the firmware sits in the idle loop and the executed work changes (it showed 22 M, which is meaningless).
  I did not use it. The only valid ablations leave behavior byte-identical.
- Direct SRAM-overlay path: byte-identical UART8 stream, **no measurable speedup**. Ruled out.
- Fat LTO, `panic=abort`, `target-cpu=native`: **no gain**. Ruled out.
- Nothing is dominated by one function. Cost is spread across decode, fetch and per-instruction bookkeeping, which is why micro-tuning has run out.

## 2. Options

| Option | Expected gain (rough) | Cost | Risk |
|---|---|---|---|
| **A. Small wins**: skip the idle `bra.b self` to the next event, event-driven interrupt polling, one `hooks_active` branch instead of several, trim fetch bookkeeping | 1.2-1.4x | Days | Low |
| **B. Pre-decoded instruction cache**: decode once per PC into a compact `Inst`, run `execute(Inst)` | ~2x on top of A (decode+fetch ≈ 55% of time) | Large refactor of `step`, ~2.9 k lines touched | Medium. Semantics must stay identical |
| **C. Block cache / threaded dispatch**: run chains of `Inst` up to the next event horizon, batch timer/count accounting, one loop per block | ~2x on top of B (per-instruction bookkeeping removed) | Medium, builds on B | Medium. Exact interrupt and count boundaries |
| **D. Lightweight JIT / IR lowering** (cranelift or dynasm to x86-64) | 10-30x | Very large: new dependency, `unsafe`, flags, faults mid-block, debuggability | High |
| **E. Execute-side tuning** after B/C: direct RAM pointer instead of `dyn Bus`, lazy CCR, superinstructions for the top sequences (`move.l an,-(a7)`, `addq #4,a7`, `jsr`/`rts`) | ~1.3-1.5x | Small each | Low |

**Recommendation: A, then B, then C, then E. Defer D behind a measured gate.**

- Why B+C and not D first: 250 host cycles per guest instruction is overhead, not an inherent limit. Cached-decode block interpreters
  typically reach 15-30 cycles per simple instruction, i.e. 100 M+ instr/s on this host. Combined estimate: 12 × 1.3 × 2 × 2 × 1.4 ≈ 85-100 M/s. These factors are
  estimates and may fall short; each slice is measured before the next starts.
- Why not D now: the guest is 32-bit ColdFire with MMIO side effects on a large fraction of stores, exact-instruction-count timing (UART `.times`,
  timers, periodic injection), and a firmware we are still reverse engineering. A JIT would need all of that plus precise-exception recovery before it is useful,
  and would be much harder to keep byte-identical. B+C produce the `Inst`/block IR a JIT would consume, so D stays possible later at no extra cost.
- Why not "only A": it tops out near 15-17 M/s and does not touch the 43% decode+execute and 15% fetch buckets.
- Why not "only B": decode is only part of the cost. Without C, per-instruction hooks, the timer lag counter and the poll still run 40 M times.
- Why not hooking firmware loops natively: the hot code is generic C++ virtual-call walking (16 parameter objects × `jsr (a0)` through `$28(vtable)`),
  which has no tight loop to replace.

## 3. Design for B + C

### Ownership and layout (kept in `dt2-core`, localized)

```
dt2-core/src/
  coldfire.rs     Cpu, run loop                     (existing, shrinks)
  coldfire/decode.rs   decode(op, fetch) -> Inst    (new: the only place opcode bits are interpreted)
  coldfire/exec.rs     execute(&mut Cpu, &Inst)     (new: the only place semantics live)
  coldfire/icache.rs   DecodeCache, Block, CodeMap  (new: cache + invalidation)
```

- `Inst` is a small `Copy` struct, about 16 bytes: an `Op` enum, `len` (halfwords), operand descriptors resolved at decode time
  (register numbers, `Size`, EA kind, immediate or displacement, branch target), and the PC.
- **One semantics implementation.** `step` today decodes and executes in the same match arm. Each arm is split into a decode half and an execute half.
  The legacy `step` becomes `decode` then `execute` (cache miss path), so there is no second interpreter. The cache only memoizes `decode`.
- **Cache**: a page table keyed by `(pc - 0x4000_0000) >> 8` (RAM window; 1 M page slots, allocated lazily), each page holding one decoded slot per halfword (128 × `Inst`).
  Expected footprint is about 1 MB for the 536 hot pages. PCs outside the RAM window (SRAM overlay, ROM alias) are not cached at first and use decode-per-step.
- **Blocks (slice 4)**: a `Block` is a run of `Inst` from an entry PC to the first control-flow, MMIO-touching, or privileged instruction, or page end. Blocks are
  built from cached `Inst`s and held in the same page structure; they are not a separate decoder.

### Invalidation

- **Code map.** `CodeMap`, a 1-bit-per-256B-page bitmap owned by the **bus** (`ProbeBus`), because the bus sees every write. The CPU marks a page when it first decodes from it
  (`bus.mark_code_page(page)`).
- **Write choke point.** All RAM writes go through six sites in `main.rs` (`self.ram.write*`/`load` at the fast paths, `write*_inner`, `load_ram`, snapshot restore). Each calls one
  `code_map.on_write(addr, len)`; if the page is marked it clears the bit and pushes the page on a `dirty_pages` list and sets a single `code_dirty` flag.
  Measured cost: zero hits in steady state, so the hot-path cost is one bitmap load and branch per store.
- **Consumers.** The CPU checks `bus.code_dirty()` at block boundaries (and once per step in single-step mode), drains the list and drops the pages and any block overlapping them.
- **Self-modifying code inside a block.** After any store that sets `code_dirty` the block ends immediately and re-enters via lookup, so the next instruction is always fetched from current memory.
  This keeps today's "memory is coherent" behavior; ColdFire's CPUSHL/CACR handling is unchanged (it stays a no-op).
- **Other writers.** eDMA, eSDHC DMA, snapshot load and `RAM_LOAD_FILE` already go through those same six sites. If a new writer is ever added it must call `on_write`; a debug assertion
  compares the cache against memory (see gates) to catch a missed site.
- **Aliasing and remaps.** `RAM_REMAP`, `RAM_SNAPSHOT`, `WATCH_WRITE` and `LOOP_DETECT` already disable the fast RAM path; they also disable the cache (same `enable_fast_ram` reason list).

### Interaction with the existing interpreter and fallback

- `Cpu::step` remains and means decode-through-cache then execute. A cache miss or uncacheable PC decodes directly. Runtime switch `DECODE_CACHE=off` forces the uncached path until the last slice is signed off,
  then the switch and its branch are removed.
- Diagnostic modes (`instruction_hooks`, `TRACE_REGS`, `stop_pc`, `LOOP_DETECT`, loop fast-forward) run with the block size forced to 1, using the same `execute`.
  They are not separate implementations, only a smaller event horizon.
- Bus access, `report_branch`/`report_tst`/`report_rte` and trap/interrupt entry stay in `execute` unchanged, so their order and arguments are preserved.

### Event horizon for blocks (what makes C exact)

A block may run only while no externally visible event can occur inside it. Before entering a block the driver computes `horizon = min(` instructions until: next timer deadline (PIT, DTIM1, DTIM3 already
keep `deadline`), next `UART8_RX_PERIODIC` firing, next SSI/eDMA rearm, `instruction_limit`, `stop_pc`. If `block.len <= horizon` the whole block runs with the counter advanced once at the end; otherwise the driver single-steps
(identical to today). The interrupt poll runs only when a device asserts a line, when INTC or SR mask changes, or at the horizon. These are tracked with a `irq_dirty` flag set by MMIO writes, `rte`,
`move to SR` and timer events. Any instruction that can change one of those (MMIO store, SR write, RTE, TRAP) terminates the block.

### What the byte-identical regression requires the cache to preserve

1. **Instruction count**: `current_instruction_count` increments once per retired instruction and advances by the exact skipped count for fast-forwarded loops. UART8 `.times` stores absolute counts per byte.
2. **Interrupt boundaries**: an interrupt is taken before the same instruction as today (poll point, SR mask at that time, priority tie-break: highest level, then INTC0 > INTC1 > INTC2).
3. **Data access sequence**: same order, widths and addresses for every load/store; MMIO side effects (UART TX, eDMA kick, DTIM writes) happen at the same instruction count.
4. **Fault behavior**: `Illegal`, `Privilege`, bus errors return the same `CpuError` (pc, op) and leave the same partial state (e.g. pre-decrement already applied). Illegal opcodes are decoded as `Inst::Illegal` and raised at execute.
5. **Flags and registers** after every instruction (CCR from `set_ccr`, X flag handling, EMAC state, A7 byte adjust).
6. **Timer accounting**: PIT/DTIM lag counters are synced at the same logical moments, so `DTCN` and `DTER` read the same value.

## 4. Incremental plan, each slice gated

### Gate (built first, slice 0)

`tools/perf-gate.sh`, run after every slice:

1. UART8 TX stream and `.times` for the encoder-traffic run: sha1 must equal `7662453ca89250adcde7065dd4285581c1e6cf95` / `ba54cae43705472fbbc046c643a96ce4e1f10f8b` (40 M instr).
2. New `STATE_HASH` diagnostic: hash of D0-7, A0-7, PC, SR, EMAC, the DTIM/PIT state and all 256 MiB of RAM + SRAM at 1 M, 8 M, 20 M and 40 M instructions. This is far stricter than the TX stream.
3. `cargo test --release` (228 core tests, 74 CLI tests).
4. Differential test (new, slice 2): for every opcode seen in the 40 M run plus a generated sweep of each family, execute legacy-decode and cached-decode on identical state and compare registers,
   memory writes and errors. In debug builds the cache also re-decodes on every hit and asserts equality with the cached `Inst`.
5. Throughput on the same run (median of 3), reported in the commit message and the handoff log.
6. Final slice only: replay boot from the pre-entry image to the `panel-1p5b.snap` point and compare the snapshot bytes.

### Slices

| # | Slice | What changes | Est. instr/s | Cumulative |
|---|---|---|---|---|
| 0 | Gate + `STATE_HASH` + this baseline | tooling only | 12 M | 1.0x |
| 1 | Idle `bra.b self` skip to next event; event-driven interrupt poll; one `hooks_active` branch | `coldfire.rs` run loop, `main.rs` poll | 15-17 M | 1.3x |
| 2 | Split `step` arm-by-arm into `decode` + `execute` (no cache). Highest-frequency families first (`2xxx`, `6xxx`, `4xxx`, `5xxx`, `7xxx` = 80% of executions) | new `decode.rs`/`exec.rs` | ~same (±5%) | 1.3x |
| 3 | `DecodeCache` + `CodeMap` invalidation, single-step driver | `icache.rs`, six bus write sites | 28-35 M | 2.5x |
| 4 | Block driver with event horizon, batched timer/count accounting | run loop | 50-65 M | 4-5x |
| 5 | Execute-side tuning (direct RAM pointer, lazy CCR, top-3 superinstructions) | `exec.rs` | 70-100 M | 6-8x |
| 6 | Decision gate: if still <10x, profile and consider D (JIT) using `Inst`/blocks as the IR | - | - | - |

Estimates are rough; the whole plan assumes the guest instruction mix measured above. If slice 3 lands under 20 M the plan is revisited before slice 4.

Keeping the old path alive: through slice 3 `DECODE_CACHE=off` runs decode-per-step with the same `execute`. Slice 2 itself is a pure refactor, so it is validated against the old binary (same gate) before any
cache exists. The old `step` is deleted only when slice 5 passes the gate, in a separate commit.

## 5. Tech-debt controls

Will not do:

- No second interpreter. There is exactly one `execute`; the legacy `step` becomes decode+execute. No `Inst::Legacy` escape hatch survives a slice: any not-yet-ported opcode is a compile or test failure, not a silent fallback.
- No per-opcode special cases in the driver or bus. Event logic lives in one `horizon()` function.
- No cache invalidation scattered in devices: only the six existing RAM write sites call `CodeMap::on_write`.
- No `unsafe` outside one small, commented RAM-pointer accessor (slice 5), and none for the cache.
- No timing hacks. Anything that would change instruction counts or interrupt boundaries is rejected by the gate rather than adjusted.
- No JIT or new dependencies until the slice 6 gate.

End-state structure for a later reader:

- `decode.rs`: opcode bits → `Inst`. Pure function, one table per family, unit-tested against the spec.
- `exec.rs`: `Inst` → effect on `Cpu` and `Bus`. No opcode bit twiddling.
- `icache.rs`: `DecodeCache`, `Block`, `CodeMap`. Invalidation rules and event horizon documented in the module header.
- `coldfire.rs`: registers, exceptions, and a short run loop (`horizon` → run block or single-step → poll if `irq_dirty`).
- The diagnostic features sit behind the same `execute`, selected by the horizon, not by separate paths.

## Status log

### Slice 0 — gate tooling (done)
- `STATE_HASH=n1,n2,...` (dt2-cli) prints an FNV-1a64 hash of the full snapshot bytes (CPU, RAM, devices) at each instruction count.
- `tools/perf-gate.sh [binary]` checks the TX stream and `.times` sha1s plus state hashes at 1M/8M/20M/40M against golden values, and prints throughput. Needs `out/panel-1p5b.snap`.
- Baseline (`b86c3d7` + STATE_HASH): GATE PASS, 12.1M instr/s. Hash runs are deterministic across runs and chunking does not change the TX stream.

### Slice 1 — idle `bra self` skip (done; under estimate)
- `Bus::idle_skip(max)`: when the previous instruction was a taken `bra` to itself, the run loop asks the bus to retire up to `max` iterations at once. `ProbeBus` caps it by the headroom to the next PIT/DTIM1/DTIM3 event (`idle_headroom`/`skip_idle`), the next `UART8_RX_PERIODIC` firing, the SSI re-arm, the injection point and the instruction limit, and refuses when instruction hooks, call dumps or loop detection are on. Counted as fast-forwarded instructions.
- Gate: PASS (TX, `.times`, all four state hashes identical). Tests green.
- Result: 5.27M of 40M instructions skipped; wall time 3.30 s → 3.15 s (~12.7M instr/s effective, vs 15–17M estimated). Idle instructions were already the cheapest ones, so the win is small.
- Dropped from slice 1: event-driven interrupt poll (<=10% ceiling, many assertion sources, high risk for the gain) and the `hooks_active` collapse (the per-instruction hook calls are already a few loads). The real gain is in slices 2–4 (decode/block cache); proceeding.

### Checkpoint — stop and report before slice 2
Slice 1 landed under estimate (12.7M effective vs 15–17M), and the same reasoning puts the later estimates in doubt. From the host-side sampling in this note: decode+execute ≈43%, fetch+RAM ≈15%, interrupt poll/timers ≈10%, slow-path bus ≈10%, run loop/hooks ≈9%. A decode cache removes mostly the fetch share and part of decode; a block driver removes most of the poll/hook/run-loop share. Even if both were free that leaves the ≈43% execute core plus slow-path bus, i.e. roughly 2× (~25M), not 50–65M. Reaching 50M+ needs the execute work itself to shrink (a JIT/IR or closure-compiled blocks), which is the slice 6 decision gate pulled forward. Slice 2 (splitting the ~800-line `step` into `Inst` decode + exec) is also the largest and riskiest piece, so it should be decided on purpose, not by momentum.

### Slices 2–3 — instruction-word cache (done; measured gain ≈ 0)
- `core/src/icache.rs`: `ICache` (64K-entry direct-mapped, entries tagged with an epoch) and `CodeMap` (256-byte page bitmap + epoch). `Bus::code_epoch`/`Bus::cacheable_code` let the RAM owner promise to bump the epoch before cached code changes. `FlatRam` and `ProbeBus` (fast RAM only, all six RAM-write sites) implement it.
- `Cpu::step` is split into `step_cached` (record or replay the instruction's words) and `execute(bus, pc0, op, ..)`. Extension-word fetches replay from the entry; `DECODE_CACHE=off` keeps the plain bus-fetch path as the reference.
- Pages are marked *before* the instruction executes and an entry is stored only if the epoch is unchanged afterwards, so self-modifying instructions are never cached stale.
- Gate: PASS with cache on and with `DECODE_CACHE=off`. Unit tests added: cached vs uncached equality, and invalidation after a code rewrite. On the gate workload: 34.68M hits, 51K misses, 0 flushes.
- **Result: ~+3% (10.6 → 11.0M).** Far under the 28–35M estimate. Fetching was never the bottleneck (a micro-benchmark on `FlatRam`, `crates/dt2-core/examples/bench.rs`, runs ~34M instr/s with or without the cache). The estimate assumed decode cost sat in fetch; it sits in `execute`, the bus run loop and the slow bus path.
- The cache stays: it is the invalidation foundation any pre-decoded tier needs.

### Re-measurement (steady-state gdb sampling, 150 samples)
- Slow-path bus accesses: ~36–39% of samples. 3.9M slow accesses in 35M instructions, all at `0x8000xxxx` (SRAM overlay: stack/data). Each took the whole device-probe chain (`source_for_access` ×4 bytes, `read8_inner`/`write8_inner` ~20 checks, BTreeMap lookups).
- Per-instruction run loop (poll, hooks, timer lag, `begin_instruction`): ~30%.
- `execute` + `ea`/`read`/`write`: ~29%.
- (The earlier "direct SRAM path gives no speedup" note was wrong; that experiment must have left part of the chain in place.)

### Slice 3b — direct SRAM-overlay path (done)
- `ProbeBus::in_fast_sram`: accesses in `0x8000_0000..0x8001_0000` go straight to `Mcf5441xSram` when the same conditions that enable fast RAM hold and nothing (seeded MMIO, eDMA TCD bytes, RGPIO) lives in that window.
- Gate PASS. **11.0 → 14.7M instr/s.**

### Slice 4 status: event-driven interrupt poll

- `Bus::poll_due()` (default `true`); `ProbeBus` tracks `irq_dirty` + `poll_countdown` (min of timer `instructions_until_event()` and SSI re-arm). Dirty is set on slow-path bus accesses, `interrupt_taken`, `set_rgpiobar`, UART8 RX injection. The CPU forces a poll after taking an interrupt or changing SR mask bits. Disabled when `pit0_oneshot` is on.
- Result: 14.7M -> 18.5M instr/s (cache on), 16.0M with `DECODE_CACHE=off`. Gate (TX, times, state hashes) passes in both modes; all tests pass.
- Next: execute-side specialization (pre-decoded hot-family variants with a generic fallback to the reference `execute`).

## Design note 2: execute-side tier (after slice 4)

Inputs: re-profile (execute+ea/read/write ~54%, run loop ~34% before slice 4), `docs/coldfire-perf-next.md`, and the digiemu project (patched Unicorn/TCG).

Findings that shape the plan:
- The decode cache gave +3%: fetch was never the bottleneck. `execute` costs ~28 ns/instr on `FlatRam` because of its size and `Result<Operand,_>` plumbing, not dispatch.
- RAM fast path and SMC detection are already done (SRAM overlay fast path, `CodeMap` page bitmap, 0 flushes).
- digiemu reaches ~2.8x real time on Unicorn only after fixing memory-hook slow paths, exit checks and MAC helpers. Its lazy-CCR bugs (flags committed stale at hook stops) are the risk to gate against for any lazy-flags work here. EMAC with load at 0x400db9e0 is a decoder test vector.
- Events split blocks only at real deadlines (same as `poll_countdown`).

Plan (each step measured, gates green, reference `execute` kept as fallback):
1. Hot-family handlers selected through an opcode-indexed table, operands pre-extracted; anything not in the table calls the reference `execute`. Per-opcode differential test against the reference over randomized register/flag state.
2. Lazy condition codes only if flags show up in the profile after step 1.
3. Block cache up to the event horizon with threaded dispatch over the same handlers.
4. Batch timer lag; trim remaining run-loop cost.
5. Evaluate Unicorn (patched) as differential oracle first, as an engine tier only if the interpreter stalls. Needs project sign-off on the patched C dependency.

### Slice 5 status: inline helpers + batched timer lag

- Finding: the core on `FlatRam` was held back by non-inlined `read`/`write`/`ea`/`set_ccr`/`flags_move` (`Result<Operand,_>` plumbing); `#[inline(always)]` on those took `examples/bench` from 33 to 66M instr/s. Inlining `add`/`sub`/`fetch16/32` as well gave nothing.
- Firmware workload: 18.5 -> 21.3M instr/s from the inlining alone (the bus and per-instruction hooks dominate there).
- Batched timer lag: `ProbeBus.timer_pending` counts retirements; `flush_timers()` adds them to PIT/DTIM1/DTIM3 (`advance_instructions(n)`) before anything can observe timers: slow-path bus accesses, `pending_interrupt`, `idle_skip`, `interrupt_taken`, `save_snapshot`, and `Bus::end_run`. Only active with `lazy_poll`. 21.3 -> 27.0M instr/s (cache off: 23.6M).
- Gates: TX, times, all four state hashes identical, cache on and off; all tests pass.
- Ablation note: stubbing hooks changes the executed workload, so use only the byte-identical gate for measurements.
- Next: remaining per-instruction hooks (`begin_instruction`, `report_instruction_state`, `report_branch/tst` Option writes), then opcode-table handlers / block loop.

### Slice 6 status: opcode sweep tool, negative results

- Added `crates/dt2-core/examples/opsweep.rs`: executes all 65536 opcodes from 6 seeded states (user and supervisor, random flags/registers/extension words) and prints one FNV-1a64 hash of every outcome. Baseline at HEAD before this slice: `opsweep d6886cdab29e6c3f` (~30 s). This is the per-instruction differential gate any change to `execute` or any new handler tier must keep identical.
- Tried and reverted (no measurable gain, gate green): hoisting the hot group-4 arms (RTS/NOP/JSR/JMP/LEA/TST/CLR) to the head of the first-match chain (+1%, hash unchanged); fat LTO + `panic=abort` (-6%); `target-cpu=native` (-8%).
- Profile now: `execute` ~33%, run-loop wrapper (hooks, cache lookup, Result plumbing) ~30%, bus accesses ~15%. Effective cost is ~36 ns per executed instruction against 15 ns on `FlatRam`, so the ~20 ns gap is the `ProbeBus` path plus hooks.
- Status: 12.1 -> ~27M instr/s on the gate workload (5.3M of the 40M are idle-skipped). Target of 50-65M not yet met; the remaining cheap wins are exhausted. Next step needs a structural change (handler tier / quiet batch loop with a single bus fast path), gated by the sweep hash.

### Slice 7 status: quiet batch loop

- Ablation first: stripping `begin_instruction`/`report_instruction_state` to their bare stores was worth ~12% (26.3 -> 29.5M, same workload without `UART8_RX_PERIODIC`), so the ceiling for this slice was ~12%.
- `Bus::quiet_horizon/quiet_step/quiet_retire` (defaults: horizon 0 = off) and `Cpu::run_quiet`: with the decode cache on and a bus that allows it, the CPU runs a batch calling only the minimal bookkeeping. The batch ends on `poll_due()`, an interrupt-mask change, a branch-to-self, an error, the instruction limit or a not-yet-due injection. `ProbeBus` allows it only with `lazy_poll`, no instruction hooks, no register capture, no loop detection, no stop PC, no pending `irq_dirty`; the horizon is also bounded by the next `UART8_RX_PERIODIC` injection.
- Result: ~27 -> 29.3-30.8M instr/s. Gate green with the cache on and off (off = old path, 22.7M); `opsweep` hash unchanged; new core test `quiet_batch_matches_the_normal_loop`.
- Remaining budget on the gate workload (est.): execute ~35%, bus ~20%, icache lookup + dispatch ~15%, hooks inside `execute` (`report_branch`/`report_tst` Option writes) ~5%.
- Next: pre-decoded handler tier (hot families with `opsweep` as differential gate) or the patched-Unicorn evaluation; neither is a small change, so this is the point to re-decide.

### Slice 8 status: handler tier (MOVE.L), second family neutral

- `ICache` entries carry a handler id classified once at insert; `step_cached` dispatches on a hit. Family 1 = MOVE.L/MOVEA.L (source modes 0-5, destination modes 0-5), mirroring `execute` step for step. Result: ~30 -> ~33.2M (+10%), clear gain, so the tier continued.
- Gate: `tools/opsweep-gate.sh`. The sweep now keeps persistent RAM/CPU per seed; the cached (second, hit) run must hash identically to an uncached double run (`8d3864fdf474ed28`). Found and fixed a harness bug (restored RAM lost the code-page mark, replaying stale entries). Known artifact: MAC opcodes A1C0/A3C0/A5C0/A7C0 differ single vs double run (accumulator persists); cached == double run, so not a handler issue.
- Family 2 (ADDQ/SUBQ.L, MOVEQ, Bcc/BRA) passed both gates but measured ~32.6M vs ~33.2M: no gain. Reverted per the stop rule; not kept as dead complexity.
- Conclusion: handler tier tops out at ~33M on this workload. Short of 50M by a wide margin; remaining cost is bus/ProbeBus and dispatch, which per-opcode handlers do not remove. Next: patched-Unicorn evaluation (oracle first, then execution tier); pure-Rust interpreter stays as fallback.

### Slice 9 status: patched-Unicorn oracle (first result), ADDI.L bug fixed

- Built digiemu's patched Unicorn 2.1.4 (m68k only, all six patches, pinned commit 8028ec4) in /tmp, outside the repo. `tools/unicorn-oracle/oracle.c` loads the snapshot's DDR/SRAM (dumped with `DUMP_RAM_FILE` in 64 MB chunks) and registers into Unicorn (CFV4E model; SR must be written before A7 or Unicorn bank-swaps the stack pointer), runs N instructions and prints registers. `tools/unicorn-oracle/bisect.py` bisects the first N where dt2 and Unicorn register state differ. MMIO is a dummy zero-returning region, so the comparison is valid only until the first device read.
- Result: bit-exact on PC/SR/D0-7/A0-7 for 4561 instructions, then SR differed: `ADDI.L #imm,Dn` (0x0680) cleared X where the ColdFire PRM (and Unicorn) set X = C. Real dt2 bug (SUBI.L already set X). Fixed in `execute`; perf gate unchanged (state hashes identical, this workload never consumes that X), opsweep baseline moves to `3522f4128fba249a` (cached == uncached).
- Next divergence at N=6594 is `A980` (EMAC accumulator read): harness artifact, ACC/MACSR/MASK are not loaded into Unicorn yet.
- Raw Unicorn speed, RAM-only run (the dummy-MMIO code ends in an idle loop, so this is an upper bound, not the workload): ~140-200M instr/s, i.e. 4-6x the current 33M. A real execution tier also needs MMIO callbacks into ProbeBus, timer/interrupt delivery at event boundaries, snapshot of Unicorn state and the patched C dependency in the build; the pure-Rust interpreter would remain the fallback.

## Slice 10: Unicorn execution-tier spike (feature `unicorn`, `DT2_UNICORN=1`)

Build: `UNICORN_LIB_DIR=/tmp/uc/build cargo build --release --features window,unicorn`
(digiemu's patched Unicorn 2.1.4, m68k only). Off by default; the Rust interpreter
is untouched and remains the fallback (perf and opsweep gates still PASS without the feature).

Design (`crates/dt2-cli/src/unicorn_tier.rs`):
- Host RAM/SRAM mapped by pointer (`Bus::host_memory`); every other range is an MMIO
  region whose callbacks call `ProbeBus` read/write.
- Slices run to `min(limit, quiet_horizon, injection, poll countdown, max_slice)`.
- A slow-path access that dirties the IRQ state makes the engine stop at the next
  instruction boundary (writes `emu_counter` into `emu_count`; offset discovered at
  startup by a probe loop). `uc_emu_stop` from a callback is not safe.
- TRAP and opcodes Unicorn rejects fall back to one Rust `cpu.step()`.
- EMAC state is injected via stub code (Rust -> Unicorn only).
- The Rust icache is disabled in this mode.

Results (40M gate workload, slice 4096): ~87M instr/s, 0.46 s wall vs 1.06 s (~2.3x
wall-clock; the Rust run idle-skips 5.3M instructions, Unicorn executes them).
Interrupt histogram nearly equals Rust (155: 404 vs 405, 170: 398 vs 399, 205: 167 vs 185).
TX is 1283 bytes vs 1284: NOT byte-identical; state hashes will not match.

Known problems:
- Slices >= 65536 stall the firmware (vector 99 delivered once, TX empty); cause unknown.
  Cap is 4096 by default. Making RTE a host step (to poll after mask change) was worse
  (61M, more lost vectors, stall persists), so native RTE is kept.
- Event ordering differs slightly (vector 205).

Needed for byte-identical gates: Unicorn -> Rust EMAC/MACSR extraction, exact idle-skip
and time accounting, and exact event ordering around RTE/mask changes. Other caveats:
patched-C dependency and build story, `emu_count` offset fragility, host writes to code
pages (stale TB cache), snapshot of Unicorn state.
