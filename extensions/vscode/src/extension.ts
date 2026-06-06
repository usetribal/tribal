import * as vscode from "vscode";
import { getWorkspaceRoot, LineageClient } from "./lineageClient";
import { LineageDecorator } from "./lineageDecorator";
import { LineageHoverProvider } from "./lineageHoverProvider";
import { SessionPanel } from "./sessionPanel";
import { SessionsProvider } from "./sessionsProvider";

let client: LineageClient | undefined;
let sessionsProvider: SessionsProvider | undefined;
let decorator: LineageDecorator | undefined;

export function activate(context: vscode.ExtensionContext): void {
    const root = getWorkspaceRoot();
    if (!root) {
        return;
    }

    client = new LineageClient(root);
    sessionsProvider = new SessionsProvider(client);
    decorator = new LineageDecorator(client);

    const tree = vscode.window.createTreeView("lineageSessions", {
        treeDataProvider: sessionsProvider,
        showCollapseAll: false,
    });

    const hoverProvider = new LineageHoverProvider(client);
    context.subscriptions.push(
        tree,
        decorator,
        vscode.languages.registerHoverProvider({ scheme: "file" }, hoverProvider),
        vscode.commands.registerCommand("lineage.refresh", async () => {
            await sessionsProvider?.load();
        }),
        vscode.commands.registerCommand("lineage.ingest", async () => {
            try {
                await vscode.window.withProgress(
                    {
                        location: vscode.ProgressLocation.Notification,
                        title: "Lineage ingest",
                        cancellable: false,
                    },
                    async () => client!.ingest()
                );
                if (
                    vscode.workspace.getConfiguration("lineage").get<boolean>("autoRefresh", true)
                ) {
                    await sessionsProvider?.load();
                }
                vscode.window.showInformationMessage("Lineage sessions ingested.");
            } catch (e) {
                vscode.window.showErrorMessage(`Lineage ingest failed: ${e}`);
            }
        }),
        vscode.commands.registerCommand("lineage.openSession", async (sessionId?: string) => {
            const id =
                sessionId ??
                (await vscode.window.showInputBox({
                    prompt: "Session ID",
                    placeHolder: "Paste a lineage session id",
                }));
            if (!id || id === "empty" || id === "error") {
                return;
            }
            try {
                const conversation = await client!.showSession(id);
                SessionPanel.show(context.extensionUri, conversation);
            } catch (e) {
                vscode.window.showErrorMessage(`Failed to open session: ${e}`);
            }
        }),
        vscode.commands.registerCommand("lineage.showLineage", async () => {
            const editor = vscode.window.activeTextEditor;
            if (!editor) {
                return;
            }
            const rel = vscode.workspace.asRelativePath(editor.document.uri);
            const line = editor.selection.active.line + 1;
            try {
                const result = await client!.blame(rel, line);
                if (result.sessions.length === 0) {
                    vscode.window.showInformationMessage(`No lineage found for ${rel}:${line}`);
                    return;
                }
                const pick = await vscode.window.showQuickPick(
                    result.sessions.map((id) => ({
                        label: id.slice(0, 16),
                        description: result.matches?.find((m) => m.conversation_id === id)
                            ?.content_preview,
                        id,
                    })),
                    { placeHolder: `Lineage for ${rel}:${line}` }
                );
                if (!pick) {
                    return;
                }
                const conversation = await client!.showSession(pick.id);
                SessionPanel.show(context.extensionUri, conversation);
            } catch (e) {
                vscode.window.showErrorMessage(`Lineage blame failed: ${e}`);
            }
        }),
        vscode.commands.registerCommand("lineage.installHook", async () => {
            try {
                const out = await client!.installHook();
                vscode.window.showInformationMessage(out.trim() || "Git hooks installed.");
            } catch (e) {
                vscode.window.showErrorMessage(`Install hook failed: ${e}`);
            }
        }),
        vscode.commands.registerCommand("lineage.doctor", async () => {
            try {
                const out = await client!.doctor();
                const doc = await vscode.workspace.openTextDocument({
                    content: out,
                    language: "plaintext",
                });
                await vscode.window.showTextDocument(doc);
            } catch (e) {
                vscode.window.showErrorMessage(`Doctor failed: ${e}`);
            }
        }),
        vscode.commands.registerCommand("lineage.materialize", async () => {
            try {
                const out = await vscode.window.withProgress(
                    {
                        location: vscode.ProgressLocation.Notification,
                        title: "Lineage materialize",
                        cancellable: false,
                    },
                    async () => client!.materialize()
                );
                vscode.window.showInformationMessage(out.trim() || "Materialize complete.");
            } catch (e) {
                vscode.window.showErrorMessage(`Materialize failed: ${e}`);
            }
        }),
        vscode.commands.registerCommand("lineage.remap", async () => {
            try {
                const out = await client!.remap();
                vscode.window.showInformationMessage(out.trim() || "Remap complete.");
            } catch (e) {
                vscode.window.showErrorMessage(`Remap failed: ${e}`);
            }
        }),
        vscode.commands.registerCommand("lineage.initConfig", async () => {
            try {
                const out = await client!.initConfig();
                vscode.window.showInformationMessage(out.trim() || "Config initialized.");
            } catch (e) {
                vscode.window.showErrorMessage(`Init config failed: ${e}`);
            }
        }),
        vscode.commands.registerCommand("lineage.deleteSession", async (sessionId?: string) => {
            const id =
                sessionId ??
                (await vscode.window.showInputBox({
                    prompt: "Session ID to delete",
                }));
            if (!id || id === "empty" || id === "error") {
                return;
            }
            const purge = await vscode.window.showQuickPick(
                [
                    { label: "Keep LFS blobs", value: false },
                    { label: "Purge unreferenced LFS blobs", value: true },
                ],
                { placeHolder: "Blob purge policy" }
            );
            if (!purge) {
                return;
            }
            try {
                const out = await client!.deleteSession(id, purge.value);
                await sessionsProvider?.load();
                vscode.window.showInformationMessage(out.trim() || `Deleted session ${id}`);
            } catch (e) {
                vscode.window.showErrorMessage(`Delete session failed: ${e}`);
            }
        }),
        vscode.commands.registerCommand("lineage.search", async () => {
            const query = await vscode.window.showInputBox({
                prompt: "Search lineage sessions",
                placeHolder: "e.g. authentication middleware",
            });
            if (!query) {
                return;
            }
            try {
                const out = await client!.search(query);
                const doc = await vscode.workspace.openTextDocument({
                    content: out,
                    language: "plaintext",
                });
                await vscode.window.showTextDocument(doc);
            } catch (e) {
                vscode.window.showErrorMessage(`Search failed: ${e}`);
            }
        })
    );

    void sessionsProvider.load();
}

export function deactivate(): void {
    client = undefined;
    sessionsProvider = undefined;
    decorator = undefined;
}
