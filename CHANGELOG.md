# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Diagnostics v0 (`specs/diagnostics-v0.md`): spec for a local event log
  (`.git/lineage/events.jsonl`, versioned per-operation entries) and a grown
  `git lineage doctor --json` contract (setup, capture, materialization, links,
  activity sections) that external developer tooling will consume; narrative
  contract only, not part of the schema/bindings pipeline
- Context oracle (`specs/context-injection-v0.md`): `git lineage context hook claude` — a Claude Code PostToolUse hook endpoint that deterministically appends provenance digests (attribution, line ranges, session summary, graded strength) to file-read results, backed by a new `lineage-oracle` crate (transport-neutral `Retriever` trait, tiered local retrieval over line objects and files-touched sessions, content-hash-keyed cache with negative caching); private sessions and forks of private sessions are never injected, and every injection is recorded in `.git/lineage/context-log.jsonl`, viewable via `git lineage context log`

- `git lineage login --server URL` — sign in to a Lineage server via the browser device flow; stores an opaque session handle in `~/.config/lineage/credentials.json` (0600) and makes it the default server. `git lineage sync` now works without flags after a login: `--server` falls back to the stored default, and the bearer token falls back from `--token`/`LINEAGE_TOKEN` to a short-lived access token minted from the stored login; a 401 on that mint means the stored login was revoked or expired and `login` must be re-run
- Generated contract bindings: `lineage-core` types emit JSON Schema (`specs/schema/`, snapshot-tested) and TypeScript zod bindings (`contracts/ts`, `@lineage/contracts`), each hop drift-checked; see `specs/decisions/0001-contract-bindings-pipeline.md`
- Sync protocol v0 (`specs/sync-protocol-v0.md`): the local↔server wire protocol — object mapping, ULID identity + write rules, content hashing, blob transfer, privacy, repo binding — with wire types (`SyncBatch`, `SyncResponse`) in `lineage-core` and generated bindings
- `git lineage sync --server URL [--token TOKEN] [--remote origin]` — push redacted sessions to a Lineage server: assembles a `sync-batch-v0` (conversations, line objects, decomposed commit links, blob manifest), uploads referenced blobs via `PUT /v0/blobs/{sha256}`, POSTs the batch, and caches the server-issued repo id in local git config; token falls back to `LINEAGE_TOKEN`; implements `specs/sync-protocol-v0.md`
- `git lineage init` — interactive setup wizard: writes config, multiselect skill install (`.agents/`, `.claude/`, `.cursor/`, all, or skip), installs hooks (with overwrite prompt), optional import with side-effect summary; `git lineage init --yes` for scripts and `make setup`
- Session author metadata — `prompted_by_email` and `prompted_by_name` stamped at import from git config; preserved on re-import for team attribution
- `git lineage init-skill` — installs bundled agent skill for Cursor (`.cursor/skills/`), Claude Code (`.claude/skills/`), and Codex (`.agents/skills/`); `--target` multiselect, `all` (default), or `none`
- `git lineage list --json` — session summaries include `git_branch`, `parent_session_id`, `is_sidechain`, `vendor_session_id`, `prompted_by_email`, and `prompted_by_name`
- README banner image (`assets/lineage-banner.png`) and prerequisite badges (Rust, Git, Node.js)

#### Developer experience

- `scripts/setup.sh` and `make setup` — one-command install (CLI, VS Code extension, `git lineage init --yes`)
- Single git repo at project root (no bundled `examples/` demo)
- `.vscode/` workspace config for extension local dev (F5 launches in this repo)
- `Makefile` and `scripts/check.sh` for local gates (`make check`, `make coverage`, `make msrv`, etc.)
- Pre-commit hooks (`.pre-commit-config.yaml`): rustfmt, clippy, typos, markdownlint, VS Code extension check
- MSRV enforcement: `rust-version = "1.86"` in workspace `Cargo.toml` and CI MSRV job
- Project-level `clippy.toml` aligned with MSRV
- CI: `cargo doc`, markdownlint, typos, MSRV (1.86)
- VS Code extension: ESLint + Prettier (`npm run check`)
- `.github/CODEOWNERS`, root `AGENTS.md` for AI-assisted development
- Project agent skills in `.cursor/skills/` (contribute, adapters, schema, git, CLI, VS Code, MCP, release)
- `typos.toml` and `.markdownlint.yaml` for docs quality
- `CONTRIBUTING.md` and PR template synced with CI (`--all-targets`, coverage script)

#### Testing & CI

- Workspace integration and unit tests across CLI, MCP, git, search, agent, and store crates
- `scripts/coverage.sh` and CI coverage job enforcing **≥80% line coverage** (llvm-cov JSON totals; `main.rs` entrypoints excluded)
- Bug fix: `purge_orphans` / `referenced_line_object_ids` now read git notes via `find_note` instead of treating note commit OIDs as blobs

