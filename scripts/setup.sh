#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export PATH="${HOME}/.cargo/bin:${PATH}"

WITH_MCP=false
IMPORT=false
FORCE_HOOKS=false
REPO_PATH=""

usage() {
  cat <<'EOF'
Install Lineage and configure a git repository.

Usage:
  ./scripts/setup.sh [options] [REPO_PATH]

Options:
  --import       Run initial import after setup
  --with-mcp     Also install the lineage-mcp server
  --force-hooks  Overwrite existing pre-commit/post-commit hooks
  -h, --help     Show this help

Examples:
  ./scripts/setup.sh                          # configure lineage repo root
  ./scripts/setup.sh /path/to/your-project    # configure another repo
  ./scripts/setup.sh --import /path/to/your-project
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --with-mcp) WITH_MCP=true; shift ;;
    --import) IMPORT=true; shift ;;
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

INIT_ARGS=(--yes)
if [[ "${FORCE_HOOKS}" == "true" ]]; then
  INIT_ARGS+=(--force-hooks)
fi
if [[ "${IMPORT}" != "true" ]]; then
  INIT_ARGS+=(--no-import)
fi
git -C "${REPO_PATH}" lineage init "${INIT_ARGS[@]}"

if [[ "${REPO_PATH}" == "${ROOT}" ]]; then
  echo "==> Installing contributor git hooks (.githooks: format + lint on commit)"
  chmod +x "${ROOT}/.githooks/pre-commit" "${ROOT}/.githooks/post-commit"
  git -C "${ROOT}" config core.hooksPath .githooks
fi

CLI_PATH="$(command -v git-lineage || echo "${HOME}/.cargo/bin/git-lineage")"

HOOKS_MSG="pre-commit (incremental import), post-commit (link sessions)"
if [[ "${REPO_PATH}" == "${ROOT}" ]]; then
  HOOKS_MSG="pre-commit (fmt + clippy + vscode lint), post-commit (link sessions)"
fi

cat <<EOF

Setup complete.

  CLI:          ${CLI_PATH}
  Repository:   ${REPO_PATH}
  Hooks:        ${HOOKS_MSG}

Next steps:
  cd ${REPO_PATH}
  git lineage import --agent all
  git lineage list
  git lineage blame <file>:<line>

VS Code extension dev:
  Open the lineage repo in VS Code and press F5.

EOF
