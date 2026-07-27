use std::fs;
use std::path::{Path, PathBuf};

use crate::citations::enrich_turn_with_citations;
use crate::claude_transcript::render_claude_transcript;
use crate::content::{enrich_turn_with_images, extract_claude_content};
use crate::metadata::{finalize_session_metadata, insert_str, normalize_model, vendor_session_id};
use crate::path_util::{
    claude_project_dir, claude_project_key, paths_match_workspace, workspace_is_under_cwd,
};
use chrono::{DateTime, Utc};
use lineage_agent::{
    no_vendor_session_id, AgentSource, RenderedTranscript, ResumeInvocation, SessionReader,
    SessionRef, SessionResumer, TranscriptWriter,
};
use lineage_core::{
    derive_session_id, AgentKind, Conversation, LineageError, LineageId, Role, Turn,
    CONVERSATION_SCHEMA, SESSION_SUMMARY_KEY,
};
use serde_json::Value;

const SKIP_TYPES: &[&str] = &[
    "file-history-snapshot",
    "progress",
    "queue-operation",
    "summary",
    "system",
];

/// The metadata key this adapter records Claude's own session id under. Named so
/// the import that writes it and the resume that reads it cannot drift apart.
const SESSION_ID_KEY: &str = "claude_session_id";

pub struct ClaudeAdapter {
    workspace_root: PathBuf,
}

impl ClaudeAdapter {
    pub fn new(workspace_root: impl AsRef<Path>) -> Self {
        Self {
            workspace_root: workspace_root.as_ref().to_path_buf(),
        }
    }

    fn session_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = Vec::new();

        let local_projects = self.workspace_root.join(".claude").join("projects");
        if local_projects.exists() {
            for entry in fs::read_dir(&local_projects)
                .into_iter()
                .flatten()
                .filter_map(|e| e.ok())
            {
                if entry.path().is_dir() {
                    dirs.push(entry.path());
                }
            }
        }

        if let Some(home) = home_dir() {
            let scoped = claude_project_dir(&home, &self.workspace_root);
            if scoped.exists() {
                dirs.push(scoped);
            }
            // Parent-workspace sessions (gap 7c): a session opened in an
            // ancestor directory files its transcript under that ancestor's
            // project key, invisible to the exact-key lookup above. The walk
            // stops below $HOME: sessions run in the home directory itself
            // are a firehose of unrelated work, and scanning that project
            // dir on every discover would cost more than the rare capture
            // is worth.
            for ancestor in self.workspace_root.ancestors().skip(1) {
                if ancestor == Path::new("/") || ancestor.as_os_str().is_empty() || ancestor == home
                {
                    break;
                }
                let dir = claude_project_dir(&home, ancestor);
                if dir.exists() {
                    dirs.push(dir);
                }
            }
        }

        dirs.sort();
        dirs.dedup();
        dirs
    }

    fn is_session_file(path: &Path) -> bool {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !name.ends_with(".jsonl") {
            return false;
        }
        if name == "history.jsonl" {
            return false;
        }
        true
    }
}

impl AgentSource for ClaudeAdapter {
    fn agent(&self) -> AgentKind {
        AgentKind::Claude
    }

    fn discover(&self) -> Result<Vec<SessionRef>, LineageError> {
        let mut sessions = Vec::new();
        for dir in self.session_dirs() {
            for entry in fs::read_dir(&dir).map_err(|e| LineageError::Other(e.to_string()))? {
                let entry = entry.map_err(|e| LineageError::Other(e.to_string()))?;
                let path = entry.path();
                if !path.is_file() || !Self::is_session_file(&path) {
                    continue;
                }
                let meta = entry
                    .metadata()
                    .map_err(|e| LineageError::Other(e.to_string()))?;
                let started_at = meta.modified().ok().map(DateTime::<Utc>::from);
                let id_hint = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown")
                    .to_string();
                sessions.push(SessionRef {
                    id_hint,
                    agent: AgentKind::Claude,
                    source_path: path,
                    started_at,
                });
            }
        }
        Ok(sessions)
    }
}

