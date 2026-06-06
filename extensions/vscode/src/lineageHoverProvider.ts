import * as vscode from "vscode";
import { LineageClient } from "./lineageClient";

export class LineageHoverProvider implements vscode.HoverProvider {
    constructor(private readonly client: LineageClient) {}

    async provideHover(
        document: vscode.TextDocument,
        position: vscode.Position
    ): Promise<vscode.Hover | undefined> {
        const config = vscode.workspace.getConfiguration("lineage");
        if (!config.get<boolean>("hoverEnabled", true)) {
            return undefined;
        }

        const rel = vscode.workspace.asRelativePath(document.uri);
        const line = position.line + 1;

        try {
            const result = await this.client.blame(rel, line);
            if (result.sessions.length === 0 && (result.matches?.length ?? 0) === 0) {
                return undefined;
            }

            const sessionId = result.sessions[0] ?? "unknown";
            const match = result.matches?.[0];
            const parts = [
                `**Lineage** · line ${line}`,
                `Session: \`${sessionId.slice(0, 20)}…\``,
                `Commit: \`${result.commit_sha.slice(0, 12)}\``,
            ];
            if (match) {
                parts.push(`Confidence: ${match.confidence}`);
                if (match.content_preview) {
                    parts.push(`\n${match.content_preview.slice(0, 300)}`);
                }
            }
            parts.push("\n*Click: Lineage: Show Lineage for Line*");

            return new vscode.Hover(
                new vscode.MarkdownString(parts.join("\n\n")),
                new vscode.Range(position.line, 0, position.line, Number.MAX_SAFE_INTEGER)
            );
        } catch {
            return undefined;
        }
    }
}
