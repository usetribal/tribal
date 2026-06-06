use std::collections::HashSet;

use chrono::{DateTime, TimeZone, Utc};
use git2::{BranchType, Commit, Repository};
use lineage_core::{files_touched, Conversation, LineageError};

use crate::patch_id::{
    commit_diff, diff_line_similarity, patch_id_for_commit, session_diff_lines, session_patch_id,
};

#[derive(Debug, Clone, serde::Serialize)]
pub struct CommitMatch {
    pub commit_sha: String,
    pub score: f64,
    pub signals: Vec<String>,
}

const MAX_COMMITS_TO_SCAN: usize = 200;

pub fn map_conversation_to_commits(
    repo: &Repository,
    conversation: &Conversation,
    limit: usize,
) -> Result<Vec<CommitMatch>, LineageError> {
    let touched = files_touched(conversation);
    let session_branch = conversation
        .metadata
        .get("git_branch")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let session_start = conversation.started_at;
    let session_end = conversation.ended_at.unwrap_or_else(Utc::now);
    let session_patch = session_patch_id(conversation);
    let session_lines = session_diff_lines(conversation);

    let mut head = repo
        .head()
        .map_err(|e| LineageError::Other(e.to_string()))?
        .peel_to_commit()
        .map_err(|e| LineageError::Other(e.to_string()))?;

    let mut matches = Vec::new();
    let mut scanned = 0usize;

    loop {
        let score_result = score_commit(
            repo,
            &head,
            &touched,
            session_branch.as_deref(),
            session_start,
            session_end,
            conversation,
            session_patch.as_deref(),
            &session_lines,
        )?;
        if score_result.score > 0.0 {
            matches.push(score_result);
        }

        scanned += 1;
        if scanned >= MAX_COMMITS_TO_SCAN || head.parent_count() == 0 {
            break;
        }
        head = head
            .parent(0)
            .map_err(|e| LineageError::Other(e.to_string()))?;
    }

    matches.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    matches.truncate(limit);
    Ok(matches)
}

pub fn best_commit_for_conversation(
    repo: &Repository,
    conversation: &Conversation,
) -> Result<Option<CommitMatch>, LineageError> {
    let matches = map_conversation_to_commits(repo, conversation, 1)?;
    Ok(matches.into_iter().next().filter(|m| m.score >= 0.25))
}

#[allow(clippy::too_many_arguments)]
fn score_commit(
    repo: &Repository,
    commit: &Commit,
    touched_files: &[String],
    session_branch: Option<&str>,
    session_start: DateTime<Utc>,
    session_end: DateTime<Utc>,
    conversation: &Conversation,
    session_patch: Option<&str>,
    session_lines: &[String],
) -> Result<CommitMatch, LineageError> {
    let sha = commit.id().to_string();
    let mut score = 0.0f64;
    let mut signals = Vec::new();

    let author = commit.author().when();
    let author_dt = Utc
        .timestamp_opt(author.seconds(), 0)
        .single()
        .unwrap_or(session_start);
    let slack = chrono::Duration::hours(2);
    if author_dt >= session_start - slack && author_dt <= session_end + slack {
        score += 0.35;
        signals.push("time_overlap".into());
    }

    let commit_files = files_in_commit(repo, commit)?;
    if !touched_files.is_empty() && !commit_files.is_empty() {
        let overlap: usize = touched_files
            .iter()
            .filter(|f| commit_files.iter().any(|c| paths_match(f, c)))
            .count();
        if overlap > 0 {
            let ratio = overlap as f64 / touched_files.len() as f64;
            score += 0.4 * ratio;
            signals.push(format!("files_overlap:{overlap}"));
        }
    }

    if let Some(branch) = session_branch {
        if commit_on_branch(repo, commit, branch) {
            score += 0.15;
            signals.push(format!("branch_match:{branch}"));
        }
    }

    if lineage_core::conversation_modified_code(conversation) {
        score += 0.05;
        signals.push("code_tools".into());
    }

    let diff = commit_diff(repo, commit)?;
    if let (Some(session_pid), Ok(commit_pid)) = (session_patch, patch_id_for_commit(repo, commit)) {
        if session_pid == commit_pid {
            score += 0.35;
            signals.push("patch_id_match".into());
        }
    }

    let similarity = diff_line_similarity(session_lines, &diff);
    if similarity > 0.0 {
        score += 0.25 * similarity;
        signals.push(format!("diff_similarity:{similarity:.2}"));
    }

    Ok(CommitMatch {
        commit_sha: sha,
        score: score.min(1.0),
        signals,
    })
}

fn commit_on_branch(repo: &Repository, commit: &Commit, branch_name: &str) -> bool {
    let Ok(branch) = repo.find_branch(branch_name, BranchType::Local) else {
        return false;
    };
    let Ok(tip) = branch.get().peel_to_commit() else {
        return false;
    };
    if commit.id() == tip.id() {
        return true;
    }
    repo.graph_descendant_of(commit.id(), tip.id())
        .unwrap_or(false)
}

fn files_in_commit(repo: &Repository, commit: &Commit) -> Result<HashSet<String>, LineageError> {
    let diff = commit_diff(repo, commit)?;
    let mut files = HashSet::new();
    diff.foreach(
        &mut |_, _| true,
        None,
        Some(&mut |delta, _| {
            if let Some(path) = delta.new_file().path().and_then(|p| p.to_str()) {
                files.insert(path.to_string());
            } else if let Some(path) = delta.old_file().path().and_then(|p| p.to_str()) {
                files.insert(path.to_string());
            }
            true
        }),
        None,
    )
    .map_err(|e| LineageError::Other(e.to_string()))?;
    Ok(files)
}

fn paths_match(session_path: &str, commit_path: &str) -> bool {
    let a = session_path.trim_start_matches("./");
    let b = commit_path.trim_start_matches("./");
    a == b || a.ends_with(b) || b.ends_with(a)
}
