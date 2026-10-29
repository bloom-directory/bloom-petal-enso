#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BLOOM_REPO="${BLOOM_REPO:-}"

"$ROOT/scripts/check-route-architecture.sh"
cargo test --manifest-path "$ROOT/route/Cargo.toml"
"$ROOT/scripts/build.sh"

# Check with the same CLI build.sh used.
"${PETAL_BIN:-$ROOT/target/petal-tool/bin/petal}" check --root "$ROOT"

if [ -n "$BLOOM_REPO" ]; then
  cargo run --manifest-path "$BLOOM_REPO/Cargo.toml" -p bloom -- petals build "$ROOT"
elif command -v bloom >/dev/null 2>&1; then
  bloom petals build "$ROOT"
else
  echo "set BLOOM_REPO=/path/to/bloom or install bloom to validate the package" >&2
  exit 127
fi
