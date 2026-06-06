# Contributing to Lineage

Thank you for your interest in contributing! Lineage is an open-source project and we welcome bug reports, feature requests, documentation improvements, and code contributions.

## Getting started

### Prerequisites

- Rust (stable, 1.86+; MSRV enforced in CI)
- Git
- For VS Code extension work: Node.js 20+

### Setup

```bash
git clone https://github.com/lineage-dev/lineage.git
cd lineage
cargo build
cargo test --workspace

# Git hooks: format + lint on commit (Rust + VS Code extension when staged)
make install-hooks

# Optional: full pre-commit framework (typos, markdownlint, all files)
pip install pre-commit   # or: brew install pre-commit
make pre-commit
```

Use `make help` for common tasks (`make check` runs the full local gate).

### Project layout

| Path | Purpose |
|------|---------|
| `crates/lineage-core` | Domain types — start here for schema changes |
| `crates/lineage-git` | Git persistence layer |
| `crates/lineage-adapters` | Agent source adapters |
| `crates/lineage-cli` | CLI binary (`git-lineage`) |
| `specs/` | Schema contracts (update before changing types) |
| `tests/fixtures/` | Golden files for adapter and git tests |
| `extensions/vscode/` | VS Code extension |
| `.cursor/skills/` | Cursor agent skills (see [AGENTS.md](AGENTS.md)) |

## How to contribute

### Reporting bugs

Open a [bug report](https://github.com/lineage-dev/lineage/issues/new?template=bug_report.yml) with:

- Steps to reproduce
- Expected vs actual behavior
- Rust version (`rustc --version`) and OS
- Relevant log output (`RUST_LOG=debug git lineage ...`)

**Do not** include agent conversation content that may contain secrets.

### Suggesting features

Open a [feature request](https://github.com/lineage-dev/lineage/issues/new?template=feature_request.yml). For large changes, open an issue for discussion before submitting a PR.

### Pull requests

1. Fork the repository and create a branch from `main`
2. Make your changes
3. Add or update tests
4. Run the full check suite locally:

   ```bash
   make check
   ```

   Or step by step:

   ```bash
   cargo fmt --all
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   ./scripts/coverage.sh
   cargo doc --workspace --no-deps --document-private-items
   make msrv
   ```

5. Update [CHANGELOG.md](CHANGELOG.md) under `[Unreleased]`
6. Open a PR with a clear description of what and why

### Commit messages

Use clear, imperative subject lines:

```text
Add Codex adapter support for JSONL sessions
Fix blame resolution for paths with colons
```

Reference issue numbers when applicable: `Fix blame for Windows paths (#42)`.

## Code guidelines

- **Minimize scope** — focused PRs are easier to review
- **Specs first** — if you change domain types, update the relevant file in `specs/` first
- **Match conventions** — follow existing naming, error handling (`thiserror`), and crate boundaries
- **Policy before persist** — never write unredacted secrets to git objects
- **Tests** — adapter changes need fixture-based tests; git changes need integration tests

## Adding a new agent adapter

1. Add a module under `crates/lineage-adapters/src/`
2. Implement `AgentSource` and `SessionReader` from `lineage-agent`
3. Map vendor format → `lineage_core::Conversation`
4. Add a feature flag in `lineage-adapters/Cargo.toml` (e.g. `adapter-foo`)
5. Register in `all_adapters()` in `lib.rs`
6. Add fixture files under `tests/fixtures/` and a golden test

## Schema changes

Schema versions are explicit (`conversation-v0`, etc.). Breaking changes require a new schema version and migration notes in `specs/` and `CHANGELOG.md`.

## Community

Be respectful and constructive. See [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).

## Questions?

Open a [discussion](https://github.com/lineage-dev/lineage/discussions) or issue if something is unclear. We're happy to help you find a good first contribution.
