# MCP server

[← Documentation index](../README.md) · [CLI reference](../cli/README.md) · [Developing](../developing.md)

The Lineage MCP server exposes repository session data to AI tools via the [Model Context Protocol](https://modelcontextprotocol.io/). Agents can search, blame, and inspect conversations without shelling out manually.

## Install

```bash
cargo install --path crates/lineage-mcp
# or: make setup WITH_MCP=1
```

## Run

```bash
LINEAGE_REPO=/path/to/your/repo lineage-mcp
```

`LINEAGE_REPO` defaults to the current working directory. Point it at the git root that contains `refs/lineage/*`.

The server reads through the same git and policy layers as the CLI. It does not bypass redaction.

## Tools

| Tool | Description |
|------|-------------|
| `lineage_list_sessions` | List imported sessions |
| `lineage_get_session` | Fetch session by id (redacted by policy) |
| `lineage_blame_line` | Lineage for file path and line number |
| `lineage_search` | Full-text search over session content |
| `lineage_doctor` | Repository lineage health |
| `lineage_materialize` | Materialize line objects at HEAD or a commit |
| `lineage_rebuild_index` | Rebuild local search index from refs |
| `lineage_export` | Export sessions with optional redaction |
| `lineage_remap` | Remap notes after rebase |

Not yet exposed via MCP: import, delete, gc (see project roadmap).

## Cursor configuration

```json
{
  "mcpServers": {
    "lineage": {
      "command": "lineage-mcp",
      "env": {
        "LINEAGE_REPO": "${workspaceFolder}"
      }
    }
  }
}
```

Restart the editor after installing the binary.

## Claude Code and Codex

Register `lineage-mcp` as an MCP server in your client configuration. Set `LINEAGE_REPO` to the repository you want agents to query.

## Agent workflow tips

- Prefer `lineage_search` before `lineage_get_session` to limit context size.
- Use `lineage_blame_line` on code under edit to load decision context.
- Run `lineage_remap` after the user rebases.
- Call `lineage_doctor` when tools return empty results unexpectedly.

## Development

```bash
cargo test -p lineage-mcp
cargo test -p lineage-mcp --test server
```

New tools should mirror CLI behavior and respect policy. Update this doc and [CLI reference](../cli/README.md) when adding capabilities.

## Related guides

- [Explore](../explore.md)
- [Privacy](../privacy.md)
- [Architecture](../ARCHITECTURE.md)
