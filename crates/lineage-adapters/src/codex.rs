use std::collections::HashMap;
use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use lineage_agent::{
    no_vendor_session_id, transcript_writing_unsupported, AgentSource, RenderedTranscript,
    ResumeInvocation, SessionReader, SessionRef, SessionResumer, TranscriptWriter,
};
use lineage_core::{
    derive_session_id, AgentKind, Artifact, Conversation, LineageError, LineageId, Role, ToolCall,
    Turn, CONVERSATION_SCHEMA,
};
use serde_json::Value;
use walkdir::WalkDir;

use crate::citations::enrich_turn_with_citations;
use crate::content::{
    artifacts_from_tool_input, enrich_turn_with_images, extract_text_content, tool_target,
};
use crate::metadata::{finalize_session_metadata, insert_str, normalize_model, vendor_session_id};
use crate::path_util::paths_match_workspace;

/// The metadata key this adapter records Codex's own session id under. Named so
/// the import that writes it and the resume that reads it cannot drift apart.
const SESSION_ID_KEY: &str = "codex_session_id";

pub struct CodexAdapter {
    workspace_root: PathBuf,
}

impl CodexAdapter {
    pub fn new(workspace_root: impl AsRef<Path>) -> Self {
        Self {
            workspace_root: workspace_root.as_ref().to_path_buf(),
        }
    }

    fn session_roots(&self) -> Vec<PathBuf> {
        let mut roots = Vec::new();
        let local = self.workspace_root.join(".codex").join("sessions");
        if local.exists() {
            roots.push(local);
        }
        if let Some(home) = home_dir() {
            let global = home.join(".codex").join("sessions");
            if global.exists() {
                roots.push(global);
            }
        }
        roots
    }
}

impl AgentSource for CodexAdapter {
    fn agent(&self) -> AgentKind {
        AgentKind::Codex
    }

    fn discover(&self) -> Result<Vec<SessionRef>, LineageError> {
        let mut sessions = Vec::new();
        for root in self.session_roots() {
            for entry in WalkDir::new(&root).into_iter().flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if !name.starts_with("rollout-") || !name.ends_with(".jsonl") {
                    continue;
                }
                if !is_codex_rollout(path)? {
                    continue;
                }
                let meta = parse_session_meta(path)?;
                let in_workspace = path.starts_with(&self.workspace_root);
                if !in_workspace {
                    if let Some(ref cwd) = meta.cwd {
                        if !paths_match_workspace(cwd, &self.workspace_root) {
                            continue;
                        }
                    } else {
                        continue;
                    }
                }
                sessions.push(SessionRef {
                    id_hint: meta.session_id.clone().unwrap_or_else(|| name.to_string()),
                    agent: AgentKind::Codex,
                    source_path: path.to_path_buf(),
                    started_at: meta.started_at,
                });
            }
        }
        Ok(sessions)
    }
}

impl SessionReader for CodexAdapter {
    fn read(&self, session: &SessionRef) -> Result<Conversation, LineageError> {
        let content = fs::read_to_string(&session.source_path)
            .map_err(|e| LineageError::Other(e.to_string()))?;

        let meta = parse_session_meta(&session.source_path)?;
        let started_at = meta
            .started_at
            .or(session.started_at)
            .unwrap_or_else(Utc::now);

        // Codex states its session id in the rollout's `session_meta` line;
        // where that is missing the filename stem carries the same id, so the
        // fallback is the stem rather than anything time-derived.
        let token = meta
            .session_id
            .as_deref()
            .unwrap_or_else(|| session.session_token());
        let id = derive_session_id(AgentKind::Codex, token);

        let parsed = parse_rollout_lines(&content, Some(self.workspace_root.as_path()))?;
        let mut conversation = Conversation {
            schema_version: CONVERSATION_SCHEMA.into(),
            id,
            agent: AgentKind::Codex,
            started_at,
            ended_at: parsed.ended_at,
            workspace_root: meta
                .cwd
                .clone()
                .unwrap_or_else(|| self.workspace_root.display().to_string()),
            parent_session_id: None,
            fork_origin: None,
            pull_origin: None,
            private: false,
            turns: parsed.turns,
            commit_shas: Vec::new(),
            metadata: serde_json::Map::from_iter([
                (
                    "source".into(),
                    Value::String(session.source_path.display().to_string()),
                ),
                (
                    SESSION_ID_KEY.into(),
                    Value::String(meta.session_id.unwrap_or_default()),
                ),
            ])
            .into_iter()
            .collect(),
        };

        insert_str(&mut conversation.metadata, "model", meta.model);
        insert_str(
            &mut conversation.metadata,
            "codex_originator",
            meta.originator,
        );
        insert_str(
            &mut conversation.metadata,
            "codex_cli_version",
            meta.cli_version,
        );
        finalize_session_metadata(&mut conversation);

        Ok(conversation)
    }
}

