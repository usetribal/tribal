---
name: vscode-extension-dev
description: >-
  Develops the Tribal VS Code extension in extensions/vscode. Covers
  TypeScript sources, lineageClient CLI integration, npm run check, and F5
  extension host launch against the tribal repository. Use when changing the VS
  Code extension, session panel, gutter blame, or extension commands.
---

# VS Code extension dev

## Layout

| File | Role |
|------|------|
| `src/extension.ts` | Activation, command registration |
| `src/lineageClient.ts` | Shells out to `git lineage` / `git-lineage` |
| `src/sessionsProvider.ts` | Activity bar tree |
| `src/sessionPanel.ts` | Session timeline webview |
| `src/lineageDecorator.ts` | Gutter decorations |
| `src/lineageHoverProvider.ts` | Hover blame |
| `src/lineageMarkdown.ts` | Session markdown rendering |
| `src/agentActions.ts` | Resume/fork CLI integration |
| `src/types.ts` | JSON shapes from CLI |

## Commands

```bash
cd extensions/vscode
npm install
npm run check    # compile + eslint + prettier
npm run package  # .vsix
```

From repo root: `make vscode` or `make vscode-lint`.

## Local dev (F5)

1. Run `./scripts/setup.sh` (builds `target/debug/git-lineage`)
2. Open **lineage repo root** in VS Code/Cursor
3. Press **F5** — uses `.vscode/launch.json`:
   - `--extensionDevelopmentPath=extensions/vscode`
   - Opens this repository as the workspace
4. `lineage.cliPath` in `.vscode/settings.json` → `target/debug/git-lineage`

## Adding a command

1. Register in `package.json` → `contributes.commands`
2. Implement handler in `extension.ts`
3. Add CLI call in `lineageClient.ts` if needed
4. Run `npm run check`

## Settings (extension contributes)

- `lineage.cliPath` — path to binary
- `lineage.decorateGutter`, `lineage.hoverEnabled`, `lineage.autoRefresh`
