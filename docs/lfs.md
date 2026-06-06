# Large content (LFS)

[← Documentation index](README.md) · [Share](share.md) · [Configuration](configuration.md)

Agent sessions can include long tool output, patches, and images. Lineage stores compact conversation JSON in git refs and moves payloads above `large_blob_threshold_bytes` into Git LFS by default (see [Configuration](configuration.md)).

## What gets stored where

| Location | Contents |
|----------|----------|
| `refs/lineage/sessions/<id>` | Conversation metadata and pointers to large blobs |
| `.git/lfs/objects/` | Local Git LFS object store (default backend) |
| `refs/lineage/lfs/<sha>` | LFS pointer blobs (pushable without a dedicated LFS server) |
| `refs/lineage/lfs-data/<sha>` | Transport blobs for ref-based push/fetch |

The local search index does not duplicate full turn text; it rebuilds from refs.

## Commands

```bash
# Compare referenced vs locally present objects
git lineage lfs status

# Push pointer and data refs to remote
git lineage lfs push
git lineage lfs push --remote origin

# Fetch missing objects after clone or pull
git lineage lfs fetch
git lineage lfs fetch --remote origin
```

Typical team workflow alongside code:

```bash
git lineage lfs push
git push origin refs/lineage/* refs/notes/lineage
```

On a fresh clone:

```bash
git fetch origin refs/lineage/* refs/notes/lineage
git lineage lfs fetch
git lineage doctor
```

## Transport modes

Set `lfs_transport` in `refs/lineage/config`:

| Mode | Behavior |
|------|----------|
| `auto` | Try git-lfs CLI, then HTTP batch API, then ref-based fallback |
| `gitcli` | Require `git-lfs` on PATH |
| `http` | Use Git LFS HTTP batch API (no git-lfs CLI required) |
| `refs` | Move data only via `refs/lineage/lfs-data/*` |

Use `refs` or `http` when contributors cannot install git-lfs. Use `gitcli` when your host already provides standard Git LFS.

## Doctor and missing objects

`git lineage doctor` reports missing LFS objects referenced from sessions. After pulling lineage refs without LFS data, run `git lineage lfs fetch` before `show`, blame, or export with hydration.

`git lineage show <id>` hydrates large text automatically. Add `--hydrate-images` when reviewing image artifacts in the session timeline.

## Garbage collection

Deleting sessions with `git lineage delete --purge-blobs` drops refcounted LFS blobs when no other session references them. `git lineage gc` sweeps orphan line objects and unreferenced blobs. Run gc after bulk deletes in long-lived repos.

## VS Code and MCP

The VS Code extension and MCP server shell out to the same CLI for hydration and export. If large content is missing locally, UI previews may be incomplete until `lfs fetch` completes.

## Related guides

- [Share with your team](share.md)
- [Maintenance](maintenance.md)
- [How it works](how-it-works.md)
