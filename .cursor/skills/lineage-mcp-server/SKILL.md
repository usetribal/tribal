---
name: lineage-mcp-server
description: >-
  Changes the lineage-mcp MCP server: handle_request, JSON-RPC tools, and
  server tests. Use when modifying MCP tools, Cursor MCP integration, or
  lineage search/git exposure via MCP.
---

# Lineage MCP server

## Layout

| Path | Role |
|------|------|
| `src/server.rs` | `handle_request` — main JSON-RPC dispatch |
| `src/lib.rs` | Crate exports |
| `tests/server.rs` | Integration tests (initialize, tools, search) |

Install: `cargo install --path crates/lineage-mcp` or `./scripts/setup.sh --with-mcp`.

## Request handling

Public entry: `lineage_mcp::server::handle_request` — keep testable without spawning a process.

Tests use `serde_json::json!` requests and assert on JSON responses.

## Test pattern

```rust
let dir = init_repo();  // tempfile + git init + commit
persist_conversation(...);
let response = handle_request(&request, dir.path()).await;
```

Rebuild search index in tests before search assertions (index is rebuildable from git refs).

## Dependencies

MCP server reads via `lineage-git` + `lineage-search` — do not bypass policy/redaction paths used by CLI import.

## Verify

```bash
cargo test -p lineage-mcp
cargo test -p lineage-mcp --test server
```
