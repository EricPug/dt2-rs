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