#[derive(Debug, Default)]
struct SessionMeta {
    session_id: Option<String>,
    cwd: Option<String>,
    model: Option<String>,
    originator: Option<String>,
    cli_version: Option<String>,
    started_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Default)]
struct ParsedRollout {
    turns: Vec<Turn>,
    ended_at: Option<DateTime<Utc>>,
}

fn is_codex_rollout(path: &Path) -> Result<bool, LineageError> {
    let file = fs::File::open(path).map_err(|e| LineageError::Other(e.to_string()))?;
    let reader = std::io::BufReader::new(file);
    let first = reader
        .lines()
        .next()
        .transpose()
        .map_err(|e| LineageError::Other(e.to_string()))?;
    let Some(line) = first else {
        return Ok(false);
    };
    let v: Value = serde_json::from_str(&line).map_err(LineageError::Serde)?;
    if event_type(&v).as_deref() != Some("session_meta") {
        return Ok(false);
    }
    let body = event_body(&v);
    let originator = pick_str(body, &["originator"]).unwrap_or_default();
    Ok(originator.contains("codex") || body.get("originator").is_some())
}

fn parse_session_meta(path: &Path) -> Result<SessionMeta, LineageError> {
    let content = fs::read_to_string(path).map_err(|e| LineageError::Other(e.to_string()))?;
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(line).map_err(LineageError::Serde)?;
        if event_type(&v).as_deref() == Some("session_meta") {
            return Ok(extract_session_meta(&v));
        }
        break;
    }
    Ok(SessionMeta::default())
}

fn extract_session_meta(v: &Value) -> SessionMeta {
    let body = event_body(v);
    SessionMeta {
        session_id: pick_str(body, &["session_id", "id"]),
        cwd: pick_str(body, &["cwd"]),
        model: normalize_model(pick_str(body, &["model"])),
        originator: pick_str(body, &["originator"]),
        cli_version: pick_str(body, &["cli_version"]),
        started_at: top_timestamp(v).or_else(|| {
            pick_str(body, &["timestamp", "started_at"]).and_then(|s| parse_timestamp(&s))
        }),
    }
}

