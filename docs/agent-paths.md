# Where Lineage finds agent history

[← Back to README](../README.md) · [Ingest](ingest.md)

Lineage scopes discovery to your **repository working directory**. It checks project-local paths first, then global agent config directories.

| Agent | Locations searched |
|-------|-------------------|
| **Cursor** | `.cursor/projects/*/agent-transcripts/`, `.cursor/agent-transcripts/`, `~/.cursor/projects/<project-key>/agent-transcripts/` |
| **Claude Code** | `.claude/projects/<encoded-path>/*.jsonl`, `~/.claude/projects/<encoded-path>/*.jsonl` |
| **Codex** | `.codex/sessions/`, `~/.codex/sessions/` (filtered by session `cwd`) |

Transcript files are JSONL. Lineage skips Claude snapshot/progress files and scopes sessions to the current workspace.
