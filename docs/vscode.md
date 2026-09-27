# VS Code extension

[← Documentation index](README.md) · [CLI reference](cli/README.md) · [Fork a session](fork-a-session.md)

The Tribal extension brings session search, timeline view, gutter blame, and resume/fork into VS Code and Cursor. It shells out to `tribal` for all repository operations.

## Install

### From source (tribal monorepo)

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

- `tribal` on PATH, or `tribal.cliPath` configured
- A git repository with tribal initialized (`tribal init`)

## Features

| Feature | Description |
|---------|-------------|
| Activity bar panel | Lists imported sessions with agent, model, branch, and prompter |
| Session timeline | Turn-by-turn webview with tool calls and artifacts |
| Gutter decorations | Icon on lines with linked tribal |
| Hover blame | Model, prompter, and quick actions on hover |
| Resume / fork | Claude Code and Codex terminal integration |
| Command palette | Import, search, doctor, materialize, remap, hooks |

## Commands

| Command | Keybinding | Description |
|---------|------------|-------------|
| Tribal: Import Sessions | | Run `tribal import` |
| Tribal: Refresh Sessions | | Reload session tree |
| Tribal: View Conversation | | Open timeline for a session |
| Tribal: Resume Conversation | | Resume Claude/Codex in terminal |
| Tribal: Fork Conversation | | Run `tribal fork` and show its output |
| Tribal: Show Tribal for Line | `Cmd+Shift+L` / `Ctrl+Shift+L` | Blame line and open session |
| Tribal: View Commit | | Show linked `git show` |
| Tribal: Search Sessions | | Full-text search |
| Tribal: Doctor | | Repository health check |
| Tribal: Materialize Line Objects | | Run materialize at HEAD |
| Tribal: Remap After Rebase | | Run remap |
| Tribal: Install Git Hooks | | Install tribal git hooks |
| Tribal: Delete Session | | Remove a session from the repo |
| Tribal: Init Config | | Write default config ref |

## Settings

| Setting | Default | Description |
|---------|---------|-------------|
| `tribal.cliPath` | `""` | Path to `tribal` binary; empty uses `tribal` on PATH |
| `tribal.decorateGutter` | `true` | Gutter icon on lines with tribal |
| `tribal.hoverEnabled` | `true` | Show tribal hover in editor |
| `tribal.autoRefresh` | `true` | Refresh session list after import |

## Typical workflow

1. Run `tribal init` in your project (or use **Init Config** + **Install Git Hooks** from the palette).
2. **Import Sessions** or commit with hooks enabled.
3. Open the Tribal activity bar to browse sessions.
4. Use **Show Tribal for Line** or gutter icons while editing.
5. **Resume** or **Fork** when continuing Claude/Codex work.

## Developing the extension

Contributors working on the extension itself:

```bash
cd extensions/vscode
npm install
npm run check
```

Press **F5** in the tribal monorepo with the **Tribal Extension** launch configuration. Default settings point `tribal.cliPath` at the debug `tribal` binary built by `make setup`.

See [Developing](developing.md) for full contributor workflow.

## Troubleshooting

| Problem | What to try |
|---------|-------------|
| Empty session list | Import sessions; check `tribal.cliPath` |
| Commands fail silently | Run `tribal doctor` in terminal |
| Resume unavailable | Confirm agent is Claude/Codex and session has vendor id |
| Stale gutter icons | **Refresh Sessions** or re-run import |

## Related guides

- [Import](import.md)
- [Explore](explore.md)
- [Git hooks](git-hooks.md)