fn parse_rollout_lines(
    content: &str,
    workspace_root: Option<&std::path::Path>,
) -> Result<ParsedRollout, LineageError> {
    let mut turns = Vec::new();
    let mut tool_results: HashMap<String, String> = HashMap::new();
    let mut ended_at = None;
    let mut turn_index = 0usize;

    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let event = event_type(&v).unwrap_or_default();
        let body = event_body(&v);
        let ts = top_timestamp(&v)
            .or_else(|| pick_str(body, &["timestamp"]).and_then(|s| parse_timestamp(&s)));

        if let Some(t) = ts {
            ended_at = Some(t);
        }

        match event.as_str() {
            "session_meta" => continue,
            "user_message" | "user" => {
                let content = extract_message_content(body);
                if !content.is_empty() {
                    turns.push(make_turn(
                        turn_index,
                        Role::User,
                        content,
                        ts,
                        vec![],
                        vec![],
                    ));
                    turn_index += 1;
                }
            }
            "assistant_message" | "assistant" | "response" => {
                let content = extract_message_content(body);
                let model = pick_str(body, &["model"]);
                if !content.is_empty() {
                    turns.push(make_turn(
                        turn_index,
                        Role::Assistant,
                        content,
                        ts,
                        vec![],
                        vec![],
                    ));
                    if let Some(model) = normalize_model(model) {
                        if let Some(last) = turns.last_mut() {
                            last.model = Some(model);
                        }
                    }
                    turn_index += 1;
                }
            }
            "tool_call" => {
                let call_id = pick_str(body, &["call_id", "id"])
                    .unwrap_or_else(|| format!("tc-{turn_index}"));
                let name = pick_str(body, &["tool_name", "name"]).unwrap_or_else(|| "tool".into());
                let args = body
                    .get("arguments")
                    .map(|a| a.to_string())
                    .unwrap_or_default();
                let artifacts =
                    artifacts_from_tool_input(&name, body.get("arguments"), workspace_root);
                let target = tool_target(&name, body.get("arguments"), workspace_root);
                turns.push(make_turn(
                    turn_index,
                    Role::Tool,
                    format!("{name}({args})"),
                    ts,
                    vec![ToolCall {
                        id: call_id,
                        name,
                        arguments: args,
                        result: None,
                        target,
                    }],
                    artifacts,
                ));
                turn_index += 1;
            }
            "tool_result" => {
                let call_id = pick_str(body, &["call_id", "id"]).unwrap_or_default();
                let result = pick_str(body, &["result", "output", "content"])
                    .unwrap_or_else(|| body.to_string());
                tool_results.insert(call_id, result);
            }
            "event_msg" => {
                let subtype = pick_str(body, &["type"]).unwrap_or_default();
                match subtype.as_str() {
                    "token_count" => continue,
                    "user_message" => {
                        let content = pick_str(body, &["message"]).unwrap_or_default();
                        if !content.is_empty() {
                            turns.push(make_turn(
                                turn_index,
                                Role::User,
                                content,
                                ts,
                                vec![],
                                vec![],
                            ));
                            turn_index += 1;
                        }
                    }
                    "agent_message" => {
                        let content = pick_str(body, &["message"]).unwrap_or_default();
                        if !content.is_empty() {
                            turns.push(make_turn(
                                turn_index,
                                Role::Assistant,
                                content,
                                ts,
                                vec![],
                                vec![],
                            ));
                            turn_index += 1;
                        }
                    }
                    _ => {
                        if let Some(msg) = body.get("message") {
                            let role =
                                pick_str(msg, &["role"]).unwrap_or_else(|| "assistant".into());
                            let content = extract_message_content(msg);
                            if !content.is_empty() {
                                let role = match role.as_str() {
                                    "user" => Role::User,
                                    "system" => Role::System,
                                    "tool" => Role::Tool,
                                    _ => Role::Assistant,
                                };
                                turns.push(make_turn(
                                    turn_index,
                                    role,
                                    content,
                                    ts,
                                    vec![],
                                    vec![],
                                ));
                                turn_index += 1;
                            }
                        }
                    }
                }
            }
            "response_item" => {
                let item_type = pick_str(body, &["type"]).unwrap_or_default();
                match item_type.as_str() {
                    "message" => {
                        let role = pick_str(body, &["role"]).unwrap_or_default();
                        let content =
                            extract_text_content(body.get("content").unwrap_or(&Value::Null));
                        if content.contains("<environment_context>") || role == "developer" {
                            continue;
                        }
                        if !content.is_empty() {
                            let role = match role.as_str() {
                                "user" => Role::User,
                                "assistant" => Role::Assistant,
                                "system" => Role::System,
                                _ => Role::User,
                            };
                            turns.push(make_turn(turn_index, role, content, ts, vec![], vec![]));
                            turn_index += 1;
                        }
                    }
                    "function_call" => {
                        let call_id = pick_str(body, &["call_id", "id"])
                            .unwrap_or_else(|| format!("tc-{turn_index}"));
                        let name = pick_str(body, &["name"]).unwrap_or_else(|| "tool".into());
                        let args = body
                            .get("arguments")
                            .map(|a| a.to_string())
                            .unwrap_or_default();
                        let parsed_args = body.get("arguments").and_then(parse_tool_arguments);
                        let artifacts =
                            artifacts_from_tool_input(&name, parsed_args.as_ref(), workspace_root);
                        let target = tool_target(&name, parsed_args.as_ref(), workspace_root);
                        turns.push(make_turn(
                            turn_index,
                            Role::Tool,
                            format!("{name}({args})"),
                            ts,
                            vec![ToolCall {
                                id: call_id,
                                name,
                                arguments: args,
                                result: None,
                                target,
                            }],
                            artifacts,
                        ));
                        turn_index += 1;
                    }
                    "function_call_output" => {
                        let call_id = pick_str(body, &["call_id", "id"]).unwrap_or_default();
                        let result = pick_str(body, &["output", "content"])
                            .unwrap_or_else(|| body.to_string());
                        tool_results.insert(call_id, result);
                    }
                    _ => {}
                }
            }
            _ => {
                if let Some(role) = pick_str(body, &["role"]) {
                    let content = extract_message_content(body);
                    if !content.is_empty() && !content.contains("<environment_context>") {
                        let role = match role.as_str() {
                            "user" => Role::User,
                            "assistant" => Role::Assistant,
                            "system" => Role::System,
                            "tool" => Role::Tool,
                            _ => Role::User,
                        };
                        turns.push(make_turn(turn_index, role, content, ts, vec![], vec![]));
                        turn_index += 1;
                    }
                }
            }
        }
    }

    for turn in &mut turns {
        for tc in &mut turn.tool_calls {
            if let Some(result) = tool_results.get(&tc.id) {
                tc.result = Some(result.chars().take(4000).collect());
            }
        }
        enrich_turn_with_citations(&turn.content, &mut turn.artifacts, workspace_root);
        enrich_turn_with_images(&turn.content, &mut turn.artifacts);
    }

    Ok(ParsedRollout { turns, ended_at })
}

