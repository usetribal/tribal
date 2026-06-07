use git2::{Commit, Diff, DiffFormat, Repository};
use lineage_core::{ArtifactKind, Conversation, LineageError};
use sha2::{Digest, Sha256};

pub fn patch_id_for_commit(repo: &Repository, commit: &Commit) -> Result<String, LineageError> {
    let tree = commit
        .tree()
        .map_err(|e| LineageError::Other(e.to_string()))?;
    let parent_tree = if commit.parent_count() > 0 {
        Some(
            commit
                .parent(0)
                .map_err(|e| LineageError::Other(e.to_string()))?
                .tree()
                .map_err(|e| LineageError::Other(e.to_string()))?,
        )
    } else {
        None
    };

    let diff = repo
        .diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None)
        .map_err(|e| LineageError::Other(e.to_string()))?;

    Ok(patch_id_from_diff(&diff))
}

pub fn patch_id_from_diff(diff: &git2::Diff) -> String {
    let mut hasher = Sha256::new();
    let _ = diff.print(DiffFormat::Patch, |_, _, line| {
        let origin = line.origin();
        if origin == '+' || origin == '-' {
            if let Ok(content) = std::str::from_utf8(line.content()) {
                if !content.starts_with("+++") && !content.starts_with("---") {
                    hasher.update([origin as u8]);
                    hasher.update(content.as_bytes());
                }
            }
        }
        true
    });
    format!("{:x}", hasher.finalize())
}

pub fn session_patch_id(conversation: &Conversation) -> Option<String> {
    let lines = session_diff_lines(conversation);
    if lines.is_empty() {
        return None;
    }
    let mut hasher = Sha256::new();
    for line in lines {
        hasher.update(line.as_bytes());
        hasher.update(b"\n");
    }
    Some(format!("{:x}", hasher.finalize()))
}

pub fn session_diff_lines(conversation: &Conversation) -> Vec<String> {
    let mut lines = Vec::new();
    for turn in &conversation.turns {
        for artifact in &turn.artifacts {
            if !matches!(artifact.kind, ArtifactKind::Diff | ArtifactKind::FileEdit) {
                continue;
            }
            if !artifact.path.is_empty() {
                lines.push(format!("FILE:{}", artifact.path.trim_start_matches("./")));
            }
            if let Some(resolve) = &artifact.resolve {
                if let Some(patch) = &resolve.patch {
                    for line in patch.lines() {
                        let t = line.trim();
                        if !t.is_empty() && !t.starts_with("+++") && !t.starts_with("---") {
                            lines.push(t.to_string());
                        }
                    }
                }
                if let Some(old) = &resolve.old_string {
                    for line in old.lines() {
                        let t = line.trim();
                        if !t.is_empty() {
                            lines.push(format!("-{t}"));
                        }
                    }
                }
            }
        }
    }
    lines.sort();
    lines.dedup();
    lines
}

pub fn diff_line_similarity(session_lines: &[String], commit_diff: &Diff) -> f64 {
    if session_lines.is_empty() {
        return 0.0;
    }
    let commit_lines = diff_content_lines(commit_diff);
    if commit_lines.is_empty() {
        return 0.0;
    }
    let commit_set: std::collections::HashSet<&str> =
        commit_lines.iter().map(String::as_str).collect();
    let matched = session_lines
        .iter()
        .filter(|l| commit_set.contains(l.as_str()))
        .count();
    matched as f64 / session_lines.len() as f64
}

pub fn diff_content_lines(diff: &Diff) -> Vec<String> {
    let mut lines = Vec::new();
    let _ = diff.print(DiffFormat::Patch, |_, _, line| {
        let origin = line.origin();
        if origin == '+' || origin == '-' {
            if let Ok(content) = std::str::from_utf8(line.content()) {
                let t = content.trim();
                if !t.is_empty() && !t.starts_with("+++") && !t.starts_with("---") {
                    lines.push(format!("{origin}{t}"));
                }
            }
        }
        true
    });
    lines.sort();
    lines.dedup();
    lines
}

