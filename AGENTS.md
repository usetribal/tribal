# Agent guide

Lineage is a Rust monorepo for **git-native AI agent provenance**: it imports sessions from Cursor, Claude Code, and Codex into git refs and notes, links them to commits and lines, and exposes that context through a CLI, MCP server, and VS Code extension. No external database — data lives in `refs/lineage/*` and `refs/notes/lineage`.

Read [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for crate boundaries and import flow before changing behavior. `lineage-core` has no `git2`; git I/O stays in `lineage-git`.

## Setup

**Prerequisites:** Rust 1.86+ (MSRV), Git 2.20+, Node.js 20+ for extension work.

```bash
make setup                   # build CLI, compile extension, git lineage init in this repo
make setup WITH_MCP=1        # also install lineage-mcp
make setup IMPORT=1          # run initial import after setup
```

Reinstall the CLI after CLI changes:

```bash
cargo install --path crates/lineage-cli
```

## Quality gates

For day-to-day work and before you commit:

```bash
make check                   # fmt, clippy, test, doc, typos, markdown, vscode lint
```

`make check` does not run coverage or MSRV — CI and PR review handle those.

Before opening a PR (or when you change behavior that needs coverage proof):

```bash
make coverage                # >=80% line coverage (main.rs excluded)
make msrv                    # verify Rust 1.86 MSRV
```

| Task | Command |
|------|---------|
| Full local gate | `make check` |
| Format Rust | `make fmt` |
| Lint Rust | `make clippy` |
| Test workspace | `make test` |
| Coverage (PR) | `make coverage` |
| MSRV (PR) | `make msrv` |
| Format/lint extension | `make vscode-fmt` / `make vscode-lint` |
| Lint markdown | `make md-lint` |
| Spell check | `make typos` |
| Git hooks (fmt + lint on commit) | `make install-hooks` |
| Optional full hook suite | `pip install pre-commit` then `make pre-commit` |

## Developing the CLI

**Crate:** `crates/lineage-cli` · **Binary:** `git-lineage` (invoked as `git lineage`)

| File | Role |
|------|------|
| `src/main.rs` | `clap` commands and dispatch |
| `src/commands.rs` | Subcommand handlers |
| `src/init_cmd.rs` | Interactive `init` wizard |
| `src/skill_cmd.rs` | `init-skill` (bundled skill install) |
| `src/hooks_cmd.rs` | Hook install/uninstall |
| `assets/hooks/` | Pre-commit / post-commit hook scripts |
| `assets/skills/lineage/` | Bundled end-user skill (installed by `init-skill`) |

```bash
cargo test -p lineage-cli
cargo test -p lineage-cli --test workflow
cargo test -p lineage-cli --test hooks_workflow
```

Integration tests use `tempfile` + `git init`. Hook tests need normal filesystem permissions. Document user-facing commands in [docs/cli/README.md](docs/cli/README.md). Update [CHANGELOG.md](CHANGELOG.md) under `[Unreleased]` for UX changes.

## Developing the MCP server

**Crate:** `crates/lineage-mcp` · **Entry:** `lineage_mcp::server::handle_request`

```bash
cargo test -p lineage-mcp
cargo test -p lineage-mcp --test server
cargo install --path crates/lineage-mcp   # or make setup WITH_MCP=1
```

Run locally:

```bash
LINEAGE_REPO=/path/to/repo lineage-mcp
```

Tools: `lineage_list_sessions`, `lineage_get_session`, `lineage_blame_line`, `lineage_search`, `lineage_doctor`, `lineage_materialize`, `lineage_rebuild_index`, `lineage_export`, `lineage_remap`. See [docs/mcp/README.md](docs/mcp/README.md). Reads through `lineage-git` + `lineage-search` — do not bypass policy/redaction.

## Developing the VS Code extension

**Path:** `extensions/vscode/` · shells out to `git lineage` via `src/lineageClient.ts`

```bash
cd extensions/vscode
npm install
npm run check        # tsc + eslint + prettier
npm run compile      # or: make vscode from repo root
npm run package      # .vsix
```

**F5 debug:** open the lineage repo root → **Lineage Extension** in `.vscode/launch.json`. `lineage.cliPath` in `.vscode/settings.json` points at `target/debug/git-lineage` (built by `make setup`).

Key sources: `extension.ts`, `sessionsProvider.ts`, `sessionPanel.ts`, `lineageDecorator.ts`, `lineageHoverProvider.ts`, `agentActions.ts`.

After CLI JSON shape changes, update `src/types.ts` and extension commands in `package.json`.

## Developing library crates

| Area | Where to work | Tests |
|------|---------------|-------|
| Schema / types | `specs/` → `lineage-core` | serde + downstream crates |
| Git persistence | `lineage-git` | `crates/lineage-git/tests/` |
| Agent adapters | `lineage-adapters` | `tests/all_fixtures.rs`, `tests/fixtures/` |
| Import pipeline | `lineage-agent` | `tests/pipeline.rs` |
| Search | `lineage-search` | crate tests + CLI/MCP integration |

```bash
cargo test -p lineage-git
cargo test -p lineage-adapters
cargo test -p lineage-core
```

## Docs

- [docs/README.md](docs/README.md) — documentation index
- [README.md](README.md) — setup and troubleshooting
- [docs/import.md](docs/import.md), [docs/explore.md](docs/explore.md), [docs/share.md](docs/share.md) — day-to-day usage
- [CONTRIBUTING.md](CONTRIBUTING.md) — human contributor guide

## Rules

1. Minimize scope — focused diffs
2. Specs before types
3. Policy before persist — no unredacted secrets in git objects
4. Add tests for behavior changes; run `make coverage` before a PR if logic changed
5. Comment the *why* in plain language, next to the code; no "comment golf" and no narrating discarded approaches
