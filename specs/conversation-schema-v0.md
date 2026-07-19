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
| `id` | string | yes | ULID, stable across re-import |
| `agent` | string | yes | `cursor`, `claude`, or `codex` |
| `started_at` | ISO8601 | yes | Session start |
| `ended_at` | ISO8601 | no | Session end |
| `workspace_root` | string | yes | Absolute path at import time |
| `parent_session_id` | string | no | Forked session |
| `private` | bool | no | Excluded from export by default |
| `turns` | Turn[] | yes | Ordered conversation turns |
| `commit_shas` | string[] | no | Linked git commits |
| `metadata` | object | no | Adapter-specific extras (e.g. `model`, `models_used`, `prompted_by_email`, `prompted_by_name`, `claude_code_version`, `codex_cli_version`) |

### Author metadata

Set at first import from the repository git config (`user.email`, `user.name`). Preserved on re-import so the original prompter is kept when sessions are refreshed by someone else.

| Key | Description |
|-----|-------------|
| `prompted_by_email` | Git email of the developer who had the agent conversation |
| `prompted_by_name` | Git name of the developer who had the agent conversation |

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
  "line_range": [1, 42],
  "resolve": {
    "strategy": "old_string",
    "old_string": "fn login() {}",
    "new_string": "fn login(user: &User) {}"
  }
}
```

`resolve` carries what line-object materialization needs to locate the edit in
a committed tree. For `old_string`-strategy edits, `new_string` (optional; the
post-edit text) is the primary anchor — it is what actually exists in the file
after the edit — with `old_string` as the fallback for transcripts captured
before `new_string` existed. `full_file` and `diff_hunk` strategies carry no
strings (whole-file heuristic; unified-diff hunk headers respectively).

Artifacts represent *produced* output. Read-style tool invocations (read,
grep, glob, search, view) produce **no artifact** — the files they touched
remain observable through the turn's `tool_calls`, but they are not edits and
must not be counted as authorship.

## ID stability

Session IDs are derived from `(agent, source_path, started_at)` at first import. Re-import updates content but preserves ID when the source key matches.
