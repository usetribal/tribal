use lineage_core::{Artifact, Conversation, Turn};

use crate::rules::{ExcludeKind, ExcludePattern, RedactionRule};

#[derive(Debug, Clone, Default)]
pub struct PolicyConfig {
    pub redaction_rules: Vec<RedactionRule>,
    pub exclude_patterns: Vec<ExcludePattern>,
    pub strip_private: bool,
}

impl PolicyConfig {
    pub fn default_safe() -> Self {
        Self {
            redaction_rules: Vec::new(),
            exclude_patterns: ExcludePattern::default_paths(),
            strip_private: true,
        }
    }
}

#[derive(Debug)]
pub struct PolicyResult {
    pub conversation: Conversation,
    pub redactions_applied: usize,
    pub artifacts_removed: usize,
}

pub fn apply_policy(config: &PolicyConfig, mut conversation: Conversation) -> PolicyResult {
    let mut redactions = 0usize;
    let mut artifacts_removed = 0usize;

    if should_exclude_session(config, &conversation) {
        conversation.private = true;
    }

    if config.strip_private && conversation.private {
        conversation.turns.clear();
        return PolicyResult {
            conversation,
            redactions_applied: 0,
            artifacts_removed: 0,
        };
    }

    for turn in &mut conversation.turns {
        if should_exclude_turn_content(config, turn) {
            turn.content.clear();
            turn.tool_calls.clear();
            turn.artifacts.clear();
            continue;
        }

        let before = turn.content.len();
        turn.content = redact_text(config, &turn.content);
        if turn.content.len() != before {
            redactions += 1;
        }

        for tc in &mut turn.tool_calls {
            let b = tc.arguments.len();
            tc.arguments = redact_text(config, &tc.arguments);
            if let Some(ref mut r) = tc.result {
                *r = redact_text(config, r);
            }
            if tc.arguments.len() != b {
                redactions += 1;
            }
        }

        let original_len = turn.artifacts.len();
        turn.artifacts
            .retain(|a| !should_exclude_artifact(config, a));
        artifacts_removed += original_len - turn.artifacts.len();
    }

    PolicyResult {
        conversation,
        redactions_applied: redactions,
        artifacts_removed,
    }
}

pub fn prepare_for_export(config: &PolicyConfig, mut conversation: Conversation) -> Conversation {
    if config.strip_private && conversation.private {
        conversation.turns.clear();
        return conversation;
    }
    apply_policy(config, conversation).conversation
}

fn should_exclude_session(config: &PolicyConfig, conversation: &Conversation) -> bool {
    if conversation.private {
        return true;
    }
    let source = conversation
        .metadata
        .get("source")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    config
        .exclude_patterns
        .iter()
        .any(|p| matches!(p.kind, ExcludeKind::Session) && glob_match(&p.pattern, source))
}

fn should_exclude_turn_content(config: &PolicyConfig, turn: &Turn) -> bool {
    config.exclude_patterns.iter().any(|p| {
        if !matches!(p.kind, ExcludeKind::Content) {
            return false;
        }
        if let Some(re) = p.compile_regex() {
            return re.is_match(&turn.content);
        }
        turn.content.contains(&p.pattern)
    })
}

fn redact_text(config: &PolicyConfig, text: &str) -> String {
    let mut out = crate::gitleaks::redact_text(text);
    for rule in &config.redaction_rules {
        if let Some(re) = rule.compile() {
            out = re.replace_all(&out, rule.replacement.as_str()).to_string();
        }
    }
    out
}

fn should_exclude_artifact(config: &PolicyConfig, artifact: &Artifact) -> bool {
    config
        .exclude_patterns
        .iter()
        .any(|p| matches!(p.kind, ExcludeKind::Path) && p.matches_path(&artifact.path))
}

fn glob_match(pattern: &str, path: &str) -> bool {
    if let Some(inner) = pattern.strip_prefix('*').and_then(|s| s.strip_suffix('*')) {
        return !inner.is_empty() && path.contains(inner);
    }
    if let Some(suffix) = pattern.strip_prefix('*') {
        return path.ends_with(suffix) || path.contains(suffix);
    }
    path.contains(pattern)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lineage_core::{AgentKind, Role};

    #[test]
    fn redacts_detected_secrets() {
        let config = PolicyConfig::default_safe();
        let mut c = Conversation::new(AgentKind::Cursor, "/tmp");
        c.turns.push(lineage_core::Turn {
            id: lineage_core::LineageId::new(),
            role: Role::User,
            content: "export STRIPE_KEY=sk_test_abcdefghijklmnopqrstuvwxyz".into(),
            tool_calls: vec![],
            model: None,
            timestamp: None,
            artifacts: vec![],
        });
        let result = apply_policy(&config, c);
        assert!(result.conversation.turns[0].content.contains("[REDACTED]"));
        assert!(result.redactions_applied > 0);
    }

    #[test]
    fn leaves_innocent_prose_unredacted() {
        let config = PolicyConfig::default_safe();
        let mut c = Conversation::new(AgentKind::Cursor, "/tmp");
        let prose = "The password: field in the schema is optional.".to_string();
        c.turns.push(lineage_core::Turn {
            id: lineage_core::LineageId::new(),
            role: Role::Assistant,
            content: prose.clone(),
            tool_calls: vec![],
            model: None,
            timestamp: None,
            artifacts: vec![],
        });
        let result = apply_policy(&config, c);
        assert_eq!(result.conversation.turns[0].content, prose);
        assert_eq!(result.redactions_applied, 0);
    }

    #[test]
    fn strips_private_sessions() {
        let config = PolicyConfig::default_safe();
        let mut c = Conversation::new(AgentKind::Cursor, "/tmp");
        c.private = true;
        c.turns.push(lineage_core::Turn {
            id: lineage_core::LineageId::new(),
            role: Role::User,
            content: "secret".into(),
            tool_calls: vec![],
            model: None,
            timestamp: None,
            artifacts: vec![],
        });
        let result = apply_policy(&config, c);
        assert!(result.conversation.turns.is_empty());
    }
}
