# Diagnostics v0

Two diagnostic surfaces: the **event log**, a local record of every operation
`git-lineage` performs, and the **doctor report**, the machine-readable output
of `git lineage doctor --json`. Both are narrative contracts defined by this
document; they are not generated from `lineage-core` types.

## Event log

Append-only JSONL at `.git/lineage/events.jsonl`, one JSON object per line,
newest last. Local plumbing: never included in `sync`, never read by a server.
Writes are best-effort — a failed write never fails the operation being
recorded.

```json
{
  "schema_version": "lineage-events-v0",
  "ts": "2026-07-18T14:03:52Z",
  "op": "import",
  "outcome": "ok",
  "detail": {}
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `schema_version` | string | yes | Always `lineage-events-v0` |
| `ts` | ISO8601 | yes | When the operation completed |
| `op` | string | yes | Operation tag, see below |
| `outcome` | string | yes | `ok`, `error`, or `silent` |
| `detail` | object | yes | Per-`op` payload, see below |

### Operations

One entry per command invocation.

| `op` | `detail` |
|------|----------|
| `init` | `{ "targets": [...], "skill_installed": bool, "hooks_installed": bool, "import_run": bool }` |
| `install_hook` | `{ "hooks": ["pre-commit", "post-commit"], "forced": bool }` |
| `install_skill` | `{ "targets": [...] }` |
| `install_claude_agent_hook` | `{ "already_installed": bool }` |
| `import` | `{ "agents": [...], "discovered": {"<agent>": count}, "imported": count, "skipped": count, "errors": count, "session_ids": [...], "line_objects_written": count }` |
| `link` | `{ "commit_sha": "...", "sessions": [{ "session_id": "...", "line_objects": count, "basis": "line_objects" \| "file_overlap" }], "skipped_no_overlap": ["session_id"], "trigger": "manual" \| "post_commit" }` — automatic linking is gated: a session is linked only with evidence (materialized line objects, or written-file overlap with the commit); refused sessions appear in `skipped_no_overlap`. Manual `link` is ungated and carries no `basis` |
| `materialize` | `{ "commit_sha": "...", "sessions": [{ "session_id": "...", "line_objects": count }] }` |
| `rebuild_index` | `{ "sessions_indexed": count }` |
| `rebuild` | `{ "commits_scanned": count, "links_written": count, "manual_replayed": count, "line_objects": count, "notes_deleted": count, "line_object_refs_deleted": count, "sessions_indexed": count }` — full derived-layer rebuild: wipes notes and line objects, relinks every commit under the evidence gate, replays manual links from this log, rebuilds the index |
| `sync` | `{ "server": "...", "remote": "...", "batch": { "conversations": count, "line_objects": count, "session_commit_links": count, "blobs": count }, "blobs_uploaded": count, "response": SyncResponse }` — `response` is the server's `sync-response-v0` object verbatim ([sync-protocol-v0](sync-protocol-v0.md)) |
| `context_hook` | `{ "file_path": "...", "harness": "claude", "session_ids": [...], "strength": "..." }`; when `outcome` is `silent` or `error`, `session_ids`/`strength` are absent and a `reason` field is present |

`git lineage context log` renders the `context_hook` entries with
`outcome: "ok"` — they are the injection log required by
[context-injection-v0 § Injection log](context-injection-v0.md#injection-log).

### Context-hook silence reasons

A context-hook fire that injects nothing records `outcome: "silent"` (or
`"error"`) with a `reason`, so silence is distinguishable from a hook that
never ran. Hook events that are not lineage-relevant (a non-Read tool, no file
path) produce no entry.

| `reason` | Condition |
|----------|-----------|
| `no_evidence` | Retrieval returned no evidence |
| `below_floor` | Evidence existed, but all of it fell below the minimum strength |
| `over_budget` | Retrieval was truncated by its time budget and had no selectable evidence (`retrieval-v0` `truncated`) |
| `unappendable_shape` | The tool response was not a shape a digest can be appended to |
| `error` | Any internal error; the message is in `detail.error` |

## Doctor report

```bash
git lineage doctor [--json] [--section <name>]...
```

`--json` emits one object; `--section` (repeatable) filters which top-level
sections are included, never their internal shape. Sections that have nothing
to report still appear, with empty arrays and zero counts. The MCP
`lineage_doctor` tool returns the same object, unfiltered.

```json
{
  "schema_version": "lineage-doctor-v0",
  "setup": {},
  "capture": {},
  "materialization": {},
  "links": {},
  "activity": {}
}
```

### `setup`

Installation and wiring state.

| Field | Type | Description |
|-------|------|-------------|
| `binary_version` | string | Version of the running `git-lineage` |
| `is_git_repo` | bool | Directory is a git repository |
| `notes_ref_ok` | bool | Lineage notes ref present or creatable |
| `index_ref_ok` | bool | Lineage index ref present or creatable |
| `config_ref_ok` | bool | `refs/lineage/config` present |
| `index_schema` | object | `{ "has_session_files": bool, "has_index_meta": bool, "generation": int }` — an index built by an older binary reports missing tables |
| `hook_wiring` | object | `{ "claude_settings_present": bool, "lineage_hook_registered": bool, "loadable_from_session_root": bool }` — the last is false when the repo's hook settings are not at the root the agent session was opened from, so the hook can never fire |
| `git_hooks` | object | `{ "pre_commit_installed": bool, "post_commit_installed": bool }` |
| `warnings` | string[] | Human-readable findings |

### `capture`

Whether the sessions that should be in lineage are in lineage.

| Field | Type | Description |
|-------|------|-------------|
| `sessions_discovered` | object | Per-agent counts from the most recent `import` event |
| `sessions_imported` | int | Sessions stored in lineage refs |
| `workspace_mismatches` | object[] | `[{ "session_id": "...", "workspace_root": "...", "repo_root": "..." }]` — sessions whose recorded workspace is not this repository (e.g. a parent directory), meaning they were captured against the wrong root |
| `broken_sessions` | string[] | Session ids whose stored conversation can no longer be read |
| `missing_lfs_blobs` | string[] | Blob ids referenced by sessions but absent from local storage |

### `materialization`

The funnel from session artifacts to line objects, with per-stage loss
reasons.

```json
{
  "total_artifacts": 0,
  "resolvable": 0,
  "resolved": 0,
  "line_objects": 0,
  "failure_reasons": {
    "no_resolve_payload": 0,
    "missing_old_string": 0,
    "old_string_not_found": 0,
    "commit_not_linked": 0
  }
}
```

| Reason | Meaning |
|--------|---------|
| `no_resolve_payload` | Edit artifact carries no content to resolve against |
| `missing_old_string` | The artifact's resolve strategy needs the pre-edit text, which was never captured |
| `old_string_not_found` | The artifact's pre-edit text no longer exists in the file at the linked commit |
| `commit_not_linked` | The session is not linked to any commit that touched the file |

### `links`

Session↔commit links and how each was established.

```json
[
  {
    "commit_sha": "abc123",
    "sessions": [{ "session_id": "01HQ…", "established_by": "line_objects" }]
  }
]
```

`established_by` prefers the link's evidence basis (`line_objects`,
`file_overlap`) when the event log recorded one; otherwise it falls back to
the trigger (`post_commit`, `manual`), `auto_match`, or `unknown` when no
event-log entry exists for the link (links predating basis recording).

### `activity`

The event log's tail: the last N entries (default 20,
`--activity-limit` to override), newest last.