#### CLI

- `git lineage delete <session-id> [--purge-blobs]` — remove session ref, line objects, and note entries; optional refcount-aware LFS purge
- `git lineage gc` — purge orphan line objects and unreferenced LFS blobs/transport refs
- `git lineage show <id> [--json] [--hydrate-images]` — optional inline image `preview_data_url` for UI export
- `git lineage lfs status|push|fetch` — LFS object transport across remotes (git-lfs CLI, HTTP batch, or ref fallback)
- `git lineage init-config` — write default `refs/lineage/config`
- `git lineage remap` — patch-id aware rebase recovery
- `git lineage materialize` — build line objects from session artifacts at a commit
- `git lineage install-hook` / `uninstall-hook` — pre-commit incremental import and post-commit linking
- `git lineage export --format jsonl`, `list --json`, `blame --json` with enriched `matches` in JSON output
- Incremental import (`--incremental`), time filter (`--since`), and `refs/lineage/last-import` tracking

#### Repository config (`refs/lineage/config`)

- `import_only_code_sessions` (default `true`) — skip sessions with no file edits or write tools
- `commit_mapping` — `auto` (multi-signal), `head`, or `none`
- `lfs_transport` — `auto`, `gitcli`, `http`, or `refs`
- `large_blob_backend` (`lfs` | `cache`) and `large_blob_threshold_bytes`
- Private session patterns, path/content excludes, export redaction defaults

#### Storage & LFS

- Git LFS backend for large session content (`.git/lfs/objects/`, `refs/lineage/lfs/*`, `refs/lineage/lfs-data/*`)
- Legacy blob cache (`.git/lineage/blobs/`) when `large_blob_backend: cache`
- Git LFS HTTP batch API transport (`lfs_transport: http`)
- Git LFS worktree integration: `.gitattributes` for `.lineage/media/**` and pointer files on media import
- Blob hydration on read for large turn content (`show`, `export`, search rebuild)
- Architecture summaries generated at import (`metadata.architecture_summary`)

#### Commit mapping & lineage graph

- Multi-signal commit mapping at import: time overlap, file overlap, branch ancestry, code-tool signal, patch-id match, diff-similarity scoring
- Line-level blame via artifact resolve hints (`old_string`, `full_file`, citations, diff hunks)
- Patch-id stored on git notes; `git lineage remap` matches rewritten commits

#### Images & media artifacts

- First-class image/diagram/screenshot artifacts with `content_hash`, `mime_type`, and LFS storage
- Adapter detection: image content blocks, `GenerateImage` tool, markdown image links, embedded data URLs
- `preview_data_url` on artifacts for hydrated UI export (ephemeral; not persisted to git refs)

#### Agent adapters

- Full Codex adapter for `~/.codex/sessions/**/rollout-*.jsonl`
- Cursor: nested `agent-transcripts/<id>/<id>.jsonl`, project-scoped discovery, `tool_use` parsing
- Claude: `~/.claude/projects/<encoded-path>/` layout, skips snapshots/progress, `tool_use`/`tool_result`
- Codex: modern `{timestamp,type,payload}` rollout format
- Model metadata: per-turn `model`, session `models_used`, agent-specific version/branch fields

#### VS Code extension

- Activity bar session tree, timeline webview, gutter decorations, status bar hint
- Minimal hover blame — model, prompter (`prompted_by_email` / `prompted_by_name`), and icon-only actions (`lineage.hoverEnabled`)
- Hover actions: **View Conversation** (`$(open-preview)`), **Fork Conversation** (`$(git-branch)`), **Resume Conversation** (`$(run)` for Claude Code and Codex)
- **Resume** runs `claude --resume` or `codex resume` in an integrated terminal (no raw CLI hints in the UI)
- **Fork** copies the agent transcript to `.lineage/forks/` then branches in the agent (Claude `--fork-session`; original transcript untouched)
- Session panel chips: prompter, model, git branch, vendor session id, parent session link, linked commits
- Extension activates without an open folder; commands register immediately with a folder-open prompt when needed
- F5 launch config: **Lineage Extension (other project)** — prompt for target repo path
- Architecture summary block in session panel
- Inline image previews for hydrated media artifacts
- **Delete Session** command (context menu on session tree)
- Commands: import, refresh, view conversation, resume, fork, show lineage, view commit, search, doctor, materialize, remap, init config, install hooks
- Session picker when multiple sessions match a blamed line
- `.vsix` packaging via `npm run package`

#### MCP server

- Tools: `lineage_list_sessions`, `lineage_get_session`, `lineage_blame_line`, `lineage_search`, `lineage_doctor`, `lineage_materialize`, `lineage_rebuild_index`, `lineage_export`, `lineage_remap`

