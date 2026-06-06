---
name: lineage-git-work
description: >-
  Work on lineage-git: git refs, notes, LFS, hooks, blame, commit mapping,
  remap, GC, and delete. Covers integration test patterns with tempfile repos.
  Use when changing git persistence, refs/lineage/*, refs/notes/lineage, LFS
  transport, or hook behavior.
---

# Lineage git work

## Storage model

| Ref/path | Content |
|----------|---------|
| `refs/lineage/sessions/<id>` | Conversation JSON blobs |
| `refs/lineage/lines/<id>` | Line object blobs |
| `refs/lineage/index` | Session manifest |
| `refs/lineage/config` | Repo policy |
| `refs/lineage/last-import` | Incremental import state (`last-ingest` legacy ref still read) |
| `refs/notes/lineage` | Per-commit session + line-object links |
| `.git/lfs/objects/` | Large turn/media content (default backend) |
| `.git/lineage/index.db` | Rebuildable search cache (not in this crate) |

See `specs/git-notes-schema-v0.md` and `docs/ARCHITECTURE.md`.

## Key modules

| Module area | Files |
|-------------|-------|
| Notes | `src/notes.rs` |
| Refs | `src/refs.rs` |
| Blame | `src/blame.rs` |
| Commit mapping | `src/commit_map.rs` |
| LFS | `src/lfs*.rs` |
| Hooks linking | `src/hooks.rs` (`link_recent_sessions_to_head`, `link_all_sessions_to_head`) |
| Import state | `src/import_state.rs` |
| GC / delete | `src/blob_gc.rs`, `src/delete.rs` |

## Integration tests

Add tests under `crates/lineage-git/tests/`:

```rust
// Pattern: tempfile + git init + commit
let dir = tempfile::tempdir().unwrap();
// git init, config user, commit fixture file
let repo = open_repo(dir.path())?;
```

Existing suites to extend (not duplicate):

- `full_workflow.rs` — end-to-end import/persist/blame
- `hooks_integration.rs` — pre/post commit behavior
- `remap_integration.rs` — rebase recovery
- `blame_integration.rs`, `delete_integration.rs`, `lfs_*_integration.rs`

Reuse `tests/fixtures/git-repo/` setup patterns where applicable.

## Rules

- **Policy before persist** — redaction happens before `lineage-git` writes
- Notes use `find_note()` — never treat note commit OIDs as content blobs
- Line coverage must stay ≥80% after changes (`./scripts/coverage.sh`)

## Common tasks

| Task | Start here |
|------|------------|
| New ref type | `refs.rs` + spec update (`schema-change` skill) |
| Blame enrichment | `blame.rs` + `blame_integration.rs` |
| LFS | `lfs_ops.rs`, `lfs_batch.rs`, `lfs_refs.rs`, `lfs_worktree.rs` (`lfs_transport` is a config field) |
| Orphan GC | `blob_gc.rs` — refcount-aware purge |
