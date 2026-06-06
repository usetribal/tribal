# Architecture

This document describes how Lineage is structured and how data flows through the system.

## Design goals

1. **Git-native** — lineage is stored as git objects (blobs, refs, notes), not an external database
2. **Rebuildable** — the search index and local caches can be reconstructed from git refs
3. **Policy-first** — sensitive content is redacted before persistence
4. **Agent-agnostic** — a canonical conversation schema; adapters translate vendor formats

## Crate dependency graph

```text
                    ┌─────────────┐
                    │ lineage-cli │
                    └──────┬──────┘
                           │
         ┌─────────────────┼─────────────────┐
         ▼                 ▼                 ▼
  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐
  │lineage-mcp   │  │lineage-search│  │lineage-adapt.│
  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘
         │                 │                 │
         └────────┬────────┴────────┬────────┘
                  ▼                 ▼
           ┌──────────────┐  ┌──────────────┐
           │ lineage-git  │  │ lineage-agent│
           └──────┬───────┘  └──────┬───────┘
                  │                 │
         ┌────────┴────────┐        │
         ▼                 ▼        ▼
  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐
  │lineage-store │  │lineage-policy│  │ lineage-core │
  └──────────────┘  └──────────────┘  └──────────────┘
```

## Ingestion flow

```text
1. Adapter discovers session files on disk
      ↓
2. Adapter reads vendor format → Conversation (lineage-core)
      ↓
3. Policy engine redacts secrets, removes excluded artifacts; repo config may mark sessions private
      ↓
4. Large turn content is compacted to Git LFS (`.git/lfs/objects/`) when above `large_blob_threshold_bytes`; transport refs enable `git lineage lfs push/fetch`
      ↓
5. lineage-git writes conversation blob + session ref
      ↓
6. lineage-git updates manifest (refs/lineage/index) and last-ingest state (refs/lineage/last-ingest)
      ↓
7. If commit_shas present: materialize line objects from artifacts (resolve `old_string`, citations, patches against commit tree) and write git notes
      ↓
8. lineage-search indexes conversation text (local cache; FTS triggers dedupe on re-index)
```

## Storage layout

### Git refs (pushed with repo)

| Ref | Content |
|-----|---------|
| `refs/lineage/index` | JSON manifest of session IDs |
| `refs/lineage/config` | Repository policy (excludes, private patterns, blob threshold) |
| `refs/lineage/last-ingest` | Last ingest timestamp and session IDs (for hook linking) |
| `refs/lineage/sessions/<id>` | OID of conversation JSON blob |
| `refs/lineage/lines/<id>` | OID of line-object JSON blob |
| `refs/notes/lineage` | Per-commit note linking sessions and line objects |

### Local cache (not pushed)

| Path | Content |
|------|---------|
| `.git/lineage/index.db` | SQLite FTS index (rebuildable) |
| `.git/lfs/objects/` | Git LFS object store for large turn content (default backend) |
| `.git/lineage/blobs/` | Legacy cache backend (`large_blob_backend: cache`) |
| `refs/lineage/lfs/<sha256>` | LFS pointer blobs (pushable) |
| `refs/lineage/lfs-data/<sha256>` | Transport blobs for git push/fetch without LFS server |

## Query paths

### Blame

1. Run `git blame` on the target file/line to find the introducing commit
2. Read the git note at `refs/notes/lineage` for that commit
3. Load linked line objects and conversation artifacts
4. Return matches where `file_path` and `line_range` overlap

### Search

1. Query the local SQLite FTS index
2. On miss or stale index: `git lineage search` auto-rebuilds, or run `git lineage rebuild-index` explicitly

### Rebase remap

1. `git lineage remap` finds orphan commit SHAs in session metadata
2. Maps orphans to rewritten commits via stored `patch_id` on git notes
3. Updates session `commit_shas`, re-links notes, and re-materializes line objects

## Extension points

| Extension | How |
|-----------|-----|
| New agent adapter | Implement `AgentSource` + `SessionReader` in `lineage-adapters` |
| New storage backend | Implement `ObjectStore` in `lineage-store` |
| Custom policy rules | Extend `PolicyConfig` with `RedactionRule` and `ExcludePattern` |
| Schema evolution | New `*-v1` schema in `specs/` with migration tooling |

## Related documents

- [conversation-schema-v0](../specs/conversation-schema-v0.md)
- [line-object-schema-v0](../specs/line-object-schema-v0.md)
- [git-notes-schema-v0](../specs/git-notes-schema-v0.md)
- [CONTRIBUTING.md](../CONTRIBUTING.md)