fn make_turn(
    index: usize,
    role: Role,
    content: String,
    timestamp: Option<DateTime<Utc>>,
    tool_calls: Vec<ToolCall>,
    artifacts: Vec<Artifact>,
) -> Turn {
    Turn {
        id: LineageId::from(format!("codex-turn-{index}")),
        role,
        content,
        tool_calls,
        model: None,
        timestamp,
        artifacts,
    }
}

fn event_type(v: &Value) -> Option<String> {
    v.get("type").and_then(|t| t.as_str()).map(String::from)
}

fn event_body(v: &Value) -> &Value {
    if let Some(item) = v.get("item") {
        return item;
    }
    if let Some(payload) = v.get("payload") {
        return payload;
    }
    v
}

fn top_timestamp(v: &Value) -> Option<DateTime<Utc>> {
    v.get("timestamp")
        .and_then(|t| t.as_str())
        .and_then(parse_timestamp)
}

fn pick_str(v: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
            return Some(s.to_string());
        }
    }
    None
}

fn extract_message_content(v: &Value) -> String {
    if let Some(s) = v.get("content").and_then(|c| c.as_str()) {
        return s.to_string();
    }
    extract_text_content(v.get("content").unwrap_or(&Value::Null))
}

fn parse_tool_arguments(v: &Value) -> Option<Value> {
    if v.is_object() {
        return Some(v.clone());
    }
    if let Some(s) = v.as_str() {
        return serde_json::from_str(s).ok();
    }
    None
}

fn parse_timestamp(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

impl TranscriptWriter for CodexAdapter {
    /// `codex fork` very likely accepts a hand-written rollout file, but that
    /// has not been executed against a real Codex install, and its unified
    /// backend may validate ids server-side. Declining until it is proven beats
    /// writing a file that silently fails to resume.
    fn render_transcript(
        &self,
        _conversation: &Conversation,
    ) -> Result<RenderedTranscript, LineageError> {
        Err(transcript_writing_unsupported(AgentKind::Codex))
    }
}

/// Codex can reopen a session it already holds even though it cannot be handed a
/// written one — which is why resuming and transcript writing are separate
/// capabilities rather than one "can be continued" flag.
impl SessionResumer for CodexAdapter {
    fn resume_invocation(
        &self,
        conversation: &Conversation,
    ) -> Result<ResumeInvocation, LineageError> {
        let session_id = vendor_session_id(conversation, SESSION_ID_KEY)
            .ok_or_else(|| no_vendor_session_id(AgentKind::Codex))?;
        Ok(ResumeInvocation {
            command: format!("codex resume {session_id}"),
            // Codex keys its rollout files by id under a single state directory,
            // so the id resolves from wherever the user happens to be.
            cwd: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lineage_agent::{AgentSource, SessionReader};

    #[test]
    fn parses_legacy_rollout_fixture() {
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/codex-history");
        let adapter = CodexAdapter::new(&fixture);
        let sessions = adapter.discover().unwrap();
        assert!(!sessions.is_empty(), "expected codex sessions in fixture");
        let conv = adapter.read(&sessions[0]).unwrap();
        assert_eq!(conv.agent, AgentKind::Codex);
        assert!(!conv.turns.is_empty());
        assert!(conv.turns.iter().any(|t| matches!(t.role, Role::User)));
        assert!(conv
            .turns
            .iter()
            .any(|t| !t.tool_calls.is_empty() || !t.artifacts.is_empty()));
    }

    #[test]
    fn captures_codex_session_metadata() {
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/codex-history");
        let adapter = CodexAdapter::new(&fixture);
        let sessions = adapter.discover().unwrap();
        let legacy = sessions
            .iter()
            .find(|s| s.source_path.to_string_lossy().contains("rollout-sample"))
            .expect("legacy rollout fixture");
        let conv = adapter.read(legacy).unwrap();
        assert_eq!(
            conv.metadata.get("model").and_then(|v| v.as_str()),
            Some("gpt-5-codex")
        );
        assert_eq!(
            conv.metadata
                .get("codex_cli_version")
                .and_then(|v| v.as_str()),
            Some("0.34.0")
        );
    }

    #[test]
    fn parses_modern_rollout_fixture() {
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/codex-history");
        let adapter = CodexAdapter::new(&fixture);
        let sessions = adapter
            .discover()
            .unwrap()
            .into_iter()
            .find(|s| s.source_path.to_string_lossy().contains("rollout-modern"))
            .expect("modern rollout fixture");
        let conv = adapter.read(&sessions).unwrap();
        assert!(conv.turns.iter().any(|t| t.content == "hi"));
    }
}
