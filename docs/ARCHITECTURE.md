# Architecture

This document describes how Lineage is structured and how data flows through the system.

## Design goals

1. **Git-native** — lineage is stored as git objects (blobs, refs, notes), not an external database
2. **Rebuildable** — the search index and local caches can be reconstructed from git refs
3. **Policy-first** — sensitive content is redacted before persistence
4. **Agent-agnostic** — a canonical conversation schema; adapters translate vendor formats

## Invariants

Design goals describe what Lineage aims for; these are the rules a change must not
break. They are load-bearing — each exists because violating it produced a real defect.
If a change appears to require breaking one, that is a signal the model needs extending,
not that the rule needs an exception.

### 1. The provenance graph is deterministic

Every node and edge must be computable from data that is already durably stored — the
conversation refs, the git history, and the notes. The same inputs must yield the same
graph on any machine, at any time, for any user.

No node or edge may depend on a heuristic whose output could reasonably differ between
runs, on a model's judgement, or on an inference nobody can reproduce. A relation that
cannot be computed deterministically does not belong in the graph. It may belong in a
derived, clearly-labelled layer above it — but the graph itself stays reproducible,
because everything downstream (attribution, blame, sync, team sharing) assumes two people
looking at the same repo see the same provenance.

### 2. The provenance graph is backfillable

Prefer relations that can be reconstructed from history over relations that must be
captured as work happens. `rebuild` and re-import are the upgrade path (see **Release
status** in [AGENTS.md](../AGENTS.md)), and that only holds while the graph can be
rederived from what git already contains.

An "as-you-go" capture mechanism — one that only records the edge if the tool was
installed and running at the moment the work occurred — creates provenance that existing
repositories can never gain. It splits the corpus into instrumented and dark regions and
makes coverage a function of adoption date rather than of history. When a new relation is
proposed, ask first whether it can be backfilled; if it cannot, that is a real cost to
weigh, not a detail.

### 3. Rendering decides how to show, never what to show

A surface that displays provenance — the injected digest, `git lineage show`, the MCP
tool responses, the VS Code panel, the web UI — formats what it is given. It must not
select, substitute, or infer content.

Concretely: if evidence names turn N, the surface renders turn N's text. It does not
quote a neighbouring turn because that one reads better, and it does not silently swap
one node for another. Doing so makes the rendered output inconsistent with the identifiers
beside it, so any follow-up traversal on those identifiers returns something the reader
did not see. Choosing *which* node is evidence is a retrieval and graph concern, expressed
through typed relations and salience — both of which are inspectable and testable.
Formatting, truncating, ordering, and laying out are rendering concerns.

When a surface looks thin, the fix is upstream: a missing edge, or a selection rule that
should be explicit. It is never a substitution made at render time.

### 4. Harness-specific knowledge lives only in `lineage-adapters`

Transcript file formats, state directories (`~/.claude/projects/`, `~/.codex/sessions/`),
settings-file locations, CLI flags, project-key encodings, and vendor id conventions
belong to the adapter for that harness and nowhere else. Everything above the adapter
layer speaks `Conversation` and handles the adapter itself returned.

The cost of breaking this is paid at the *next* harness, not at the change that breaks
it. A `.claude` path written into a CLI command works perfectly until a second harness
needs the same command, at which point there is no seam to extend — only a working
implementation to copy and re-specialise. Each such copy multiplies: adding a harness
stops being "write an adapter" and becomes "find every place the last harness was
special-cased". The layering exists so `AgentKind` is the only thing above the adapters
that names a vendor.

This invariant is stated aspirationally: the current code does not fully satisfy it.
Four known leaks sit in `lineage-cli` — `context_cmd.rs` (Claude settings file),
`init_cmd.rs` (`.claude/skills/` and settings paths), `skill_cmd.rs` (per-harness skill
paths), and `doctor_cmd.rs` (`.claude/settings.json` in a diagnostic). They are filed as
debt, and they are the evidence for the rule rather than an exception to it. The rule's
job is to stop the pile growing.

When a change appears to need harness knowledge above the adapter layer, the fix is a new
adapter capability that returns what the caller needs, not a vendor branch in the caller.
That is what the transcript writer is: continuing a session needs a vendor-native file in
a vendor-specific place, so the adapter produces both and the caller learns neither.

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

## Import flow

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
6. lineage-git updates manifest (refs/lineage/index) and last-import state (refs/lineage/last-import)
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
| `refs/lineage/last-import` | Last import timestamp and session IDs (for hook linking) |
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
| Continuing a session in its harness | Implement `TranscriptWriter` in `lineage-adapters` — renders a `Conversation` to a vendor-native transcript and returns the handle needed to open it. Optional: an adapter without one declines explicitly |
| New storage backend | Implement `ObjectStore` in `lineage-store` |
| Custom policy rules | Extend `PolicyConfig` with `RedactionRule` and `ExcludePattern` |
| Schema evolution | New `*-v1` schema in `specs/` with migration tooling |

## Related documents

- [conversation-schema-v0](../specs/conversation-schema-v0.md)
- [line-object-schema-v0](../specs/line-object-schema-v0.md)
- [git-notes-schema-v0](../specs/git-notes-schema-v0.md)
- [CONTRIBUTING.md](../CONTRIBUTING.md)
