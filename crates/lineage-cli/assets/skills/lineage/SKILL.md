---
name: lineage
description: >-
  Retrieves and manages engineering context from tribal: AI agent
  conversations, line-level provenance, team sharing, rebase recovery, and
  session resume/fork. Use when answering why code exists, finding past
  decisions, onboarding, before risky edits, after rebase, or when sharing
  lineage with teammates.
---

# Tribal agent skill

Tribal stores **AI agent session provenance** in git (`refs/lineage/*`, `refs/notes/lineage`). Setup is handled by `tribal init` (humans run that once). **Your job is to query, interpret, and advise on lineage data.**

## When to use lineage

- Why was this code written? What prompt or decision led to it?
- What did agents discuss about this module, API, or bug?
- Before editing unfamiliar code: linked sessions, blame, architecture summary
- After `git rebase` / `git rebase -i`: orphaned lineage notes
- Sharing context with teammates or reviewing what would leave the repo
- Continuing a prior agent session (Claude Code, Codex; VS Code extension)

## Always pass `--no-interactive`

You are not at a terminal. `--no-interactive` is global — every command takes it —
and guarantees no command opens a session selector or asks you a question. Without
it, `tribal list` and `tribal show` open a TUI for a human.

Add `--json` on top when you need to parse the output. `--no-interactive` says you
are a machine; `--json` pins the shape. Use both when you parse, `--no-interactive`
alone when you only need to read.

```bash
tribal context query "authentication middleware" --no-interactive
tribal list --no-interactive --json
tribal list --commit <sha> --no-interactive --json
tribal show <session-id> --no-interactive --json
tribal blame path/to/file.rs:42 --no-interactive --json
tribal export --redact --format jsonl
```

`tribal --discover` prints the whole command surface as JSON, including commands
that are hidden from `--help`. Read it when you need a verb you cannot guess.

**Workflow:** query by topic, then blame for a specific line, then `show` the best session. Cite `session_id` and turn content. Do not invent history if lineage returns nothing.

Session JSON may include `metadata.architecture_summary`, `prompted_by_email`, `prompted_by_name`, `vendor_session_id`, `parent_session_id`, and `git_branch`.

## Tribal blame

Combines `git blame` with lineage notes at the introducing commit. JSON `matches` include confidence and content previews. Line objects materialize when sessions link to commits (hooks or manual import + commit).

## Sharing with your team

Tribal refs travel with the repo:

```bash
tribal lfs push
git push origin refs/lineage/* refs/notes/lineage
```

Teammates on a fresh clone:

```bash
git fetch origin refs/lineage/* refs/notes/lineage
tribal lfs fetch
tribal doctor
```

Before sharing publicly: `tribal export --redact --format jsonl` and review output. Policy and excludes live in `refs/lineage/config`.

## After a rebase

Rewritten SHAs orphan lineage notes. Recovery:

```bash
tribal remap
```

Uses patch-id metadata on git notes to match rewritten commits, then re-materializes line objects. Run after interactive rebase or history rewrite; verify with `tribal list --commit <new-sha> --no-interactive --json`.

## Hooks and ongoing import

If `tribal init --hooks` is active:

- **pre-commit:** incremental import (`--no-link-head --incremental`)
- **post-commit:** link recent sessions to the new commit

Manual refresh: `tribal import --agent all --incremental` (alias: `ingest`)

## Continue sessions

`tribal fork` carries on a stored session — including a teammate's,
since it resolves from lineage's refs rather than a local transcript file:

```bash
tribal fork <session-id>            # reopens yours, writes out anyone else's
tribal fork <session-id> --new      # write out even if it could be reopened
tribal fork <session-id> --brief    # context block for a subagent
```

A session this machine already holds is reopened as itself: nothing is written,
and continuing it adds to its history. Any other is written out as a new session
that belongs to you, leaving the original untouched and recording it as an
ancestor. Tool activity is replayed as prose, so you get their context but no
replayable tool handles. Writing out is Claude Code only.

The VS Code / Cursor extension offers both from the session tree and from hover.

Cursor sessions are view-and-inject only: `cursor-agent` keeps a different session
store from the Cursor IDE transcripts lineage imports.

## Maintenance commands

| Command | Use when |
|---------|----------|
| `tribal doctor` | Verify config, refs, LFS integrity |
| `tribal rebuild index` | Search returns nothing / stale index |
| `tribal materialize` | Rebuild line objects at a commit |
| `tribal delete <id> [--purge-blobs]` | Remove a session from the repo |
| `tribal gc` | Purge orphan line objects and unreferenced LFS blobs |
| `tribal remap` | After rebase |

## MCP (optional)

If `lineage-mcp` is configured, prefer MCP tools (`lineage_list_sessions`, `lineage_search`, `lineage_get_session`, `lineage_blame_line`, `lineage_remap`, etc.) inside the agent loop.

## Privacy and side effects

- Import reads local agent transcript files; only normalized, policy-filtered data is written to git refs
- Secrets should be redacted before persistence; still avoid pasting raw exports into public issues
- `delete` and `gc` are destructive; confirm session id before running
- Import + hooks increase repo size (refs, notes, LFS for large artifacts)

## If nothing is found

- `tribal import --agent all --incremental`
- Confirm [agent paths](https://github.com/usetribal/tribal/blob/main/docs/agent-paths.md) for Cursor / Claude / Codex
- Teammates may need `git fetch origin refs/lineage/* refs/notes/lineage`
- Fall back to `git log` and code comments; lineage augments git, it does not replace it
