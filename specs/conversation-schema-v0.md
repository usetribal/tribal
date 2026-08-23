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
  "fork_origin": null,
  "private": false,
  "turns": [],
  "commit_shas": ["abc123def456"],
  "metadata": {}
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `schema_version` | string | yes | Always `conversation-v0` |
| `id` | string | yes | Stable across re-import; derived for an imported session, a ULID for a fork — see [ID stability](#id-stability) |
| `agent` | string | yes | `cursor`, `claude`, or `codex` |
| `started_at` | ISO8601 | yes | Session start |
| `ended_at` | ISO8601 | no | Session end |
| `workspace_root` | string | yes | Absolute path at import time |
| `parent_session_id` | string | no | Session this one descends from — a fork's source, or the parent of a harness-spawned branch |
| `fork_origin` | ForkOrigin | no | Present only on a session created by forking another (see [Fork origin](#fork-origin)) |
| `private` | bool | no | Excluded from export by default |
| `turns` | Turn[] | yes | Ordered conversation turns |
| `commit_shas` | string[] | no | Linked git commits |
| `metadata` | object | no | Adapter-specific extras (e.g. `model`, `models_used`, `prompted_by_email`, `prompted_by_name`, `claude_code_version`, `codex_cli_version`) |

### Fork origin

A fork is one developer picking up another's session and continuing it. It is a
new session id with its own turns; the source session is unchanged.

```json
{
  "source_session_id": "01HQZX8K9V2M3N4P5Q6R7S8T9U",
  "forked_session_handle": "7c1e9d02-4a3b-4f18-9c07-2e5b8a1d6f30",
  "forked_at": "2026-06-07T09:12:00Z",
  "lineage_version": "0.1.0",
  "source_tenant": null,
  "source_repo": null
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `source_session_id` | string | yes | Tribal id of the forked session; mirrors `parent_session_id` |
| `forked_session_handle` | string | yes | Vendor id minted for the forked copy — never the source session's |
| `forked_at` | ISO8601 | yes | When the fork was taken |
| `lineage_version` | string | yes | Version that wrote the edge |
| `source_tenant` | string | no | Tenant the source came from, when the fork crossed a server |
| `source_repo` | string | no | Repo the source belonged to, when known |

`parent_session_id` alone does not mean a fork: harness-spawned branches
(Claude sidechains and subagents) set it too. `fork_origin` is what distinguishes
the two, and consumers that care about the difference must read it rather than
infer from the parent.

**Attribution.** Turns recorded after a fork belong to the forker. The source
session's author is an ancestor edge, never a co-author of the forked session's
lines. Line objects materialized from a forked session bind to the fork's own
conversation id.

### Author metadata

Set at first import from the repository git config (`user.email`, `user.name`). Preserved on re-import so the original prompter is kept when sessions are refreshed by someone else.

| Key | Description |
|-----|-------------|
| `prompted_by_email` | Git email of the developer who had the agent conversation |
| `prompted_by_name` | Git name of the developer who had the agent conversation |
| `session_summary` | Human-readable title from the harness (Claude Code `"type":"summary"` entry); preferred over `architecture_summary` for display |
| `architecture_summary` | Heuristic summary generated at import when no vendor summary exists |
| `source_mtime` | Modification time of the transcript when it was last read, RFC 3339. An incremental import skips a session whose transcript has not been written since; `ended_at` cannot serve this purpose because the agent keeps writing records after the final turn's timestamp |

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

**Cursor never sets this.** Its IDE agent-transcript JSONL carries no
timestamp at any level — confirmed against every local transcript observed,
not just the absence of a field name guess. A consumer that orders or
buckets turns by `timestamp` sees every Cursor turn as absent and must fall
back to session-level `started_at`/`ended_at` (themselves derived from file
mtime for this agent) rather than treating the gap as a parsing bug.

## ToolCall

```json
{
  "id": "tc-001",
  "name": "edit_file",
  "arguments": "{\"path\":\"src/auth.rs\"}",
  "result": "ok",
  "target": { "kind": "path", "value": "src/auth.rs" }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | string | yes | Vendor call id; a `tool_result` repeats the id of the call it answers |
| `name` | string | yes | Tool name as the harness reported it |
| `arguments` | string | no | Vendor-raw JSON, keys differing per harness |
| `result` | string | no | Output, present on the entry carrying a tool's answer |
| `target` | ToolTarget | no | What the call acted on, resolved at import |

## ToolTarget

The one argument that says what a call acted on. `arguments` keys differ per
harness, so resolving them belongs with the adapter that knows each harness;
consumers read this field instead of re-parsing the blob.

```json
{ "kind": "path", "value": "src/auth.rs" }
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `kind` | string | yes | `path`, `command`, or `subject` |
| `value` | string | yes | Repo-relative for `path` when a workspace root was known |

Absent when a call names nothing worth showing, and on documents written before
the field existed — a consumer must treat it as optional.

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

An imported session's id is `sha256("session-v2" : agent : vendor_token)`, truncated to 26 hex characters. `vendor_token` is the agent's own name for the session, supplied by its adapter — in practice the transcript's filename stem, which every supported agent derives from its session id. Codex prefers the `session_id` in its `session_meta` record and falls back to the stem.

The key is deliberately narrow. Nothing machine-local enters it, so two people — or two git worktrees — importing the same transcript derive the same id and their copies merge rather than duplicate. Nothing that grows with the transcript enters it either, so appending turns to a live session leaves its id unchanged and re-import updates that session in place.

The token must be per-transcript rather than per-conversation: an agent may record several transcripts under one vendor session id (Claude's subagent transcripts repeat their parent's `sessionId`), and keying on the shared value would collapse them into a single session.

Forked sessions do not use this key. A fork is a new session rather than a re-observation of an existing one, so it mints a fresh ULID and records its origin in `fork_origin`.
