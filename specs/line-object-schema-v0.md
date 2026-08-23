# Line Object Schema v0

Maps source lines to the agent conversation turn that introduced or modified them.

## LineObject

```json
{
  "schema_version": "line-object-v0",
  "id": "01HQZX8K9V2M3N4P5Q6R7S8T9W",
  "file_path": "src/auth.rs",
  "line_range": [10, 25],
  "commit_sha": "abc123def4567890abcdef1234567890abcdef12",
  "conversation_id": "01HQZX8K9V2M3N4P5Q6R7S8T9U",
  "turn_id": "01HQZX8K9V2M3N4P5Q6R7S8T9V",
  "confidence": "exact",
  "metadata": {}
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `schema_version` | string | yes | Always `line-object-v0` |
| `id` | string | yes | ULID |
| `file_path` | string | yes | Repo-relative path |
| `line_range` | [u32, u32] | yes | Inclusive start, end (1-indexed) |
| `commit_sha` | string | yes | Commit where mapping applies |
| `conversation_id` | string | yes | Parent session |
| `turn_id` | string | yes | Turn that produced the change |
| `confidence` | string | yes | `exact`, `heuristic`, or `manual` |
| `metadata` | object | no | Extra context |

## Confidence levels

- **exact**: Derived from structured tool call (edit_file with line range)
- **heuristic**: Inferred from blame + session time overlap
- **manual**: User-linked via `tribal link`

## Many-to-one

One turn may produce many line objects. One line may have multiple objects across commits (history).

## Rebase behavior

On rebase, lineage attempts remap via:

1. Patch-id match on commit
2. File path + surrounding context at new commit
3. Fallback: mark `confidence: heuristic` and flag for rebuild
