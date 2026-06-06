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

Fork from the terminal copies transcript material for manual use:

```bash
# Fork workflow is primarily exposed through the extension and agent CLIs.
# Ensure the session was imported with vendor metadata:
git lineage show <session-id> --json
```

Look for `vendor_session_id` in session JSON. Without it, resume and fork are not available for that session.

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
