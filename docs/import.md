# Import agent sessions

[← Documentation index](README.md) · [Agent paths](agent-paths.md) · [Configuration](configuration.md)

Import discovers agent transcripts on disk, normalizes them to Lineage conversations, applies policy, and writes git refs. This is the primary way session provenance enters your repository.

## Quick start

```bash
# All supported agents
git lineage import --agent all

# One agent
git lineage import --agent cursor
git lineage import --agent claude
git lineage import --agent codex
```

Alias: `git lineage ingest` (same command).

## First import vs incremental

**First import** pulls every discoverable session for the workspace and links per [Configuration](configuration.md) `commit_mapping` (default: intelligent auto-mapping to recent commits).

**Incremental import** skips sessions already recorded unless the source transcript file changed:

```bash
git lineage import --agent all --incremental
```

Hooks use incremental import on every commit. Day-to-day workflow: enable hooks or run incremental import before pushing.

## Useful flags

| Flag | Purpose |
|------|---------|
| `--since 2026-01-01` | Only sessions started on or after date (RFC 3339 or `YYYY-MM-DD`) |
| `--incremental` | Skip unchanged already-imported sessions |
| `--no-link-head` | Import without linking to current `HEAD` (used by pre-commit hook) |

Example bounded import:

```bash
git lineage import --agent claude --since 2026-03-01 --incremental
```

## What happens during import

1. **Discover** — adapters scan [agent paths](agent-paths.md) scoped to the repo working directory.
2. **Read** — vendor JSONL becomes canonical conversation JSON.
3. **Policy** — redaction, path excludes, private session marking ([Privacy](privacy.md)).
4. **Persist** — conversation blob + `refs/lineage/sessions/<id>` + manifest update.
5. **Compact** — large turn bodies go to LFS when above threshold ([LFS](lfs.md)).
6. **Link** — assign `commit_shas` and materialize line objects when mapping succeeds.
7. **Index** — search index updated for new text.

## Code-only sessions

By default (`import_only_code_sessions: true`), sessions without file edits or write-tool usage are skipped. Planning-only chats are excluded unless you change config. See [Configuration](configuration.md).

## Author attribution

Import stamps `prompted_by_email` and `prompted_by_name` from the repository git `user.email` and `user.name` at import time. Values are preserved across re-import so teams can see who ran each agent session.

## Setup integration

`git lineage init` can run the first import interactively. Non-interactive:

```bash
git lineage init --yes
git lineage init --yes --no-import
```

Manual import any time:

```bash
git lineage import --agent all --incremental
```

## Automatic import

[Git hooks](git-hooks.md) run incremental import on pre-commit and link sessions on post-commit. Recommended for keeping lineage current without remembering commands.

## Troubleshooting

| Issue | What to check |
|-------|----------------|
| `discovered 0 session(s)` | [Agent paths](agent-paths.md); run from repo root; `git lineage doctor` |
| Sessions missing from manifest | `import_only_code_sessions`; session may have no code edits |
| No blame after import | Commit with hooks or `git lineage link` / `materialize` |
| Secrets in session | [Privacy](privacy.md); tighten `exclude_paths` |

## Related guides

- [Explore](explore.md) — list and search after import
- [Maintenance](maintenance.md) — materialize and link manually
- [CLI reference](cli/README.md)
