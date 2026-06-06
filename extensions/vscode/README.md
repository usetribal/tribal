# VS Code extension

[← Back to README](../../README.md) · [Setup](../../README.md#setup) · [CLI](../../docs/cli/README.md)

Git-native AI agent provenance inside VS Code and Cursor.

## Features

- **Activity bar panel** listing ingested sessions (agent, model, branch, prompter)
- **Session timeline webview** with turn-by-turn history, tool calls, and artifacts
- **Minimal hover blame** — model, prompter, and icon actions (view / fork / resume)
- **Gutter decorations** and status bar hint for lines with lineage
- **Resume / fork** — runs `claude --resume` or `codex resume` in a terminal; fork copies transcript to `.lineage/forks/`
- **Search**, doctor, materialize, remap, and git hook commands from the palette

## Install & build

From the lineage repo (also run by `./scripts/setup.sh`):

```bash
cargo install --path ../../crates/lineage-cli
cd extensions/vscode
npm install
npm run compile
```

Package a `.vsix` for side-loading:

```bash
npm run package
```

## Local development (F5)

The repo includes `.vscode/settings.json`, `launch.json`, and `tasks.json`:

1. Open the **lineage** repo root in VS Code or Cursor
2. Install recommended extensions when prompted
3. Press **F5** — select **Lineage Extension** (or **Lineage Extension (other project)** to open a different repo)

`lineage.cliPath` in `.vscode/settings.json` points at `target/debug/git-lineage` (built during setup).

## Commands

| Command | Keybinding | Description |
|---------|------------|-------------|
| Lineage: Ingest Sessions | | Ingest agent history into git refs |
| Lineage: Refresh Sessions | | Reload the session tree |
| Lineage: View Conversation | | Open timeline webview for a session |
| Lineage: Resume Conversation | | Resume Claude/Codex session in terminal |
| Lineage: Fork Conversation | | Copy transcript and branch in agent |
| Lineage: Show Lineage for Line | `Cmd+Shift+L` | Blame current line and open session |
| Lineage: View Commit | | Show `git show` for linked commit |
| Lineage: Search Sessions | | Full-text search |
| Lineage: Doctor | | Run `git lineage doctor` |
| Lineage: Install Git Hooks | | Run `git lineage install-hook` |

## Settings

| Setting | Default | Description |
|---------|---------|-------------|
| `lineage.decorateGutter` | `true` | Show gutter icon on lines with lineage |
| `lineage.autoRefresh` | `true` | Refresh session list after ingest |
| `lineage.cliPath` | `""` | Path to `git-lineage` binary (empty = use `git lineage`) |
| `lineage.hoverEnabled` | `true` | Show lineage hover on editor lines |

## Hover actions

When hovering a line with lineage data:

| Icon | Action |
|------|--------|
| `$(open-preview)` | View conversation timeline |
| `$(git-branch)` | Fork — copies transcript to `.lineage/forks/`, then branches in agent |
| `$(run)` | Resume — opens Claude Code or Codex session in integrated terminal |

Resume and fork are available for **Claude Code** and **Codex** when a vendor session id is stored. Cursor sessions support view only (no resume CLI).
