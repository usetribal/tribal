# Release status & compatibility policy

Where Lineage currently sits on the release spectrum, and what that means for
backwards compatibility. **Agents and humans: read this before preserving old
behavior "for compatibility" — the discipline below is deliberately different
from a shipped product's.** Update this document when the status changes; it
is the single source of truth for the question "who could we break?"

## Current status (2026-07-19): pre-release, unpublished

- The `oss/` toolchain is **not yet published** — no crates.io releases, no
  public repository, no external installs.
- The platform has **no external customers**. The only people with live
  lineage data are the developers of lineage.
- Version numbers are `0.x`. Per [SemVer](https://semver.org) and the
  [Go modules v0 convention](https://go.dev/doc/modules/version-numbers),
  0.x makes **no stability or backwards-compatibility guarantees**.

## Policy while pre-release

**Simplicity beats compatibility.** When a cleaner shape conflicts with
keeping old data or old interfaces working, choose the cleaner shape:

- **Data**: no migrations required. `git lineage rebuild` and re-import are
  the upgrade path; developer repos that break can be re-derived or
  re-initialized. Losing derived state is always acceptable; losing stored
  conversations for the handful of dev repos is acceptable when the
  simplification warrants it (say so in the PR).
- **CLI/API surfaces**: rename and remove freely; deprecation aliases are a
  courtesy for the devs, not an obligation, and should be short-lived.
- **Wire/schema**: shapes may change in place under the same `-v0` name.

**What we keep anyway (the shape that lets us change discipline later):**

- `schema_version` stamps on every document and event — versioning machinery
  stays load-bearing even while v0 churns.
- Specs in `oss/specs/` stay authoritative and updated in the same PR as the
  change — history of *what* changed is preserved even though old data isn't.
- Rebuildability: derived state must always be recomputable from stored
  conversations + git history (`rebuild`), and capture must degrade openly,
  never corrupt.

**Optional fields**: optionality in a schema should reflect the **domain**
(the data can genuinely be absent — e.g. `ArtifactResolve.new_string` is
absent for harnesses whose edit tools expose no post-image), never *frozen
compatibility* with our own older captures. If a field is optional only
because old dev data lacks it, make it required and re-derive.

## What flips at each stage

Modeled on maturity-channel policies
([k6](https://grafana.com/docs/k6/latest/reference/versioning-and-stability-guarantees/),
[Gateway API](https://gateway-api.sigs.k8s.io/docs/concepts/versioning/)):

| Stage | Trigger | Discipline change |
|-------|---------|-------------------|
| **Pre-release** (now) | — | Everything above |
| **Published oss, pre-1.0** | `oss/` extracted + public | Breaking changes still allowed, but: CHANGELOG discipline becomes user-facing, `git lineage rebuild`/re-import must cover every local-data change (no "re-init your repo"), deprecation aliases live for one minor version |
| **Platform beta** (first external team) | External data in the pool | Server data becomes migration-only (no wipes); sync protocol changes become versioned (`-v1`), never in-place; local tooling keeps a compatibility window with the oldest supported server |
| **1.0 / GA** | Declared | SemVer honored in full; specs freeze per version; formal deprecation policy |

The stage-transition PRs must update this document and the relevant
`AGENTS.md` pointers.
