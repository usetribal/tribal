// Generates src/generated/ zod bindings from the canonical JSON Schemas in
// specs/schema/ (which are themselves generated from the lineage-core Rust
// types — see the schema_snapshot test there). With --check, verifies the
// committed bindings match what generation would produce, without writing.
import { mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { basename, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { jsonSchemaToZod } from "json-schema-to-zod";

const here = dirname(fileURLToPath(import.meta.url));
const schemaDir = resolve(here, "../../../specs/schema");
const outDir = resolve(here, "../src/generated");
const checkOnly = process.argv.includes("--check");

const header =
  "// GENERATED FILE — do not edit.\n" +
  "// Source: specs/schema (canonical: lineage-core Rust types).\n" +
  "// Regenerate with: pnpm --filter @lineage/contracts generate\n\n";

const lowerFirst = (s) => s.charAt(0).toLowerCase() + s.slice(1);

// json-schema-to-zod emits z.any() for unresolved $refs, so internal
// "#/$defs/..." pointers are inlined before conversion. The committed
// .schema.json artifacts keep their $defs; only this conversion flattens them.
const dereference = (node, defs, seen = new Set()) => {
  if (Array.isArray(node)) {
    return node.map((item) => dereference(item, defs, seen));
  }
  if (node === null || typeof node !== "object") {
    return node;
  }
  if (typeof node.$ref === "string") {
    const match = node.$ref.match(/^#\/\$defs\/(.+)$/);
    if (!match || !(match[1] in defs)) {
      console.error(`unresolvable $ref: ${node.$ref}`);
      process.exit(1);
    }
    if (seen.has(match[1])) {
      console.error(`recursive $ref not supported: ${node.$ref}`);
      process.exit(1);
    }
    return dereference(defs[match[1]], defs, new Set([...seen, match[1]]));
  }
  return Object.fromEntries(
    Object.entries(node)
      .filter(([key]) => key !== "$defs")
      .map(([key, value]) => [key, dereference(value, defs, seen)]),
  );
};

const schemaFiles = readdirSync(schemaDir)
  .filter((f) => f.endsWith(".schema.json"))
  .sort();

if (schemaFiles.length === 0) {
  console.error(`no schemas found in ${schemaDir}`);
  process.exit(1);
}

const expected = new Map();
const indexLines = [];

for (const file of schemaFiles) {
  const schema = JSON.parse(readFileSync(resolve(schemaDir, file), "utf8"));
  if (!schema.title) {
    console.error(`${file} has no title; cannot derive binding names`);
    process.exit(1);
  }
  const moduleName = basename(file, ".schema.json");
  const code = jsonSchemaToZod(dereference(schema, schema.$defs ?? {}), {
    name: `${lowerFirst(schema.title)}Schema`,
    module: "esm",
    type: schema.title,
  });
  expected.set(`${moduleName}.ts`, header + code);
  indexLines.push(`export * from "./${moduleName}.js";`);
}
expected.set("index.ts", header + indexLines.join("\n") + "\n");

let drifted = false;
for (const [file, content] of expected) {
  const path = resolve(outDir, file);
  if (checkOnly) {
    let committed;
    try {
      committed = readFileSync(path, "utf8");
    } catch {
      committed = null;
    }
    if (committed !== content) {
      console.error(`drift: src/generated/${file} does not match specs/schema`);
      drifted = true;
    }
  } else {
    mkdirSync(outDir, { recursive: true });
    writeFileSync(path, content);
  }
}

if (drifted) {
  console.error("regenerate with: pnpm --filter @lineage/contracts generate");
  process.exit(1);
}
