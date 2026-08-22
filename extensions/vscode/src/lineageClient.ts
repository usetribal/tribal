import { execFile } from "child_process";
import { promisify } from "util";
import * as vscode from "vscode";
import type { BlameResult, Conversation, SessionSummary } from "./types";

const execFileAsync = promisify(execFile);

export class LineageClient {
    private readonly sessionCache = new Map<string, Conversation>();

    constructor(readonly workspaceRoot: string) {}

    clearSessionCache(): void {
        this.sessionCache.clear();
    }

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

    async importSessions(incremental = true): Promise<string> {
        const args = ["import", "--agent", "all", "--no-link-head"];
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
        return this.run(["init", "--config"]);
    }

    async listSessions(): Promise<SessionSummary[]> {
        const out = await this.run(["list", "--json"]);
        return JSON.parse(out) as SessionSummary[];
    }

    async showSession(id: string, hydrateImages = true): Promise<Conversation> {
        const conv = await this.fetchSession(id, hydrateImages);
        this.sessionCache.set(id, conv);
        return conv;
    }

    async getSession(id: string): Promise<Conversation | undefined> {
        const cached = this.sessionCache.get(id);
        if (cached) {
            return cached;
        }
        try {
            return await this.fetchSession(id, false);
        } catch {
            return undefined;
        }
    }

    private async fetchSession(id: string, hydrateImages: boolean): Promise<Conversation> {
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

    async viewCommit(sha: string): Promise<string> {
        const { stdout } = await execFileAsync("git", ["show", "--stat", "--patch", sha], {
            cwd: this.workspaceRoot,
            maxBuffer: 10 * 1024 * 1024,
        });
        return stdout;
    }

    async blame(path: string, line: number): Promise<BlameResult> {
        const out = await this.run(["blame", `${path}:${line}`, "--json"]);
        return JSON.parse(out) as BlameResult;
    }

    async search(query: string): Promise<string> {
        return this.run(["search", query]);
    }

    async fork(id: string): Promise<string> {
        return this.run(["fork", "--new", id]);
    }

    async resume(id: string): Promise<string> {
        return this.run(["fork", id]);
    }

    async installHook(): Promise<string> {
        return this.run(["init", "--hooks"]);
    }

    async deleteSession(id: string, purgeBlobs = false): Promise<string> {
        const args = ["delete", id];
        if (purgeBlobs) {
            args.push("--purge-blobs");
        }
        return this.run(args);
    }
}

// `execFile` rejects with an Error whose `message` is its own summary ("Command
// failed: ...") and whose `stderr` holds what the CLI actually said. Lineage's
// errors are written to be read by the person who hit them, so prefer stderr.
type ExecFailure = { stderr?: string };

export function cliMessage(error: unknown): string {
    const stderr = (error as ExecFailure)?.stderr;
    if (typeof stderr === "string" && stderr.trim().length > 0) {
        return stderr.trim();
    }
    return error instanceof Error ? error.message : String(error);
}

export function getWorkspaceRoot(): string | undefined {
    return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
}
