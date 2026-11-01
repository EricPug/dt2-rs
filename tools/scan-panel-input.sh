#!/bin/bash
# usage: scan-panel-input.sh <dt2-cli binary> <snapshot> <first-byte-hex-list> <value-hex-list>; injects each two-byte panel message every 2M instructions for 40M instructions from the snapshot and reports the TX traffic and whether the rendered frame changed (needs a snapshot saved with a display attached).
bin=$1; snap=$2; firsts=$3; values=$4
cd "$(dirname "$0")/.." || exit 1
tools/run-section3.sh "$bin" 1000 RGPIO_READY_MODEL GPIO_LOOPBACK ESDHC_EMMC LIVE_PNG=out/scan-ref.png SNAPSHOT_LOAD="$snap" > /dev/null 2>&1
for first in $firsts; do
  for value in $values; do
    msg="$first$value"
    tools/run-section3.sh "$bin" 40000000 RGPIO_READY_MODEL GPIO_LOOPBACK ESDHC_EMMC LIVE_PNG=out/scan.png "UART8_RX_PERIODIC=2000000:$msg" UART8_TX_FILE=out/scan.tx SNAPSHOT_LOAD="$snap" > out/scan.log 2>&1
    changed=same; cmp -s out/scan-ref.png out/scan.png || changed=CHANGED
    echo "$msg tx=$(stat -c %s out/scan.tx) $changed $(xxd -p out/scan.tx | tr -d '\n' | cut -c1-40)"
  done
done
