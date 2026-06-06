# MCP server

[← Back to README](../../README.md) · [Setup](../../README.md#setup)

Expose lineage data to AI tools via the [Model Context Protocol](https://modelcontextprotocol.io/).

## Install

```bash
cargo install --path crates/lineage-mcp
# or: make setup WITH_MCP=1
```

## Run

```bash
LINEAGE_REPO=/path/to/your/repo lineage-mcp
```

The server reads lineage refs from `LINEAGE_REPO` (defaults to the current working directory).

## Tools

| Tool | Description |
|------|-------------|
| `lineage_list_sessions` | List all ingested sessions |
| `lineage_get_session` | Fetch a session by ID (redacted by default) |
| `lineage_blame_line` | Get lineage for a file and line number |
| `lineage_search` | Full-text search over sessions |
| `lineage_doctor` | Check repository lineage health |
| `lineage_materialize` | Materialize line objects at HEAD or a commit |
| `lineage_rebuild_index` | Rebuild the local search index |
| `lineage_export` | Export sessions (optionally redacted) |
| `lineage_remap` | Remap lineage after rebase |

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

## Claude Code / Codex

Point your MCP client at the `lineage-mcp` binary with `LINEAGE_REPO` set to the git repository root you want to query.
