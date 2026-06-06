# Explore your lineage

[← Back to README](../README.md) · [Ingest](ingest.md) · [Share](share.md)

```bash
# List ingested sessions (human-readable)
git lineage list

# JSON for scripts and tooling
git lineage list --json

# Show a full conversation (hydrates large LFS-backed content automatically)
git lineage show <session-id>

# Machine-readable session export
git lineage show <session-id> --json

# Which agent turn touched a specific line?
git lineage blame src/main.rs:42
git lineage blame src/main.rs:42 --json

# Full-text search over session content
git lineage search "authentication middleware"

# Sessions linked to a specific commit
git lineage list --commit <sha>
```

**Lineage blame** combines `git blame` with lineage notes: it finds the introducing commit, loads linked sessions and line objects, and returns matching turns (including confidence and content previews in JSON mode).
