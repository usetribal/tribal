# Specs

The contract surface of Lineage, in three parts:

- **`schema/`** — generated JSON Schema, the canonical artifact other languages
  consume. Generated from the `lineage-core` Rust types (the canonical
  definition) and snapshot-tested there: after an intentional type change, run
  `LINEAGE_UPDATE_SCHEMAS=1 cargo test -p lineage-core --test schema_snapshot`,
  then regenerate the TS bindings with
  `pnpm --filter @lineage/contracts generate`. Never edit by hand.
- **`*.md`** — narrative documentation of the same contracts: intent, examples,
  invariants that don't fit in a schema. Where prose and schema disagree, the
  schema wins.
- **`decisions/`** — durable records of contract-architecture decisions.

See [decisions/0001-contract-bindings-pipeline.md](decisions/0001-contract-bindings-pipeline.md)
for how the generation pipeline fits together and the evolution rules.
