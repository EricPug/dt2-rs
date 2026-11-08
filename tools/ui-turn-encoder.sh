#!/bin/bash
set -euo pipefail
exec "$(dirname "$0")/ui-recipe.sh" encoder "$@"
