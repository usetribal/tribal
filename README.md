<p align="center">
  <img
    src="assets/lineage-banner.png"
    alt="Lineage: Preserve the prompts, decisions, and context behind every commit"
    width="1024"
  />
</p>

# Lineage

[![Rust](https://img.shields.io/badge/rust-1.86%2B-orange?logo=rust&logoColor=white)](https://rust-lang.org/tools/install)
[![Git](https://img.shields.io/badge/git-2.20%2B-blue?logo=git&logoColor=white)](https://git-scm.com/)
[![Node.js](https://img.shields.io/badge/node.js-20%2B-green?logo=node.js&logoColor=white)](https://nodejs.org/)

**Preserve the prompts, decisions, and context behind every commit.**

```bash
git lineage init
```

Lineage is agent-first engineering context memory for your codebase. It imports sessions from Cursor, Claude Code, and Codex into your git repo, linked to the commits, files, and lines they touched, so your team and your agents can search past decisions, blame a line back to the prompt that wrote it, and pick up where a conversation left off. Stored as git refs and notes.

## Why Lineage?

Git tells you what changed. It doesn't tell you why, or what your agents already discussed three sprints ago.

Lineage closes that gap with **local, in-repo memory**: agent context lives in your git repository as refs and notes, not in a vendor cloud. It travels with `git push`, code review, onboarding, and the next agent session. Secrets are redacted before anything is written.

| | |
|---|---|
| **Agent-first** | Built for coding agents: import, search, blame, resume, and fork |
| **Agent-agnostic** | Cursor, Claude Code, and Codex today; more adapters coming |
| **Queryable everywhere** | [CLI](docs/cli/README.md), [MCP](docs/mcp/README.md), and [VS Code](extensions/vscode/README.md) |

## Setup

From your project root, run the interactive wizard:

```bash
git lineage init
```

`git lineage init` walks you through:

| Step | What it does |
|------|--------------|
| Config | Write `refs/lineage/config`, policy defaults, and `.gitattributes` |
| Agent skill | Multiselect install: `.agents/` (Open Standard), `.claude/` (Claude Code), `.cursor/` (Cursor), all, or skip |
| Git hooks | Pre-commit incremental import and post-commit session linking |
| Import (optional) | Pull agent history into git refs (`--agent all --incremental`) |

### Install the CLI (first time)

Clone lineage, install `git-lineage`, and build the VS Code extension:

```bash
git clone https://github.com/lineage-dev/lineage.git
cd lineage
make setup
git lineage init
```

Ensure `~/.cargo/bin` is on your `PATH`:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
git lineage --version
```

### Development

```bash
make setup    # first-time install (see above)
make check    # full contributor gate (fmt, clippy, test, coverage, extension lint)
make test     # run workspace tests
make coverage # line coverage gate (>=80%)
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines, [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for crate layout, and [AGENTS.md](AGENTS.md) for AI-assisted development.

## Troubleshooting

| Problem | What to try |
|---------|-------------|
| `git: 'lineage' is not a git command` | Ensure `~/.cargo/bin` is on `PATH`; or use `git-lineage` directly |
| `discovered 0 … session(s)` | Run `git lineage doctor`; verify [agent paths](docs/agent-paths.md); confirm you are in the repo root |
| `missing LFS` in doctor | Run `git lineage lfs fetch` after pulling refs from remote |
| Search returns nothing | Run `git lineage rebuild-index`, or search again (auto-rebuilds on empty results) |
| Blame shows no sessions | Run `git lineage import` then commit (or `install-hook`); line objects materialize at link time |
| Sessions contain secrets | Run `git lineage init-config`; use `export --redact` before sharing; review `refs/lineage/config` excludes |

Target a different repository path with `--repo /path/to/repo` on any command.

## Roadmap

```text
[x] Rebase-aware lineage remapping (git lineage remap)
[x] Git LFS backend for large session content (git lineage lfs push/fetch)
[x] Repo config ref (refs/lineage/config) and incremental import
[x] Pre-commit and post-commit hooks for automatic import and linking
[x] One-command project setup (make setup)
[x] Bundled agent skill install (git lineage init-skill)
[x] Session author attribution (prompted_by_email / prompted_by_name)
[x] Multi-signal commit mapping, code-only import default, session delete/purge, and git lineage gc
[x] Image artifacts (content-addressed LFS) and heuristic architecture summaries
[x] LFS HTTP batch API transport (alongside git-lfs CLI and ref fallback)
[x] VS Code extension: session timeline, gutter decorations, hover blame, resume/fork (Claude & Codex), .vsix packaging
[x] MCP server: list, get, blame, search, doctor, materialize, rebuild-index, export, remap
[ ] Additional agent adapters
[ ] MCP import, delete, and gc tools
[ ] Cursor resume/fork support (pending stable agent CLI)
[ ] LLM-generated architecture summaries
[ ] Full GitHub-killer SaaS platform
```

## License

MIT. See [LICENSE](LICENSE).

## Security

Report vulnerabilities per [SECURITY.md](SECURITY.md). Do not open public issues for security bugs.
