#!/bin/bash
# usage: panel-keys.sh <snapshot> <out-prefix> <budget-instructions> <key>...
# Replays panel keys one after another from a snapshot (each key is pressed, released, then left
# for a gap), renders the final display to <out-prefix>.png and saves the machine to
# <out-prefix>.snap. Keys: SETTINGS YES NO UP DOWN LEFT RIGHT PLAY STOP FUNC TRK PAGE, or a raw
# two-byte hex message such as 2301; "wait" is a pause; "+hex" sends the message without releasing it. Extra run-bin flags come from $EXTRA.
# GAP (instructions, default 6000000) is the time per key slot.
snap=$1; prefix=$2; budget=$3; shift 3
cd "$(dirname "$0")/.." || exit 1
seq=""
for key in "$@"; do
  case $key in
    SETTINGS) msg="2080 2000" ;; YES) msg="2102 2100" ;; UP) msg="2104 2100" ;;
    NO) msg="2108 2100" ;; LEFT) msg="2110 2100" ;; DOWN) msg="2120 2100" ;;
    RIGHT) msg="2140 2100" ;; PLAY) msg="2208 2200" ;; STOP) msg="2210 2200" ;;
    FUNC) msg="2201 2200" ;; TRK) msg="2180 2100" ;; PAGE) msg="2220 2200" ;;
    wait) msg="- -" ;;
    +*) msg="${key#+}" ;;
    *) msg="$key ${key:0:2}00" ;;
  esac
  for m in $msg; do seq="$seq,$m"; done
done
seq=${seq#,}
tools/run-section3.sh target/release/dt2-cli "$budget" RGPIO_READY_MODEL GPIO_LOOPBACK ESDHC_EMMC \
  INTC1_BOOTSTRAP SSI_AUDIO_IRQ=100000 DTIM3_BOOTSTRAP DTIM3_TICKS_PER_INSTRUCTION=16 \
  SNAPSHOT_LOAD="$snap" LIVE_PNG="$prefix.png" SNAPSHOT_SAVE="$prefix.snap" UART8_RX_ONCE \
  "UART8_RX_PERIODIC=${GAP:-6000000}:$seq" $EXTRA 2>&1 | tee "${FULL_LOG:-/dev/null}" | grep -a "^LIVE_PNG\|^stop\|saved snapshot\|ESDHC_DUMP\|bus error\|unmapped"
