#!/bin/bash
set -euo pipefail
if [[ ${1:-} == --messages ]]; then
  shift
  exec "$(dirname "$0")/ui-recipe.sh" trigger-track --messages "$@"
fi
[[ $# -gt 0 ]] || { echo "usage: $0 <trigger 1-16> <snapshot> <base.png> <out-prefix> <expected-sha256> [dt2-cli]" >&2; exit 2; }
trigger=$1
shift
exec "$(dirname "$0")/ui-recipe.sh" trigger-track "$trigger" "$@"
