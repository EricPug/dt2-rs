# Patched Unicorn for the execution tier

`dt2-cli --features unicorn` with `DT2_UNICORN=1` runs the CPU in Unicorn (QEMU TCG) and keeps the
Rust interpreter as the fallback. `build.sh` builds Unicorn 2.1.4 (m68k only) with digiemu's six
patches plus `unicorn-2.1.4-m68k-dt2-irq-emac.patch`, which adds:

- `uc_digikit_set_irq_level`: the host holds an interrupt of this level pending. Any SR change that
  drops the mask below it (MOVE/ANDI/ORI to SR, native RTE) stops emulation at the next
  instruction boundary, so the host can deliver it exactly when the Rust interpreter would.
- `uc_digikit_stop_at_boundary`, `uc_digikit_counter`, `uc_digikit_count_limit`: stop from MMIO or
  INTR callbacks and read the exact retired-instruction count (replaces a `uc_struct` offset probe).
- `uc_digikit_emac_get` / `uc_digikit_emac_set`: raw 48-bit accumulators, MACSR and MASK.

Licence: the patches are derivative works of Unicorn/QEMU (GPL-2.0), not covered by this
repository's MIT licence. A `dt2-cli` binary linked against the patched library is a GPL-2.0
combination.
