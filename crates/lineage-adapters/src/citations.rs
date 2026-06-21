use lineage_core::{normalize_repo_path, Artifact, ArtifactKind, ArtifactResolve, ResolveStrategy};

/// Extract `start:end:path` citations from backtick-delimited spans.
pub fn extract_citations_from_text(text: &str) -> Vec<Artifact> {
    let mut artifacts = Vec::new();
    let mut i = 0;
    let bytes = text.as_bytes();

    while i < bytes.len() {
        if bytes[i] != b'`' {
            i += 1;
            continue;
        }

        let tick_run = if i + 2 < bytes.len() && bytes[i + 1] == b'`' && bytes[i + 2] == b'`' {
            3
        } else {
            1
        };
        let start = i + tick_run;
        let mut end = start;
        while end < bytes.len() {
            if tick_run == 3 && end + 2 < bytes.len() && bytes[end..end + 3] == [b'`', b'`', b'`'] {
                break;
            }
            if tick_run == 1 && bytes[end] == b'`' {
                break;
            }
            end += 1;
        }
        if end >= bytes.len() {
            break;
        }
        let inner = &text[start..end];
        if let Some(artifact) = parse_citation(inner) {
            artifacts.push(artifact);
        }
        i = if tick_run == 3 { end + 3 } else { end + 1 };
    }

    artifacts
}

fn parse_citation(inner: &str) -> Option<Artifact> {
    let parts: Vec<&str> = inner.splitn(3, ':').collect();
    if parts.len() != 3 {
        return None;
    }
    let start = parts[0].parse::<u32>().ok()?;
    let end = parts[1].parse::<u32>().ok()?;
    let path = normalize_repo_path(parts[2], None);
    if path.is_empty() || start == 0 || end == 0 || end < start {
        return None;
    }
    Some(Artifact {
        kind: ArtifactKind::FileEdit,
        path,
        blob_ref: None,
        content_hash: None,
        mime_type: None,
        preview_data_url: None,
        line_range: Some([start, end]),
        resolve: Some(ArtifactResolve {
            strategy: ResolveStrategy::Citation,
            old_string: None,
            patch: None,
        }),
    })
}

pub fn enrich_turn_with_citations(
    content: &str,
    artifacts: &mut Vec<Artifact>,
    workspace_root: Option<&std::path::Path>,
) {
    for citation in extract_citations_from_text(content) {
        let mut citation = citation;
        citation.path = normalize_repo_path(&citation.path, workspace_root);
        if !artifacts
            .iter()
            .any(|a| a.path == citation.path && a.line_range == citation.line_range)
        {
            artifacts.push(citation);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_code_citation() {
        let text = "See `12:25:src/auth.rs` for context";
        let cites = extract_citations_from_text(text);
        assert_eq!(cites.len(), 1);
        assert_eq!(cites[0].path, "src/auth.rs");
        assert_eq!(cites[0].line_range, Some([12, 25]));
    }

    #[test]
    fn parses_triple_backtick_citation() {
        let text = "See ```10:20:src/main.rs``` here";
        let cites = extract_citations_from_text(text);
        assert_eq!(cites.len(), 1);
        assert_eq!(cites[0].line_range, Some([10, 20]));
    }
}
