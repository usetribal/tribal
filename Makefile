.PHONY: help setup fmt clippy test coverage doc autofix check msrv vscode vscode-lint vscode-fmt md-lint typos pre-commit install-hooks

# Optional setup flags (e.g. make setup REPO=/path/to/app IMPORT=1 WITH_MCP=1 FORCE_HOOKS=1)
REPO ?=
IMPORT ?=
WITH_MCP ?=
FORCE_HOOKS ?=

SETUP_FLAGS :=
ifdef IMPORT
SETUP_FLAGS += --import
endif
ifdef WITH_MCP
SETUP_FLAGS += --with-mcp
endif
ifdef FORCE_HOOKS
SETUP_FLAGS += --force-hooks
endif

help:
	@echo "Lineage targets:"
	@echo "  make setup        - one-command local dev setup (CLI, extension, git lineage init)"
	@echo "    REPO=path        target git repo (default: lineage repo root)"
	@echo "    IMPORT=1         run initial import after setup"
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
	@echo "  make autofix      - every mechanical fix (see scripts/autofix.sh); run before check"
	@echo "  make check        - full local gate (see scripts/check.sh)"
	@echo "  make pre-commit   - run pre-commit framework on all files (optional)"
	@echo "  make install-hooks - install .githooks/pre-commit (format + lint on commit)"

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

autofix:
	./scripts/autofix.sh

check:
	./scripts/check.sh

pre-commit:
	pre-commit run --all-files

install-hooks:
	chmod +x .githooks/pre-commit .githooks/post-commit
	git config core.hooksPath .githooks
