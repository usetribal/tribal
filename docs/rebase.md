# After a rebase

[← Documentation index](README.md) · [Share](share.md) · [Maintenance](maintenance.md)

Interactive rebase, squash, and amend rewrite commit SHAs. Lineage git notes and session `commit_shas` may still point at old commits, leaving blame and per-commit session lists out of date until you remap.

## When to remap

Run remap after:

- `git rebase` or `git rebase -i`
- `git commit --amend` on published lineage-linked commits
- History filtering that changes commit ids without changing patch content

You do not need remap for normal fast-forward pulls or merge commits that preserve existing SHAs.

## Command

```bash
git lineage remap
```

Remap:

1. Finds orphan SHAs referenced from sessions and notes.
2. Matches rewritten commits using patch-id metadata stored on git notes.
3. Updates session `commit_shas` and re-links notes where a match exists.
4. Re-materializes line objects at the new commits.

Verify:

```bash
git lineage list --commit <new-sha> --json
git lineage blame path/to/file.rs:42
```

## Limits

Remap succeeds when patch-id correspondence exists between old and new commits. Extreme history surgery (splitting patches unrelated to original commits) may leave orphans. Manual `git lineage link` and `materialize` can repair individual sessions.

## Workflow tip

```bash
git rebase -i main
git lineage remap
git lineage doctor
# push updated refs if sharing lineage with team
git push origin refs/lineage/* refs/notes/lineage
```

## VS Code and MCP

**Lineage: Remap After Rebase** in the extension runs the same CLI command. MCP exposes `lineage_remap` for agent-driven recovery after rebase operations.

## Related guides

- [How it works](how-it-works.md) — git notes and patch ids
- [Explore](explore.md) — verify blame after remap
- [CLI reference](cli/README.md)
