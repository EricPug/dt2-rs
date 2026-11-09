#!/bin/bash
set -euo pipefail
recipe=encoder
if [[ ${1:-} == --tune-16-one-count-steps ]]; then
  recipe=encoder-tune-16
  shift
fi
exec "$(dirname "$0")/ui-recipe.sh" "$recipe" "$@"
