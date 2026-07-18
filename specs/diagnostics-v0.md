# Diagnostics v0

A single authoritative local record of what `git-lineage` did (the **event log**),
and a human-and-machine diagnosis surface built on it plus config/refs/index state
(the **doctor `--json` contract**). This is a **narrative contract only** — unlike
`schema/`, these shapes are not generated from `lineage-core` domain types and MUST
NOT enter the schema/bindings pipeline (see
[decisions/0001-contract-bindings-pipeline.md](decisions/0001-contract-bindings-pipeline.md)
for the contrast). The event log is local plumbing, never synced; the doctor
`--json` shape is a cross-repo contract because external tooling (e.g. a
Lineage developer's inspection CLI) consumes it.

Status: **draft for review** — written against `diagnostic-tooling` plan tasks 1-4.

## Scope

- **In scope:** the event log entry schema (versioned, one variant per operation
  type); which operations emit entries and what each entry carries; the doctor
  `--json` output contract (five sections, section filters); the silent-fire
  reasons for the context hook.
- **Out of scope, not foreclosed:** a server-side operations log and debug API
  endpoints (event log is local-only in v0); `git lineage fix` (healing) — the log
  and doctor sections are designed so a future guided-repair command can read
  findings from these structures rather than re-diagnosing.
- **Never in scope:** the event log blocking or failing a user-facing operation.
  Logging is best-effort; "authoritative" means it is the one place to look, not
  that operations abort without it.

## Event log

Append-only JSONL at `.git/lineage/events.jsonl`, one JSON object per line, newest
last. Local plumbing: never included in `sync`, never read by any server. Written
best-effort — a failed log write is swallowed (optionally surfaced as a
`tracing` warning) and never propagates to the calling command's `Result`. This is
the same fail-open posture `context_cmd.rs`'s hook path already has; the event log
generalizes it to every operation, not just the hook.

Every entry:

| Field | Type | Required | Meaning |
|-------|------|----------|---------|
| `schema_version` | string | yes | `lineage-events-v0` |
| `ts` | RFC 3339 string | yes | Wall-clock time the operation completed (or failed) |
| `op` | string | yes | Operation tag — see table below |
| `outcome` | string | yes | `ok`, `error`, or `silent` (context-hook fires that produced no injection) |
| `detail` | object | yes | Op-specific payload, shape given per `op` below |

Entries are written from `lineage-cli`'s command layer (`commands.rs`,
`context_cmd.rs`, `hooks_cmd.rs`), not from `lineage-git`/`lineage-search` — those
crates stay free of event-log concerns, matching the existing layering where
`commands.rs` is the thin orchestration point over `lineage-git`/`lineage-search`
library calls.

### Operation catalogue

| `op` | Emitted from | `detail` payload |
|------|---------------|-------------------|
| `init` | `init_cmd::init` | `{ "targets": [...], "skill_installed": bool, "hooks_installed": bool, "import_run": bool }` |
| `install_hook` | `hooks_cmd::install_hook` | `{ "hooks": ["pre-commit", "post-commit"], "forced": bool }` — one entry per invocation; `install_hook` installs both git hooks in a single call, so the entry lists what was written |
| `install_skill` | `skill_cmd::init_skill` | `{ "targets": [...] }` |
| `install_claude_agent_hook` | `context_cmd::install_claude_agent_hook` | `{ "already_installed": bool }` |
| `import` | `commands::import` | `{ "agents": [...], "discovered": {agent: count}, "imported": count, "skipped": count, "errors": count, "session_ids": [...], "line_objects_written": count }` — one entry per `import` invocation, not per session; per-session detail lives in the `session_ids` array, matching today's `import` summary line |
| `link` | `commands::link`, `hooks_cmd::post_commit` | `{ "commit_sha": "...", "sessions": [{ "session_id": "...", "line_objects": count }], "trigger": "manual" \| "post_commit" }` — one entry per invocation; `post_commit` links every recently imported session to HEAD in one call, so the entry carries the full list (today `link_recent_sessions_to_head` returns only a count — its return value grows per-session detail, a state extension, not new logic in `lineage-git`) |
| `materialize` | `commands::materialize` | `{ "commit_sha": "...", "sessions": [{ "session_id": "...", "line_objects": count }] }` — one entry per invocation; with no `--session` flag the command materializes every session at the commit |
| `rebuild_index` | `commands::rebuild_index` | `{ "sessions_indexed": count }` |
| `sync` | `commands::sync` | `{ "server": "...", "remote": "...", "batch": { "conversations": count, "line_objects": count, "session_commit_links": count, "blobs": count }, "blobs_uploaded": count, "response": { "schema_version": "...", "repo_id": "...", "results": [...], "metadata": {...} } }` — `response` is the **full** `SyncResponse` wire object the server returned (sync-protocol-v0), per-object `results` included verbatim, not the client's tallied summary counts, so server behavior stays traceable from the client's own record; `blobs_uploaded` is client-side (blob PUTs happen before the batch POST and are not part of `SyncResponse`) |
| `context_hook` | `context_cmd::run_claude_hook` | `{ "file_path": "...", "harness": "claude", "reason": "(silent-fire reason, omitted when outcome is ok)", "session_ids": [...], "strength": "..." }` — superset of today's `context-log.jsonl` entry; `outcome: "ok"` entries are the injection-log surface [context-injection-v0 § Injection log](context-injection-v0.md#injection-log) requires, `context log` reads them, and `context-log.jsonl` is retired in favor of this |

