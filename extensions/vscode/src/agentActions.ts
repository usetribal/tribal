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

// Whether to *offer* resume, not whether resume will work — the CLI decides
// that, and declines by name when it does not. This only avoids showing an
// action that is certain to fail: without a vendor id there is no session on
// this machine to reopen, whatever the agent. `agent` is the one vendor fact
// allowed above the adapter layer (ARCHITECTURE.md invariant 4); Cursor's
// recorded id belongs to its IDE store, which its resume CLI does not read.
export function canResumeSession(conv: Conversation): boolean {
    if (!vendorSessionId(conv)) {
        return false;
    }
    const agent = conv.agent.toLowerCase();
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

// Resume is `tribal resume`, entire — the same arrangement as fork below,
// and for the same reason. The extension knows no harness verb, no flag, and no
// id convention (ARCHITECTURE.md invariant 4); it runs the command and shows
// what the command said.
//
// It no longer sends the command to a terminal for the user. Resume reopens a
// live session, and which shell that lands in is the user's decision. Showing
// the command also puts the directory it must be run from on screen, which a
// terminal spawned at the workspace root quietly assumed.
export async function resumeAgentSession(client: LineageClient, sessionId: string): Promise<void> {
    const output = await client.resume(sessionId);
    await showCliOutput(output);
}

// Forking is `tribal fork`, entire. The extension holds no fork logic: it
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
    await showCliOutput(output);
}

async function showCliOutput(output: string): Promise<void> {
    const doc = await vscode.workspace.openTextDocument({
        content: output,
        language: "plaintext",
    });
    await vscode.window.showTextDocument(doc, { preview: false });
}
