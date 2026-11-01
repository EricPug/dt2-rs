#!/bin/bash
# usage: check-1b-baseline.sh <dt2-cli binary> <label> [extra run-bin args, e.g. PIT_EAGER]; runs the 1B section-3 boot with the synthetic in-memory eMMC and prints the SHA-256 of the saved snapshot and of the interrupt/RTE log (out/baseline-<label>.irq; diff two logs to find the first divergence). Equal hashes between builds mean identical machine state; use it as the acceptance test for performance changes.
bin=$1; label=$2; shift 2
[ -n "$bin" ] && [ -n "$label" ] || { echo "usage: $0 <dt2-cli binary> <label>" >&2; exit 2; }
cd "$(dirname "$0")/.." || exit 1
snap="out/baseline-$label.snap"
tools/run-section3.sh "$bin" 1000000000 RGPIO_READY_MODEL GPIO_LOOPBACK ESDHC_EMMC "SNAPSHOT_SAVE=$snap" "IRQ_LOG=out/baseline-$label.irq" "$@" > "out/baseline-$label.log" 2>&1
grep -a "^fast path\|runner throughput\|^stop:\|^PC=" "out/baseline-$label.log" | cut -c1-160
echo "irq/rte log: $(wc -l < "out/baseline-$label.irq") lines, sha256 $(sha256sum "out/baseline-$label.irq" | cut -d' ' -f1)"
echo "snapshot sha256: $(sha256sum "$snap" | cut -d' ' -f1)  ($label)"
