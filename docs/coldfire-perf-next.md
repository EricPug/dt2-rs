# ColdFire performance: follow-on ideas

This is an unprioritized list of possible experiments, not the current plan.
The original interpreter optimization proposal and measured implementation
results are in [coldfire-perf.md](coldfire-perf.md); current project status is
in [section3-handoff.md](section3-handoff.md).

Potential areas to investigate if profiling shows they remain relevant:

- Direct-threaded dispatch or selective compilation of frequently executed
  blocks.
- Guarded inline caches for hot indirect calls.
- Host-register mapping for selected guest registers.
- RAM/MMIO fast paths and lower-cost self-modifying-code invalidation.

Any future optimization should preserve the Rust interpreter as a reference
path and be measured against the existing output, state, and timing regression
gates. These ideas do not imply a performance target or implementation
commitment.
