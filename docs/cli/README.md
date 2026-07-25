# CLI reference

[← Documentation index](../README.md) · [Setup](../../README.md#setup)

`git lineage` is the primary interface for importing, querying, sharing, and maintaining agent provenance in a git repository.

## Install

```bash
cargo install --path crates/lineage-cli
# or: make setup
```

Ensure `~/.cargo/bin` is on your `PATH` so `git lineage` resolves. The binary name is `git-lineage`.

Target another repository:

```bash
git lineage --repo /path/to/repo <command>
```

---

## Project setup

```bash
git lineage init                    # interactive wizard
git lineage init --yes              # non-interactive defaults
git lineage init --no-import        # skip first import
git lineage init-config             # write default refs/lineage/config only
```

### Agent skill

Bundled skill teaches agents to use lineage features (search, blame, share, rebase, resume). Installed during init or manually:

```bash
git lineage init-skill
git lineage init-skill --target cursor
git lineage init-skill --target claude --target codex
git lineage init-skill --target all --force
```

| Target | Install path |
|--------|--------------|
| `cursor` | `.cursor/skills/lineage/SKILL.md` |
| `claude` | `.claude/skills/lineage/SKILL.md` |
| `codex` / `agents` | `.agents/skills/lineage/SKILL.md` |
| `all` | All three (default) |

Re-run with `--force` after upgrading the CLI to refresh skill content.

---

## Command reference

### Setup and health

| Command | Description |
|---------|-------------|
| `git lineage doctor [--json] [--section NAME] [--activity-limit N]` | Six-section health report (setup, capture, materialization, coverage, links, activity); see `specs/diagnostics-v0.md` |
| `git lineage init [options]` | Interactive setup: config, skill, hooks, optional import |
| `git lineage init-config` | Write default `refs/lineage/config` |
| `git lineage init-skill [options]` | Install bundled agent skill |

### Import

| Command | Description |
|---------|-------------|
| `git lineage import [options]` | Import agent history (`ingest` alias) |

Flags: `--agent cursor|claude|codex|all`, `--since DATE`, `--incremental`, `--no-link-head`.

See [Import](../import.md).

### Query

| Command | Description |
|---------|-------------|
| `git lineage list [--commit SHA] [--json]` | List sessions or sessions at commit |
| `git lineage show <id> [--json] [--hydrate-images]` | Show conversation |
| `git lineage blame <path>[:line] [--json]` | Lineage for a file line |
| `git lineage search <query>` | Full-text search (auto-rebuilds stale index) |
| `git lineage rebuild [--embed]` | Rebuild all derived state (links, line objects, index) from stored sessions; `--embed` also runs the dense-embedding backfill |
| `git lineage rebuild index` | Rebuild only the search index (`rebuild-index` is a deprecated alias) |
| `git lineage rebuild embeddings` | Rebuild only the dense embeddings — the semantic backfill (shows a per-session progress bar) |
| `git lineage export [--redact] [--format json\|jsonl]` | Export sessions |

See [Explore](../explore.md).

### Linking and history

| Command | Description |
|---------|-------------|
| `git lineage link <session-id> <commit-sha>` | Manually link session and materialize |
| `git lineage materialize [--commit SHA] [--session ID]` | Build line objects |
| `git lineage remap` | Recover lineage after rebase |

See [Rebase](../rebase.md) and [Maintenance](../maintenance.md).

### LFS

| Command | Description |
|---------|-------------|
| `git lineage lfs status` | Referenced vs local LFS objects |
| `git lineage lfs push [--remote origin]` | Push LFS refs |
| `git lineage lfs fetch [--remote origin]` | Fetch missing LFS objects |

See [LFS](../lfs.md).

### Context injection

| Command | Description |
|---------|-------------|
| `git lineage context hook claude` | Agent-hook endpoint (Claude Code PostToolUse on stdin); emits injection JSON or nothing |
| `git lineage context hook claude-session-start` | Agent-hook endpoint (Claude Code SessionStart); emits the traversal vocabulary once per session |
| `git lineage context log [--limit N]` | Show recorded context injections, newest last |
| `git lineage context install [--user]` | Wire the context hook per-repo or user-level (all repos) |
| `git lineage context uninstall [--user]` | Remove lineage context-hook wiring |
| `git lineage context query "<text>" [--timing]` | Retrieve past turns matching a free-text intent. With no leg/`--file` flag the query is **dispatched**: a named file that exists (in the corpus or the working tree) routes to the temporal plan on that anchor, everything else to the fused plan; `--timing` prints the chosen `route:` and per-stage timings |
| `git lineage context query "<text>" [--lexical\|--dense\|--fused]` | Force one leg, skipping the dispatcher — the flags exist to see a leg in isolation |
| `git lineage context query --file <path>[:<line>] ["text"]` | Force the line-anchored temporal plan (skips the dispatcher): the turns that authored the file/line, time-ordered (walked back through ancestry); with text, the text re-ranks those anchored turns |
| `git lineage context salience` | Report the corpus's turn-salience breakdown (what indexing keeps and drops) |
| `git lineage context chain <file>:<line>` | Print the temporal chain for a line — one hop per row (short sha, date, session, turn, confidence, or `DARK(kind)`) — resolved from the index (one live blame anchors HEAD, the rest are indexed reads) |

#### Traversal verbs

The moves a receiving agent makes when the injected evidence is close but not
right. Each takes a digest handle (`session#turn`) exactly as rendered, is
read-only and privacy-gated, and is bounded by `--limit`.

| Command | Repairs |
|---------|---------|
| `git lineage context search-within "<text>" --session <handle>...` | Right sessions, wrong turns — searches the text of named sessions in one call rather than N greps |
| `git lineage context around <handle> [--radius N]` | Right turn, missing its argument — the turns adjacent to it in its session |
| `git lineage context produced-by <handle>` | Right turn, want its outcome — the code that turn produced, as `file:lines` |
| `git lineage context sessions-for-commit <sha>` | Have a commit, want the reasoning — the sessions behind it (short shas resolve as elsewhere in git) |

The same four are MCP tools (`lineage_search_within`, `lineage_turns_around`,
`lineage_produced_by`, `lineage_sessions_for_commit`); paired registry tests
assert neither surface can gain or lose a verb without the other. MCP agents
discover them from `tools/list`; CLI sessions learn them from the `SessionStart`
hook that `context install` wires.

`context hook` is wired into the agent harness, not run by hand: when the agent
reads a file with provenance, a digest (attribution, line ranges, session
summary) is appended to the tool result — deterministically, without spending
an agent turn. It fails open: on any error, missing provenance, or private
sessions it prints nothing and exits 0. Every injection — and every
fired-but-silent outcome, with its reason — is recorded locally in the event
log at `.git/lineage/events.jsonl` (never synced); `context log` is the surface
to see exactly what your agent was told. See `specs/context-injection-v0.md`
and `specs/diagnostics-v0.md`.

`context query` routing precedence, highest first: `--file` forces the temporal
plan on that anchor; `--lexical`/`--dense`/`--fused` force one leg and skip the
dispatcher; with none of those, a rules-only dispatcher routes the free text. It
extracts path-shaped and identifier-shaped tokens (no model, microseconds) and
**hit-tests** each path against the corpus and the working tree — a path that
exists routes to the temporal plan, everything else (unknown path, bare
identifier, prose) to the fused plan, which degrades to honest-nothing. So the
router never answers wrongly by construction. Each routing decision
(`plan`/`anchor`/`signals`) is appended to the event log under `op:
"context_query"` — separate from `context log` (which shows `context_hook`
injections), so read it from `.git/lineage/events.jsonl` directly.

### Sync

| Command | Description |
|---------|-------------|
| `git lineage login --server URL` | Sign in to a Lineage server (browser device flow) |
| `git lineage sync [--server URL] [--token TOKEN] [--remote origin]` | Push redacted sessions to a Lineage server |

`login` prints a verification URL and code, waits for the browser approval, and
stores an opaque session handle in `~/.config/lineage/credentials.json` (0600;
`XDG_CONFIG_HOME` respected, `LINEAGE_CONFIG_DIR` overrides). The handle is the
durable credential — short-lived access tokens are minted from it per command,
and the identity-provider refresh token never leaves the server. Signing in to
the server's web app once beforehand is required (the server maps the login to
an existing account). If a stored login expires or is revoked, the next sync
says to run `login` again.

