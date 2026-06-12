// GENERATED FILE — do not edit.
// Source: specs/schema (canonical: lineage-core Rust types).
// Regenerate with: pnpm --filter @lineage/contracts generate

import { z } from "zod"

export const gitNoteSchema = z.object({ "commit_sha": z.string(), "line_object_ids": z.array(z.string()).optional(), "patch_id": z.union([z.string(), z.null()]).optional(), "schema_version": z.string(), "session_ids": z.array(z.string()) })
export type GitNote = z.infer<typeof gitNoteSchema>
