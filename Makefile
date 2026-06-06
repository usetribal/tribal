.PHONY: help setup fmt clippy test coverage doc check msrv vscode vscode-lint vscode-fmt md-lint typos pre-commit install-hooks

# Optional setup flags (e.g. make setup REPO=/path/to/app INGEST=1 WITH_MCP=1 FORCE_HOOKS=1)
REPO ?=
INGEST ?=
WITH_MCP ?=
FORCE_HOOKS ?=

SETUP_FLAGS :=
ifdef INGEST
SETUP_FLAGS += --ingest
endif
ifdef WITH_MCP
SETUP_FLAGS += --with-mcp
endif
ifdef FORCE_HOOKS
SETUP_FLAGS += --force-hooks
endif

help:
	@echo "Lineage targets:"
	@echo "  make setup        - one-command local dev setup (CLI, extension, config, hooks)"
	@echo "    REPO=path        target git repo (default: lineage repo root)"
	@echo "    INGEST=1         run initial ingest after setup"
	@echo "    WITH_MCP=1       also install lineage-mcp"
	@echo "    FORCE_HOOKS=1    overwrite existing git hooks"
	@echo ""
	@echo "Developer targets:"
	@echo "  make fmt          - rustfmt (write)"
	@echo "  make clippy       - clippy with -D warnings"
	@echo "  make test         - cargo test --workspace"
	@echo "  make coverage     - llvm-cov gate (>=80% lines)"
	@echo "  make doc          - cargo doc --workspace"
	@echo "  make msrv         - test on Rust 1.86 (MSRV)"
	@echo "  make vscode       - compile VS Code extension"
	@echo "  make vscode-lint  - ESLint + Prettier check"
	@echo "  make md-lint      - markdownlint"
	@echo "  make typos        - spell check"
	@echo "  make check        - full local gate (see scripts/check.sh)"
	@echo "  make pre-commit   - run all pre-commit hooks"
	@echo "  make install-hooks - install pre-commit framework hooks (contributors)"

setup:
	./scripts/setup.sh $(SETUP_FLAGS) $(REPO)

fmt:
	cargo fmt --all

clippy:
	cargo clippy --workspace --all-targets -- -D warnings

test:
	cargo test --workspace

coverage:
	./scripts/coverage.sh

doc:
	cargo doc --workspace --no-deps --document-private-items

msrv:
	./scripts/msrv.sh

vscode:
	cd extensions/vscode && npm run compile

vscode-lint:
	cd extensions/vscode && npm run check

vscode-fmt:
	cd extensions/vscode && npm run format

md-lint:
	npx --yes markdownlint-cli2

typos:
	typos

check:
	./scripts/check.sh

pre-commit:
	pre-commit run --all-files

install-hooks:
	pre-commit install
