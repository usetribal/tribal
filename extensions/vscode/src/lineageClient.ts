import { execFile } from "child_process";
import { promisify } from "util";
import * as vscode from "vscode";
import type { BlameResult, Conversation, SessionSummary } from "./types";

const execFileAsync = promisify(execFile);

export class LineageClient {
    constructor(private readonly workspaceRoot: string) {}

    private async run(args: string[]): Promise<string> {
        const config = vscode.workspace.getConfiguration("lineage");
        const cliPath = config.get<string>("cliPath", "").trim();

        if (cliPath) {
            const { stdout } = await execFileAsync(cliPath, args, {
                cwd: this.workspaceRoot,
                maxBuffer: 10 * 1024 * 1024,
            });
            return stdout;
        }

        const { stdout } = await execFileAsync("git", ["lineage", ...args], {
            cwd: this.workspaceRoot,
            maxBuffer: 10 * 1024 * 1024,
        });
        return stdout;
    }

    async doctor(): Promise<string> {
        return this.run(["doctor"]);
    }

    async ingest(incremental = true): Promise<string> {
        const args = ["ingest", "--agent", "all", "--no-link-head"];
        if (incremental) {
            args.push("--incremental");
        }
        return this.run(args);
    }

    async materialize(sessionId?: string): Promise<string> {
        const args = ["materialize"];
        if (sessionId) {
            args.push("--session", sessionId);
        }
        return this.run(args);
    }

    async remap(): Promise<string> {
        return this.run(["remap"]);
    }

    async initConfig(): Promise<string> {
        return this.run(["init-config"]);
    }

    async listSessions(): Promise<SessionSummary[]> {
        const out = await this.run(["list", "--json"]);
        return JSON.parse(out) as SessionSummary[];
    }

    async showSession(id: string, hydrateImages = true): Promise<Conversation> {
        const args = ["show", id, "--json"];
        if (hydrateImages) {
            args.push("--hydrate-images");
        }
        const out = await this.run(args);
        const conv = JSON.parse(out) as Conversation & { metadata?: Record<string, unknown> };
        const meta = conv.metadata ?? {};
        const summary = meta.architecture_summary;
        if (typeof summary === "string") {
            conv.architecture_summary = summary;
        }
        return conv;
    }

    async blame(path: string, line: number): Promise<BlameResult> {
        const out = await this.run(["blame", `${path}:${line}`, "--json"]);
        return JSON.parse(out) as BlameResult;
    }

    async search(query: string): Promise<string> {
        return this.run(["search", query]);
    }

    async installHook(): Promise<string> {
        return this.run(["install-hook"]);
    }

    async deleteSession(id: string, purgeBlobs = false): Promise<string> {
        const args = ["delete", id];
        if (purgeBlobs) {
            args.push("--purge-blobs");
        }
        return this.run(args);
    }
}

export function getWorkspaceRoot(): string | undefined {
    return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
}
