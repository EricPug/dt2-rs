#!/bin/bash
# usage: probe-from-snapshot.sh <name> <instruction-limit> [extra args]; restores $SNAP (default out/idle2.snap) and logs to out/probe-<name>.log.
name=$1; limit=$2; shift 2
cd "$(dirname "$0")/.." || exit 1
tools/run-section3.sh "${BIN:-target/release/dt2-cli}" "$limit" UART8_TX SCHED_TRACE "SNAPSHOT_LOAD=${SNAP:-out/idle2.snap}" "$@" > "out/probe-$name.log" 2>&1
echo "== $name"
grep -a "^restored\|^stop\|^executed\|captured\|^PC=" "out/probe-$name.log" | cut -c1-150
