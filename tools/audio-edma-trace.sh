#!/bin/bash
# Reproduce the sample-load and post-load eDMA traces used by the audio handoff investigation.
set -euo pipefail
cd "$(dirname "$0")/.."

BIN=${1:-target/release/dt2-cli}
OUT=out/audio-edma-trace
required=(
  out/section3-pre-entry-image.bin
  out/section3-panel-discovery-pre-entry-ddr.bin
  out/section3-panel-discovery-pre-entry-sram.bin
  out/dt2-os-1.17-container-nor.bin
  out/run-card.img
  out/shots/action-down.snap
  out/shots/main-after-load.snap
)
for path in "${required[@]}"; do
  if [[ ! -f "$path" ]]; then
    echo "missing audio-trace input: $path" >&2
    exit 2
  fi
done
if [[ $(stat -c %s out/run-card.img) -ne 4294967296 ]]; then
  echo "out/run-card.img must be a 4 GiB sparse eMMC backing image" >&2
  exit 2
fi

if [[ ! -x "$BIN" ]]; then
  cargo build --release -p dt2-cli
fi
mkdir -p "$OUT"

run_trace() {
  local name=$1 snapshot=$2 limit=$3 messages=$4 raw_capture=${5:-} watch_range=${6:-} trace_pc=${7:-}
  local -a options=(
    RGPIO_READY_MODEL GPIO_LOOPBACK ESDHC_EMMC ESDHC_FILE=out/run-card.img
    INTC1_BOOTSTRAP SSI_AUDIO_IRQ=100000 DTIM3_BOOTSTRAP
    DTIM3_TICKS_PER_INSTRUCTION=16
    SNAPSHOT_LOAD="$snapshot" SNAPSHOT_SAVE="$OUT/$name.snap"
    LIVE_PNG="$OUT/$name.png" UART8_RX_ONCE
    "UART8_RX_PERIODIC=6000000:$messages"
    DSP_LINK EDMA_TRACE ESDHC_TRACE
  )
  if [[ -n "$raw_capture" ]]; then
    options+=("SSI_TX_RAW=$raw_capture")
  fi
  if [[ -n "$watch_range" ]]; then
    options+=("WATCH_WRITE=$watch_range")
  fi
  if [[ -n "$trace_pc" ]]; then
    options+=("TRACE_REGS=$trace_pc")
  fi
  tools/run-section3.sh "$BIN" "$limit" \
    "${options[@]}" \
    >"$OUT/$name.log" 2>&1
  if ! grep -q '^stop: instruction limit$' "$OUT/$name.log"; then
    echo "$name trace did not reach its instruction limit; see $OUT/$name.log" >&2
    exit 1
  fi
}

check_requests() {
  local name=$1 channel=$2 expected=$3 actual
  actual=$(grep -c "^EDMA_TRACE request channel=$channel " "$OUT/$name.log" || true)
  if [[ "$actual" -ne "$expected" ]]; then
    echo "$name: expected $expected eDMA requests on channel $channel, got $actual" >&2
    exit 1
  fi
  echo "PASS $name channel $channel requests=$actual"
}

echo "Sample-load probe uses synthetic DSP_LINK replies; no DSP or audio is modeled."
run_trace sample-load out/shots/action-down.snap 200000000 \
  2104,2100,2104,2100,2104,2100,2104,2100,2102,2100,-,-,-,-,2102,2100 \
  "" 0x43176a30:0x43176a40 0x4003314e
check_requests sample-load 14 1
check_requests sample-load 15 1
check_requests sample-load 28 1999
check_requests sample-load 29 1999
check_requests sample-load 35 991
check_requests sample-load 59 2
grep -q 'DADDR=0x4060d770' "$OUT/sample-load.log"
grep -q 'DADDR=0x40605770' "$OUT/sample-load.log"
grep -q '^EDMA_TRACE request channel=29 .*NBYTES=0x00000004 .*DADDR=0xec038034 .*CITER=0x055e ' \
  "$OUT/sample-load.log"
grep -q '^EDMA_TRACE request channel=28 .*NBYTES=0x00000002 .*DADDR=0x80001000 .*CITER=0x055e ' \
  "$OUT/sample-load.log"
grep -q '^DSP_LINK block 0 len=5496 8001 ' "$OUT/sample-load.log"
echo "PASS sample-load DSPI2 frames=1374 RX_bytes=2748 PUSHR_source_bytes=5496"
grep -q 'write watch at 0x43176a30..0x43176a40:' "$OUT/sample-load.log"
grep -q 'PC=0x40033160 address=0x43176a3c .*new=0000012c' \
  "$OUT/sample-load.log"
grep -q 'PC=0x40012568 address=0x43176a32 .*new=012c' \
  "$OUT/sample-load.log"
grep -q 'PC=0x4001257a address=0x43176a30 .*new=0001' \
  "$OUT/sample-load.log"
grep -q '^register trace at PC=0x4003314e: [1-9][0-9]* hit(s)' \
  "$OUT/sample-load.log"
grep -q 'stack(A7)=\[4019fc7e, 0000012c, 4525722c' \
  "$OUT/sample-load.log"

run_trace trig-play out/shots/main-after-load.snap 100000000 \
  2301,2300,2208,2200 "$OUT/trig-play.ssi.raw"
check_requests trig-play 28 999
check_requests trig-play 29 999
check_requests trig-play 35 1775
grep -q 'SSI TX raw capture: 999 DMA block(s), 2045952 byte(s), 0 nonzero' \
  "$OUT/trig-play.log"

echo "AUDIO EDMA TRACE PASS"
