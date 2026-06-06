# Git hooks

[← Back to README](../README.md) · [Ingest](ingest.md)

[Setup](../README.md#setup) installs hooks automatically. To manage them manually:

```bash
git lineage install-hook          # pre-commit ingest + post-commit linking
git lineage install-hook --force  # overwrite existing hooks
git lineage uninstall-hook
```

| Hook | Action |
|------|--------|
| `pre-commit` | Incremental ingest (`--no-link-head --incremental`) |
| `post-commit` | Link recently ingested sessions to the new commit |
