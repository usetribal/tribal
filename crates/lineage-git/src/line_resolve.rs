use std::collections::HashSet;
use std::path::Path;

use git2::{Oid, Repository};
use lineage_core::{
    Artifact, ArtifactKind, Confidence, Conversation, LineObject, LineageError, ResolveStrategy,
};
use lineage_core::derive_line_object_id;

pub fn materialize_line_objects(
    repo: &Repository,
    conversation: &Conversation,
    commit_sha: &str,
    link_confidence: Confidence,
) -> Result<Vec<LineObject>, LineageError> {
    let changed = files_changed_in_commit(repo, commit_sha)?;
    let mut objects = Vec::new();

    for turn in &conversation.turns {
        for artifact in &turn.artifacts {
            if !artifact_is_materializable(artifact) {
                continue;
            }
            if !changed.is_empty() && !changed.contains(&artifact.path) {
                continue;
            }

            let ranges = resolve_artifact_ranges(repo, commit_sha, artifact)?;
            for (range, confidence) in ranges {
                let id = derive_line_object_id(
                    &conversation.id,
                    &turn.id,
                    &artifact.path,
                    range,
                    commit_sha,
                );
                objects.push(LineObject::with_id(
                    id,
                    &artifact.path,
                    range,
                    commit_sha,
                    conversation.id.clone(),
                    turn.id.clone(),
                    merge_confidence(confidence, link_confidence),
                ));
            }
        }
    }

    objects.sort_by(|a, b| {
        (
            a.file_path.as_str(),
            a.line_range[0],
            a.turn_id.as_str(),
        )
            .cmp(&(
                b.file_path.as_str(),
                b.line_range[0],
                b.turn_id.as_str(),
            ))
    });
    objects.dedup_by(|a, b| {
        a.id == b.id
            || (a.file_path == b.file_path
                && a.line_range == b.line_range
                && a.turn_id == b.turn_id
                && a.commit_sha == b.commit_sha)
    });

    Ok(objects)
}

fn artifact_is_materializable(artifact: &Artifact) -> bool {
    matches!(
        artifact.kind,
        ArtifactKind::FileEdit | ArtifactKind::Diff
    )
}

fn merge_confidence(resolved: Confidence, link: Confidence) -> Confidence {
    if link == Confidence::Manual {
        return Confidence::Manual;
    }
    resolved
}

fn resolve_artifact_ranges(
    repo: &Repository,
    commit_sha: &str,
    artifact: &Artifact,
) -> Result<Vec<([u32; 2], Confidence)>, LineageError> {
    if let Some(range) = artifact.line_range {
        return Ok(vec![(range, Confidence::Exact)]);
    }

    let Some(resolve) = artifact.resolve.as_ref() else {
        return Ok(Vec::new());
    };

    match resolve.strategy {
        ResolveStrategy::Citation => Ok(Vec::new()),
        ResolveStrategy::OldString => {
            let Some(old_string) = resolve.old_string.as_ref() else {
                return Ok(Vec::new());
            };
            let content = match git_file_at_commit(repo, commit_sha, &artifact.path)? {
                Some(c) => c,
                None => return Ok(Vec::new()),
            };
            Ok(resolve_old_string(&content, old_string))
        }
        ResolveStrategy::FullFile => {
            let content = match git_file_at_commit(repo, commit_sha, &artifact.path)? {
                Some(c) => c,
                None => {
                    return Ok(vec![( [1, 1], Confidence::Heuristic )]);
                }
            };
            let lines = content.lines().count().max(1) as u32;
            Ok(vec![([1, lines], Confidence::Heuristic)])
        }
        ResolveStrategy::DiffHunk => {
            let patch = resolve.patch.as_deref().unwrap_or("");
            Ok(parse_diff_hunks(patch, &artifact.path))
        }
    }
}

pub fn resolve_old_string(content: &str, old_string: &str) -> Vec<([u32; 2], Confidence)> {
    if old_string.is_empty() {
        return Vec::new();
    }

    let mut matches = Vec::new();
    let mut start = 0;
    while let Some(pos) = content[start..].find(old_string) {
        let abs = start + pos;
        let start_line = line_number_at(content, abs);
        let end_line = line_number_at(content, abs + old_string.len().saturating_sub(1));
        matches.push(([start_line, end_line.max(start_line)], Confidence::Exact));
        start = abs + old_string.len();
    }

    if matches.len() > 1 {
        matches
            .iter_mut()
            .for_each(|(_, c)| *c = Confidence::Heuristic);
    }

    matches
}

