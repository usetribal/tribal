# Lineage (Open Source)

This directory is the **open-source portion of Lineage**: the local, single-engineer
toolchain — the Rust workspace, the editor extensions, and the **public contracts** that
define how Lineage data is shaped and how it syncs.

Everything here is intended to be public. It is developed inside a larger private workspace
today, but it is structured to be **self-contained** so it can be extracted into a standalone
public repository at any time with no rewrites (see [Repository model](#repository-model)).

---

## The one rule

> **Code in `oss/` may depend only on public contracts. It must never depend on the closed
> platform, and it must build and test in isolation.**

This is the invariant the whole layout exists to protect. Concretely:

- Dependency direction is **closed → public, never public → closed**. The platform may consume
  what's here; nothing here may reach into the platform.
- The only cross-boundary dependency permitted is on the **public contracts** in this directory.
- This subtree owns its own manifests (`Cargo.toml`, `pnpm-workspace.yaml`) and **must build from
  a clean checkout of `oss/` alone**. CI enforces this with an isolated build (see
  [Standalone build guarantee](#standalone-build-guarantee)). If that build fails, the boundary
  has been violated.

If you're about to add something here that needs platform internals, it does not belong here —
it belongs in the closed platform, talking to this code only through the public contracts.

---

## Layout

```
oss/
├── README.md              # this file
├── Cargo.toml             # Rust workspace root
├── pnpm-workspace.yaml    # TS workspace root
│
├── specs/                 # Canonical contract definitions (markdown). The source of truth.
│
├── contracts/             # Published, public contract artifacts
│   └── ts/                #   @lineage/contracts — TS types + zod schemas, generated from specs/
│
├── crates/                # Rust workspace members (mirrors the existing local workspace)
│   ├── lineage-core/      #   canonical types (incl. Rust contract types), pure domain logic
│   ├── lineage-policy/    #   redaction / policy engine (runs before anything is persisted)
│   ├── lineage-git/       #   the only crate touching git I/O
│   ├── lineage-adapters/  #   agent transcript adapters (Claude Code, Codex, Cursor)
│   ├── lineage-store/     #   large-content tiering
│   ├── lineage-search/    #   local, disposable SQLite index
│   ├── lineage-mcp/       #   MCP server (stdio JSON-RPC)
│   └── git-lineage/       #   the `git lineage` CLI
│
└── extensions/
    └── vscode/            # VS Code extension (TS) — shells out to the CLI
```

The exact crate set tracks the existing local Lineage workspace; the names above are the
current members. Keep the strict layering already established in that workspace: `lineage-core`
and `lineage-policy` at the bottom (no git, no IO), git/agent crates above, adapters/search/mcp
above those, and the CLI on top.

---

## Contracts

Contracts are the **only** thing the rest of the system — including the closed platform — is
allowed to depend on across the boundary, so they live here, in public.

- **`specs/` is canonical.** The markdown contracts are the single source of truth for the data
  schemas (Conversation, Artifact, LineObject, …) and the sync wire protocol. Language bindings
  are generated from or kept in sync with `specs/`, never the other way around.
- **TypeScript** bindings are published from `contracts/ts` as `@lineage/contracts` (types + zod
  schemas, and the sync/OpenAPI surface).
- **Rust** contract types live in `crates/lineage-core` (the existing serde types), which is also
  consumed downstream.

Consumers — including the closed platform — depend on contracts **by package name**
(`@lineage/contracts` via `workspace:*` today, a published version after extraction; the
`lineage-core` crate for Rust). **Never** reference contracts via a relative path that points
into or out of `oss/`. Path-based dependencies break the moment this directory is extracted; name-
based ones survive it.

If you change a contract, change `specs/` first, regenerate/realign the bindings, and treat it as
a versioned change — downstream code in another language and another repo depends on it.

---

## Polyglot setup

This subtree is both a Cargo workspace and a pnpm workspace, side by side:

- **Cargo** owns the Rust crates. `Cargo.toml` at the root defines the workspace members.
- **pnpm** owns the TypeScript packages (`contracts/ts`, `extensions/vscode`).
  `pnpm-workspace.yaml` at the root defines them.

Don't force the Rust into the JS task runner; let each toolchain manage its own language. Typical
commands:

```bash
# Rust
cargo build --workspace
cargo test  --workspace

# TypeScript
pnpm install
pnpm -r build
pnpm -r test
```

---

## Standalone build guarantee

The boundary is enforced by building this directory in isolation. Locally you can reproduce what
CI does:

```bash
# from a clean checkout containing ONLY oss/
cargo build --workspace
pnpm install && pnpm -r build
```

If either step needs anything outside `oss/`, the build fails — which means a dependency on the
closed platform (or a stray relative path) has crept in. Fix it by moving the offending code into
the closed platform, or by routing the dependency through a public contract.

This same isolated build is what makes extraction mechanical: if `oss/` builds alone, it can
become its own repository unchanged.

---

## Adding new code — where does it go?

Ask, in order:

1. **Does it need platform internals** (the cloud API's database, auth, server-side services)?
   → It belongs in the **closed platform**, not here. Have it talk to this code via public contracts.
2. **Is it part of the local, single-engineer experience** (a new agent adapter, a CLI command, an
   editor integration, local indexing)? → It belongs here. Implement against public contracts only.
3. **Is it a contract change?** → Edit `specs/` first, then regenerate bindings, then update consumers.

A useful test: _if this were already a public repo, would this code make sense in it, and would it
still build?_ If yes, it goes here. If it would drag in closed code, it doesn't.

---

## Repository model

This directory is developed as a **subtree** of a larger private monorepo and will be extracted to
a standalone public repository when ready, via `git subtree split`. Two consequences shape how you
work:

- **Nothing here may reference code outside `oss/`** (enforced by the isolated build). The closed
  platform depends inward on this code's published contracts; this code depends on nothing outward.
- **Extraction is one command** when the invariant holds. On extraction, `workspace:*` contract
  dependencies are swapped for published versions; no source changes are required.

Because of the subtree model, treat history and module boundaries here as if this were already the
public repo — because one day it will be.
