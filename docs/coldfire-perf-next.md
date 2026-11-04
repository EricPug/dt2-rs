ColdFire performance — additional techniques

The current plan (decode cache → block cache) is the foundation. Measurements show that even if both are free the interpreter is still limited to ~25 M instr/s, because the dominant costs are dispatch, the firmware’s virtual calls, and bus checks. The following are standard techniques that attack those costs more directly. They should be evaluated as later slices, after the decode/block-cache foundation exists, using the same regression gates and fallback interpreter.
Techniques to consider

    Direct threading
    Each decoded instruction ends with the address of the next handler. Removes the central dispatch loop. Typical 1.5–2× on top of a decode cache; simpler than a JIT.
    Inline caching of hot virtual calls
    The hot firmware path is generic C++ virtual-call walking. Record the actual target of hot indirect calls and emit a guarded specialized sequence. A plain block cache does not remove this cost.
    Selective compilation of hot blocks only
    Compile only the blocks that dominate execution (top 1 000 PCs already cover 91 %). Cold code stays interpreted. Keeps complexity down while still giving large gains.
    Host-register mapping for the hottest guest registers
    Keep a few ColdFire registers in host registers across a block. High payoff; needs correct invalidation at block boundaries and exception points.
    Fast-path RAM vs MMIO
    Split plain RAM (raw pointer) from MMIO/guarded pages. The bus is still a large fraction of time; the common case should be a simple load/store.
    Cheaper self-modifying-code detection
    Page-granularity dirty bits or a small watched set for code pages, instead of a full check on every write.

Constraints (same as before)

    Existing interpreter remains the fallback until the new path is proven.
    Regression gates (TX stream, state hashes, cached-vs-uncached differential) stay green at every slice.
    Exact instruction counts and interrupt/timer boundaries are preserved (blocks run only up to the next event).
    No scattered special cases; each technique should be localized and measurable.
    Stop and report if a slice lands far under its estimate.

Suggested order after the current block-cache work

    Direct threading
    Fast-path RAM
    Inline caching of the hottest virtual calls
    Selective compilation of hot blocks
    Register mapping and cheaper SMC detection as follow-ons

Evaluate each with a short measurement note before implementing the next.
