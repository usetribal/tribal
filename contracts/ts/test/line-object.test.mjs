import assert from "node:assert/strict";
import { test } from "node:test";

import { lineObjectSchema } from "../dist/index.js";

// Emitted verbatim by lineage_core::LineObject::to_json — the Rust types and
// these bindings must accept the same documents.
const rustEmitted = {
  schema_version: "line-object-v0",
  id: "01KTXBF6D7SV69PWAKRTPWNDWX",
  file_path: "src/main.rs",
  line_range: [10, 24],
  commit_sha: "f3a9c2d1e8b7a6c5d4e3f2a1b0c9d8e7f6a5b4c3",
  conversation_id: "01JXG6BJ2M9QZJ4Y8W7V6T5R4E",
  turn_id: "01JXG6BJ3N0RAK5Z9X8W7V6T5S",
  confidence: "exact",
};

test("accepts a Rust-emitted line object", () => {
  const parsed = lineObjectSchema.parse(rustEmitted);
  assert.equal(parsed.confidence, "exact");
  assert.deepEqual(parsed.line_range, [10, 24]);
});

test("rejects an out-of-vocabulary confidence", () => {
  assert.throws(() =>
    lineObjectSchema.parse({ ...rustEmitted, confidence: "guessed" }),
  );
});

test("rejects a malformed line range", () => {
  assert.throws(() =>
    lineObjectSchema.parse({ ...rustEmitted, line_range: [10] }),
  );
});
