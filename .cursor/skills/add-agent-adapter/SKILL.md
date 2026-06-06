---
name: add-agent-adapter
description: >-
  Adds or modifies agent source adapters in lineage-adapters (Cursor, Claude,
  Codex, or new agents). Covers AgentSource/SessionReader traits, feature flags,
  fixture golden tests, and transcript path discovery. Use when adding agent
  support, changing import discovery, or mapping vendor JSONL to Conversation.
---

# Add agent adapter

## Workflow

1. Add `crates/lineage-adapters/src/<agent>.rs`
2. Implement `AgentSource` + `SessionReader` from `lineage-agent`
3. Map vendor format → `lineage_core::Conversation` (`conversation-v0` schema)
4. Add feature flag in `crates/lineage-adapters/Cargo.toml` (e.g. `adapter-foo = []`)
5. Register in `all_adapters()` in `lib.rs` behind `#[cfg(feature = "...")]`
6. Add fixture under `tests/fixtures/<agent>-history/`
7. Extend `crates/lineage-adapters/tests/all_fixtures.rs` golden test

## Trait surface

```rust
// lineage-agent
trait AgentSource {
    fn agent(&self) -> AgentKind;
    fn discover(&self) -> Result<Vec<SessionRef>>;
}
trait SessionReader {
    fn read(&self, session: &SessionRef) -> Result<Conversation>;
}
```

Wrap via `ErasedAdapterImpl` in `lib.rs` — follow `cursor`, `claude`, `codex` modules.

## Transcript discovery paths

| Agent | Locations |
|-------|-----------|
| Cursor | `.cursor/projects/*/agent-transcripts/`, `~/.cursor/projects/<key>/agent-transcripts/` |
| Claude | `.claude/projects/<encoded-path>/*.jsonl` (skip snapshot/progress files) |
| Codex | `.codex/sessions/`, `~/.codex/sessions/` |

Encoded path: `pwd -P | sed 's|/|-|g'` (e.g. `-Users-you-project`).

Scope discovery to the **repository working directory**.

## Feature flags

```toml
# crates/lineage-adapters/Cargo.toml
[features]
default = ["cursor", "claude", "codex", "foo"]
foo = []
```

Enable in `lineage-cli` / workspace consumers if the adapter ships by default.

## Tests

- Unit tests in adapter module for parsing edge cases
- `tests/all_fixtures.rs` — read fixture, assert stable session id / turn count / golden JSON snapshot
- Use existing fixtures as templates: `tests/fixtures/claude-code-history/`, `cursor-history/`

## Do not

- Persist raw vendor files — normalize to `Conversation` first
- Skip policy pass — import pipeline applies `lineage-policy` after read
- Commit real agent transcripts with secrets — use sanitized fixtures only
