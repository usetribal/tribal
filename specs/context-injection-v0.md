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
  (Claude Code adapter specified); the transport-neutral retrieval contract
  (`context-query-v0`, `retrieval-v0`); evidence tiers and strength; the digest
  format and attribution; cache keying and invalidation; privacy; the
  injection log.
- **Out of scope, not foreclosed:** prompt-keyed and session-start triggers;
  adapters for other harnesses (the trigger section is per-adapter by design);
  the server-side retrieval endpoint and team-mode cache hydration (the
  contract here is written so a server can implement it; endpoint semantics
  version separately).
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
| `tier` | string | yes | `line_objects` or `files_touched` |
| `strength` | string | yes | `high`, `medium`, or `low` (see below) |
| `match_confidence` | string | no | For `line_objects` evidence: the line object's `confidence` (`exact`, `heuristic`, `manual`) |
| `line_ranges` | array | no | `[start, end]` pairs in the current file, when line-level evidence exists |
| `summary` | string | yes | Human-readable provenance summary for this file from this session |
| `attribution` | string | yes | Display-only source label (who/when/agent); never an authorization identity |

### Evidence tiers and strength

Strength is the single ordered scale that selection floors and cache
heuristics act on. It is derived, never asserted independently of its inputs:

| Tier | Evidence | Strength |
|------|----------|----------|
| `line_objects` | A line object in this file links the session to specific lines | `exact`/`manual` match → `high`; `heuristic` match → `medium` |
| `files_touched` | The session **wrote** this path (edit/diff artifacts); summary is heuristic. Read-only touches are never evidence — a session that merely consulted a file must not become its provenance (and oracle-triggered reads would otherwise feed back into the corpus as evidence) | `low` |

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
- One block per selected evidence entry: attribution, line ranges when
  present, then the summary.
- Selection defaults (all locally configurable): minimum strength `low`
  (inject `files_touched` evidence; silence still wins when there is none),
  at most 3 evidence entries, total digest capped at 1,024 tokens
  (~4 KiB UTF-8).
- A selector MUST render only what the retrieval contains — no derivation,
  aggregation, or external lookups at render time.

## Cache

A local, derived-data cache makes repeat and negative answers effectively
free. Implementations MUST treat it as disposable: deleting it is always safe
and merely costs re-retrieval.

- **Key:** `(file_path, file_blob_sha, corpus_generation, retriever_version)`.
  Every retriever input is a key part, so within one key a hit is exactly what
  the retriever would answer. `corpus_generation` is a counter bumped on every
  import (new sessions invalidate); `retriever_version` invalidates on logic
  changes.
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

## Injection log

Every injection MUST be locally recorded: timestamp, `file_path`, session ids
injected, strength, and the triggering harness session when known. The log is
append-only, local plumbing (never syncs — same class as
`.git/lineage/index.db` in the sync-protocol
[Object mapping](sync-protocol-v0.md#object-mapping)), and is the surface a
user consults to see what their agent was told. Silent enrichment without a
consultable record is out of contract.
