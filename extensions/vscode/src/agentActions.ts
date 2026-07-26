import * as vscode from "vscode";
import type { LineageClient } from "./lineageClient";
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

export function canResumeSession(conv: Conversation): boolean {
    const agent = conv.agent.toLowerCase();
    const vendorId = vendorSessionId(conv);
    if (!vendorId) {
        return false;
    }
    return agent === "claude" || agent === "codex";
}

// Fork asks the CLI, which resolves the session from lineage's own refs — so no
// local transcript file and no vendor session id are required. Gating on either
// is what made a teammate's session unforkable: `metadata["source"]` is an
// absolute path on the machine that did the import, and it dangles everywhere
// else. `agent` is the only vendor fact allowed above the adapter layer
// (ARCHITECTURE.md invariant 4), and today claude is the only one whose
// transcripts can be written back.
export function canForkSession(conv: Conversation): boolean {
    return conv.agent.toLowerCase() === "claude";
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

// Forking is `git lineage fork`, entire. The extension holds no fork logic: it
// does not know the harness state directory, the transcript format, the id
// convention, or the flags that reopen a session — all adapter knowledge
// (ARCHITECTURE.md invariant 4). It runs the command and shows what the command
// said.
//
// The CLI's answer is several lines — whose session this was, what it was about,
// what it touched, where the transcript landed, and only then the command to
// run. A notification truncates that to its first clause, so it is opened as a
// document instead (the `doctor` precedent) where it can be read and the command
// copied. The command is deliberately not executed for the user: it opens a
// colleague's work in an interactive agent, which is a thing to choose rather
// than have happen.
export async function forkAgentSession(client: LineageClient, sessionId: string): Promise<void> {
    const output = await client.fork(sessionId);
    const doc = await vscode.workspace.openTextDocument({
        content: output,
        language: "plaintext",
    });
    await vscode.window.showTextDocument(doc, { preview: false });
}

function shellQuote(value: string): string {
    if (/^[a-zA-Z0-9._-]+$/.test(value)) {
        return value;
    }
    return `'${value.replace(/'/g, `'\\''`)}'`;
}
