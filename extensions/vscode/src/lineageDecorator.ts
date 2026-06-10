import * as vscode from "vscode";
import { LineageClient } from "./lineageClient";

export class LineageDecorator implements vscode.Disposable {
    private readonly decorationType: vscode.TextEditorDecorationType;
    private readonly disposables: vscode.Disposable[] = [];
    private debounce: NodeJS.Timeout | undefined;
    private lastKey = "";

    constructor(private readonly client: LineageClient) {
        this.decorationType = vscode.window.createTextEditorDecorationType({
            gutterIconPath: this.gutterIcon(),
            gutterIconSize: "contain",
        });

        this.disposables.push(
            vscode.window.onDidChangeActiveTextEditor((e) => this.scheduleUpdate(e)),
            vscode.window.onDidChangeTextEditorSelection((e) => this.scheduleUpdate(e.textEditor)),
            this.decorationType
        );

        if (vscode.window.activeTextEditor) {
            this.scheduleUpdate(vscode.window.activeTextEditor);
        }
    }

    dispose(): void {
        if (this.debounce) {
            clearTimeout(this.debounce);
        }
        for (const d of this.disposables) {
            d.dispose();
        }
    }

    private scheduleUpdate(editor: vscode.TextEditor | undefined): void {
        const config = vscode.workspace.getConfiguration("lineage");
        if (!config.get<boolean>("decorateGutter", true)) {
            return;
        }
        if (!editor || editor.document.uri.scheme !== "file") {
            return;
        }

        if (this.debounce) {
            clearTimeout(this.debounce);
        }
        this.debounce = setTimeout(() => void this.update(editor), 250);
    }

    private async update(editor: vscode.TextEditor): Promise<void> {
        const rel = vscode.workspace.asRelativePath(editor.document.uri);
        const line = editor.selection.active.line + 1;
        const key = `${rel}:${line}`;
        if (key === this.lastKey) {
            return;
        }
        this.lastKey = key;

        try {
            const result = await this.client.blame(rel, line);
            const hasLineage = result.sessions.length > 0 || result.line_objects.length > 0;

            if (!hasLineage) {
                editor.setDecorations(this.decorationType, []);
                vscode.window.setStatusBarMessage("");
                return;
            }

            const range = new vscode.Range(line - 1, 0, line - 1, 0);
            editor.setDecorations(this.decorationType, [{ range }]);

            const session = result.sessions[0] ?? "unknown";
            const match = result.matches?.[0];
            const detail = match
                ? ` · ${match.confidence} · ${match.content_preview.slice(0, 40)}`
                : "";
            vscode.window.setStatusBarMessage(
                `Lineage: line ${line} → session ${session.slice(0, 12)}…${detail}`
            );
        } catch {
            editor.setDecorations(this.decorationType, []);
        }
    }

    private gutterIcon(): vscode.Uri {
        const svg = Buffer.from(
            `<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16">
        <circle cx="8" cy="8" r="5" fill="%23c5c5c5"/>
        <circle cx="8" cy="8" r="2.5" fill="%230078d4"/>
      </svg>`,
            "utf8"
        );
        return vscode.Uri.parse(`data:image/svg+xml;base64,${svg.toString("base64")}`);
    }
}
