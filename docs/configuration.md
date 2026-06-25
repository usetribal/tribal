# Configuration

[← Documentation index](README.md) · [Privacy and policy](privacy.md) · [Import](import.md)

Lineage stores repository settings in a git ref: `refs/lineage/config`. The ref holds a JSON document (`lineage-config-v0`) that controls import behavior, privacy, blob storage, and commit linking. It is written during `git lineage init` or `git lineage init-config`.

Configuration travels with the repository when you push `refs/lineage/*`. Teammates receive the same policy after fetch.

## Viewing configuration

There is no dedicated show command today. Read the ref directly:

```bash
git show refs/lineage/config
```

Or use `git lineage doctor`, which reports whether config exists and whether required refs are healthy.

## Configuration fields

| Field | Default | Purpose |
|-------|---------|---------|
| `import_only_code_sessions` | `true` | Skip sessions with no detected file edits or write-tool use |
| `commit_mapping` | `auto` | How imported sessions link to commits (`auto`, `head`, `none`) |
| `large_blob_threshold_bytes` | `1048576` (1 MiB) | Turn content above this size is stored outside the conversation blob |
| `large_blob_backend` | `lfs` | Where large content lives (`lfs` or legacy `cache`) |
| `lfs_transport` | `auto` | How LFS objects move to remotes (`auto`, `gitcli`, `http`, `refs`) |
| `exclude_paths` | `.env`, `*.pem`, `*credentials*` | Glob patterns; matching artifacts are dropped at import |
| `exclude_content_patterns` | `[]` | Patterns matched against turn text; matching turns are cleared |
| `private_session_patterns` | `*private*` | Source paths matching these mark sessions private |
| `strip_private_on_export` | `true` | Private sessions export with empty turns when redaction is on |

Legacy key `ingest_only_code_sessions` is accepted as an alias for `import_only_code_sessions`.

## Commit mapping modes

| Mode | Behavior |
|------|----------|
| `auto` | Score recent commits using file overlap, timing, and branch metadata; link to the best match |
| `head` | Always link imported sessions to the current `HEAD` commit |
| `none` | Import without auto-linking; use hooks, `git lineage link`, or manual materialize |

Hooks typically import with `--no-link-head` on pre-commit, then post-commit linking attaches sessions to the new commit. This pairs well with `auto` or `head` depending on your workflow.

## Large content

Sessions can include long turn text, tool output, and images. Content above `large_blob_threshold_bytes` is compacted into Git LFS (default) or a local cache backend. Conversation JSON in git refs keeps pointers, not the full payload.

See [Large content (LFS)](lfs.md) for push, fetch, and transport details.

## Editing configuration

1. Read the current JSON from `refs/lineage/config`.
2. Edit fields as needed.
3. Write the updated JSON back to the same ref (via a small script, custom tooling, or future CLI commands).

Invalid JSON or an unknown `schema_version` causes import and doctor checks to fail until corrected. Prefer small, reviewable changes and test with `git lineage doctor` after updates.

## Relationship to policy

Configuration drives the policy engine at import time:

- `exclude_paths` and `exclude_content_patterns` extend default artifact and content filters.
- `private_session_patterns` mark sessions that should not expose turn content on export.
- Built-in gitleaks rules (vendored from upstream `gitleaks.toml`) always run regardless of config. Optional `redaction_rules` in policy extend that set.

See [Privacy and policy](privacy.md) for the full picture.

## When to change what

| Goal | Fields to adjust |
|------|------------------|
| Import planning-only chats | Set `import_only_code_sessions` to `false` |
| Never auto-link on import | `commit_mapping`: `none` |
| Always link to latest commit | `commit_mapping`: `head` |
| Drop `.env` artifacts | Add patterns to `exclude_paths` |
| Hide sessions from exports | Add source globs to `private_session_patterns` |
| Push large blobs without git-lfs CLI | `lfs_transport`: `http` or `refs` |

## Related guides

- [Import](import.md) — flags and incremental behavior
- [Git hooks](git-hooks.md) — automatic import and linking
- [Architecture](ARCHITECTURE.md) — where config fits in the import pipeline
