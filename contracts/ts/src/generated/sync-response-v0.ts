// GENERATED FILE — do not edit.
// Source: specs/schema (canonical: lineage-core Rust types).
// Regenerate with: pnpm --filter @lineage/contracts generate

import { z } from "zod"

export const syncResponseSchema = z.object({ "metadata": z.record(z.string(), z.any()).optional(), "repo_id": z.string().describe("Server-resolved repo id; cache and send back as\n`repo.server_repo_id`."), "results": z.array(z.object({ "id": z.string().describe("The object's id: ULID, `conversation_id:commit_sha` for links,\nsha256 hex for blobs."), "kind": z.enum(["conversation","turn","line_object","session_commit_link","blob"]), "reason": z.union([z.enum(["hash_mismatch","private","schema_version","too_large","invalid"]), z.null()]).optional(), "status": z.enum(["accepted","noop","rejected","pending"]) })), "schema_version": z.string() })
export type SyncResponse = z.infer<typeof syncResponseSchema>
