---
name: lineage-contribute
description: >-
  Lineage repo contribution conventions: make check, 80% coverage gate, MSRV
  1.86, specs-first schema work, policy-before-persist, crate boundaries, and
  CHANGELOG updates. Use when changing any code or docs in the lineage
  monorepo, reviewing PRs, or fixing CI in this repository.
---

# Lineage contribute

## Before coding

1. Read `docs/ARCHITECTURE.md` for crate boundaries and data flow.
2. Identify the right crate — do not leak `git2` into `lineage-core`.
3. **Policy before persist** — never write unredacted secrets to git objects.

## Local gates

```bash
make check              # fmt, clippy, test, doc, typos, markdown, vscode
./scripts/coverage.sh   # >=80% line coverage (main.rs excluded)
make msrv               # Rust 1.86
```

Individual targets: `make fmt`, `make clippy`, `make test`, `make vscode-lint`.

Clippy must pass with `--all-targets` and `-D warnings`.

## Crate map

| Path | Role |
|------|------|
| `crates/lineage-core` | Domain types — see `schema-change` skill |
| `crates/lineage-policy` | Redaction, excludes, private sessions |
| `crates/lineage-git` | Git refs, notes, LFS, hooks, blame, GC |
| `crates/lineage-agent` | `AgentSource`, `SessionReader`, import pipeline |
| `crates/lineage-adapters` | Vendor adapters — see `add-agent-adapter` skill |
| `crates/lineage-store` | Blob/filesystem storage |
| `crates/lineage-search` | Rebuildable SQLite FTS index |
| `crates/lineage-cli` | `git-lineage` — see `lineage-cli-command` skill |
| `crates/lineage-mcp` | MCP server — see `lineage-mcp-server` skill |
| `extensions/vscode/` | VS Code UI — see `vscode-extension-dev` skill |
| `specs/` | Schema contracts (source of truth) |

## PR checklist

- [ ] `make check` and `./scripts/coverage.sh` pass
- [ ] `CHANGELOG.md` updated under `[Unreleased]` if user-facing
- [ ] `specs/` updated if schema/types changed
- [ ] No secrets or private conversation content in commits
- [ ] Tests match change type: fixtures for adapters, integration tests for git/CLI

## Specialized skills

| Task | Skill |
|------|-------|
| New agent adapter | `add-agent-adapter` |
| Schema/type change | `schema-change` |
| Git persistence | `lineage-git-work` |
| CLI subcommand | `lineage-cli-command` |
| VS Code extension | `vscode-extension-dev` |
| MCP server | `lineage-mcp-server` |
| Release prep | `lineage-release-prep` |
