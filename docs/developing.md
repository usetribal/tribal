# Developing Tribal

[← Documentation index](README.md) · [Architecture](ARCHITECTURE.md) · [CONTRIBUTING.md](../CONTRIBUTING.md)

Guide for contributors working on the Tribal monorepo: CLI, MCP server, VS Code extension, and Rust library crates.

## Prerequisites

- Rust 1.86+ (MSRV enforced in CI)
- Git 2.20+
- Node.js 20+ for the VS Code extension
- Optional: `pre-commit` Python package for extended local hooks

## First-time setup

```bash
git clone https://github.com/usetribal/tribal.git
cd tribal
make setup
```

`make setup` builds the CLI, compiles the extension, runs `git lineage init` in the tribal repo, and installs contributor git hooks (format + lint on commit).

Optional flags:

```bash
make setup WITH_MCP=1    # also install lineage-mcp
make setup IMPORT=1      # run initial import in configured repo
```

Reinstall the CLI after changing `lineage-cli`:

```bash
cargo install --path crates/lineage-cli
```

## Project components

| Component | Location | User-facing doc |
|-----------|----------|-----------------|
| CLI | `crates/lineage-cli` | [CLI reference](cli/README.md) |
| MCP server | `crates/lineage-mcp` | [MCP](mcp/README.md) |
| VS Code extension | `extensions/vscode` | [VS Code](vscode.md) |
| Domain types | `crates/lineage-core` | [Schemas](schemas.md) |
| Git persistence | `crates/lineage-git` | [Architecture](ARCHITECTURE.md) |
| Adapters | `crates/lineage-adapters` | [Adapters](adapters.md) |
| Policy | `crates/lineage-policy` | [Privacy](privacy.md) |
| Search index | `crates/lineage-search` | [Explore](explore.md) |

Dependency rule: `lineage-core` has no direct git dependency. Git operations live in `lineage-git`. Policy runs before any persist path writes conversation blobs.

## Quality gates

### Before each commit

Via `make install-hooks`: `cargo fmt` and `clippy` when Rust files are staged; Prettier and ESLint when extension files are staged.

### Day-to-day

```bash
make check          # fmt, clippy, test, doc, typos, markdown, vscode lint
make test           # workspace tests
make fmt            # write rustfmt
make clippy         # Rust lint
make vscode-lint    # extension lint
make vscode-fmt     # extension format
```

### Before opening a PR

```bash
make coverage       # >=80% line coverage gate
make msrv           # verify Rust 1.86
```

Optional extended hooks:

```bash
pip install pre-commit
make pre-commit     # typos, markdownlint, all files
```

## Working on the CLI

Handlers live in `crates/lineage-cli/src/`. Integration tests use temporary git repositories under `crates/lineage-cli/tests/`.

```bash
cargo test -p lineage-cli
cargo test -p lineage-cli --test workflow
```

Document user-visible changes in [CLI reference](cli/README.md) and [CHANGELOG.md](../CHANGELOG.md).

## Working on the MCP server

```bash
cargo test -p lineage-mcp
LINEAGE_REPO=/path/to/repo lineage-mcp
```

Keep tool responses aligned with CLI JSON shapes. MCP does not yet expose import, delete, or gc (see roadmap in README).

## Working on the extension

```bash
cd extensions/vscode
npm run check
```

Press **F5** with the tribal repo open. CLI JSON changes require updating extension TypeScript types and command registrations.

## Contributor skills

Project agent skills under `.cursor/skills/`, `.agents/skills/`, and `.claude/skills/` are synced via `./scripts/sync-skills.sh`. Edit under `.cursor/skills/` first.

Human and agent entry points: [AGENTS.md](../AGENTS.md), [CLAUDE.md](../CLAUDE.md).

## Pull requests

1. Focused diff; specs before type changes.
2. Tests for behavior changes ([Testing](testing.md)).
3. `CHANGELOG.md` under `[Unreleased]` for user-facing work.
4. No secrets or raw agent transcripts in commits.

Full process: [CONTRIBUTING.md](../CONTRIBUTING.md).

## Related guides

- [Architecture](ARCHITECTURE.md)
- [Testing](testing.md)
- [Adapters](adapters.md)
