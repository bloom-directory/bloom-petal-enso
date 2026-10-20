#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# A missing `rg` would make every pattern check below look like "no match".
if ! command -v rg >/dev/null 2>&1; then
  echo "ripgrep (rg) is required to check the route architecture; install it and retry" >&2
  exit 1
fi

if rg -n   'session_view|session_route|pub fn [a-zA-Z0-9_]+_route[[:space:]]*\('   "$ROOT/route/src" "$ROOT/route/files"; then
  echo "route-facing dispatch facade found; route composition belongs in route/files" >&2
  exit 1
fi

if rg -n   'crate::(workflow::)?[a-zA-Z0-9_]+_route[[:space:]]*\('   "$ROOT/route/files"; then
  echo "route file delegates to a route-mirroring facade" >&2
  exit 1
fi

route_count="$(
  find "$ROOT/route/files" -type f -name '*.rs' | wc -l | tr -d ' '
)"
if [[ "$route_count" != "31" ]]; then
  echo "expected 31 file-based route controllers, found $route_count" >&2
  exit 1
fi

echo "checked 31 file-based route controllers"
