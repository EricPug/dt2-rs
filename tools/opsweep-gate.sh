#!/bin/sh
# Handler-tier differential gate: every opcode, 6 seeded states; the cache-hit run (with the
# pre-decoded handlers) must hash identically to the same run on the uncached interpreter.
set -e
cd "$(dirname "$0")/.."
cargo build --release --example opsweep -p dt2-core 2>&1 | grep -E "^error" -A8 && exit 1
a=$(DOUBLE=1 target/release/examples/opsweep)
b=$(CACHED=1 target/release/examples/opsweep)
echo "uncached: $a"; echo "cached:   $b"
[ "$a" = "$b" ] && echo "OPSWEEP PASS" || { echo "OPSWEEP FAIL"; exit 1; }