`import`'s per-agent `discovered` counts and `session_ids` are enough for the
doctor "capture" section to compare discovered-vs-imported without re-deriving
adapter behavior — see [Doctor](#doctor).

### Silent-fire reasons (context hook)

`context_cmd::run_claude_hook` today has several early-return points that all
collapse to "print nothing." Diagnostics v0 requires each to carry a reason so
`context doctor`/the activity section can distinguish "never fired" (no event
entry at all — e.g. hook not wired, wrong tool name, unappendable event shape
before repo/index are even opened) from "fired and silent" (an entry exists with
a reason):

| Reason | Condition |
|--------|-----------|
| `no_evidence` | Retrieval returned an empty `evidence` array (honest-nothing, per context-injection-v0) |
| `below_floor` | Retrieval had evidence, but all entries were below `MIN_STRENGTH` after selection |
| `over_budget` | Retrieval hit `budget_ms` and the truncated result had no selectable evidence. Requires the retriever to mark budget-truncated retrievals (a `truncated` flag on `retrieval-v0`, absent/false in prior cache entries via serde default) — without it this case is indistinguishable from `no_evidence` |
| `unappendable_shape` | `tool_response` shape didn't match a known appendable form (`response_is_appendable` returned `false`) |
| `error` | Any `Err` from repo open, index open, cache open/read, or retrieval — the hook still exits 0 and prints nothing, but the reason and error message are captured in the event entry |

Not every early return in `run_claude_hook` is a loggable event — `tool_name !=
"Read"` and a missing `tool_input.file_path` are not lineage-relevant at all
(any tool call, not just ones touching this repo, hits this endpoint) and remain
un-logged. `unappendable_shape` **is** logged even though today's code checks
appendability before opening the repo: a Read in this repo whose response shape
we could not append to is exactly the harness-drift signal doctor needs, and
logging it only costs a repo open on that already-rare path. Every other
fired-but-silent outcome (repo opened, retrieval attempted or completed) logs
one of the five reasons above. This is the
line gap 7's "fired-vs-silent indistinguishability" needs: doctor's activity
section shows fired-and-silent entries with their reason, and their absence
across a session where the hook should have run is itself diagnostic (a setup
problem, not a retrieval one).

## Doctor

`git lineage doctor` grows from today's single flat report (git/notes/index/config
ref checks, session count, broken sessions, missing LFS blobs — see
`lineage-git::doctor::DoctorReport`) into five ordered sections. Human-readable
text remains the default; `--json` emits the same data as a single object;
`--section <name>` (repeatable) filters to a subset of sections in either mode.

```bash
git lineage doctor [--json] [--section setup] [--section capture] ...
```

### `--json` shape

```json
{
  "schema_version": "lineage-doctor-v0",
  "setup": { ... },
  "capture": { ... },
  "materialization": { ... },
  "links": { ... },
  "activity": { ... }
}
```

Every section is always present in `--json` output (empty/ok sections still
appear, with empty arrays/zero counts) so a consumer never has to special-case a
missing key; `--section` filtering only affects which top-level keys are
included, never their internal shape.

### `setup`

Today's ref/config checks (`is_git_repo`, `notes_ref_ok`, `index_ref_ok`,
`config_ref_ok`) plus:

