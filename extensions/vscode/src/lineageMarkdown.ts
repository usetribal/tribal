import * as vscode from "vscode";
import type { Conversation } from "./types";

export function trustedMarkdown(): vscode.MarkdownString {
    const md = new vscode.MarkdownString();
    md.isTrusted = true;
    md.supportThemeIcons = true;
    return md;
}

export function commandLink(label: string, command: string, args: unknown[] = []): string {
    return `[${label}](command:${command}?${encodeURIComponent(JSON.stringify(args))})`;
}

export function agentLabel(agent: string): string {
    switch (agent.toLowerCase()) {
        case "cursor":
            return "Cursor";
        case "claude":
            return "Claude Code";
        case "codex":
            return "Codex";
        default:
            return agent;
    }
}

export function agentIcon(agent: string): string {
    switch (agent.toLowerCase()) {
        case "cursor":
            return "$(terminal)";
        case "claude":
            return "$(comment-discussion)";
        case "codex":
            return "$(rocket)";
        default:
            return "$(question)";
    }
}

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

/** A harness-spawned branch (sidechain/subagent). A fork has a parent too, but
 *  it is a deliberate continuation by a person, not a branch the agent made. */
export function isBranchSession(conv: Conversation): boolean {
    if (conv.fork_origin) {
        return false;
    }
    if (conv.parent_session_id) {
        return true;
    }
    return conv.metadata?.is_sidechain === true;
}

export function primaryModel(conv: Conversation): string | undefined {
    for (const turn of conv.turns) {
        if (turn.model) {
            return turn.model;
        }
    }
    return metaString(conv, "model");
}

export function promptedByLabel(conv: Conversation): string | undefined {
    const email =
        typeof conv.metadata?.prompted_by_email === "string"
            ? conv.metadata.prompted_by_email
            : undefined;
    const name =
        typeof conv.metadata?.prompted_by_name === "string"
            ? conv.metadata.prompted_by_name
            : undefined;
    if (name && email) {
        return `${name} <${email}>`;
    }
    return email ?? name;
}
