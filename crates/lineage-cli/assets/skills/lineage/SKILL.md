---
name: lineage
description: >-
  Retrieves engineering context from git-lineage: prior AI agent conversations,
  architecture decisions, and line-level provenance stored in the repository.
  Use when answering why code exists, what prompt produced a change, past design
  decisions, onboarding context, or before modifying unfamiliar code.
---

# Lineage — engineering conversation context

This repository stores **AI agent session provenance** in git (`refs/lineage/*`, `refs/notes/lineage`). Use it before guessing why code was written.

## When to use

- "Why was this implemented this way?"
- "What was the original requirement / prompt?"
- Architecture or design decision history
- Onboarding: what agents discussed about this module
- Before editing a file — check linked sessions and blame

## Prerequisites

```bash
git lineage doctor          # verify lineage is configured
git lineage ingest --agent all --incremental   # refresh if sessions may be new
```

## Retrieve context (prefer JSON for agents)

```bash
# Search conversations for a topic
git lineage search "authentication middleware"

# List sessions (optionally for one commit)
git lineage list --json
git lineage list --commit <sha> --json

# Full session transcript
git lineage show <session-id> --json

# Line-level: which turn(s) touched this line?
git lineage blame path/to/file.rs:42 --json

# Export all sessions (respect redaction)
git lineage export --redact --format jsonl
```

## How to use results

1. **Search first** for topic keywords (feature name, error message, ADR topic).
2. **Blame** when you have a specific file and line.
3. **Show** the best-matching session for full turn-by-turn context (user prompt + assistant reply + tool calls).
4. Check `metadata.architecture_summary` in session JSON when present.
5. Cite session id and turn content in your answer; do not invent history if lineage returns nothing.

## If nothing is found

- Run `git lineage ingest --agent all --incremental`
- Confirm hooks are installed: `git lineage install-hook`
- Teammates may need: `git fetch origin refs/lineage/* refs/notes/lineage`
- Fall back to git log / code comments — lineage does not replace git history

## MCP (optional)

If `lineage-mcp` is configured in your editor, prefer MCP tools for the same queries inside the agent loop.

## Privacy

Sessions may contain redacted content. Do not paste raw exports into public issues. Use `git lineage export --redact` before sharing outside the repo.
