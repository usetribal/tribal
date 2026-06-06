#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

echo "==> rustfmt"
cargo fmt --all -- --check

echo "==> clippy"
cargo clippy --workspace --all-targets -- -D warnings

echo "==> test"
cargo test --workspace

echo "==> doc"
cargo doc --workspace --no-deps --document-private-items

echo "==> typos"
if command -v typos >/dev/null 2>&1; then
  typos
else
  echo "typos not installed; skip (install: cargo install typos-cli)"
fi

echo "==> markdownlint"
npx --yes markdownlint-cli2

echo "==> vscode extension"
(
  cd extensions/vscode
  npm install --silent
  npm run check
)

echo "All checks passed."
