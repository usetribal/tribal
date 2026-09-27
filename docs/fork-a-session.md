# Fork a session

[← Documentation index](README.md) · [VS Code extension](vscode.md) · [Explore](explore.md)

`tribal fork` carries on an agent session. There are two ways that can
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
| Codex | Yes | Yes | Reopening needs `vendor_session_id` from import; writing out needs neither |
| Cursor | View only | No | Tribal reads Cursor's IDE sessions; `cursor-agent` has its own separate store |

## Usage

```bash
tribal fork <session-id>              # tribal id, prefix, or Claude UUID
tribal fork --query "RLS audit"       # search, then pick (--pick N if several)
tribal fork                           # interactive picker on a TTY
tribal fork <session-id> --new        # write out even if it could be reopened
tribal fork <session-id> --json       # structured preflight for agents
```

`tribal list` shows **titles first** (from Claude's session summary when
imported, otherwise the opening ask), then id, date, agent, model, and author —
so you can pick a session before continuing it. Session ids are interoperable:
pass a Claude vendor UUID or a unique tribal id prefix wherever a full id works.

Sessions are resolved from tribal's own refs, so one pulled from a teammate
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
- **Only what tribal stored.** Redaction runs before persist, so it is at most
  as complete as the redacted stored copy.

Claude Code and Codex. Cursor sessions decline by name rather than failing
obscurely.

### How a Codex session is written

Codex resolves `codex resume <id>` by scanning `~/.codex/sessions/` for a rollout
whose first record names the id, so tribal writes one there, filed by date like
Codex's own. Its `cwd` is your workspace, and its `originator` and `cli_version`
name tribal as the writer. Checked against codex-cli 0.153.4, where Codex resumes
the written session and answers from its history.

### Why not Cursor

`cursor-agent --resume` exists, so the old claim that Cursor has no resume CLI is
out of date. It does not make Cursor sessions writable by tribal:
`cursor-agent` keeps its own session store, and tribal's adapter reads Cursor's
**IDE** transcripts ([agent paths](agent-paths.md)). Those are different sessions
in different places, so writing an IDE-shaped transcript would not produce
something `cursor-agent` can open. Cursor sessions stay view-and-inject only.

## VS Code and Cursor extension

Install the [VS Code extension](vscode.md) in VS Code or Cursor.

### From the session tree

- **View** — opens the session timeline webview.
- **Resume** and **Fork** both run `tribal fork` and open its output, so
  you can read whose session it was and what it was about before running the
  command it prints. The command is shown, never run for you: it opens a live
  agent, and which shell that lands in stays your decision.

### From editor hover

When hover blame finds tribal for a line, icon actions offer view, fork
(Claude), and resume (Claude/Codex).

Commands are also available from the command palette (`Tribal: Resume
Conversation`, `Tribal: Fork Conversation`).

## Prerequisites

1. Session imported with `tribal import` (or hooks), or fetched from a
   teammate's tribal refs.
2. Agent CLI installed (`claude`, `codex`) and on PATH.
3. For extension actions: `tribal.cliPath` set if `tribal` is not on PATH
   (see [VS Code](vscode.md)).

## Related guides

- [Import](import.md) — populate vendor session ids
- [Agent paths](agent-paths.md)
- [Explore](explore.md)
