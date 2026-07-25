# Context Injection v0

How provenance context flows back into a live agent session: the hook trigger
protocol, the retrieval request/response contract, the digest format, and the
cache semantics. A harness hook adapter and any retriever implementation —
in-process local or a future server endpoint — are both implemented against
this document.

Status: **draft for review** — written against `conversation-schema-v0`,
`line-object-schema-v0`, and `sync-protocol-v0`.

## Scope

- **In scope:** file-keyed injection triggered by an agent reading a file
  (Claude Code adapter specified); prompt-keyed (intent) injection triggered
  by the user submitting a message; the transport-neutral retrieval contract
  (`context-query-v0`, `intent-query-v0`, `retrieval-v0`); evidence tiers and
  strength; the digest formats (summary and verbatim-turn) and attribution;
  affordance pointers, addressable handles, and the traversal verb vocabulary;
  the session-start trigger that teaches it; cache keying and invalidation;
  privacy; the injection log.
- **Out of scope, not foreclosed:** adapters for other harnesses (the trigger
  section is per-adapter by design); the server-side retrieval endpoint and
  team-mode cache hydration (the contract here is written so a server can
  implement it; endpoint semantics version separately).
- **Never in scope:** injection without visible attribution, and injection of
  `private` objects (see [Privacy](#privacy)).

## Trigger protocol

Injection is deterministic: it fires on an observable harness event, never on
the agent deciding to ask. Each harness gets a thin adapter that maps its
native hook mechanism onto the retrieval contract; the core is
harness-agnostic.

### Claude Code adapter

- **Event:** `PostToolUse` on file-reading tools (`Read`), configured in the
  harness settings by the installer.
- **Input:** the hook's stdin JSON; the adapter extracts the tool's `file_path`
  and the file content the agent just received.
- **Output on evidence:** `hookSpecificOutput.updatedToolOutput` — the original
  tool output with the digest appended, preserving the response's shape. Read
  responses observed live (2026-07-18) are
  `{"type": "text", "file": {"content", …}}`; the digest is appended to
  `file.content`. A bare-string response is appended to directly. Any other
  shape MUST produce silence, never a reshaped tool result. This is the
  channel that reaches model context without spending an agent turn.
  (`PreToolUse` `additionalContext` is not injected into model context and
  MUST NOT be used.)
- **Output on no evidence, error, or deadline overrun:** exit 0 with no
  output. A conforming adapter MUST fail open — nothing the injection path
  does may ever surface an error inside the agent session or block the tool
  call.

### Claude Code adapter — intent trigger

- **Event:** `UserPromptSubmit`, configured in the harness settings by the
  installer. Fires deterministically on every user message; the message text
  is the query. CLI-first: `additionalContext` is known-broken in the VSCode
  extension, and a conforming adapter MUST probe the live payload shape before
  trusting it (the file-keyed adapter's lesson).
- **Input:** the hook's stdin JSON; the adapter extracts the prompt text and
  builds an `intent-query-v0`.
- **Output on evidence:** `hookSpecificOutput.additionalContext` carrying the
  rendered digest (see Digest format — verbatim turns). This reaches model
  context without spending an agent turn.
- **Output on no evidence, error, or deadline overrun:** exit 0 with no
  output — identical fail-open discipline to the file-keyed trigger. The
  miss path is latency-sensitive (it runs on every message): a retriever
  SHOULD answer "nothing" well inside the budget rather than exhaust it.

### Claude Code adapter — session-start trigger

Teaches the [traversal vocabulary](#traversal-vocabulary) once per session.
Unlike the other two triggers this one runs no retrieval: it names capability,
so it fires whether or not the corpus has anything to say.

- **Event:** `SessionStart`, configured in the harness settings by the
  installer. `SessionStart` groups carry no matcher — there is no tool to
  match on. Payload shape probed live 2026-07-25: stdin carries `session_id`,
  `transcript_path`, `cwd`, `hook_event_name`, and `source`
  (`startup` | `resume` | `clear` | `compact` | `fork`).
- **Output:** `hookSpecificOutput.additionalContext` carrying the vocabulary.
  The adapter MUST NOT read the payload for anything it needs — the vocabulary
  is identical across every `source` — so a payload that fails to parse still
  emits it. This is the fail-open shape for a trigger whose whole output is
  static.
- **A hook of this kind MUST state capability, never instruct.** Telling an
  agent to use lineage would make any measurement of injection a measurement
  of the prompt instead.

Delivery is a hook rather than an agent skill deliberately: whether a skill
loads depends on how many are installed, the shape of the opening prompt, and
where the harness looks. A hook fires deterministically — the same property
that makes the other triggers trustworthy.

MCP consumers need no equivalent: `tools/list` is verb discovery for free on
that path. A CLI session has no such channel, which is the whole reason this
trigger exists.

## Retrieval contract

One request/response shape regardless of where retrieval runs. Solo mode
answers it in-process from local data; team mode answers it server-side behind
the same shapes. The wire encoding is JSON with `snake_case` fields.

### `context-query-v0`

| Field | Type | Required | Meaning |
|-------|------|----------|---------|
| `file_path` | string | yes | Repo-relative path the agent read |
| `file_blob_sha` | string | yes | Lowercase hex SHA-256 of the file content the agent received |
| `repo` | object | yes | Repo identity hints, as in sync-protocol-v0 [Repo binding](sync-protocol-v0.md#repo-binding) |
| `budget_ms` | integer | no | Caller's remaining latency budget; a retriever SHOULD return what it has rather than overrun |

### `intent-query-v0`

The prompt-keyed request: free intent text, no file or line anchor. A distinct
shape rather than a `context-query-v0` variant — the file-keyed shape is
frozen and its fields are all required.

| Field | Type | Required | Meaning |
|-------|------|----------|---------|
| `text` | string | yes | The user's message / intent text to match against the corpus |
| `budget_ms` | integer | no | As in `context-query-v0` |

### `retrieval-v0`

| Field | Type | Required | Meaning |
|-------|------|----------|---------|
| `evidence` | array | yes | Zero or more evidence entries, strongest first |
| `strength` | string | yes | Overall grade: max of entries, or `none` when empty |
| `truncated` | boolean | no | Retrieval stopped early on the query's `budget_ms`; absent means false |

An empty `evidence` array is a first-class answer ("honestly nothing"), and it
is cached like any other (see [Cache](#cache)).

Each evidence entry:

| Field | Type | Required | Meaning |
|-------|------|----------|---------|
| `session_id` | string | yes | Conversation ULID the evidence comes from |
| `turn_id` | string | no | Turn ULID when the evidence is turn-grained (`intent_match`, and `line_objects` where the line object records it). Additive: consumers that only understand sessions ignore it |
| `tier` | string | yes | `line_objects`, `files_touched`, or `intent_match` |
| `strength` | string | yes | `high`, `medium`, or `low` (see below) |
| `match_confidence` | string | no | For `line_objects` evidence: the line object's `confidence` (`exact`, `heuristic`, `manual`) |
| `line_ranges` | array | no | `[start, end]` pairs in the current file, when line-level evidence exists |
| `summary` | string | yes | The evidence payload: a provenance summary, or for turn-grained evidence the verbatim (capped) turn text |
| `attribution` | string | yes | Display-only source label (who/when/agent); never an authorization identity |

### Evidence tiers and strength

Strength is the single ordered scale that selection floors and cache
heuristics act on. It is derived, never asserted independently of its inputs:

| Tier | Evidence | Strength |
|------|----------|----------|
| `line_objects` | A line object in this file links the session to specific lines | `exact`/`manual` match → `high`; `heuristic` match → `medium` |
| `files_touched` | The session **wrote** this path (edit/diff artifacts); summary is heuristic. Read-only touches are never evidence — a session that merely consulted a file must not become its provenance (and oracle-triggered reads would otherwise feed back into the corpus as evidence) | `low` |
| `intent_match` | A turn's content matched an intent query (lexical or dense). Not anchored to a file or line; ranking within a retrieval preserves the retriever's relevance order, and strength is only the coarse floor | `medium` |

`line-object-schema-v0`'s `confidence` is a match-quality scale, not an
ordered relevance scale — `strength` exists because selection needs a total
order and `manual` is not "less than" `exact`. Future retrievers (including
server-side refinement) MAY introduce new tiers but MUST map them onto the
same three-value `strength` scale.

## Digest format

The injected text, rendered by the **selector** from a `retrieval-v0` — always
at presentation time, never cached:

- First line is the attribution header and MUST identify the injection as
  Lineage-originated, e.g.
  `Lineage: 2 past sessions touched src/auth.rs — details below.`
- One block per selected evidence entry: its **handle**, attribution, line
  ranges when present, then the summary.
- Minimum strength `low` (inject `files_touched` evidence; silence still wins
  when there is none). Entry and byte caps are per trigger — see below.
- A selector MUST render only what the retrieval contains — no derivation,
  aggregation, or external lookups at render time.

### Handles

Every entry carries an addressable handle so the agent can name what it was
given to a [traversal verb](#traversal-vocabulary). The handle is
`<session_id>#<turn_id>` for turn-grained evidence and the bare `<session_id>`
when the entry has no `turn_id`. It is the one string in a digest that MUST
round-trip: a verb accepts a handle exactly as rendered.

### Per-trigger budgets

The two evidence-bearing triggers fire under different conditions and MUST NOT
share one budget:

| | `PostToolUse`/`Read` (file-keyed) | `UserPromptSubmit` (intent) |
|---|---|---|
| Entry cap | 1 | 3 |
| Byte cap | ~200 tokens (~800 B UTF-8) | 1,024 tokens (~4 KiB UTF-8) |
| Verb footer | absent | present |
| Position | bottom | bottom |

The file-keyed trigger fires constantly mid-task and is appended into a tool
result the agent is actively reading, so a false positive costs more (it
repeats per read) and the agent mostly does not want diverting. The intent
trigger fires at a decision point before the agent has committed to an
approach, where exploration is worth most.

**Position is bottom on both.** For the file-keyed hook, appending after
content preserves what a `Read` is expected to return; front-loading
provenance ahead of file content would be hostile to the primary task.

### Verbatim-turn digest (intent trigger)

Turn-grained evidence injects the turn's own words, not a heuristic summary —
the digest body for an `intent_match` entry is the evidence `summary` field
carrying the verbatim turn text, capped at extraction time so render stays a
pure formatting step:

- The attribution header identifies the injection as Lineage-originated and
  names the trigger, e.g.
  `Lineage: 2 past turns match this prompt — details below.`
- One block per selected turn: handle, attribution (agent, session date,
  author when known), then the verbatim turn text.
- A block states **which edges its node has** — it names nouns, not commands.
  Rendering a command per edge per entry costs over 13% of the intent budget on
  navigation; naming the vocabulary once recovers that.
- The digest ends with a single **verb footer** naming the traversal vocabulary
  the installed CLI supports. Reconciling with "a selector MUST omit relations
  it cannot honour": the footer names verbs the CLI has, the per-entry edge
  statements name edges the node has, and the agent intersects them. Still
  truthful, materially cheaper.
- Navigation MUST cost under 5% of the trigger's byte cap.

## Traversal vocabulary

The digest is an entry point, not an answer. A receiving agent handed evidence
that is close but not right needs to move through the provenance graph itself,
so the injection surface is paired with a **closed, named vocabulary** of
moves. Each verb is derived from a way the injected set can be wrong:

| Failure of the injected set | Verb | Repairs by |
|---|---|---|
| Right sessions, wrong turns | `search-within` | Searching the text of named sessions — one call, not N greps |
| Right turn, missing its argument | `around` | Reading the turns adjacent to it in its session |
| Right turn, want its outcome | `produced-by` | Listing the code that turn produced |
| Have a commit, want the reasoning | `sessions-for-commit` | Naming the sessions behind it |

Every verb MUST be:

- **read-only** — indexing and rebuild operations are never exposed;
- **gated** — anything returning turn text passes the same privacy filter as
  the digest (see [Privacy](#privacy));
- **bounded** — every verb takes a limit.

The vocabulary is one set with two renderings, and **no capability may exist
for one consumer and not the other**: a conforming implementation exposes
exactly this set on every surface it offers (CLI subcommands, MCP tools), and
the relation names above are abstract — each surface spells them its own way.
An MCP consumer never shells out, so a runnable command string is a rendering
detail and MUST NOT be the vocabulary's definition.

`sessions-for-commit` is the one verb whose entry point is not an injected
digest: it composes lineage with ordinary git work.

## Cache

A local, derived-data cache makes repeat and negative answers effectively
free. Implementations MUST treat it as disposable: deleting it is always safe
and merely costs re-retrieval.

- **Key:** `(file_path, file_blob_sha, corpus_generation, retriever_version)`.
  Every retriever input is a key part, so within one key a hit is exactly what
  the retriever would answer. `corpus_generation` is a counter bumped on every
  import (new sessions invalidate); `retriever_version` invalidates on logic
  changes.
- **Intent-path key:** `(query_hash, corpus_generation, retriever_version)`,
  where `query_hash` is a content hash of the normalized query text. Free text
  is a weak cache key — repeat hits come mostly from the same prompt re-fired
  (harness retries, resumed sessions) — so the intent cache exists chiefly for
  **negative caching**: the "nothing" answer for a repeated prompt must stay
  effectively free.
- **Value:** the serialized `retrieval-v0` — structured evidence, never the
  rendered digest — plus a `schema_version` for the value encoding. An
  unreadable or version-mismatched value is a miss and is deleted. Selection
  runs per trigger, so presentation and policy changes require no
  invalidation.
- **Negative caching:** empty retrievals are stored under the same key. Most
  files have no provenance; answering "nothing" instantly is what keeps the
  trigger path invisible.
- **Team mode (deferred):** when retrieval runs server-side,
  `corpus_generation` is not locally observable — cache entries then rely on
  the stored `strength` and freshness heuristics instead of provable
  currency. This asymmetry is by design and is why strength is recorded per
  entry from day one.

## Privacy

- Objects with `private: true` — and conversations whose `parent_session_id`
  chain reaches a private conversation — are **never** included as evidence.
  This is enforced inside the retriever, before any caching or selection, so
  no presentation-layer mistake can leak them. Mirrors
  sync-protocol-v0 [Privacy](sync-protocol-v0.md#privacy): filtering at the
  source is the guarantee, not a downstream courtesy.
- Cached values inherit the guarantee: a conforming retriever never emits
  private evidence, so no private content can enter the cache.
- The guarantee holds across **many exits**. Digest rendering was once the
  single path out, and the filter could sit on it; the
  [traversal vocabulary](#traversal-vocabulary) adds a path per verb. An
  implementation MUST therefore enforce the filter structurally rather than
  by convention — in a typed language, by making "turn text that has passed
  the gate" a distinct type that only the gate can construct, so a new verb
  that forgets to filter fails to compile. Auditing each exit by hand is not
  a conforming substitute: the failure mode is a future exit nobody audits.
- A private session is refused **whole**, not merely redacted: verbs that
  return no turn text (`sessions-for-commit`) still MUST NOT name it, because
  the spec's rule is that it is never evidence at all. A session id that
  cannot be read is refused for the same reason — unknown is not public.

## Injection log

Every injection MUST be locally recorded: timestamp, `file_path`, session ids
injected, strength, and the triggering harness session when known. The log is
append-only, local plumbing (never syncs — same class as
`.git/lineage/index.db` in the sync-protocol
[Object mapping](sync-protocol-v0.md#object-mapping)), and is the surface a
user consults to see what their agent was told. Silent enrichment without a
consultable record is out of contract.

The session-start trigger is recorded too, with its `source`. It injects no
evidence, so it has no session ids or strength to log — but an agent that was
told the vocabulary exists was told something, and the log is where a user
sees that.
