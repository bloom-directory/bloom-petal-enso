#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PETAL_REV="4fefa736df67fa1ee51aeaa0c749d2f11414257d"

if [[ "${PETAL_COMPILE_TIME_SECRET+x}" == "x" ]]; then
  if [[ -z "$PETAL_COMPILE_TIME_SECRET" ]]; then
    echo "PETAL_COMPILE_TIME_SECRET is not configured" >&2
    exit 1
  fi
  export ENSO_API_KEY="$PETAL_COMPILE_TIME_SECRET"
  unset PETAL_COMPILE_TIME_SECRET
else
  # The route embeds ENSO_API_KEY at compile time. Only the release secret
  # above may supply it, never a variable left in a developer's shell.
  unset ENSO_API_KEY
fi

# Bloom materializes composed route artifacts here after package validation.
# They are derived from the route sources and must not survive a source
# rebuild, or the next `bloom petals build/install` rejects the stale bytes
# before it gets a chance to regenerate them.
rm -rf -- "$ROOT/artifacts"

# Build with an explicitly supplied CLI, otherwise with the one pinned at
# PETAL_REV, never whichever `petal` happens to be first on PATH.
if [[ -n "${PETAL_BIN:-}" ]]; then
  petal_bin="$PETAL_BIN"
else
  tool_root="$ROOT/target/petal-tool"
  cargo install \
    --git https://github.com/bloom-directory/petal \
    --rev "$PETAL_REV" \
    --locked \
    --root "$tool_root" \
    bloom-petal-cli
  petal_bin="$tool_root/bin/petal"
fi
"$petal_bin" build --root "$ROOT"
