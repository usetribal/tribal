# Resume and fork

[← Documentation index](README.md) · [VS Code extension](vscode.md) · [Explore](explore.md)

Two different things, often confused:

- **Resume** reopens a session that is already on this machine, by handing its
  vendor id back to the agent that created it. Nothing is written.
- **Fork** takes a session lineage has stored — yours or a teammate's — and
  writes it out as a *new* session in your own agent, carrying their context.
  The original is untouched and the fork belongs to you.

Fork is the one that works on a session you did not record yourself.

## What is supported today

| Agent | Resume | Fork | Notes |
|-------|--------|------|-------|
| Claude Code | Yes | Yes | Fork needs no local transcript and no `vendor_session_id` |
| Codex | Yes | Not yet | Resume needs `vendor_session_id` from import |
| Cursor | View only | No | Lineage reads Cursor's IDE sessions; `cursor-agent` has its own separate store |

## Fork

`git lineage fork <session-id>` renders the stored session into a transcript
your agent can open, records the fork edge, and prints the command that opens it:

```bash
git lineage fork <session-id>
git lineage fork <session-id> --dry-run   # show what would be written
```

`git lineage list` shows what is here — id, date, agent, model, and who ran it —
so you can pick a session before you fork it. `git lineage fork --help` explains
what a fork is and what it does and does not carry.

The session is resolved from lineage's own refs, so it works on a session you
pulled from a teammate exactly as it does on one you imported yourself: no
`vendor_session_id`, and no local transcript file. Full output and caveats:
[CLI reference](cli/README.md#fork-a-session).

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
out of date. It does not make Cursor forkable by lineage: `cursor-agent` keeps
its own session store, and lineage's adapter reads Cursor's **IDE** transcripts
([agent paths](agent-paths.md)). Those are different sessions in different
places, so writing an IDE-shaped transcript would not produce something
`cursor-agent` can open. Cursor sessions stay view-and-inject only.

### What a fork carries

- **Their context, not their tools.** Tool activity is replayed as prose. You do
  not get replayable tool handles, and `/rewind` will not reach back into the
  original session.
- **A new identity.** A fresh vendor session id is minted; the source session's
  file is never read or modified.
- **An ancestor edge, not co-authorship.** The fork records `fork_origin`
  ([conversation schema](../specs/conversation-schema-v0.md)). Lines you write
  after forking are attributed to you.
- **Only what lineage stored.** Redaction runs before persist, so a fork is at
  most as complete as the redacted stored copy.

## Resume

Resume hands a `vendor_session_id` back to the agent that produced it, so it only
works for a session recorded on this machine. A teammate's session has no vendor
id here — fork it instead.

```bash
git lineage resume <session-id>
```

It prints the command that reopens the session, and the directory to run it from
when the harness resolves a session relative to one. Nothing is written and no
new session is recorded — this is the original session continued.

## VS Code and Cursor extension

Install the [VS Code extension](vscode.md) in VS Code or Cursor.

### From the session tree

- **View** — opens the session timeline webview.
- **Resume** — runs `git lineage resume` and opens its output. The command is
  shown, never run for you, so which shell the session lands in stays your
  decision and the directory it must run from is on screen.
- **Fork** — runs `git lineage fork` and opens its output, so you can read whose
  session it was and what it was about before you run the command it prints. The
  command is shown, never run for you: it opens a colleague's work in a live
  agent.

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
