#!/usr/bin/env bash
# scripts/test.sh — Run the full yMux test suite.
# Safe on Linux (no GTK/Tauri desktop deps).
# Exit on first failure.
set -euo pipefail

echo "=== cargo fmt check ==="
cargo fmt --all --check

echo ""
echo "=== TypeScript typecheck ==="
npx tsc --noEmit

echo ""
echo "=== vitest (frontend unit tests) ==="
pnpm exec vitest run

echo ""
echo "=== cargo clippy (tools + shared crates) ==="
cargo clippy -p ytheme -p yipc -p ymon -p ydir -p ycode -p ylauncher -- -D warnings

echo ""
echo "=== cargo clippy (ymux lib, no desktop) ==="
cargo clippy --no-default-features --lib --tests -p ymux -- -D warnings

echo ""
echo "=== cargo test (tools + shared crates) ==="
cargo test -p ytheme -p yipc -p ymon -p ydir -p ycode -p ylauncher

echo ""
echo "=== cargo test (ymux lib, no desktop) ==="
cargo test --no-default-features --lib -p ymux

echo ""
echo "✓ All checks passed."
