# Schemas

[← Documentation index](README.md) · [Architecture](ARCHITECTURE.md) · [specs/](../specs/)

Tribal domain data uses explicit versioned schemas. The `specs/` directory is the source of truth; Rust types in `lineage-core` and JSON blobs in git refs must match.

## Schema documents

| Spec | Version constant | Stored at |
|------|------------------|-----------|
| [conversation-schema-v0](../specs/conversation-schema-v0.md) | `conversation-v0` | `refs/lineage/sessions/<id>` blobs |
| [line-object-schema-v0](../specs/line-object-schema-v0.md) | `line-object-v0` | `refs/lineage/lines/<id>` blobs |
| [git-notes-schema-v0](../specs/git-notes-schema-v0.md) | (note payload) | `refs/notes/lineage` |
| Repository config | `lineage-config-v0` | `refs/lineage/config` |
| Last import state | `lineage-last-import-v0` | `refs/lineage/last-import` |

Read the spec files for field-level definitions, examples, and invariants.

## Conversation overview

A conversation contains:

- Stable `session_id` and `schema_version`
- `agent` kind (cursor, claude, codex, …)
- Ordered `turns` with role, content, tool calls, and artifacts
- `metadata` (architecture summary, prompter identity, vendor ids, branch, privacy flags)
- `commit_shas` linking to git history

Sessions are immutable blobs addressed by ref. Re-import updates by writing a new blob and moving the ref when content changes.

## Line object overview

Line objects connect a file path and line range to a specific turn and artifact slice at a commit. They power lineage blame and gutter decorations. Materialization resolves artifacts (patches, citations, search/replace blocks) against the commit tree.

## Git notes overview

Notes attach to commit OIDs and index which session and line object refs apply at that commit. Notes carry patch-id metadata used by [rebase remap](rebase.md).

## Changing schemas

1. Edit the relevant `specs/*.md` file first.
2. Decide: backward-compatible v0 extension vs new `*-v1` schema.
3. Update `lineage-core` types and serde tests.
4. Update persist/read paths in `lineage-git`, adapters, CLI JSON output, MCP tools, and VS Code types.
5. Document migration expectations in spec and [CHANGELOG.md](../CHANGELOG.md).
6. Refresh fixtures and golden tests.

Breaking changes require a new schema version string and a migration story. Do not silently rename fields in place.

## JSON CLI output

Commands with `--json` emit serde-shaped structures aligned with specs. If you add public JSON fields, update [CLI reference](cli/README.md) and consumer docs.

## Validation

Import and read paths validate `schema_version` and fail with explicit errors on mismatch. `git lineage doctor` helps detect broken refs but does not replace schema review in PRs.

## Related guides

- [How it works](how-it-works.md)
- [Configuration](configuration.md)
- [Developing](developing.md)
- [Testing](testing.md)
