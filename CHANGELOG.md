# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `git lineage resume <session-id>` — reopen one of your own sessions in the agent that produced it, printing the command rather than running it (same reasoning as `fork`). A separate verb rather than a flag on `fork` because the two do genuinely different things and the distinction is the thing a user has to get right: **fork writes a new session out of stored turns** and therefore works for a teammate's session; **resume writes nothing**, naming a session the harness already holds, and therefore only works for one this machine imported. A session with no vendor id — a teammate's, pulled from a server — is refused with a pointer at `fork` rather than a generic failure, because that is the user's actual next move. Claude Code and Codex resume; Cursor declines by name
- Adapter resume capability (`lineage-agent::SessionResumer`): adapters return the whole invocation — the command string and, when the harness resolves a session relative to a directory, which directory. Deliberately **separate from `TranscriptWriter`** rather than one "can be continued" flag: Codex can reopen a session it already holds but cannot be handed a written one, so collapsing the two would have made Codex sessions un-resumable to match their un-forkability. Claude names a cwd (its project key is derived from the launch directory); Codex does not (it keys rollouts by id under one state directory), and the CLI prints "run this from …" only when the adapter says a directory matters

- `git lineage pull [--server URL] [--token TOKEN] [--remote origin] [--dry-run]` — bring teammates' synced sessions down into this repository's lineage refs, so `list`, `show`, `search`, and `fork` see them the same as sessions imported here. Until now lineage could only push: a session Alice synced was visible in the web app but unreachable from Bob's checkout, which is the half that makes forking a teammate's session actually multiplayer. **Pull is not sync with the arrows reversed** — push merges into an authority, pull merges into a cache — so the rules mirror the push write rules rather than inverting them. Pull **never deletes**: a session the server does not mention is left exactly as it is, because this machine may hold sessions it has never pushed and reconciling by deletion would destroy unsynced local work. Container fields merge **monotonically** (`commit_shas` set union, `ended_at` max, turn set grow-only, `metadata` first-write-wins per key), so a local copy that is *ahead* of the server's is never regressed by pulling, and Alice-push-then-Bob-pull converges to the same state as the reverse. Turns are immutable and content-addressed, so a second identical pull writes nothing. The cursor is a **content digest, not a sequence number or an `updated_at` watermark**: the client sends `{conversationId, turnCount, endedAt}` per stored session to `POST /v0/pull/negotiate` and a second `POST /v0/pull/fetch` retrieves what differs — no gapless counter to get wrong under concurrency, no clock to trust, and it widens to artifacts and line objects later rather than being redesigned. `--dry-run` reports what would arrive and writes nothing; `--server`, `--token`, and `--remote` resolve exactly as `sync`'s do. A commit sha naming history this checkout has not fetched is dropped rather than failing the pull — the union merge picks it up after a `git fetch`

- `Conversation.pull_origin` (`conversation-schema-v0`) — where a session came from when it was not imported on this machine: server, optional tenant, when, and the lineage version that wrote the marker. A typed field rather than a metadata key for the same reason as `fork_origin`: `metadata` merges first-write-wins per key, which silently drops a provenance edge that cannot be recomputed, and this one is read on the **push** path too, where a filter deciding what to upload should not key off a string every caller has to remember. Stamped **first-write-wins** — a session already here keeps the marker it had, because where it first came from does not change when the same server serves it again. A *fork of* a pulled session carries `fork_origin` and no `pull_origin`, so it is Bob's and still pushes

- Traversal verbs are now recorded in the event log (`context_traversal`, `specs/diagnostics-v0.md`). The four verbs were the only agent-facing lineage operations leaving no trace: the log could show that context was *injected* but never that the agent went on to follow it, so `git lineage context log` and doctor's activity section under-reported what lineage did in a session, and the handle round-trip — the one mechanism that makes agent uptake observable rather than inferred — was invisible after the fact. Each run appends `relation` (the abstract name from `lineage-retrieval::VERBS`, never a CLI or MCP spelling, so a shelled and an MCP-issued traversal log identically), the handle as given, the session ids it resolved to, and the result count. `results: 0` is recorded rather than skipped — an honest-nothing traversal is still one the agent chose to make. `context_hook` gains `handles`: the digest's rendered handles in selection order, because `session_ids` alone cannot say which turn of a session was offered, and a traversal names a turn

