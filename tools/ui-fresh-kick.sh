#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."

BIN=${1:-target/release/dt2-cli}
OUT=out/ui-smoke/fresh-kick
required=(
  out/section3-pre-entry-image.bin
  out/section3-panel-discovery-pre-entry-ddr.bin
  out/section3-panel-discovery-pre-entry-sram.bin
  out/dt2-os-1.17-container-nor.bin
  out/run-card.img
  "samples/008 kick verb.wav"
)
for path in "${required[@]}"; do
  if [[ ! -f "$path" ]]; then
    echo "missing fresh-kick input: $path" >&2
    exit 2
  fi
done
if [[ $(stat -c %s out/run-card.img) -ne 4294967296 ]]; then
  echo "out/run-card.img must be a 4 GiB sparse eMMC backing image" >&2
  exit 2
fi
if [[ ! -x "$BIN" ]]; then
  echo "dt2-cli binary is not executable: $BIN" >&2
  exit 2
fi

mkdir -p "$OUT"
run=$(mktemp -d "$OUT/run.XXXXXXXX")
card="$run/card.img"
modeled_card="$run/modeled-card.img"
trap 'rm -f -- "$card" "$modeled_card"' EXIT
cp --reflink=auto --sparse=always out/run-card.img "$card"

check_sha256() {
  local path=$1 expected=$2 actual
  actual=$(sha256sum "$path" | cut -d' ' -f1)
  if [[ "$actual" != "$expected" ]]; then
    echo "FAIL $path: expected $expected, got $actual" >&2
    exit 1
  fi
  echo "PASS $path sha256=$actual"
}

check_instruction_limit() {
  if ! grep -q '^stop: instruction limit$' "$1"; then
    echo "run did not reach the instruction limit; see $1" >&2
    exit 1
  fi
}

if ! tools/run-section3.sh "$BIN" 1500000000 \
  RGPIO_READY_MODEL GPIO_LOOPBACK ESDHC_EMMC "ESDHC_FILE=$card" \
  INTC1_BOOTSTRAP SSI_AUDIO_IRQ=100000 DTIM3_BOOTSTRAP \
  "UART8_TX_FILE=$run/boot.tx" "LIVE_PNG=$run/boot.png" \
  "SNAPSHOT_SAVE=$run/boot.snap" >"$run/boot.log" 2>&1; then
  echo "fresh-kick boot failed; see $run/boot.log" >&2
  exit 1
fi
check_instruction_limit "$run/boot.log"
check_sha256 "$run/boot.png" \
  8cdccf4db484a161695d1453c0841b4a454ea6cb839ec9ded67ff7f255616e6a

# Copy the card after boot so both replays start with the same card state.
cp --reflink=auto --sparse=always "$card" "$modeled_card"
browse=$(tools/ui-browse-sample.sh --messages)
route="$browse,2120,2100,2102,2100,-,-,-,-,-,-,-,-,-,-,2120,2100,-,-,-,-"
common=(
  RGPIO_READY_MODEL GPIO_LOOPBACK ESDHC_EMMC ESDHC_TRACE
  INTC1_BOOTSTRAP SSI_AUDIO_IRQ=100000 DTIM3_BOOTSTRAP
  DTIM3_TICKS_PER_INSTRUCTION=4 DSP_LINK
  "SNAPSHOT_LOAD=$run/boot.snap" UART8_RX_ONCE
)

if ! tools/run-section3.sh "$BIN" 200000000 "${common[@]}" \
  "ESDHC_FILE=$card" "UART8_RX_PERIODIC=6000000:$route" \
  "UART8_TX_FILE=$run/ui.tx" >"$run/ui.log" 2>&1; then
  echo "fresh-kick UI replay failed; see $run/ui.log" >&2
  exit 1
fi
check_instruction_limit "$run/ui.log"
for block in 005f01c0 005f0200; do
  if ! grep -q "CMD=0x12 .*argument_block=0x$block" "$run/ui.log"; then
    echo "fresh-kick UI replay missed kick card read $block; see $run/ui.log" >&2
    exit 1
  fi
done
if ! grep -q '^RGPIO trace: 125061 events,' "$run/ui.log"; then
  echo "fresh-kick UI replay did not produce the expected upload trace; see $run/ui.log" >&2
  exit 1
fi
python3 tools/render-uart-frames.py --base "$run/boot.png" \
  "$run/ui.tx" "$run/ui-frame" >"$run/ui-render.log"
check_sha256 "$run/ui-frame-final.png" \
  451d9260e91f6214487b39e770f3f85fb4b8177db272dc2583544e7f38c0f022

mkdir -p "$run/modeled/packets"
trigger=$(tools/ui-trigger-track.sh --messages 1)
if ! tools/run-section3.sh "$BIN" 400000000 "${common[@]}" \
  "ESDHC_FILE=$modeled_card" "UART8_RX_PERIODIC=6000000:$route,$trigger" \
  "MODELED_DSP_WAV=$run/modeled/sample.wav" \
  "MODELED_DSP_CAPTURE=$run/modeled/packets" >"$run/modeled/run.log" 2>&1; then
  echo "fresh-kick modeled replay failed; see $run/modeled/run.log" >&2
  exit 1
fi
check_instruction_limit "$run/modeled/run.log"
python3 tools/verify-modeled-dsp.py "$run/modeled" "samples/008 kick verb.wav" 0x9c00
echo "FRESH KICK UI + MODELED DSP PASS: $run"
