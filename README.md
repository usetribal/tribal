<p align="center">
  <script src="https://unpkg.com/@lottiefiles/lottie-player@latest/dist/lottie-player.js"></script>
  <lottie-player
    src="./assets/lineage-banner.lottie.json"
    background="transparent"
    speed="1"
    style="width: 100%; max-width: 1400px; height: auto;"
    loop
    autoplay>
  </lottie-player>
</p>

# Lineage

**Preserve the prompts, decisions, and context behind every commit.**

Lineage is agent-first engineering context memory for your codebase. It ingests sessions from Cursor, Claude Code, and Codex into your git repo — linked to the commits, files, and lines they touched — so your team and your agents can search past decisions, blame a line back to the prompt that wrote it, and pick up where a conversation left off. Stored as git refs and notes. No SaaS. No separate database.

**[Setup](#setup)** · **[Ingest & explore](#ingest-and-explore)** · [CLI & agent skill](docs/cli/README.md) · [MCP server](docs/mcp/README.md) · [VS Code extension](extensions/vscode/README.md)

## Why Lineage?

Git tells you what changed. It doesn't tell you why — or what your agents already discussed three sprints ago.

Lineage closes that gap. Engineering context travels with the repo: through `git push`, code review, onboarding, and the next agent session. Secrets are redacted before anything is written.

| | |
|---|---|
| **Agent-first** | Built for coding agents — ingest, search, blame, resume, and fork |
| **In-repo memory** | Context lives in git refs and notes, not a vendor silo |
| **Agent-agnostic** | Cursor, Claude Code, and Codex today; more adapters coming |
| **Queryable everywhere** | [CLI](docs/cli/README.md), [MCP](docs/mcp/README.md), and [VS Code](extensions/vscode/README.md) |

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
| [Rust](https://rust-lang.org/tools/install) | 1.86+ | Build from source (`rustup` recommended) |
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

### Using Lineage on another project

If the CLI is already installed globally, you only need:

```bash
cd /path/to/your-app
git lineage init-config
git lineage init-skill
git lineage install-hook
git lineage ingest --agent all --incremental
```

## Ingest and explore

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

## Where Lineage finds agent history

Lineage scopes discovery to your **repository working directory**. It checks project-local paths first, then global agent config directories.

| Agent | Locations searched |
|-------|-------------------|
| **Cursor** | `.cursor/projects/*/agent-transcripts/`, `.cursor/agent-transcripts/`, `~/.cursor/projects/<project-key>/agent-transcripts/` |
| **Claude Code** | `.claude/projects/<encoded-path>/*.jsonl`, `~/.claude/projects/<encoded-path>/*.jsonl` |
| **Codex** | `.codex/sessions/`, `~/.codex/sessions/` (filtered by session `cwd`) |

Transcript files are JSONL. Lineage skips Claude snapshot/progress files and scopes sessions to the current workspace.

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

## Troubleshooting

| Problem | What to try |
|---------|-------------|
| `git: 'lineage' is not a git command` | Ensure `~/.cargo/bin` is on `PATH`; or use `git-lineage` directly |
| `discovered 0 … session(s)` | Run `git lineage doctor`; verify [agent paths](#where-lineage-finds-agent-history); confirm you are in the repo root |
| `missing LFS` in doctor | Run `git lineage lfs fetch` after pulling refs from remote |
| Search returns nothing | Run `git lineage rebuild-index`, or search again (auto-rebuilds on empty results) |
| Blame shows no sessions | Run `git lineage ingest` then commit (or `install-hook`); line objects materialize at link time |
| Sessions contain secrets | Run `git lineage init-config`; use `export --redact` before sharing; review `refs/lineage/config` excludes |

Target a different repository path with `--repo /path/to/repo` on any command.

## How it works

Lineage stores three kinds of data inside your git repository:

1. **Conversations** at `refs/lineage/sessions/<id>` (normalized agent sessions as JSON blobs)
2. **Line objects** at `refs/lineage/lines/<id>` (mappings from file lines to conversation turns)
3. **Git notes** at `refs/notes/lineage` (per-commit indexes linking sessions and line objects)

A manifest at `refs/lineage/index` lists all known sessions. Repository policy lives at `refs/lineage/config`. Search uses a local SQLite index at `.git/lineage/index.db`. Large artifacts are stored in Git LFS by default.

**Schemas:** [conversation-schema-v0](specs/conversation-schema-v0.md) · [line-object-schema-v0](specs/line-object-schema-v0.md) · [git-notes-schema-v0](specs/git-notes-schema-v0.md)

## Development

```bash
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --all -- --check
cargo build --release
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines, [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for crate layout, and [AGENTS.md](AGENTS.md) for AI-assisted development.

## Roadmap

### Done

- [x] Rebase-aware lineage remapping (`git lineage remap`)
- [x] Git LFS backend for large session content (`git lineage lfs push/fetch`)
- [x] Repo config ref (`refs/lineage/config`) and incremental ingest
- [x] Pre-commit and post-commit hooks for automatic ingest and linking
- [x] One-command project setup (`./scripts/setup.sh`)
- [x] Bundled agent skill install (`git lineage init-skill`)
- [x] Session author attribution (`prompted_by_email` / `prompted_by_name`)
- [x] Multi-signal commit mapping, code-only ingest default, session delete/purge, and `git lineage gc`
- [x] Image artifacts (content-addressed LFS) and heuristic architecture summaries
- [x] LFS HTTP batch API transport (alongside git-lfs CLI and ref fallback)
- [x] VS Code extension — session timeline, gutter decorations, hover blame, resume/fork (Claude & Codex), `.vsix` packaging
- [x] MCP server — list, get, blame, search, doctor, materialize, rebuild-index, export, remap

### Planned

- [ ] Additional agent adapters
- [ ] MCP ingest, delete, and gc tools
- [ ] Cursor resume/fork support (pending stable agent CLI)
- [ ] LLM-generated architecture summaries
- [ ] Full GitHub-killer SaaS platform

## License

MIT. See [LICENSE](LICENSE).

## Security

Report vulnerabilities per [SECURITY.md](SECURITY.md). Do not open public issues for security bugs.