- `git lineage fork <session-id> [--dry-run]` — pick up a teammate's agent session and continue it in your own harness. Resolves the session from lineage's own refs, **never** from `metadata["source"]`: that field is an absolute path on the importing machine, and gating on it is exactly what made the extension's fork action un-shareable (it disabled itself on any machine that had not done the import, even though the mechanism underneath would have worked). Renders the conversation through the adapter's transcript writer, records the fork edge, and **prints the command to run rather than spawning a terminal** — so a human can read it before acting and an agent that invoked the CLI can act on it directly. The command string, its flags, and the directory it must run from are adapter-supplied; the CLI names no vendor path or flag (`ARCHITECTURE.md` invariant 4). Output leads with whose session it was, when, what they asked for, what it changed, and which commits it reached, because deciding whether to continue someone's work needs the work to be recognisable first. `--dry-run` shows the target path and the command without writing or recording anything. Claude Code only; Codex and Cursor decline by name

- `Conversation.fork_origin` (`conversation-schema-v0`) — the fork edge as a typed field: source session id, the vendor id minted for the copy, when, the lineage version, plus optional source tenant/repo for a fork that crossed a server. It is a field rather than a flag beside `parent_session_id` because that field is already overloaded — Claude sidechains and subagents set it for harness-spawned branches, and `git lineage list` was reporting *any* session with a parent as a sidechain. `parent_session_id` is still set on a fork, so the privacy fork-chain gate needs no special case and a fork of a private session is refused exactly as a sidechain of one is. **Attribution:** post-fork lines are the forker's — the source session is an ancestor edge and never a co-author, and line objects materialized from a fork bind to the fork's own conversation id. `git lineage list --json` gains `is_fork` and no longer conflates the two; `git lineage show` names the source session

- Adapter transcript-writer capability (`lineage-agent::TranscriptWriter`): adapters can now render a `Conversation` back into a vendor-native transcript plus the handle the harness resolves it by, which is the mechanism a session fork needs. Claude Code is the only implementation — it writes chained-`parentUuid` JSONL to `~/.claude/projects/<project-key>/<session-id>.jsonl` under a **freshly minted** session id (reusing the source id would collide if two users share a machine); Cursor and Codex decline explicitly by name rather than silently no-opping. Tool activity is flattened into prose rather than replayed as `tool_use`/`tool_result` blocks: a transcript claiming live tool handles that no longer exist can convince the resuming model it already made edits it did not make. No user-facing command consumes this yet. `ARCHITECTURE.md` gains a fourth invariant — harness-specific knowledge (file formats, state directories, CLI flags, id conventions) lives only in `lineage-adapters` — which this capability exists to satisfy

