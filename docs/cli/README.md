# CLI reference

[← Documentation index](../README.md) · [Setup](../../README.md#setup)

`git lineage` is the primary interface for importing, querying, sharing, and maintaining agent provenance in a git repository.

## Install

```bash
cargo install --path crates/lineage-cli
# or: make setup
```

Ensure `~/.cargo/bin` is on your `PATH` so `git lineage` resolves. The binary name is `git-lineage`.

Target another repository:

```bash
git lineage --repo /path/to/repo <command>
```

---

## Project setup

```bash
git lineage init                    # interactive wizard
git lineage init --yes              # non-interactive defaults
git lineage init --no-import        # skip first import
git lineage init-config             # write default refs/lineage/config only
```

### Agent skill

Bundled skill teaches agents to use lineage features (search, blame, share, rebase, resume). Installed during init or manually:

```bash
git lineage init-skill
git lineage init-skill --target cursor
git lineage init-skill --target claude --target codex
git lineage init-skill --target all --force
```

| Target | Install path |
|--------|--------------|
| `cursor` | `.cursor/skills/lineage/SKILL.md` |
| `claude` | `.claude/skills/lineage/SKILL.md` |
| `codex` / `agents` | `.agents/skills/lineage/SKILL.md` |
| `all` | All three (default) |

Re-run with `--force` after upgrading the CLI to refresh skill content.

---

## Command reference

### Setup and health

| Command | Description |
|---------|-------------|
| `git lineage doctor` | Check config, refs, notes, and LFS integrity |
| `git lineage init [options]` | Interactive setup: config, skill, hooks, optional import |
| `git lineage init-config` | Write default `refs/lineage/config` |
| `git lineage init-skill [options]` | Install bundled agent skill |

### Import

| Command | Description |
|---------|-------------|
| `git lineage import [options]` | Import agent history (`ingest` alias) |

Flags: `--agent cursor|claude|codex|all`, `--since DATE`, `--incremental`, `--no-link-head`.

See [Import](../import.md).

### Query

| Command | Description |
|---------|-------------|
| `git lineage list [--commit SHA] [--json]` | List sessions or sessions at commit |
| `git lineage show <id> [--json] [--hydrate-images]` | Show conversation |
| `git lineage blame <path>[:line] [--json]` | Lineage for a file line |
| `git lineage search <query>` | Full-text search (auto-rebuilds stale index) |
| `git lineage rebuild-index` | Rebuild search index from refs |
| `git lineage export [--redact] [--format json\|jsonl]` | Export sessions |

See [Explore](../explore.md).

### Linking and history

| Command | Description |
|---------|-------------|
| `git lineage link <session-id> <commit-sha>` | Manually link session and materialize |
| `git lineage materialize [--commit SHA] [--session ID]` | Build line objects |
| `git lineage remap` | Recover lineage after rebase |

See [Rebase](../rebase.md) and [Maintenance](../maintenance.md).

### LFS

| Command | Description |
|---------|-------------|
| `git lineage lfs status` | Referenced vs local LFS objects |
| `git lineage lfs push [--remote origin]` | Push LFS refs |
| `git lineage lfs fetch [--remote origin]` | Fetch missing LFS objects |

See [LFS](../lfs.md).

### Hooks

| Command | Description |
|---------|-------------|
| `git lineage install-hook [--force]` | Install pre-commit and post-commit hooks |
| `git lineage uninstall-hook` | Remove lineage hooks |

See [Git hooks](../git-hooks.md).

### Cleanup

| Command | Description |
|---------|-------------|
| `git lineage delete <session-id> [--purge-blobs]` | Remove session and related objects |
| `git lineage gc` | Purge orphan line objects and unreferenced LFS blobs |

See [Maintenance](../maintenance.md) and [Privacy](../privacy.md).

---

## Session metadata

Import records `prompted_by_email` and `prompted_by_name` from git `user.email` / `user.name`. Sessions may include `vendor_session_id` for resume/fork, `parent_session_id`, `git_branch`, and `architecture_summary` metadata.

Schema details: [Schemas](../schemas.md).

## Related guides

- [Configuration](../configuration.md)
- [VS Code extension](../vscode.md)
- [MCP server](../mcp/README.md)
