# CLI & agent skill

[← Back to README](../../README.md) · [Setup](../../README.md#setup)

Lineage ships as `git lineage` — a git subcommand for ingesting, querying, and sharing agent provenance in your repository.

## Install

```bash
cargo install --path crates/lineage-cli
# or: ./scripts/setup.sh
```

Ensure `~/.cargo/bin` is on your `PATH` so `git lineage` resolves.

---

## Agent skill

The CLI bundles an **agent skill** so coding agents know how to retrieve engineering context — prior conversations, architecture decisions, and line-level provenance.

Install into your project (also run automatically by `./scripts/setup.sh`):

```bash
git lineage init-skill                    # all targets (default)
git lineage init-skill --target cursor    # Cursor only
git lineage init-skill --target claude --target codex
git lineage init-skill --target all --force
```

| Target | Install path | Docs |
|--------|--------------|------|
| `cursor` | `.cursor/skills/lineage/SKILL.md` | [Cursor Skills](https://cursor.com/docs/skills) |
| `claude` | `.claude/skills/lineage/SKILL.md` | [Claude Code](https://code.claude.com/docs/en/claude-directory) |
| `codex` / `agents` | `.agents/skills/lineage/SKILL.md` | [Codex customization](https://developers.openai.com/codex/concepts/customization) |
| `all` | All three (default when no `--target` is given) | — |

The same bundled skill ([`crates/lineage-cli/assets/skills/lineage/SKILL.md`](../../crates/lineage-cli/assets/skills/lineage/SKILL.md)) is copied verbatim to each path — it tells agents to run `git lineage search`, `blame`, and `show --json` before answering *why* code exists. Re-run with `--force` to refresh after upgrading the CLI.

---

## Command reference

| Command | Description |
|---------|-------------|
| `git lineage doctor` | Check repo configuration and session integrity |
| `git lineage init-config` | Write default `refs/lineage/config` (policy, excludes, blob threshold) |
| `git lineage init-skill [--target cursor\|claude\|codex\|agents\|all] [--force]` | Install bundled agent skill (default: all targets) |
| `git lineage ingest [--agent cursor\|claude\|codex\|all] [--since DATE] [--incremental]` | Ingest agent history into git refs |
| `git lineage list [--commit SHA] [--json]` | List sessions, or sessions linked to a commit |
| `git lineage show <id> [--json] [--hydrate-images]` | Display a session |
| `git lineage blame <path>[:line] [--json]` | Show which agent turn touched a line |
| `git lineage search <query>` | Full-text search over ingested sessions (auto-rebuilds stale index) |
| `git lineage rebuild-index` | Rebuild the local search index from git refs |
| `git lineage export [--redact] [--format json\|jsonl]` | Export sessions |
| `git lineage link <session-id> <commit-sha>` | Manually link a session to a commit (materializes line objects) |
| `git lineage materialize [--commit SHA] [--session ID]` | Build line objects from session artifacts at a commit |
| `git lineage remap` | Remap orphaned lineage notes after rebase (patch-id aware) |
| `git lineage lfs status` | Show referenced vs local LFS objects |
| `git lineage lfs push [--remote origin]` | Push LFS pointer and data refs |
| `git lineage lfs fetch [--remote origin]` | Fetch missing LFS objects from remote |
| `git lineage delete <session-id> [--purge-blobs]` | Remove session, line objects, notes entries; refcount-aware LFS purge |
| `git lineage gc` | Purge orphan line objects and unreferenced LFS blobs |
| `git lineage install-hook [--force]` | Install pre-commit and post-commit hooks |
| `git lineage uninstall-hook` | Remove lineage git hooks |

Use `--repo <path>` to target a repository other than the current directory.

### Session author metadata

At ingest, Lineage stamps `prompted_by_email` and `prompted_by_name` from the repository git config (`user.email`, `user.name`). Values are preserved on re-ingest so team members can see who had each agent conversation. See [conversation-schema-v0](../../specs/conversation-schema-v0.md).