impl SessionReader for ClaudeAdapter {
    fn read(&self, session: &SessionRef) -> Result<Conversation, LineageError> {
        let content = fs::read_to_string(&session.source_path)
            .map_err(|e| LineageError::Other(e.to_string()))?;

        let started_at = session.started_at.unwrap_or_else(Utc::now);
        let id = derive_session_id(
            AgentKind::Claude,
            session.source_path.to_str().unwrap_or(""),
            &started_at.to_rfc3339(),
        );

        let mut conversation = Conversation {
            schema_version: CONVERSATION_SCHEMA.into(),
            id,
            agent: AgentKind::Claude,
            started_at,
            ended_at: None,
            workspace_root: self.workspace_root.display().to_string(),
            parent_session_id: None,
            fork_origin: None,
            pull_origin: None,
            private: false,
            turns: Vec::new(),
            commit_shas: Vec::new(),
            metadata: serde_json::Map::from_iter([
                (
                    "source".into(),
                    Value::String(session.source_path.display().to_string()),
                ),
                (
                    "claude_project_key".into(),
                    Value::String(claude_project_key(&self.workspace_root)),
                ),
            ])
            .into_iter()
            .collect(),
        };

        let mut turn_index = 0usize;
        let mut ancestor_cwd: Option<String> = None;
        let mut session_id: Option<String> = None;
        let mut is_sidechain = false;
        let mut claude_code_version: Option<String> = None;
        let mut git_branch: Option<String> = None;
        let mut session_summary: Option<String> = None;

        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let v: Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue,
            };

            if v.get("isMeta").and_then(|m| m.as_bool()) == Some(true) {
                continue;
            }

            let entry_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if entry_type == "summary" {
                if let Some(text) = v.get("summary").and_then(|s| s.as_str()) {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        session_summary = Some(trimmed.to_string());
                    }
                }
                continue;
            }
            if SKIP_TYPES.contains(&entry_type) {
                continue;
            }

            if session_id.is_none() {
                session_id = v
                    .get("sessionId")
                    .and_then(|s| s.as_str())
                    .map(String::from);
            }
            if claude_code_version.is_none() {
                claude_code_version = v.get("version").and_then(|s| s.as_str()).map(String::from);
            }
            if git_branch.is_none() {
                git_branch = v
                    .get("gitBranch")
                    .and_then(|s| s.as_str())
                    .filter(|s| !s.is_empty())
                    .map(String::from);
            }
            if v.get("isSidechain").and_then(|s| s.as_bool()) == Some(true) {
                is_sidechain = true;
            }

            if let Some(cwd) = v.get("cwd").and_then(|c| c.as_str()) {
                if paths_match_workspace(cwd, &self.workspace_root) {
                    conversation.workspace_root = cwd.to_string();
                } else if workspace_is_under_cwd(cwd, &self.workspace_root) {
                    // Parent-workspace entry: keep the repo as workspace_root
                    // so downstream normalization stays repo-relative; where
                    // the session actually ran is preserved as metadata below.
                    ancestor_cwd = Some(cwd.to_string());
                } else {
                    continue;
                }
            }

            let role = match entry_type {
                "assistant" => Role::Assistant,
                "user" => Role::User,
                "system" => Role::System,
                _ => continue,
            };

            let message = v.get("message").unwrap_or(&v);
            let (text, tool_calls, artifacts, is_tool_result) =
                extract_claude_content(message, Some(&self.workspace_root));
            let role = if is_tool_result { Role::Tool } else { role };

            if text.is_empty() && tool_calls.is_empty() {
                continue;
            }

            let timestamp = v
                .get("timestamp")
                .and_then(|t| t.as_str())
                .and_then(parse_timestamp);

            if let Some(ts) = timestamp {
                conversation.ended_at = Some(ts);
            }

            let model = normalize_model(
                message
                    .get("model")
                    .or_else(|| v.get("model"))
                    .and_then(|m| m.as_str())
                    .map(String::from),
            );

            let mut artifacts = artifacts;
            enrich_turn_with_citations(&text, &mut artifacts, Some(&self.workspace_root));
            enrich_turn_with_images(&text, &mut artifacts);

            conversation.turns.push(Turn {
                id: LineageId::from(format!("{}-{}", conversation.id.as_str(), turn_index)),
                role,
                content: text,
                tool_calls,
                model,
                timestamp,
                artifacts,
            });
            turn_index += 1;
        }

        if is_sidechain {
            if let Some(ref sid) = session_id {
                conversation.parent_session_id = Some(LineageId::from(sid.as_str()));
            }
            conversation
                .metadata
                .insert("is_sidechain".into(), Value::Bool(true));
        }

        if let Some(cwd) = ancestor_cwd {
            // Adopt a parent-workspace session only if it actually touched
            // this repo — extraction leaves repo-local paths relative and
            // foreign ones absolute, so relevance is "any relative path".
            // Otherwise every repo under the workspace would import it.
            let touches_repo = lineage_core::files_touched(&conversation)
                .iter()
                .any(|p| Path::new(p).is_relative());
            if touches_repo {
                insert_str(&mut conversation.metadata, "session_cwd", Some(cwd));
            } else {
                conversation.turns.clear();
            }
        }

        insert_str(&mut conversation.metadata, SESSION_ID_KEY, session_id);
        insert_str(
            &mut conversation.metadata,
            SESSION_SUMMARY_KEY,
            session_summary,
        );
        insert_str(
            &mut conversation.metadata,
            "claude_code_version",
            claude_code_version,
        );
        insert_str(&mut conversation.metadata, "git_branch", git_branch);
        finalize_session_metadata(&mut conversation);

        Ok(conversation)
    }
}

