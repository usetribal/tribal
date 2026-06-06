# Agent adapters

[← Documentation index](README.md) · [Agent paths](agent-paths.md) · [Architecture](ARCHITECTURE.md)

Adapters translate vendor agent transcript formats into Lineage's canonical conversation schema. They live in `lineage-adapters` and are invoked by the import pipeline in `lineage-agent`.

## Supported agents

| Agent | Feature flag | Discovery scope |
|-------|--------------|-----------------|
| Cursor | `cursor` (default) | Project `.cursor/` paths and user-level project transcripts |
| Claude Code | `claude` (default) | `.claude/projects/<encoded-path>/` JSONL sessions |
| Codex | `codex` (default) | `.codex/sessions/` with workspace `cwd` filtering |

See [Agent paths](agent-paths.md) for the full path list.

## Import pipeline

```text
Adapter discovers SessionRef entries on disk
      ↓
SessionReader loads vendor JSONL → Conversation (lineage-core)
      ↓
Policy redacts and filters artifacts
      ↓
lineage-git persists blobs and updates refs
```

Adapters never write directly to git. They only produce normalized `Conversation` values.

## Trait responsibilities

### AgentSource

- Returns the `AgentKind` identifier.
- Discovers candidate sessions under the repository working directory.

### SessionReader

- Reads a discovered session file into `Conversation`.
- Maps vendor roles, tool calls, artifacts, and metadata fields.
- Sets stable `session_id` derivation consistent with re-import.

Both traits are implemented per agent module and registered behind feature flags.

## Adding a new adapter

1. Create a module under `crates/lineage-adapters/src/`.
2. Implement `AgentSource` and `SessionReader`.
3. Map vendor JSONL (or other format) to `conversation-v0` fields documented in [Schemas](schemas.md).
4. Add a feature flag in `lineage-adapters/Cargo.toml`.
5. Register in `all_adapters(workspace_root)` behind `#[cfg(feature = "...")]`.
6. Add sanitized fixtures under `tests/fixtures/<agent>-history/`.
7. Extend `crates/lineage-adapters/tests/all_fixtures.rs` with golden expectations.
8. Wire the agent name into CLI `import --agent` parsing and documentation.

Enable the feature in workspace consumers if the adapter ships by default.

## Mapping guidelines

- Preserve `vendor_session_id` when the agent provides one (needed for resume/fork).
- Capture `git_branch`, model name, and timestamps in session metadata when available.
- Normalize paths in artifacts relative to the repository root where possible.
- Skip vendor noise files (Claude progress/snapshot types, empty rolls).
- Run citations and image enrichment hooks shared across adapters where applicable.

## Feature flags

Default build includes `cursor`, `claude`, and `codex`. Optional adapters stay behind empty feature flags until stable:

```toml
[features]
default = ["cursor", "claude", "codex"]
myagent = []
```

## Testing

```bash
cargo test -p lineage-adapters
cargo test -p lineage-adapters --test all_fixtures
```

Never commit real transcripts containing secrets. Copy structure from existing fixtures and redact content.

## Commit mapping interaction

Imported conversations receive `commit_shas` based on `commit_mapping` in [Configuration](configuration.md). Adapters should attach file paths and edit artifacts accurately so auto-mapping and line-object materialization can succeed.

## Related guides

- [Import](import.md)
- [Schemas](schemas.md)
- [Testing](testing.md)
- [Developing](developing.md)
