# How it works

[← Back to README](../README.md) · [Architecture](ARCHITECTURE.md)

Lineage stores three kinds of data inside your git repository:

1. **Conversations** at `refs/lineage/sessions/<id>` (normalized agent sessions as JSON blobs)
2. **Line objects** at `refs/lineage/lines/<id>` (mappings from file lines to conversation turns)
3. **Git notes** at `refs/notes/lineage` (per-commit indexes linking sessions and line objects)

A manifest at `refs/lineage/index` lists all known sessions. Repository policy lives at `refs/lineage/config`. Search uses a local SQLite index at `.git/lineage/index.db`. Large artifacts are stored in Git LFS by default.

**Schemas:** [conversation-schema-v0](../specs/conversation-schema-v0.md) · [line-object-schema-v0](../specs/line-object-schema-v0.md) · [git-notes-schema-v0](../specs/git-notes-schema-v0.md)
