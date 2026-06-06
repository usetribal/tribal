import * as vscode from "vscode";
import { canForkSession, canResumeSession } from "./agentActions";
import { LineageClient } from "./lineageClient";
import { commandLink, primaryModel, promptedByLabel, trustedMarkdown } from "./lineageMarkdown";

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

            const sessionId = result.sessions[0];
            if (!sessionId) {
                return undefined;
            }

            const conv = await this.client.getSession(sessionId);
            const md = trustedMarkdown();

            const model = (conv ? primaryModel(conv) : undefined) ?? "unknown model";
            const author = (conv ? promptedByLabel(conv) : undefined) ?? "unknown author";
            md.appendMarkdown(`\`${model}\` · ${author}\n\n`);

            const actions: string[] = [
                commandLink("$(open-preview)", "lineage.openSession", [sessionId]),
            ];
            if (conv && canForkSession(conv)) {
                actions.push(commandLink("$(git-branch)", "lineage.forkSession", [sessionId]));
            }
            if (conv && canResumeSession(conv)) {
                actions.push(commandLink("$(run)", "lineage.resumeSession", [sessionId]));
            }
            md.appendMarkdown(actions.join(" "));

            return new vscode.Hover(
                md,
                new vscode.Range(position.line, 0, position.line, Number.MAX_SAFE_INTEGER)
            );
        } catch {
            return undefined;
        }
    }
}
