#!/bin/bash
# Regression gate for ColdFire core performance work (docs/coldfire-perf.md).
# Requires out/panel-1p5b.snap. Usage: tools/perf-gate.sh [binary]
set -u
cd "$(dirname "$0")/.."
BIN=${1:-target/release/dt2-cli}
GOLD_TX=7662453ca89250adcde7065dd4285581c1e6cf95
GOLD_TIMES=ba54cae43705472fbbc046c643a96ce4e1f10f8b
GOLD_HASH="STATE_HASH 1000000 8ae0f23727bb21a4
STATE_HASH 8000000 85a056a4b728bdec
STATE_HASH 20000000 137d257b7f735853
STATE_HASH 40000000 16ea982faa2bd2c1"
FLAGS="RGPIO_READY_MODEL GPIO_LOOPBACK ESDHC_EMMC INTC1_BOOTSTRAP SSI_AUDIO_IRQ=100000 DTIM3_BOOTSTRAP DTIM3_TICKS_PER_INSTRUCTION=16 SNAPSHOT_LOAD=out/panel-1p5b.snap UART8_RX_PERIODIC=8000000:3010"
EXTRA=${GATE_EXTRA:-}
T=$(mktemp -d)
fail=0

# 1. TX stream + timing + throughput (plain run)
tools/run-section3.sh "$BIN" 40000000 $FLAGS $EXTRA UART8_TX_FILE=$T/tx.bin >$T/run.log 2>&1
tx=$(sha1sum <$T/tx.bin | cut -d' ' -f1)
tm=$(sha1sum <$T/tx.bin.times | cut -d' ' -f1)
[ "$tx" = "$GOLD_TX" ] || { echo "FAIL TX $tx"; fail=1; }
[ "$tm" = "$GOLD_TIMES" ] || { echo "FAIL TIMES $tm"; fail=1; }
grep -E "runner throughput" $T/run.log

# 2. State hashes
tools/run-section3.sh "$BIN" 40000000 $FLAGS $EXTRA STATE_HASH=1000000,8000000,20000000 >$T/h.log 2>&1
got=$(grep '^STATE_HASH' $T/h.log)
[ "$got" = "$GOLD_HASH" ] || { echo "FAIL state hashes:"; echo "$got"; fail=1; }

rm -rf "$T"
[ $fail = 0 ] && echo "GATE PASS" || echo "GATE FAIL"
exit $fail
