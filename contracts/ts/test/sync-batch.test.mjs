import assert from "node:assert/strict";
import { test } from "node:test";

import { syncBatchSchema, syncResponseSchema } from "../dist/index.js";

// Emitted verbatim by serde from lineage_core::SyncBatch — the Rust types and
// these bindings must accept the same documents.
const rustEmitted = {
  schema_version: "sync-batch-v0",
  repo: {
    normalized_remote_url: "github.com/acme/widgets",
    root_commit_sha: "9c4b6f2e8a1d3c5b7e9f0a2d4c6e8b1a3d5f7c9e",
  },
  line_objects: [
    {
      schema_version: "line-object-v0",
      id: "01JXG6BJ5Q2TCM7B1Z0Y9X8W7V",
      file_path: "src/main.rs",
      line_range: [10, 24],
      commit_sha: "f3a9c2d1e8b7a6c5d4e3f2a1b0c9d8e7f6a5b4c3",
      conversation_id: "01JXG6BJ2M9QZJ4Y8W7V6T5R4E",
      turn_id: "01JXG6BJ3N0RAK5Z9X8W7V6T5S",
      confidence: "exact",
    },
  ],
  session_commit_links: [
    {
      conversation_id: "01JXG6BJ2M9QZJ4Y8W7V6T5R4E",
      commit_sha: "f3a9c2d1e8b7a6c5d4e3f2a1b0c9d8e7f6a5b4c3",
      patch_id: "7d2e9a4c6b8f1e3a5c7d9b2f4e6a8c1d3b5f7e9a",
    },
  ],
  blobs: [
    {
      sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
      byte_size: 2097152,
      content_type: "text/plain",
    },
  ],
};

test("accepts a Rust-emitted sync batch", () => {
  const parsed = syncBatchSchema.parse(rustEmitted);
  assert.equal(parsed.line_objects.length, 1);
  assert.equal(parsed.session_commit_links[0].patch_id.length, 40);
  assert.equal(parsed.blobs[0].byte_size, 2097152);
});

test("rejects a batch without repo binding", () => {
  const { repo: _repo, ...withoutRepo } = rustEmitted;
  assert.throws(() => syncBatchSchema.parse(withoutRepo));
});

test("accepts a response and rejects unknown statuses", () => {
  const response = {
    schema_version: "sync-response-v0",
    repo_id: "5d3b1f9e-2a7c-4e8b-9d1f-3c5a7e9b2d4f",
    results: [
      {
        kind: "turn",
        id: "01JXG6BJ3N0RAK5Z9X8W7V6T5S",
        status: "rejected",
        reason: "hash_mismatch",
      },
      {
        kind: "blob",
        id: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        status: "pending",
      },
    ],
  };
  const parsed = syncResponseSchema.parse(response);
  assert.equal(parsed.results[0].reason, "hash_mismatch");
  assert.throws(() =>
    syncResponseSchema.parse({
      ...response,
      results: [{ ...response.results[0], status: "exploded" }],
    }),
  );
});
