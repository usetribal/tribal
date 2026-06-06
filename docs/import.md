# Import agent sessions

[← Back to README](../README.md) · [Explore](explore.md) · [Agent paths](agent-paths.md)

After [setup](../README.md#setup), import pulls agent transcripts into git refs.

Import discovers transcripts on disk, normalizes them, applies policy, and writes git refs. By default, sessions are **linked to the current `HEAD` commit**.

```bash
# All supported agents (Cursor, Claude Code, Codex)
git lineage import --agent all

# Or one agent at a time
git lineage import --agent cursor
git lineage import --agent claude
git lineage import --agent codex
```

## Useful flags

| Flag | Purpose |
|------|---------|
| `--since 2026-01-01` | Only import sessions started on or after this date (RFC 3339 or `YYYY-MM-DD`) |
| `--incremental` | Skip sessions already imported unless the source file changed |
| `--no-link-head` | Import without linking to the current commit (use with hooks) |

Example incremental import (typical for day-to-day use):

```bash
git lineage import --agent all --incremental
```

If no sessions are found, confirm agent history exists for this workspace. See [agent paths](agent-paths.md).
