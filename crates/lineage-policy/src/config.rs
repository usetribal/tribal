use lineage_core::LineageRepoConfig;

use crate::rules::{ExcludeKind, ExcludePattern};
use crate::PolicyConfig;

pub fn policy_from_repo_config(repo: &LineageRepoConfig) -> PolicyConfig {
    let mut exclude_patterns: Vec<ExcludePattern> = ExcludePattern::default_paths();
    for pattern in &repo.exclude_paths {
        exclude_patterns.push(ExcludePattern {
            pattern: pattern.clone(),
            kind: ExcludeKind::Path,
        });
    }
    for pattern in &repo.exclude_content_patterns {
        exclude_patterns.push(ExcludePattern {
            pattern: pattern.clone(),
            kind: ExcludeKind::Content,
        });
    }

    PolicyConfig {
        redaction_rules: Vec::new(),
        exclude_patterns,
        strip_private: repo.strip_private_on_export,
    }
}

pub fn is_private_session(source_path: &str, config: &LineageRepoConfig) -> bool {
    let basename = source_path
        .rsplit('/')
        .next()
        .or_else(|| source_path.rsplit('\\').next())
        .unwrap_or(source_path);
    config
        .private_session_patterns
        .iter()
        .any(|p| glob_match_simple(p, basename))
}

fn glob_match_simple(pattern: &str, path: &str) -> bool {
    if let Some(inner) = pattern.strip_prefix('*').and_then(|s| s.strip_suffix('*')) {
        return path.contains(inner);
    }
    if let Some(suffix) = pattern.strip_prefix('*') {
        return path.ends_with(suffix);
    }
    path.contains(pattern)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lineage_core::LineageRepoConfig;

    #[test]
    fn private_session_pattern_matches() {
        let config = LineageRepoConfig::default();
        assert!(is_private_session(
            "/home/user/.cursor/agent-transcripts/private-chat.jsonl",
            &config
        ));
        assert!(!is_private_session(
            "/home/user/.cursor/agent-transcripts/normal.jsonl",
            &config
        ));
    }

    #[test]
    fn private_session_pattern_ignores_macos_private_var_prefix() {
        let config = LineageRepoConfig::default();
        assert!(!is_private_session(
            "/private/var/folders/T/tmp/.cursor/agent-transcripts/session-001.jsonl",
            &config
        ));
    }

    #[test]
    fn repo_config_maps_to_policy() {
        let mut config = LineageRepoConfig::default();
        config.exclude_paths.push("secrets/".into());
        let policy = policy_from_repo_config(&config);
        assert!(policy
            .exclude_patterns
            .iter()
            .any(|p| p.pattern == "secrets/"));
    }
}
