# Git hooks

[← Documentation index](README.md) · [Import](import.md) · [Configuration](configuration.md)

Tribal can install two git hooks that keep imported sessions current and linked to commits without manual import commands.

## Install and remove

```bash
git lineage init --hooks
git lineage init --hooks --force   # overwrite existing hook files
git lineage init --uninstall
```

`git lineage init` offers hook installation in the setup wizard. Use `--force-hooks` / `--force` on init to replace existing hook scripts.

## Hook behavior

| Hook | When | Action |
|------|------|--------|
| `pre-commit` | Before commit is created | Incremental import: `import --agent all --no-link-head --incremental` |
| `post-commit` | After commit is created | Link recently imported sessions to the new `HEAD` commit |

Pre-commit import deliberately avoids linking to the in-progress commit (`--no-link-head`). Post-commit linking attaches sessions that arrived since the last commit.

Import failures on pre-commit log to stderr but **do not block** the commit by default — your code changes always land even if agent history import fails.

## Requirements

- `git-lineage` or `git lineage` on PATH inside hook environment (`~/.cargo/bin` is prepended in hook scripts).
- Repository initialized with lineage config (`git lineage init` or `init-config`).

## Contributor repos vs application repos

The Tribal **monorepo** uses separate contributor hooks (format + lint) via `make install-hooks` and `core.hooksPath .githooks`. Application repositories use lineage import hooks from `git lineage init --hooks`.

Do not assume both hook systems on the same repo without merging scripts manually.

## VS Code

**Lineage: Install Git Hooks** from the command palette runs `git lineage init --hooks`.

## When hooks are not enough

- One-off full backfill: `git lineage import --agent all` without `--incremental`.
- Import without linking: `--no-link-head` manually, then `git lineage link`.
- Disable auto import: `uninstall-hook` and import on demand.

## Related guides

- [Import](import.md)
- [Maintenance](maintenance.md) — manual link and materialize
- [Developing](developing.md) — contributor hook setup
