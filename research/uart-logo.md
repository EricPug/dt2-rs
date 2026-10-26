# UART8 Logo-Like Record Stream

## Observation

The section-2 natural-PIT0 run captured 1,321 bytes on the TX-only UART8 model. In the rendered bitmap, the user identifies the central mark as the Elektron logo and the lower-left mark as a capital “B”; these are visual identifications, not independent hardware verification. The capture occurs during bootstrap, before the main OS handoff, so it does not establish that UART8 carries the main UI framebuffer.

The captured stream is structured as a 40-byte header/handshake, 128 records of 10 bytes each (`tag`, `y-coordinate`, eight payload bytes), and a final `0xB8` terminator. Record tags span `0x10`–`0x17`; y-coordinates run from `0x00` through `0x78` in steps of eight. The diagnostic renderer treats each payload as an 8x8 monochrome tile with bit 7 leftmost. This is a visualization hypothesis, not a proven device protocol.

## Golden Hashes

These hashes cover the raw UART bytes decoded from the run log's `UART8 TX hex:` line, not the textual log or rendered bitmap.

- SHA-256: `66c9154fecca8259cc0037c86836060d9b18b1702c974ed2bd08c9b546074cab`
- FNV-1a64 regression fingerprint: `74b7474d6f783291`

The firmware, raw capture, and generated image remain ignored and are not checked in. With the local capture present, run the opt-in regression:

```sh
cargo test -p dt2-cli --test uart_logo_regression -- --ignored
```

Regenerate the diagnostic bitmap with:

```sh
cargo run --release -p dt2-cli -- render-uart-records out/pit0-natural-run.log out/uart-record-bitmap.ppm
```

The natural-run capture used the staged OS 1.17 NOR image, combined SBF/SRAM/DDR mapping, a seeded CR27 bit-3 value, and opt-in PIT/INTC2 diagnostics. It stopped at unsupported opcode `0xB782` after a natural PIT0 interrupt; section 3 was not loaded.
