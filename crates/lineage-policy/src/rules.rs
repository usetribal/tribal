use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedactionRule {
    pub name: String,
    pub pattern: String,
    pub replacement: String,
}

impl RedactionRule {
    /// Optional extra rule for repo-specific policy. Built-in detection uses vendored gitleaks rules.
    pub fn api_key() -> Self {
        Self {
            name: "api_key".into(),
            pattern: r"(?i)(api[_-]?key|token|secret|password)\s*[:=]\s*\S+".into(),
            replacement: "[REDACTED]".into(),
        }
    }

    /// Optional extra rule for repo-specific policy. Built-in detection uses vendored gitleaks rules.
    pub fn env_file() -> Self {
        Self {
            name: "env_var".into(),
            pattern: r"(?i)^[A-Z_][A-Z0-9_]*=.+$".into(),
            replacement: "[REDACTED]".into(),
        }
    }

    pub fn compile(&self) -> Option<Regex> {
        Regex::new(&self.pattern).ok()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExcludePattern {
    pub pattern: String,
    pub kind: ExcludeKind,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExcludeKind {
    Path,
    Content,
    Session,
}

impl ExcludePattern {
    pub fn default_paths() -> Vec<Self> {
        [
            ".env",
            ".env.*",
            "*credentials*",
            "*.pem",
            "*.key",
            "id_rsa",
        ]
        .into_iter()
        .map(|p| Self {
            pattern: p.into(),
            kind: ExcludeKind::Path,
        })
        .collect()
    }

    pub fn matches_path(&self, path: &str) -> bool {
        if !matches!(self.kind, ExcludeKind::Path) {
            return false;
        }
        glob_match(&self.pattern, path)
    }

    pub fn compile_regex(&self) -> Option<Regex> {
        if matches!(self.kind, ExcludeKind::Content) {
            Regex::new(&self.pattern).ok()
        } else {
            None
        }
    }
}

fn glob_match(pattern: &str, path: &str) -> bool {
    if let Some(inner) = pattern.strip_prefix('*').and_then(|s| s.strip_suffix('*')) {
        return !inner.is_empty() && path.contains(inner);
    }
    if let Some(suffix) = pattern.strip_prefix('*') {
        return path.ends_with(suffix) || path.contains(suffix);
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return path.starts_with(prefix) || path.split('/').any(|seg| seg.starts_with(prefix));
    }
    path == pattern
        || path.ends_with(&format!("/{pattern}"))
        || path.split('/').any(|seg| seg == pattern)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excludes_env_paths() {
        let rules = ExcludePattern::default_paths();
        assert!(rules.iter().any(|r| r.matches_path(".env")));
        assert!(rules
            .iter()
            .any(|r| r.matches_path("secrets/credentials.json")));
    }
}
