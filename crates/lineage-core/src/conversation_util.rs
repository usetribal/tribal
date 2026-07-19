use crate::{ArtifactKind, Conversation, Role, Turn};

const CODE_EDIT_TOOLS: &[&str] = &[
    "edit",
    "edit_file",
    "write",
    "write_file",
    "str_replace",
    "apply_patch",
    "search_replace",
    "multiedit",
    "create",
    "patch",
];

/// True when the conversation contains evidence of workspace file modifications.
pub fn conversation_modified_code(conv: &Conversation) -> bool {
    for turn in &conv.turns {
        if turn_modified_code(turn) {
            return true;
        }
    }
    false
}

pub fn turn_modified_code(turn: &Turn) -> bool {
    for artifact in &turn.artifacts {
        if matches!(artifact.kind, ArtifactKind::FileEdit | ArtifactKind::Diff) {
            return true;
        }
    }
    for tc in &turn.tool_calls {
        let name = tc.name.to_lowercase();
        if CODE_EDIT_TOOLS.iter().any(|t| name.contains(t)) {
            return true;
        }
    }
    false
}

/// Paths the conversation *wrote* (edit/diff artifacts only) — the authorship
/// signal, deliberately excluding tool-call reads so consumers like link
/// gating are not polluted by files the session merely consulted.
pub fn files_written(conv: &Conversation) -> Vec<String> {
    let mut paths: Vec<String> = conv
        .turns
        .iter()
        .flat_map(|t| &t.artifacts)
        .filter(|a| {
            matches!(a.kind, ArtifactKind::FileEdit | ArtifactKind::Diff) && !a.path.is_empty()
        })
        .map(|a| a.path.clone())
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

/// Repo-relative or logical paths touched by code-changing artifacts and tools.
pub fn files_touched(conv: &Conversation) -> Vec<String> {
    let mut paths = Vec::new();
    for turn in &conv.turns {
        for artifact in &turn.artifacts {
            if matches!(
                artifact.kind,
                ArtifactKind::FileEdit | ArtifactKind::Diff | ArtifactKind::TerminalCommand
            ) && !artifact.path.is_empty()
                && !artifact.path.starts_with("turn-")
            {
                paths.push(artifact.path.clone());
            }
        }
        for tc in &turn.tool_calls {
            if let Some(path) = extract_path_from_tool_args(&tc.arguments) {
                paths.push(path);
            }
        }
    }
    paths.sort();
    paths.dedup();
    paths
}

fn extract_path_from_tool_args(args: &str) -> Option<String> {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(args) {
        for key in ["path", "file_path", "file", "target_file"] {
            if let Some(p) = v.get(key).and_then(|x| x.as_str()) {
                if !p.is_empty() {
                    return Some(p.to_string());
                }
            }
        }
    }
    None
}

/// Heuristic architecture/decision summary from session content (no LLM).
pub fn generate_architecture_summary(conv: &Conversation) -> String {
    let title = conv
        .turns
        .iter()
        .find(|t| t.role == Role::User)
        .map(|t| truncate_line(&t.content, 200))
        .unwrap_or_else(|| "Agent session".into());

    let files = files_touched(conv);
    let file_line = if files.is_empty() {
        "Files: (none detected)".to_string()
    } else if files.len() <= 5 {
        format!("Files: {}", files.join(", "))
    } else {
        format!(
            "Files: {} (+{} more)",
            files[..3].join(", "),
            files.len() - 3
        )
    };

    let model = conv
        .primary_model()
        .map(|m| format!("Model: {m}"))
        .unwrap_or_default();

    let mut parts = vec![format!("{} ({})", conv.agent.as_str(), title), file_line];
    if !model.is_empty() {
        parts.push(model);
    }
    parts.join("\n")
}

fn truncate_line(s: &str, max: usize) -> String {
    let one_line: String = s.lines().next().unwrap_or(s).trim().to_string();
    if one_line.chars().count() <= max {
        one_line
    } else {
        format!("{}…", one_line.chars().take(max).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentKind, LineageId};

    #[test]
    fn detects_file_edit_artifact() {
        let mut c = Conversation::new(AgentKind::Cursor, "/tmp");
        c.turns.push(Turn {
            id: LineageId::new(),
            role: Role::Assistant,
            content: String::new(),
            tool_calls: vec![],
            model: None,
            timestamp: None,
            artifacts: vec![crate::Artifact {
                kind: ArtifactKind::FileEdit,
                path: "src/main.rs".into(),
                blob_ref: None,
                content_hash: None,
                mime_type: None,
                preview_data_url: None,
                line_range: None,
                resolve: None,
            }],
        });
        assert!(conversation_modified_code(&c));
        assert_eq!(files_touched(&c), vec!["src/main.rs"]);
    }

    #[test]
    fn detects_tool_call_edit() {
        let mut c = Conversation::new(AgentKind::Claude, "/tmp");
        c.turns.push(Turn {
            id: LineageId::new(),
            role: Role::Assistant,
            content: String::new(),
            tool_calls: vec![crate::ToolCall {
                id: "tc1".into(),
                name: "edit_file".into(),
                arguments: r#"{"path":"src/lib.rs"}"#.into(),
                result: None,
            }],
            model: None,
            timestamp: None,
            artifacts: vec![],
        });
        assert!(turn_modified_code(&c.turns[0]));
        assert_eq!(files_touched(&c), vec!["src/lib.rs"]);
    }

    #[test]
    fn architecture_summary_includes_title_and_files() {
        let mut c = Conversation::new(AgentKind::Cursor, "/tmp");
        c.turns.push(Turn {
            id: LineageId::new(),
            role: Role::User,
            content: "Add caching layer".into(),
            tool_calls: vec![],
            model: Some("gpt-4".into()),
            timestamp: None,
            artifacts: vec![],
        });
        let summary = generate_architecture_summary(&c);
        assert!(summary.contains("caching"));
        assert!(summary.contains("gpt-4"));
    }
}
