---
name: lineage
description: >-
  Retrieves and manages engineering context from git-lineage: AI agent
  conversations, line-level provenance, team sharing, rebase recovery, and
  session resume/fork. Use when answering why code exists, finding past
  decisions, onboarding, before risky edits, after rebase, or when sharing
  lineage with teammates.
---

# Tribal agent skill

Tribal stores **AI agent session provenance** in git (`refs/lineage/*`, `refs/notes/lineage`). Setup is handled by `git lineage init` (humans run that once). **Your job is to query, interpret, and advise on lineage data.**

## When to use lineage

- Why was this code written? What prompt or decision led to it?
- What did agents discuss about this module, API, or bug?
- Before editing unfamiliar code: linked sessions, blame, architecture summary
- After `git rebase` / `git rebase -i`: orphaned lineage notes
- Sharing context with teammates or reviewing what would leave the repo
- Continuing a prior agent session (Claude Code, Codex; VS Code extension)

## Retrieve context (prefer `--json`)

```bash
git lineage context query "authentication middleware"
git lineage list --json
git lineage list --commit <sha> --json
git lineage show <session-id> --json
git lineage blame path/to/file.rs:42 --json
git lineage export --redact --format jsonl
```

**Workflow:** query by topic, then blame for a specific line, then `show` the best session. Cite `session_id` and turn content. Do not invent history if lineage returns nothing.

Session JSON may include `metadata.architecture_summary`, `prompted_by_email`, `prompted_by_name`, `vendor_session_id`, `parent_session_id`, and `git_branch`.

## Tribal blame

Combines `git blame` with lineage notes at the introducing commit. JSON `matches` include confidence and content previews. Line objects materialize when sessions link to commits (hooks or manual import + commit).

## Sharing with your team

Tribal refs travel with the repo:

```bash
git lineage lfs push
git push origin refs/lineage/* refs/notes/lineage
```

Teammates on a fresh clone:

```bash
git fetch origin refs/lineage/* refs/notes/lineage
git lineage lfs fetch
git lineage doctor
```

Before sharing publicly: `git lineage export --redact --format jsonl` and review output. Policy and excludes live in `refs/lineage/config`.

## After a rebase

Rewritten SHAs orphan lineage notes. Recovery:

```bash
git lineage remap
```

Uses patch-id metadata on git notes to match rewritten commits, then re-materializes line objects. Run after interactive rebase or history rewrite; verify with `git lineage list --commit <new-sha>`.

## Hooks and ongoing import

If `git lineage init --hooks` is active:

- **pre-commit:** incremental import (`--no-link-head --incremental`)
- **post-commit:** link recent sessions to the new commit

Manual refresh: `git lineage import --agent all --incremental` (alias: `ingest`)

## Continue sessions

`git lineage continue` carries on a stored session — including a teammate's,
since it resolves from lineage's refs rather than a local transcript file:

```bash
git lineage continue <session-id>            # reopens yours, writes out anyone else's
git lineage continue <session-id> --fork     # write out even if it could be reopened
git lineage continue <session-id> --brief    # context block for a subagent
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
| `git lineage doctor` | Verify config, refs, LFS integrity |
| `git lineage rebuild index` | Search returns nothing / stale index |
| `git lineage materialize` | Rebuild line objects at a commit |
| `git lineage delete <id> [--purge-blobs]` | Remove a session from the repo |
| `git lineage gc` | Purge orphan line objects and unreferenced LFS blobs |
| `git lineage remap` | After rebase |

## MCP (optional)

If `lineage-mcp` is configured, prefer MCP tools (`lineage_list_sessions`, `lineage_search`, `lineage_get_session`, `lineage_blame_line`, `lineage_remap`, etc.) inside the agent loop.

## Privacy and side effects

- Import reads local agent transcript files; only normalized, policy-filtered data is written to git refs
- Secrets should be redacted before persistence; still avoid pasting raw exports into public issues
- `delete` and `gc` are destructive; confirm session id before running
- Import + hooks increase repo size (refs, notes, LFS for large artifacts)

## If nothing is found

- `git lineage import --agent all --incremental`
- Confirm [agent paths](https://github.com/usetribal/tribal/blob/main/docs/agent-paths.md) for Cursor / Claude / Codex
- Teammates may need `git fetch origin refs/lineage/* refs/notes/lineage`
- Fall back to `git log` and code comments; lineage augments git, it does not replace it
