# Fork a session

[← Documentation index](README.md) · [VS Code extension](vscode.md) · [Explore](explore.md)

`git lineage fork` carries on an agent session. There are two ways that can
happen, and which one applies is a property of the session rather than a choice
you make:

- A session **your harness still holds** is reopened by handing its vendor id
  back to the agent that created it. Nothing is written, and it stays the same
  session.
- **Any other** — a teammate's, or one pulled from a server — is written out as
  a *new* session in your own agent, carrying their context. The original is
  untouched and the new session belongs to you.

Writing one out is what works on a session you did not record yourself. It is
tried only when reopening is not possible, so the common case stays the cheaper
one. `--new` forces it for a session that could have been reopened.

## What is supported today

| Agent | Reopen | Write out | Notes |
|-------|--------|-----------|-------|
| Claude Code | Yes | Yes | Writing out needs no local transcript and no `vendor_session_id` |
| Codex | Yes | Not yet | Reopening needs `vendor_session_id` from import |
| Cursor | View only | No | Tribal reads Cursor's IDE sessions; `cursor-agent` has its own separate store |

## Usage

```bash
git lineage fork <session-id>              # lineage id, prefix, or Claude UUID
git lineage fork --query "RLS audit"       # search, then pick (--pick N if several)
git lineage fork                           # interactive picker on a TTY
git lineage fork <session-id> --new        # write out even if it could be reopened
git lineage fork <session-id> --json       # structured preflight for agents
```

`git lineage list` shows **titles first** (from Claude's session summary when
imported, otherwise the opening ask), then id, date, agent, model, and author —
so you can pick a session before continuing it. Session ids are interoperable:
pass a Claude vendor UUID or a unique lineage id prefix wherever a full id works.

Sessions are resolved from lineage's own refs, so one pulled from a teammate
works exactly as one you imported yourself. Full output and caveats:
[CLI reference](cli/README.md#fork-a-session).

## Reopening

The command that reopens the session is printed, along with the directory to run
it from when the harness resolves a session relative to one. Nothing is written
and no new session is recorded — this is the original session continued.

The command is printed rather than run: it opens an interactive agent, which is
a thing to choose rather than have happen.

## Writing one out

- **Their context, not their tools.** Tool activity is replayed as prose. You do
  not get replayable tool handles, and `/rewind` will not reach back into the
  original session.
- **A new identity.** A fresh vendor session id is minted; the source session's
  file is never read or modified.
- **An ancestor edge, not co-authorship.** The new session records `fork_origin`
  ([conversation schema](../specs/conversation-schema-v0.md)). Lines you write
  afterwards are attributed to you.
- **Only what lineage stored.** Redaction runs before persist, so it is at most
  as complete as the redacted stored copy.

Claude Code only for now. Codex and Cursor sessions decline by name rather than
failing obscurely.

### Why not Codex yet

`codex fork <id>` exists, and Codex's id resolution appears to scan rollout files
rather than an index — so this very likely works. It has not been executed
against a lineage-written transcript, and Codex's unified backend may validate
ids server-side. One test decides it; until that test is run, lineage refuses
rather than writing a file that may not open.

### Why not Cursor

`cursor-agent --resume` exists, so the old claim that Cursor has no resume CLI is
out of date. It does not make Cursor sessions writable by lineage:
`cursor-agent` keeps its own session store, and lineage's adapter reads Cursor's
**IDE** transcripts ([agent paths](agent-paths.md)). Those are different sessions
in different places, so writing an IDE-shaped transcript would not produce
something `cursor-agent` can open. Cursor sessions stay view-and-inject only.

## VS Code and Cursor extension

Install the [VS Code extension](vscode.md) in VS Code or Cursor.

### From the session tree

- **View** — opens the session timeline webview.
- **Resume** and **Fork** both run `git lineage fork` and open its output, so
  you can read whose session it was and what it was about before running the
  command it prints. The command is shown, never run for you: it opens a live
  agent, and which shell that lands in stays your decision.

### From editor hover

When hover blame finds lineage for a line, icon actions offer view, fork
(Claude), and resume (Claude/Codex).

Commands are also available from the command palette (`Lineage: Resume
Conversation`, `Lineage: Fork Conversation`).

## Prerequisites

1. Session imported with `git lineage import` (or hooks), or fetched from a
   teammate's lineage refs.
2. Agent CLI installed (`claude`, `codex`) and on PATH.
3. For extension actions: `lineage.cliPath` set if `git lineage` is not on PATH
   (see [VS Code](vscode.md)).

## Related guides

- [Import](import.md) — populate vendor session ids
- [Agent paths](agent-paths.md)
- [Explore](explore.md)
