# Lineage VS Code Extension

Git-native AI agent provenance inside VS Code.

## Features

- **Sessions panel** in the activity bar lists all ingested lineage sessions
- **Timeline webview** shows turn-by-turn conversation history with role styling
- **Gutter decorations** mark lines that have lineage data
- **Status bar** shows the linked session for the current line
- **Search** across ingested session text
- **Install git hooks** from the command palette

## Setup

```bash
# Install the CLI first
cargo install --path ../../crates/lineage-cli

# Build the extension
npm install
npm run compile
```

Press **F5** in VS Code to launch the extension development host.

## Commands

| Command | Keybinding | Description |
|---------|------------|-------------|
| Lineage: Ingest Sessions | | Ingest agent history into git refs |
| Lineage: Refresh Sessions | | Reload the session tree |
| Lineage: Open Session | | Open timeline webview for a session |
| Lineage: Show Lineage for Line | `Cmd+Shift+L` | Blame current line and open session |
| Lineage: Search Sessions | | Full-text search |
| Lineage: Install Git Hooks | | Run `git lineage install-hook` |

## Settings

| Setting | Default | Description |
|---------|---------|-------------|
| `lineage.decorateGutter` | `true` | Show gutter icon on lines with lineage |
| `lineage.autoRefresh` | `true` | Refresh session list after ingest |
| `lineage.cliPath` | `""` | Path to `git-lineage` binary (empty = use `git lineage`) |
