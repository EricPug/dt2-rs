#!/bin/bash
# Builds the patched Unicorn (m68k only) that the `unicorn` feature of dt2-cli links against.
#
#   DIGIEMU_PATCHES=/path/to/digiemu/patches tools/unicorn/build.sh [build-dir]
#
# Prints the UNICORN_LIB_DIR to use with `cargo build --release --features window,unicorn`.
# UNICORN_SRC overrides the upstream clone URL (a local checkout works too).
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
DEST=${1:-/tmp/dt2-unicorn}
SRC=${UNICORN_SRC:-https://github.com/unicorn-engine/unicorn}
COMMIT=8028ec436f2d9376525352dd38ed9ed6b9f6be10 # Unicorn 2.1.4
PATCHES=${DIGIEMU_PATCHES:?set DIGIEMU_PATCHES to the digiemu patches directory}

# Order matters: each patch applies on top of the previous ones.
SERIES=(
  unicorn-2.1.4-m68k-hook-ccr-sync.patch
  unicorn-2.1.4-m68k-emac-mac-load.patch
  unicorn-2.1.4-m68k-emac-modes.patch
  unicorn-2.1.4-m68k-fast-mem.patch
  unicorn-2.1.4-m68k-digikit-accel.patch
  unicorn-2.1.4-m68k-digikit-speed.patch
)

rm -rf "$DEST"
git clone --quiet "$SRC" "$DEST"
git -C "$DEST" checkout --quiet "$COMMIT"
for p in "${SERIES[@]}"; do
  git -C "$DEST" apply "$PATCHES/$p"
done
# Our additions: IRQ-unmask stop, exported counters and raw EMAC access.
git -C "$DEST" apply "$HERE/unicorn-2.1.4-m68k-dt2-irq-emac.patch"

cmake -S "$DEST" -B "$DEST/build" -DCMAKE_BUILD_TYPE=Release -DUNICORN_ARCH=m68k >/dev/null
cmake --build "$DEST/build" -j"$(nproc)" >/dev/null
nm -D "$DEST/build/libunicorn.so" | grep -q uc_digikit_set_irq_level
echo "UNICORN_LIB_DIR=$DEST/build"