- `coverage` section in `git lineage doctor` (always on, no flag) — where the materialization funnel answers "is capture healthy?", coverage answers "how much of the codebase can provenance actually explain?". Reports commit note coverage, **reach** (tracked files with any line object), **depth** (mean per-file coverage within covered files), total line coverage, and a per-file bucket histogram. Reach and depth stay separate figures because they mean different things — a line no captured session ever edited is legitimately dark, whereas a touched file with no line objects is a defect — and the histogram is what makes the distribution's shape visible where a single blended percentage hides it. Sourced from the `line_objects` index mirror plus the `HEAD` tree, so it costs ~0.1s on a ~12k-object corpus (walking `refs/lineage/lines/*` with `cat-file` would cost ~40s); spans are merged and clamped to each file's current length, so overlapping objects are counted once and coverage cannot exceed 100%. Also in `--json` and the MCP `lineage_doctor` payload (`specs/diagnostics-v0.md`)
- Agent traversal interface: a receiving agent handed an injected digest can now navigate the provenance graph itself instead of being stuck with whatever ≤3 turns a canned plan chose. Four verbs — `search-within` (scoped FTS over named sessions, one call instead of N greps), `around` (the turns adjacent to a turn in its session), `produced-by` (the code a turn produced), `sessions-for-commit` (the sessions behind a commit) — each derived from a way the injected set can be *wrong*, each read-only, privacy-gated, and bounded. They are one closed vocabulary (`lineage-retrieval::VERBS`) exposed on **both** consumers: `git lineage context <verb>` and MCP tools (`lineage_search_within`, `lineage_turns_around`, `lineage_produced_by`, `lineage_sessions_for_commit`), with paired registry tests asserting the CLI subcommand set and the MCP tool set each equal the registry in both directions — so no capability can exist for one consumer and not the other. `lineage-mcp` gains a direct `lineage-retrieval` dependency rather than reaching through the binary `lineage-cli` crate. Verbs take a digest handle (`session#turn`) exactly as rendered
- `Gated<T>` privacy type (`lineage-retrieval::session`): the privacy gate used to be structurally safe only because `materialize_turns` was its single exit, and traversal adds an exit per verb. Turn text is now a distinct type that only `SessionGate` can construct, so a primitive that returns text without running the gate does not compile — proven by a `compile_fail` doctest. A private session (or a fork of one) is refused whole, including by verbs that return no turn text
- `session_commits` index table mirroring `Conversation.commit_shas` — populated on index, wiped on rebuild, and maintained incrementally by the post-commit hook and `git lineage link` (which write the edge to the conversation ref without re-indexing). Makes `sessions-for-commit` one indexed lookup; short shas resolve as they do elsewhere in git
- `SessionStart` hook: `git lineage context install` now writes a second hook group that teaches the traversal vocabulary once per session (payload shape probed live 2026-07-25). MCP consumers get verb discovery free from `tools/list`; a CLI session has no such channel, so without this it would have capability it could not find. States capability, never instructs. Fails open — a payload that does not parse still emits the vocabulary. Install backfills the new group into settings wired by an older binary, and uninstall removes both
- Digest reshape: entries carry an addressable `session#turn` handle and state the edges their node has (nouns, not commands), with the verb vocabulary named once in a shared footer rather than per entry — three entries × three affordances was over 13% of the intent budget spent on navigation. Per-trigger budgets replace the shared one: the file-keyed `PostToolUse`/`Read` trigger fires constantly mid-task and gets ~200 tokens, one entry, no footer; the `UserPromptSubmit` intent trigger fires at a decision point and keeps 1,024 tokens, three entries, and the footer
- Rules-only plan dispatcher (`lineage-retrieval::route`): a plain `git lineage context query "<text>"` (no `--file`, no leg flag) now routes through a deterministic, model-free dispatcher that selects a canned plan. It extracts path-shaped (`/` or a known source extension) and identifier-shaped (snake/kebab/Camel/dotted) tokens from the text, then **hit-tests** each path candidate against the corpus (`line_objects`/`session_files` indexed lookups) and the working tree. A path that hit-tests routes to the line-anchored **temporal** plan on that anchor (behaving exactly as `--file` would); everything else — an unknown path, a bare identifier, pure prose — stays on the **fused** plan, which degrades to honest-nothing. Identifiers never anchor a walk (no line to walk; the fused leg's tokenizer already ranks them). The decision (`plan`/`anchor`/`signals`) is appended to the event log under `context_query` (best-effort), and printed as a `route:` line under `--timing`. Precedence: `--file` forces temporal, `--lexical`/`--dense`/`--fused` force one leg and skip the dispatcher, no flag dispatches
- Retrieval primitives + thin plan runner (`lineage-retrieval`): the intent path is now composed from small, independently-tested primitives — `TurnRef`/`LineRef` node types, `time_search` (ancestry walk), `turns_from_line_objects` (file[:line] → attributing turns), `turns_to_sessions` (dedupe upward keeping best rank), and `materialize_turns` (turn refs → verbatim `Evidence`, with the privacy filter pinned inside so no plan can emit unfiltered evidence). A `PlanRun` threads one deadline through named stages, records per-stage elapsed, and flags `truncated` on overrun. Two canned plans run through it: the **fused-salient-turn** plan (FTS ∥ dense → RRF → ≤3 verbatim turns) is the default `context query` path, and the new **line-anchored temporal** plan resolves a file[:line] anchor through `line_objects`/ancestry to time-ordered turns. Both emit the unchanged `retrieval-v0` shape, so cache/selection/event-log are untouched. Digest blocks now carry **affordance pointers** as runnable commands (`git lineage show <session>`, and `git lineage context chain <file>:<line>` for line-anchored evidence), omitting relations the CLI cannot honour (spec: context-injection-v0 verbatim-turn digest)
- `git lineage context query --file <path>[:<line>] ["text"]` — the line-anchored temporal plan. Alone, it returns the turns that authored the anchor's file/line (time-ordered, walked back through ancestry); with text, the text re-ranks those anchored turns by FTS score (a cheap filter, not a second retrieval). `git lineage context query [...] --timing` prints the per-stage plan timings. On the dev corpus: `--file README.md:40` resolves in ~12ms (turns_from_line_objects/time_search ~0ms, materialize ~12ms), a no-lineage anchor answers honest-nothing in ~18ms; the fused plan's single retrieve stage is ~250ms (dominated by the dense leg's brute-force cosine)
- Precomputed temporal chaining: two derived index tables (`line_objects`, `line_ancestry` in `.git/lineage/index.db`) turn "walk this line back through the turns that touched it" from live blame hops (measured ~0.5–1.4s, unbounded on bulk-import notes) into indexed lookups. `line_objects` mirrors `refs/lineage/lines/*` with commit time for temporal ordering; `line_ancestry` records blame-hunk-grain edges keyed by child position (not line-object id), so a chain continues through **dark** commits (no lineage note) and diverges at sub-ranges. Both are populated in `rebuild index` / `rebuild` (full recompute, wipe-and-rebuild, with a `chaining` progress bar), incremental `import`, and the post-commit link hook; unreachable-commit rows are skipped. `git lineage context chain <file>:<line>` prints the resolved chain — one live blame anchors HEAD, everything after is indexed reads (~30ms for `README.md:40` vs the prototype's ~540ms, matching it hop-for-hop). Full population on the dev corpus (~11.8k line objects): ~49s alongside the session index rebuild
- `git lineage rebuild embeddings` — the dense-embedding backfill as its own subcommand, symmetric with `rebuild index`. `rebuild` and `rebuild index` no longer run the embed pass — the backfill re-embeds the whole corpus and can take minutes, so it is opt-in via `git lineage rebuild --embed` (composite) or `rebuild embeddings` (embeddings alone). All the long passes now render a per-session `indicatif` progress bar (n/m with elapsed/eta) on stderr, so JSON/stdout consumers are unaffected and an empty corpus shows nothing; the embeddings bar counts only sessions still due at the current model version

- Turn-grain intent retrieval (`specs/context-injection-v0.md` extended): the retrieval unit is now the conversation **turn**, not the whole session. The index stores one FTS document per salient turn (`turns`/`turns_fts`), dense chunk vectors carry an anchor turn, both legs and RRF fusion rank turns, and evidence gains an additive `turn_id` plus a **verbatim** (capped) turn-text payload in place of the mechanical session summary — a query now answers with the past turn's own words. Salience is a v0 rule set applied at index time (`lineage-core::salience`): tool-result and read-only/low-prose exploration turns leave the corpus entirely, everything else (user/edit/decision/narration) is indexed — this is what stops a session's wide exploratory open from outranking the session that actually decided the thing. Existing indexes need `git lineage rebuild-index` (retriever versions bumped; caches re-derive)
- `git lineage context salience` — per-corpus breakdown of the salience classes (counts, percentages, % of turns indexed), the reproducibility surface for the rule set's measured baseline
- Spec: `UserPromptSubmit` intent trigger + `intent-query-v0` query shape, verbatim-turn digest format with affordance pointers, and an intent-path cache key (`query_hash`) documented in `context-injection-v0` ahead of the hook wiring

- Shell-mediated file writes are now captured as provenance. **Heredocs** (`cat > f << EOF … EOF`) and **python literal-replace scripts** (`open(p).read()` → `.replace(old, new)` → `open(p,'w').write`) — the two dominant ways agents edit files by shelling out — previously vanished into an opaque terminal-command artifact; the adapter now recognizes both and emits `FileEdit` artifacts (heredoc body as post-image; replace as an `OldString` edit) that feed normal line-object materialization. Regex substitution (`re.compile`/`.subn`) and read-only inspect scripts are excluded, and the heredoc-into-a-flag case (`git commit -m "$(cat<<)"`, `--body`) is rejected. Command text is parsed, never executed. On the dev corpus these tripled line objects (3002 → 10218) and nearly tripled resolved artifacts (738 → 2035)
- Materialization precision and read/write provenance (context-oracle gaps 9+11): edit artifacts now capture `new_string` (the post-edit text, `conversation-schema-v0`), and line-object resolution anchors on it first — the pre-image was consumed by the edit, so post-image matching is what actually locates edits in committed trees. Read-style tools (read/grep/glob/search) no longer produce `file_edit` artifacts, so reads cannot masquerade as authorship in the link gate, the materialization funnel, or oracle evidence; the search index records a `wrote` flag per touched file and the oracle's files-touched evidence tier is written-files-only (retriever version bumped — caches re-derive)
- `git lineage rebuild` — recompute the whole derived layer (commit links, line objects, search index) from stored sessions and git history under current code: wipes pre-gate links, relinks every commit through the evidence gate, replays manual links from the event log (manual links predating the event log are dropped, with a warning), and rebuilds the index; `rebuild index` scopes to the index alone (`rebuild-index` remains as a hidden alias)
- `git lineage context install [--user]` / `context uninstall [--user]` — wire the Claude Code context hook per-repo or once user-level (`~/.claude/settings.json`); user-level works because the hook now derives its repo from the file the agent read (not cwd) and fails open outside lineage repos
- Parent-workspace session capture: `import` now discovers Claude transcripts filed under ancestor directories of the repo (e.g. sessions opened in `~/src` working on `~/src/repo`), adopting a session only when it actually touched the repo; the original cwd is preserved as `session_cwd` metadata. The ancestor walk stops below `$HOME`
- `git lineage doctor` grew from a flat check list into the five-section diagnostics-v0 report — setup (binary/index schema versions, Claude hook wiring including whether the hook is loadable from every session root, git hooks), capture (discovered vs imported, workspace mismatches), materialization (artifact→line-object funnel with per-stage loss reasons), links (per-commit sessions with how each link was established), activity (event-log tail) — with `--json`, repeatable `--section` filters, and `--activity-limit`; the MCP `lineage_doctor` tool now returns the same object
- Local event log (`.git/lineage/events.jsonl`): every operation the tool performs — init, skill/hook install, import, link, materialize, rebuild-index, sync (with the server's full per-object response), and every context-hook fire including fired-but-silent outcomes with an explicit reason (`no_evidence`, `below_floor`, `over_budget`, `unappendable_shape`, `error`) — appends a versioned entry. Written best-effort: a failed log write never fails the operation. `context log` now reads from it and `.git/lineage/context-log.jsonl` is retired; `retrieval-v0` gains an optional `truncated` flag so budget-truncated retrievals are distinguishable from empty ones
- Diagnostics v0 (`specs/diagnostics-v0.md`): spec for a local event log
  (`.git/lineage/events.jsonl`, versioned per-operation entries) and a grown
  `git lineage doctor --json` contract (setup, capture, materialization, links,
  activity sections) that external developer tooling will consume; narrative
  contract only, not part of the schema/bindings pipeline
- Context oracle (`specs/context-injection-v0.md`): `git lineage context hook claude` — a Claude Code PostToolUse hook endpoint that deterministically appends provenance digests (attribution, line ranges, session summary, graded strength) to file-read results, backed by a new `lineage-oracle` crate (transport-neutral `Retriever` trait, tiered local retrieval over line objects and files-touched sessions, content-hash-keyed cache with negative caching); private sessions and forks of private sessions are never injected, and every injection is recorded in the local event log, viewable via `git lineage context log`

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

- **`git lineage sync` no longer re-uploads sessions it pulled from a server.** A session carrying `pull_origin` is dropped from the batch during assembly, along with its line objects and commit links: the server it came from already holds it, so re-pushing spent bandwidth on a guaranteed no-op (the write rules make it one — same content-derived ids, same hashes) and recorded the puller as the `uploaded_by_user_id` of work someone else wrote. The test is each session's own `pull_origin` and is deliberately **not** transitive: a **fork of** a pulled session has no `pull_origin` of its own, and is the forker's new session that no server has seen, so it still pushes with its fork edge to the pulled parent intact
- `git lineage sync` posts conversation-sized chunks (10 conversations per `POST /v0/sync`) instead of one monolith request. Blobs still upload once up front; a mid-run timeout leaves earlier chunks committed. Progress prints `chunk i/n` when more than one chunk is needed.
- **The VS Code extension's fork action is now `git lineage fork` and nothing else.** It previously required `metadata["source"]` — an absolute transcript path on the machine that did the import — so on any other machine the action was disabled even though the mechanism underneath would have worked. It now enables for any Claude session lineage has stored, calls the CLI through `lineageClient`, and shows the CLI's output in a document rather than a truncating notification: whose session it was, what it was about, and only then the command to run. It holds **no** fork logic of its own — no harness state directory, no transcript format, no id convention, and no vendor flags (`ARCHITECTURE.md` invariant 4). The command is shown, never executed for you: it opens a colleague's work in a live agent, which is a thing to choose. The CLI's stderr is surfaced verbatim on failure, so an unknown id or an unsupported agent says which
- **`git lineage list` is now scannable.** Rows carry the date and who ran the session alongside id/agent/turns/model, forks are marked `(fork)`, and the list is ordered newest first (ref order was neither chronological nor stable across machines). Every field was already collected for `--json` and simply discarded at print time; on a corpus of ~120 sessions the old four-column row gave a reader nothing to choose between. `--json` gains only the ordering
- `git lineage fork --help` now explains what a fork is — that you get their context and not their tools, that the fork is a new session belonging to you with theirs as an ancestor, and that the original is never modified — rather than listing two flags. `--dry-run` reports the transcript size as a magnitude (`1.5 MB`) rather than a byte count
- `git lineage fork` refuses a session with nothing replayable (all system turns, or content redacted away at import) instead of writing an empty transcript. The empty file would otherwise be rejected later by the harness as "session not found", pointing at the harness for something lineage already knew
- Dense embedding now uses a **model2vec static embedder** (`minishlab/potion-retrieval-32M`) instead of fastembed/jina-ONNX. Inference is a token→vector lookup + mean-pool + L2-normalize — no ONNX runtime, no per-invocation model session — so a query embeds in microseconds and the embedder fits the hook's latency budget (fastembed's multi-second model load never could, and its ~0.4s/chunk backfill was ~33min on this corpus). The model (two files, ~130 MB) downloads once into `~/.cache/lineage/embed` (`LINEAGE_EMBED_CACHE` override) and runs offline thereafter; a failed download errors clearly rather than panicking. `DENSE_RETRIEVER_VERSION` bumped, so vectors re-embed on the next backfill
- The **`dense` build feature is gone**: dense and fused retrieval compile into the default build, since the static embedder is lightweight. `git lineage context query --dense/--fused`, `rebuild --embed`, and `rebuild embeddings` work on every build — the lexical-only rejection paths are removed
- **Salience is now binary** (indexed or not) rather than a fractional ranking weight. Assistant narration is indexed at parity with user/edit/decision turns; only tool results and pure exploration stay out. The FTS leg ranks by plain bm25 (no salience multiplier) so it and the dense leg rank over the same corpus — the old 0.3 narration weight skewed bm25 while being invisible to the dense leg's cosine, so the legs diverged. `FTS_RETRIEVER_VERSION` bumped (ranking changed); `git lineage context salience` now reports per-class counts and indexed yes/no with a % of turns indexed
- Automatic session↔commit linking is now evidence-gated (context provenance precision): the post-commit hook links a session only when line objects materialized or the session wrote a file the commit changed; refused sessions are recorded as `skipped_no_overlap` in the event log and links carry a `basis` (`line_objects`/`file_overlap`) surfaced by doctor as `established_by`. Manual `git lineage link` is unchanged and stays ungated

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

- The extension's `.lineage/forks/` transcript copy. It was written on every fork and never read back by anything — the working half was always the shell-out. `git lineage fork` writes the transcript the harness actually opens, so the copy has no remaining role. Existing `.lineage/forks/` directories are inert and can be deleted
- `examples/demo-repo` bundled demo; use `tests/fixtures/` for samples and `./scripts/setup.sh` on your own project

### Fixed

- The retrieval cache no longer stores a budget-exhausted empty retrieval, which was silencing the corpus's richest files permanently. The cache key is content-addressed and carries no TTL, so nothing about it changes while a file and the corpus are unchanged: one cold retrieval that overran the hook's 200ms budget wrote an empty result that every later fire then returned in ~20ms without ever retrying. The effect was self-concealing — the file looked like it had no provenance, and the `over_budget` events that recorded the truth were indistinguishable from a genuinely slow repo. Measured on the dev corpus against `lineage-search/src/index.rs` (the single most-injected file): before, one `high`-strength injection followed by permanent silence; after, a 0.24s cold retrieval that caches and then answers in 0.02–0.05s, five fires out of five. Truncation *with* evidence is still cached — the retriever returns what it has by design, and that is a usable answer — as is an empty retrieval that completed, which is a real "honestly nothing" (`is_cacheable`, spec: context-injection-v0 § Cache)

- Fixed `git lineage doctor --json | head` (and any piped invocation) panicking with a broken-pipe error: SIGPIPE default behavior is restored at startup on unix

- Claude sessions are now discovered in repositories whose path contains `.` or `_`. Claude Code encodes the launch directory by substituting each of `/`, `.` and `_` with `-`, but `claude_project_key` replaced only `/` — so for a repo at `/srv/my_app` or any path with a dot, lineage looked for a project directory that never existed and `git lineage import` reported `discovered 0` with no error to explain it. `git lineage fork` had the mirror-image defect, writing its transcript to a path the harness would not resolve, which surfaces as Claude reporting the session is not found. The substitution rule was established by running Claude Code in probe directories and reading back the names it created, not inferred: it is per character and never collapses runs, so a path segment already containing `-` keeps every dash. Nothing needs re-importing beyond running `git lineage import` again in an affected repo. The round-trip test previously restated the substitution inline and so agreed with the same bug; it now calls the shared derivation, and tempdir paths contain dots, so a future divergence fails it

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
