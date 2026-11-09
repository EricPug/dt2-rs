#!/bin/bash
# Deterministic UI smoke gate. Requires ignored firmware-derived boot assets plus
# out/shots/main-after-load.snap, its out/run-card.img backing file, and matching PNG.
set -euo pipefail
cd "$(dirname "$0")/.."

BIN=${1:-target/release/dt2-cli}
OUT=out/ui-smoke

required=(
  out/section3-pre-entry-image.bin
  out/section3-panel-discovery-pre-entry-ddr.bin
  out/section3-panel-discovery-pre-entry-sram.bin
  out/dt2-os-1.17-container-nor.bin
  out/shots/main-after-load.snap
  out/shots/main-after-load.png
  out/run-card.img
)
for path in "${required[@]}"; do
  if [[ ! -f "$path" ]]; then
    echo "missing smoke-test input: $path" >&2
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

check_sha256() {
  local path=$1 expected=$2 actual
  actual=$(sha256sum "$path" | cut -d' ' -f1)
  if [[ "$actual" != "$expected" ]]; then
    echo "FAIL $path: expected $expected, got $actual" >&2
    exit 1
  fi
  echo "PASS $path sha256=$actual"
}

# Full diagnostic boot: required synthetic/reset options are centralized in run-section3.sh.
tools/run-section3.sh "$BIN" 1500000000 \
  RGPIO_READY_MODEL GPIO_LOOPBACK ESDHC_EMMC INTC1_BOOTSTRAP \
  SSI_AUDIO_IRQ=100000 DTIM3_BOOTSTRAP \
  UART8_TX_FILE="$OUT/boot.tx" LIVE_PNG="$OUT/boot.png" \
  SNAPSHOT_SAVE="$OUT/boot.snap" \
  >"$OUT/boot.log" 2>&1
grep -q '^stop: instruction limit$' "$OUT/boot.log"
check_sha256 "$OUT/boot.png" \
  8cdccf4db484a161695d1453c0841b4a454ea6cb839ec9ded67ff7f255616e6a

# The sample-loaded pin is required for page navigation and modeled playback.
check_sha256 out/shots/main-after-load.png \
  6e4f2126469865d8b9c49cbae17ecf9cb61e22bc85c522acb6e2bc01b2c62ff6
check_sha256 out/shots/main-after-load.snap \
  225760283abcf448018f924c3eb71890666694e5b0c5d2825df7d545efa297a6

tools/ui-turn-encoder.sh --tune-16-one-count-steps \
  out/shots/main-after-load.snap out/shots/main-after-load.png \
  "$OUT/tune-16-single-steps" \
  0121a4cf0880c5892dabd094dbc7cf4caf6d14fd9d93cfd5b276d37981a74368 "$BIN"

tools/ui-change-page.sh FLTR \
  out/shots/main-after-load.snap out/shots/main-after-load.png \
  "$OUT/fltr" \
  20b7b62611a13133779f3da6e452591182fc7ff5bc37c2b9b3ada5e16b7133c3 "$BIN"
tools/ui-open-sample-page.sh \
  "$OUT/fltr.snap" "$OUT/fltr-frame-final.png" "$OUT/sample-page" \
  18f5fff0850f13a36134f16346e12e56a00edd62adfb78eb434badcae0aede7b "$BIN"

# Load and trigger the sample through the opt-in modeled DSP, then check the
# deterministic WAV hash and exact sample segment in the companion runner.
tools/modeled-dsp-sample.sh "$BIN"
check_sha256 out/modeled-dsp/sample.wav \
  7b45eaff1a7315ff954445dc1292a0915b6dcb3b3ec778657301375ce4cc4200

echo "UI + MODELED DSP SMOKE PASS"
