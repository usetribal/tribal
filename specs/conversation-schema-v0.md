# Conversation Schema v0

Stable contract for agent session data stored in lineage.

## Session

```json
{
  "schema_version": "conversation-v0",
  "id": "01HQZX8K9V2M3N4P5Q6R7S8T9U",
  "agent": "cursor",
  "started_at": "2026-06-06T10:00:00Z",
  "ended_at": "2026-06-06T10:45:00Z",
  "workspace_root": "/Users/dev/myproject",
  "parent_session_id": null,
  "private": false,
  "turns": [],
  "commit_shas": ["abc123def456"],
  "metadata": {}
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `schema_version` | string | yes | Always `conversation-v0` |
| `id` | string | yes | ULID, stable across re-ingest |
| `agent` | string | yes | `cursor`, `claude`, or `codex` |
| `started_at` | ISO8601 | yes | Session start |
| `ended_at` | ISO8601 | no | Session end |
| `workspace_root` | string | yes | Absolute path at ingest time |
| `parent_session_id` | string | no | Forked session |
| `private` | bool | no | Excluded from export by default |
| `turns` | Turn[] | yes | Ordered conversation turns |
| `commit_shas` | string[] | no | Linked git commits |
| `metadata` | object | no | Adapter-specific extras (e.g. `model`, `models_used`, `claude_code_version`, `codex_cli_version`) |

## Turn

```json
{
  "id": "01HQZX8K9V2M3N4P5Q6R7S8T9V",
  "role": "user",
  "content": "Add authentication middleware",
  "tool_calls": [],
  "model": "claude-sonnet-4",
  "timestamp": "2026-06-06T10:01:00Z"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | string | yes | ULID |
| `role` | string | yes | `user`, `assistant`, `system`, `tool` |
| `content` | string | yes | Message body (may be redacted) |
| `tool_calls` | ToolCall[] | no | Tool invocations |
| `model` | string | no | Model identifier |
| `timestamp` | ISO8601 | no | Turn timestamp |

## ToolCall

```json
{
  "id": "tc-001",
  "name": "edit_file",
  "arguments": "{\"path\":\"src/auth.rs\"}",
  "result": "ok"
}
```

## Artifact

File edits and diffs are referenced by hash, not always inlined:

```json
{
  "kind": "file_edit",
  "path": "src/auth.rs",
  "blob_ref": "sha256:abc...",
  "line_range": [1, 42]
}
```

## ID stability

Session IDs are derived from `(agent, source_path, started_at)` at first ingest. Re-ingest updates content but preserves ID when the source key matches.
