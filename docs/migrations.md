# Migrations

How a change that breaks state already on a user's machine gets shipped.

The CLI persists things outside its own binary: a config directory, keys in
`.git/config`, hook scripts in `.git/hooks/`, agent skill files in the worktree.
A release that renames or reshapes any of these leaves every existing
installation holding state the new version does not recognise. A **migration**
is how that state is carried across.

## The rule

**A change to anything the CLI persists registers a migration in the same pull
request.** Not a follow-up, not a release note telling people to re-run setup —
a migration, landing with the change that made it necessary.

This applies to:

- the config directory and the files in it
- git config keys the CLI reads or writes
- hook scripts, and the marker comments that identify them
- bundled agent skills installed into a repository
- the shape of any file the CLI writes and later reads

It does not apply to derived state. An index or a cache is rebuilt, not
migrated — `rebuild` already exists for that, and re-deriving is always cheaper
and safer than converting.

## Writing one

Migrations live in `crates/lineage-cli/src/migrate.rs`. Add a `Migration` to
`MIGRATIONS` with an id, the version that introduced it, and its steps:

```rust
Migration {
    id: "0002-something",
    introduced_in: "0.6.0",
    steps: &[Step { id: "…", detect, apply }],
}
```

`introduced_in` is the release the change ships in. It is recorded with every
applied step, so the record reads as a version timeline.

### Steps are the unit

Split a migration into steps along the boundaries of what it touches — one per
substrate, not one per file. Each step is detected, applied and recorded on its
own.

This is what makes a run **resumable**, which matters because it cannot be
atomic. A home directory, `.git/config`, a hooks directory and a worktree share
no transaction: once a step has moved a directory, a later failure cannot put it
back. Recording each step as it lands means an interrupted run leaves the
completed work marked done, and running `upgrade` again finishes the rest
instead of starting over.

**Order steps by what is at stake.** The step carrying real data runs first,
while the least has changed; cheap re-runnable ones run last. In `0001` the
config directory moves before hooks are re-stamped, because a half-migrated
login is worth more than a half-stamped hook.

### Every step must be idempotent

A step that has nothing to do says so and succeeds. It never fails, and it never
assumes it is running first.

For a rename, write the three cases out rather than inferring them:

| Found | Do |
| --- | --- |
| old only | move it |
| both | keep the new one, leave the old, say so |
| new only | nothing |

Never express a rename as "make the new thing exist" — that reads as
drop-and-add, and destroys whatever the old name held. The "both" case is a
real state a user can reach, and picking one to delete is not a decision a
migration makes silently.

### Detection changes nothing

`detect` answers whether there is work, without doing any. `upgrade --dry-run`
reports exactly what `detect` finds, so a detector with a side effect makes the
preview a lie.

### Failing one repository must not fail the run

Steps that walk repositories (from the registry, plus the current one) skip and
warn on a checkout that has been deleted, moved, or has hooks the user wrote
themselves. One stale entry cannot be allowed to block a machine from
migrating.

## Testing one

`crates/lineage-cli/tests/migrate_workflow.rs` is the pattern. Cover:

- the ordinary case — old state present, converted
- **each idempotency case** — old only, both, new only
- a second run finding nothing to do
- **resumability** — a step applied outside the runner is recorded on the next
  run, and nothing is repeated
- anything the step must *not* touch: a hook the user wrote, a repository that
  never opted into skills

Tests that drive `HOME` must serialise on a lock and restore it: the variable is
process-global, and a leak sends another test at the developer's real
configuration.

## Escape hatches

- `tribal upgrade --dry-run` — report what would run, change nothing
- `tribal upgrade` — apply; safe to re-run at any point
- `tribal rebuild` — re-derive index and derived state from stored sessions

State that cannot be converted is re-derivable: sessions live in
`refs/lineage/*`, and rebuilding is always available as the floor.

## What is deliberately not migrated

Anything that can reach a remote. `refs/lineage/*` is wildcard-pushed, and
`.lineage/media` is tracked in the worktree behind a committed `.gitattributes`
filter — renaming either would rewrite history in every clone and break
already-pushed objects for everyone else. Names that travel are permanent;
migrations are for state that stays on one machine.
