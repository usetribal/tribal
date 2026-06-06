# Agent paths

[← Documentation index](README.md) · [Import](import.md) · [Adapters](adapters.md)

Lineage discovers agent transcripts on disk during import. Discovery is scoped to your **repository working directory** — run commands from the project root you initialized.

## Search order

Adapters check project-local directories first, then user-global agent folders filtered to the current workspace.

## Cursor

| Location | Notes |
|----------|-------|
| `.cursor/projects/*/agent-transcripts/` | Per-workspace transcript folders |
| `.cursor/agent-transcripts/` | Legacy/alternate layout |
| `~/.cursor/projects/<project-key>/agent-transcripts/` | Global Cursor projects dir |

Project key is derived from the absolute workspace path. Transcripts are JSONL under per-session folders.

## Claude Code

| Location | Notes |
|----------|-------|
| `.claude/projects/<encoded-path>/*.jsonl` | Project-scoped sessions |
| `~/.claude/projects/<encoded-path>/*.jsonl` | User-level mirror |

Encoded path replaces `/` with `-` in the absolute workspace path. Snapshot, progress, queue, and summary JSONL types are skipped.

## Codex

| Location | Notes |
|----------|-------|
| `.codex/sessions/` | Repo-local session rolls |
| `~/.codex/sessions/` | Global rolls filtered by session `cwd` matching workspace |

Codex walks dated subfolders under `sessions/`.

## Requirements for discovery

- Transcript files must exist before import runs.
- Workspace path used at import must match the path agents used when writing sessions.
- For CI or headless import, agent history may be absent — discovery legitimately returns zero sessions.

## After moving or renaming repos

Agent tools may write to new encoded paths after a directory move. Re-run import from the new root. Old transcripts remain under previous paths until agents create new sessions.

## Related guides

- [Import](import.md)
- [Troubleshooting](../README.md#troubleshooting)
- [Adapters](adapters.md)
