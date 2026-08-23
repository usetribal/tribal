---
name: lineage-cli-command
description: >-
  Adds or changes tribal CLI subcommands in lineage-cli. Covers lib.rs
  handlers, main.rs clap wiring, hooks_cmd, ui.rs presentation, and integration
  tests in tests/workflow.rs. Use when adding CLI commands or flags, or when
  changing tribal stdout / stderr formatting.
---

# Tribal CLI command

## Structure

| File | Role |
|------|------|
| `src/main.rs` | `clap` `Commands` enum + dispatch |
| `src/lib.rs` | Crate root — exports `commands`, `hooks_cmd`, `init_cmd`, `skill_cmd`, `ui` |
| `src/commands.rs` | Command handler implementations |
| `src/ui.rs` | Human-facing stdout — the only layout language |
| `src/init_cmd.rs` | Interactive `init` wizard (box-drawing chrome only) |
| `src/skill_cmd.rs` | `init-skill` — bundled skill to `.cursor/`, `.claude/`, `.agents/` |
| `src/hooks_cmd.rs` | `install-hook`, `uninstall-hook`, `post_commit` |
| `tests/workflow.rs` | Integration tests for CLI flows |
| `tests/hooks_workflow.rs` | Hook install/uninstall tests |

## Human output

All human-facing stdout goes through `crate::ui`. Do not `println!` a new layout
in a command module. Clippy denies `print_stdout` and `use_debug` on the crate;
`./scripts/check-cli-ui.sh` (in `make check`) rejects leftover `println!` and a
second colour crate.

Four kinds — pick the one that matches the command:

| Kind | Helpers | Use for |
| --- | --- | --- |
| **Scan list** | `ScanRow`, `print_scan_rows`, `format_scan_rows` / `format_scan_row` | `list`, search hits, picker labels |
| **Detail** | `heading`, `kv` / `kv_width`, `section` | `show`, `blame`, doctor, `lfs status` |
| **Action** | `action`, `indent`, `hero` | import, sync, pull, share, fork, login |
| **Empty** | `empty` | zero-result paths that name the fix |

Rules:

- **Title first** on scan rows; one line (inquire uses `format_scan_rows` as Select labels and paints them itself — do not embed ANSI in those strings).
- **Human labels**, not raw metadata keys (`Author`, not `prompted_by_email`).
- **Dates:** `ui::day` (`YYYY-MM-DD`) in lists; `ui::human_date` in detail.
- **Roles / confidence:** `ui::role_name` / `ui::turn` / `ui::confidence_name` — never Debug (`{:?}`). Turns: user cyan, assistant green, tool dim, system yellow. Ranked hits: `ui::ranked_hit` (number cycles those colours).
- **Colour:** `anstyle` + `anstream` only, on every human command. Honour `NO_COLOR` / TTY via `ui::color_enabled`. No `--color` flag. Do not add `owo-colors`, `colored`, `comfy-table`, or `tabled`. `action` is bold; `row` / `warn` / `rank_label` / `ok` / `caution` colour list and status chrome; clap `--help` uses the same cyan/green.
- **Logo:** `ui::banner` on root help and interactive init only. Mark above the collar; title + tagline inside. Tagline does not say git.
- **Machine output** stays machine-shaped: `ui::json` / `ui::jsonl` for `--json` and `--discover`; `ui::raw` / `ui::raw_line` for `context hook` JSON and `fork --brief`. Field names stay stable — schema changes need the `schema-change` skill.
- **Leave alone:** `init` wizard boxes (`init_cmd.rs`), progress bars (`progress.rs`, stderr), `eprintln!` errors / hook logs.

## Key commands

- `init` — interactive wizard; `--yes` for non-interactive; `--no-import` / `--no-ingest` to skip import
- `init-config`, `init-skill` — individual setup steps (also run from `init`)
- `import` — alias `ingest`; `--incremental`, `--no-link-head`

Bundled end-user skill: `assets/skills/lineage/SKILL.md` (not the contributor skills under repo `.cursor/skills/`).

## Add a subcommand

1. Add variant to `Commands` in `main.rs` with `clap` args
2. Implement handler in `src/commands.rs` (public fns for integration tests)
3. Print through `ui` — scan / detail / action / empty, or `ui::json` if `--json`
4. Dispatch in `main.rs` `match` — use `--repo` global arg via `repo_path()`
5. Add integration test in `tests/workflow.rs`
6. Document in `docs/cli/README.md` command reference table
7. `CHANGELOG.md` under `[Unreleased]` if user-facing

## Testing

```bash
cargo test -p lineage-cli
cargo test -p lineage-cli --test workflow
cargo test -p lineage-cli --test hooks_workflow
./scripts/check-cli-ui.sh
```

Use `tempfile::TempDir` + `git init` for repo-scoped commands.

Hook tests need filesystem permissions (not sandbox-restricted).

## Binary names

- Installed: `tribal` (invoked directly)
- Library crate: `lineage-cli` with `[lib]` for integration tests
