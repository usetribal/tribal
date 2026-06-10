use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use lineage_agent::{AgentSource, SessionReader, SessionRef};
use lineage_core::{
    derive_session_id, AgentKind, Conversation, LineageError, LineageId, Role, Turn,
    CONVERSATION_SCHEMA,
};
use serde::Deserialize;

use crate::citations::enrich_turn_with_citations;
use crate::content::{enrich_turn_with_images, extract_cursor_content};
use crate::metadata::{finalize_session_metadata, insert_str, normalize_model};
use crate::path_util::cursor_transcripts_dir;

pub struct CursorAdapter {
    workspace_root: PathBuf,
}

impl CursorAdapter {
    pub fn new(workspace_root: impl AsRef<Path>) -> Self {
        Self {
            workspace_root: workspace_root.as_ref().to_path_buf(),
        }
    }

    fn transcript_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = Vec::new();

        let local_projects = self.workspace_root.join(".cursor").join("projects");
        if local_projects.exists() {
            for entry in fs::read_dir(&local_projects)
                .into_iter()
                .flatten()
                .filter_map(|e| e.ok())
            {
                let transcripts = entry.path().join("agent-transcripts");
                if transcripts.is_dir() {
                    dirs.push(transcripts);
                }
            }
        }

        let local = self
            .workspace_root
            .join(".cursor")
            .join("agent-transcripts");
        if local.exists() {
            dirs.push(local);
        }

        if let Some(home) = home_dir() {
            let scoped = cursor_transcripts_dir(&home, &self.workspace_root);
            if scoped.exists() {
                dirs.push(scoped);
            }
        }

        dirs.sort();
        dirs.dedup();
        dirs
    }

    fn discover_in_dir(dir: &Path) -> Result<Vec<SessionRef>, LineageError> {
        let mut sessions = Vec::new();

        for entry in fs::read_dir(dir).map_err(|e| LineageError::Other(e.to_string()))? {
            let entry = entry.map_err(|e| LineageError::Other(e.to_string()))?;
            let path = entry.path();

            if path.is_dir() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                let nested = path.join(format!("{name}.jsonl"));
                if nested.is_file() {
                    sessions.push(session_ref_from_path(&nested)?);
                }
                continue;
            }

            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }

            sessions.push(session_ref_from_path(&path)?);
        }

        Ok(sessions)
    }
}

fn session_ref_from_path(path: &Path) -> Result<SessionRef, LineageError> {
    let meta = fs::metadata(path).map_err(|e| LineageError::Other(e.to_string()))?;
    let started_at = meta.modified().ok().map(DateTime::<Utc>::from);
    let id_hint = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    Ok(SessionRef {
        id_hint,
        agent: AgentKind::Cursor,
        source_path: path.to_path_buf(),
        started_at,
    })
}

#[derive(Debug, Deserialize)]
struct CursorTranscriptLine {
    role: Option<String>,
    #[serde(default)]
    message: Option<serde_json::Value>,
    #[serde(default)]
    text: Option<String>,
}

impl AgentSource for CursorAdapter {
    fn agent(&self) -> AgentKind {
        AgentKind::Cursor
    }

    fn discover(&self) -> Result<Vec<SessionRef>, LineageError> {
        let mut sessions = Vec::new();
        for dir in self.transcript_dirs() {
            sessions.extend(Self::discover_in_dir(&dir)?);
        }
        Ok(sessions)
    }
}

impl SessionReader for CursorAdapter {
    fn read(&self, session: &SessionRef) -> Result<Conversation, LineageError> {
        let content = fs::read_to_string(&session.source_path)
            .map_err(|e| LineageError::Other(e.to_string()))?;

        let started_at = session.started_at.unwrap_or_else(Utc::now);
        let started_key = started_at.to_rfc3339();
        let id = derive_session_id(
            AgentKind::Cursor,
            session.source_path.to_str().unwrap_or(""),
            &started_key,
        );

        let mut conversation = Conversation {
            schema_version: CONVERSATION_SCHEMA.into(),
            id,
            agent: AgentKind::Cursor,
            started_at,
            ended_at: None,
            workspace_root: self.workspace_root.display().to_string(),
            parent_session_id: None,
            private: false,
            turns: Vec::new(),
            commit_shas: Vec::new(),
            metadata: serde_json::Map::from_iter([(
                "source".into(),
                serde_json::Value::String(session.source_path.display().to_string()),
            )])
            .into_iter()
            .collect(),
        };

        let mut turn_index = 0usize;
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let row: CursorTranscriptLine = match serde_json::from_str(line) {
                Ok(r) => r,
                Err(_) => continue,
            };

            let role_str = row
                .role
                .clone()
                .or_else(|| {
                    row.message
                        .as_ref()
                        .and_then(|m| m.get("role"))
                        .and_then(|r| r.as_str())
                        .map(String::from)
                })
                .unwrap_or_else(|| "user".into());

            let (text, tool_calls, artifacts) = if let Some(text) = row.text {
                (text, vec![], vec![])
            } else if let Some(ref message) = row.message {
                extract_cursor_content(message)
            } else {
                continue;
            };

            if text.is_empty() && tool_calls.is_empty() {
                continue;
            }

            let model = row
                .message
                .as_ref()
                .and_then(|m| m.get("model"))
                .and_then(|m| m.as_str())
                .map(String::from);

            let mut artifacts = artifacts;
            enrich_turn_with_citations(&text, &mut artifacts);
            enrich_turn_with_images(&text, &mut artifacts);

            conversation.turns.push(Turn {
                id: LineageId::from(format!("{}-{}", conversation.id.as_str(), turn_index)),
                role: parse_role(&role_str),
                content: text,
                tool_calls,
                model: normalize_model(model),
                timestamp: None,
                artifacts,
            });
            turn_index += 1;
        }

        insert_str(
            &mut conversation.metadata,
            "cursor_session_id",
            session
                .source_path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(String::from),
        );
        finalize_session_metadata(&mut conversation);

        if let Some(last) = conversation.turns.last() {
            if let Some(ts) = last.timestamp {
                conversation.ended_at = Some(ts);
            }
        }

        Ok(conversation)
    }
}

fn parse_role(s: &str) -> Role {
    match s.to_lowercase().as_str() {
        "assistant" => Role::Assistant,
        "system" => Role::System,
        "tool" => Role::Tool,
        _ => Role::User,
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lineage_agent::{AgentSource, SessionReader};
    use lineage_core::ResolveStrategy;

    #[test]
    fn discovers_nested_cursor_transcripts() {
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/cursor-history");
        let adapter = CursorAdapter::new(&fixture);
        let sessions = adapter.discover().unwrap();
        assert!(!sessions.is_empty(), "expected cursor sessions in fixture");
        let conv = adapter.read(&sessions[0]).unwrap();
        assert_eq!(conv.agent, AgentKind::Cursor);
        assert!(!conv.turns.is_empty());
        assert!(conv.turns.iter().any(|t| !t.tool_calls.is_empty()));
        assert!(conv.turns.iter().any(|t| {
            t.artifacts
                .iter()
                .any(|a| a.resolve.as_ref().map(|r| r.strategy) == Some(ResolveStrategy::OldString))
        }));
        assert_eq!(
            conv.metadata
                .get("cursor_session_id")
                .and_then(|v| v.as_str()),
            Some("cursor-sess-001")
        );
    }
}
