#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BLOOM_REPO="${BLOOM_REPO:-}"

"$ROOT/scripts/check-route-architecture.sh"
cargo test --manifest-path "$ROOT/route/Cargo.toml"
"$ROOT/scripts/build.sh"

if [[ -n "${PETAL_BIN:-}" ]]; then
  "$PETAL_BIN" check --root "$ROOT"
elif command -v petal >/dev/null 2>&1; then
  petal check --root "$ROOT"
else
  "$ROOT/target/petal-tool/bin/petal" check --root "$ROOT"
fi

if [ -n "$BLOOM_REPO" ]; then
  cargo run --manifest-path "$BLOOM_REPO/Cargo.toml" -p bloom -- petals build "$ROOT"
elif command -v bloom >/dev/null 2>&1; then
  bloom petals build "$ROOT"
else
  echo "set BLOOM_REPO=/path/to/bloom or install bloom to validate the package" >&2
  exit 127
fi
