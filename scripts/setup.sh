#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export PATH="${HOME}/.cargo/bin:${PATH}"

WITH_MCP=false
INGEST=false
FORCE_HOOKS=false
REPO_PATH=""

usage() {
  cat <<'EOF'
Install Lineage and configure a git repository.

Usage:
  ./scripts/setup.sh [options] [REPO_PATH]

Options:
  --ingest       Run initial ingest after setup
  --with-mcp     Also install the lineage-mcp server
  --force-hooks  Overwrite existing pre-commit/post-commit hooks
  -h, --help     Show this help

Examples:
  ./scripts/setup.sh                          # configure lineage repo root
  ./scripts/setup.sh /path/to/your-project    # configure another repo
  ./scripts/setup.sh --ingest /path/to/your-project
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --with-mcp) WITH_MCP=true; shift ;;
    --ingest) INGEST=true; shift ;;
    --force-hooks) FORCE_HOOKS=true; shift ;;
    -h | --help)
      usage
      exit 0
      ;;
    -*)
      echo "error: unknown option: $1" >&2
      usage >&2
      exit 1
      ;;
    *)
      REPO_PATH="$1"
      shift
      ;;
  esac
done

if ! command -v cargo >/dev/null 2>&1; then
  echo "error: Rust is required. Install from https://rustup.rs" >&2
  exit 1
fi

if ! command -v git >/dev/null 2>&1; then
  echo "error: git is required" >&2
  exit 1
fi

if ! command -v npm >/dev/null 2>&1; then
  echo "error: Node.js/npm is required for the VS Code extension build" >&2
  exit 1
fi

echo "==> Installing git-lineage CLI"
cargo install --path "${ROOT}/crates/lineage-cli"

if [[ "${WITH_MCP}" == "true" ]]; then
  echo "==> Installing lineage-mcp server"
  cargo install --path "${ROOT}/crates/lineage-mcp"
fi

echo "==> Building CLI debug binary (VS Code extension dev)"
cargo build -p lineage-cli --quiet

echo "==> Building VS Code extension"
(
  cd "${ROOT}/extensions/vscode"
  npm install --silent
  npm run compile
)

if [[ -z "${REPO_PATH}" ]]; then
  REPO_PATH="${ROOT}"
fi

REPO_PATH="$(cd "${REPO_PATH}" && pwd)"

if [[ "${REPO_PATH}" == "${ROOT}" ]] && [[ ! -d "${ROOT}/.git" ]]; then
  echo "==> Initializing git repository at lineage root"
  git -C "${ROOT}" init -b main
fi

if ! git -C "${REPO_PATH}" rev-parse --git-dir >/dev/null 2>&1; then
  echo "error: ${REPO_PATH} is not a git repository" >&2
  exit 1
fi

echo "==> Configuring repository: ${REPO_PATH}"

git -C "${REPO_PATH}" lineage init-config
git -C "${REPO_PATH}" lineage init-skill
git -C "${REPO_PATH}" lineage doctor

if [[ "${FORCE_HOOKS}" == "true" ]]; then
  git -C "${REPO_PATH}" lineage install-hook --force
else
  git -C "${REPO_PATH}" lineage install-hook
fi

if [[ "${INGEST}" == "true" ]]; then
  echo "==> Running initial ingest"
  git -C "${REPO_PATH}" lineage ingest --agent all --incremental || true
fi

CLI_PATH="$(command -v git-lineage || echo "${HOME}/.cargo/bin/git-lineage")"

cat <<EOF

Setup complete.

  CLI:          ${CLI_PATH}
  Repository:   ${REPO_PATH}
  Hooks:        pre-commit (incremental ingest), post-commit (link sessions)

Next steps:
  cd ${REPO_PATH}
  git lineage ingest --agent all
  git lineage list
  git lineage blame <file>:<line>

VS Code extension dev:
  Open the lineage repo in VS Code and press F5.

EOF
