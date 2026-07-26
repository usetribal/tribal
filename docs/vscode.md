# VS Code extension

[← Documentation index](README.md) · [CLI reference](cli/README.md) · [Resume and fork](resume-and-fork.md)

The Lineage extension brings session search, timeline view, gutter blame, and resume/fork into VS Code and Cursor. It shells out to `git lineage` for all repository operations.

## Install

### From source (lineage monorepo)

```bash
make setup
```

### Side-load a package

```bash
cd extensions/vscode
npm install
npm run package
```

Install the generated `.vsix` via **Extensions: Install from VSIX**.

### Requirements

- `git lineage` on PATH, or `lineage.cliPath` configured
- A git repository with lineage initialized (`git lineage init`)

## Features

| Feature | Description |
|---------|-------------|
| Activity bar panel | Lists imported sessions with agent, model, branch, and prompter |
| Session timeline | Turn-by-turn webview with tool calls and artifacts |
| Gutter decorations | Icon on lines with linked lineage |
| Hover blame | Model, prompter, and quick actions on hover |
| Resume / fork | Claude Code and Codex terminal integration |
| Command palette | Import, search, doctor, materialize, remap, hooks |

## Commands

| Command | Keybinding | Description |
|---------|------------|-------------|
| Lineage: Import Sessions | | Run `git lineage import` |
| Lineage: Refresh Sessions | | Reload session tree |
| Lineage: View Conversation | | Open timeline for a session |
| Lineage: Resume Conversation | | Resume Claude/Codex in terminal |
| Lineage: Fork Conversation | | Run `git lineage fork` and show its output |
| Lineage: Show Lineage for Line | `Cmd+Shift+L` / `Ctrl+Shift+L` | Blame line and open session |
| Lineage: View Commit | | Show linked `git show` |
| Lineage: Search Sessions | | Full-text search |
| Lineage: Doctor | | Repository health check |
| Lineage: Materialize Line Objects | | Run materialize at HEAD |
| Lineage: Remap After Rebase | | Run remap |
| Lineage: Install Git Hooks | | Install lineage git hooks |
| Lineage: Delete Session | | Remove a session from the repo |
| Lineage: Init Config | | Write default config ref |

## Settings

| Setting | Default | Description |
|---------|---------|-------------|
| `lineage.cliPath` | `""` | Path to `git-lineage` binary; empty uses `git lineage` on PATH |
| `lineage.decorateGutter` | `true` | Gutter icon on lines with lineage |
| `lineage.hoverEnabled` | `true` | Show lineage hover in editor |
| `lineage.autoRefresh` | `true` | Refresh session list after import |

## Typical workflow

1. Run `git lineage init` in your project (or use **Init Config** + **Install Git Hooks** from the palette).
2. **Import Sessions** or commit with hooks enabled.
3. Open the Lineage activity bar to browse sessions.
4. Use **Show Lineage for Line** or gutter icons while editing.
5. **Resume** or **Fork** when continuing Claude/Codex work.

## Developing the extension

Contributors working on the extension itself:

```bash
cd extensions/vscode
npm install
npm run check
```

Press **F5** in the lineage monorepo with the **Lineage Extension** launch configuration. Default settings point `lineage.cliPath` at the debug `git-lineage` binary built by `make setup`.

See [Developing](developing.md) for full contributor workflow.

## Troubleshooting

| Problem | What to try |
|---------|-------------|
| Empty session list | Import sessions; check `lineage.cliPath` |
| Commands fail silently | Run `git lineage doctor` in terminal |
| Resume unavailable | Confirm agent is Claude/Codex and session has vendor id |
| Stale gutter icons | **Refresh Sessions** or re-run import |

## Related guides

- [Import](import.md)
- [Explore](explore.md)
- [Git hooks](git-hooks.md)
