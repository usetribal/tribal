import * as vscode from "vscode";
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
            `Lineage: ${conversation.agent}`,
            column,
            { enableScripts: true, retainContextWhenHidden: true }
        );

        SessionPanel.current = new SessionPanel(panel, extensionUri);
        SessionPanel.current.update(conversation);
    }

    private update(conversation: Conversation): void {
        this.panel.title = `Lineage: ${conversation.agent}`;
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

function renderSessionHtml(conversation: Conversation): string {
    const turns = conversation.turns.map(renderTurn).join("");
    const privateNote = conversation.private
        ? "<p class='private'>This session is marked private.</p>"
        : "";
    const summaryBlock = conversation.architecture_summary
        ? `<div class="summary"><h2>Architecture summary</h2><pre>${escapeHtml(conversation.architecture_summary)}</pre></div>`
        : "";
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
    <h1>${escapeHtml(conversation.agent)} session</h1>
    <div class="meta">
      ${escapeHtml(conversation.id)} · started ${escapeHtml(conversation.started_at)}
      · ${conversation.turns.length} turns
    </div>
    ${privateNote}
  </div>
  ${summaryBlock}
  <div class="timeline">${turns || "<p>No turns recorded.</p>"}</div>
</body>
</html>`;
}
