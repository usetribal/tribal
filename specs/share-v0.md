# Share v0

How one conversation becomes a link anyone can open without an account, and what
a client may rely on when it fetches through that link. A server's share
endpoints and the CLI's `share` and `fork <share-url>` commands are both
implemented against this document.

Status: **draft for review** — written against `conversation-schema-v0` and
[sync-protocol-v0](sync-protocol-v0.md), whose wire shapes the share fetch
reuses verbatim.

## Scope

- **In scope:** minting a share for one already-synced conversation; the token
  format and how it is stored; the snapshot the share pins; the unauthenticated
  fetch and what it is allowed to return.
- **Out of scope, not foreclosed:** expiry, view counts, and revoke UX
  (`revoked_at` exists from v0; only the column is specified here); shares over
  a set of conversations; write access of any kind through a share.
- **Never in scope:** a share carrying more than the conversation it pins. A
  share token is not a credential for the tenant, the repo, or any other
  session.

## Token

- A share token is **~22 characters of base62** (`[0-9A-Za-z]`) encoding **128
  bits** from a cryptographically secure random source. The server mints it;
  it is never derived from the conversation id, the tenant, or a timestamp.
- The server stores **only `sha256(token)`** (lowercase hex). The plaintext
  token is returned once, in the share-create response, and is unrecoverable
  afterwards. A database disclosure therefore does not yield working links.
- Lookup is by hash: the server hashes the token from the URL and matches it
  against stored hashes. There is no enumeration order and no listing endpoint.
- **The URL is the credential.** Holding `/s/<token>` is sufficient and is the
  entire authorization for the fetch. Servers MUST serve share pages and share
  fetches with `X-Robots-Tag: noindex, nofollow` and a `Referrer-Policy` that
  does not leak the token to third parties.

## Snapshot pin

A share pins `(conversation_id, turn_count)` at creation time.

Turns are immutable and grow-only ([sync-protocol-v0](sync-protocol-v0.md)
write rules 2 and 3), so a count is a complete description of a prefix: turn
`N` never changes and turns only ever append. That makes the pair a true
immutable snapshot without copying any data.

Consequences a client may rely on:

- A fetch returns **exactly the first `turn_count` turns** in `idx` order,
  never more. Continuing the conversation after the share was created changes
  nothing for link holders.
- A share never becomes stale in a way that needs invalidation, and never
  becomes broader than it was at creation.
- Re-sharing the same conversation later mints a **new** token pinned at the
  then-current count. Shares are not updated in place.

## Endpoints

| Endpoint | Auth | Purpose |
|----------|------|---------|
| `POST /v0/shares` | Bearer token | Mint a share for one conversation; returns the token once |
| `GET /v0/shares/{token}` | **None** | Fetch the pinned conversation prefix |

### Create

Authenticated with the same bearer token as sync ([sync-protocol-v0
Transport](sync-protocol-v0.md#transport)), and scoped the same way: the
request names the repository by its normalized remote URL, and the workspace is
inferred from that repository's owner namespace against the caller's
memberships — the bearer token identifies the user, never a workspace, and
nothing in the request body can name one. The token's verified claims supply
the creating user. The request names the conversation within that repository;
the server reads its current turn count and pins it — a client never supplies
the count.

The server MUST refuse to share a conversation marked `private: true`, or one
descended from a private conversation, with a specific error rather than a
silent empty share. Privacy is not something a share may unset.

### Fetch

Unauthenticated by construction: presenting the token *is* the authentication.
The server hashes the presented token, resolves the share, and rejects when the
hash is unknown or `revoked_at` is set — both indistinguishable to the caller
(`404`), so a revoked link does not confirm that it once existed.

What a fetch returns is bounded three ways:

1. Only the pinned conversation — never a sibling, a parent, or a fork.
2. Only its first `turn_count` turns.
3. Only blob content that one of those turns references. **A share token must
   not become a general blob oracle**: content-addressed storage means a
   caller who learns a digest elsewhere would otherwise be able to read it
   through the share.

### Finding the API

A share link names the **web** origin — the page a receiver opens. Fetching the
share needs the **API** origin, and the two are not the same in most
deployments: one publishes the app at `app.<domain>` and the API at
`api.<domain>/api`, a development stack splits them across ports, and a
single-origin deployment serves both from one place under one prefix. None of
that is recoverable from the link, so a receiver must not guess it.

Every origin that serves share pages MUST therefore publish a discovery
document at `/.well-known/lineage.json`, served without authentication:

```json
{ "api": "https://api.example.com/api" }
```

`api` is the absolute origin the API is served from, **path prefix included**,
with no trailing slash. It is the base a client joins `/v0/shares/{token}` onto.
Servers MAY add fields; clients MUST ignore ones they do not know.

A client resolving a share link MUST try, in order:

1. An explicitly configured API origin, if the invocation supplies one.
2. The discovery document at the link origin's `/.well-known/lineage.json`.
3. A last-resort derivation from the link origin, so a deployment that predates
   discovery still resolves.

Step 3 exists only so an older server is not bricked, and MUST NOT be reached
when a discovery document is present. A client MUST tolerate the document being
absent, unparsable, or served as something other than JSON — single-page apps
commonly answer unknown paths with HTML rather than a 404 — and MUST treat any
of those as "not published" rather than as a failure to report.

## Wire shapes

The share fetch does **not** define a conversation encoding. It returns the
down-sync conversation shape from [sync-protocol-v0](sync-protocol-v0.md)
(conversation with embedded turns, blob-tiered content resolved the same way),
so a client that can consume a pull can consume a share with no new decoding
path. Only the share envelope — the pin and the sharing repo's identity, which
is what lets a receiver locate or clone the repo — is new.

Share-specific request and response shapes are published through the standard
pipeline ([decisions/0001](decisions/0001-contract-bindings-pipeline.md)) as
zod bindings in `@lineage/contracts`.

## Revocation

`revoked_at` is a nullable timestamp on the share record. Setting it kills the
link permanently; the same token is never re-enabled and never re-issued.
Revocation is a property of the share, not of the conversation: revoking one
share leaves other shares of the same conversation working.

## Privacy

The share path adds no new redaction stage. Content reaching a share was
already redacted client-side before it was synced ([sync-protocol-v0
Privacy](sync-protocol-v0.md#privacy)) — policy-before-persist means a share
can only expose what the sharer already chose to upload. What the share path
adds is the `private` refusal above and the scoping rules under
[Fetch](#fetch).

## Evolution

Protocol version is the URL prefix (`/v0/`). Within v0, evolution is additive
only, matching [sync-protocol-v0
Evolution](sync-protocol-v0.md#evolution): new optional-with-default fields,
growing enum vocabularies, nothing renamed or removed. Servers MUST ignore
unknown fields; clients MUST tolerate unknown enum values in responses.

## Conformance checklist (share implementer)

A share implementation is conforming when:

1. Two shares of the same conversation have different tokens, and neither token
   is derivable from the conversation id.
2. The stored record contains no value from which the token can be recovered.
3. A fetch after the conversation grew returns exactly the pinned turn count.
4. A fetch with a revoked token and a fetch with a never-issued token are
   indistinguishable to the caller.
5. Creating a share for a private conversation, or a descendant of one, fails
   with a specific error and creates no record.
6. A blob digest referenced only by turns beyond the pin, or by another
   conversation, is not readable through the token.
