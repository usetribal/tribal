# Lineage

**Git-native provenance for AI coding agents.**

Lineage connects agent conversations to the code they produce. Sessions from Cursor, Claude Code, and Codex are ingested into your repository as git refs and notes, so you can trace any line back to the prompt that wrote it. No external service. No separate database.

**[Setup](#setup)** · **[Getting started](#getting-started)** · [Agent skill](#agent-skill) · [CLI reference](#cli-reference) · [MCP server](#mcp-server) · [VS Code extension](#vs-code-extension)

## Why Lineage?

AI-generated code is everywhere. Git shows you what changed. Lineage shows you why.

It preserves the conversation behind each change and links it to commits, files, and line ranges. Your team gets context that survives `git push`, code review, and onboarding months later.

| | |
|---|---|
| **In-repo storage** | Lineage data travels with your repository |
| **Agent-agnostic** | Works with Cursor, Claude Code, and Codex |
| **Policy-first** | Secrets are redacted before anything is written |
| **Queryable** | CLI, MCP server, and VS Code extension |

## Setup

One command installs the CLI, builds the VS Code extension, writes repo config, and installs git hooks:

```bash
git clone https://github.com/lineage-dev/lineage.git && cd lineage && ./scripts/setup.sh /path/to/your-project
```

Already cloned? Configure your project or the lineage repo itself:

```bash
./scripts/setup.sh /path/to/your-project   # your app
./scripts/setup.sh                           # develop lineage in this repo
```

### What setup does

| Step | Action |
|------|--------|
| Install CLI | `cargo install` → `git-lineage` on `PATH` as `git lineage` |
| VS Code extension | `npm install` + compile under `extensions/vscode/` |
| Repo config | `git lineage init-config` → `refs/lineage/config` + `.gitattributes` |
| Agent skill | `git lineage init-skill` → `.cursor/`, `.claude/`, `.agents/skills/` (or pick targets) |
| Health check | `git lineage doctor` |
| Git hooks | `git lineage install-hook` — pre-commit ingest + post-commit linking |

Setup options:

```bash
./scripts/setup.sh --ingest /path/to/your-project   # also run first ingest
./scripts/setup.sh --with-mcp                       # install MCP server too
./scripts/setup.sh --force-hooks                    # overwrite existing hooks
```

### Prerequisites

| Requirement | Version | Notes |
|-------------|---------|-------|
| [Rust](https://rustlang.org/tools/install) | 1.86+ | Build from source (`rustup` recommended) |
| [Git](https://git-scm.com/) | 2.20+ | Notes support required (`refs/notes/*`) |
| [Node.js](https://nodejs.org/) | 20+ | VS Code extension build during setup |
| AI agent history | — | Cursor, Claude Code, or Codex (see [agent paths](#where-lineage-finds-agent-history)) |

Ensure `~/.cargo/bin` is on your `PATH`:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
git lineage --version
```

New repos need at least one commit before sessions can link to `HEAD`:

```bash
git add . && git commit -m "initial commit"
```

### VS Code extension (local dev)

The repo includes `.vscode/settings.json`, `launch.json`, and `tasks.json`. After setup:

1. Open the **lineage** repo in VS Code
2. Install recommended extensions when prompted
3. Press **F5** — launches the extension in this repository

`lineage.cliPath` in `.vscode/settings.json` points at `target/debug/git-lineage` (built during setup).

### Optional: MCP server

```bash
cargo install --path crates/lineage-mcp
# or: ./scripts/setup.sh --with-mcp
```

---

## Getting started

After [setup](#setup), ingest and explore lineage in your repository.

### 1. Ingest agent sessions

Ingest discovers transcripts on disk, normalizes them, applies policy, and writes git refs. By default, sessions are **linked to the current `HEAD` commit**.

```bash
# All supported agents (Cursor, Claude Code, Codex)
git lineage ingest --agent all

# Or one agent at a time
git lineage ingest --agent cursor
git lineage ingest --agent claude
git lineage ingest --agent codex
```

Useful flags:

| Flag | Purpose |
|------|---------|
| `--since 2026-01-01` | Only ingest sessions started on or after this date (RFC 3339 or `YYYY-MM-DD`) |
| `--incremental` | Skip sessions already ingested unless the source file changed |
| `--no-link-head` | Ingest without linking to the current commit (use with hooks) |

Example incremental ingest (typical for day-to-day use):

```bash
git lineage ingest --agent all --incremental
```

If no sessions are found, confirm agent history exists for this workspace — see [agent paths](#where-lineage-finds-agent-history) below.

### 2. Explore your lineage

```bash
# List ingested sessions (human-readable)
git lineage list

# JSON for scripts and tooling
git lineage list --json

# Show a full conversation (hydrates large LFS-backed content automatically)
git lineage show <session-id>

# Machine-readable session export
git lineage show <session-id> --json

# Which agent turn touched a specific line?
git lineage blame src/main.rs:42
git lineage blame src/main.rs:42 --json

# Full-text search over session content
git lineage search "authentication middleware"

# Sessions linked to a specific commit
git lineage list --commit <sha>
```

**Lineage blame** combines `git blame` with lineage notes: it finds the introducing commit, loads linked sessions and line objects, and returns matching turns (including confidence and content previews in JSON mode).

### 3. Share lineage with your team

Lineage data lives in git refs and notes — push them alongside your code:

```bash
# Push session refs, notes, and LFS transport refs
git lineage lfs push
git push origin refs/lineage/* refs/notes/lineage
```

On a fresh clone, teammates fetch lineage data before blaming or searching:

```bash
git fetch origin refs/lineage/* refs/notes/lineage
git lineage lfs fetch
git lineage doctor
```

Before sharing publicly, export with redaction to review what would leave the repo:

```bash
git lineage export --redact --format jsonl > lineage-export.jsonl
```

### 4. After a rebase

If commit SHAs changed, remap orphaned lineage notes to the rewritten history:

```bash
git lineage remap
```

This uses patch-id metadata stored on git notes to match rewritten commits where possible, then re-materializes line objects.

---

### Where Lineage finds agent history

Lineage scopes discovery to your **repository working directory**. It checks project-local paths first, then global agent config directories.

| Agent | Locations searched |
|-------|-------------------|
| **Cursor** | `.cursor/projects/*/agent-transcripts/`, `.cursor/agent-transcripts/`, `~/.cursor/projects/<project-key>/agent-transcripts/` |
| **Claude Code** | `.claude/projects/<encoded-path>/*.jsonl`, `~/.claude/projects/<encoded-path>/*.jsonl` |
| **Codex** | `.codex/sessions/`, `~/.codex/sessions/` |

Transcript files are JSONL. Lineage skips Claude snapshot/progress files and scopes sessions to the current workspace.

### Troubleshooting

| Problem | What to try |
|---------|-------------|
| `git: 'lineage' is not a git command` | Ensure `~/.cargo/bin` is on `PATH`; or use `git-lineage` directly |
| `discovered 0 … session(s)` | Run `git lineage doctor`; verify agent paths above; confirm you are in the repo root |
| `missing LFS` in doctor | Run `git lineage lfs fetch` after pulling refs from remote |
| Search returns nothing | Run `git lineage rebuild-index`, or search again (auto-rebuilds on empty results) |
| Blame shows no sessions | Run `git lineage ingest` then commit (or `install-hook`); line objects materialize at link time |
| Sessions contain secrets | Run `git lineage init-config`; use `export --redact` before sharing; review `refs/lineage/config` excludes |

Target a different repository path with `--repo /path/to/repo` on any command.

## Agent skill

Lineage ships an **agent skill** with the CLI so coding agents know how to retrieve engineering context — prior conversations, architecture decisions, and line-level provenance.

Install into your project (also run automatically by `./scripts/setup.sh`):

```bash
git lineage init-skill                    # all targets (default)
git lineage init-skill --target cursor  # Cursor only
git lineage init-skill --target claude --target codex
git lineage init-skill --target all --force
```

| Target | Install path | Docs |
|--------|--------------|------|
| `cursor` | `.cursor/skills/lineage/SKILL.md` | [Cursor Skills](https://cursor.com/docs/skills) |
| `claude` | `.claude/skills/lineage/SKILL.md` | [Claude Code](https://code.claude.com/docs/en/claude-directory) |
| `codex` / `agents` | `.agents/skills/lineage/SKILL.md` | [Codex customization](https://developers.openai.com/codex/concepts/customization) |
| `all` | All three (default when no `--target` is given) | — |

The same bundled skill ([`crates/lineage-cli/assets/skills/lineage/SKILL.md`](crates/lineage-cli/assets/skills/lineage/SKILL.md)) is copied verbatim to each path — it tells agents to run `git lineage search`, `blame`, and `show --json` before answering *why* code exists. Re-run with `--force` to refresh after upgrading the CLI.

## CLI reference

| Command | Description |
|---------|-------------|
| `git lineage doctor` | Check repo configuration and session integrity |
| `git lineage init-config` | Write default `refs/lineage/config` (policy, excludes, blob threshold) |
| `git lineage init-skill [--target cursor\|claude\|codex\|agents\|all] [--force]` | Install bundled agent skill (default: all targets) |
| `git lineage ingest [--agent cursor\|claude\|codex\|all] [--since DATE] [--incremental]` | Ingest agent history into git refs |
| `git lineage list [--commit SHA] [--json]` | List sessions, or sessions linked to a commit |
| `git lineage show <id> [--json]` | Display a session |
| `git lineage blame <path>[:line] [--json]` | Show which agent turn touched a line |
| `git lineage search <query>` | Full-text search over ingested sessions (auto-rebuilds stale index) |
| `git lineage rebuild-index` | Rebuild the local search index from git refs |
| `git lineage export [--redact] [--format json\|jsonl]` | Export sessions |
| `git lineage link <session-id> <commit-sha>` | Manually link a session to a commit (materializes line objects) |
| `git lineage materialize [--commit SHA] [--session ID]` | Build line objects from session artifacts at a commit |
| `git lineage remap` | Remap orphaned lineage notes after rebase (patch-id aware) |
| `git lineage lfs status` | Show referenced vs local LFS objects |
| `git lineage lfs push [--remote origin]` | Push LFS pointer and data refs |
| `git lineage lfs fetch [--remote origin]` | Fetch missing LFS objects from remote |
| `git lineage delete <session-id> [--purge-blobs]` | Remove session, line objects, notes entries; refcount-aware LFS purge |
| `git lineage gc` | Purge orphan line objects and unreferenced LFS blobs |
| `git lineage show <id> [--json] [--hydrate-images]` | Display session (optional inline image data URLs for UI) |
| `git lineage install-hook [--force]` | Install pre-commit and post-commit hooks |
| `git lineage uninstall-hook` | Remove lineage git hooks |

Use `--repo <path>` to target a repository other than the current directory.

## MCP server

Expose lineage data to AI tools via the [Model Context Protocol](https://modelcontextprotocol.io/):

```bash
LINEAGE_REPO=/path/to/your/repo lineage-mcp
```

| Tool | Description |
|------|-------------|
| `lineage_list_sessions` | List all ingested sessions |
| `lineage_get_session` | Fetch a session by ID (redacted by default) |
| `lineage_blame_line` | Get lineage for a file and line number |
| `lineage_search` | Full-text search over sessions |
| `lineage_doctor` | Check repository lineage health |
| `lineage_materialize` | Materialize line objects at HEAD or a commit |
| `lineage_rebuild_index` | Rebuild the local search index |
| `lineage_export` | Export sessions (optionally redacted) |
| `lineage_remap` | Remap lineage after rebase |

**Cursor configuration:**

```json
{
  "mcpServers": {
    "lineage": {
      "command": "lineage-mcp",
      "env": {
        "LINEAGE_REPO": "${workspaceFolder}"
      }
    }
  }
}
```

## VS Code extension

The extension in [extensions/vscode](extensions/vscode) provides a full lineage UI:

- **Activity bar panel** listing ingested sessions
- **Session timeline webview** with styled turn-by-turn history
- **Gutter decorations** and status bar hint for lines with lineage
- **Commands:** ingest, refresh, open session, show lineage for line, search, doctor, materialize, remap, init config, install git hooks
- **Session picker** when multiple sessions match a blamed line
- **Tool calls and artifacts** rendered in the session timeline webview

Run `./scripts/setup.sh` to compile the extension. Open the lineage repo in VS Code and press **F5** (see [VS Code extension (local dev)](#vs-code-extension-local-dev)).

Package a `.vsix` for side-loading:

```bash
cd extensions/vscode && npm run package
```

**Extension features:** hover blame (`lineage.hoverEnabled`), architecture summaries in the session panel, gutter decorations, and session timeline with tool calls and image artifacts.

## Git hooks

[Setup](#setup) installs hooks automatically. To manage them manually:

```bash
git lineage install-hook          # pre-commit ingest + post-commit linking
git lineage install-hook --force  # overwrite existing hooks
git lineage uninstall-hook
```

| Hook | Action |
|------|--------|
| `pre-commit` | Incremental ingest (`--no-link-head --incremental`) |
| `post-commit` | Link recently ingested sessions to the new commit |

## How it works

Lineage stores three kinds of data inside your git repository:

1. **Conversations** at `refs/lineage/sessions/<id>` (normalized agent sessions as JSON blobs)
2. **Line objects** at `refs/lineage/lines/<id>` (mappings from file lines to conversation turns)
3. **Git notes** at `refs/notes/lineage` (per-commit indexes linking sessions and line objects)

A manifest at `refs/lineage/index` lists all known sessions. Repository policy lives at `refs/lineage/config` (private session patterns, path/content excludes, large-blob threshold). Last ingest metadata is tracked at `refs/lineage/last-ingest` for smarter hook linking.

Search uses a local SQLite index at `.git/lineage/index.db`. Large artifact content above the configured threshold is stored in Git LFS layout (`.git/lfs/objects/`) by default, with pushable transport refs at `refs/lineage/lfs/` (pointers) and `refs/lineage/lfs-data/` (raw blobs for git transport). Legacy `cache` backend still uses `.git/lineage/blobs/`. Run `git lineage lfs push` before sharing large sessions.

**Ingest defaults** (configurable in `refs/lineage/config`):

| Setting | Default | Description |
|---------|---------|-------------|
| `ingest_only_code_sessions` | `true` | Skip sessions with no file edits or write tools |
| `commit_mapping` | `auto` | Multi-signal commit matching (`head`, `none`) |
| `lfs_transport` | `auto` | `git-lfs` CLI → HTTP batch API → ref transport (`gitcli`, `http`, `refs`) |

Images and diagrams from agent transcripts are stored as content-addressed LFS artifacts (`content_hash`, `mime_type`). Architecture summaries are generated at ingest and shown in the VS Code session panel.

**Schemas:**

| Schema | Description |
|--------|-------------|
| [conversation-schema-v0](specs/conversation-schema-v0.md) | Agent session and turn format |
| [line-object-schema-v0](specs/line-object-schema-v0.md) | File line to turn mapping |
| [git-notes-schema-v0](specs/git-notes-schema-v0.md) | Ref namespace and note layout |

## Architecture

```text
lineage/
├── lineage-core/       Domain types (Session, Turn, LineObject)
├── lineage-policy/     Redaction, excludes, private sessions
├── lineage-store/      Git blob and filesystem object storage
├── lineage-git/        Notes, refs, blame, commit mapping
├── lineage-agent/      Ingestion traits and pipeline
├── lineage-adapters/   Cursor, Claude, Codex source adapters
├── lineage-cli/        git-lineage binary
├── lineage-search/     Rebuildable SQLite FTS index
└── lineage-mcp/        MCP server
```

## Sharing lineage data

Lineage refs are ordinary git objects. Push them with your code (see [share lineage with your team](#3-share-lineage-with-your-team) for the full fetch/push workflow):

```bash
git lineage lfs push
git push origin refs/lineage/* refs/notes/lineage
```

Run `git lineage export --redact` before sharing if sessions may contain secrets.

## Development

```bash
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --all -- --check
cargo build --release
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines and [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for a deeper dive.

## Roadmap

- [x] Rebase-aware lineage remapping (`git lineage remap`)
- [x] Git LFS backend for large session content (`git lineage lfs push/fetch`)
- [x] Repo config ref (`refs/lineage/config`) and incremental ingest
- [x] Pre-commit hook for automatic ingest
- [x] Rich VS Code webview with session timeline and gutter decorations
- [x] Multi-signal commit mapping, code-only ingest default, session delete/purge
- [x] Image artifacts (content-addressed LFS), architecture summaries, hover blame
- [x] LFS HTTP batch API transport (alongside git-lfs CLI and ref fallback)
- [x] VS Code `.vsix` packaging (`npm run package`)
- [ ] Additional agent adapters

## License

MIT. See [LICENSE](LICENSE).

## Security

Report vulnerabilities per [SECURITY.md](SECURITY.md). Do not open public issues for security bugs.
