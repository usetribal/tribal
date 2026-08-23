---
name: schema-change
description: >-
  Changes Tribal domain schemas (Conversation, LineObject, git notes, repo
  config). Requires specs/ updates first, lineage-core type changes, migration
  notes, and downstream crate/test updates. Use when modifying specs/*.md,
  lineage-core types, JSON output shapes, or schema_version fields.
---

# Schema change

## Order of operations

1. **Edit `specs/` first** — source of truth
   - `specs/conversation-schema-v0.md`
   - `specs/line-object-schema-v0.md`
   - `specs/git-notes-schema-v0.md`
2. Decide: in-place v0 change vs new version (`conversation-v1`, etc.)
3. Update `lineage-core` types and constants (`CONVERSATION_SCHEMA`, etc.)
4. Update consumers: `lineage-git`, `lineage-adapters`, `lineage-cli` JSON output, `lineage-mcp`
5. Migration notes in spec + `CHANGELOG.md`
6. Fixture/golden test updates under `tests/fixtures/`

## Breaking vs non-breaking

| Change | Action |
|--------|--------|
| New optional field | Update spec + types; old blobs still deserialize |
| Renamed/removed field | New schema version + migration |
| Notes/ref layout change | New git-notes schema version; remap/migration tooling |

## Downstream checklist

- [ ] `lineage-core` types + serde tests
- [ ] `lineage-git` persist/read paths
- [ ] `lineage-adapters` mapping to new fields
- [ ] `lineage-cli` `--json` output documented in `docs/cli/README.md` if public
- [ ] `lineage-mcp` tool responses
- [ ] `extensions/vscode` types in `src/types.ts` if UI exposes field
- [ ] Integration tests in `lineage-git/tests/`

## Repo config (`refs/lineage/config`)

Config schema changes live in `lineage-git` config types + `README.md` or `docs/ARCHITECTURE.md` as appropriate. Defaults via `tribal init` or `tribal init --config`.

## Changelog

Under `[Unreleased]`, note breaking changes explicitly and link to spec version.
