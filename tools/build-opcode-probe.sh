#!/bin/bash
# usage: build-opcode-probe.sh [scratch dir]; builds the opcode-recording dt2-cli and the decode_probe example from the current tree (outputs under <scratch>/target).
repo="$(cd "$(dirname "$0")/.." && pwd)"
scratch=${1:-/tmp/dt2-ops}
rm -rf "$scratch" && mkdir -p "$scratch"
(cd "$repo" && tar cf - --exclude=./target --exclude=./out --exclude=./.git .) | tar xf - -C "$scratch"
python3 "$repo/tools/opcode_probe_patch.py" "$scratch"
mkdir -p "$scratch/crates/dt2-core/examples" && cp "$repo/tools/decode_probe.rs" "$scratch/crates/dt2-core/examples/"
cd "$scratch" && CARGO_TARGET_DIR="$scratch/target" cargo build --release -p dt2-cli && CARGO_TARGET_DIR="$scratch/target" cargo build --release -p dt2-core --example decode_probe
