use std::fs;
use std::path::{Path, PathBuf};

/// Claude Code encodes the launch directory as a path prefix:
/// `/Users/dev/my-project` -> `-Users-dev-my-project`
pub fn claude_project_key(workspace: &Path) -> String {
    let path = fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());
    path.display().to_string().replace('/', "-")
}

/// Cursor encodes the workspace path under `~/.cursor/projects/`:
/// `/Users/dev/my-project` -> `Users-dev-my-project`
pub fn cursor_project_key(workspace: &Path) -> String {
    let path = fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());
    let mut s = path.display().to_string();
    if s.starts_with('/') {
        s = s[1..].to_string();
    }
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

pub fn claude_project_dir(home: &Path, workspace: &Path) -> PathBuf {
    home.join(".claude")
        .join("projects")
        .join(claude_project_key(workspace))
}

pub fn cursor_transcripts_dir(home: &Path, workspace: &Path) -> PathBuf {
    home.join(".cursor")
        .join("projects")
        .join(cursor_project_key(workspace))
        .join("agent-transcripts")
}

/// True when the workspace (repo root) sits strictly *under* the session's
/// recorded cwd — a parent-workspace session (e.g. cwd `~/src`, repo
/// `~/src/repo`). Such sessions can belong to this repo if they touched it.
pub fn workspace_is_under_cwd(stored_cwd: &str, workspace: &Path) -> bool {
    let workspace_canonical =
        fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());
    let Ok(cwd) = fs::canonicalize(stored_cwd) else {
        return false;
    };
    workspace_canonical != cwd && workspace_canonical.starts_with(&cwd)
}

pub fn paths_match_workspace(stored_cwd: &str, workspace: &Path) -> bool {
    if stored_cwd == "." || stored_cwd == "./" {
        return true;
    }

    let workspace_canonical =
        fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());

    if let Ok(cwd) = fs::canonicalize(stored_cwd) {
        return cwd == workspace_canonical;
    }

    if let Ok(relative) = workspace_canonical.join(stored_cwd).canonicalize() {
        return relative == workspace_canonical;
    }

    let workspace_str = workspace_canonical.display().to_string();
    stored_cwd == workspace_str || stored_cwd.starts_with(&workspace_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_key_replaces_slashes() {
        let key = claude_project_key(Path::new("/Users/dev/proj"));
        assert_eq!(key, "-Users-dev-proj");
    }

    #[test]
    fn cursor_key_strips_leading_slash() {
        let key = cursor_project_key(Path::new("/Users/dev/proj"));
        assert!(key.starts_with("Users"));
        assert!(!key.starts_with('-'));
    }
}
