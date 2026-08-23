//! Recovering per-file artifacts from a V4A `apply_patch` payload.
//!
//! Cursor's `ApplyPatch` tool (and Codex's `apply_patch`/`shell` equivalent)
//! passes its whole payload as a single raw string in OpenAI's V4A patch
//! format — `*** Begin Patch` / one or more `*** {Add,Update,Delete} File:
//! <path>` sections / `*** End Patch` — not as a JSON object. Every existing
//! key lookup (`input.get("path")`, `first_str(input, PATH_KEYS)`) operates on
//! a `serde_json::Value::Object` and returns `None` unconditionally against a
//! `Value::String`, so this tool call silently produced zero artifacts and no
//! resolved target — the edit happened but Lineage never saw which file it
//! touched. A patch can also name several files in one call, which a single
//! `path` field could never represent, so this returns one artifact per file
//! section rather than trying to fit the shape used by object-argument tools.

use lineage_core::{
    normalize_repo_path_unscoped, Artifact, ArtifactKind, ArtifactResolve, ResolveStrategy,
};
use std::path::Path;

/// One file section recovered from a V4A patch body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchedFile {
    pub path: String,
    /// This file's own hunk text (the `@@` sections between its header and
    /// the next `*** ... File:` header or `*** End Patch`), preserved as the
    /// `DiffHunk` resolve strategy expects.
    pub hunk: String,
}

const HEADER_PREFIXES: [&str; 3] = ["*** Add File: ", "*** Update File: ", "*** Delete File: "];

/// True when the raw string looks like a V4A patch body — callers use this to
/// decide whether a string-shaped `input` should go through this parser
/// instead of the object-keyed lookups the other tools use.
pub fn looks_like_v4a_patch(input: &str) -> bool {
    input.contains("*** Begin Patch") && HEADER_PREFIXES.iter().any(|p| input.contains(p))
}

/// Split a V4A patch body into one entry per `Add`/`Update`/`Delete File`
/// section. `Delete File` sections carry no hunk body worth keeping (nothing
/// downstream can anchor on content that no longer exists) but the path is
/// still reported so the deletion is visible in provenance.
pub fn parse_v4a_patch(input: &str) -> Vec<PatchedFile> {
    let mut files = Vec::new();
    let lines: Vec<&str> = input.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let header = HEADER_PREFIXES.iter().find_map(|prefix| {
            line.strip_prefix(prefix).map(|path| {
                (
                    path.trim().to_string(),
                    line.starts_with("*** Delete File: "),
                )
            })
        });
        let Some((path, is_delete)) = header else {
            i += 1;
            continue;
        };
        i += 1;
        let body_start = i;
        while i < lines.len() && !is_section_header(lines[i]) {
            i += 1;
        }
        let hunk = if is_delete {
            String::new()
        } else {
            lines[body_start..i].join("\n")
        };
        files.push(PatchedFile { path, hunk });
    }
    files
}

fn is_section_header(line: &str) -> bool {
    line == "*** End Patch" || HEADER_PREFIXES.iter().any(|p| line.starts_with(p))
}

/// Build the artifacts a `PatchedFile` list resolves to, paths normalized
/// repo-relative the same way every other tool-input path is.
pub fn artifacts_from_v4a_patch(
    files: &[PatchedFile],
    workspace_root: Option<&Path>,
) -> Vec<Artifact> {
    files
        .iter()
        .filter(|f| !f.hunk.is_empty())
        .map(|f| Artifact {
            kind: ArtifactKind::Diff,
            path: normalize_repo_path_unscoped(&f.path, workspace_root),
            blob_ref: None,
            content_hash: None,
            mime_type: None,
            preview_data_url: None,
            line_range: None,
            resolve: Some(ArtifactResolve {
                strategy: ResolveStrategy::DiffHunk,
                old_string: None,
                new_string: None,
                patch: Some(f.hunk.clone()),
            }),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Transcribed from a real Cursor `ApplyPatch` call (docker-compose.yml)
    /// captured on a local machine — not hand-written, to match the actual
    /// payload shape rather than an assumed one.
    const REAL_MULTI_HUNK: &str = "*** Begin Patch\n*** Update File: /Users/dev/proj/docker-compose.yml\n@@\n   minio:\n     image: minio/minio:latest\n@@\n       retries: 5\n \n+  localstack:\n+    image: localstack/localstack:latest\n+\n volumes:\n   lineage-postgres:\n+  lineage-localstack:\n*** End Patch\n";

    #[test]
    fn parses_a_single_update_file_section() {
        let files = parse_v4a_patch(REAL_MULTI_HUNK);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "/Users/dev/proj/docker-compose.yml");
        assert!(files[0].hunk.contains("+  localstack:"));
        assert!(files[0].hunk.contains("@@"));
    }

    #[test]
    fn parses_multiple_file_sections_in_one_patch() {
        let input = "*** Begin Patch\n*** Add File: src/new.rs\n+fn main() {}\n*** Update File: src/lib.rs\n@@\n-old\n+new\n*** End Patch\n";
        let files = parse_v4a_patch(input);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "src/new.rs");
        assert!(files[0].hunk.contains("+fn main() {}"));
        assert_eq!(files[1].path, "src/lib.rs");
        assert!(files[1].hunk.contains("+new"));
    }

    #[test]
    fn delete_file_section_reports_path_with_no_hunk() {
        let input = "*** Begin Patch\n*** Delete File: src/old.rs\n*** End Patch\n";
        let files = parse_v4a_patch(input);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "src/old.rs");
        assert!(files[0].hunk.is_empty());
    }

    #[test]
    fn detects_v4a_shape_and_rejects_plain_text() {
        assert!(looks_like_v4a_patch(REAL_MULTI_HUNK));
        assert!(!looks_like_v4a_patch("just a normal string argument"));
        assert!(!looks_like_v4a_patch("*** Begin Patch\nno file headers\n"));
    }

    #[test]
    fn artifacts_skip_delete_sections_but_keep_edits() {
        let input =
            "*** Begin Patch\n*** Delete File: gone.rs\n*** Add File: new.rs\n+hi\n*** End Patch\n";
        let files = parse_v4a_patch(input);
        let artifacts = artifacts_from_v4a_patch(&files, None);
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].path, "new.rs");
        assert_eq!(
            artifacts[0].resolve.as_ref().unwrap().patch.as_deref(),
            Some("+hi")
        );
    }
}
