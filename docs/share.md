# Share lineage with your team

[← Back to README](../README.md) · [Explore](explore.md) · [After a rebase](rebase.md)

Lineage data lives in git refs and notes. Push them alongside your code:

```bash
# Push session refs, notes, and LFS transport refs
git lineage lfs push
git push origin refs/lineage/* refs/notes/lineage
```

On a fresh clone, teammates fetch lineage data before blaming or searching:

```bash
git fetch origin refs/lineage/* refs/notes/lineage
git lineage lfs fetch
git lineage doctor
```

Before sharing publicly, export with redaction to review what would leave the repo:

```bash
git lineage export --redact --format jsonl > lineage-export.jsonl
```
