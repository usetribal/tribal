# Profiler harnesses

Performance harnesses for tribal surfaces — **development only**, not part of release
installers or `cargo-dist` artifacts. **Runtime** emission (`TRIBAL_TUI_PROFILE=1`) lives in
the [`lineage-profiler`](../../crates/lineage-profiler) crate behind the `profile` feature on
`lineage-cli`; **headless measurement** lives in
[`lineage-profiler-harness`](../../crates/lineage-profiler-harness) (`publish = false`).

Use the [`profiler`](../../../.agents/skills/profiler/SKILL.md) agent skill for workflows and
env vars when this tree lives inside a larger repo). In an `oss/`-only checkout, copy or read
that skill from the enclosing repository, or follow the commands below.

## Layout

| Path | Surface | Status |
| --- | --- | --- |
| `crates/lineage-profiler-harness/src/cli/` | `tribal` TUI prep | Implemented |
| `crates/lineage-profiler-harness/src/select/` | ratatui terminal phases | Implemented |
| `harness/profiler/web/` | Product web app | Reserved — add Playwright/timing harnesses here |

Add a new surface under `lineage-profiler-harness/src/<surface>/`, export from `lib.rs`, and
extend the skill’s harness table.

## CLI — headless prep breakdown

```bash
cd oss
TRIBAL_TUI_PROFILE_REPO=/path/to/git/repo \
  cargo test -p lineage-profiler-harness tui_prep_breakdown -- --ignored --nocapture
```

Warm second run: `tui_prep_breakdown_warm_second_run`.

## CLI — ratatui terminal (needs a TTY)

```bash
cd oss
script -q /dev/null cargo test -p lineage-profiler-harness ratatui -- --ignored --nocapture
```

## Live run (interactive terminal)

Build with profiling enabled, then:

```bash
cargo install --path crates/lineage-cli --features profile
TRIBAL_TUI_PROFILE=1 tribal list
```

JSON timing lines go to **stderr** (prep from the CLI path; ratatui phases from the selector loop).
Release installs from `usetribal.io/install.sh` do not include this code path.
