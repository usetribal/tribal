# Agent guide

Lineage is a Rust monorepo: git-native AI agent provenance (CLI, MCP, VS Code extension).

## Skills

Project skills live in `.cursor/skills/`. Cursor loads them by description when relevant.

| Task | Skill |
|------|-------|
| Any change in this repo | [lineage-contribute](.cursor/skills/lineage-contribute/SKILL.md) |
| New agent adapter | [add-agent-adapter](.cursor/skills/add-agent-adapter/SKILL.md) |
| Schema / type change | [schema-change](.cursor/skills/schema-change/SKILL.md) |
| Git refs, notes, LFS, blame | [lineage-git-work](.cursor/skills/lineage-git-work/SKILL.md) |
| `git lineage` command | [lineage-cli-command](.cursor/skills/lineage-cli-command/SKILL.md) |
| VS Code extension | [vscode-extension-dev](.cursor/skills/vscode-extension-dev/SKILL.md) |
| MCP server | [lineage-mcp-server](.cursor/skills/lineage-mcp-server/SKILL.md) |
| Release / merge prep | [lineage-release-prep](.cursor/skills/lineage-release-prep/SKILL.md) |

## Quick commands

```bash
make setup                   # install + configure this repo
make check                   # full contributor gate
make coverage                # 80% line coverage
```

## Docs

- [docs/README.md](docs/README.md) — documentation index
- [README.md](README.md): setup and troubleshooting
- [docs/ingest.md](docs/ingest.md), [docs/explore.md](docs/explore.md), [docs/share.md](docs/share.md): day-to-day usage
- [docs/cli/README.md](docs/cli/README.md) — CLI and agent skill
- [docs/mcp/README.md](docs/mcp/README.md) — MCP server
- [extensions/vscode/README.md](extensions/vscode/README.md) — VS Code extension
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — crates and data flow
- [CONTRIBUTING.md](CONTRIBUTING.md) — human contributor guide
- [specs/](specs/) — schema contracts
- [tests/fixtures/](tests/fixtures/) — adapter and git test fixtures

## Rules

1. Minimize scope — focused diffs
2. Specs before types
3. Policy before persist — no unredacted secrets in git objects
4. Tests + coverage for behavior changes
