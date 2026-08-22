# Explore your lineage

[← Documentation index](README.md) · [Import](import.md) · [Continue a session](continue-a-session.md)

After import, query sessions by id, commit, file line, or full-text search. Prefer `--json` for scripts and agent tooling.

## List sessions

```bash
git lineage list
git lineage list --json
git lineage list --commit <sha> --json
```

Human output shows session ids, agents, and summary metadata. JSON includes prompter fields, branch, linked commits, and timestamps.

## Show a conversation

```bash
git lineage show <session-id>
git lineage show <session-id> --json
git lineage show <session-id> --hydrate-images
```

Show hydrates large LFS-backed turn content automatically. Use `--hydrate-images` when reviewing image artifacts in the terminal or piping to other tools.

## Tribal blame

Combines `git blame` with lineage notes at the introducing commit:

```bash
git lineage blame src/main.rs:42
git lineage blame path/to/file.py:10 --json
```

JSON `matches` include confidence scores and content previews linking turns to line ranges. Blame requires line objects materialized at the commit — import, hooks, or `git lineage materialize`.

### Suggested workflow for agents

1. `git lineage search "topic"` to find candidate sessions.
2. `git lineage blame file:line` for code you are editing.
3. `git lineage show <session-id> --json` for full decision context.

## Full-text search

```bash
git lineage search "authentication middleware"
```

Search uses a local SQLite FTS index (`.git/lineage/index.db`). The index rebuilds automatically when stale, or run `git lineage rebuild index` explicitly.

Search indexes conversation text after policy; private stripped content is not searchable after export-style clearing.

## Sessions by commit

```bash
git lineage list --commit abc1234 --json
```

Shows sessions linked to a specific commit SHA via git notes. Useful in code review: see which agent conversations contributed to a changeset.

## Architecture summaries

Some sessions include `metadata.architecture_summary` — a heuristic overview generated at import. Use show JSON to read it when onboarding to unfamiliar modules.

## VS Code and MCP

The [VS Code extension](vscode.md) provides a session tree, timeline webview, gutter icons, and hover blame. The [MCP server](mcp/README.md) exposes `lineage_search`, `lineage_get_session`, `lineage_blame_line`, and `lineage_list_sessions` for in-editor agents.

## When nothing appears

- Run `git lineage import --agent all --incremental`.
- Fetch team refs: `git fetch origin refs/lineage/* refs/notes/lineage`.
- Rebuild index: `git lineage rebuild index`.
- Confirm [agent paths](agent-paths.md) if local import finds zero sessions.

## Related guides

- [How it works](how-it-works.md) — data model behind blame and search
- [Share](share.md) — teammate access to the same sessions
- [CLI reference](cli/README.md)
