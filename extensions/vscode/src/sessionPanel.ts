import * as vscode from "vscode";
import {
    agentLabel,
    isBranchSession,
    primaryModel,
    promptedByLabel,
    vendorSessionId,
} from "./lineageMarkdown";
import type { Artifact, Conversation, ToolCall, Turn } from "./types";

export class SessionPanel {
    public static current: SessionPanel | undefined;
    private readonly panel: vscode.WebviewPanel;

    private constructor(
        panel: vscode.WebviewPanel,
        private readonly extensionUri: vscode.Uri
    ) {
        this.panel = panel;
        this.panel.onDidDispose(() => {
            SessionPanel.current = undefined;
        });
        this.panel.webview.onDidReceiveMessage((msg: { command?: string; id?: string }) => {
            if (msg.command === "openSession" && msg.id) {
                void vscode.commands.executeCommand("lineage.openSession", msg.id);
            }
            if (msg.command === "viewCommit" && msg.id) {
                void vscode.commands.executeCommand("lineage.viewCommit", msg.id);
            }
        });
    }

    public static show(extensionUri: vscode.Uri, conversation: Conversation): void {
        const column = vscode.ViewColumn.Beside;
        if (SessionPanel.current) {
            SessionPanel.current.panel.reveal(column);
            SessionPanel.current.update(conversation);
            return;
        }

        const panel = vscode.window.createWebviewPanel(
            "lineageSession",
            `Lineage: ${agentLabel(conversation.agent)}`,
            column,
            { enableScripts: true, retainContextWhenHidden: true }
        );

        SessionPanel.current = new SessionPanel(panel, extensionUri);
        SessionPanel.current.update(conversation);
    }

    private update(conversation: Conversation): void {
        this.panel.title = `Lineage: ${agentLabel(conversation.agent)}`;
        this.panel.webview.html = renderSessionHtml(conversation);
    }
}

