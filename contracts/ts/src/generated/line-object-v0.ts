// GENERATED FILE — do not edit.
// Source: specs/schema (canonical: lineage-core Rust types).
// Regenerate with: pnpm --filter @lineage/contracts generate

import { z } from "zod"

export const lineObjectSchema = z.object({ "commit_sha": z.string(), "confidence": z.enum(["exact","heuristic","manual"]), "conversation_id": z.string(), "file_path": z.string(), "id": z.string(), "line_range": z.array(z.number().int().gte(0)).min(2).max(2), "metadata": z.record(z.string(), z.any()).optional(), "schema_version": z.string(), "turn_id": z.string() })
export type LineObject = z.infer<typeof lineObjectSchema>