#### Tests & CI

- Integration tests: persist/read, LFS hydrate, line resolve, commit mapping, delete
- Adapter fixture tests (Cursor, Claude, Codex)
- Policy config tests for private sessions and repo config mapping

### Changed

- Import-time secret redaction now uses vendored [gitleaks](https://github.com/gitleaks/gitleaks) rules (regex, entropy, allowlists) instead of broad `api_key`/`env_var` regexes — fewer false positives on agent prose while keeping high-confidence secret detection
- Gitleaks config parser reads `[[rules.allowlists]]` (plural) and triple-quoted path arrays; conformance fixtures cover identification edge cases
- README repositioned as agent-first engineering context memory; primary setup is `git lineage init` (replaces four-command flow)
- Usage guides split into [docs/import.md](docs/import.md), [docs/explore.md](docs/explore.md), [docs/share.md](docs/share.md), [docs/rebase.md](docs/rebase.md), [docs/agent-paths.md](docs/agent-paths.md), [docs/git-hooks.md](docs/git-hooks.md), and [docs/how-it-works.md](docs/how-it-works.md)
- Component docs in [docs/cli/README.md](docs/cli/README.md), [docs/mcp/README.md](docs/mcp/README.md), and [extensions/vscode/README.md](extensions/vscode/README.md)
- Bundled agent skill ([`crates/lineage-cli/assets/skills/lineage/SKILL.md`](crates/lineage-cli/assets/skills/lineage/SKILL.md)) refocused on lineage features (search, blame, share, rebase, hooks, resume/fork, privacy); setup delegated to `git lineage init`
- `make setup` / [`scripts/setup.sh`](scripts/setup.sh) run `git lineage init --yes` instead of separate `init-config`, `init-skill`, and `install-hook` calls; `Makefile` accepts `REPO`, `IMPORT`, `WITH_MCP`, and `FORCE_HOOKS` flags
- README roadmap refreshed (done vs planned items)
- Renamed CLI command `git lineage ingest` → `git lineage import` (`ingest` kept as alias); init flag `--no-import`; docs at [docs/import.md](docs/import.md); config `import_only_code_sessions`; tracking ref `refs/lineage/last-import`

- Conversation schema docs — document `prompted_by_email` and `prompted_by_name` author metadata (`specs/conversation-schema-v0.md`)
- Default import skips sessions that did not modify code (`import_only_code_sessions: true`)
- Import uses multi-signal commit mapping by default (`commit_mapping: auto`) instead of always linking to HEAD
- Pre-commit hook uses incremental import; post-commit links only recently imported sessions
- Search auto-rebuilds index on empty results; FTS triggers dedupe duplicate rows on re-index
- Hooks no longer swallow stderr on failure (errors surface to the terminal)
- Blame uses repo-relative paths (fixes git2 blame resolution)
- `git lineage list` and `show` surface model info in human-readable output
- `init-config` ensures `.gitattributes` for `.lineage/media/**`
- Doctor checks for missing LFS blobs and config ref

### Removed

- `examples/demo-repo` bundled demo; use `tests/fixtures/` for samples and `./scripts/setup.sh` on your own project

### Fixed

- Import no longer panics when resolving line numbers for edits whose matched text ends in a multibyte character (e.g. an em dash in transcript content): `line_number_at` floors the byte offset to a char boundary before slicing
- `git lineage delete` now overwrites git notes instead of merging, so session and line-object IDs are actually removed from commit notes

### Known limitations

- Cursor transcripts do not include tool results (per Cursor storage format)
- MCP server uses a minimal JSON-RPC stdio loop (no delete/gc/import tools yet)
- LFS HTTP batch requires HTTPS remotes with standard `/info/lfs` endpoints and git credentials
- `.lineage/media/**` pointers require `git lfs install` locally for full smudge/clean filter behavior
- Architecture summaries are heuristic (first user message + files + model), not LLM-generated

## [0.1.0] - 2026-06-06

### Added

- Initial open-source release of the Lineage workspace and crates
- `git-lineage` CLI with `doctor`, `import`, `list`, `show`, `blame`, `search`, `export`, `link`, and `rebuild-index`
- Agent adapters for Cursor, Claude Code, and Codex
- Git-native storage via `refs/lineage/*` and `refs/notes/lineage`
- Policy engine with API key redaction and path excludes
- Rebuildable SQLite full-text search index
- MCP server (`lineage-mcp`) with list, get, blame, and search tools
- VS Code extension skeleton
- Schema specifications v0 (conversation, line-object, git-notes)
- CI workflow (test, clippy, fmt)

[Unreleased]: https://github.com/lineage-dev/lineage/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/lineage-dev/lineage/releases/tag/v0.1.0
