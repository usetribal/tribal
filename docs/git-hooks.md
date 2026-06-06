# Git hooks

[← Back to README](../README.md) · [Import](import.md)

[Setup](../README.md#setup) installs hooks automatically. To manage them manually:

```bash
git lineage install-hook          # pre-commit import + post-commit linking
git lineage install-hook --force  # overwrite existing hooks
git lineage uninstall-hook
```

| Hook | Action |
|------|--------|
| `pre-commit` | Incremental import (`--no-link-head --incremental`) |
| `post-commit` | Link recently imported sessions to the new commit |
