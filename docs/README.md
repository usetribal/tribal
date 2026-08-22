# Documentation

Tribal stores AI agent session provenance in your git repository. These guides cover setup, day-to-day use, and contributing to the project.

[← Back to README](../README.md)

## Getting started

| Guide | Description |
|-------|-------------|
| [Setup](../README.md#setup) | Install the CLI and run `git lineage init` |
| [Import](import.md) | Pull agent sessions into git refs |
| [Explore](explore.md) | List, show, blame, and search sessions |
| [Configuration](configuration.md) | Repository policy at `refs/lineage/config` |
| [Privacy and policy](privacy.md) | Redaction, private sessions, safe export |

## Working with lineage data

| Guide | Description |
|-------|-------------|
| [How it works](how-it-works.md) | Conversations, line objects, notes, and the search index |
| [Git hooks](git-hooks.md) | Automatic import and commit linking |
| [Agent paths](agent-paths.md) | Where Cursor, Claude, and Codex store transcripts |
| [Share with your team](share.md) | Push refs, notes, and LFS content |
| [Large content (LFS)](lfs.md) | Transport modes, push, fetch, and status |
| [After a rebase](rebase.md) | Remap orphaned lineage notes |
| [Maintenance](maintenance.md) | Doctor, delete, garbage collection, materialize |
| [Fork a session](fork-a-session.md) | Continue or branch agent conversations |

## Interfaces

| Guide | Description |
|-------|-------------|
| [CLI reference](cli/README.md) | Full `git lineage` command list |
| [MCP server](mcp/README.md) | Model Context Protocol tools for agents |
| [VS Code extension](vscode.md) | Panel, hover blame, gutter decorations |

## For contributors

| Guide | Description |
|-------|-------------|
| [Developing](developing.md) | Local setup, make targets, and workflow |
| [Architecture](ARCHITECTURE.md) | Crates, data flow, and design goals |
| [Testing](testing.md) | Running tests, fixtures, and coverage |
| [Schemas](schemas.md) | Domain contracts and versioning |
| [Adapters](adapters.md) | Adding support for new coding agents |
| [CONTRIBUTING.md](../CONTRIBUTING.md) | PR process and community guidelines |
