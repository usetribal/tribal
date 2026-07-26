import * as vscode from "vscode";
import { forkAgentSession, resumeAgentSession } from "./agentActions";
import { cliMessage, getWorkspaceRoot, LineageClient } from "./lineageClient";
import { LineageDecorator } from "./lineageDecorator";
import { LineageHoverProvider } from "./lineageHoverProvider";
import { SessionPanel } from "./sessionPanel";
import { SessionsProvider } from "./sessionsProvider";

let client: LineageClient | undefined;
let sessionsProvider: SessionsProvider | undefined;
let decorator: LineageDecorator | undefined;
let workspaceDisposables: vscode.Disposable[] = [];

function disposeWorkspace(): void {
    for (const d of workspaceDisposables) {
        d.dispose();
    }
    workspaceDisposables = [];
    decorator = undefined;
    client = undefined;
    sessionsProvider?.setClient(undefined);
}

function initWorkspace(context: vscode.ExtensionContext, root: string): void {
    disposeWorkspace();

    client = new LineageClient(root);
    if (!sessionsProvider) {
        sessionsProvider = new SessionsProvider();
    }
    sessionsProvider.setClient(client);
    decorator = new LineageDecorator(client);

    const hoverProvider = new LineageHoverProvider(client);
    workspaceDisposables.push(
        decorator,
        vscode.languages.registerHoverProvider({ scheme: "file" }, hoverProvider)
    );

    void sessionsProvider.load();
}

function requireClient(): LineageClient | undefined {
    const root = getWorkspaceRoot();
    if (!root) {
        void promptOpenFolder();
        return undefined;
    }
    if (!client || client.workspaceRoot !== root) {
        // Workspace folder changed (or first use after opening a folder).
        const ctx = extensionContext;
        if (ctx) {
            initWorkspace(ctx, root);
        }
    }
    return client;
}

let extensionContext: vscode.ExtensionContext | undefined;

async function resolveSessionId(sessionId?: string): Promise<string | undefined> {
    const id =
        sessionId ??
        (await vscode.window.showInputBox({
            prompt: "Session ID",
            placeHolder: "Paste a lineage session id",
        }));
    if (!id || id === "empty" || id === "error" || id === "hint") {
        return undefined;
    }
    return id;
}

async function promptOpenFolder(): Promise<void> {
    const choice = await vscode.window.showErrorMessage(
        "Lineage needs an open folder. Use File → Open Folder… and pick a git repository.",
        "Open Folder"
    );
    if (choice === "Open Folder") {
        await vscode.commands.executeCommand("workbench.action.files.openFolder");
    }
}