impl TranscriptWriter for ClaudeAdapter {
    fn render_transcript(
        &self,
        conversation: &Conversation,
    ) -> Result<RenderedTranscript, LineageError> {
        let home = home_dir().ok_or_else(|| {
            LineageError::Other("cannot locate the Claude state directory: HOME is unset".into())
        })?;
        Ok(render_claude_transcript(
            conversation,
            &home,
            &self.workspace_root,
        ))
    }
}

impl SessionResumer for ClaudeAdapter {
    fn resume_invocation(
        &self,
        conversation: &Conversation,
    ) -> Result<ResumeInvocation, LineageError> {
        let session_id = vendor_session_id(conversation, SESSION_ID_KEY)
            .ok_or_else(|| no_vendor_session_id(AgentKind::Claude))?;
        Ok(ResumeInvocation {
            command: format!("claude --resume {session_id}"),
            // Claude derives the project key from the launch directory, so the
            // same id resolves to nothing from anywhere else.
            cwd: Some(self.workspace_root.clone()),
        })
    }
}

fn parse_timestamp(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lineage_agent::{AgentSource, SessionReader, SessionRef};
    use lineage_core::ToolCall;

    #[test]
    fn reads_claude_fixture_with_tool_use() {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/claude-code-history");
        let adapter = ClaudeAdapter::new(&fixture);
        let sessions = adapter.discover().unwrap();
        // Ancestor-dir discovery may surface unrelated transcripts on a real
        // machine; assert on the fixture's own session, not position 0.
        let fixture_session = sessions
            .iter()
            .find(|s| s.source_path.starts_with(&fixture))
            .expect("fixture session discovered");
        let conv = adapter.read(fixture_session).unwrap();
        assert_eq!(conv.agent, AgentKind::Claude);
        assert!(!conv.turns.is_empty());
        assert!(conv.turns.iter().any(|t| !t.tool_calls.is_empty()));
        assert!(conv
            .turns
            .iter()
            .any(|t| { t.tool_calls.iter().any(|tc: &ToolCall| tc.name == "Read") }));
        assert_eq!(
            conv.metadata
                .get("claude_code_version")
                .and_then(|v| v.as_str()),
            Some("2.0.31")
        );
        assert_eq!(
            conv.metadata.get("git_branch").and_then(|v| v.as_str()),
            Some("main")
        );
        assert!(conv.primary_model().is_some());
    }

    #[test]
    fn captures_the_last_claude_summary_as_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let transcript = dir.path().join("session.jsonl");
        fs::write(
            &transcript,
            r#"{"type":"summary","summary":"First title","sessionId":"abc"}
{"type":"user","sessionId":"abc","cwd":".","message":{"role":"user","content":[{"type":"text","text":"hello"}]},"timestamp":"2026-06-06T10:01:00Z"}
{"type":"summary","summary":"Lineage platform RLS audit","sessionId":"abc"}
"#,
        )
        .unwrap();
        let adapter = ClaudeAdapter::new(dir.path());
        let session = SessionRef {
            id_hint: "abc".into(),
            agent: AgentKind::Claude,
            source_path: transcript,
            started_at: Some(Utc::now()),
        };
        let conv = adapter.read(&session).unwrap();
        assert_eq!(
            conv.metadata
                .get(SESSION_SUMMARY_KEY)
                .and_then(|v| v.as_str()),
            Some("Lineage platform RLS audit")
        );
        assert_eq!(conv.turns.len(), 1);
    }
}
