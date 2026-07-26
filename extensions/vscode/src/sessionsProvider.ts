import * as vscode from "vscode";
import { LineageClient } from "./lineageClient";
import { agentLabel } from "./lineageMarkdown";
import type { SessionSummary } from "./types";

export class SessionTreeItem extends vscode.TreeItem {
    constructor(
        public readonly session: SessionSummary,
        options?: { command?: boolean }
    ) {
        const isPlaceholder =
            session.id === "hint" || session.id === "error" || session.id === "empty";
        super(
            isPlaceholder
                ? session.started_at
                : `${agentLabel(session.agent)} · ${session.turns} turns`,
            vscode.TreeItemCollapsibleState.None
        );
        this.description = isPlaceholder ? undefined : sessionTreeDescription(session);
        this.tooltip = [
            session.id,
            session.started_at,
            session.model ? `model: ${session.model}` : "",
            session.git_branch ? `branch: ${session.git_branch}` : "",
            session.parent_session_id ? `parent: ${session.parent_session_id}` : "",
            session.vendor_session_id
                ? `${agentLabel(session.agent)} id: ${session.vendor_session_id}`
                : "",
            session.prompted_by_email ? `prompted by: ${session.prompted_by_email}` : "",
            session.prompted_by_name ? session.prompted_by_name : "",
            session.is_fork ? "forked from another session" : "",
            session.is_sidechain ? "branched conversation" : "",
            session.models_used?.length ? `models: ${session.models_used.join(", ")}` : "",
        ]
            .filter(Boolean)
            .join("\n");
        this.contextValue = isPlaceholder ? undefined : "lineageSession";
        if (options?.command !== false && session.id !== "hint") {
            this.command = {
                command: "lineage.openSession",
                title: "Open Session",
                arguments: [session.id],
            };
        }
        this.iconPath = agentIcon(session.agent);
    }
}

function sessionTreeDescription(session: SessionSummary): string {
    const parts = [session.id.slice(0, 8)];
    if (session.model) {
        parts.push(session.model);
    }
    if (session.git_branch) {
        parts.push(session.git_branch);
    }
    if (session.is_fork) {
        parts.push("forked");
    } else if (session.is_sidechain || session.parent_session_id) {
        parts.push("branched");
    }
    if (session.prompted_by_email) {
        parts.push(session.prompted_by_email);
    }
    return parts.join(" · ");
}

function agentIcon(agent: string): vscode.ThemeIcon {
    switch (agent) {
        case "cursor":
            return new vscode.ThemeIcon("terminal");
        case "claude":
            return new vscode.ThemeIcon("comment");
        case "codex":
            return new vscode.ThemeIcon("rocket");
        case "hint":
            return new vscode.ThemeIcon("folder-opened");
        case "error":
            return new vscode.ThemeIcon("error");
        default:
            return new vscode.ThemeIcon("question");
    }
}

export class SessionsProvider implements vscode.TreeDataProvider<SessionTreeItem> {
    private readonly _onDidChange = new vscode.EventEmitter<void>();
    readonly onDidChangeTreeData = this._onDidChange.event;

    private client: LineageClient | undefined;
    private sessions: SessionSummary[] = [];
    private error: string | undefined;
    private hint: string | undefined;

    setClient(client: LineageClient | undefined): void {
        this.client = client;
    }

    refresh(): void {
        this._onDidChange.fire();
    }

    async load(): Promise<void> {
        if (!this.client) {
            this.sessions = [];
            this.error = undefined;
            this.hint = "Open a folder (File → Open Folder…) to use Lineage";
            this.refresh();
            return;
        }

        try {
            this.sessions = await this.client.listSessions();
            this.error = undefined;
            this.hint = undefined;
        } catch (e) {
            this.sessions = [];
            this.error = String(e);
            this.hint = undefined;
        }
        this.refresh();
    }

    getTreeItem(element: SessionTreeItem): vscode.TreeItem {
        return element;
    }

    async getChildren(): Promise<SessionTreeItem[]> {
        if (this.hint) {
            return [
                new SessionTreeItem(
                    {
                        id: "hint",
                        agent: "hint",
                        turns: 0,
                        started_at: this.hint,
                    },
                    { command: false }
                ),
            ];
        }
        if (this.error) {
            return [
                new SessionTreeItem(
                    {
                        id: "error",
                        agent: "error",
                        turns: 0,
                        started_at: this.error,
                    },
                    { command: false }
                ),
            ];
        }
        if (this.sessions.length === 0) {
            return [
                new SessionTreeItem(
                    {
                        id: "empty",
                        agent: "none",
                        turns: 0,
                        started_at: "Run Lineage: Ingest Sessions",
                    },
                    { command: false }
                ),
            ];
        }
        return this.sessions.map((s) => new SessionTreeItem(s));
    }
}
