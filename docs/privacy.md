# Privacy and policy

[← Documentation index](README.md) · [Configuration](configuration.md) · [Share](share.md)

Lineage imports agent transcripts from your machine into git objects. Policy runs **before** anything is persisted, so secrets and sensitive paths should never reach `refs/lineage/*` in their raw form. You still control what gets pushed and what teammates can read.

## Policy before persist

Every import passes through the policy engine:

1. **Redaction** — regex rules replace likely secrets in turn text and tool arguments.
2. **Path excludes** — artifacts matching configured globs are dropped.
3. **Content excludes** — entire turns matching content patterns are cleared.
4. **Private sessions** — source paths matching `private_session_patterns` are marked private.

Default redaction uses the vendored [gitleaks](https://github.com/gitleaks/gitleaks) rule set (regex + entropy + allowlists). Default path excludes cover `.env`, credentials files, keys, and similar paths.

## Private sessions

A session becomes private when:

- Its transcript source path matches a `private_session_patterns` glob (default: `*private*`), or
- It is explicitly marked private in metadata.

Private sessions still exist in the manifest for your repo, but turn content may be stripped on export when `strip_private_on_export` is true (the default).

To keep a session local in practice, avoid pushing `refs/lineage/*` until you have reviewed exports, or delete the session with `git lineage delete`.

## Export and sharing

Before pushing lineage refs publicly or to a broad team audience:

```bash
git lineage export --redact --format jsonl > review.jsonl
```

Review the file for content you are comfortable sharing. `--redact` applies export-time policy (including private session stripping).

Sharing refs themselves:

```bash
git lineage lfs push
git push origin refs/lineage/* refs/notes/lineage
```

Anyone with repository access can read pushed lineage data. Treat pushed refs like source code: only share what you intend to be team-visible.

## What lineage reads locally

Import scans agent transcript directories on disk (see [Agent paths](agent-paths.md)). It does not upload transcripts to a Lineage cloud service. Normalized, policy-filtered JSON is written into your local git object store.

## What teammates see

After fetch, teammates get the same conversation blobs, notes, and line objects you pushed. Search and blame operate on shared refs. Local search indexes (`.git/lineage/index.db`) are rebuilt per machine and are not pushed.

## Images and large artifacts

Image and large text artifacts may live in Git LFS. They follow the same path excludes and redaction rules as inline content. Use `git lineage show <id> --hydrate-images` only in trusted environments when reviewing media.

## Configuration levers

| Concern | Configuration |
|---------|---------------|
| Drop credential file artifacts | `exclude_paths` |
| Clear turns mentioning secrets | `exclude_content_patterns` |
| Mark planning sessions private | `private_session_patterns` |
| Empty private sessions on export | `strip_private_on_export` |

Details: [Configuration](configuration.md).

## Operational hygiene

- Run `git lineage doctor` after changing policy or cloning a repo with lineage refs.
- Use `git lineage delete <id> --purge-blobs` to remove a session and refcount-aware LFS data when a conversation should not remain in history.
- Run `git lineage gc` periodically to drop orphan line objects and unreferenced blobs.
- Do not paste raw `git lineage show` or export output into public issue trackers.

## Reporting security issues

See [SECURITY.md](../SECURITY.md). Do not file public issues for vulnerability reports.

## Related guides

- [Share with your team](share.md)
- [Maintenance](maintenance.md) — delete and garbage collection
- [Import](import.md)