`sync` redacts and drops private sessions before anything crosses the wire,
assembles a `sync-batch-v0` (conversations with embedded turns, line objects,
decomposed commit links, and a blob manifest), uploads referenced blobs, and
POSTs the batch. The server resolves the repo from the `--remote` URL and root
commit; its returned id is cached in local git config (`lineage.serverRepoId`).
The data always lands in the workspace that owns the remote's namespace (the
`<owner>` in `github.com/<owner>/<name>`), never a different workspace you also
belong to. A sync into a namespace you have no membership in is rejected with
the owner named in the error — sign in to the server's web app again if your
memberships are stale, or check the `--remote` URL. `--server` defaults to the
server stored by `login`; the token comes from `--token`, then `LINEAGE_TOKEN`,
then the stored login. Implements
[sync-protocol-v0](../../specs/sync-protocol-v0.md).

### Hooks

| Command | Description |
|---------|-------------|
| `git lineage install-hook [--force]` | Install pre-commit and post-commit hooks |
| `git lineage uninstall-hook` | Remove lineage hooks |

See [Git hooks](../git-hooks.md).

### Cleanup

| Command | Description |
|---------|-------------|
| `git lineage delete <session-id> [--purge-blobs]` | Remove session and related objects |
| `git lineage gc` | Purge orphan line objects and unreferenced LFS blobs |

See [Maintenance](../maintenance.md) and [Privacy](../privacy.md).

---

## Session metadata

Import records `prompted_by_email` and `prompted_by_name` from git `user.email` / `user.name`. Sessions may include `vendor_session_id` for resume/fork, `parent_session_id`, `git_branch`, and `architecture_summary` metadata.

Schema details: [Schemas](../schemas.md).

## Related guides

- [Configuration](../configuration.md)
- [VS Code extension](../vscode.md)
- [MCP server](../mcp/README.md)
