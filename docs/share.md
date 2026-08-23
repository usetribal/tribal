# Share lineage with your team

[← Documentation index](README.md) · [LFS](lfs.md) · [Privacy](privacy.md)

Tribal data lives in git refs and notes alongside your code. Teammates with repository access can fetch the same sessions, blame, and search after pulling lineage refs.

## What to push

Lineage-specific refs are separate from branch tips:

```bash
tribal lfs push
git push origin refs/lineage/* refs/notes/lineage
```

| Ref pattern | Contents |
|-------------|----------|
| `refs/lineage/sessions/*` | Conversation blobs |
| `refs/lineage/lines/*` | Line object blobs |
| `refs/lineage/index` | Session manifest |
| `refs/lineage/config` | Repository policy |
| `refs/lineage/last-import` | Incremental import state |
| `refs/lineage/lfs/*` | LFS pointer blobs |
| `refs/lineage/lfs-data/*` | Ref-transported large payloads |
| `refs/notes/lineage` | Per-commit session indexes |

Standard `git push` without these refspecs does not publish lineage data.

## Teammate onboarding

After cloning your application repository:

```bash
git fetch origin refs/lineage/* refs/notes/lineage
tribal lfs fetch
tribal doctor
tribal rebuild index
```

Install the CLI (`make setup` or `cargo install --path crates/lineage-cli`) and optionally run `tribal init` for hooks and agent skills. Init-config is only needed if `refs/lineage/config` was not fetched.

Verify access:

```bash
tribal list --json
tribal search "module name"
```

## Large content

Sessions with long tool output or images require LFS objects locally. Always pair ref push with `tribal lfs push` and advise teammates to `lfs fetch`. See [Large content (LFS)](lfs.md).

## Review before public release

Open-source or public mirrors need extra care:

```bash
tribal export --redact --format jsonl > review.jsonl
```

Read [Privacy and policy](privacy.md). Remove or delete sensitive sessions (`tribal delete`) before pushing if review finds issues.

## VS Code and MCP

Teammates can use the [VS Code extension](vscode.md) or [MCP server](mcp/README.md) against the same refs after fetch. No separate Tribal account or cloud sync is required.

## Rebases and force-push

If your team rebases shared branches, run `tribal remap` after rewrite and push updated notes. See [After a rebase](rebase.md).

## Removing shared data

Deleting a session locally and pushing updated refs removes it for future clones. Rewriting public history to expunge blobs may still require git history remediation — treat lineage refs like any other sensitive git data.

## Related guides

- [Explore](explore.md)
- [Configuration](configuration.md)
- [Maintenance](maintenance.md)
