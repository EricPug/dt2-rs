#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."

recipe=$1
shift

trigger_key() {
  local track=$1 row=23 bit
  if [[ ! "$track" =~ ^[0-9]+$ ]] || (( track < 1 || track > 16 )); then
    echo "track/trigger index must be between 1 and 16: $track" >&2
    exit 2
  fi
  if (( track > 8 )); then
    row=24
  fi
  bit=$(( (track - 1) % 8 ))
  printf '%s%02x' "$row" "$((1 << bit))"
}

messages_for() {
  local name=$1 argument=${2:-} key
  case "$name" in
    encoder)
      echo '3005,3000'
      ;;
    page)
      case "$argument" in
        SRC|SAMPLE) echo '2002,2000' ;;
        FLTR) echo '2004,2000' ;;
        AMP) echo '2006,2000' ;;
        FX) echo '2008,2000' ;;
        MOD) echo '2010,2000' ;;
        *)
          echo "unsupported page: $argument" >&2
          exit 2
          ;;
      esac
      ;;
    open-sample-page)
      echo '2002,2000'
      ;;
    select-track)
      key=$(trigger_key "$argument")
      echo "2180,2100,${key},${key:0:2}00"
      ;;
    trigger-track)
      key=$(trigger_key "$argument")
      echo "${key},${key:0:2}00,2208,2200"
      ;;
    browse-sample)
      echo '2201,2601,2600,2200,2102,2100,2120,2100,2102,2100,2102,2100,2140,2100'
      ;;
    load-sample)
      echo '2104,2100,2104,2100,2104,2100,2104,2100,2102,2100,-,-,-,-,2102,2100'
      ;;
    *)
      echo "unknown UI recipe: $name" >&2
      exit 2
      ;;
  esac
}

if [[ ${1:-} == --messages ]]; then
  shift
  messages_for "$recipe" "${1:-}"
  exit
fi

usage() {
  echo "usage: $0 <snapshot> <base.png> <out-prefix> <expected-sha256> [dt2-cli]" >&2
  exit 2
}

case "$recipe" in
  encoder|open-sample-page)
    [[ $# -ge 4 && $# -le 5 ]] || usage
    snapshot=$1 base_png=$2 out_prefix=$3 expected=$4
    bin=${5:-target/release/dt2-cli}
    period=3000000 budget=100000000
    ;;
  page)
    [[ $# -ge 5 && $# -le 6 ]] || usage
    page=$1
    shift
    snapshot=$1 base_png=$2 out_prefix=$3 expected=$4
    bin=${5:-target/release/dt2-cli}
    period=3000000 budget=100000000
    ;;
  select-track|trigger-track)
    [[ $# -ge 5 && $# -le 6 ]] || usage
    index=$1
    trigger_key "$index" >/dev/null
    shift
    snapshot=$1 base_png=$2 out_prefix=$3 expected=$4
    bin=${5:-target/release/dt2-cli}
    budget=100000000
    if [[ "$recipe" == trigger-track ]]; then
      period=6000000
    else
      period=3000000
    fi
    ;;
  browse-sample)
    [[ $# -ge 4 && $# -le 5 ]] || usage
    snapshot=$1 base_png=$2 out_prefix=$3 expected=$4
    bin=${5:-target/release/dt2-cli}
    period=6000000 budget=300000000
    ;;
  load-sample)
    [[ $# -ge 4 && $# -le 5 ]] || usage
    snapshot=$1 base_png=$2 out_prefix=$3 expected=$4
    bin=${5:-target/release/dt2-cli}
    period=6000000 budget=300000000
    ;;
  *)
    echo "unknown UI recipe: $recipe" >&2
    exit 2
    ;;
esac

if [[ ! -f "$snapshot" || ! -f "$base_png" ]]; then
  echo "missing recipe snapshot or base frame: $snapshot / $base_png" >&2
  exit 2
fi
if [[ ! "$expected" =~ ^[0-9a-f]{64}$ ]]; then
  echo "expected screen SHA-256 must be 64 lowercase hex digits" >&2
  exit 2
fi
if [[ ! -x "$bin" ]]; then
  echo "dt2-cli binary is not executable: $bin" >&2
  exit 2
fi
if [[ ! -f out/run-card.img ]]; then
  echo "missing UI recipe eMMC image: out/run-card.img" >&2
  exit 2
fi
if [[ $(stat -c %s out/run-card.img) -ne 4294967296 ]]; then
  echo "out/run-card.img must be a 4 GiB sparse eMMC backing image" >&2
  exit 2
fi

mkdir -p "$(dirname "$out_prefix")"
messages=$(messages_for "$recipe" "${page:-${index:-}}")
tools/run-section3.sh "$bin" "$budget" \
  RGPIO_READY_MODEL GPIO_LOOPBACK ESDHC_EMMC ESDHC_FILE=out/run-card.img \
  INTC1_BOOTSTRAP SSI_AUDIO_IRQ=100000 DTIM3_BOOTSTRAP \
  DTIM3_TICKS_PER_INSTRUCTION=16 \
  SNAPSHOT_LOAD="$snapshot" SNAPSHOT_SAVE="$out_prefix.snap" \
  UART8_TX_FILE="$out_prefix.tx" UART8_RX_ONCE \
  "UART8_RX_PERIODIC=$period:$messages" \
  >"$out_prefix.log" 2>&1
if ! grep -q '^stop: instruction limit$' "$out_prefix.log"; then
  echo "UI recipe did not reach its instruction limit; see $out_prefix.log" >&2
  exit 1
fi

python3 tools/render-uart-frames.py --base "$base_png" \
  "$out_prefix.tx" "$out_prefix-frame"
actual=$(sha256sum "$out_prefix-frame-final.png" | cut -d' ' -f1)
if [[ "$actual" != "$expected" ]]; then
  echo "FAIL $out_prefix-frame-final.png: expected $expected, got $actual" >&2
  exit 1
fi
echo "PASS $recipe screen sha256=$actual"
