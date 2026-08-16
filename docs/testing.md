# Testing

[← Documentation index](README.md) · [Developing](developing.md) · [Adapters](adapters.md)

Tribal uses Rust integration tests, fixture-based golden tests, and TypeScript checks for the extension. CI runs the same gates as local `make check` plus coverage and MSRV.

## Run tests

```bash
# Entire workspace
make test
cargo test --workspace

# Single crate
cargo test -p lineage-git
cargo test -p lineage-cli
cargo test -p lineage-adapters
cargo test -p lineage-mcp
cargo test -p lineage-agent

# Named integration suite
cargo test -p lineage-cli --test workflow
cargo test -p lineage-cli --test hooks_workflow
cargo test -p lineage-git --test full_workflow
cargo test -p lineage-mcp --test server
```

## Extension checks

```bash
make vscode-lint
cd extensions/vscode && npm run check
```

## Full local gate

```bash
make check
```

Runs rustfmt check, clippy with warnings denied, workspace tests, `cargo doc`, typos, markdownlint, and extension compile/lint.

## Coverage

```bash
make coverage
./scripts/coverage.sh
```

Enforces **≥80% line coverage** workspace-wide. CLI `main.rs` entrypoints are excluded from the gate. Run before opening a PR when you change logic-heavy code.

## MSRV

```bash
make msrv
./scripts/msrv.sh
```

Verifies the workspace builds and tests on Rust 1.86.

## Fixtures

| Path | Used for |
|------|----------|
| `tests/fixtures/cursor-history/` | Cursor adapter and import tests |
| `tests/fixtures/claude-code-history/` | Claude adapter parsing |
| `tests/fixtures/codex-history/` | Codex adapter parsing |
| `tests/fixtures/git-repo/` | Git integration setup patterns |

Fixtures contain sanitized transcript snippets, not real secrets. Add new fixture trees when introducing agent formats; extend `all_fixtures.rs` golden tests in `lineage-adapters`.

## Integration test patterns

**Git crates** — create a temporary directory, `git init`, configure user identity, commit seed files, then call `lineage-git` APIs or CLI commands against that repo.

**CLI** — invoke library handlers or binary via `assert_cmd` patterns in `workflow.rs`; hook tests need unrestricted filesystem access for hook installation.

**MCP** — build JSON-RPC requests, call `handle_request` with a fixture repo path, assert tool payloads.

**Search** — rebuild index in tests before asserting search hits; the index is not checked into git.

## What to test when

| Change type | Expected tests |
|-------------|----------------|
| Adapter parsing | Fixture golden test + unit edge cases |
| Git persist / notes | `lineage-git/tests/*_integration.rs` |
| CLI flag or output | `lineage-cli/tests/workflow.rs` |
| Policy rule | `lineage-policy` unit tests |
| MCP tool | `lineage-mcp/tests/server.rs` |
| Extension command | Manual F5 or future extension tests |

## Pre-commit hooks

`make install-hooks` runs format and clippy (and extension lint) on staged files at commit time. Optional `make pre-commit` runs the full pre-commit framework across all files.

## CI

GitHub Actions runs Linux and macOS test matrices, coverage, `cargo doc`, typos, and extension checks. Match CI locally with `make check` before pushing.

## Related guides

- [Developing](developing.md)
- [Adapters](adapters.md)
- [Schemas](schemas.md)
