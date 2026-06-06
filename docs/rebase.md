# After a rebase

[← Back to README](../README.md) · [Share](share.md)

If commit SHAs changed, remap orphaned lineage notes to the rewritten history:

```bash
git lineage remap
```

This uses patch-id metadata stored on git notes to match rewritten commits where possible, then re-materializes line objects.
