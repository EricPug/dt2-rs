#!/bin/bash
set -euo pipefail
if [[ ${1:-} == --messages ]]; then
  exec "$(dirname "$0")/ui-recipe.sh" load-sample --messages
fi
exec "$(dirname "$0")/ui-recipe.sh" load-sample "$@"
