#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."

BIN=${1:-target/release/dt2-cli}
OUT=out/modeled-dsp
LIMIT=300000000
EXPECTED_WAV_SHA256=7b45eaff1a7315ff954445dc1292a0915b6dcb3b3ec778657301375ce4cc4200
required=(
  out/section3-pre-entry-image.bin
  out/section3-panel-discovery-pre-entry-ddr.bin
  out/section3-panel-discovery-pre-entry-sram.bin
  out/dt2-os-1.17-container-nor.bin
  out/run-card.img
  out/shots/action-down.snap
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
mkdir -p "$OUT"

messages=(
  2104,2100,2104,2100,2104,2100,2104,2100,2102,2100
  -,-,-,-,2102,2100
  -,-,-,-,2301,2300,2208,2200
)
message_list=$(IFS=,; echo "${messages[*]}")
if ! tools/run-section3.sh "$BIN" "$LIMIT" \
  RGPIO_READY_MODEL GPIO_LOOPBACK ESDHC_EMMC ESDHC_FILE=out/run-card.img \
  INTC1_BOOTSTRAP SSI_AUDIO_IRQ=100000 DTIM3_BOOTSTRAP \
  DTIM3_TICKS_PER_INSTRUCTION=16 SNAPSHOT_LOAD=out/shots/action-down.snap \
  UART8_RX_ONCE "UART8_RX_PERIODIC=6000000:$message_list" DSP_LINK \
  "MODELED_DSP_WAV=$OUT/sample.wav" \
  "DUMP_RAM_FILE=0x4060d770:0x8000:$OUT/sample-first.bin" \
  "DUMP_RAM_FILE=0x40605770:0x1c00:$OUT/sample-tail.bin" \
  >"$OUT/run.log" 2>&1; then
  tail -n 40 "$OUT/run.log" >&2
  exit 1
fi
if ! grep -q '^stop: instruction limit$' "$OUT/run.log"; then
  echo "modeled-DSP run did not reach its instruction limit; see $OUT/run.log" >&2
  exit 1
fi
grep -q '12 RGPIO upload call(s)' "$OUT/run.log" || {
  echo "modeled-DSP run did not capture all 12 RGPIO upload calls; see $OUT/run.log" >&2
  exit 1
}

python3 - "$OUT" "$EXPECTED_WAV_SHA256" <<'PY'
import hashlib
import re
import struct
import sys
import wave
from pathlib import Path

out = Path(sys.argv[1])
expected_sha256 = sys.argv[2]
log = (out / "run.log").read_text(encoding="utf-8", errors="replace")
uploads = re.findall(
    r"^MODELED_DSP RGPIO call=(\d+) prefix=([0-9a-f]{8}) (header|page=(\d+))",
    log,
    re.MULTILINE,
)
expected_uploads = [(1, "ffffffff", "header", "")] + [
    (call, f"{call - 2:08x}", f"page={call - 2}", str(call - 2))
    for call in range(2, 12)
] + [(12, "ffffffff", "header", "")]
if [(int(call), prefix, kind, page) for call, prefix, kind, page in uploads] != expected_uploads:
    raise SystemExit(f"unexpected RGPIO upload order: {uploads}")
if "MODELED_DSP RGPIO call=12 prefix=ffffffff header slot=300 sample_length=0x9c00" not in log:
    raise SystemExit("final RGPIO header did not identify slot 300 / sample length 0x9c00")

match = re.search(
    r"first sample-note edge: block (\d+), instruction (\d+), track (\d+), note=(0x[0-9a-f]+)",
    log,
)
if not match:
    raise SystemExit("MODELED_DSP_WAV summary has no sample-note edge")
block, instruction, track, note = match.groups()
if int(track) != 2 or int(note, 16) != 0x2D00:
    raise SystemExit(f"unexpected sample trigger: track={track}, note={note}")

source = (out / "sample-first.bin").read_bytes() + (out / "sample-tail.bin").read_bytes()
if len(source) != 0x9C00:
    raise SystemExit(f"reconstructed source is {len(source):#x} bytes, expected 0x9c00")
source = source[6:]
if len(source) % 2:
    raise SystemExit("sample PCM payload is not 16-bit aligned")
expected = [int.from_bytes(source[i : i + 2], "big", signed=True) for i in range(0, len(source), 2)]

with wave.open(str(out / "sample.wav"), "rb") as wav:
    if (wav.getnchannels(), wav.getsampwidth(), wav.getframerate(), wav.getcomptype()) != (
        1,
        2,
        48_000,
        "NONE",
    ):
        raise SystemExit("WAV is not mono 16-bit PCM at 48 kHz")
    frame_count = wav.getnframes()
    pcm = wav.readframes(frame_count)
actual = struct.unpack(f"<{len(pcm) // 2}h", pcm)
start = int(block) * 32
segment = actual[start : start + len(expected)]
if len(segment) != len(expected):
    raise SystemExit(f"WAV ends before the sample segment at frame {start}")
if not any(actual) or not any(segment):
    raise SystemExit("WAV or played sample segment is all zero")
if list(segment) != expected:
    raise SystemExit("played WAV segment does not exactly match the loaded sample waveform")
wav_sha256 = hashlib.sha256((out / "sample.wav").read_bytes()).hexdigest()
if wav_sha256 != expected_sha256:
    raise SystemExit(f"WAV SHA-256 differs: expected {expected_sha256}, got {wav_sha256}")

print(
    f"PASS modeled DSP: {len(expected)} exact sample frames at WAV frame {start}; "
    f"trigger instruction={instruction}; WAV SHA-256={wav_sha256}; "
    f"waveform SHA-256={hashlib.sha256(source).hexdigest()}"
)
PY
