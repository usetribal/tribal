---
name: lineage-release-prep
description: >-
  Prepares Lineage releases and merge-ready PRs: CHANGELOG versioning,
  coverage gate, docs lint, export redaction review, and CI checklist. Use when
  cutting a release, preparing v0.x.0, or finalizing a large PR for merge.
---

# Lineage release prep

## Pre-merge checklist

```bash
make check
./scripts/coverage.sh
make msrv
npx markdownlint-cli2
```

CI also runs: Linux + macOS tests, `cargo doc`, typos, VS Code `npm run check`.

## CHANGELOG

1. Move `[Unreleased]` items to a new version section with date
2. Keep [Keep a Changelog](https://keepachangelog.com/) format
3. Update compare links at bottom of `CHANGELOG.md`

## Security before public push

```bash
git lineage export --redact --format jsonl > /tmp/review.jsonl
# Review for leaked secrets/conversation content
```

Verify `refs/lineage/config` redaction and private-session patterns.

## Sharing lineage refs

```bash
git lineage lfs push
git push origin refs/lineage/* refs/notes/lineage
```

Document breaking changes to ref layout or config in CHANGELOG + specs.

## Version bumps

Workspace version: `Cargo.toml` `[workspace.package] version` + `extensions/vscode/package.json` if extension ships together.

## PR template

Confirm CONTRIBUTING checklist: specs, coverage, CHANGELOG, no secrets.