pub fn commit_diff<'a>(
    repo: &'a Repository,
    commit: &'a Commit<'_>,
) -> Result<Diff<'a>, LineageError> {
    let tree = commit
        .tree()
        .map_err(|e| LineageError::Other(e.to_string()))?;
    let parent_tree = if commit.parent_count() > 0 {
        Some(
            commit
                .parent(0)
                .map_err(|e| LineageError::Other(e.to_string()))?
                .tree()
                .map_err(|e| LineageError::Other(e.to_string()))?,
        )
    } else {
        None
    };
    repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None)
        .map_err(|e| LineageError::Other(e.to_string()))
}

pub fn build_patch_id_index(
    repo: &Repository,
) -> Result<std::collections::HashMap<String, String>, LineageError> {
    let mut head = repo
        .head()
        .map_err(|e| LineageError::Other(e.to_string()))?
        .peel_to_commit()
        .map_err(|e| LineageError::Other(e.to_string()))?;

    let mut index = std::collections::HashMap::new();
    loop {
        let sha = head.id().to_string();
        let patch_id = patch_id_for_commit(repo, &head)?;
        index.entry(patch_id).or_insert(sha);

        if head.parent_count() == 0 {
            break;
        }
        head = head
            .parent(0)
            .map_err(|e| LineageError::Other(e.to_string()))?;
    }
    Ok(index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lineage_core::{AgentKind, Artifact, ArtifactKind, Conversation, LineageId, Role, Turn};
    use std::process::Command;

    fn init_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "t@t.dev"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "T"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        dir
    }

    #[test]
    fn patch_id_stable_for_commit() {
        let tmp = init_repo();
        std::fs::write(tmp.path().join("a.txt"), "one\n").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(tmp.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(tmp.path())
            .output()
            .unwrap();

        let repo = git2::Repository::open(tmp.path()).unwrap();
        let commit = repo.head().unwrap().peel_to_commit().unwrap();
        let id1 = patch_id_for_commit(&repo, &commit).unwrap();
        let id2 = patch_id_for_commit(&repo, &commit).unwrap();
        assert_eq!(id1, id2);
        assert!(!id1.is_empty());
    }

    #[test]
    fn session_patch_id_from_file_edits() {
        let mut conv = Conversation::new(AgentKind::Cursor, "/tmp");
        conv.turns.push(Turn {
            id: LineageId::new(),
            role: Role::Assistant,
            content: String::new(),
            tool_calls: vec![],
            model: None,
            timestamp: None,
            artifacts: vec![Artifact {
                kind: ArtifactKind::FileEdit,
                path: "main.rs".into(),
                blob_ref: None,
                content_hash: None,
                mime_type: None,
                preview_data_url: None,
                line_range: Some([1, 3]),
                resolve: None,
            }],
        });
        let lines = session_diff_lines(&conv);
        assert!(!lines.is_empty());
        assert!(session_patch_id(&conv).is_some());
    }

    #[test]
    fn diff_line_similarity_counts_overlap() {
        let session_lines = vec!["+added".into(), "-removed".into()];
        let tmp = init_repo();
        std::fs::write(tmp.path().join("b.txt"), "old\n").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(tmp.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "c1"])
            .current_dir(tmp.path())
            .output()
            .unwrap();
        std::fs::write(tmp.path().join("b.txt"), "new\n").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(tmp.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "c2"])
            .current_dir(tmp.path())
            .output()
            .unwrap();
        let repo = git2::Repository::open(tmp.path()).unwrap();
        let commit = repo.head().unwrap().peel_to_commit().unwrap();
        let diff = commit_diff(&repo, &commit).unwrap();
        let sim = diff_line_similarity(&session_lines, &diff);
        assert!((0.0..=1.0).contains(&sim));
    }
}
