# VS Code extension

[← Back to README](../../README.md) · [Full documentation](../../docs/vscode.md)

Git-native AI agent provenance in VS Code and Cursor.

User guide, commands, settings, and troubleshooting: **[docs/vscode.md](../../docs/vscode.md)**.

## Build from source

```bash
make setup
# or:
cd extensions/vscode && npm install && npm run compile
```

Package: `npm run package` → install `.vsix` via **Extensions: Install from VSIX**.

## Develop

```bash
npm run check
```

Open the lineage repo root, press **F5**, select **Lineage Extension**. See [Developing](../../docs/developing.md).
