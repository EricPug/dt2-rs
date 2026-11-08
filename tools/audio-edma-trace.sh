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
  local name=$1 snapshot=$2 limit=$3 messages=$4 raw_capture=${5:-} watch_range=${6:-} trace_pc=${7:-} read_range=${8:-}
  local -a options=(
    RGPIO_READY_MODEL GPIO_LOOPBACK ESDHC_EMMC ESDHC_FILE=out/run-card.img
    INTC1_BOOTSTRAP SSI_AUDIO_IRQ=100000 DTIM3_BOOTSTRAP
    DTIM3_TICKS_PER_INSTRUCTION=16
    SNAPSHOT_LOAD="$snapshot" SNAPSHOT_SAVE="$OUT/$name.snap"
    LIVE_PNG="$OUT/$name.png" UART8_RX_ONCE
    "UART8_RX_PERIODIC=6000000:$messages"
    DSP_LINK EDMA_TRACE NONRAM_BULK_TRACE FLEXBUS_TRACE ESDHC_TRACE
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
  if [[ -n "$read_range" ]]; then
    options+=("WATCH_READ=$read_range")
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

check_bulk_actor() {
  local name=$1 actor=$2 expected=$3 actual
  actual=$(grep -c "^  BULK_IO actor=$actor" "$OUT/$name.log" || true)
  if [[ "$actual" -ne "$expected" ]]; then
    echo "$name: expected $expected $actor bulk transfers, got $actual" >&2
    exit 1
  fi
}

check_no_cpu_bulk() {
  local name=$1
  if grep -q '^  BULK_IO actor=CPU ' "$OUT/$name.log"; then
    echo "$name: unexpected qualifying CPU bulk write" >&2
    exit 1
  fi
  echo "PASS $name non-RAM bulk DMA counts verified; CPU bulk writes=0"
}

check_dspi2_fields() {
  local name=$1 variant=$2
  python3 - "$OUT/$name.log" "$variant" <<'PY'
import collections
import re
import sys

counts = collections.Counter()
blocks = 0
for line in open(sys.argv[1], encoding="utf-8", errors="replace"):
    match = re.match(r"DSP_LINK block (\d+) len=\d+ (.*)", line)
    if not match:
        continue
    words = match.group(2).split()
    if len(words) < 8:
        raise SystemExit(f"short DSPI2 block: {match.group(1)}")
    counts[(words[3], words[7])] += 1
    blocks += 1

expected = {
    "note1": {("0000", "0000"): 64, ("3c00", "0000"): 570, ("3c00", "2d00"): 365},
    "note2": {("0000", "0000"): 646, ("0000", "2d00"): 353},
}[sys.argv[2]]
if blocks != 999 or counts != expected:
    raise SystemExit(
        f"{sys.argv[1]}: blocks={blocks}, fields={dict(counts)}, expected={expected}"
    )
print(f"PASS {sys.argv[1]} DSPI2 offsets 0x02/0x06: {dict(counts)}")
PY
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
check_bulk_actor sample-load DMA29 1999
check_bulk_actor sample-load DMA50 1999
check_no_cpu_bulk sample-load
grep -q 'DADDR=0x4060d770' "$OUT/sample-load.log"
grep -q 'DADDR=0x40605770' "$OUT/sample-load.log"
grep -q '^EDMA_TRACE request channel=29 .*NBYTES=0x00000004 .*DADDR=0xec038034 .*CITER=0x055e ' \
  "$OUT/sample-load.log"
grep -q '^EDMA_TRACE request channel=28 .*NBYTES=0x00000002 .*DADDR=0x80001000 .*CITER=0x055e ' \
  "$OUT/sample-load.log"
grep -q '^DSP_LINK block 0 len=5496 8001 ' "$OUT/sample-load.log"
echo "PASS sample-load DSPI2 frames=1374 RX_bytes=2748 PUSHR_source_bytes=5496"
grep -q '^RGPIO trace: 135300 events, showing first 32' "$OUT/sample-load.log"
grep -q '^FlexBus trace: 135300 access(es); offset <0x10=135300 offset >=0x10=0' \
  "$OUT/sample-load.log"
if grep -Eq '^EDMA_TRACE .*(SADDR|DADDR)=0x8c' "$OUT/sample-load.log"; then
  echo "sample-load: unexpected eDMA endpoint in the 0x8c window" >&2
  exit 1
fi
grep -q 'read16 0x8c000002' "$OUT/sample-load.log"
grep -q 'write16 0x8c00000a' "$OUT/sample-load.log"
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

if [[ ${SKIP_SAMPLE_BUFFER_PROBE:-0} != 1 ]]; then
  run_trace sample-load-buffer out/shots/action-down.snap 200000000 \
    2104,2100,2104,2100,2104,2100,2104,2100,2102,2100,-,-,-,-,2102,2100 \
    "" 0x4fe79340:0x4fe79640 "" 0x4fe79340:0x4fe79640
  check_requests sample-load-buffer 14 1
  check_requests sample-load-buffer 15 1
  grep -q '^write watch at 0x4fe79340..0x4fe79640: 0 matching writes' \
    "$OUT/sample-load-buffer.log"
  grep -q '^read watch at 0x4fe79340..0x4fe79640: 0 matching reads' \
    "$OUT/sample-load-buffer.log"
  grep -Eq '^  initial=0+$' "$OUT/sample-load-buffer.log"
fi

run_trace trig-play out/shots/main-after-load.snap 100000000 \
  2301,2300,2208,2200 "$OUT/trig-play.ssi.raw"
check_requests trig-play 28 999
check_requests trig-play 29 999
check_requests trig-play 35 1775
check_bulk_actor trig-play DMA29 999
check_bulk_actor trig-play DMA50 999
check_no_cpu_bulk trig-play
grep -q 'SSI TX raw capture: 999 DMA block(s), 2045952 byte(s), 0 nonzero' \
  "$OUT/trig-play.log"
grep -q '^FlexBus trace: 0 access(es); offset <0x10=0 offset >=0x10=0' \
  "$OUT/trig-play.log"
if grep -Eq '^EDMA_TRACE .*(SADDR|DADDR)=0x8c' "$OUT/trig-play.log"; then
  echo "trig-play: unexpected eDMA endpoint in the 0x8c window" >&2
  exit 1
fi
check_dspi2_fields trig-play note1

run_trace trig-play-note2 out/shots/main-after-load.snap 100000000 \
  2302,2300,2208,2200
check_requests trig-play-note2 28 999
check_requests trig-play-note2 29 999
check_dspi2_fields trig-play-note2 note2

echo "AUDIO EDMA TRACE PASS"
