# Agent guide

Tribal is a Rust monorepo for **git-native AI agent session history**: it imports sessions from Cursor, Claude Code, and Codex into git refs and notes, links them to commits and lines, and exposes that context through a CLI, MCP server, and VS Code extension. No external database — data lives in `refs/lineage/*` and `refs/notes/lineage`.

Read [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for crate boundaries and import flow before changing behavior — including its **Invariants**, which are rules a change must not break: the provenance graph is deterministic and backfillable, and rendering decides how to show, never what to show. If a change seems to need an exception, the model needs extending instead. **Release status: pre-release, unpublished — simplicity beats backwards compatibility; no data migrations or deprecation aliases required (re-import/rebuild is the upgrade path). The full policy lives in the enclosing workspace until publication.** `lineage-core` has no `git2`; git I/O stays in `lineage-git`.

## Setup

**Prerequisites:** Rust 1.86+ (MSRV), Git 2.20+, Node.js 20+ for extension work.

```bash
make setup                   # build CLI, compile extension, tribal init in this repo
make setup WITH_MCP=1        # also install lineage-mcp
make setup IMPORT=1          # run initial import after setup
```

Reinstall the CLI after CLI changes:

```bash
cargo install --path crates/lineage-cli
```

## Quality gates

For day-to-day work and before you commit:

```bash
make check                   # fmt, clippy, test, doc, typos, markdown, vscode lint
```

`make check` does not run coverage or MSRV — CI and PR review handle those.

## Releasing

Pushing a `v*` tag builds and publishes. `.github/workflows/release.yml` is
**generated** by [`dist`](https://github.com/axodotdev/cargo-dist) from
`dist-workspace.toml` — edit that file and regenerate, then commit both; a
hand-edit to the workflow is reverted by the next regeneration.

Regenerate from a copy of this directory rather than in place:

```bash
tmp="$(mktemp -d)"
tar --exclude=target --exclude=node_modules -cf - . | tar -xf - -C "$tmp"
(cd "$tmp" && git init -q && dist generate)
cp "$tmp/.github/workflows/release.yml" .github/workflows/release.yml
```

`dist` resolves the workspace by walking **up** from where it runs, so inside
the enclosing monorepo it finds that root and writes the workflow there instead
of here — landing a release file in the wrong repository, where nothing
attributes it to this change. From a standalone checkout it can be run directly.

Only `lineage-cli` is distributed: `dist` packages one app per crate, so adding
a second would mean a second set of installers. `lineage-mcp` is marked
`dist = false` and is installed with `cargo install --path crates/lineage-mcp`.
Installers are shell only for now; Homebrew and npm each need a registry
identity, which is a separate decision from being installable at all.

Before opening a PR (or when you change behavior that needs coverage proof):

```bash
make coverage                # >=80% line coverage (main.rs excluded)
make msrv                    # verify Rust 1.86 MSRV
```

| Task | Command |
|------|---------|
| Full local gate | `make check` |
| Format Rust | `make fmt` |
| Lint Rust | `make clippy` |
| Test workspace | `make test` |
| Coverage (PR) | `make coverage` |
| MSRV (PR) | `make msrv` |
| Format/lint extension | `make vscode-fmt` / `make vscode-lint` |
| Lint markdown | `make md-lint` |
| Spell check | `make typos` |
| Git hooks (fmt + lint on commit) | `make install-hooks` |
| Optional full hook suite | `pip install pre-commit` then `make pre-commit` |

## Developing the CLI

**Crate:** `crates/lineage-cli` · **Binary:** `tribal`

| File | Role |
|------|------|
| `src/main.rs` | `clap` commands and dispatch |
| `src/commands.rs` | Subcommand handlers |
| `src/ui.rs` | Human-facing stdout — commands print through this, not `println!` |
| `src/init_cmd.rs` | Interactive `init` wizard |
| `src/skill_cmd.rs` | `init-skill` (bundled skill install) |
| `src/hooks_cmd.rs` | Hook install/uninstall |
| `assets/hooks/` | Pre-commit / post-commit hook scripts |
| `assets/skills/` | Bundled end-user skills — `lineage/`, `share/` (installed by `init-skill`) |

Human output uses `ui` (scan list / detail / action / empty). `--json`, `--discover`, hook JSON, and `fork --brief` stay machine-shaped via `ui::json` / `ui::raw`. Enforced by clippy (`print_stdout`, `use_debug`) and `./scripts/check-cli-ui.sh`. Detail: `lineage-cli-command` skill.

```bash
cargo test -p lineage-cli
cargo test -p lineage-cli --test workflow
cargo test -p lineage-cli --test hooks_workflow
```

Integration tests use `tempfile` + `git init`. Hook tests need normal filesystem permissions. Document user-facing commands in [docs/cli/README.md](docs/cli/README.md). Update [CHANGELOG.md](CHANGELOG.md) under `[Unreleased]` for UX changes.

## Developing the MCP server

**Crate:** `crates/lineage-mcp` · **Entry:** `lineage_mcp::server::handle_request`

```bash
cargo test -p lineage-mcp
cargo test -p lineage-mcp --test server
cargo install --path crates/lineage-mcp   # or make setup WITH_MCP=1
```

Run locally:

```bash
LINEAGE_REPO=/path/to/repo lineage-mcp
```

Tools: `lineage_list_sessions`, `lineage_get_session`, `lineage_blame_line`, `lineage_search`, `lineage_doctor`, `lineage_materialize`, `lineage_rebuild_index`, `lineage_export`, `lineage_remap`, plus the traversal verbs `lineage_search_within`, `lineage_turns_around`, `lineage_produced_by`, `lineage_sessions_for_commit`, and `lineage_fork_brief`. See [docs/mcp/README.md](docs/mcp/README.md). Reads through `lineage-git` + `lineage-search` + `lineage-retrieval` — do not bypass policy/redaction. The traversal verbs are defined once in `lineage-retrieval::VERBS`, and continuing a session in `lineage-retrieval::CONTINUE_SESSION` beside it; both must stay equal across the CLI and MCP surfaces (registry tests enforce this).

## Developing the VS Code extension

**Path:** `extensions/vscode/` · shells out to `tribal` via `src/lineageClient.ts`

```bash
cd extensions/vscode
npm install
npm run check        # tsc + eslint + prettier
npm run compile      # or: make vscode from repo root
npm run package      # .vsix
```

**F5 debug:** open the tribal repo root → **Tribal Extension** in `.vscode/launch.json`. `lineage.cliPath` in `.vscode/settings.json` points at `target/debug/tribal` (built by `make setup`).

Key sources: `extension.ts`, `sessionsProvider.ts`, `sessionPanel.ts`, `lineageDecorator.ts`, `lineageHoverProvider.ts`, `agentActions.ts`.

After CLI JSON shape changes, update `src/types.ts` and extension commands in `package.json`.

## Developing library crates

| Area | Where to work | Tests |
|------|---------------|-------|
| Schema / types | `lineage-core` → `specs/schema/` (generated) | serde + schema snapshot + downstream crates |
| Git persistence | `lineage-git` | `crates/lineage-git/tests/` |
| Agent adapters | `lineage-adapters` | `tests/all_fixtures.rs`, `tests/fixtures/` |
| Import pipeline | `lineage-agent` | `tests/pipeline.rs` |
| Search | `lineage-search` | crate tests + CLI/MCP integration |

```bash
cargo test -p lineage-git
cargo test -p lineage-adapters
cargo test -p lineage-core
```

## Docs

- [docs/README.md](docs/README.md) — documentation index
- [README.md](README.md) — setup and troubleshooting
- [docs/import.md](docs/import.md), [docs/explore.md](docs/explore.md), [docs/share.md](docs/share.md) — day-to-day usage
- [CONTRIBUTING.md](CONTRIBUTING.md) — human contributor guide

## Rules

1. Minimize scope — focused diffs
2. Types are the contract source — regenerate `specs/schema/` and bindings after type changes (see `specs/decisions/0001-contract-bindings-pipeline.md`)
3. Policy before persist — no unredacted secrets in git objects
4. Graph before rendering — provenance edges are deterministic and backfillable; display surfaces format what they are given and never select or substitute content ([docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#invariants))
5. Add tests for behavior changes; run `make coverage` before a PR if logic changed
6. A change to state the CLI persists on a machine — config directory, git config keys,
   hooks, bundled skills, or the shape of a file it writes — registers a migration in
   `crates/lineage-cli/src/migrate.rs` in the same PR ([docs/migrations.md](docs/migrations.md))
7. Comment the *why* in plain language, next to the code; no "comment golf" and no narrating discarded approaches
