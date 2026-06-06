# Git Notes Schema v0

How lineage data is stored inside a git repository.

## Ref namespace

| Ref | Purpose |
|-----|---------|
| `refs/lineage/sessions/<session-id>` | Points to conversation blob OID |
| `refs/lineage/index` | JSON manifest of all session IDs |
| `refs/lineage/config` | Repo lineage config (optional) |

## Git notes

Notes ref: `refs/notes/lineage`

Each commit with linked sessions gets a note:

```json
{
  "schema_version": "git-notes-v0",
  "commit_sha": "abc123...",
  "session_ids": ["01HQZX8K9V2M3N4P5Q6R7S8T9U"],
  "line_object_ids": ["01HQZX8K9V2M3N4P5Q6R7S8T9W"],
  "patch_id": "a1b2c3..."
}
```

`patch_id` (optional) is a stable hash of the commit's normalized diff, used by `git lineage remap` after rebases.

## Blob storage

Conversation and line-object JSON are stored as git blobs (UTF-8). Objects larger than the configured threshold use the Git LFS backend by default (`large_blob_backend: lfs` in `refs/lineage/config`).

```text
.git/lfs/objects/           # LFS object store (sha256 layout)
refs/lineage/lfs/<sha256>   # LFS pointer refs (pushable)
refs/lineage/lfs-data/<sha256>  # Transport blobs for git push/fetch

.git/lineage/
  index.db          # SQLite search index (rebuildable)
  blobs/            # Legacy cache when large_blob_backend: cache
```

## Merge / push

Lineage refs and notes are normal git objects. `git push` distributes them when refs are pushed:

```bash
git push origin refs/lineage/* refs/notes/lineage
```

## Doctor checks

1. `refs/notes/lineage` exists or can be created
2. `refs/lineage/index` is valid JSON
3. All session refs resolve to readable blobs
