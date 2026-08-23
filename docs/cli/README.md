# CLI reference

[← Documentation index](../README.md) · [Setup](../../README.md#setup)

`tribal` is the primary interface for importing, querying, sharing, and maintaining agent provenance in a git repository.

## Install

```bash
cargo install --path crates/lineage-cli
# or: make setup
```

Ensure `~/.cargo/bin` is on your `PATH` so `tribal` resolves. The binary name is `tribal`.

Target another repository:

```bash
tribal --repo /path/to/repo <command>
```

---

## Project setup

```bash
tribal init                    # interactive wizard
tribal init --yes              # non-interactive defaults
tribal init --no-import        # skip first import
tribal init --config             # write default refs/lineage/config only
```

### Agent skills

Two bundled skills: `lineage` teaches agents to use lineage features (search, blame, share, rebase, resume), and `share` teaches them to turn the current session into a link. Installed during init or manually:

```bash
tribal init --skills
tribal init --skills --target cursor
tribal init --skills --target claude --target codex
tribal init --skills --target all --force
```

| Target | Install path |
|--------|--------------|
| `cursor` | `.cursor/skills/{lineage,share}/SKILL.md` |
| `claude` | `.claude/skills/{lineage,share}/SKILL.md` |
| `codex` / `agents` | `.agents/skills/{lineage,share}/SKILL.md` |
| `all` | All three (default) |

Re-run with `--force` after upgrading the CLI to refresh skill content.

---

## Discovering the surface

`tribal --discover` prints the whole command surface as JSON — every
command including the hidden ones, with its group, aliases, options, and nested
subcommands. It is walked from the parser itself, so it cannot drift from what
the binary accepts, and it needs no repository.

```bash
tribal --discover | jq '.commands[] | select(.hidden) | .name'
```

Intended for an agent deciding what to call: `--help` shows what a person should
reach for first, `--discover` shows everything that exists.

## Command reference

### Setup and health

| Command | Description |
|---------|-------------|
| `tribal doctor [--json] [--section NAME] [--activity-limit N]` | Six-section health report (setup, capture, materialization, coverage, links, activity); see `specs/diagnostics-v0.md` |
| `tribal init [options]` | Interactive setup: config, agent skills, hooks, optional import |
| `tribal init --config` | Write default `refs/lineage/config` |
| `tribal init --skills [options]` | Install the bundled agent skills |
| `tribal init --hooks [--force-hooks]` | Install the pre-commit and post-commit hooks |
| `tribal init --uninstall` | Remove the hooks and agent-hook wiring that setup installed |

### Import

| Command | Description |
|---------|-------------|
| `tribal import [options]` | Import agent history (`ingest` alias) |

Flags: `--agent cursor|claude|codex|all`, `--since DATE`, `--incremental`, `--no-link-head`.

See [Import](../import.md).

### Query

| Command | Description |
|---------|-------------|
| `tribal list [--commit SHA] [--json]` | List sessions or sessions at commit |
| `tribal show <id> [--json] [--hydrate-images]` | Show conversation |
| `tribal blame <path>[:line] [--json]` | Tribal for a file line |
| `tribal fork [<session-id>] [--query <text>] [--pick N] [--new] [--json]` | Continue a session: reopens one this machine holds, writes out any other. `--new` writes one out even when it could be reopened (see [Fork a session](#fork-a-session)) |
| `tribal fork <share-url> [--into DIR] [--no-open] [--server URL]` | Continue a session from a share link — clones the repo if needed and opens the harness (see [Fork a session](#fork-a-session)) |
| `tribal fork <session-id> --brief` | Print a context block for starting a subagent on that session; writes nothing (see [Brief a subagent](#brief-a-subagent-on-a-session)) |
| `tribal search <query>` | Full-text search (auto-rebuilds stale index). Superseded by `tribal context query`, which ranks and returns follow-up commands |
| `tribal rebuild [--embed]` | Rebuild all derived state (links, line objects, index) from stored sessions; `--embed` also runs the dense-embedding backfill |
| `tribal rebuild index` | Rebuild only the search index |
| `tribal rebuild embeddings` | Rebuild only the dense embeddings — the semantic backfill (shows a per-session progress bar) |
| `tribal export [--redact] [--format json\|jsonl]` | Export sessions |

See [Explore](../explore.md).

### Linking and history

| Command | Description |
|---------|-------------|
| `tribal link <session-id> <commit-sha>` | Manually link session and materialize |
| `tribal materialize [--commit SHA] [--session ID]` | Build line objects |
| `tribal remap` | Recover lineage after rebase |

See [Rebase](../rebase.md) and [Maintenance](../maintenance.md).

### LFS

| Command | Description |
|---------|-------------|
| `tribal lfs status` | Referenced vs local LFS objects |
| `tribal lfs push [--remote origin]` | Push LFS refs |
| `tribal lfs fetch [--remote origin]` | Fetch missing LFS objects |

See [LFS](../lfs.md).

### Context injection

| Command | Description |
|---------|-------------|
| `tribal context hook claude` | Agent-hook endpoint (Claude Code PostToolUse on stdin); emits injection JSON or nothing |
| `tribal context hook claude-session-start` | Agent-hook endpoint (Claude Code SessionStart); emits the traversal vocabulary and the continuation capability (`continue`, `continue --brief`) once per session |
| `tribal context log [--limit N]` | Show recorded context injections, newest last |
| `tribal context install [--user]` | Wire the context hook per-repo or user-level (all repos) |
| `tribal context uninstall [--user]` | Remove lineage context-hook wiring |
| `tribal context query "<text>" [--timing]` | Retrieve past turns matching a free-text intent. With no leg/`--file` flag the query is **dispatched**: a named file that exists (in the corpus or the working tree) routes to the temporal plan on that anchor, everything else to the fused plan; `--timing` prints the chosen `route:` and per-stage timings |
| `tribal context query "<text>" [--lexical\|--dense\|--fused]` | Force one leg, skipping the dispatcher — the flags exist to see a leg in isolation |
| `tribal context query --file <path>[:<line>] ["text"]` | Force the line-anchored temporal plan (skips the dispatcher): the turns that authored the file/line, time-ordered (walked back through ancestry); with text, the text re-ranks those anchored turns |
| `tribal context salience` | Report the corpus's turn-salience breakdown (what indexing keeps and drops) |
| `tribal context chain <file>:<line>` | Print the temporal chain for a line — one hop per row (short sha, date, session, turn, confidence, or `DARK(kind)`) — resolved from the index (one live blame anchors HEAD, the rest are indexed reads) |

#### Traversal verbs

The moves a receiving agent makes when the injected evidence is close but not
right. Each takes a digest handle (`session#turn`) exactly as rendered, is
read-only and privacy-gated, and is bounded by `--limit`.

| Command | Repairs |
|---------|---------|
| `tribal context search-within "<text>" --session <handle>...` | Right sessions, wrong turns — searches the text of named sessions in one call rather than N greps |
| `tribal context around <handle> [--radius N]` | Right turn, missing its argument — the turns adjacent to it in its session |
| `tribal context produced-by <handle>` | Right turn, want its outcome — the code that turn produced, as `file:lines` |
| `tribal context sessions-for-commit <sha>` | Have a commit, want the reasoning — the sessions behind it (short shas resolve as elsewhere in git) |

The same four are MCP tools (`lineage_search_within`, `lineage_turns_around`,
`lineage_produced_by`, `lineage_sessions_for_commit`); paired registry tests
assert neither surface can gain or lose a verb without the other. MCP agents
discover them from `tools/list`; CLI sessions learn them from the `SessionStart`
hook that `context install` wires.

The `SessionStart` vocabulary also names `tribal fork` and `fork --brief`,
so a CLI session knows a session can be *continued* and not only read. That
capability is registered alongside the verbs (`lineage-retrieval::CONTINUE_SESSION`)
and reaches MCP as `lineage_fork_brief`, under the same paired tests — but it is
not a traversal verb and so is not in `VERBS`: it takes a bare session id rather
than a `session#turn` handle, and it is not read-only.

It is taught by the hook rather than the bundled skill on purpose: a skill loads
only if it was installed and only if the harness looks for it, whereas the hook
fires every session. The vocabulary states what the commands do and never when
to reach for them — an agent told to use lineage would make any measurement of
injection a measurement of the prompt.

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
| `tribal login [--server URL]` | Sign in to a Tribal server (browser device flow; defaults to production) |
| `tribal sync [--server URL] [--token TOKEN] [--remote origin]` | Exchange sessions with a Tribal server: push, then pull |
| `tribal push [--server URL] [--token TOKEN] [--remote origin]` | Push only — the one-way half of `sync` |
| `tribal pull [--server URL] [--token TOKEN] [--remote origin] [--dry-run]` | Pull only — the other half (see [Pull teammates' sessions](#pull-teammates-sessions)) |

| `tribal share [--session ID] [--server URL] [--token TOKEN] [--remote origin] [--no-open]` | Share one session as a link anyone can open (see [Share a session as a link](#share-a-session-as-a-link)) |

`sync` is the one to reach for; `push` and `pull` stay addressable for a
one-directional run and are hidden from `tribal -h`.

**You do not have to run `login` first.** Any command that talks to a server
resolves its token the same way — explicit `--token`, then `LINEAGE_TOKEN`, then
the stored login — and if none of those yields one, it signs you in and carries
on. That covers a login that expired or was revoked, which is the same situation
from where you are standing. Signing in needs a terminal, so off one (CI, a
hook, a pipe) the command fails with the message naming the fix rather than
blocking on a browser approval nobody can give.

`login` prints a verification URL and code, waits for the browser approval, and
stores an opaque session handle in `~/.config/lineage/credentials.json` (0600;
`XDG_CONFIG_HOME` respected, `LINEAGE_CONFIG_DIR` overrides). The handle is the
durable credential — short-lived access tokens are minted from it per command,
and the identity-provider refresh token never leaves the server.

Signing in for the first time takes **two browser approvals**: the first proves
who you are, and the second lets the server read which organizations you belong
to, so it can work out your workspaces. The second approval is a separate step
because the identity provider's device flow cannot ask for that permission. The
server uses that access once and discards it. Returning logins take one approval.

If a stored login expires or is revoked, the next sync says to run `login` again.

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
| `tribal init --hooks [--force]` | Install pre-commit and post-commit hooks |
| `tribal init --uninstall` | Remove lineage hooks |

See [Git hooks](../git-hooks.md).

### Cleanup

| Command | Description |
|---------|-------------|
| `tribal delete <session-id> [--purge-blobs]` | Remove a session from this repository. Hidden from `tribal -h`: it is local-only, so it does not stop a synced or shared session reaching anyone else |
| `tribal gc` | Purge orphan line objects and unreferenced LFS blobs |

See [Maintenance](../maintenance.md) and [Privacy](../privacy.md).

---

## Pull teammates' sessions

`tribal pull` brings sessions your teammates have synced down into this
repository's lineage refs, so `list`, `show`, `search`, and `fork` see them the
same as sessions you imported yourself.

```bash
tribal pull
```

```text
https://api.usetribal.io/api has 3 conversation(s) this repository is missing or behind on (2 new, 1 grown).
Wrote 3 session(s):
    01HQZX8K9V2M3N4P5Q6R7S8T9U
    01HQZW1A2B3C4D5E6F7G8H9J0K
    01HQZV7J8U1L2M3N4P5Q6R7S8T

Pull never deletes: sessions the server did not mention are untouched,
and turns you already had were kept as they were.

`tribal list` shows them; `tribal fork <id>` continues one.
```

`--dry-run` reports what would arrive and writes nothing. `--server`, `--token`,
and `--remote` resolve exactly as they do for `sync`.

Two round-trips, not one. `pull` first sends a digest of what this repository
already holds — one `{conversationId, turnCount, endedAt}` entry per stored
session — and the server answers with the ids that differ; a second call fetches
those conversations with their turns. The cursor is that content digest rather
than a sequence number or an `updated_at` watermark, so there is no gapless
counter to get wrong under concurrency and no clock to trust.

Merge rules, mirroring the push write rules in
[sync-protocol-v0](../../specs/sync-protocol-v0.md) so the two compose:

- **Pull never deletes.** A session the server does not mention is left exactly
  as it is. You may hold sessions you have never pushed, and reconciling by
  deletion would destroy unsynced local work.
- **Turns are immutable**, so a turn you already have is kept as it is and a
  second identical pull writes nothing at all.
- **Container fields merge monotonically**: `commit_shas` is a set union,
  `ended_at` takes the later time, the turn set grows and never shrinks, and
  `metadata` is first-write-wins per key. Nothing your local copy knows is lost
  by pulling — including a local copy that is *ahead* of the server's.
- **`pull_origin` is first-write-wins.** A session already marked keeps the
  marker it had; re-pulling does not restamp it.

Notes:

- Pulled sessions carry `pull_origin` (server, when, lineage version) so the
  push path can skip re-uploading them — the server they came from is already
  their source of truth. A **fork of** a pulled session is yours and does push.
- A commit sha naming history this checkout has not fetched is dropped rather
  than failing the pull. Run `git fetch` and pull again; the union merge adds it
  then.
- Turns arrive as text. Tool calls and edit artifacts are not on the pull wire
  yet, so a pulled session has no line objects of its own — `fork` renders tool
  activity as prose regardless, so it still reads as history.
- Private sessions are never emitted by the server, so nothing that arrives here
  needs re-filtering.

## Share a session as a link

`tribal share` turns the session you are in into a link anyone can open
without a Tribal account, and opens it in your browser.

```bash
tribal share
```

```text
sharing session 01HQZX8K9V2M3N4P5Q6R7S8T9U (14 turn(s))

    https://app.usetribal.io/s/7Kq2mXvB9nRt4Ls0Yc1Hpz

Pinned at 14 turn(s): continuing this session does not change what the link shows.
```

Capture is the `sync` path with a filter, not a second upload route: the session
is re-imported so the link catches the turns it has *now*, the same redaction
rules run before anything crosses the wire, and what is pushed is an ordinary
`sync-batch-v0` narrowed to that one conversation. `--server`, `--token`, and
`--remote` resolve exactly as they do for `sync`. Implements
[share-v0](../../specs/share-v0.md).

**Which session gets shared.** With no arguments, the most recently modified
agent transcript for this working directory — the one you are almost certainly
in. `--session` overrides it and takes the same id forms as `fork`: a lineage id,
an id prefix, or the harness UUID you can copy out of your terminal.

```bash
tribal share --session 01HQZX8K9V2M3N4P5Q6R7S8T9U
tribal share --session 550e8400-e29b-41d4-a716-446655440000
```

**A private session is refused, never stripped.** Sharing a session your repo
config marks private fails with the session named and nothing is uploaded.
Privacy is not something a link may unset — un-mark the session
(`refs/lineage/config` decides what counts as private) if you meant to share it.

Notes:

- **The URL is the credential.** Whoever holds `/s/<token>` can read that one
  conversation and nothing else — not the repo, not your other sessions. The
  server stores only the token's hash, so it cannot show you the link twice.
- **A share never grows.** It is pinned at the turn count it had when you ran
  the command. Continuing the session afterwards changes nothing for people
  holding the link; run `share` again to publish a longer prefix, which mints a
  new link rather than updating the old one.
- `--no-open` prints the link without launching a browser. The browser is a
  convenience either way: on a headless box the launch fails, the command says
  so, and the printed link is still the share.
- A session pulled from a server is not shareable from here — the server that
  holds it is the one to share it from.

## Fork a session

`tribal fork <session-id>` carries on an agent session. Which of the two
ways that happens is a property of the session, not a choice you make:

- A session **your harness still holds** is reopened in place. Nothing is
  written, and it stays the same session — continuing it adds to its history.
- **Any other** — a teammate's, or one pulled from a server — is written out as
  a new session that is yours, carrying their context with theirs recorded as
  the ancestor. `--new` forces this even for a session that could be reopened.

Which one happened is printed. Claude Code and Codex can be reopened; Cursor
declines by name, because the id lineage records comes from its IDE store and
`cursor-agent --resume` reads a separate CLI store.

`tribal fork <share-url>` does the same from a share link — no account, no
`tribal init`, no prior setup. It fetches the shared session, works out
where to land it (the current checkout if its remote matches, the most recently
used matching checkout it has seen, or a fresh clone into `./<name>` — printed,
never asked), writes the transcript, and opens the harness on it. `--into <dir>`
forks somewhere explicit instead, `--no-open` prints the resume command rather
than running it, and `--server` overrides the API origin.

The API origin is asked for, not guessed: a share link names the web app, and
where the API sits relative to it differs per deployment, so the link's origin is
read for `/.well-known/lineage.json` naming its API base (`share-v0`). An origin
that publishes nothing falls back to rewriting `app.<domain>` to `api.<domain>`,
which is why a deployment serving its API under a path prefix needs either the
document or `--server`.

Find one with `tribal list`, which shows id, date, turn count, agent, model,
and who ran it, newest first:

```text
01HQZX8K9V2M3N4P5Q6R7S8T9U  2026-07-26     8 turns  claude  claude-sonnet-4  Alice Chen
01HQZW1A2B3C4D5E6F7G8H9J0K  2026-07-25    41 turns  claude  claude-opus-4    Bob Reyes  (fork)
```

Then fork it:

```bash
tribal fork 01HQZX8K9V2M3N4P5Q6R7S8T9U
```

```text
Alice Chen's claude session, 26 July 2026
  8 turns, claude-sonnet-4
  Asked for: the login endpoint accepts an empty password when the user record has no salt — figure out why and fix it
  Changed:   src/auth.rs, src/session.rs
  Commits:   ad068c4a

Wrote /home/bob/.claude/projects/-home-bob-src-app/019f9d91-3f94-48f4-8cbf-663330ac0cee.jsonl
Recorded fork 01KYES2FWND56B76290VMWRXXH (continues 01HQZX8K9V2M3N4P5Q6R7S8T9U)

To continue it, run this from /home/bob/src/app:

    claude --resume 019f9d91-3f94-48f4-8cbf-663330ac0cee
```

The command is printed, not run — so a human can read it before acting on it and
an agent that invoked the CLI can act on it directly.

Notes:

- The session is resolved from lineage's own refs, not from a local transcript
  path, so a session pulled from a teammate forks the same as one you imported.
- A fresh vendor session id is minted; the source session's file is never read
  or modified, and your continuation is a new session either way.
- Tool activity is replayed as prose rather than as `tool_use` blocks. You get
  their context and reasoning; you do not get replayable tool handles, and
  `/rewind` will not reach back into their session.
- Forking is recorded as `fork_origin` on the new session
  ([conversation schema](../../specs/conversation-schema-v0.md)). Lines you write
  after the fork are attributed to you; the source session is an ancestor, never
  a co-author.
- Claude Code only for now. Codex and Cursor sessions decline by name — see
  [Fork a session](../fork-a-session.md) for why each is not supported yet.
- A session with nothing replayable (all system turns, or content redacted away
  at import) is refused here rather than written out as an empty transcript the
  harness would later reject as "session not found".

## Brief a subagent on a session

`tribal fork <session-id> --brief` writes nothing. It prints a
self-contained context block for handing to a **subagent** — one the calling
agent spawns with its own tool — so someone else's session can be investigated
without loading it into the current window.

Tribal cannot spawn the subagent; that is model-initiated. Its whole job here
is to emit the text.

```bash
tribal fork 01HQZX8K9V2M3N4P5Q6R7S8T9U --brief
```

The block has three parts:

1. **The brief** — whose session it was, when, and the selected turns, each
   headed by the `session#turn` handle the traversal commands take.
2. **The traversal vocabulary** — the same commands the `SessionStart` hook
   teaches. They are embedded because that hook fires for the session you are
   in and *not* for a subagent it spawns: without them the subagent could read
   this block and not move beyond it. `fork` is deliberately not among them — a
   subagent cannot tell it is already inside one.
3. **A marked task slot** — a trailing `--- TASK (append the subagent's task
   below this line) ---` line. Tribal does not supply the task; the calling
   agent appends it.

Which turns appear is a fixed rule, not a judgement:

- every user prompt with content — all of them, because the prompts are the
  intent thread and one prompt makes a negotiation look like a single question,
- every turn that changed code,
- the last assistant turn, which is where the work stopped.

Capped at 100 turns and 64 KiB. Over either cap, turns are dropped
lowest-priority first — edit turns oldest-first, then user prompts oldest-first
— and the last assistant turn is never dropped. **Anything dropped is stated in
the output** (`12 of 40 edit turns shown`), because a partial account of
someone's session that does not say it is partial is worse than none; the
traversal commands are how the reader fills the gap.

Notes:

- Nothing is written and no fork edge is recorded. This is an initial context
  load, not a fork.
- It works for any stored session, including one pulled from a teammate and one
  whose agent this build cannot write a transcript for. Reading a session and
  continuing it are different capabilities, so `--brief` does not inherit
  fork's requirement of a renderable transcript.

## Reopening versus writing one out

`tribal fork` reopens a session your harness holds:

```bash
tribal fork 01HQZX8K9V2M3N4P5Q6R7S8T9U
```

```text
claude session 01HQZX8K9V2M3N4P5Q6R7S8T9U

To reopen it, run this from /home/bob/src/app:

    claude --resume 019f9d91-3f94-48f4-8cbf-663330ac0cee

This is the original session, not a copy: continuing it adds to its history.
To write it out as a new session of your own instead, add --new.
```

Notes:

- The command is printed, not run: it opens an interactive agent, which is a
  thing to choose rather than have happen.
- A teammate's session carries no vendor id here, so there is nothing to reopen.
  That is not an error — it is exactly the case that gets written out instead.
- Whether a directory matters is the harness's business: Claude derives a project
  key from the launch directory, so its output names one; Codex resolves by id
  from anywhere, so its output does not.

## Session metadata

Import records `prompted_by_email` and `prompted_by_name` from git `user.email` / `user.name`. Sessions may include `vendor_session_id` for resume/fork, `parent_session_id`, `git_branch`, and `architecture_summary` metadata.

Schema details: [Schemas](../schemas.md).

## Related guides

- [Configuration](../configuration.md)
- [VS Code extension](../vscode.md)
- [MCP server](../mcp/README.md)
