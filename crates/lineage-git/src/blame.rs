use std::path::{Path, PathBuf};

use git2::{BlameOptions, Repository};
use lineage_core::{Confidence, LineObject, LineageError, LineageId, ResolveStrategy};

use crate::line_resolve::resolve_old_string;
use crate::notes::read_note_for_commit;
use crate::refs::{read_conversation, read_line_object};

#[derive(Debug, Clone, serde::Serialize)]
pub struct BlameMatch {
    pub turn_id: LineageId,
    pub conversation_id: LineageId,
    pub line_range: Option<[u32; 2]>,
    pub confidence: Confidence,
    pub content_preview: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BlameResult {
    pub line: u32,
    pub commit_sha: String,
    pub line_objects: Vec<LineObject>,
    pub sessions: Vec<LineageId>,
    pub matches: Vec<BlameMatch>,
}

pub fn blame_with_lineage(
    repo: &Repository,
    file_path: &Path,
    line: u32,
) -> Result<BlameResult, LineageError> {
    let rel_path = repo_relative_path(repo, file_path)?;
    let mut opts = BlameOptions::new();
    opts.min_line(line as usize);
    opts.max_line(line as usize);

    let blame = repo
        .blame_file(&rel_path, Some(&mut opts))
        .map_err(|e| LineageError::Other(format!("blame failed: {e}")))?;

    let file_path_str = rel_path.to_string_lossy().to_string();

    let hunk = blame
        .get_line(line as usize)
        .ok_or_else(|| LineageError::Other(format!("no blame hunk for line {line}")))?;

    let commit_sha = hunk.final_commit_id().to_string();

    let mut line_objects = Vec::new();
    let mut sessions = Vec::new();
    let mut matches = Vec::new();

    if let Some(note) = read_note_for_commit(repo, &commit_sha)? {
        sessions = note.session_ids.clone();
        for obj_id in &note.line_object_ids {
            if let Some(obj) = read_line_object(repo, obj_id)? {
                if obj.file_path == file_path_str && obj.contains_line(line) {
                    line_objects.push(obj.clone());
                    if let Some(conv) = read_conversation(repo, &obj.conversation_id)? {
                        if let Some(turn) = conv.turns.iter().find(|t| t.id == obj.turn_id) {
                            matches.push(BlameMatch {
                                turn_id: obj.turn_id.clone(),
                                conversation_id: obj.conversation_id.clone(),
                                line_range: Some(obj.line_range),
                                confidence: obj.confidence,
                                content_preview: preview_content(&turn.content),
                            });
                        }
                    }
                }
            }
        }
    }

    if line_objects.is_empty() {
        for session_id in &sessions {
            if let Some(conv) = read_conversation(repo, session_id)? {
                for turn in &conv.turns {
                    for artifact in &turn.artifacts {
                        if artifact.path != file_path_str {
                            continue;
                        }

                        if let Some(range) = artifact.line_range {
                            if line >= range[0] && line <= range[1] {
                                let confidence = artifact
                                    .resolve
                                    .as_ref()
                                    .map(|r| {
                                        if r.strategy == ResolveStrategy::Citation {
                                            Confidence::Exact
                                        } else {
                                            Confidence::Heuristic
                                        }
                                    })
                                    .unwrap_or(Confidence::Heuristic);

                                line_objects.push(LineObject::new(
                                    &file_path_str,
                                    range,
                                    &commit_sha,
                                    conv.id.clone(),
                                    turn.id.clone(),
                                    confidence,
                                ));
                                matches.push(BlameMatch {
                                    turn_id: turn.id.clone(),
                                    conversation_id: conv.id.clone(),
                                    line_range: Some(range),
                                    confidence,
                                    content_preview: preview_content(&turn.content),
                                });
                            }
                            continue;
                        }

                        if let Some(resolve) = artifact.resolve.as_ref() {
                            if resolve.strategy == ResolveStrategy::OldString {
                                if let Some(old_string) = resolve.old_string.as_ref() {
                                    if let Ok(Some(content)) =
                                        crate::line_resolve::git_file_at_commit(
                                            repo,
                                            &commit_sha,
                                            &file_path_str,
                                        )
                                    {
                                        for (range, confidence) in
                                            resolve_old_string(&content, old_string)
                                        {
                                            if line >= range[0] && line <= range[1] {
                                                matches.push(BlameMatch {
                                                    turn_id: turn.id.clone(),
                                                    conversation_id: conv.id.clone(),
                                                    line_range: Some(range),
                                                    confidence,
                                                    content_preview: preview_content(&turn.content),
                                                });
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(BlameResult {
        line,
        commit_sha,
        line_objects,
        sessions,
        matches,
    })
}

fn preview_content(content: &str) -> String {
    content.chars().take(160).collect()
}

fn repo_relative_path(repo: &Repository, file_path: &Path) -> Result<PathBuf, LineageError> {
    if file_path.is_relative() {
        return Ok(file_path.to_path_buf());
    }
    if let Some(workdir) = repo.workdir() {
        if let Ok(rel) = file_path.strip_prefix(workdir) {
            return Ok(rel.to_path_buf());
        }
    }
    Ok(file_path.to_path_buf())
}
