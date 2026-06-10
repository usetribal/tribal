import * as fs from "fs/promises";
import * as path from "path";
import * as vscode from "vscode";
import type { Conversation } from "./types";

function metaString(conv: Conversation, key: string): string | undefined {
    const value = conv.metadata?.[key];
    return typeof value === "string" && value.length > 0 ? value : undefined;
}

export function vendorSessionId(conv: Conversation): string | undefined {
    return (
        metaString(conv, "cursor_session_id") ??
        metaString(conv, "claude_session_id") ??
        metaString(conv, "codex_session_id")
    );
}

export function transcriptPath(conv: Conversation): string | undefined {
    return metaString(conv, "source");
}

export function canResumeSession(conv: Conversation): boolean {
    const agent = conv.agent.toLowerCase();
    const vendorId = vendorSessionId(conv);
    if (!vendorId) {
        return false;
    }
    return agent === "claude" || agent === "codex";
}

export function canForkSession(conv: Conversation): boolean {
    return Boolean(transcriptPath(conv)) && canResumeSession(conv);
}

function runInWorkspaceTerminal(workspaceRoot: string, title: string, command: string): void {
    const terminal = vscode.window.createTerminal({
        name: title,
        cwd: workspaceRoot,
    });
    terminal.show(true);
    terminal.sendText(command, true);
}

export async function resumeAgentSession(conv: Conversation, workspaceRoot: string): Promise<void> {
    const agent = conv.agent.toLowerCase();
    const vendorId = vendorSessionId(conv);
    if (!vendorId) {
        throw new Error(`No ${agent} session id available to resume`);
    }

    let command: string | undefined;
    switch (agent) {
        case "claude":
            command = `claude --resume ${shellQuote(vendorId)}`;
            break;
        case "codex":
            command = `codex resume ${shellQuote(vendorId)}`;
            break;
        default:
            throw new Error(`Resume is not supported for ${agent} sessions yet`);
    }

    runInWorkspaceTerminal(workspaceRoot, "Lineage: Resume", command);
    vscode.window.setStatusBarMessage(`Lineage: resuming ${agent} session`, 3000);
}

export async function forkAgentSession(conv: Conversation, workspaceRoot: string): Promise<void> {
    const source = transcriptPath(conv);
    if (!source) {
        throw new Error("No transcript path stored for this session");
    }

    const forksDir = path.join(workspaceRoot, ".lineage", "forks");
    await fs.mkdir(forksDir, { recursive: true });

    const ext = path.extname(source) || ".jsonl";
    const stem = path.basename(source, ext).replace(/[^a-zA-Z0-9._-]+/g, "-");
    const forkFile = path.join(forksDir, `${stem}.fork-${Date.now()}${ext}`);
    await fs.copyFile(source, forkFile);

    const agent = conv.agent.toLowerCase();
    const vendorId = vendorSessionId(conv);

    if (agent === "claude" && vendorId) {
        runInWorkspaceTerminal(
            workspaceRoot,
            "Lineage: Fork",
            `claude --resume ${shellQuote(vendorId)} --fork-session`
        );
        vscode.window.showInformationMessage(
            `Forked Claude session (original transcript copied to ${path.relative(workspaceRoot, forkFile)})`
        );
        return;
    }

    if (agent === "codex" && vendorId) {
        runInWorkspaceTerminal(
            workspaceRoot,
            "Lineage: Fork",
            `codex resume ${shellQuote(vendorId)}`
        );
        vscode.window.showInformationMessage(
            `Fork transcript saved to ${path.relative(workspaceRoot, forkFile)}. Codex resume opens a branch without modifying the original rollout file.`
        );
        return;
    }

    vscode.window.showInformationMessage(
        `Conversation copied to ${path.relative(workspaceRoot, forkFile)}`
    );
}

function shellQuote(value: string): string {
    if (/^[a-zA-Z0-9._-]+$/.test(value)) {
        return value;
    }
    return `'${value.replace(/'/g, `'\\''`)}'`;
}
