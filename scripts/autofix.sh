#!/usr/bin/env bash
# Every mechanical fix `make check` would otherwise fail on. Run this before
# `make check`; what survives it needs a human decision.
#
# Not everything a gate rejects is fixable here: clippy --fix applies only the
# lints rustc marks machine-applicable, and markdownlint --fix skips rules with
# no single correct repair (MD024 duplicate headings, for one). A green autofix
# is not a green check.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

# --fix before the formatters: rewritten code still has to end up rustfmt-clean.
echo "==> clippy --fix"
cargo clippy --fix --workspace --all-targets --allow-dirty --allow-staged

echo "==> rustfmt"
cargo fmt --all

echo "==> typos"
if command -v typos >/dev/null 2>&1; then
  # Exits non-zero when corrections remain ambiguous; those are for a human.
  typos --write-changes || echo "typos left corrections it could not make unambiguously"
else
  echo "typos not installed; skip (install: cargo install typos-cli)"
fi

echo "==> markdownlint --fix"
npx --yes markdownlint-cli2 --fix || echo "markdownlint left unfixable issues; run make md-lint"

echo "==> vscode extension"
(
  cd extensions/vscode
  npm install --silent
  # Through the package's own script: it pins eslint 8 and sets
  # ESLINT_USE_FLAT_CONFIG=false, which a bare `npx eslint` loses — npx would
  # resolve the platform root's eslint 9 and fail loading the TS rules.
  npm run lint:fix
  npm run format
)

echo "Autofix complete. Run 'make check' to see what is left."
