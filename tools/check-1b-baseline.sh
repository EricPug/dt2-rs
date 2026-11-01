#!/bin/bash
# usage: check-1b-baseline.sh <dt2-cli binary> <label>; runs the 1B section-3 boot with the synthetic in-memory eMMC and prints the SHA-256 of the saved snapshot. Equal hashes between builds mean identical machine state; use it as the acceptance test for performance changes.
bin=$1; label=$2
[ -n "$bin" ] && [ -n "$label" ] || { echo "usage: $0 <dt2-cli binary> <label>" >&2; exit 2; }
cd "$(dirname "$0")/.." || exit 1
snap="out/baseline-$label.snap"
tools/run-section3.sh "$bin" 1000000000 RGPIO_READY_MODEL GPIO_LOOPBACK ESDHC_EMMC "SNAPSHOT_SAVE=$snap" > "out/baseline-$label.log" 2>&1
grep -a "^fast path\|runner throughput\|^stop:\|^PC=" "out/baseline-$label.log" | cut -c1-160
echo "snapshot sha256: $(sha256sum "$snap" | cut -d' ' -f1)  ($label)"
