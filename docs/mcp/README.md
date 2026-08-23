# MCP server

[← Documentation index](../README.md) · [CLI reference](../cli/README.md) · [Developing](../developing.md)

The Tribal MCP server exposes repository session data to AI tools via the [Model Context Protocol](https://modelcontextprotocol.io/). Agents can search, blame, and inspect conversations without shelling out manually.

## Install

```bash
cargo install --path crates/lineage-mcp
# or: make setup WITH_MCP=1
```

## Run

```bash
LINEAGE_REPO=/path/to/your/repo lineage-mcp
```

`LINEAGE_REPO` defaults to the current working directory. Point it at the git root that contains `refs/lineage/*`.

The server reads through the same git and policy layers as the CLI. It does not bypass redaction.

## Tools

| Tool | Description |
|------|-------------|
| `lineage_list_sessions` | List imported sessions |
| `lineage_get_session` | Fetch session by id (redacted by policy) |
| `lineage_blame_line` | Tribal for file path and line number |
| `lineage_search` | Full-text search over session content |
| `lineage_doctor` | Repository lineage health |
| `lineage_materialize` | Materialize line objects at HEAD or a commit |
| `lineage_rebuild_index` | Rebuild local search index from refs |
| `lineage_export` | Export sessions with optional redaction |
| `lineage_remap` | Remap notes after rebase |

### Traversal verbs

The moves an agent makes when the evidence it was handed is close but not
right. Each is read-only, privacy-gated, and bounded by an optional `limit`.

| Tool | Repairs |
|------|---------|
| `lineage_search_within` | Right sessions, wrong turns — searches the text of given `session_ids` in one call |
| `lineage_turns_around` | Right turn, missing its argument — the turns within `radius` of `turn_id` in its session |
| `lineage_produced_by` | Right turn, want its outcome — the code that turn produced |
| `lineage_sessions_for_commit` | Have a commit, want the reasoning — the sessions behind `commit_sha` |

These are the same four verbs the CLI exposes as `tribal context <verb>`,
defined once in `lineage-retrieval::VERBS`; registry tests assert neither
surface can gain or lose one without the other. `tools/list` is verb discovery
for free on this path, so MCP needs no equivalent of the CLI's `SessionStart`
hook.

### Continuing a session

| Tool | Description |
|------|-------------|
| `lineage_fork_brief` | A self-contained context block for `session_id` — whose session it was, what they asked for, the turns that changed code, the traversal verbs, and an empty task slot — for starting a subagent on it |

This is the brief half of `tribal fork` and **only** that half: it writes
no transcript and records no fork edge. The CLI's `fork` prints a command for a
human to run and can write a transcript into the harness's state directory; over
MCP there is nobody at a terminal to read a printed command, and writing a
colleague's session into the caller's harness as a side effect of a tool call is
a thing to choose rather than a thing to have happen. Private sessions — and
forks of private ones — are refused, the same gate the traversal verbs run.

The block embeds the *traversal* vocabulary and not the fork one: a subagent was
spawned to explore one session somebody already chose, and has no way to tell it
is already inside a fork.

`lineage-retrieval::CONTINUE_SESSION` registers this capability beside the
traversal verbs — deliberately not *inside* `VERBS`, since it takes a bare
session id rather than a `session#turn` handle and is not read-only — and the
same paired registry tests assert it cannot reach one surface without the other.

Not yet exposed via MCP: import, delete, gc (see project roadmap).

## Cursor configuration

```json
{
  "mcpServers": {
    "lineage": {
      "command": "lineage-mcp",
      "env": {
        "LINEAGE_REPO": "${workspaceFolder}"
      }
    }
  }
}
```

Restart the editor after installing the binary.

## Claude Code and Codex

Register `lineage-mcp` as an MCP server in your client configuration. Set `LINEAGE_REPO` to the repository you want agents to query.

## Agent workflow tips

- Prefer `lineage_search` before `lineage_get_session` to limit context size.
- Use `lineage_blame_line` on code under edit to load decision context.
- Run `lineage_remap` after the user rebases.
- Call `lineage_doctor` when tools return empty results unexpectedly.

## Development

```bash
cargo test -p lineage-mcp
cargo test -p lineage-mcp --test server
```

New tools should mirror CLI behavior and respect policy. Update this doc and [CLI reference](../cli/README.md) when adding capabilities.

## Related guides

- [Explore](../explore.md)
- [Privacy](../privacy.md)
- [Architecture](../ARCHITECTURE.md)