fn parse_diff_hunks(patch: &str, path: &str) -> Vec<([u32; 2], Confidence)> {
    let mut ranges = Vec::new();
    let normalized_path = path.trim_start_matches("./");

    for line in patch.lines() {
        if line.starts_with("@@") {
            if let Some(range) = parse_unified_hunk_header(line) {
                ranges.push((range, Confidence::Exact));
            }
            continue;
        }
        if line.starts_with("+++ ") || line.starts_with("--- ") {
            let file = line[4..].trim().trim_start_matches("b/").trim_start_matches("a/");
            if file.ends_with(normalized_path) || normalized_path.ends_with(file) {
                continue;
            }
        }
    }

    ranges
}

fn parse_unified_hunk_header(line: &str) -> Option<[u32; 2]> {
    // @@ -old,count +new,count @@
    let plus = line.split('+').nth(1)?;
    let part = plus.split_whitespace().next()?;
    let (start, count_str) = part.split_once(',')?;
    let start_line: u32 = start.parse().ok()?;
    let count: u32 = count_str.parse().unwrap_or(1);
    if start_line == 0 {
        return None;
    }
    let end = start_line.saturating_add(count.saturating_sub(1));
    Some([start_line, end.max(start_line)])
}

pub fn line_number_at(content: &str, byte_pos: usize) -> u32 {
    let slice = &content[..byte_pos.min(content.len())];
    (slice.matches('\n').count() as u32) + 1
}

pub fn git_file_at_commit(
    repo: &Repository,
    commit_sha: &str,
    path: &str,
) -> Result<Option<String>, LineageError> {
    let oid = Oid::from_str(commit_sha)
        .map_err(|e| LineageError::Other(format!("invalid commit: {e}")))?;
    let commit = repo
        .find_commit(oid)
        .map_err(|e| LineageError::Other(e.to_string()))?;
    let tree = commit
        .tree()
        .map_err(|e| LineageError::Other(e.to_string()))?;
    let entry = match tree.get_path(Path::new(path)) {
        Ok(e) => e,
        Err(e) if e.code() == git2::ErrorCode::NotFound => return Ok(None),
        Err(e) => return Err(LineageError::Other(e.to_string())),
    };
    let blob = entry
        .to_object(repo)
        .map_err(|e| LineageError::Other(e.to_string()))?
        .peel_to_blob()
        .map_err(|e| LineageError::Other(e.to_string()))?;
    let text = String::from_utf8(blob.content().to_vec())
        .map_err(|e| LineageError::Other(e.to_string()))?;
    Ok(Some(text))
}

pub fn files_changed_in_commit(repo: &Repository, commit_sha: &str) -> Result<HashSet<String>, LineageError> {
    let oid = Oid::from_str(commit_sha)
        .map_err(|e| LineageError::Other(format!("invalid commit: {e}")))?;
    let commit = repo
        .find_commit(oid)
        .map_err(|e| LineageError::Other(e.to_string()))?;
    let new_tree = commit
        .tree()
        .map_err(|e| LineageError::Other(e.to_string()))?;

    let old_tree = commit
        .parent(0)
        .ok()
        .and_then(|p| p.tree().ok());

    let diff = repo
        .diff_tree_to_tree(old_tree.as_ref(), Some(&new_tree), None)
        .map_err(|e| LineageError::Other(e.to_string()))?;

    let mut files = HashSet::new();
    diff.foreach(
        &mut |delta, _| {
            if let Some(path) = delta.new_file().path() {
                files.insert(path.to_string_lossy().to_string());
            } else if let Some(path) = delta.old_file().path() {
                files.insert(path.to_string_lossy().to_string());
            }
            true
        },
        None,
        None,
        None,
    )
    .map_err(|e| LineageError::Other(e.to_string()))?;

    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_unique_old_string() {
        let content = "line1\nline2\nold code\nline4";
        let ranges = resolve_old_string(content, "old code");
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].0, [3, 3]);
        assert_eq!(ranges[0].1, Confidence::Exact);
    }

    #[test]
    fn ambiguous_old_string_is_heuristic() {
        let content = "foo\nfoo\n";
        let ranges = resolve_old_string(content, "foo");
        assert_eq!(ranges.len(), 2);
        assert!(ranges.iter().all(|(_, c)| *c == Confidence::Heuristic));
    }

    #[test]
    fn parses_unified_diff_hunk() {
        let patch = "@@ -1,3 +10,5 @@\n context";
        let ranges = parse_diff_hunks(patch, "src/a.rs");
        assert_eq!(ranges, vec![([10, 14], Confidence::Exact)]);
    }
}
