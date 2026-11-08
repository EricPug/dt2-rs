#!/bin/bash
set -euo pipefail
[[ $# -gt 0 ]] || { echo "usage: $0 <page> <snapshot> <base.png> <out-prefix> <expected-sha256> [dt2-cli]" >&2; exit 2; }
page=$1
shift
exec "$(dirname "$0")/ui-recipe.sh" page "$page" "$@"
