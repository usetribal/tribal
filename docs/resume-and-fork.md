# Resume and fork

[← Documentation index](README.md) · [VS Code extension](vscode.md) · [Explore](explore.md)

Lineage links stored sessions to vendor agent identifiers so you can continue or branch conversations without hunting transcript files on disk.

## What is supported today

| Agent | Resume | Fork | Notes |
|-------|--------|------|-------|
| Claude Code | Yes | Yes | Requires `vendor_session_id` from import |
| Codex | Yes | Yes | Requires `vendor_session_id` from import |
| Cursor | View only | View only | No stable resume CLI from Lineage yet |

Resume opens the agent with the vendor session id. Fork copies transcript material into `.lineage/forks/` and starts a branched session in the agent.

## VS Code and Cursor extension

Install the [VS Code extension](vscode.md) in VS Code or Cursor.

### From the session tree

- **Resume** — runs `claude --resume` or `codex resume` in the integrated terminal.
- **Fork** — copies transcript data to `.lineage/forks/`, then starts a forked agent session.
- **View** — opens the session timeline webview.

### From editor hover

When hover blame finds lineage for a line, icon actions offer view, fork, and resume (Claude/Codex only).

Commands are also available from the command palette (`Lineage: Resume Conversation`, `Lineage: Fork Conversation`).

## CLI

`git lineage fork <session-id>` renders the stored session into a vendor-native
transcript, records the fork edge, and prints the command that opens it:

```bash
git lineage fork <session-id>
git lineage fork <session-id> --dry-run   # show what would be written
```

It resolves the session from lineage's own refs, so it works on a session you
pulled from a teammate as well as one you imported yourself — no
`vendor_session_id` and no local transcript file required. Full output and
caveats: [CLI reference](cli/README.md#fork-a-session).

The extension's fork action is still the older path described below and does
require `vendor_session_id`.

## Prerequisites

1. Session imported with `git lineage import` (or hooks).
2. Agent CLI installed (`claude`, `codex`) and on PATH for resume.
3. For extension actions: `lineage.cliPath` set if `git lineage` is not on PATH (see [VS Code](vscode.md)).

## Fork directory

Fork copies land under `.lineage/forks/` in the repository working tree. Add this path to `.gitignore` if you do not want fork working files committed. Lineage session refs remain separate from fork scratch files.

## Cursor limitation

Cursor sessions are imported and fully searchable, but Lineage does not invoke a Cursor resume command. Use the session panel to read history until a stable Cursor resume API exists.

## Related guides

- [Import](import.md) — populate vendor session ids
- [Agent paths](agent-paths.md)
- [Explore](explore.md)