function escapeHtml(s: string): string {
    return s
        .replace(/&/g, "&amp;")
        .replace(/</g, "&lt;")
        .replace(/>/g, "&gt;")
        .replace(/"/g, "&quot;");
}

function escapeAttr(s: string): string {
    return escapeHtml(s).replace(/'/g, "&#39;");
}

function metaString(conv: Conversation, key: string): string | undefined {
    const value = conv.metadata?.[key];
    return typeof value === "string" && value.length > 0 ? value : undefined;
}

function renderMetaLink(label: string, command: string, id: string): string {
    return `<button class="link-btn" data-command="${escapeAttr(command)}" data-id="${escapeAttr(id)}">${escapeHtml(label)}</button>`;
}

function renderHeaderMeta(conversation: Conversation): string {
    const items: string[] = [];
    const promptedBy = promptedByLabel(conversation);
    if (promptedBy) {
        items.push(`<span class="chip author">Prompted by: ${escapeHtml(promptedBy)}</span>`);
    }
    const model = primaryModel(conversation);
    if (model) {
        items.push(`<span class="chip">Model: ${escapeHtml(model)}</span>`);
    }
    const branch = metaString(conversation, "git_branch");
    if (branch) {
        items.push(`<span class="chip branch">Branch: ${escapeHtml(branch)}</span>`);
    }
    if (isBranchSession(conversation)) {
        items.push(`<span class="chip branch">Branched conversation</span>`);
    }
    const vendorId = vendorSessionId(conversation);
    if (vendorId) {
        items.push(
            `<span class="chip vendor">${escapeHtml(agentLabel(conversation.agent))} ID: <code>${escapeHtml(vendorId)}</code></span>`
        );
    }
    const source = metaString(conversation, "source");
    if (source) {
        items.push(
            `<span class="chip source">Transcript: <code>${escapeHtml(source)}</code></span>`
        );
    }
    if (conversation.parent_session_id) {
        items.push(
            renderMetaLink(
                `Parent session ${conversation.parent_session_id.slice(0, 12)}…`,
                "openSession",
                conversation.parent_session_id
            )
        );
    }
    for (const sha of conversation.commit_shas ?? []) {
        items.push(renderMetaLink(`Commit ${sha.slice(0, 12)}`, "viewCommit", sha));
    }
    if (!items.length) {
        return "";
    }
    return `<div class="meta-chips">${items.join("")}</div>`;
}

function renderToolCalls(toolCalls: ToolCall[]): string {
    if (!toolCalls.length) {
        return "";
    }
    const items = toolCalls
        .map((tc) => {
            const args =
                tc.arguments.length > 500
                    ? `${escapeHtml(tc.arguments.slice(0, 500))}…`
                    : escapeHtml(tc.arguments);
            const result = tc.result
                ? `<pre class="tool-result">${escapeHtml(
                      tc.result.length > 800 ? `${tc.result.slice(0, 800)}…` : tc.result
                  )}</pre>`
                : "";
            return `
        <li class="tool-call">
          <span class="tool-name">${escapeHtml(tc.name)}</span>
          <pre class="tool-args">${args}</pre>
          ${result}
        </li>
      `;
        })
        .join("");
    return `<ul class="tool-calls">${items}</ul>`;
}

function isMediaArtifact(kind: string): boolean {
    return kind === "image" || kind === "screenshot" || kind === "diagram";
}

function renderArtifacts(artifacts: Artifact[]): string {
    if (!artifacts.length) {
        return "";
    }
    const items = artifacts
        .map((a) => {
            const range = a.line_range ? `:${a.line_range[0]}-${a.line_range[1]}` : "";
            const preview = a.preview_data_url;
            if (isMediaArtifact(a.kind) && preview) {
                return `<li class="media-artifact">
          <code>${escapeHtml(a.kind)}</code> ${escapeHtml(a.path)}${range}
          <img class="artifact-preview" src="${escapeHtml(preview)}" alt="${escapeHtml(a.kind)}" />
        </li>`;
            }
            return `<li><code>${escapeHtml(a.kind)}</code> ${escapeHtml(a.path)}${range}</li>`;
        })
        .join("");
    return `<ul class="artifacts">${items}</ul>`;
}

function renderTurn(turn: Turn, index: number): string {
    const preview =
        turn.content.length > 2000
            ? `${escapeHtml(turn.content.slice(0, 2000))}…`
            : escapeHtml(turn.content);
    const meta = [turn.model, turn.timestamp].filter(Boolean).join(" · ");
    const tools = turn.tool_calls ? renderToolCalls(turn.tool_calls) : "";
    const artifacts = turn.artifacts ? renderArtifacts(turn.artifacts) : "";
    return `
    <article class="turn turn-${turn.role}">
      <header>
        <span class="role">${escapeHtml(turn.role)}</span>
        <span class="index">#${index + 1}</span>
        ${meta ? `<span class="meta">${escapeHtml(meta)}</span>` : ""}
      </header>
      ${preview ? `<pre class="content">${preview}</pre>` : ""}
      ${tools}
      ${artifacts}
    </article>
  `;
}

function sessionHeading(conversation: Conversation): string {
    const metadata = conversation.metadata ?? {};
    const summary =
        (typeof metadata.session_summary === "string" && metadata.session_summary.trim()) ||
        (typeof metadata.architecture_summary === "string" &&
            metadata.architecture_summary.split("\n")[0]?.trim()) ||
        "";
    if (summary) {
        return summary;
    }
    return `${agentLabel(conversation.agent)} conversation`;
}

function renderSessionHtml(conversation: Conversation): string {
    const turns = conversation.turns.map(renderTurn).join("");
    const privateNote = conversation.private
        ? "<p class='private'>This session is marked private.</p>"
        : "";
    const metadata = conversation.metadata ?? {};
    const vendorSummary =
        typeof metadata.session_summary === "string" ? metadata.session_summary : undefined;
    const summaryBlock = vendorSummary
        ? `<div class="summary"><h2>Session summary</h2><pre>${escapeHtml(vendorSummary)}</pre></div>`
        : conversation.architecture_summary
          ? `<div class="summary"><h2>Architecture summary</h2><pre>${escapeHtml(conversation.architecture_summary)}</pre></div>`
          : "";
    const ended = conversation.ended_at ? ` · ended ${escapeHtml(conversation.ended_at)}` : "";
    return `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1.0" />
  <style>
    :root {
      --bg: var(--vscode-editor-background);
      --fg: var(--vscode-editor-foreground);
      --border: var(--vscode-panel-border);
      --muted: var(--vscode-descriptionForeground);
      --link: var(--vscode-textLink-foreground);
      --user-bg: color-mix(in srgb, var(--vscode-inputValidation-infoBorder) 12%, var(--bg));
      --assistant-bg: color-mix(in srgb, var(--vscode-gitDecoration-addedResourceForeground) 10%, var(--bg));
      --tool-bg: color-mix(in srgb, var(--vscode-inputValidation-warningBorder) 10%, var(--bg));
    }
    * { box-sizing: border-box; }
    body {
      margin: 0;
      padding: 16px;
      font-family: var(--vscode-font-family);
      font-size: var(--vscode-font-size);
      color: var(--fg);
      background: var(--bg);
    }
    .header {
      margin-bottom: 20px;
      padding-bottom: 12px;
      border-bottom: 1px solid var(--border);
    }
    .header h1 {
      margin: 0 0 6px;
      font-size: 1.1rem;
      font-weight: 600;
    }
    .header .meta { color: var(--muted); font-size: 0.85rem; }
    .meta-chips {
      display: flex;
      flex-wrap: wrap;
      gap: 8px;
      margin-top: 10px;
    }
    .chip, .link-btn {
      display: inline-flex;
      align-items: center;
      gap: 4px;
      padding: 4px 8px;
      border-radius: 6px;
      border: 1px solid var(--border);
      background: color-mix(in srgb, var(--link) 8%, var(--bg));
      font-size: 0.78rem;
      color: var(--fg);
    }
    .link-btn {
      cursor: pointer;
      color: var(--link);
      font-family: inherit;
    }
    .link-btn:hover { text-decoration: underline; }
    .chip.branch { border-color: var(--vscode-gitDecoration-modifiedResourceForeground); }
    .chip code { font-size: 0.75rem; }
    .private { color: var(--vscode-inputValidation-warningForeground); }
    .timeline { display: flex; flex-direction: column; gap: 12px; }
    .turn {
      border: 1px solid var(--border);
      border-radius: 8px;
      overflow: hidden;
    }
    .turn-user { background: var(--user-bg); }
    .turn-assistant { background: var(--assistant-bg); }
    .turn-tool { background: var(--tool-bg); }
    .turn header {
      display: flex;
      gap: 8px;
      align-items: center;
      padding: 8px 12px;
      border-bottom: 1px solid var(--border);
      font-size: 0.8rem;
    }
    .role { font-weight: 600; text-transform: uppercase; letter-spacing: 0.04em; }
    .index { color: var(--muted); }
    .meta { margin-left: auto; color: var(--muted); }
    .artifact-preview {
      display: block;
      max-width: 100%;
      max-height: 320px;
      margin-top: 8px;
      border: 1px solid var(--border);
      border-radius: 4px;
    }
    .media-artifact { list-style: none; margin-bottom: 8px; }
    .content, .tool-args, .tool-result {
      margin: 0;
      padding: 12px;
      white-space: pre-wrap;
      word-break: break-word;
      font-family: var(--vscode-editor-font-family);
      font-size: 0.9rem;
      line-height: 1.45;
      background: transparent;
      color: inherit;
    }
    .tool-calls, .artifacts {
      margin: 0;
      padding: 8px 12px 12px 28px;
      font-size: 0.85rem;
    }
    .tool-name { font-weight: 600; }
    .tool-result { padding-top: 4px; color: var(--muted); }
    .artifacts code { font-size: 0.8rem; }
    .summary {
      margin-bottom: 16px;
      padding: 12px;
      border: 1px solid var(--border);
      border-radius: 8px;
      background: color-mix(in srgb, var(--vscode-textLink-foreground) 8%, var(--bg));
    }
    .summary h2 { margin: 0 0 8px; font-size: 0.95rem; }
    .summary pre { margin: 0; white-space: pre-wrap; font-size: 0.85rem; }
  </style>
</head>
<body>
  <div class="header">
    <h1>${escapeHtml(sessionHeading(conversation))}</h1>
    <div class="meta">
      <code>${escapeHtml(conversation.id)}</code>
      · started ${escapeHtml(conversation.started_at)}${ended}
      · ${conversation.turns.length} turns
    </div>
    ${renderHeaderMeta(conversation)}
    ${privateNote}
  </div>
  ${summaryBlock}
  <div class="timeline">${turns || "<p>No turns recorded.</p>"}</div>
  <script>
    const vscode = acquireVsCodeApi();
    document.querySelectorAll('.link-btn').forEach((btn) => {
      btn.addEventListener('click', () => {
        vscode.postMessage({
          command: btn.dataset.command,
          id: btn.dataset.id,
        });
      });
    });
  </script>
</body>
</html>`;
}
