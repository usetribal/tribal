# Sync Protocol v0

How the local tool's git-native storage (refs, notes, LFS-style blobs) maps onto
the platform's database + object-store model, and the wire protocol that moves
data between them. The platform's ingest endpoint and the CLI's `sync` command
are both implemented against this document.

Status: **draft for review** — written against `conversation-schema-v0`,
`line-object-schema-v0`, `git-notes-schema-v0`, and the platform's sync
semantics (write rules summarized in [Write rules](#write-rules); the durable
reasoning lives in the platform's `docs/sync-semantics.md`).

## Scope

- **In scope:** one-way up-sync (local → platform): conversations with embedded
  turns, line objects, session↔commit links, large-content blobs; identity and
  idempotency; blob transfer; privacy; repo binding.
- **Out of scope, not foreclosed:** down-sync (platform → local) — the read API
  returns canonical spec shapes so the CLI can become a consumer later; have/want
  dedup negotiation; presigned direct-to-bucket blob upload; backfill (same
  endpoint semantics, versioned separately).
- **Never in scope:** raw git objects. Git is transport on the local side only;
  the platform receives JSON documents and content-addressed blobs, never refs,
  notes, or packfiles.

## Transport

HTTPS + JSON. Two endpoints:

| Endpoint | Purpose |
|----------|---------|
| `POST /v0/sync` | Upload one batch of metadata objects (`sync-batch-v0`) |
| `PUT /v0/blobs/{sha256}` | Upload one content-addressed blob (`application/octet-stream`) |

Authentication: every request carries the platform-minted bearer token obtained
via the device flow (`Authorization: Bearer …`). The token's verified claims —
tenant and user — are inputs to the write rules below; nothing in the request
body can override them.

The wire types (`sync-batch-v0`, `sync-response-v0`) are defined in
`lineage-core` and published through the standard pipeline
([decisions/0001](decisions/0001-contract-bindings-pipeline.md)):
`schema/sync-batch-v0.schema.json`, `schema/sync-response-v0.schema.json`, and
the zod bindings in `@lineage/contracts`.

## Object mapping

| Local (git-native) | Wire | Platform |
|--------------------|------|----------|
| `refs/lineage/sessions/<id>` → conversation blob | `conversations[]` (conversation-v0 documents, turns embedded) | `conversation` + `turn` rows |
| line-object blobs | `line_objects[]` (line-object-v0 documents) | `line_object` rows |
| `refs/notes/lineage` note entries (`session_ids` × `commit_sha`, `patch_id`) | `session_commit_links[]` | `session_commit` rows |
| LFS objects (`.git/lfs/objects/`, sha256 layout) | blob manifest entry + `PUT /v0/blobs/{sha256}` | `blob_ref` row + object store |
| `refs/lineage/index`, `refs/lineage/config`, `.git/lineage/index.db` | — | — (local plumbing; never syncs) |

Git notes are decomposed client-side: one note linking commit `C` to sessions
`S1, S2` with patch-id `P` becomes two `session_commit_links` entries
(`{S1, C, P}`, `{S2, C, P}`). The note's `line_object_ids` are not part of the
link — line objects sync as first-class objects and already carry `commit_sha`.

## Identity

- Every object carries the client-minted ULID from its spec. **Client ids are
  authoritative and are never regenerated server-side.** Platform rows are keyed
  `(tenant_id, id)`.
- Idempotency = upsert on id, governed by the write rules below. Re-posting an
  identical batch is a no-op by construction.
- The same id may legitimately arrive from two different authenticated users of
  one tenant (sessions travel via git refs and may be re-imported by a
  teammate). This is the central scenario the write rules exist for.

## Write rules

These four properties are protocol semantics, not server implementation detail.
A conforming server MUST implement all four; a conforming client MAY rely on
them.

1. **Sync never deletes.** A batch means "objects I have," never "the complete
   set." Absence of an object, a turn, or a commit SHA removes nothing.
2. **Immutable objects are write-once, verified by content hash.** Turns are
   immutable events. On first insert the server stores
   `content_hash = sha256(canonical serialization)` (see
   [Content hash](#content-hash)). Re-upload of an existing id: matching hash →
   **`noop`**; differing hash → **`rejected` (`hash_mismatch`)** — for an
   immutable type that is corruption or tampering, never a merge.
3. **Container fields merge monotonically.** Per conversation:
   `commit_shas` = set union; `ended_at` = max; turn set = grow-only union (each
   arriving turn individually subject to rule 2); `metadata` = first-write-wins
   per key. Every merge function is order-independent: Alice-then-Bob and
   Bob-then-Alice converge to identical state.
4. **Forks are new identities.** A fork is a new conversation ULID with
   `parent_session_id` pointing at its origin. The server treats
   `parent_session_id` as an opaque immutable field; the parent need not exist
   on the platform (it may be private or not yet synced).

Line objects are the one mutable exception: `remap` legitimately rewrites
`commit_sha` after a rebase. Policy: latest write wins whole-object (derived,
recomputable data; low stakes).

## Content hash

`content_hash` = lowercase hex SHA-256 of the object's **canonical
serialization**: the object's JSON document as defined by its schema (fields
absent when optional-and-empty, exactly as `lineage-core` emits), canonicalized
per **RFC 8785 (JCS)** and encoded as UTF-8.

- The reference implementation lives in `lineage-core`; golden conformance
  vectors in `specs/conformance/` are executed by both the Rust and TS test
  suites (vectors and implementation land together — import-plan task 8).
- The server always computes the hash itself from the received document. The
  client MAY include `content_hash` on a turn; if present the server MUST
  verify it and reject the turn (`hash_mismatch`) on disagreement — this turns
  silent transport/encoding corruption into a loud error.
- Hash scope v0: turns. Conversations are mutable containers (rule 3) and line
  objects are latest-wins; neither is hash-verified in v0.

## Blob transfer

The local LFS object is the unit of blob transfer and maps 1:1 onto the
platform's `blob_ref`:

- **Identity:** bare lowercase sha256 hex on the wire. Local references
  (`lfs:sha256:<hex>`, `sha256:<hex>`, pointer-file `oid sha256:<hex>`)
  normalize to the bare digest. Platform stores `blob_ref(sha256, byte_size,
  content_type, storage_key)`.
- **Flow:** the batch's `blobs[]` manifest declares every blob the batch's
  objects reference (`sha256`, `byte_size`, optional `content_type`). Blob
  content is uploaded via `PUT /v0/blobs/{sha256}` — idempotent; the server
  verifies the digest of the received bytes and rejects on mismatch. Order
  (blobs before or after the batch) is unspecified; a batch whose manifest
  entries are not yet uploaded is still accepted, with those blobs reported
  `pending` in the response. v0 has no have/want negotiation: clients MAY
  upload unconditionally; servers treat re-uploads as `noop`.
- **Tiering threshold is a protocol parameter.** Content ≤ threshold stays
  inline in the turn document (`content`, `Artifact.blob_ref` absent); content
  above it is externalized and referenced. The v0 value is **1 MiB
  (1,048,576 bytes)** — identical to the local `large_blob_backend` default —
  so the client never re-tiers at sync time. A client whose local threshold
  differs syncs whatever tiering its documents already have; the server accepts
  inline content up to the protocol threshold and rejects larger documents.
- Blobs are content-addressed and immutable; per-blob privacy follows the
  objects referencing them (a blob referenced only by unsynced/private objects
  is never uploaded).

## Privacy

- Objects with `private: true` are **never exported** by the client — the
  export path filters them before any batch is assembled. Client-side filtering
  is the guarantee.
- The server MUST also reject any received object with `private: true`
  (`rejected`, reason `private`) — defense-in-depth, not the mechanism of
  privacy.
- Redaction is client-side and happens before persistence locally
  (policy-before-persist); the protocol carries only already-redacted content.
  Server-side scanning is the platform's hygiene concern, out of protocol
  scope.

## Repo binding

Every batch carries the repo identity hints; the **server** owns resolution:

- Client sends `repo: { normalized_remote_url, root_commit_sha }`.
  - `normalized_remote_url`: origin remote URL with scheme/login stripped to
    `host/owner/name` lowercase, `.git` suffix removed
    (e.g. `github.com/lineage-context/lineage-platform`).
  - `root_commit_sha`: the SHA of the repository's root commit (first parentless
    commit reachable from the default branch).
- The server resolves the forge's numeric repo id using its own forge
  connection (the user's GitHub token server-side) — the CLI never needs a
  forge API credential. Unknown repo → implicit first-push registration.
  Resolution precedence: forge numeric id (primary), then
  `(root_commit_sha, normalized_remote_url)` as fallback/hints.
- The server returns the platform `repo_id` in the response; the client SHOULD
  cache it in local git config (not committed — avoids fork inheritance) and
  send it as `repo.platform_repo_id` on subsequent syncs.

## Batch processing and responses

- A batch is an unordered set. The server MUST NOT depend on intra-batch order
  and MUST achieve the same final state for any permutation (guaranteed by the
  write rules).
- Internal references may be dangling (a line object whose conversation is
  private and absent, a link to a not-yet-synced conversation). The server
  accepts and stores them; referential assembly is a read-side concern.
- Per-object results; one object's rejection does not abort the batch.
  Persistence of the accepted subset is atomic (a client retrying after a
  network failure re-posts the same batch; idempotency makes this safe).
- Response (`sync-response-v0`): `repo_id` plus one result per object:
  `{ kind, id, status, reason? }` where `status ∈ accepted | noop | rejected |
  pending` (`pending` is blob-manifest-only) and `reason` (rejections only)
  `∈ hash_mismatch | private | schema_version | too_large | invalid`.
- HTTP codes: `200` (processed, inspect per-object results), `400` (batch
  undecodable), `401`/`403` (auth/tenant), `413` (batch size). Servers MUST NOT
  use HTTP errors for per-object outcomes.

## Evolution

- Protocol version is the URL prefix (`/v0/`); object schema versions travel in
  each document's `schema_version`.
- Within v0, evolution is additive only: new optional-with-default fields, enum
  vocabularies grow, nothing renamed or removed
  ([decisions/0001](decisions/0001-contract-bindings-pipeline.md)). Servers
  MUST ignore unknown fields; clients MUST tolerate unknown enum values in
  responses.
- Old client / new server is the supported skew direction. A server MAY refuse
  a `schema_version` it no longer accepts (`rejected`, `schema_version`); that
  is the only permitted breaking lever inside v0.

## Hash-chained identity: evaluated, deferred

The v1 candidate model — turns as a hash chain, a session id as a *ref* to a
tip hash, fork = new ref, server accepting only fast-forward ref moves — was
evaluated for adoption in this protocol:

- **For:** maximal tamper-evidence (a chain head attests the whole history);
  automatic structural dedup of shared fork prefixes; "git for conversations"
  is philosophically on-brand; server-side verification becomes a single
  fast-forward check instead of per-object rules.
- **Against:** it rewrites the local tool's identity model — every adapter,
  ref layout, and the remap path mint and consume ULIDs today; the chain must
  be canonicalized exactly like `content_hash` (same parity problem, larger
  surface); stale-copy re-imports (the everyday teammate case) become
  non-fast-forward pushes needing a merge protocol that monotonic container
  merge already provides for free; and none of the slice's correctness
  requirements need it — properties 1–4 above already deliver convergence,
  safety, and fork lineage.

**Decision: deferred.** v0 ships ULID + write-once-hash + monotonic merge.
What v0 does to avoid foreclosing v1: turns are already immutable and
content-hashed (the chain's leaves exist); `parent_session_id` already forms
the fork DAG; the response vocabulary and additive evolution rules leave room
for a `sync-batch-v1` that negotiates ref heads. Revisit when tamper-evidence
becomes a product requirement or fork-heavy teams make prefix dedup
material — re-evaluate at sync-protocol v1, with this section as the starting
point.

## Conformance checklist (ingest implementer)

A platform ingest implementation is conforming when:

1. Double-posting any batch yields all-`noop` and zero row changes.
2. A turn re-uploaded with altered content is rejected `hash_mismatch`; the
   stored turn is unchanged.
3. A stale teammate copy (fewer turns, missing `ended_at`, fewer
   `commit_shas`) removes and regresses nothing, in either arrival order.
4. `private: true` anywhere in the batch → that object rejected `private`,
   rest of the batch unaffected.
5. An unknown repo auto-registers; a known repo resolves to the same `repo_id`
   from any member of the tenant.
6. Blob re-upload is `noop`; digest mismatch on upload is rejected; a manifest
   entry without uploaded content reports `pending`.
7. `uploaded_by_user_id` comes from the verified token; prompter fields from
   conversation metadata are stored as display-only passthrough.
