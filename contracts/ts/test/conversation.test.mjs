import assert from "node:assert/strict";
import { test } from "node:test";

import { conversationSchema } from "../dist/index.js";

// Emitted verbatim by lineage_core::Conversation::to_json — the Rust types
// and these bindings must accept the same documents.
const rustEmitted = {
  schema_version: "conversation-v0",
  id: "01JXG6BJ2M9QZJ4Y8W7V6T5R4E",
  agent: "claude",
  started_at: "2026-06-12T08:00:00Z",
  workspace_root: "/home/dev/project",
  private: false,
  turns: [
    {
      id: "01JXG6BJ3N0RAK5Z9X8W7V6T5S",
      role: "user",
      content: "add a retry to the fetch",
      timestamp: "2026-06-12T08:00:00Z",
    },
    {
      id: "01JXG6BJ4P1SBL6A0Y9X8W7V6T",
      role: "assistant",
      content: "done",
      tool_calls: [
        {
          id: "tc1",
          name: "edit_file",
          arguments: '{"path":"src/fetch.ts"}',
          result: "ok",
        },
      ],
      model: "claude-sonnet-4",
      timestamp: "2026-06-12T08:00:05Z",
    },
  ],
  commit_shas: ["f3a9c2d1e8b7a6c5d4e3f2a1b0c9d8e7f6a5b4c3"],
};

test("accepts a Rust-emitted conversation", () => {
  const parsed = conversationSchema.parse(rustEmitted);
  assert.equal(parsed.agent, "claude");
  assert.equal(parsed.turns.length, 2);
  assert.equal(parsed.turns[1].tool_calls[0].name, "edit_file");
});

test("rejects an unknown agent", () => {
  assert.throws(() =>
    conversationSchema.parse({ ...rustEmitted, agent: "copilot" }),
  );
});

test("rejects a non-datetime started_at", () => {
  assert.throws(() =>
    conversationSchema.parse({ ...rustEmitted, started_at: "yesterday" }),
  );
});
