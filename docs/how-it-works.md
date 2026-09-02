# How it works

[← Documentation index](README.md) · [Architecture](ARCHITECTURE.md) · [Schemas](schemas.md)

Tribal stores agent session history inside your git object database. No separate Tribal server holds your conversations — refs and notes travel with `git push`.

## Three persisted layers

### 1. Conversations

Normalized agent sessions live as JSON blobs referenced by `refs/lineage/sessions/<session-id>`. Each conversation includes turns (user, assistant, tool), artifacts (files touched, patches, images), metadata, and linked commit SHAs.

Schema: [conversation-schema-v0](../specs/conversation-schema-v0.md).

### 2. Line objects

Line objects map a file path and line range to a specific turn and artifact at a commit. They power lineage blame and editor gutter hints. Stored at `refs/lineage/lines/<line-object-id>`.

Schema: [line-object-schema-v0](../specs/line-object-schema-v0.md).

Materialization runs when sessions link to commits and artifacts can be resolved against the commit tree (patch apply, citation ranges, search/replace blocks).

### 3. Git notes

`refs/notes/lineage` attaches to commit OIDs. Each note lists session ids and line object ids relevant at that commit. Notes include patch-id metadata for [rebase remap](rebase.md).

Schema: [git-notes-schema-v0](../specs/git-notes-schema-v0.md).

## Supporting refs

| Ref | Role |
|-----|------|
| `refs/lineage/index` | Manifest of all known session ids |
| `refs/lineage/config` | Import and policy settings |
| `refs/lineage/last-import` | Incremental import bookkeeping for hooks |
| `refs/lineage/lfs/*` | Large content pointers and transport |

## Local caches (not pushed)

| Path | Role |
|------|------|
| `.git/lineage/index.db` | SQLite FTS search index — rebuildable |
| `.git/lfs/objects/` | Git LFS store for large turn bodies and media |

Teammates rebuild search indexes locally after fetch. See [Explore](explore.md).

## End-to-end flow

### Import path

```text
Transcript on disk → adapter → policy → git blob + session ref → optional link + line objects → search index
```

### Blame path

```text
git blame → introducing commit → git note → line objects + conversation → matching turns
```

### Search path

```text
Query → FTS index → session ids → load conversations from refs
```

Details: [Architecture](ARCHITECTURE.md).

## Policy layer

Nothing reaches git refs before redaction and excludes run. Configuration at `refs/lineage/config` extends default rules. See [Privacy and policy](privacy.md).

## Interfaces

The same refs back all interfaces:

- **CLI** — import, query, maintain ([CLI reference](cli/README.md))
- **MCP** — agent tool access ([MCP](mcp/README.md))
- **VS Code** — tree, timeline, hover ([VS Code](vscode.md))

## Related guides

- [Import](import.md)
- [Configuration](configuration.md)
- [Large content (LFS)](lfs.md)