export function activate(context: vscode.ExtensionContext): void {
    extensionContext = context;

    sessionsProvider = new SessionsProvider();
    const tree = vscode.window.createTreeView("lineageSessions", {
        treeDataProvider: sessionsProvider,
        showCollapseAll: false,
    });
    context.subscriptions.push(tree);

    const withClient = <T extends unknown[]>(
        handler: (...args: T) => Promise<void> | void
    ): ((...args: T) => Promise<void>) => {
        return async (...args: T) => {
            if (!requireClient()) {
                return;
            }
            await handler(...args);
        };
    };

    context.subscriptions.push(
        vscode.commands.registerCommand(
            "lineage.refresh",
            withClient(async () => {
                await sessionsProvider?.load();
            })
        ),
        vscode.commands.registerCommand(
            "lineage.import",
            withClient(async () => {
                try {
                    await vscode.window.withProgress(
                        {
                            location: vscode.ProgressLocation.Notification,
                            title: "Lineage import",
                            cancellable: false,
                        },
                        async () => client!.importSessions()
                    );
                    client!.clearSessionCache();
                    if (
                        vscode.workspace
                            .getConfiguration("lineage")
                            .get<boolean>("autoRefresh", true)
                    ) {
                        await sessionsProvider?.load();
                    }
                    vscode.window.showInformationMessage("Lineage sessions imported.");
                } catch (e) {
                    vscode.window.showErrorMessage(`Lineage import failed: ${e}`);
                }
            })
        ),
        vscode.commands.registerCommand(
            "lineage.openSession",
            withClient(async (sessionId?: string) => {
                const id =
                    sessionId ??
                    (await vscode.window.showInputBox({
                        prompt: "Session ID",
                        placeHolder: "Paste a lineage session id",
                    }));
                if (!id || id === "empty" || id === "error" || id === "hint") {
                    return;
                }
                try {
                    const conversation = await client!.showSession(id);
                    SessionPanel.show(context.extensionUri, conversation);
                } catch (e) {
                    vscode.window.showErrorMessage(`Failed to open session: ${e}`);
                }
            })
        ),
        vscode.commands.registerCommand(
            "lineage.resumeSession",
            withClient(async (sessionId?: string) => {
                const id = await resolveSessionId(sessionId);
                if (!id) {
                    return;
                }
                try {
                    await resumeAgentSession(client!, id);
                } catch (e) {
                    // The CLI's stderr names the agent that cannot be resumed,
                    // or points at `git lineage fork` when the session is not on
                    // this machine. execFile's own prose buries both.
                    vscode.window.showErrorMessage(`Resume session failed: ${cliMessage(e)}`);
                }
            })
        ),
        vscode.commands.registerCommand(
            "lineage.forkSession",
            withClient(async (sessionId?: string) => {
                const id = await resolveSessionId(sessionId);
                if (!id) {
                    return;
                }
                try {
                    await forkAgentSession(client!, id);
                    await sessionsProvider?.load();
                } catch (e) {
                    // The CLI's stderr is the specific message — unknown id and
                    // what to do about it, or the agent that cannot be forked by
                    // name. Wrapping it in execFile's own prose buries that.
                    vscode.window.showErrorMessage(`Fork session failed: ${cliMessage(e)}`);
                }
            })
        ),
        vscode.commands.registerCommand(
            "lineage.showLineage",
            withClient(async () => {
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
            })
        ),
        vscode.commands.registerCommand(
            "lineage.installHook",
            withClient(async () => {
                try {
                    const out = await client!.installHook();
                    vscode.window.showInformationMessage(out.trim() || "Git hooks installed.");
                } catch (e) {
                    vscode.window.showErrorMessage(`Install hook failed: ${e}`);
                }
            })
        ),
        vscode.commands.registerCommand(
            "lineage.doctor",
            withClient(async () => {
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
            })
        ),
        vscode.commands.registerCommand(
            "lineage.materialize",
            withClient(async () => {
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
            })
        ),
        vscode.commands.registerCommand(
            "lineage.remap",
            withClient(async () => {
                try {
                    const out = await client!.remap();
                    vscode.window.showInformationMessage(out.trim() || "Remap complete.");
                } catch (e) {
                    vscode.window.showErrorMessage(`Remap failed: ${e}`);
                }
            })
        ),
        vscode.commands.registerCommand(
            "lineage.initConfig",
            withClient(async () => {
                try {
                    const out = await client!.initConfig();
                    vscode.window.showInformationMessage(out.trim() || "Config initialized.");
                } catch (e) {
                    vscode.window.showErrorMessage(`Init config failed: ${e}`);
                }
            })
        ),
        vscode.commands.registerCommand(
            "lineage.deleteSession",
            withClient(async (sessionId?: string) => {
                const id =
                    sessionId ??
                    (await vscode.window.showInputBox({
                        prompt: "Session ID to delete",
                    }));
                if (!id || id === "empty" || id === "error" || id === "hint") {
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
            })
        ),
        vscode.commands.registerCommand(
            "lineage.viewCommit",
            withClient(async (sha?: string) => {
                const commit =
                    sha ??
                    (await vscode.window.showInputBox({
                        prompt: "Commit SHA",
                        placeHolder: "e.g. 3f1be95317a2",
                    }));
                if (!commit) {
                    return;
                }
                try {
                    const out = await client!.viewCommit(commit);
                    const doc = await vscode.workspace.openTextDocument({
                        content: out,
                        language: "diff",
                    });
                    await vscode.window.showTextDocument(doc, { preview: true });
                } catch (e) {
                    vscode.window.showErrorMessage(`View commit failed: ${e}`);
                }
            })
        ),
        vscode.commands.registerCommand(
            "lineage.search",
            withClient(async () => {
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
        ),
        vscode.workspace.onDidChangeWorkspaceFolders(() => {
            const root = getWorkspaceRoot();
            if (root) {
                initWorkspace(context, root);
            } else {
                disposeWorkspace();
                void sessionsProvider?.load();
            }
        })
    );

    const root = getWorkspaceRoot();
    if (root) {
        initWorkspace(context, root);
    } else {
        void sessionsProvider.load();
        void vscode.window.showWarningMessage(
            "Lineage: open a folder (File → Open Folder…) to enable sessions and blame."
        );
    }
}

export function deactivate(): void {
    disposeWorkspace();
    client = undefined;
    sessionsProvider = undefined;
    extensionContext = undefined;
}
