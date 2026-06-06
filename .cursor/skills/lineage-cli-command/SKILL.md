---
name: lineage-cli-command
description: >-
  Adds or changes git-lineage CLI subcommands in lineage-cli. Covers lib.rs
  handlers, main.rs clap wiring, hooks_cmd, and integration tests in
  tests/workflow.rs. Use when adding CLI commands, flags, or git lineage UX.
---

# Lineage CLI command

## Structure

| File | Role |
|------|------|
| `src/main.rs` | `clap` `Commands` enum + dispatch |
| `src/lib.rs` | Crate root — re-exports `commands`, `hooks_cmd` |
| `src/commands.rs` | Command handler implementations |
| `src/hooks_cmd.rs` | `install-hook`, `uninstall-hook`, `post_commit` |
| `tests/workflow.rs` | Integration tests for CLI flows |
| `tests/hooks_workflow.rs` | Hook install/uninstall tests |

## Add a subcommand

1. Add variant to `Commands` in `main.rs` with `clap` args
2. Implement handler in `src/commands.rs` (public fns for integration tests)
3. Dispatch in `main.rs` `match` — use `--repo` global arg via `repo_path()`
4. Add integration test in `tests/workflow.rs`
5. Document in `docs/cli/README.md` command reference table
6. `CHANGELOG.md` under `[Unreleased]` if user-facing

## Testing

```bash
cargo test -p lineage-cli
cargo test -p lineage-cli --test workflow
cargo test -p lineage-cli --test hooks_workflow
```

Use `tempfile::TempDir` + `git init` for repo-scoped commands.

Hook tests need filesystem permissions (not sandbox-restricted).

## Binary names

- Installed: `git-lineage` → invoked as `git lineage`
- Library crate: `lineage-cli` with `[lib]` for integration tests

## JSON output

Commands with `--json` must keep stable field names; schema changes need `schema-change` skill.
