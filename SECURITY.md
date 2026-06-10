# Security Policy

## Supported versions

| Version | Supported          |
| ------- | ------------------ |
| 0.1.x   | :white_check_mark: |

## Reporting a vulnerability

**Please do not report security vulnerabilities through public GitHub issues.**

Instead, report them by email to [security@lineage.dev](mailto:security@lineage.dev).

Include:

- A description of the vulnerability
- Steps to reproduce
- Potential impact
- Any suggested fix (optional)

You should receive a response within 72 hours. If the issue is confirmed, we will:

1. Work on a fix in a private branch
2. Coordinate disclosure timing with you
3. Credit you in the release notes (unless you prefer anonymity)

## Security considerations

Lineage handles agent conversation data that may contain secrets. Built-in mitigations:

- **Policy engine** — redacts API keys, tokens, and env-file patterns before persistence
- **Path excludes** — `.env`, credentials, keys, and PEM files are excluded by default
- **Export redaction** — `git lineage export --redact` strips sensitive content
- **MCP responses** — session data is redacted by default in MCP tool output

### Recommendations for users

- Run `git lineage export --redact` before sharing lineage data
- Review imported sessions before pushing `refs/lineage/*` to a remote
- Do not import sessions from untrusted sources without reviewing policy rules
- Keep `git-lineage` and `lineage-mcp` updated

## Scope

The following are in scope for security reports:

- Secret leakage through import, export, or MCP tools
- Path traversal or arbitrary file read via adapters
- Injection in search queries or MCP tool arguments
- Unsafe deserialization of conversation blobs

Out of scope:

- Vulnerabilities in upstream agent tools (Cursor, Claude, Codex)
- Issues requiring physical access to the developer machine
- Social engineering
