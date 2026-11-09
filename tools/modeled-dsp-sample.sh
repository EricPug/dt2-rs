#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."

BIN=${1:-target/release/dt2-cli}
OUT=out/modeled-dsp
LIMIT=300000000
EXPECTED_UI_SHA256=a449e4f800b8032ab1de7f2423d90f8e676cb4a75c0644edce60a528c081674e
required=(
  out/section3-pre-entry-image.bin
  out/section3-panel-discovery-pre-entry-ddr.bin
  out/section3-panel-discovery-pre-entry-sram.bin
  out/dt2-os-1.17-container-nor.bin
  out/run-card.img
  out/shots/action-down.snap
  out/shots/action-down.png
  "samples/008 kick verb.wav"
  "samples/303 clap real.wav"
)
for path in "${required[@]}"; do
  if [[ ! -f "$path" ]]; then
    echo "missing modeled-DSP input: $path" >&2
    exit 2
  fi
done
if [[ $(stat -c %s out/run-card.img) -ne 4294967296 ]]; then
  echo "out/run-card.img must be a 4 GiB sparse eMMC backing image" >&2
  exit 2
fi
if [[ $# -eq 0 ]]; then
  cargo build --release -p dt2-cli
elif [[ ! -x "$BIN" ]]; then
  echo "dt2-cli binary is not executable: $BIN" >&2
  exit 2
fi

load_messages=$(tools/ui-load-sample.sh --messages)
trigger_messages=$(tools/ui-trigger-track.sh --messages 1)
message_list="$load_messages,-,-,-,-,$trigger_messages"

run_sample() {
  local name=$1 source=$2 length=$3 dir="$OUT/$1" card
  mkdir -p "$dir/packets"
  card=$(mktemp "$dir/card.XXXXXXXX.img")
  cp --reflink=auto --sparse=always out/run-card.img "$card"
  if [[ "$name" == clap ]]; then
    python3 tools/substitute-clap-card.py "$card"
  fi
  if ! tools/run-section3.sh "$BIN" "$LIMIT" \
    RGPIO_READY_MODEL GPIO_LOOPBACK ESDHC_EMMC "ESDHC_FILE=$card" \
    INTC1_BOOTSTRAP SSI_AUDIO_IRQ=100000 DTIM3_BOOTSTRAP \
    DTIM3_TICKS_PER_INSTRUCTION=16 SNAPSHOT_LOAD=out/shots/action-down.snap \
    "UART8_TX_FILE=$dir/ui.tx" UART8_RX_ONCE \
    "UART8_RX_PERIODIC=6000000:$message_list" DSP_LINK \
    "MODELED_DSP_WAV=$dir/sample.wav" "MODELED_DSP_CAPTURE=$dir/packets" \
    >"$dir/run.log" 2>&1; then
    tail -n 40 "$dir/run.log" >&2
    exit 1
  fi
  grep -q '^stop: instruction limit$' "$dir/run.log"
  python3 tools/verify-modeled-dsp.py "$dir" "$source" "$length"
  rm -- "$card"
}

run_sample kick "samples/008 kick verb.wav" 0x9c00
run_sample clap "samples/303 clap real.wav" 0x3022

python3 tools/render-uart-frames.py --base out/shots/action-down.png \
  "$OUT/kick/ui.tx" "$OUT/kick/ui-frame" >"$OUT/kick/ui-render.log"
ui_sha256=$(sha256sum "$OUT/kick/ui-frame-final.png" | cut -d' ' -f1)
if [[ "$ui_sha256" != "$EXPECTED_UI_SHA256" ]]; then
  echo "FAIL modeled-DSP UI frame: expected $EXPECTED_UI_SHA256, got $ui_sha256" >&2
  exit 1
fi
echo "PASS modeled-DSP UI frame sha256=$ui_sha256"
