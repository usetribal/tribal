import * as vscode from "vscode";
import { LineageClient } from "./lineageClient";
import type { SessionSummary } from "./types";

export class SessionTreeItem extends vscode.TreeItem {
    constructor(public readonly session: SessionSummary) {
        super(`${session.agent} · ${session.turns} turns`, vscode.TreeItemCollapsibleState.None);
        this.description = session.model
            ? `${session.id.slice(0, 8)} · ${session.model}`
            : session.id.slice(0, 12);
        this.tooltip = [
            session.id,
            session.started_at,
            session.model ? `model: ${session.model}` : "",
            session.models_used?.length ? `models: ${session.models_used.join(", ")}` : "",
        ]
            .filter(Boolean)
            .join("\n");
        this.contextValue = "lineageSession";
        this.command = {
            command: "lineage.openSession",
            title: "Open Session",
            arguments: [session.id],
        };
        this.iconPath = agentIcon(session.agent);
    }
}

function agentIcon(agent: string): vscode.ThemeIcon {
    switch (agent) {
        case "cursor":
            return new vscode.ThemeIcon("terminal");
        case "claude":
            return new vscode.ThemeIcon("comment");
        case "codex":
            return new vscode.ThemeIcon("rocket");
        default:
            return new vscode.ThemeIcon("question");
    }
}

export class SessionsProvider implements vscode.TreeDataProvider<SessionTreeItem> {
    private readonly _onDidChange = new vscode.EventEmitter<void>();
    readonly onDidChangeTreeData = this._onDidChange.event;

    private sessions: SessionSummary[] = [];
    private error: string | undefined;

    constructor(private readonly client: LineageClient) {}

    refresh(): void {
        this._onDidChange.fire();
    }

    async load(): Promise<void> {
        try {
            this.sessions = await this.client.listSessions();
            this.error = undefined;
        } catch (e) {
            this.sessions = [];
            this.error = String(e);
        }
        this.refresh();
    }

    getTreeItem(element: SessionTreeItem): vscode.TreeItem {
        return element;
    }

    async getChildren(): Promise<SessionTreeItem[]> {
        if (this.error) {
            return [
                new SessionTreeItem({
                    id: "error",
                    agent: "error",
                    turns: 0,
                    started_at: this.error,
                }),
            ];
        }
        if (this.sessions.length === 0) {
            return [
                new SessionTreeItem({
                    id: "empty",
                    agent: "none",
                    turns: 0,
                    started_at: "Run Lineage: Ingest Sessions",
                }),
            ];
        }
        return this.sessions.map((s) => new SessionTreeItem(s));
    }
}
