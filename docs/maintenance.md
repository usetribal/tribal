# Maintenance

[← Documentation index](README.md) · [Privacy](privacy.md) · [CLI reference](cli/README.md)

Day-to-day lineage operations beyond import and search: health checks, index rebuilds, manual linking, cleanup, and session removal.

## Health check

```bash
tribal doctor
```

Doctor verifies that configuration exists, session refs resolve, notes are readable, and LFS references have local objects (or reports gaps). Run after clone, policy changes, or when blame/search behave unexpectedly.

## Search index

Search uses a local SQLite full-text index at `.git/lineage/index.db`. It is rebuildable from git refs and is not pushed to remotes.

```bash
# Explicit rebuild
tribal rebuild index

# Search also rebuilds when results look stale
tribal search "your query"
```

Rebuild after bulk import, delete, or gc if search misses known sessions.

## Materialize and link

Line objects connect file lines to conversation turns. They are created when sessions link to commits and artifacts can be resolved against the commit tree.

```bash
# Link a session to a commit and materialize line objects
tribal link <session-id> <commit-sha>

# Rebuild line objects for a commit or single session
tribal materialize
tribal materialize --commit <sha>
tribal materialize --session <session-id>
```

Hooks and import normally handle linking. Use these commands when auto-mapping was skipped (`commit_mapping: none`), after manual history edits, or when blame returns no matches for a linked commit.

## Delete sessions

```bash
# Remove session ref, line objects, and note entries
tribal delete <session-id>

# Also drop unreferenced LFS blobs referenced only by this session
tribal delete <session-id> --purge-blobs
```

Deletion is destructive. Confirm the session id with `tribal list` first. Pushed refs require a follow-up push to remove data from the remote.

## Garbage collection

```bash
tribal gc
```

Purges orphan line objects and unreferenced LFS blobs after deletes or failed imports. Safe to run periodically; refcounting prevents deleting blobs still used by other sessions.

## Export for audit

```bash
tribal export --redact --format jsonl > audit.jsonl
tribal export --format json > single-session.json
```

Use export to review what would leave the repo before push. See [Privacy and policy](privacy.md).

## Rebase recovery

After history rewrite, run:

```bash
tribal remap
```

See [After a rebase](rebase.md) for details.

## LFS maintenance

```bash
tribal lfs status
tribal lfs fetch
```

See [Large content (LFS)](lfs.md).

## Common situations

| Symptom | What to try |
|---------|-------------|
| Search returns nothing | `rebuild-index`, then search again |
| Blame empty but session exists | `materialize`, confirm session is linked to introducing commit |
| Doctor reports missing LFS | `lfs fetch` |
| Stale sessions after agent work | `import --incremental`, commit with hooks enabled |
| Sensitive session in repo | `delete --purge-blobs`, review [privacy](privacy.md), force-push only if appropriate |

## Related guides

- [CLI reference](cli/README.md)
- [Configuration](configuration.md)
- [Share](share.md)