- `binary_version`: the running `git-lineage` version (`CARGO_PKG_VERSION`).
- `index_schema`: `{ "has_session_files": bool, "has_index_meta": bool,
  "generation": int }` — detected by querying `sqlite_master` for
  `session_files`/`index_meta` table existence before calling
  `LineageIndex::generation()`; an index.db created before those tables existed
  (this week's "stale index" dogfood) reports `has_session_files: false` and is
  flagged in `warnings`.
- `hook_wiring`: `{ "claude_settings_present": bool, "lineage_hook_registered":
  bool, "loadable_from_session_root": bool }`. The last field is the gap 7 trap:
  Claude Code only loads hooks from the settings at the session's root, so a repo
  correctly wired at its own root can still never fire if the session was opened
  from a parent workspace — doctor checks whether `repo_path` (the directory
  doctor is run from / `--repo`) is itself the nearest `.claude/settings.json`
  scope, not merely whether that file has the hook entry.
- `git_hooks`: `{ "pre_commit_installed": bool, "post_commit_installed": bool }`.

### `capture`

- `sessions_discovered`: per-agent counts from the most recent `import` event
  log entries (not a live re-discovery — doctor reads the log, it does not
  re-invoke adapters).
- `sessions_imported`: total imported session count (from `list_session_ids`).
- `workspace_mismatches`: sessions whose `workspace_root` is not equal to (and
  not a descendant relationship consistent with) the repo's own root — the gap 7
  signature "authoring session not captured" is a session whose `workspace_root`
  is a **parent** of the repo, meaning it was captured for the wrong repo or
  never captured at all. Reported as
  `[{ "session_id": "...", "workspace_root": "...", "repo_root": "..." }]`.

### `materialization`

The funnel: artifacts → resolvable → resolved → line objects, with per-stage
failure reasons — the gap 11 audit as a built-in section rather than a one-off
session audit. Per session (or aggregated, with `--json` carrying per-session
detail):

```json
{
  "total_artifacts": int,
  "resolvable": int,
  "resolved": int,
  "line_objects": int,
  "failure_reasons": {
    "no_resolve_payload": int,
    "old_string_not_found_post_edit": int,
    "missing_new_string": int,
    "commit_not_linked": int
  }
}
```

`failure_reasons` keys map directly onto gap 11's (a)-(d): `old_string_not_found_post_edit`
is `resolve_old_string` searching pre-edit text in post-edit content;
`missing_new_string` is artifacts whose adapter never captured `new_string`;
`no_resolve_payload` is `file_edit` artifacts with no resolve payload at all
(read-only sessions, gap 9-adjacent); `commit_not_linked` is edits that landed in
a commit the session was never linked to. This section reads existing line-object
and artifact state — it does not require gap 11's fix to land first; it makes the
gap measurable now, per the plan's "Out of scope" note that fixing the gaps is
separate work.

### `links`

Per commit with any linked sessions:
`[{ "commit_sha": "...", "sessions": [{ "session_id": "...", "established_by":
"post_commit" | "manual" | "auto_match" }] }]` — `established_by` is read from the
`link` event log entries' `trigger` field (falling back to `"auto_match"` when
`commit_mapping: auto` recorded a `commit_match_score` on import, and `"unknown"`
when no event log entry exists for a link found only in git-notes, e.g. from a
pre-diagnostics-v0 install).

### `activity`

The event log's tail, human-summarized: recent injections (`context_hook` entries
with `outcome: "ok"`), recent silences with their reason, recent syncs with their
summary counts. `--json` gives the last N raw entries (default 20, `--activity-limit`
to override); text mode renders one line per entry in the style of today's
`context log` output.

## Acceptance

Each of this week's four dogfood failure modes must be visible in doctor output on
a fixture that reproduces it:

1. **Hook unloadable from session root** → `setup.hook_wiring.loadable_from_session_root: false`.
2. **Stale index schema** → `setup.index_schema.has_session_files: false` (or
   `has_index_meta: false`), surfaced as a `warnings` entry.
3. **Authoring session not captured** → `capture.workspace_mismatches` lists the
   session with its parent `workspace_root`.
4. **Edit artifacts resolving to zero** → `materialization.failure_reasons` has a
   nonzero count in the relevant bucket for that session.

## MCP

`lineage_doctor` (`lineage-mcp/src/server.rs`) returns the same `--json` shape
unfiltered (all five sections) — no separate contract; the MCP tool is a thin
wrapper over the same `run_doctor`-successor that the CLI's `--json` flag calls.
