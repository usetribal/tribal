<p align="center">
  <img
    src="assets/lineage-banner.png"
    alt="Lineage: Preserve the prompts, decisions, and context behind every commit"
    width="1024"
  />
</p>

# Lineage

**Preserve the prompts, decisions, and context behind every commit.**

Lineage is agent-first engineering context memory for your codebase. It ingests sessions from Cursor, Claude Code, and Codex into your git repo, linked to the commits, files, and lines they touched, so your team and your agents can search past decisions, blame a line back to the prompt that wrote it, and pick up where a conversation left off. Stored as git refs and notes. No SaaS. No separate database.

**[Setup](#setup)** · [Ingest](docs/ingest.md) · [Explore](docs/explore.md) · [Share](docs/share.md) · [CLI & agent skill](docs/cli/README.md) · [MCP server](docs/mcp/README.md) · [VS Code extension](extensions/vscode/README.md)

[![Rust](https://img.shields.io/badge/rust-1.86%2B-orange?logo=rust&logoColor=white)](https://rust-lang.org/tools/install)
[![Git](https://img.shields.io/badge/git-2.20%2B-blue?logo=git&logoColor=white)](https://git-scm.com/)
[![Node.js](https://img.shields.io/badge/node.js-20%2B-green?logo=node.js&logoColor=white)](https://nodejs.org/)

## Why Lineage?

Git tells you what changed. It doesn't tell you why, or what your agents already discussed three sprints ago.

Lineage closes that gap. Engineering context travels with the repo: through `git push`, code review, onboarding, and the next agent session. Secrets are redacted before anything is written.

| | |
|---|---|
| **Agent-first** | Built for coding agents: ingest, search, blame, resume, and fork |
| **In-repo memory** | Context lives in git refs and notes, not a vendor silo |
| **Agent-agnostic** | Cursor, Claude Code, and Codex today; more adapters coming |
| **Queryable everywhere** | [CLI](docs/cli/README.md), [MCP](docs/mcp/README.md), and [VS Code](extensions/vscode/README.md) |

## Setup

From your project root, run these four commands:

```bash
cd /path/to/your-app
git lineage init-config
git lineage init-skill
git lineage install-hook
git lineage ingest --agent all --incremental
```

| Command | What it does |
|---------|--------------|
| `git lineage init-config` | Write `refs/lineage/config`, policy defaults, and `.gitattributes` |
| `git lineage init-skill` | Install the bundled agent skill for Cursor, Claude Code, and Codex |
| `git lineage install-hook` | Auto-ingest on commit and link sessions to new commits |
| `git lineage ingest --agent all --incremental` | Pull agent history into git refs (skips unchanged sessions) |

New repos need at least one commit before sessions can link to `HEAD`:

```bash
git add . && git commit -m "initial commit"
```

### Install the CLI (first time)

Clone lineage, install `git-lineage`, and build the VS Code extension:

```bash
git clone https://github.com/lineage-dev/lineage.git && cd lineage && make setup
```

Then run the four commands above in your project. `make setup` can also target a repo directly:

```bash
make setup REPO=/path/to/your-app
```

That runs the same `init-config`, `init-skill`, `install-hook`, and `doctor` steps via [`scripts/setup.sh`](scripts/setup.sh). See `make help` for all targets.

Ensure `~/.cargo/bin` is on your `PATH`:

```bash
export PATH="$HOME/.cargo/bin:$PATH"
git lineage --version
```

## Usage

| Guide | Description |
|-------|-------------|
| [Ingest](docs/ingest.md) | Pull agent sessions into git refs |
| [Explore](docs/explore.md) | List, show, blame, and search sessions |
| [Share](docs/share.md) | Push lineage refs with your code |
| [After a rebase](docs/rebase.md) | Remap lineage after history rewrite |
| [Agent paths](docs/agent-paths.md) | Where Cursor, Claude, and Codex store transcripts |
| [Git hooks](docs/git-hooks.md) | Automatic ingest and commit linking |
| [How it works](docs/how-it-works.md) | Git refs, notes, and line objects |

## Troubleshooting

| Problem | What to try |
|---------|-------------|
| `git: 'lineage' is not a git command` | Ensure `~/.cargo/bin` is on `PATH`; or use `git-lineage` directly |
| `discovered 0 … session(s)` | Run `git lineage doctor`; verify [agent paths](docs/agent-paths.md); confirm you are in the repo root |
| `missing LFS` in doctor | Run `git lineage lfs fetch` after pulling refs from remote |
| Search returns nothing | Run `git lineage rebuild-index`, or search again (auto-rebuilds on empty results) |
| Blame shows no sessions | Run `git lineage ingest` then commit (or `install-hook`); line objects materialize at link time |
| Sessions contain secrets | Run `git lineage init-config`; use `export --redact` before sharing; review `refs/lineage/config` excludes |

Target a different repository path with `--repo /path/to/repo` on any command.

## Development

```bash
make setup    # first-time install (see Setup above)
make check    # full contributor gate (fmt, clippy, test, coverage, extension lint)
make test
make coverage
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines, [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for crate layout, and [AGENTS.md](AGENTS.md) for AI-assisted development.

## Roadmap

### Done

- [x] Rebase-aware lineage remapping (`git lineage remap`)
- [x] Git LFS backend for large session content (`git lineage lfs push/fetch`)
- [x] Repo config ref (`refs/lineage/config`) and incremental ingest
- [x] Pre-commit and post-commit hooks for automatic ingest and linking
- [x] One-command project setup (`make setup`)
- [x] Bundled agent skill install (`git lineage init-skill`)
- [x] Session author attribution (`prompted_by_email` / `prompted_by_name`)
- [x] Multi-signal commit mapping, code-only ingest default, session delete/purge, and `git lineage gc`
- [x] Image artifacts (content-addressed LFS) and heuristic architecture summaries
- [x] LFS HTTP batch API transport (alongside git-lfs CLI and ref fallback)
- [x] VS Code extension: session timeline, gutter decorations, hover blame, resume/fork (Claude & Codex), `.vsix` packaging
- [x] MCP server: list, get, blame, search, doctor, materialize, rebuild-index, export, remap

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
